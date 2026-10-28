#!/usr/bin/env bash
# Allowlist export of the public reference-keeper tree; see export_public.py.
#
#   ./scripts/export_public.sh <target-dir-that-does-not-exist>
set -euo pipefail
exec python3 -I "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/export_public.py" "$@"
