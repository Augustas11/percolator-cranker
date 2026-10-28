#!/usr/bin/env bash
# Clone the upstream wrapper (percolator-prog) at the pinned SHA into
# upstream/percolator-prog. The crates depend on it by path; the engine is a
# git dependency pinned by rev in Cargo.toml. Read-only: fetch + checkout.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/pins.env"
DEST="$REPO_ROOT/upstream/percolator-prog"
if [ ! -d "$DEST/.git" ]; then
  mkdir -p "$REPO_ROOT/upstream"
  git clone -q "$PERCOLATOR_PROG_REPO" "$DEST"
fi
git -C "$DEST" fetch -q origin
git -C "$DEST" checkout -q --detach "$PERCOLATOR_PROG_SHA"
echo "percolator-prog @ $(git -C "$DEST" rev-parse --short HEAD)"
want="$PERCOLATOR_ENGINE_REV"
got="$(grep -o 'rev = "[0-9a-f]*"' "$DEST/Cargo.toml" | head -1 | cut -d'"' -f2)"
if [ "$got" != "$want" ]; then
  echo "pin mismatch: wrapper pins engine $got, scripts/pins.env says $want" >&2
  exit 1
fi
echo "engine pin $want matches the wrapper's Cargo pin"
