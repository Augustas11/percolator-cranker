#!/usr/bin/env bash
# Test-USDC faucet for the devnet SOL-PERP staging market: mint test USDC
# (our own 6-decimal mint, not real USDC) to a tester's wallet.
# Reference / dry-run only; not a paid live service; M3 NO-GO. Owner-run only.
#
#   ./scripts/staging_usdc_faucet.sh <tester-wallet> <amount-usdc>         # print only
#   ./scripts/staging_usdc_faucet.sh <tester-wallet> <amount-usdc> --yes   # mint
#
# Reads the devnet launch (scripts/launch_market.sh <config> devnet): the RPC
# from the config, the mint from the run's state.json, its authority (the
# run's payer) from the run's keys. Environment overrides:
#   LAUNCH_CONFIG          config (default launch/sol-perp.toml)
#   LAUNCH_RUN_DIR         run dir (default launch-runs/devnet)
#   STAGING_RPC_URL, USDC_MINT, MINT_AUTHORITY_KEYPAIR   explicit values
#   FAUCET_MAX_USDC        per-call cap (default 100000)

set -euo pipefail
[ $# -ge 2 ] || { echo "usage: $0 <tester-wallet> <amount-usdc> [--yes]" >&2; exit 2; }
TO="$1"; AMOUNT="$2"; YES=0
[ "${3:-}" = "--yes" ] && YES=1
die() { echo "ERROR: $*" >&2; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/env.sh"

REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CONFIG="${LAUNCH_CONFIG:-$REPO_ROOT/launch/sol-perp.toml}"
RUN_DIR="${LAUNCH_RUN_DIR:-$REPO_ROOT/launch-runs/devnet}"
RPC="${STAGING_RPC_URL:-$(python3 -I -c 'import sys, tomllib; print(tomllib.load(open(sys.argv[1], "rb"))["cluster"]["devnet"]["rpc"])' "$CONFIG")}"
USDC_MINT="${USDC_MINT:-$(python3 -I -c 'import json, sys; print(json.load(open(sys.argv[1]))["mint"])' "$RUN_DIR/state.json" 2>/dev/null || true)}"
MINT_AUTHORITY_KEYPAIR="${MINT_AUTHORITY_KEYPAIR:-$RUN_DIR/keys/payer.json}"
case "$(printf '%s' "$RPC" | tr '[:upper:]' '[:lower:]')" in
  *mainnet*|m) die "refusing mainnet ($RPC)" ;;
esac
if genesis="$(solana genesis-hash -u "$RPC" 2>/dev/null)"; then
  [ "$genesis" != "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d" ] || die "$RPC serves mainnet; refusing"
elif [ "$YES" = 1 ]; then
  die "cannot reach $RPC to verify the cluster"
fi
: "${USDC_MINT:?no mint: run launch_market.sh <config> devnet first, or set USDC_MINT}"
[ -f "${MINT_AUTHORITY_KEYPAIR:-}" ] || die "MINT_AUTHORITY_KEYPAIR must name a keypair file"
[[ "$AMOUNT" =~ ^[0-9]+$ ]] && [ "$AMOUNT" -gt 0 ] || die "amount must be a whole number of USDC"
[ "$AMOUNT" -le "${FAUCET_MAX_USDC:-100000}" ] || die "amount above FAUCET_MAX_USDC"

run() { printf '+'; printf ' %q' "$@"; printf '\n'; if [ "$YES" = 1 ]; then "$@"; fi; }
# The tester's test-USDC token account, created (paid by us) when missing.
ata_out="$(spl-token address -u "$RPC" --verbose --token "$USDC_MINT" --owner "$TO" 2>/dev/null || true)"
ATA="$(printf '%s\n' "$ata_out" | awk '/Associated token address:/ {print $4}')"
if [ -z "$ATA" ] || ! solana account -u "$RPC" "$ATA" >/dev/null 2>&1; then
  run spl-token create-account -u "$RPC" --fee-payer "$MINT_AUTHORITY_KEYPAIR" --owner "$TO" "$USDC_MINT"
fi
run spl-token mint -u "$RPC" --mint-authority "$MINT_AUTHORITY_KEYPAIR" --fee-payer "$MINT_AUTHORITY_KEYPAIR" \
  --recipient-owner "$TO" "$USDC_MINT" "$AMOUNT"
[ "$YES" = 1 ] || echo "(print only; pass --yes to mint)"
