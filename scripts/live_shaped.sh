#!/usr/bin/env bash
# Deterministic live-shaped load scenario against the local validator.
#
# Reuses a running solana-test-validator (never resets the ledger). If none
# is up, start one first with: KEEP_VALIDATOR=0 ./scripts/m1_bringup.sh
#
# Knobs (env): CRANKER_SCENARIO_SEED, CRANKER_SCENARIO_TRADERS (40),
# CRANKER_SCENARIO_ASSETS (3), CRANKER_CYCLES (200), PERCOLATOR_PROGRAM_ID
# (defaults to the SBF keypair id), CRANKER_CYCLE_MS, CRANKER_CYCLE_MAX_TX,
# CRANKER_SCENARIO_RUN (default 0). Keys derive from (seed, run), so on a
# reused ledger give every run a new CRANKER_SCENARIO_RUN.
#
# Usage: ./scripts/live_shaped.sh 2>&1 | tee live-shaped-run.log

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/env.sh"

if ! solana cluster-version --url http://127.0.0.1:8899 >/dev/null 2>&1; then
  echo "no validator on 127.0.0.1:8899 — run: KEEP_VALIDATOR=0 ./scripts/m1_bringup.sh" >&2
  exit 2
fi

if [ -z "${PERCOLATOR_PROGRAM_ID:-}" ]; then
  PERCOLATOR_PROGRAM_ID="$(solana-keygen pubkey "${PERCOLATOR_PROG_DIR:-$REPO_ROOT/repo-prog}/target/deploy/percolator_prog-keypair.json")"
fi
export PERCOLATOR_PROGRAM_ID
if ! solana program show "$PERCOLATOR_PROGRAM_ID" --url http://127.0.0.1:8899 >/dev/null 2>&1; then
  echo "program $PERCOLATOR_PROGRAM_ID not deployed — run: KEEP_VALIDATOR=1 ./scripts/m1_bringup.sh" >&2
  exit 2
fi

CRANKER_SCENARIO=live_shaped exec cargo run --release --manifest-path "${CRANKER_MANIFEST:-$REPO_ROOT/cranker/Cargo.toml}"
