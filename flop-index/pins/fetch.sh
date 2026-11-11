#!/usr/bin/env bash
# Download a pin's files into a data folder: fetch.sh <pin> <out dir>
# e.g. fetch.sh 2026-11-11 ~/flop-score-data/2026-11-11
# The files are not committed (one source's licence is unclear). flop-score
# checks every sha256 against the MANIFEST before reading anything.
set -euo pipefail
pin="$(cd "$(dirname "$0")/$1" && pwd)"
out="$2"
mkdir -p "$out"
grep -v '^#' "$pin/SOURCES" | while read -r file url; do
  [ -n "$file" ] || continue
  curl -fsSL "$url" -o "$out/$file"
done
cp "$pin/MANIFEST" "$out/MANIFEST"
echo "fetched into $out; flop-score --data $out --rule spec checks the hashes"
