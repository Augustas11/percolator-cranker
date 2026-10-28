#!/usr/bin/env bash
# Owner-run devnet staging deploy for the Percolator wrapper.
#
#   ./scripts/staging_devnet_deploy.sh            # dry run: print every action
#   ./scripts/staging_devnet_deploy.sh --execute  # perform the actions
#
# Steps (each one idempotent; re-running skips what is already done):
#   1. preflight: tools, keypair files, RPC is not mainnet, wrapper pin
#   2. SBF build of the wrapper at PERCOLATOR_PROG_SHA
#   3. deploy (or verify) the program under the owner's program keypair,
#      with the owner's upgrade authority
#   4. staging SPL mint and the market vault ATA
#   5. market account + InitMarket + post-init config: needs a staging CLI.
#      Without STAGING_CLI this step prints the exact commands needed and
#      marks the gap.
#
# Keys: only keypair *paths* are read, from the environment. The script
# never writes, copies or prints private key material; it prints public keys
# (solana-keygen pubkey) and file paths only. Keep keypair files 0600 and
# outside the repository.
#
# Environment:
#   STAGING_RPC_URL             RPC URL or moniker (default: devnet).
#                               Mainnet is refused, by name and by genesis hash.
#   PAYER_KEYPAIR               fee payer for deploy and setup          (required)
#   PROGRAM_KEYPAIR             program address keypair, first deploy   (required)
#   UPGRADE_AUTHORITY_KEYPAIR   program upgrade authority               (required)
#   MARKETAUTH_KEYPAIR          InitMarket signer -> marketauth         (required for step 5)
#   MARKET_KEYPAIR              market account address keypair          (required for step 5)
#   MINT_KEYPAIR                staging mint address keypair            (step 4; or set STAGING_MINT)
#   MINT_AUTHORITY_KEYPAIR      staging mint authority                  (step 4, when creating the mint)
#   STAGING_MINT                existing mint pubkey (skips mint creation)
#   STAGING_ASSETS              InitMarket max_portfolio_assets (default 3)
#   PROGRAM_MAX_LEN             optional --max-len for the first deploy
#   FORCE_SBF_REBUILD=1         rebuild the .so even if present
#   ALLOW_UPGRADE=1             allow replacing on-chain bytes that differ
#   STAGING_CLI                 path to the staging CLI (see step 5 contract)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

EXECUTE=0
for arg in "$@"; do
  case "$arg" in
    --execute) EXECUTE=1 ;;
    -h|--help) sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $arg (try --help)" >&2; exit 2 ;;
  esac
done

die()  { echo "ERROR: $*" >&2; exit 1; }
warn() { echo "WARN:  $*" >&2; }
info() { echo "==> $*"; }
# Print a command (shell-quoted) and run it only with --execute.
run() {
  printf '+'; printf ' %q' "$@"; printf '\n'
  if [ "$EXECUTE" = 1 ]; then "$@"; fi
}

if [ "$EXECUTE" = 1 ]; then info "mode: EXECUTE"; else info "mode: dry run (pass --execute to act)"; fi

# shellcheck disable=SC1091
source "$SCRIPT_DIR/env.sh"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/pins.env"

# ---- 1. preflight ---------------------------------------------------------
RPC="${STAGING_RPC_URL:-devnet}"
rpc_lc="$(printf '%s' "$RPC" | tr '[:upper:]' '[:lower:]')"
case "$rpc_lc" in
  *mainnet*|m) die "refusing to run against mainnet ($RPC)" ;;
esac

for tool in solana solana-keygen spl-token cargo git cmp; do
  command -v "$tool" >/dev/null 2>&1 || die "missing tool: $tool (source scripts/env.sh)"
done

# Mainnet genesis hash. A URL that does not say "mainnet" but serves it is
# still refused.
MAINNET_GENESIS="5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d"
if genesis="$(solana genesis-hash -u "$RPC" 2>/dev/null)"; then
  [ "$genesis" = "$MAINNET_GENESIS" ] && die "RPC $RPC serves mainnet (genesis $genesis); refusing"
  info "rpc: $RPC (genesis $genesis)"
else
  [ "$EXECUTE" = 1 ] && die "cannot reach RPC $RPC to verify the cluster"
  warn "cannot reach RPC $RPC; cluster not verified (dry run continues)"
fi

file_mode() { stat -f %Lp "$1" 2>/dev/null || stat -c %a "$1"; }
# Validate a keypair path and print its PUBLIC key only.
keypair_pubkey() {
  local var="$1" path="${!1:-}"
  [ -n "$path" ] || die "$var is not set (path to a keypair file)"
  [ -f "$path" ] || die "$var: no such file: $path"
  case "$(cd "$(dirname "$path")" && pwd)/" in
    "$REPO_ROOT"/*) warn "$var lives inside the repository; move it out (it must never be committed)" ;;
  esac
  local mode; mode="$(file_mode "$path")"
  case "$mode" in
    600|400) ;;
    *) warn "$var has mode $mode; use chmod 600" ;;
  esac
  solana-keygen pubkey "$path"
}

PAYER="$(keypair_pubkey PAYER_KEYPAIR)"
PROGRAM_ID="$(keypair_pubkey PROGRAM_KEYPAIR)"
UPGRADE_AUTH="$(keypair_pubkey UPGRADE_AUTHORITY_KEYPAIR)"
info "payer:             $PAYER"
info "program id:        $PROGRAM_ID"
info "upgrade authority: $UPGRADE_AUTH"
if [ -n "${MARKETAUTH_KEYPAIR:-}" ]; then
  MARKETAUTH="$(keypair_pubkey MARKETAUTH_KEYPAIR)"
  info "marketauth:        $MARKETAUTH"
  [ "$MARKETAUTH" != "$UPGRADE_AUTH" ] || warn "marketauth == upgrade authority; use separate keys"
fi

PROG_DIR="${PERCOLATOR_PROG_DIR:?PERCOLATOR_PROG_DIR unset (scripts/env.sh)}"
[ -d "$PROG_DIR/.git" ] || [ -f "$PROG_DIR/.git" ] || die "no wrapper checkout at $PROG_DIR (run scripts/fetch_upstream.sh)"
head_sha="$(git -C "$PROG_DIR" rev-parse HEAD)"
[ "$head_sha" = "$PERCOLATOR_PROG_SHA" ] || die "wrapper at $head_sha, pin is $PERCOLATOR_PROG_SHA"
if [ -n "$(git -C "$PROG_DIR" status --porcelain --untracked-files=no)" ]; then
  die "wrapper checkout has local modifications; deploy only the pinned source"
fi
info "wrapper pin ok: $PERCOLATOR_PROG_SHA"

# ---- 2. SBF build ---------------------------------------------------------
SO="$PROG_DIR/target/deploy/percolator_prog.so"
if [ ! -f "$SO" ] || [ "${FORCE_SBF_REBUILD:-0}" = 1 ]; then
  info "build: cargo build-sbf at the pin"
  run cargo build-sbf --manifest-path "$PROG_DIR/Cargo.toml" --tools-version v1.52
else
  info "build: $SO present (FORCE_SBF_REBUILD=1 rebuilds)"
fi
if [ -f "$SO" ]; then
  so_len="$(wc -c < "$SO" | tr -d ' ')"
  max_len="${PROGRAM_MAX_LEN:-$so_len}"
  [ "$max_len" -ge "$so_len" ] || die "PROGRAM_MAX_LEN ($max_len) < .so size ($so_len)"
  info "program size $so_len bytes; program data rent at max-len $max_len: $(solana rent $((max_len + 45)) 2>/dev/null | head -1)"
  info "a deploy also holds a same-size buffer until it completes (refunded)"
else
  so_len=""
  info "(dry run: .so not built yet, size unknown)"
fi
if bal="$(solana balance -u "$RPC" "$PAYER" 2>/dev/null)"; then info "payer balance: $bal"; fi

# ---- 3. deploy / verify ---------------------------------------------------
if show="$(solana program show -u "$RPC" "$PROGRAM_ID" 2>/dev/null)"; then
  onchain_auth="$(printf '%s\n' "$show" | awk '/^Authority:/ {print $2}')"
  info "program exists on-chain; upgrade authority: ${onchain_auth:-unknown}"
  [ "$onchain_auth" = "$UPGRADE_AUTH" ] || die "on-chain upgrade authority ($onchain_auth) is not UPGRADE_AUTHORITY_KEYPAIR ($UPGRADE_AUTH)"
  if [ -n "$so_len" ]; then
    tmpdir="$(mktemp -d)"; trap 'rm -rf "$tmpdir"' EXIT
    solana program dump -u "$RPC" "$PROGRAM_ID" "$tmpdir/onchain.so" >/dev/null
    if cmp -s -n "$so_len" "$SO" "$tmpdir/onchain.so"; then
      info "deploy: on-chain bytes match the pinned build; nothing to do"
    elif [ "${ALLOW_UPGRADE:-0}" = 1 ]; then
      info "deploy: on-chain bytes differ; upgrading (ALLOW_UPGRADE=1)"
      run solana program deploy -u "$RPC" -k "$PAYER_KEYPAIR" \
        --program-id "$PROGRAM_KEYPAIR" --upgrade-authority "$UPGRADE_AUTHORITY_KEYPAIR" "$SO"
    else
      die "on-chain bytes differ from the pinned build; set ALLOW_UPGRADE=1 to replace them"
    fi
  fi
else
  info "deploy: program not found on $RPC; first deploy"
  deploy_args=(solana program deploy -u "$RPC" -k "$PAYER_KEYPAIR"
    --program-id "$PROGRAM_KEYPAIR" --upgrade-authority "$UPGRADE_AUTHORITY_KEYPAIR")
  [ -n "${PROGRAM_MAX_LEN:-}" ] && deploy_args+=(--max-len "$PROGRAM_MAX_LEN")
  run "${deploy_args[@]}" "$SO"
fi

# ---- 4. staging mint and vault ---------------------------------------------
account_exists() { solana account -u "$RPC" "$1" >/dev/null 2>&1; }

MINT="${STAGING_MINT:-}"
if [ -z "$MINT" ] && [ -n "${MINT_KEYPAIR:-}" ]; then
  MINT="$(keypair_pubkey MINT_KEYPAIR)"
  if account_exists "$MINT"; then
    info "mint $MINT exists"
  else
    MINT_AUTH="$(keypair_pubkey MINT_AUTHORITY_KEYPAIR)"
    [ "$MINT_AUTH" != "$PAYER" ] || warn "mint authority == payer; keep the mint authority off bot keys"
    run spl-token create-token -u "$RPC" --fee-payer "$PAYER_KEYPAIR" \
      --decimals 6 --mint-authority "$MINT_AUTH" "$MINT_KEYPAIR"
  fi
fi

if [ -n "${MARKET_KEYPAIR:-}" ]; then
  MARKET="$(keypair_pubkey MARKET_KEYPAIR)"
  info "market account: $MARKET"
  VAULT_AUTH="$(solana find-program-derived-address "$PROGRAM_ID" string:vault "pubkey:$MARKET" | head -1)"
  info "vault authority PDA [\"vault\", market]: $VAULT_AUTH"
  if [ -n "$MINT" ]; then
    # The canonical vault is the ATA of the vault PDA for the mint.
    if vault_out="$(spl-token address -u "$RPC" --verbose --token "$MINT" --owner "$VAULT_AUTH" 2>/dev/null)"; then
      VAULT="$(printf '%s\n' "$vault_out" | awk '/Associated token address:/ {print $4}')"
    else
      VAULT=""
    fi
    if [ -n "$VAULT" ] && account_exists "$VAULT"; then
      info "vault $VAULT exists"
    else
      run spl-token create-account -u "$RPC" --fee-payer "$PAYER_KEYPAIR" --owner "$VAULT_AUTH" "$MINT"
    fi
  else
    warn "no STAGING_MINT / MINT_KEYPAIR: skipping the vault"
  fi
fi

# ---- 5. market account + InitMarket + post-init config -------------------
# Staging CLI contract (the CLI is not part of this script):
#   $STAGING_CLI init-market  --url U --program-id P --payer KP --marketauth KP
#                             --market-keypair KP --mint M --assets N --params staging
#       creates the market account at market_account_len_for_capacity(N),
#       sends InitMarket with the STAGING column of docs/OPERATOR_PLAYBOOK.md (b);
#       must be a no-op when the market account already exists and is initialized.
#   $STAGING_CLI configure    --url U --program-id P --market M --marketauth KP --params staging
#       ConfigureAuthMark (all assets), ConfigurePermissionlessResolve,
#       fee policies; skips steps whose policy_sequence is already applied.
GAP=0
if [ -z "${MARKET_KEYPAIR:-}" ] || [ -z "${MARKETAUTH_KEYPAIR:-}" ] || [ -z "$MINT" ]; then
  warn "step 5 needs MARKET_KEYPAIR, MARKETAUTH_KEYPAIR and a mint; skipped"
elif account_exists "$MARKET"; then
  owner="$(solana account -u "$RPC" "$MARKET" --output json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["account"]["owner"])' 2>/dev/null || true)"
  [ "$owner" = "$PROGRAM_ID" ] || die "market account $MARKET exists but is owned by ${owner:-unknown}, not $PROGRAM_ID"
  info "market account exists and is program-owned; InitMarket presumed done (check with the monitor)"
elif [ -n "${STAGING_CLI:-}" ]; then
  [ -x "$STAGING_CLI" ] || die "STAGING_CLI is not executable: $STAGING_CLI"
  common=(--url "$RPC" --program-id "$PROGRAM_ID" --payer "$PAYER_KEYPAIR" --marketauth "$MARKETAUTH_KEYPAIR")
  run "$STAGING_CLI" init-market "${common[@]}" --market-keypair "$MARKET_KEYPAIR" \
    --mint "$MINT" --assets "${STAGING_ASSETS:-3}" --params staging
  run "$STAGING_CLI" configure "${common[@]}" --market "$MARKET" --params staging
else
  GAP=1
  cat <<EOF
==> GAP: no staging CLI (set STAGING_CLI). These steps still need one:
    # 5a. create the market account (system CreateAccount, owner = program)
    STAGING_CLI init-market --url $RPC --program-id $PROGRAM_ID \\
      --payer \$PAYER_KEYPAIR --marketauth \$MARKETAUTH_KEYPAIR \\
      --market-keypair \$MARKET_KEYPAIR --mint $MINT --assets ${STAGING_ASSETS:-3} --params staging
    # 5b. ConfigureAuthMark x assets, ConfigurePermissionlessResolve 9000/9000,
    #     UpdateLiquidationFeePolicy 5000, UpdateMaintenanceFeePolicy 2000,
    #     UpdateTradeFeePolicy 5, UpdateBackingFeePolicy 10/2000 per domain
    STAGING_CLI configure --url $RPC --program-id $PROGRAM_ID \\
      --payer \$PAYER_KEYPAIR --marketauth \$MARKETAUTH_KEYPAIR --market $MARKET --params staging
    # 5c. role split (UpdateAssetAuthority, then UpdateAuthority last),
    #     TopUpInsurance, TopUpBackingBucket: see docs/STAGING.md section 3.
EOF
fi

info "done (program $PROGRAM_ID${MARKET:+, market $MARKET})"
if [ "$GAP" = 1 ] && [ "$EXECUTE" = 1 ]; then
  echo "market init NOT performed: staging CLI missing (see GAP above)" >&2
  exit 3
fi
