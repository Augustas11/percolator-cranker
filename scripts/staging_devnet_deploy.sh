#!/usr/bin/env bash
# Devnet staging: deploy the pinned wrapper and run the SOL-PERP staging bots
# on devnet (one asset, SOL/USD on AuthMark, test USDC collateral).
# Reference / dry-run only; not a paid live service; M3 NO-GO.
# NOT RUN: no devnet market exists. Owner-run only.
#
#   ./scripts/staging_devnet_deploy.sh         # print the steps, do nothing
#   ./scripts/staging_devnet_deploy.sh --yes   # perform them
#
# Stage order follows the shape of the upstream CLI devnet smoke
# (percolator-cli tests/v16/devnet-smoke.ts, T10: create market account,
# InitMarket with one asset, ConfigureAuthMark, PushAuthMark). Shape only:
# this script uses our binaries.
#
# Steps:
#   1. preflight: tools, keypair files, the RPC is not mainnet (name and
#      genesis hash), the wrapper checkout is at the pin and clean
#   2. SBF build of the wrapper at PERCOLATOR_PROG_SHA
#   3. deploy the program under PROGRAM_KEYPAIR with UPGRADE_AUTHORITY_KEYPAIR
#      (skipped when the program already exists)
#   4. test USDC: create a 6-decimal mint we control (USDC_MINT_KEYPAIR,
#      mint authority = PAYER_KEYPAIR), skipped when it already exists;
#      testers get tokens from scripts/staging_usdc_faucet.sh
#   5. run `staging` against the cluster (SOL-PERP setup + bots,
#      docs/STAGING.md); PAYER_KEYPAIR pays and funds the per-run role keys
#
# Price source: the pusher signs marks from `mock_sol_usd` today. On devnet it
# is meant to read Pyth SOL/USD (PYTH_SOL_USD_FEED_ID); that read is NOT built,
# so a devnet run today pushes the mock path.
#
# Environment (keypair *paths* only; keep the files 0600, outside the repo):
#   STAGING_RPC_URL            devnet RPC URL (default https://api.devnet.solana.com)
#   PAYER_KEYPAIR              fee payer for deploy and staging       (required)
#   PROGRAM_KEYPAIR            program address keypair, first deploy  (required)
#   UPGRADE_AUTHORITY_KEYPAIR  upgrade authority, not a bot key       (required)
#   USDC_MINT_KEYPAIR          test USDC mint address keypair         (required)
#   PYTH_SOL_USD_FEED_ID       Pyth SOL/USD feed id (default below; not read yet)
#   STAGING_HOURS              how long the bots run (default 6)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
YES=0
case "${1:-}" in
  --yes) YES=1 ;;
  "") ;;
  *) echo "usage: $0 [--yes]" >&2; exit 2 ;;
esac

die() { echo "ERROR: $*" >&2; exit 1; }
run() { printf '+'; printf ' %q' "$@"; printf '\n'; if [ "$YES" = 1 ]; then "$@"; fi; }
[ "$YES" = 1 ] && echo "==> mode: EXECUTE" || echo "==> mode: print only (pass --yes to act)"

# shellcheck disable=SC1091
source "$SCRIPT_DIR/env.sh"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/pins.env"

# SOL-PERP market, asset and LP values, in one block. They mirror the
# constants in staging/src/main.rs (reasoning: docs/OPERATOR_PLAYBOOK.md
# section 2); the binary does not read them yet. Change both together.
MARKET_ASSETS=1                 # one asset: SOL, priced in USD
INITIAL_PRICE_E6=150000000      # $150
INITIAL_MARGIN_BPS=2000         # 5x
MAINTENANCE_MARGIN_BPS=1000
MAX_PRICE_MOVE_BPS_PER_SLOT=5
LIQUIDATION_FEE_BPS=50
TRADE_FEE_BASE_BPS=0            # fee paths off
MAINTENANCE_FEE_PER_SLOT=0
STALE_SLOTS=9000                # permissionless resolve window
FORCE_CLOSE_DELAY_SLOTS=20
COLLATERAL="test USDC, 6 decimals"
LP_CLIP_SOL=10                  # our matcher is the only LP; OI cap per side
LP_DEPOSIT_USDC=30000

# 1. preflight
RPC="${STAGING_RPC_URL:-https://api.devnet.solana.com}"
case "$(printf '%s' "$RPC" | tr '[:upper:]' '[:lower:]')" in
  *mainnet*|m) die "refusing mainnet ($RPC)" ;;
esac
MAINNET_GENESIS="5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d"
if genesis="$(solana genesis-hash -u "$RPC" 2>/dev/null)"; then
  [ "$genesis" != "$MAINNET_GENESIS" ] || die "$RPC serves mainnet; refusing"
  echo "==> rpc $RPC (genesis $genesis)"
elif [ "$YES" = 1 ]; then
  die "cannot reach $RPC to verify the cluster"
fi
for v in PAYER_KEYPAIR PROGRAM_KEYPAIR UPGRADE_AUTHORITY_KEYPAIR USDC_MINT_KEYPAIR; do
  [ -f "${!v:-}" ] || die "$v must name a keypair file"
  case "$(cd "$(dirname "${!v}")" && pwd)/" in
    "$REPO_ROOT"/*) die "$v is inside the repository; move it out" ;;
  esac
done
PROGRAM_ID="$(solana-keygen pubkey "$PROGRAM_KEYPAIR")"
echo "==> payer $(solana-keygen pubkey "$PAYER_KEYPAIR"), program $PROGRAM_ID"
PROG_DIR="${PERCOLATOR_PROG_DIR:?}"
[ "$(git -C "$PROG_DIR" rev-parse HEAD)" = "$PERCOLATOR_PROG_SHA" ] || die "wrapper is not at the pin $PERCOLATOR_PROG_SHA"
[ -z "$(git -C "$PROG_DIR" status --porcelain --untracked-files=no)" ] || die "wrapper checkout has local changes"

# 2. build
SO="$PROG_DIR/target/deploy/percolator_prog.so"
[ -f "$SO" ] || run cargo build-sbf --manifest-path "$PROG_DIR/Cargo.toml" --tools-version v1.52
run cargo build --release --manifest-path "$REPO_ROOT/staging/Cargo.toml"

# 3. deploy (first deploy only; upgrades are a separate, deliberate step)
if solana program show -u "$RPC" "$PROGRAM_ID" >/dev/null 2>&1; then
  echo "==> program $PROGRAM_ID already deployed; not touching it"
else
  run solana program deploy -u "$RPC" -k "$PAYER_KEYPAIR" \
    --program-id "$PROGRAM_KEYPAIR" --upgrade-authority "$UPGRADE_AUTHORITY_KEYPAIR" "$SO"
fi

# 4. test USDC mint (6 decimals, mint authority = payer)
USDC_MINT="$(solana-keygen pubkey "$USDC_MINT_KEYPAIR")"
if solana account -u "$RPC" "$USDC_MINT" >/dev/null 2>&1; then
  echo "==> test USDC mint $USDC_MINT exists"
else
  run spl-token create-token -u "$RPC" --fee-payer "$PAYER_KEYPAIR" --decimals 6 \
    --mint-authority "$(solana-keygen pubkey "$PAYER_KEYPAIR")" "$USDC_MINT_KEYPAIR"
fi

# 5. SOL-PERP staging bots
PYTH_SOL_USD_FEED_ID="${PYTH_SOL_USD_FEED_ID:-0xef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d}"
echo "==> SOL-PERP: $MARKET_ASSETS asset at \$$((INITIAL_PRICE_E6 / 1000000)), IM ${INITIAL_MARGIN_BPS} / MM ${MAINTENANCE_MARGIN_BPS} bps, move ${MAX_PRICE_MOVE_BPS_PER_SLOT} bps/slot, liq fee ${LIQUIDATION_FEE_BPS} bps, trade fee ${TRADE_FEE_BASE_BPS}, maint fee ${MAINTENANCE_FEE_PER_SLOT}, stale ${STALE_SLOTS} / force-close ${FORCE_CLOSE_DELAY_SLOTS} slots, $COLLATERAL, LP clip ${LP_CLIP_SOL} SOL, LP deposit \$${LP_DEPOSIT_USDC}"
echo "==> Pyth SOL/USD feed $PYTH_SOL_USD_FEED_ID (for the future Pyth price source; not read yet)"
run env STAGING_RPC_URL="$RPC" PERCOLATOR_PROGRAM_ID="$PROGRAM_ID" STAGING_PAYER="$PAYER_KEYPAIR" \
  STAGING_USDC_MINT="$USDC_MINT" STAGING_HOURS="${STAGING_HOURS:-6}" "$REPO_ROOT/staging/target/release/staging"
