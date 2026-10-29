#!/usr/bin/env bash
# Day-0 launch: one SOL-PERP market from the launch config, in eight timed,
# resumable steps. Reference / dry-run only; not a paid live service; M3 NO-GO.
#
#   scripts/launch_market.sh <config> local|devnet          # launch, or resume
#   scripts/launch_market.sh <config> mainnet --mainnet-i-understand
#   scripts/launch_market.sh stop <cluster>                 # stop bots + monitor
#
# Steps (docs/LAUNCH.md): 1 init-market, 2 oracle (resolve policy + AuthMark),
# 3 insurance, 4 backing, 5 lp (the off-chain matcher has no on-chain config;
# LP and test-trader portfolios with deposits), 6 handover (asset-0 roles to
# their holders, then marketauth to the multisig) run as `staging step`; then
# 7 start the bots (`staging bots`: pusher, matcher, backing, insurance,
# keeper) and the monitor, 8 health check (a clean crank cycle, a fill, the
# mark lag, sane monitor metrics, the on-chain role table).
#
# Run dir launch-runs/<cluster>/ (LAUNCH_RUN_DIR overrides; gitignored):
# state.json (addresses, completed steps), keys/, handover/ (mainnet),
# bots.log, monitor.log, pids/, timings.txt.
# Local expects a running validator at the config's RPC (scripts/day0_drill.sh
# starts one). Mainnet: a fresh launch key (keys/launch_authority.json) signs
# steps 1-5; the handover writes co-sign transactions and stops (exit 10);
# re-run after they land. Mainnet bots run from deploy/, not from here.
# Exit 0 launched; 10 waiting for co-signatures; else failed (re-run retries).
# Knobs: LAUNCH_RPC_URL (overrides the config's rpc), LAUNCH_SKIP_BUILD=1, LAUNCH_HEALTH_TIMEOUT (s, 180),
# LAUNCH_LAG_STRICT=1 (mark lag over target fails instead of warning),
# LAUNCH_LAG_SAMPLES (pushes measured before judging the lag, 5).

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/env.sh"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/pins.env"
die() { echo "ERROR: $*" >&2; exit 1; }
now() { python3 -c 'import time; print(f"{time.time():.3f}")'; }
since() { python3 -c "import sys, time; print(f'{time.time() - float(sys.argv[1]):.1f}')" "$1"; }
STAGING="$REPO_ROOT/staging/target/release/staging"
MONITOR="$REPO_ROOT/monitor/target/release/monitor"

stop_bots() {
  for f in "$RUN_DIR"/pids/*.pid; do
    [ -f "$f" ] || continue
    pid="$(cat "$f")"
    kill "$pid" 2>/dev/null && echo "stopped $(basename "$f" .pid) ($pid)"
    rm -f "$f"
  done
}
alive() { [ -f "$RUN_DIR/pids/$1.pid" ] && kill -0 "$(cat "$RUN_DIR/pids/$1.pid")" 2>/dev/null; }

if [ "${1:-}" = stop ]; then
  RUN_DIR="${LAUNCH_RUN_DIR:-$REPO_ROOT/launch-runs/${2:?usage: $0 stop <cluster>}}"
  stop_bots
  exit 0
fi

[ $# -ge 2 ] || die "usage: $0 <config> local|devnet|mainnet [--mainnet-i-understand] | $0 stop <cluster>"
CONFIG="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
CLUSTER="$2"
case "$CLUSTER" in local|devnet|mainnet) ;; *) die "cluster must be local, devnet or mainnet" ;; esac
RUN_DIR="${LAUNCH_RUN_DIR:-$REPO_ROOT/launch-runs/$CLUSTER}"
mkdir -p "$RUN_DIR/pids"
cfg() { python3 -I -c 'import sys, tomllib
c = tomllib.load(open(sys.argv[1], "rb"))
for k in sys.argv[2].split("."): c = c[k]
print(c)' "$CONFIG" "$1"; }

# ---- preflight: pins, program id, cluster guards
[ "$(cfg pins.wrapper_sha)" = "$PERCOLATOR_PROG_SHA" ] && [ "$(cfg pins.engine_rev)" = "$PERCOLATOR_ENGINE_REV" ] ||
  die "config pins differ from scripts/pins.env (the source of truth): update both, then scripts/fetch_upstream.sh"
RPC="${LAUNCH_RPC_URL:-$(cfg "cluster.$CLUSTER.rpc")}"  # a paid RPC stays out of the config
[ -n "$RPC" ] || die "[cluster.$CLUSTER] rpc is empty"
# Paid RPC URLs carry credentials (userinfo, path or query): print the host only.
RPC_SHOWN="$(printf '%s' "$RPC" | sed -E 's#^([a-z]+://)?([^@/?]*@)?([^/?]+).*#\1\3/...#')"
PROGRAM_ID="$(cfg "cluster.$CLUSTER.program_id")"
if [ -z "$PROGRAM_ID" ] && [ "$CLUSTER" = local ]; then
  PROGRAM_ID="$(solana-keygen pubkey "$PERCOLATOR_PROG_DIR/target/deploy/percolator_prog-keypair.json")"
fi
[ -n "$PROGRAM_ID" ] || die "[cluster.$CLUSTER] program_id is empty"
export PERCOLATOR_PROGRAM_ID="$PROGRAM_ID"
MAINNET_GENESIS="5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d"
genesis="$(solana genesis-hash -u "$RPC" 2>/dev/null)" || die "cannot reach $RPC_SHOWN"
if [ "$CLUSTER" = mainnet ]; then
  [ "${3:-}" = --mainnet-i-understand ] || die "mainnet needs --mainnet-i-understand"
  [ "$genesis" = "$MAINNET_GENESIS" ] || die "$RPC_SHOWN is not mainnet"
  export LAUNCH_MAINNET_I_UNDERSTAND=1
else
  [ "$genesis" != "$MAINNET_GENESIS" ] || die "$RPC_SHOWN serves mainnet; refusing $CLUSTER"
fi
solana program show -u "$RPC" "$PROGRAM_ID" >/dev/null 2>&1 || die "program $PROGRAM_ID is not deployed on $RPC_SHOWN"
echo "==> $CLUSTER: rpc $RPC_SHOWN, program $PROGRAM_ID, run dir $RUN_DIR"

if [ "${LAUNCH_SKIP_BUILD:-0}" != 1 ]; then
  echo "==> building staging + monitor"
  cargo build --release --quiet --manifest-path "$REPO_ROOT/staging/Cargo.toml"
  cargo build --release --quiet --manifest-path "$REPO_ROOT/monitor/Cargo.toml"
fi

T_START="$(now)"
timed() {
  local name="$1" t0 rc=0
  shift
  t0="$(now)"
  "$@" || rc=$?
  printf '%-16s %7ss  rc=%s\n' "$name" "$(since "$t0")" "$rc" | tee -a "$RUN_DIR/timings.txt"
  return "$rc"
}

# ---- steps 1-6: on-chain setup and handover, resumable (state.json)
n=0
for s in init-market oracle insurance backing lp handover; do
  n=$((n + 1))
  rc=0
  timed "$n $s" "$STAGING" step "$s" "$CONFIG" "$CLUSTER" "$RUN_DIR" || rc=$?
  if [ "$rc" = 10 ]; then
    echo "==> waiting for co-signatures: $RUN_DIR/handover/*.b58, in order ($RUN_DIR/handover/README.md)."
    echo "    After they land, resume with: $0 $1 $CLUSTER --mainnet-i-understand"
    exit 10
  fi
  [ "$rc" = 0 ] || die "step $s failed; fix the cause and re-run the same command (completed steps are skipped)"
done
MARKET="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["market"])' "$RUN_DIR/state.json")"

if [ "$CLUSTER" = mainnet ]; then
  echo "==> market $MARKET is set up and handed over. Next (docs/LAUNCH.md): retire the launch key,"
  echo "    start the bots and monitor on the host (deploy/), then the health check against it."
  exit 0
fi

# ---- step 7: bots + monitor (background, logs and pid files in the run dir)
LISTEN="$(cfg monitor.listen)"
touch "$RUN_DIR/bots.log"
FROM_LINE=$(($(wc -l < "$RUN_DIR/bots.log") + 1))
start_bots() {
  if alive bots; then echo "bots already running"; else
    nohup "$STAGING" bots "$CONFIG" "$CLUSTER" "$RUN_DIR" >> "$RUN_DIR/bots.log" 2>&1 &
    echo $! > "$RUN_DIR/pids/bots.pid"
  fi
  if alive monitor; then echo "monitor already running"; else
    nohup "$MONITOR" --program "$PROGRAM_ID" --market "$MARKET" --rpc "$RPC" \
      --keeper-log "$RUN_DIR/bots.log" --listen "$LISTEN" >> "$RUN_DIR/monitor.log" 2>&1 &
    echo $! > "$RUN_DIR/pids/monitor.pid"
  fi
}
timed "7 start-bots" start_bots

# ---- step 8: health check
health() {
  python3 -I - "$RUN_DIR/bots.log" "$FROM_LINE" "http://$LISTEN/snapshot.json" "$(cfg oracle.lag_target_ms)" \
    "${LAUNCH_HEALTH_TIMEOUT:-180}" "${LAUNCH_LAG_STRICT:-0}" "$RUN_DIR/pids/bots.pid" "${LAUNCH_LAG_SAMPLES:-5}" \
    "$MARKET" "$RUN_DIR/pids/monitor.pid" <<'PY' || return 1
import json, os, re, statistics, sys, time, urllib.request
log, first, url, target, timeout, strict, pidfile, samples, market, monpid = sys.argv[1:]
first, target, samples, deadline = int(first), int(target), int(samples), time.time() + int(timeout)
while True:
    lines = open(log, errors="replace").read().splitlines()[first - 1:]
    # A clean keeper cycle: cranks submitted, none failed. On a market the
    # matcher refreshes every tick they are mostly idle (EngineNonProgress).
    cycles = [dict(kv.split("=") for kv in l.split(" summary: ")[1].split()) for l in lines if re.match(r"cycle \d+ summary: ", l)]
    clean = [c for c in cycles if c["failed"] == "0" and c.get("unconfirmed", "0") == "0" and int(c["progressed"]) + int(c["idle"]) > 0]
    # The bots' own keeper verdict too (it also counts failed txs and RPC
    # errors inside cycles that progressed, which the summary line misses):
    # the latest tick's keeper result must not be a failure.
    verdicts = [m.group(1) for m in (re.search(r" keeper=(\S+)", l) for l in lines) if m and m.group(1) != "-"]
    crank = bool(clean) and bool(verdicts) and not verdicts[-1].startswith("FAIL")
    fill = any(re.search(r" trade=[+-]\d", l) for l in lines)
    lags = [json.loads(l)["ms"] for l in lines if l.startswith('{"event":"mark_lag"')]
    snap, why = None, []
    try:
        snap = json.load(urllib.request.urlopen(url, timeout=3))
        k = snap.get("keeper") or {}
        # Only this launch's monitor counts: the right market, a fresh snapshot.
        if snap.get("market") != market: why.append(f"monitor serves market {snap.get('market')}, not {market}")
        if time.time() - (snap.get("snapshot_unix_seconds") or 0) > 30: why.append("monitor snapshot stale")
        if snap.get("chain_read_ok") != 1: why.append("chain read failing")
        if snap.get("mode") != "live": why.append(f"market mode {snap.get('mode')}")
        if not k.get("log_ok"): why.append("keeper log not read")
        if (snap.get("accounts") or {}).get("discovered", 0) < 2: why.append("portfolios not discovered")
        if (snap.get("buckets") or {}).get("counts", {}).get("fresh", 0) < 2: why.append("backing buckets not fresh")
        if (snap.get("collateral") or {}).get("senior_shortfall_atoms", 0) not in (0, "0"): why.append("senior shortfall")
    except Exception as e:
        why.append(f"monitor: {e}")
    if crank and fill and len(lags) >= samples and not why:
        # Checked once more after success: both processes still up.
        alive = True
        for pf in (pidfile, monpid):
            try: os.kill(int(open(pf).read()), 0)
            except Exception: alive = False
        if alive: break
    for pf, name in ((pidfile, "bots"), (monpid, "monitor")):
        try:
            os.kill(int(open(pf).read()), 0)
        except Exception:
            sys.exit(f"{name} process exited; see {name}.log")
    if time.time() > deadline:
        sys.exit(f"health: crank={crank} fill={fill} lag_samples={len(lags)} monitor={why or 'ok'}")
    time.sleep(2)
p50 = statistics.median(lags)
landed = sum(int(c["progressed"]) for c in clean)
print(f"health: keeper cycle clean ({landed} landed, {sum(int(c['idle']) for c in clean)} idle), fill landed, monitor ok; mark lag p50 {p50:.0f} ms, max {max(lags)} ms over {len(lags)} pushes (target {target} ms)")
if p50 > target:
    msg = f"mark lag p50 {p50:.0f} ms over the {target} ms target (research/replay/REPORT.md)"
    if strict == "1":
        sys.exit("health: " + msg)
    print("WARN: " + msg)
PY
  "$STAGING" roles "$CONFIG" "$CLUSTER" "$RUN_DIR"
}
timed "8 health" health || die "health check failed; bots left running (stop: $0 stop $CLUSTER)"
printf '%-16s %7ss\n' "total" "$(since "$T_START")" | tee -a "$RUN_DIR/timings.txt"
echo "==> SOL-PERP live on $CLUSTER: market $MARKET, monitor http://$LISTEN/ ; stop: $0 stop $CLUSTER"
