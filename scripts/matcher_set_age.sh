#!/usr/bin/env bash
# Apply [cluster.<name>].matcher_max_ref_age_slots to each configured matcher
# ctx, signed by the matcher LP owner key (owner decision, 2026-10-29). Every
# other parameter is read from the ctx and written back unchanged (SetParams
# rewrites them all, inventory included). Prints the steps unless --yes.
#
#   scripts/matcher_set_age.sh <config> <cluster> <run dir> [--yes]
set -euo pipefail
CONFIG="$1" CLUSTER="$2" RUN="$3" YES="${4:-}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OPS="$ROOT/matcher/host/target/release/matcher_ops"
[ "$CLUSTER" != mainnet ] || [ "${MATCHER_MAINNET_I_UNDERSTAND:-}" = 1 ] || { echo "mainnet needs MATCHER_MAINNET_I_UNDERSTAND=1" >&2; exit 2; }
read -r PROGRAM AGE CTXS < <(python3 -I - "$CONFIG" "$CLUSTER" <<'PY'
import sys, tomllib
c = tomllib.load(open(sys.argv[1], "rb")); k = c["cluster"][sys.argv[2]]
print(c["matcher"]["program_id"], k["matcher_max_ref_age_slots"], ",".join(k.get("matcher_ctxs", [])) or "-")
PY
)
[ "$CTXS" != - ] || { echo "no matcher_ctxs for $CLUSTER"; exit 0; }
[ -x "$OPS" ] || (cd "$ROOT/matcher/host" && cargo build --release --quiet --bin matcher_ops)
for CTX in ${CTXS//,/ }; do
  S="$("$OPS" show --matcher "$PROGRAM" --ctx "$CTX")"
  v() { grep -oE "(^| )$1=[^ ]+" <<<"$S" | head -1 | sed 's/.*=//'; }
  for f in max_fill_abs max_inventory_abs max_mark_dev_bps price_authority paused inventory; do
    [ -n "$(v $f)" ] || { echo "ctx $CTX: could not read $f from matcher_ops show" >&2; exit 3; }
  done
  ARGS=(--matcher "$PROGRAM" --payer "$RUN/keys/payer.json" --lp-owner "$RUN/keys/matcher_lp_owner.json"
        --ctx "$CTX" --fill "$(v max_fill_abs)" --cap "$(v max_inventory_abs)" --age "$AGE"
        --deviation "$(v max_mark_dev_bps)" --authority "$(v price_authority)" --paused "$(v paused)"
        --inventory "$(v inventory)")
  echo "ctx $CTX: max_ref_age_slots $(v max_ref_age_slots) -> $AGE"
  if [ "$YES" = --yes ]; then "$OPS" set-params "${ARGS[@]}"; else echo "  (print only) matcher_ops set-params ${ARGS[*]}"; fi
done
