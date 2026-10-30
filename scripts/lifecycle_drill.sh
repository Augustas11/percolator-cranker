#!/usr/bin/env bash
# Lifecycle drill: listing -> Live -> Recovery -> Resolved -> relisted,
# with payouts, portfolio/slab close and rent swept back to the payer.
# Reference / dry-run only; not a paid live service; M3 NO-GO.
#
#   scripts/lifecycle_drill.sh <config> local|devnet <drill dir>
#
# Local expects a running validator at the config's RPC, wrapper deployed.
# DRILL_PAYER seeds both payers; DRILL_WIND_DOWN_RELIST=0 leaves the relist live
# (bots stopped). Default 1 winds it down too. Logs and timings stay in the
# drill dir; on failure both runs' bots and monitors are stopped.

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/env.sh"
[ $# = 3 ] || { echo "usage: $0 <config> local|devnet <drill dir>" >&2; exit 1; }
CONFIG="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
CLUSTER="$2"
case "$CLUSTER" in local|devnet) ;; *) echo "lifecycle drill requires local or devnet" >&2; exit 1 ;; esac
mkdir -p "$3"
RUN_DIR="$(cd "$3" && pwd)"
STAGING="$REPO_ROOT/staging/target/release/staging"
cfg() { python3 -I -c 'import sys, tomllib
c = tomllib.load(open(sys.argv[1], "rb"))
for k in sys.argv[2].split("."): c = c[k]
print(c)' "$CONFIG" "$1"; }
RPC="${LAUNCH_RPC_URL:-$(cfg "cluster.$CLUSTER.rpc")}"
PROGRAM_ID="$(cfg "cluster.$CLUSTER.program_id")"
if [ -z "$PROGRAM_ID" ] && [ "$CLUSTER" = local ]; then
  PROGRAM_ID="$(solana-keygen pubkey "$PERCOLATOR_PROG_DIR/target/deploy/percolator_prog-keypair.json")"
fi
export PERCOLATOR_PROGRAM_ID="$PROGRAM_ID"
# The bots' first trade can precede the first push; retry soon, not in 300 s.
export STAGING_TRADE_EVERY_SECS="${STAGING_TRADE_EVERY_SECS:-30}"

cleanup() {
  LAUNCH_RUN_DIR="$RUN_DIR/listing" "$SCRIPT_DIR/launch_market.sh" stop "$CLUSTER" || true
  LAUNCH_RUN_DIR="$RUN_DIR/relist" "$SCRIPT_DIR/launch_market.sh" stop "$CLUSTER" || true
}
trap cleanup EXIT
for run in listing relist; do
  mkdir -p "$RUN_DIR/$run/keys"
  payer="$RUN_DIR/$run/keys/payer.json"
  if [ ! -f "$payer" ]; then
    if [ -n "${DRILL_PAYER:-}" ]; then
      cp "$DRILL_PAYER" "$payer"
    elif [ "$run" = relist ]; then
      cp "$RUN_DIR/listing/keys/payer.json" "$payer"
    else
      solana-keygen new --no-bip39-passphrase --silent --outfile "$payer"
    fi
    chmod 0600 "$payer"
  fi
done
LPAYER="$RUN_DIR/listing/keys/payer.json"
RPAYER="$RUN_DIR/relist/keys/payer.json"
LKEY="$(solana-keygen pubkey "$LPAYER")"
RKEY="$(solana-keygen pubkey "$RPAYER")"
balance() {
  local a b=0
  a="$(solana balance -u "$RPC" -k "$LPAYER" --lamports | awk '{print $1}')"
  if [ "$LKEY" != "$RKEY" ]; then
    b="$(solana balance -u "$RPC" -k "$RPAYER" --lamports | awk '{print $1}')"
  fi
  echo "$((a + b))"
}
now() { python3 -c 'import time; print(f"{time.time():.3f}")'; }
since() { python3 -c "import sys, time; print(f'{time.time() - float(sys.argv[1]):.1f}')" "$1"; }
timed() {
  local name="$1" t0 rc=0
  shift
  t0="$(now)"
  "$@" || rc=$?
  printf '%-16s %7ss  rc=%s\n' "$name" "$(since "$t0")" "$rc" | tee -a "$RUN_DIR/timings.txt"
  return "$rc"
}
BEFORE="$(balance)"
echo "payer before: $BEFORE lamports" | tee -a "$RUN_DIR/timings.txt"
timed listing env LAUNCH_RUN_DIR="$RUN_DIR/listing" "$SCRIPT_DIR/launch_market.sh" "$CONFIG" "$CLUSTER"
timed stop-bots env LAUNCH_RUN_DIR="$RUN_DIR/listing" "$SCRIPT_DIR/launch_market.sh" stop "$CLUSTER"
timed lifecycle "$STAGING" lifecycle "$CONFIG" "$CLUSTER" "$RUN_DIR/listing"
timed relist env LAUNCH_RUN_DIR="$RUN_DIR/relist" "$SCRIPT_DIR/launch_market.sh" "$CONFIG" "$CLUSTER"
timed stop-bots env LAUNCH_RUN_DIR="$RUN_DIR/relist" "$SCRIPT_DIR/launch_market.sh" stop "$CLUSTER"
if [ "${DRILL_WIND_DOWN_RELIST:-1}" = 1 ]; then
  timed lifecycle "$STAGING" lifecycle "$CONFIG" "$CLUSTER" "$RUN_DIR/relist"
fi
AFTER="$(balance)"
echo "payer after: $AFTER lamports; spent: $((BEFORE - AFTER)) lamports (net of local airdrops)" | tee -a "$RUN_DIR/timings.txt"
