#!/usr/bin/env bash
# Allowlist export of the public reference-keeper tree.
#
#   ./scripts/export_public.sh <target-dir>
#
# Copies ONLY the paths listed below, applies the public layout
# (crates/cranker, crates/bringup, upstream/ via scripts/fetch_upstream.sh),
# overlays the public-only files under public/, then runs
# scripts/check_public_tree.sh with the private denylist. Nothing is copied by
# default: a new file reaches the public tree only when it is added here.
#
# Never exported (by construction, and asserted at the end): prep.md,
# ROADMAP.md, cranker-strategic-questions.md, prompts/, watcher outputs,
# repo/, repo-prog/, .preview/, ledgers, keypairs, .env, the denylist, this
# script, and the private READMEs / findings.
#
# Env: PUBLIC_NAME (default "percolator-reference-keeper") replaces
# @PUBLIC_NAME@ in overlay docs once the owner picks a name.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$(cd "$SCRIPT_DIR/.." && pwd)"
DEST="${1:?usage: export_public.sh <target-dir>}"
PUBLIC_NAME="${PUBLIC_NAME:-percolator-reference-keeper}"

if [ -e "$DEST" ] && [ -n "$(ls -A "$DEST" 2>/dev/null | grep -v '^target$' || true)" ]; then
  echo "refusing: $DEST exists and is not empty" >&2
  exit 1
fi
mkdir -p "$DEST"
DEST="$(cd "$DEST" && pwd)"

# Copy one file from the private tree (committed content of the working
# tree) to a public path.
put() {
  local from="$1" to="$2"
  mkdir -p "$(dirname "$DEST/$to")"
  cp "$SRC/$from" "$DEST/$to"
}
# Copy a directory, keeping only the listed file patterns.
put_dir() {
  local from="$1" to="$2"; shift 2
  (cd "$SRC/$from" && find . -type f \( "$@" \) -not -path './target/*') | while read -r f; do
    put "$from/${f#./}" "$to/${f#./}"
  done
}

# ---- allowlist ----------------------------------------------------------
# cranker crate (library, binary, integration tests)
put cranker/Cargo.toml          crates/cranker/Cargo.toml
put cranker/build.rs            crates/cranker/build.rs
put_dir cranker/src             crates/cranker/src   -name '*.rs'
put_dir cranker/tests           crates/cranker/tests -name '*.rs'
# M1 bring-up client
put client/Cargo.toml           crates/bringup/Cargo.toml
put client/build.rs             crates/bringup/build.rs
put_dir client/src              crates/bringup/src   -name '*.rs'
# scripts usable outside this repo
put scripts/m1_bringup.sh       scripts/m1_bringup.sh
put scripts/check_public_tree.sh scripts/check_public_tree.sh
put scripts/fetch_upstream.sh   scripts/fetch_upstream.sh
put scripts/pins.env            scripts/pins.env
# the same CI pipeline as this repo (layout-neutral via scripts/env.sh;
# public/scripts/env.sh sets the public layout)
put .github/workflows/ci.yml    .github/workflows/ci.yml
# public-only overlay (README, docs, CI, workspace, env.sh, pins, fetch)
(cd "$SRC/public" && find . -type f) | while read -r f; do
  put "public/${f#./}" "${f#./}"
done
# The root Cargo.lock comes from the overlay (public/Cargo.lock): the
# workspace-resolved lock, seeded from cranker/Cargo.lock. Re-seed it when
# crate dependencies change; the verify step builds with --locked.

# ---- layout rewrites (explicit, each one checked) -----------------------
rewrite() { # file, from, to
  local f="$DEST/$1"
  grep -qF -- "$2" "$f" || { echo "rewrite anchor missing in $1: $2" >&2; exit 1; }
  python3 - "$f" "$2" "$3" <<'PY'
import sys
p, a, b = sys.argv[1:]
s = open(p).read()
open(p, "w").write(s.replace(a, b))
PY
}
rewrite crates/cranker/Cargo.toml 'path = "../repo-prog"' 'path = "../../upstream/percolator-prog"'
rewrite crates/bringup/Cargo.toml 'path = "../repo-prog"' 'path = "../../upstream/percolator-prog"'
for f in crates/cranker/Cargo.toml crates/bringup/Cargo.toml; do
  python3 - "$DEST/$f" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
# The workspace root owns the lockfile and profile; drop crate-local notes
# that refer to the private layout.
s = s.replace("# Cargo.lock IS committed", "# Cargo.lock lives at the workspace root")
open(p, "w").write(s)
PY
done
PUBLIC_NAME="$PUBLIC_NAME" python3 - "$DEST" <<'PY'
import os, sys, pathlib
name = os.environ["PUBLIC_NAME"]
for p in pathlib.Path(sys.argv[1]).rglob("*"):
    if p.is_file() and "target" not in p.parts and p.suffix in {".md", ".yml", ".toml", ".sh", ".html"}:
        s = p.read_text()
        if "@PUBLIC_NAME@" in s:
            p.write_text(s.replace("@PUBLIC_NAME@", name))
PY
chmod +x "$DEST"/scripts/*.sh

# ---- assert nothing private slipped through -----------------------------
forbidden=(prep.md ROADMAP.md cranker-strategic-questions.md prompts digests signals.md radar.md
  contributors.md watchlist.md releases state repo repo-prog .preview .solana-test-ledger
  public_denylist.txt export_public.sh scenario-findings.md)
for name in "${forbidden[@]}"; do
  if [ -n "$(find "$DEST" -name "$name" -not -path '*/target/*' | head -1)" ]; then
    echo "export leaked forbidden path: $name" >&2
    exit 1
  fi
done

echo "exported to $DEST ($(find "$DEST" -type f -not -path '*/target/*' | wc -l | tr -d ' ') files)"
"$SCRIPT_DIR/check_public_tree.sh" "$DEST" "$SCRIPT_DIR/public_denylist.txt"
