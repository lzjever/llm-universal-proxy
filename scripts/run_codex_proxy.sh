#!/usr/bin/env bash

set -euo pipefail

exec python3 scripts/interactive_cli.py --client codex "$@"
