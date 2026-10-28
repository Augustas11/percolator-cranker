#!/usr/bin/env bash
# Day-0 drill, local only: a fresh validator, the wrapper deployed to a FRESH
# program id ("upstream publishes a program ID"), that id written into a temp
# copy of the config, then scripts/launch_market.sh on local, timed per step
# and in total, then everything stopped.
# Reference / dry-run only; not a paid live service; M3 NO-GO.
#
#   scripts/day0_drill.sh [config]        # default launch/sol-perp.toml
#
# Run dir launch-runs/drill-<timestamp>/ (DRILL_RUN_DIR overrides; gitignored).
# DRILL_RPC_PORT (8699) keeps it apart from staging (8799) and M1 (8899).
# DRILL_SKIP_SBF_BUILD=1 reuses the existing .so (the nightly builds it first).
# Exit: launch_market.sh's.

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/env.sh"
CONFIG="${1:-$REPO_ROOT/launch/sol-perp.toml}"
PORT="${DRILL_RPC_PORT:-8699}"
RUN_DIR="${DRILL_RUN_DIR:-$REPO_ROOT/launch-runs/drill-$(date +%Y%m%d-%H%M%S)}"
RPC="http://127.0.0.1:$PORT"
SO="$PERCOLATOR_PROG_DIR/target/deploy/percolator_prog.so"
mkdir -p "$RUN_DIR/keys"

if [ "${DRILL_SKIP_SBF_BUILD:-0}" != 1 ]; then
  echo "==> building percolator_prog.so"
  ( cd "$PERCOLATOR_PROG_DIR" && cargo build-sbf --tools-version v1.52 )
fi
echo "==> building staging + monitor"
cargo build --release --quiet --manifest-path "$REPO_ROOT/staging/Cargo.toml"
cargo build --release --quiet --manifest-path "$REPO_ROOT/monitor/Cargo.toml"

PAYER="$RUN_DIR/keys/payer.json"
solana-keygen new --no-bip39-passphrase --silent --force --outfile "$PAYER"
echo "==> validator on $RPC (run dir $RUN_DIR)"
solana-test-validator --reset --quiet \
  --ledger "$RUN_DIR/ledger" --limit-ledger-size 50000000 \
  --bind-address 127.0.0.1 --rpc-port "$PORT" --faucet-port $((PORT + 1000)) \
  --gossip-port $((PORT + 1010)) --dynamic-port-range "$((PORT + 1020))-$((PORT + 1120))" \
  --mint "$(solana-keygen pubkey "$PAYER")" > "$RUN_DIR/validator.log" 2>&1 &
VALIDATOR_PID=$!
cleanup() {
  LAUNCH_RUN_DIR="$RUN_DIR/launch" "$SCRIPT_DIR/launch_market.sh" stop local || true
  kill "$VALIDATOR_PID" 2>/dev/null || true
}
trap cleanup EXIT
for _ in $(seq 1 60); do
  solana cluster-version -u "$RPC" >/dev/null 2>&1 && break
  sleep 1
done
solana cluster-version -u "$RPC" >/dev/null || { echo "validator did not start" >&2; exit 2; }

# "Upstream publishes a program ID": deploy to a newly generated id.
PROG_KP="$RUN_DIR/keys/program.json"
solana-keygen new --no-bip39-passphrase --silent --force --outfile "$PROG_KP"
PROGRAM_ID="$(solana-keygen pubkey "$PROG_KP")"
t0="$(python3 -c 'import time; print(time.time())')"
solana program deploy -u "$RPC" -k "$PAYER" --program-id "$PROG_KP" "$SO" >/dev/null
DEPLOY_S="$(python3 -c "import sys, time; print(f'{time.time() - float(sys.argv[1]):.1f}')" "$t0")"
echo "==> program $PROGRAM_ID deployed in ${DEPLOY_S}s (not counted: upstream's job)"

# The config edit: the new id and the drill RPC, in a temp copy.
CFG="$RUN_DIR/sol-perp.toml"
python3 -I - "$CONFIG" "$CFG" "$PROGRAM_ID" "$RPC" <<'PY'
import re, sys
src, dst, pid, rpc = sys.argv[1:]
text = open(src).read()
head, sep, rest = text.partition("[cluster.local]")
local, nxt, tail = rest.partition("\n[")
local = re.sub(r'(?m)^program_id = ".*"', f'program_id = "{pid}"', local)
local = re.sub(r'(?m)^rpc = ".*"', f'rpc = "{rpc}"', local)
open(dst, "w").write(head + sep + local + nxt + tail)
PY

mkdir -p "$RUN_DIR/launch/keys"
cp "$PAYER" "$RUN_DIR/launch/keys/payer.json"
echo "==> launch_market.sh $CFG local"
rc=0
LAUNCH_SKIP_BUILD=1 LAUNCH_RUN_DIR="$RUN_DIR/launch" "$SCRIPT_DIR/launch_market.sh" "$CFG" local || rc=$?
echo "==> Day-0 drill times (program id published -> live), $(date -u +%Y-%m-%dT%H:%MZ)"
cat "$RUN_DIR/launch/timings.txt" 2>/dev/null || true
echo "==> exit $rc"
exit "$rc"
