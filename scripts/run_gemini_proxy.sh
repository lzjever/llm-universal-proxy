#!/usr/bin/env bash

set -euo pipefail

cat >&2 <<'EOF'
Native Gemini CLI wiring has been removed from llmup.

Use Gemini only as a Google OpenAI-compatible upstream:
  api_root: https://generativelanguage.googleapis.com/v1beta/openai
  format: openai-completion
EOF
exit 64
