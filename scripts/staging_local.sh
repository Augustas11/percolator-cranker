#!/usr/bin/env bash
# Local staging market: a dedicated solana-test-validator with the wrapper
# loaded, then the `staging` binary (staging/) for STAGING_HOURS. Local only.
# Reference / dry-run only; not a paid live service; M3 NO-GO.
#
#   ./scripts/staging_local.sh                    # 6 hours
#   STAGING_HOURS=0.17 ./scripts/staging_local.sh # ~10 minute smoke run
#
# Everything lands in a run dir (default .staging-runs/<timestamp>, gitignored):
# payer.json (generated, local only), ledger/ (deleted at exit unless
# KEEP_LEDGER=1), validator.log, staging.log.
# The exit code is the staging binary's: 0 = normal, 1 = abnormal failures.
# Market, amounts, matcher and oracle params come from launch/sol-perp.toml
# (STAGING_CONFIG overrides), the same file as the Day-0 launch. Other
# STAGING_* knobs (tick, seed, backing tenor, ...) pass through; see
# docs/STAGING.md.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/env.sh"

PROG_DIR="${PERCOLATOR_PROG_DIR:-$REPO_ROOT/repo-prog}"
PROG_SO="$PROG_DIR/target/deploy/percolator_prog.so"
PROG_KEYPAIR="$PROG_DIR/target/deploy/percolator_prog-keypair.json"
# Own ports, so it can run beside the M1 validator on 8899.
PORT="${STAGING_RPC_PORT:-8799}"
RUN_DIR="${STAGING_RUN_DIR:-$REPO_ROOT/.staging-runs/$(date +%Y%m%d-%H%M%S)}"
export STAGING_HOURS="${STAGING_HOURS:-6}"
export STAGING_RPC_URL="http://127.0.0.1:$PORT"

# Always build: an existing .so may come from another wrapper revision
# (incremental, so this is quick when nothing changed).
echo "==> building percolator_prog.so (cargo build-sbf --tools-version v1.52)"
mkdir -p "$HOME/.cache/solana"
( cd "$PROG_DIR" && cargo build-sbf --tools-version v1.52 )
export PERCOLATOR_PROGRAM_ID="$(solana-keygen pubkey "$PROG_KEYPAIR")"

echo "==> building staging"
cargo build --release --quiet --manifest-path "$REPO_ROOT/staging/Cargo.toml"

mkdir -p "$RUN_DIR"
export STAGING_PAYER="$RUN_DIR/payer.json"
[ -f "$STAGING_PAYER" ] || solana-keygen new --no-bip39-passphrase --silent --outfile "$STAGING_PAYER"

echo "==> validator on $STAGING_RPC_URL (run dir $RUN_DIR)"
solana-test-validator --reset --quiet \
  --ledger "$RUN_DIR/ledger" --limit-ledger-size 10000 \
  --bind-address 127.0.0.1 --rpc-port "$PORT" --faucet-port $((PORT + 1000)) \
  --gossip-port $((PORT + 1010)) --dynamic-port-range "$((PORT + 1020))-$((PORT + 1120))" \
  --mint "$(solana-keygen pubkey "$STAGING_PAYER")" \
  --bpf-program "$PERCOLATOR_PROGRAM_ID" "$PROG_SO" \
  > "$RUN_DIR/validator.log" 2>&1 &
VALIDATOR_PID=$!
# Stop the validator; drop its ledger (KEEP_LEDGER=1 keeps it). Logs stay.
cleanup() {
  local rc=$?
  kill "$VALIDATOR_PID" 2>/dev/null || true
  wait "$VALIDATOR_PID" 2>/dev/null || true
  [ "${KEEP_LEDGER:-0}" = 1 ] || rm -rf "$RUN_DIR/ledger"
  exit "$rc"
}
trap cleanup EXIT
for _ in $(seq 1 60); do
  solana cluster-version --url "$STAGING_RPC_URL" >/dev/null 2>&1 && break
  sleep 1
done
solana cluster-version --url "$STAGING_RPC_URL" >/dev/null || { echo "validator did not start" >&2; exit 2; }

echo "==> staging for $STAGING_HOURS h (log $RUN_DIR/staging.log)"
set +e
"$REPO_ROOT/staging/target/release/staging" 2>&1 | tee "$RUN_DIR/staging.log"
STATUS=${PIPESTATUS[0]}
set -e
echo "==> final summary"
grep '"summary":"final"' "$RUN_DIR/staging.log" || echo "(no final summary: setup failed?)"
echo "==> exit $STATUS"
exit "$STATUS"
