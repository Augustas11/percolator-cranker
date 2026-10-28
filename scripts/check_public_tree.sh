#!/usr/bin/env bash
# Content gate for the public tree; see check_public_tree.py for the rules.
#
#   ./scripts/check_public_tree.sh [--draft] [--allow-email ADDR] <dir> [denylist-file]
set -euo pipefail
exec python3 -I "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/check_public_tree.py" "$@"
