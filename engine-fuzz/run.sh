#!/usr/bin/env bash
# Run the money fuzzer in strict mode.
#
#   engine-fuzz/run.sh [pin|master] [cases]
#
# pin    : the production engine pin in Cargo.toml (same rev as repo-prog).
# master : upstream master. Exported read-only from the local engine checkout
#          (../repo, ref origin/master by default) into target/percolator-master and
#          patched in with `--config patch...path=...`. Override the source with
#          MASTER_SRC=/path/to/percolator or the ref with MASTER_REF=<git ref>.
#
# Steps: replays, a survey histogram over `cases` random scenarios, then one strict
# proptest per known bug-class invariant so each distinct class gets its own shrunk
# minimal repro. Exit status is non-zero if any step failed.
set -uo pipefail

target="${1:-pin}"
cases="${2:-2000}"
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here"

cargo_args=(--release)
if [[ "$target" == "master" ]]; then
  src="${MASTER_SRC:-}"
  if [[ -z "$src" ]]; then
    ref="${MASTER_REF:-origin/master}"
    src="$here/target/percolator-master"
    rm -rf "$src" && mkdir -p "$src"
    git -C "$here/../repo" archive "$ref" | tar -x -C "$src" || { echo "cannot export $ref from ../repo"; exit 2; }
    echo "engine: $ref = $(git -C "$here/../repo" rev-parse "$ref")"
  else
    echo "engine: $src"
  fi
  # Patching rewrites Cargo.lock; keep the pin's lock untouched.
  cp Cargo.lock "$here/target/Cargo.lock.pin" 2>/dev/null || true
  trap 'cp "$here/target/Cargo.lock.pin" "$here/Cargo.lock" 2>/dev/null || true' EXIT
  cargo_args+=(--config "patch.\"https://github.com/aeyakovenko/percolator\".percolator.path=\"$src\"")
  export CARGO_TARGET_DIR="$here/target/master"
elif [[ "$target" == "pin" ]]; then
  echo "engine: pin $(grep -o 'rev = "[0-9a-f]*"' Cargo.toml)"
else
  echo "usage: $0 [pin|master] [cases]"; exit 2
fi

status=0
step() { echo; echo "=== $*"; }

step "replays"
cargo test "${cargo_args[@]}" --test replays -- --test-threads 1 || status=1

step "survey: $cases scenarios"
FUZZ_STRICT=1 FUZZ_SURVEY=1 FUZZ_CASES="$cases" \
  cargo test "${cargo_args[@]}" --test money_fuzz money_survey -- --nocapture || status=1

for inv in inv2_no_default_payout inv2_no_default_retired inv4_provider_principal \
           inv3_loss_share_total inv3_loss_share_per_winner terminal_stuck_unbacked_claim; do
  step "strict fuzz targeting $inv ($cases cases)"
  FUZZ_STRICT=1 FUZZ_INVARIANT="$inv" FUZZ_CASES="$cases" \
    cargo test "${cargo_args[@]}" --test money_fuzz money_fuzz -- --nocapture || status=1
done

echo
[[ $status -eq 0 ]] && echo "run.sh $target: all clean" || echo "run.sh $target: violations found (see above)"
exit $status
