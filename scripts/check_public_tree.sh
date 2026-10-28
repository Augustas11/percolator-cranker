#!/usr/bin/env bash
# Content gate for the public tree. FAIL-CLOSED: any scan error is a failure.
#
#   ./scripts/check_public_tree.sh [--draft] <dir> [denylist-file]
#
# Fails (exit 1, every hit listed) on:
#   * file names outside [A-Za-z0-9._/-] (no spaces, quotes, unicode);
#   * symlinks and non-text files (binary, UTF-16, ...);
#   * secret-looking file names (.env*, id.json*, *keypair*.json,
#     wallet*.json, *.pem, *.key), keypair byte arrays (64 numbers in
#     brackets, across any whitespace or line breaks), and private-key /
#     mnemonic markers;
#   * absolute local paths (/Users/, /home/, /root/, /Volumes/,
#     /private/var|tmp/, C:\Users\), case-insensitive;
#   * email addresses;
#   * URLs whose host/path is not on the allowlist (case-insensitive scheme,
#     anchored hosts), and scheme-less github.com/<owner> links for any
#     owner but the upstream author (and the CI actions used);
#   * operator-stack paths (private by owner decision), case-insensitive;
#   * unreplaced template placeholders (@NAME@), unless --draft;
#   * any pattern in the optional denylist (case-insensitive fixed strings,
#     `#` comments). The denylist itself is never exported.
#
# Exemption: a line ending in `// public-check: allow-test-fixture` inside
# crates/*/tests/ may contain URLs/emails whose hosts are loopback or
# documentation placeholders (*.example, *.example.com, *.invalid, *.evil).
# Nothing else is exempt.
#
# Only <dir>/target, <dir>/upstream, <dir>/.git (top level) and this script
# itself (<dir>/scripts/check_public_tree.sh) are skipped.

set -uo pipefail

DRAFT=0
if [ "${1:-}" = "--draft" ]; then DRAFT=1; shift; fi
DIR="${1:?usage: check_public_tree.sh [--draft] <dir> [denylist-file]}"
DENY="${2:-}"
DIR="$(cd "$DIR" && pwd)" || { echo "no such dir" >&2; exit 2; }
FAIL=0
hit() { printf '  %s\n' "$1"; FAIL=1; }

echo "== check_public_tree: $DIR"

# ---- file list (NUL-safe) -------------------------------------------------
FILES=()
while IFS= read -r -d '' f; do
  rel="${f#"$DIR"/}"
  case "$rel" in
    target/*|upstream/*|.git/*|scripts/check_public_tree.sh) continue ;;
  esac
  FILES+=("$rel")
done < <(find "$DIR" \( -type f -o -type l \) -print0)

SAFE_FILES=()
for rel in "${FILES[@]}"; do
  if [[ ! "$rel" =~ ^[A-Za-z0-9._/-]+$ ]]; then
    hit "bad file name (allowed: A-Za-z0-9._/-): $(printf '%q' "$rel")"
    continue
  fi
  if [ -L "$DIR/$rel" ]; then hit "symlink: $rel"; continue; fi
  if [ -s "$DIR/$rel" ] && ! LC_ALL=C grep -qI . "$DIR/$rel"; then
    hit "non-text file: $rel"
    continue
  fi
  SAFE_FILES+=("$rel")
done

# grep wrapper over relative paths: -H file names, -n lines. Exit status 2+
# is a scan error and fails the check; it is never swallowed.
g() {
  local out rc
  out="$(cd "$DIR" && LC_ALL=C grep -Hn "$@")"; rc=$?
  if [ "$rc" -ge 2 ]; then hit "SCAN ERROR (grep exit $rc)"; fi
  if [ -n "$out" ]; then printf '%s\n' "$out"; fi
  return 0
}

if [ "${#SAFE_FILES[@]}" -gt 0 ]; then

FIXTURE='// public-check: allow-test-fixture'
fixture_ok() { # "<file>:<line>:<text>" -> 0 for a valid, narrowly scoped fixture line
  local l="$1" file="${1%%:*}" h
  [[ "$file" =~ ^crates/[A-Za-z0-9_-]+/tests/ ]] || return 1
  [[ "$l" == *"$FIXTURE" ]] || return 1
  while IFS= read -r h; do
    [ -z "$h" ] && continue
    h="$(printf '%s' "$h" | tr 'A-Z' 'a-z')"
    [[ "$h" == "127.0.0.1" || "$h" == "localhost" || "$h" == "[" ||
       "$h" =~ \.(example|example\.com|invalid|evil)$ ]] || return 1
  done < <(printf '%s\n' "$l" | grep -oiE '(https?://|@)[A-Za-z0-9.[-]+' | sed -E 's#^(https?://|@)##I')
  return 0
}

# 1. secrets
shopt -s nocasematch
for rel in "${SAFE_FILES[@]}"; do
  base="${rel##*/}"
  if [[ "$base" =~ ^\.env(\..*)?$ || "$base" =~ ^id\.json || "$base" =~ keypair.*\.json$ ||
        "$base" =~ ^wallet.*\.json$ || "$base" =~ \.(pem|key)$ ]]; then
    hit "secret-looking file: $rel"
  fi
done
shopt -u nocasematch
for rel in "${SAFE_FILES[@]}"; do
  if tr -d ' \t\r\n' < "$DIR/$rel" | grep -qE '\[([0-9]{1,3},){63}[0-9]{1,3}\]'; then
    hit "keypair-like 64-byte array: $rel"
  fi
done
while IFS= read -r l; do [ -n "$l" ] && hit "private key marker: $l"; done < <(
  g -iE -- '-----BEGIN [A-Z ]*PRIVATE KEY|seed phrase:|mnemonic:' "${SAFE_FILES[@]}")

# 2. absolute local paths
while IFS= read -r l; do [ -n "$l" ] && hit "absolute path: $l"; done < <(
  g -iE -- '/(users|home)/[A-Za-z]|/root/|/Volumes/|/private/(var|tmp)/|C:\\Users\\' "${SAFE_FILES[@]}")

# 3. emails
EMAIL='[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+\.[A-Za-z0-9.-]*[A-Za-z]{2,}'
while IFS= read -r l; do
  [ -z "$l" ] && continue
  fixture_ok "$l" || hit "email: $l"
done < <(g -E -- "$EMAIL" "${SAFE_FILES[@]}")

# 4. URLs (Cargo.lock exempt: its sources come from upstream manifests)
ALLOW_URL='^https?://(github\.com/aeyakovenko/percolator(-prog)?([/#?.]|$)|(release\.anza\.xyz|docs\.rs|crates\.io|static\.rust-lang\.org|sh\.rustup\.rs)(/|$)|github\.com/rust-lang/crates\.io-index$|(127\.0\.0\.1|localhost|\[::1\])(:[0-9]+)?(/|$))'
NONLOCK=()
for rel in "${SAFE_FILES[@]}"; do [[ "${rel##*/}" == Cargo.lock ]] || NONLOCK+=("$rel"); done
if [ "${#NONLOCK[@]}" -gt 0 ]; then
  while IFS= read -r l; do
    [ -z "$l" ] && continue
    fixture_ok "$l" && continue
    loc="$(printf '%s' "$l" | cut -d: -f1-2)"
    while IFS= read -r url; do
      [ -z "$url" ] && continue
      url="${url%%[)\"\'\`>,]*}"
      lower="$(printf '%s' "$url" | tr 'A-Z' 'a-z')"
      [[ "$lower" =~ $ALLOW_URL ]] || hit "url not allowlisted: $loc: $url"
    done < <(printf '%s\n' "$l" | grep -oiE 'https?://[^ )"'"'"'`<>]+')
  done < <(g -iE -- 'https?://' "${NONLOCK[@]}")
  while IFS= read -r l; do
    [ -z "$l" ] && continue
    fixture_ok "$l" || hit "github link to a non-upstream owner: $l"
  done < <(g -iE -- 'github\.com/[A-Za-z0-9_.-]+' "${NONLOCK[@]}" |
           grep -viE 'github\.com/(aeyakovenko/percolator|rust-lang/crates\.io-index)' || true)
fi

# 5. operator stack (private by owner decision): paths, case-insensitive
OP='(^|/)(staging|monitor|scenario|deploy|trader|matcher|backing[_-]?manager|load[_-]?test|devnet|hosting|operator[_-]?playbook|live[_-]?shaped)[a-z0-9_.-]*(/|$)|(^|/)[a-z0-9_.-]*(operator[_-]?playbook|live[_-]?shaped|staging[_-]|devnet[_-]|backing[_-]?manager)[a-z0-9_.-]*$|(^|/)(alerts|dashboard)\.[a-z]+$'
shopt -s nocasematch
for rel in "${SAFE_FILES[@]}"; do
  [[ "$rel" =~ $OP ]] && hit "operator-stack path: $rel"
done
shopt -u nocasematch

# 6. unreplaced placeholders
if [ "$DRAFT" -eq 0 ]; then
  while IFS= read -r l; do [ -n "$l" ] && hit "unreplaced placeholder: $l"; done < <(
    g -E -- '@[A-Z][A-Z_]+@' "${SAFE_FILES[@]}")
fi

# 7. denylist
if [ -n "$DENY" ]; then
  if [ ! -f "$DENY" ]; then echo "denylist not found: $DENY" >&2; exit 2; fi
  while IFS= read -r pat || [ -n "$pat" ]; do
    pat="${pat%%#*}"; pat="$(printf '%s' "$pat" | sed 's/[[:space:]]*$//')"
    [ -z "$pat" ] && continue
    while IFS= read -r l; do [ -n "$l" ] && hit "denylisted '$pat': $l"; done < <(
      g -iF -- "$pat" "${SAFE_FILES[@]}")
    # file names too
    for rel in "${SAFE_FILES[@]}"; do
      if printf '%s' "$rel" | grep -qiF -- "$pat"; then hit "denylisted '$pat' in file name: $rel"; fi
    done
  done < "$DENY"
else
  echo "  (no denylist given: generic checks only)"
fi

fi

if [ "$FAIL" -ne 0 ]; then
  echo "== FAIL"
  exit 1
fi
echo "== OK (${#SAFE_FILES[@]} files checked)"
