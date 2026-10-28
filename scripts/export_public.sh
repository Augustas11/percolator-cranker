#!/usr/bin/env bash
# Allowlist export of the public reference-keeper tree.
#
#   ./scripts/export_public.sh <target-dir>
#
# Copies ONLY the manifest's paths, applies the public layout
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
# SECURITY_CONTACT replaces @SECURITY_CONTACT@; without it the export is a
# DRAFT (the check runs with --draft) and is not releasable.
#
# Exactly the files in scripts/public_manifest.txt are exported, read from
# the COMMITTED tree (HEAD), never the working tree. The export fails if a
# tracked file under the exported directories is not in the manifest.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$(cd "$SCRIPT_DIR/.." && pwd)"
DEST="${1:?usage: export_public.sh <target-dir>}"
PUBLIC_NAME="${PUBLIC_NAME:-percolator-reference-keeper}"

if [ -e "$DEST" ] && [ -n "$(ls -A "$DEST" 2>/dev/null | grep -v '^target$' || true)" ]; then
  echo "refusing: $DEST exists and is not empty" >&2
  exit 1
fi
CREATED=0
[ -e "$DEST" ] || CREATED=1
mkdir -p "$DEST"
DEST="$(cd "$DEST" && pwd)"
# A failed export never leaves a half-built tree behind (only a tree this
# run created is removed; an existing empty dir is emptied).
cleanup_on_fail() {
  local rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "export failed (exit $rc); removing partial tree $DEST" >&2
    if [ "$CREATED" -eq 1 ]; then rm -rf -- "$DEST"; else find "$DEST" -mindepth 1 -maxdepth 1 ! -name target -exec rm -rf -- {} +; fi
  fi
}
trap 'cleanup_on_fail; rm -f "${LISTED:-}"' EXIT

# ---- manifest: exactly these files, from the COMMITTED tree (HEAD) -------
MANIFEST="$SCRIPT_DIR/public_manifest.txt"
LISTED="$(mktemp)"   # bash 3.2-compatible set of manifest source paths
while IFS= read -r line || [ -n "$line" ]; do
  line="${line%%#*}"
  [ -z "${line// /}" ] && continue
  read -r from to <<<"$line"
  [ -n "$from" ] && [ -n "$to" ] || { echo "bad manifest line: $line" >&2; exit 1; }
  case "$to" in
    target/*|upstream/*|.git/*|/*|*..*)
      echo "manifest destination not allowed (the gate would skip or escape it): $to" >&2; exit 1 ;;
  esac
  printf '%s\n' "$from" >> "$LISTED"
  if ! git -C "$SRC" cat-file -e "HEAD:$from" 2>/dev/null; then
    echo "manifest file not committed at HEAD: $from" >&2; exit 1
  fi
  mkdir -p "$(dirname "$DEST/$to")"
  git -C "$SRC" show "HEAD:$from" > "$DEST/$to"
  case "$to" in *.sh) chmod +x "$DEST/$to" ;; esac
done < "$MANIFEST"
# Every tracked file in the exported directories must be listed: a new file
# reaches the public tree only by being added to the manifest on purpose.
while IFS= read -r -d '' f; do
  if ! grep -qxF -- "$f" "$LISTED"; then
    echo "tracked file not in $MANIFEST (add it, or keep it private by moving it): $f" >&2
    exit 1
  fi
done < <(git -C "$SRC" ls-files -z -- cranker/src cranker/tests client/src public)

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
PUBLIC_NAME="$PUBLIC_NAME" SECURITY_CONTACT="${SECURITY_CONTACT:-}" python3 - "$DEST" <<'PY'
import os, sys, pathlib
subs = {"@PUBLIC_NAME@": os.environ["PUBLIC_NAME"]}
if os.environ.get("SECURITY_CONTACT"):
    subs["@SECURITY_CONTACT@"] = os.environ["SECURITY_CONTACT"]
for p in pathlib.Path(sys.argv[1]).rglob("*"):
    if p.is_file() and "target" not in p.parts and p.suffix in {".md", ".yml", ".toml", ".sh", ".html"}:
        s = p.read_text()
        t = s
        for k, v in subs.items():
            t = t.replace(k, v)
        if t != s:
            p.write_text(t)
PY
chmod +x "$DEST"/scripts/*.sh

# ---- assert nothing private slipped through -----------------------------
forbidden=(prep.md ROADMAP.md cranker-strategic-questions.md prompts digests signals.md radar.md
  contributors.md watchlist.md releases state repo repo-prog .preview .solana-test-ledger
  public_denylist.txt export_public.sh scenario-findings.md
  # operator stack: private by owner decision
  staging monitor scenario live_shaped.rs live_shaped.sh deploy OPERATOR_PLAYBOOK.md
  STAGING.md MONITOR.md staging_devnet_deploy.sh staging_local.sh)
for name in "${forbidden[@]}"; do
  if [ -n "$(find "$DEST" -name "$name" -not -path '*/target/*' | head -1)" ]; then
    echo "export leaked forbidden path: $name" >&2
    exit 1
  fi
done

echo "exported to $DEST ($(find "$DEST" -type f -not -path '*/target/*' | wc -l | tr -d ' ') files)"
if [ -n "${SECURITY_CONTACT:-}" ]; then
  "$SCRIPT_DIR/check_public_tree.sh" "$DEST" "$SCRIPT_DIR/public_denylist.txt"
else
  echo "DRAFT: SECURITY_CONTACT not set, so @SECURITY_CONTACT@ remains; this tree is not releasable yet."
  "$SCRIPT_DIR/check_public_tree.sh" --draft "$DEST" "$SCRIPT_DIR/public_denylist.txt"
fi
