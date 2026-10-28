#!/usr/bin/env bash
# Content gate for the public tree.
#
#   ./scripts/check_public_tree.sh <dir> [denylist-file]
#
# Fails (exit 1, every hit listed) on:
#   * keypair JSON (a 64-number byte array) or private-key / mnemonic markers;
#   * .env files and keypair-looking filenames;
#   * absolute local paths (/Users/…, /home/…, C:\Users\…);
#   * email addresses;
#   * URLs whose host/path is not on the allowlist (the two upstream repos,
#     the toolchain installers, crate registries);
#   * any pattern in the optional denylist file (one case-insensitive
#     fixed string per line, `#` comments). The denylist itself is kept out
#     of the public tree.
# Build output (`target/`) and the fetched upstream checkout are skipped.
# Cargo.lock is exempt from the URL rule only (its sources are dictated by
# upstream manifests); it is still scanned for secrets, paths and denylist.

set -euo pipefail

DIR="${1:?usage: check_public_tree.sh <dir> [denylist-file]}"
DENY="${2:-}"
FAIL=0

files() {
  # The check script itself is skipped: it necessarily contains its patterns.
  find "$DIR" -type f -not -path '*/target/*' -not -path '*/upstream/*' -not -path '*/.git/*' \
    -not -name check_public_tree.sh
}
hit() { echo "  ${1//$DIR\//}"; FAIL=1; }

echo "== check_public_tree: $DIR"

# 1. secrets
while IFS= read -r f; do
  case "$(basename "$f")" in
    .env|*.env.local|id.json|*keypair*.json) hit "secret-looking file: ${f#$DIR/}" ;;
  esac
  if grep -Eq '^\s*\[\s*([0-9]{1,3}\s*,\s*){63}[0-9]{1,3}\s*\]\s*$' "$f" 2>/dev/null; then
    hit "keypair JSON content: ${f#$DIR/}"
  fi
done < <(files)
while IFS= read -r line; do hit "private key marker: $line"; done < <(
  files | xargs grep -nE -- '-----BEGIN [A-Z ]*PRIVATE KEY|seed phrase|mnemonic:' 2>/dev/null || true)

# 2. absolute local paths
while IFS= read -r line; do hit "absolute path: $line"; done < <(
  files | xargs grep -nE '/Users/[A-Za-z]|/home/[a-z][a-z0-9_-]*/|C:\\Users\\' 2>/dev/null || true)

# 3. emails (noreply addresses of CI bots are not expected either)
while IFS= read -r line; do hit "email: $line"; done < <(
  files | xargs grep -nEo '[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+\.[A-Za-z0-9.-]*[A-Za-z]{2,}' 2>/dev/null \
    | grep -vE '@(PUBLIC_NAME|v[0-9])' || true)

# 4. URLs: allowlist only
ALLOW_URL='^https?://(github\.com/aeyakovenko/percolator(-prog)?([/#.?].*)?$|release\.anza\.xyz/|docs\.rs/|crates\.io/|static\.rust-lang\.org/|sh\.rustup\.rs|github\.com/rust-lang/crates\.io-index$|(127\.0\.0\.1|localhost)(:[0-9]+)?(/.*)?$)'
while IFS= read -r line; do
  url="${line#*:*:}"
  url="${url%%[)\"\'\`>,]*}"
  if ! [[ "$url" =~ $ALLOW_URL ]]; then hit "url not allowlisted: $line"; fi
done < <(files | grep -v '/Cargo\.lock$' | xargs grep -nEo 'https?://[^ )"'"'"'`<>]+' 2>/dev/null || true)

# 5. denylist
if [ -n "$DENY" ]; then
  if [ ! -f "$DENY" ]; then echo "denylist not found: $DENY" >&2; exit 2; fi
  while IFS= read -r pat; do
    pat="${pat%%#*}"; pat="$(echo "$pat" | sed 's/[[:space:]]*$//')"
    [ -z "$pat" ] && continue
    while IFS= read -r line; do hit "denylisted '$pat': $line"; done < <(
      files | xargs grep -niF -- "$pat" 2>/dev/null || true)
  done < "$DENY"
else
  echo "  (no denylist given: generic checks only)"
fi

if [ "$FAIL" -ne 0 ]; then
  echo "== FAIL"
  exit 1
fi
echo "== OK ($(files | wc -l | tr -d ' ') files checked)"
