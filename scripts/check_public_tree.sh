#!/usr/bin/env bash
# Leak helper for the exported public tree: a plain pattern check.
#
#   ./scripts/check_public_tree.sh [--draft] [--allow-email ADDR] [--ipv4]
#       [--keep-placeholder NAME]... [--allow-phrase WORDS]... <dir> [denylist-file]
#
# This is an advisory helper, not a security boundary. What leaves the
# private repo is decided by the allowlist export (only manifest files are
# copied) and by the owner reading the exported tree before the first push.
# This script catches the obvious mistakes:
#   - private-key shapes: a 64-number JSON/Rust array, a PEM private-key
#     header, keypair / id.json / .env file names;
#   - home-directory paths;
#   - email addresses (except --allow-email ADDR, the security contact, and
#     the reserved documentation domains example.*, .invalid, .test, .localhost);
#   - every fixed string in the denylist (owner handles, the private repo
#     name and URL, private doc names), in contents and file names;
#   - unreplaced template placeholders (at-sign NAME at-sign), unless --draft;
#     --keep-placeholder NAME allows that one on purpose;
#   - with --ipv4: IPv4 literals other than loopback, private, unspecified
#     and documentation ranges (a server address);
#   - --allow-phrase WORDS: a denylisted string inside a whole-word occurrence
#     of WORDS (case-insensitive; letters/digits/space/-/_ only) is not a hit,
#     e.g. a product name that contains a denylisted word. Every other
#     occurrence on the line still is.
# Text files only: grep -I skips binaries. Top-level target/, upstream/ and
# .git/ are skipped. Exits 1 on any hit, 2 on bad usage.
set -euo pipefail

draft=0
ipv4=0
allow_email=""
keep=""
phrases=""
args=()
while [ $# -gt 0 ]; do
  case "$1" in
    --draft) draft=1 ;;
    --ipv4) ipv4=1 ;;
    --allow-email) allow_email="${2:-}"; shift ;;
    --keep-placeholder)
      [[ "${2:-}" =~ ^[A-Z][A-Z_]+$ ]] || { echo "bad --keep-placeholder: ${2:-}" >&2; exit 2; }
      keep="${keep:+$keep|}$2"; shift ;;
    --allow-phrase)
      [[ "${2:-}" =~ ^[A-Za-z0-9\ _-]+$ ]] || { echo "bad --allow-phrase: ${2:-}" >&2; exit 2; }
      phrases="$phrases$(printf '%s' "$2" | tr '[:upper:]' '[:lower:]')"$'\n'; shift ;;
    *) args+=("$1") ;;
  esac
  shift
done
if [ "${#args[@]}" -lt 1 ] || [ ! -d "${args[0]}" ]; then
  echo "usage: check_public_tree.sh [--draft] [--allow-email ADDR] <dir> [denylist]" >&2
  exit 2
fi
root="${args[0]}"
deny="${args[1]:-}"
if [ -n "$deny" ] && [ ! -f "$deny" ]; then
  echo "denylist not found: $deny" >&2
  exit 2
fi
if [ -n "$deny" ]; then
  deny="$(cd "$(dirname "$deny")" && pwd)/$(basename "$deny")"  # absolute: we cd below
fi

echo "== check_public_tree: $root"
cd "$root"
hits=0
report() { # label, then matching lines on stdin
  local label="$1" line
  while IFS= read -r line; do
    echo "  $label: $line"
    hits=$((hits + 1))
  done
}
scan() { # label, grep flags and pattern...
  local label="$1"
  shift
  report "$label" < <(grep -rnI --exclude-dir=.git "$@" . | grep -Ev '^\./(target|upstream)/' || true)
}

files() { find . -path ./.git -prune -o -path ./target -prune -o -path ./upstream -prune -o -type f -print; }

report "secret-looking file" < <(files | grep -Ei '/(\.env[^/]*|id\.json[^/]*|[^/]*keypair[^/]*\.json|[^/]*\.pem)$' || true)
scan "64-number key array" -E '\[[[:space:]]*([0-9]{1,3}[[:space:]]*,[[:space:]]*){63}[0-9]{1,3}[[:space:]]*,?[[:space:]]*\]'
scan "private key header" -E 'BEGIN [A-Z ]*PRIVATE KEY'
scan "home path" -E '/(Users|home)/[^/[:space:]]'
email_hits() {
  grep -rnIoE --exclude-dir=.git '[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}' . \
    | grep -Ev '^\./(target|upstream)/' \
    | grep -Ev '@([A-Za-z0-9-]+\.)*(example(\.(com|org|net))?|invalid|test|localhost)$' || true
}
# grep -o lines are ./file:line:address; the approved contact may appear in SECURITY.md only.
report "email" < <(email_hits | awk -F: -v ok="$allow_email" '!(ok != "" && $1 == "./SECURITY.md" && $3 == ok)')
if [ "$draft" = 0 ]; then
  if [ -n "$keep" ]; then
    report "unreplaced placeholder" < <(grep -rnI --exclude-dir=.git -E '@[A-Z][A-Z_]+@' . \
      | grep -Ev '^\./(target|upstream)/' | sed -E "s/@($keep)@//g" | grep -E '@[A-Z][A-Z_]+@' || true)
  else
    scan "unreplaced placeholder" -E '@[A-Z][A-Z_]+@'
  fi
fi
if [ "$ipv4" = 1 ]; then
  # ./file:line:a.b.c.d; drop loopback, 0/8, 10/8, 172.16/12, 192.168/16,
  # documentation nets, out-of-range octets and leading zeros (not addresses).
  # Every dotted quad on each line; drop loopback, 0/8, 10/8, 172.16/12,
  # 192.168/16, documentation nets, out-of-range octets, leading zeros and
  # longer dotted runs (versions), all checked per match, not per line.
  report "ipv4 literal" < <(grep -rnIE --exclude-dir=.git '[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+' . \
    | grep -Ev '^\./(target|upstream)/' \
    | awk '{
        line = $0; pos = 1
        while (match(substr(line, pos), /[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+/)) {
          s = pos + RSTART - 1; ip = substr(line, s, RLENGTH); pos = s + RLENGTH
          b = (s > 1) ? substr(line, s - 1, 1) : ""; a = substr(line, pos, 1); a2 = substr(line, pos + 1, 1)
          if (b ~ /[0-9.]/ || a ~ /[0-9]/ || (a == "." && a2 ~ /[0-9]/)) continue
          split(ip, o, "."); bad = 0
          for (i = 1; i <= 4; i++) if (o[i] + 0 > 255 || (length(o[i]) > 1 && substr(o[i], 1, 1) == "0")) bad = 1
          if (bad) continue
          if (o[1] == 127 || o[1] == 0 || o[1] == 10 || (o[1] == 192 && o[2] == 168)) continue
          if (o[1] == 172 && o[2] >= 16 && o[2] <= 31) continue
          if ((o[1] == 192 && o[2] == 0 && o[3] == 2) || (o[1] == 198 && o[2] == 51 && o[3] == 100) || (o[1] == 203 && o[2] == 0 && o[3] == 113)) continue
          print ip " in " line
        }
      }' || true)
fi
if [ -n "$deny" ]; then
  patterns="$(sed 's/#.*//; s/^[[:space:]]*//; s/[[:space:]]*$//; /^$/d' "$deny")" \
    || { echo "cannot read denylist: $deny" >&2; exit 2; }
  if [ -n "$patterns" ]; then
    if [ -n "$phrases" ]; then
      # A hit is allowed only where an allowed phrase, bounded by non-letters
      # and non-digits on both sides, covers that occurrence; every other
      # occurrence of every pattern on the line still counts.
      report "denylisted" < <(grep -rnI --exclude-dir=.git -iF -e "$patterns" . \
        | grep -Ev '^\./(target|upstream)/' \
        | PH="$phrases" PAT="$patterns" awk '
          function word(c) { return c ~ /[a-z0-9]/ }
          function covered(l, s, e,   k, ph, j, a, b, off) {
            for (k = 1; k <= np; k++) {
              ph = P[k]; if (ph == "") continue
              off = 0
              while ((j = index(substr(l, off + 1), ph)) > 0) {
                a = off + j; b = a + length(ph) - 1; off = a
                if (a > s || b < e) continue
                if (a > 1 && word(substr(l, a - 1, 1))) continue
                if (word(substr(l, b + 1, 1))) continue
                return 1
              }
            }
            return 0
          }
          BEGIN { np = split(ENVIRON["PH"], P, "\n"); nq = split(tolower(ENVIRON["PAT"]), Q, "\n") }
          {
            l = tolower($0)
            for (i = 1; i <= nq; i++) {
              q = Q[i]; if (q == "") continue
              off = 0
              while ((j = index(substr(l, off + 1), q)) > 0) {
                st = off + j; off = st
                if (!covered(l, st, st + length(q) - 1)) { print; next }
              }
            }
          }' || true)
    else
      scan "denylisted" -iF -e "$patterns"
    fi
    report "denylisted file name" < <(files | grep -iF -e "$patterns" || true)
  fi
else
  echo "  (no denylist given: generic checks only)"
fi

if [ "$hits" -gt 0 ]; then
  echo "== FAIL ($hits hits)"
  exit 1
fi
echo "== OK"
