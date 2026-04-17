#!/usr/bin/env bash
# Compatibility shim for the real CLI matrix harness.
# Preserves the familiar entrypoint while delegating the implementation to Python.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

exec python3 "${SCRIPT_DIR}/real_cli_matrix.py" "$@"
