#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

exec python3 -B "${SCRIPT_DIR}/interactive_cli.py" --client codex "$@"
