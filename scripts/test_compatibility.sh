#!/usr/bin/env bash
# ============================================================
# LLM Universal Proxy — Comprehensive Compatibility Test
# ============================================================
# Tests all protocol combinations against real LLM backends.
#
# Prerequisites:
#   1. Build the proxy: cargo build --release
#   2. Configure provider-neutral preset vars in .env.test:
#        PRESET_OPENAI_ENDPOINT_BASE_URL, PRESET_ANTHROPIC_ENDPOINT_BASE_URL,
#        PRESET_ENDPOINT_MODEL, PRESET_ENDPOINT_API_KEY
#   3. Start the proxy with a rendered runtime config, or use auto-start below.
#   4. Run this script: bash scripts/test_compatibility.sh
#
# Or run with auto-start (script starts/stops proxy for you):
#   bash scripts/test_compatibility.sh --auto-start
# ============================================================

set -euo pipefail

BASE_URL="${BASE_URL:-http://127.0.0.1:18888}"
DEFAULT_CONFIG="scripts/fixtures/cli_matrix/default_proxy_test_matrix.yaml"
DEFAULT_ENV_FILE=".env.test"
PROXY_KEY="${LLM_UNIVERSAL_PROXY_KEY:-llmup-compat-proxy-key}"
PASS=0
FAIL=0
SKIP=0
RESULTS=()
AUTO_START_RUNTIME_DIR=""
AUTO_START_RUNTIME_CONFIG=""

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

log_pass() { echo -e "${GREEN}[PASS]${NC} $1"; PASS=$((PASS+1)); RESULTS+=("PASS: $1"); }
log_fail() { echo -e "${RED}[FAIL]${NC} $1 — $2"; FAIL=$((FAIL+1)); RESULTS+=("FAIL: $1 — $2"); }
log_skip() { echo -e "${YELLOW}[SKIP]${NC} $1"; SKIP=$((SKIP+1)); RESULTS+=("SKIP: $1"); }
log_header() { echo -e "\n${CYAN}=== $1 ===${NC}"; }

# --- Helper: HTTP POST, return status + body ---
http_post() {
    local url="$1"
    local data="$2"
    curl -sS -w "\n%{http_code}" -X POST "$url" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer ${PROXY_KEY}" \
        -d "$data" 2>/dev/null || echo -e "\n000"
}

# --- Helper: SSE POST, return status + body ---
sse_post() {
    local url="$1"
    local data="$2"
    curl -sS -w "\n%{http_code}" -X POST "$url" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer ${PROXY_KEY}" \
        -d "$data" 2>/dev/null || echo -e "\n000"
}

# --- Wait for proxy to be healthy ---
wait_for_proxy() {
    local max_wait=15
    local elapsed=0
    while [ $elapsed -lt $max_wait ]; do
        if curl -sS "$BASE_URL/health" 2>/dev/null | grep -q "ok"; then
            echo "Proxy is healthy."
            return 0
        fi
        sleep 0.5
        elapsed=$((elapsed + 1))
    done
    echo "Proxy did not become healthy within ${max_wait}s" >&2
    return 1
}

load_auto_start_env_file() {
    local env_file="$1"
    if [ ! -f "$env_file" ]; then
        return 0
    fi
    set -a
    # shellcheck source=/dev/null
    source "$env_file"
    set +a
}

cleanup_auto_start_runtime_config() {
    if [ -n "${AUTO_START_RUNTIME_DIR:-}" ] && [ -d "$AUTO_START_RUNTIME_DIR" ]; then
        rm -rf "$AUTO_START_RUNTIME_DIR"
    fi
    AUTO_START_RUNTIME_DIR=""
    AUTO_START_RUNTIME_CONFIG=""
}

render_auto_start_config() {
    local config_source="${1:-$DEFAULT_CONFIG}"
    local env_file="${2:-$DEFAULT_ENV_FILE}"
    local runtime_dir
    runtime_dir=$(mktemp -d "${TMPDIR:-/tmp}/llmup-compat-runtime.XXXXXX")
    AUTO_START_RUNTIME_DIR="$runtime_dir"
    AUTO_START_RUNTIME_CONFIG="$runtime_dir/runtime-config.yaml"
    local trace_path="$runtime_dir/debug-trace.jsonl"

    if ! python3 - "$config_source" "$env_file" "$AUTO_START_RUNTIME_CONFIG" "$trace_path" "$BASE_URL" <<'PY'
import importlib.util
import os
import pathlib
import sys
import urllib.parse

config_source = pathlib.Path(sys.argv[1])
env_file = pathlib.Path(sys.argv[2])
runtime_config = pathlib.Path(sys.argv[3])
trace_path = pathlib.Path(sys.argv[4])
base_url = sys.argv[5]
repo_root = pathlib.Path.cwd()
script_path = repo_root / "scripts" / "real_cli_matrix.py"

spec = importlib.util.spec_from_file_location("real_cli_matrix_for_compat", script_path)
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

parsed = module.parse_proxy_source(config_source.read_text(encoding="utf-8"))
dotenv_env = module.load_dotenv_file(env_file)
for key in module.required_preset_endpoint_env_keys(parsed):
    if os.environ.get(key):
        dotenv_env[key] = os.environ[key]
for key in ("LOCAL_QWEN_BASE_URL", "LOCAL_QWEN_MODEL", "LOCAL_QWEN_API_KEY"):
    if os.environ.get(key):
        dotenv_env[key] = os.environ[key]
module.validate_preset_endpoint_env(parsed, dotenv_env)

parsed_url = urllib.parse.urlparse(base_url)
listen_host = parsed_url.hostname or "127.0.0.1"
if parsed_url.port is not None:
    listen_port = parsed_url.port
elif parsed_url.scheme == "https":
    listen_port = 443
else:
    listen_port = 80

runtime_config.write_text(
    module.build_runtime_config_text(
        parsed,
        dotenv_env,
        listen_host=listen_host,
        listen_port=listen_port,
        trace_path=trace_path,
    ),
    encoding="utf-8",
)
PY
    then
        cleanup_auto_start_runtime_config
        return 1
    fi

    return 0
}

# ============================================================
# Test functions
# ============================================================

# $1 = label, $2 = url, $3 = json body, $4.. = expected markers in response body
test_json() {
    local label="$1" url="$2" body="$3"; shift 3
    local response
    response=$(http_post "$url" "$body")
    local status=$(echo "$response" | tail -1)
    local body_content=$(echo "$response" | sed '$d')

    if [ "$status" != "200" ]; then
        log_fail "$label" "HTTP $status — $(echo "$body_content" | head -c 200)"
        return
    fi

    for marker in "$@"; do
        if ! echo "$body_content" | grep -Fq "$marker"; then
            log_fail "$label" "Missing marker: $marker — $(echo "$body_content" | head -c 300)"
            return
        fi
    done
    log_pass "$label"
}

# $1 = label, $2 = url, $3 = json body, $4.. = expected markers in SSE stream
test_sse() {
    local label="$1" url="$2" body="$3"; shift 3
    local response
    response=$(sse_post "$url" "$body")
    local status=$(echo "$response" | tail -1)
    local body_content=$(echo "$response" | sed '$d')

    if [ "$status" != "200" ]; then
        log_fail "$label" "HTTP $status — $(echo "$body_content" | head -c 200)"
        return
    fi

    for marker in "$@"; do
        if ! echo "$body_content" | grep -q "$marker"; then
            log_fail "$label" "Missing marker: $marker — $(echo "$body_content" | head -c 300)"
            return
        fi
    done
    log_pass "$label"
}

# ============================================================
# Health Check
# ============================================================
test_health() {
    log_header "Health Check"
    local status
    status=$(curl -sS -o /dev/null -w "%{http_code}" "$BASE_URL/health" 2>/dev/null || echo "000")
    if [ "$status" = "200" ]; then
        log_pass "GET /health → 200"
    else
        log_fail "GET /health" "HTTP $status"
    fi
}

# ============================================================
# Preset Anthropic-compatible upstream
# ============================================================
test_preset_anthropic_compatible() {
    log_header "Preset Anthropic-compatible Upstream — OpenAI Chat Completions Client"

    test_json \
        "Non-stream: OpenAI Chat → preset Anthropic-compatible" \
        "$BASE_URL/openai/v1/chat/completions" \
        '{"model":"preset-anthropic-compatible","messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":false}' \
        '"content"' '"OK"'

    test_sse \
        "Stream: OpenAI Chat → preset Anthropic-compatible" \
        "$BASE_URL/openai/v1/chat/completions" \
        '{"model":"preset-anthropic-compatible","messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":true}' \
        "data:" "[DONE]"

    log_header "Preset Anthropic-compatible Upstream — OpenAI Responses Client"

    test_json \
        "Non-stream: OpenAI Responses → preset Anthropic-compatible" \
        "$BASE_URL/openai/v1/responses" \
        '{"model":"preset-anthropic-compatible","input":"Reply with exactly: OK","stream":false}' \
        '"text"'

    test_sse \
        "Stream: OpenAI Responses → preset Anthropic-compatible" \
        "$BASE_URL/openai/v1/responses" \
        '{"model":"preset-anthropic-compatible","input":"Reply with exactly: OK","stream":true}' \
        "response.completed"

    log_header "Preset Anthropic-compatible Upstream — Anthropic Messages Client"

    test_json \
        "Non-stream: Anthropic Messages → preset Anthropic-compatible (passthrough)" \
        "$BASE_URL/anthropic/v1/messages" \
        '{"model":"preset-anthropic-compatible","max_tokens":256,"messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":false}' \
        '"text"'

    test_sse \
        "Stream: Anthropic Messages → preset Anthropic-compatible (passthrough)" \
        "$BASE_URL/anthropic/v1/messages" \
        '{"model":"preset-anthropic-compatible","max_tokens":64,"messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":true}' \
        "message_start" "message_stop"
}

# ============================================================
# Preset OpenAI-compatible upstream
# ============================================================
test_preset_openai_compatible() {
    log_header "Preset OpenAI-compatible Upstream — OpenAI Chat Completions Client"

    test_json \
        "Non-stream: OpenAI Chat → preset OpenAI-compatible (passthrough)" \
        "$BASE_URL/openai/v1/chat/completions" \
        '{"model":"preset-openai-compatible","messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":false}' \
        '"content"'

    test_sse \
        "Stream: OpenAI Chat → preset OpenAI-compatible (passthrough)" \
        "$BASE_URL/openai/v1/chat/completions" \
        '{"model":"preset-openai-compatible","messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":true}' \
        "data:" "[DONE]"

    log_header "Preset OpenAI-compatible Upstream — OpenAI Responses Client"

    test_json \
        "Non-stream: OpenAI Responses → preset OpenAI-compatible" \
        "$BASE_URL/openai/v1/responses" \
        '{"model":"preset-openai-compatible","input":"Reply with exactly: OK","stream":false}' \
        '"text"'

    test_sse \
        "Stream: OpenAI Responses → preset OpenAI-compatible" \
        "$BASE_URL/openai/v1/responses" \
        '{"model":"preset-openai-compatible","input":"Reply with exactly: OK","stream":true}' \
        "response.completed"

    log_header "Preset OpenAI-compatible Upstream — Anthropic Messages Client"

    test_json \
        "Non-stream: Anthropic Messages → preset OpenAI-compatible" \
        "$BASE_URL/anthropic/v1/messages" \
        '{"model":"preset-openai-compatible","max_tokens":64,"messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":false}' \
        '"type":"message"'

    test_sse \
        "Stream: Anthropic Messages → preset OpenAI-compatible" \
        "$BASE_URL/anthropic/v1/messages" \
        '{"model":"preset-openai-compatible","max_tokens":64,"messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":true}' \
        "message_start" "message_stop"
}

# ============================================================
# Local qwen3.5-9b-awq via OpenAI-compatible upstream
# ============================================================
test_local_qwen() {
    if [ -z "${LOCAL_QWEN_BASE_URL:-}" ] || [ -z "${LOCAL_QWEN_MODEL:-}" ] || [ -z "${LOCAL_QWEN_API_KEY:-}" ]; then
        log_header "Local qwen3.5 Upstream"
        log_skip "Local qwen tests require LOCAL_QWEN_BASE_URL, LOCAL_QWEN_MODEL, and LOCAL_QWEN_API_KEY"
        return 0
    fi

    log_header "Local qwen3.5 Upstream — OpenAI Chat Completions Client"

    test_json \
        "Non-stream: OpenAI Chat → Local qwen (passthrough)" \
        "$BASE_URL/openai/v1/chat/completions" \
        '{"model":"qwen-local","messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":false}' \
        '"content"'

    test_sse \
        "Stream: OpenAI Chat → Local qwen (passthrough)" \
        "$BASE_URL/openai/v1/chat/completions" \
        '{"model":"qwen-local","messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":true}' \
        "data:" "[DONE]"

    log_header "Local qwen3.5 Upstream — OpenAI Responses Client"

    test_json \
        "Non-stream: OpenAI Responses → Local qwen" \
        "$BASE_URL/openai/v1/responses" \
        '{"model":"qwen-local","input":"Reply with exactly: OK","stream":false}' \
        '"text"'

    test_sse \
        "Stream: OpenAI Responses → Local qwen" \
        "$BASE_URL/openai/v1/responses" \
        '{"model":"qwen-local","input":"Reply with exactly: OK","stream":true}' \
        "response.completed"

    log_header "Local qwen3.5 Upstream — Anthropic Messages Client"

    test_json \
        "Non-stream: Anthropic Messages → Local qwen" \
        "$BASE_URL/anthropic/v1/messages" \
        '{"model":"qwen-local","max_tokens":64,"messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":false}' \
        '"type":"message"'

    test_sse \
        "Stream: Anthropic Messages → Local qwen" \
        "$BASE_URL/anthropic/v1/messages" \
        '{"model":"qwen-local","max_tokens":64,"messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":true}' \
        "message_start" "message_stop"
}

# ============================================================
# Tool call / function calling tests
# ============================================================
test_tool_calls() {
    log_header "Tool Call Translation"

    # OpenAI Chat → preset Anthropic-compatible (tool call)
    test_json \
        "Tool call: OpenAI Chat → preset Anthropic-compatible" \
        "$BASE_URL/openai/v1/chat/completions" \
        '{"model":"preset-anthropic-compatible","messages":[{"role":"user","content":"What is the weather in Tokyo?"}],"tools":[{"type":"function","function":{"name":"get_weather","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}}],"stream":false}' \
        '"tool_calls"' '"get_weather"'

    # Anthropic Messages → preset OpenAI-compatible (tool call)
    test_json \
        "Tool call: Anthropic Messages → preset OpenAI-compatible" \
        "$BASE_URL/anthropic/v1/messages" \
        '{"model":"preset-openai-compatible","max_tokens":256,"messages":[{"role":"user","content":"What is the weather in Tokyo?"}],"tools":[{"name":"get_weather","input_schema":{"type":"object","properties":{"city":{"type":"string"}}}}],"stream":false}' \
        '"tool_use"' '"get_weather"'
}

# ============================================================
# Summary
# ============================================================
print_summary() {
    log_header "Test Summary"
    echo ""
    for r in "${RESULTS[@]}"; do
        if [[ "$r" == PASS* ]]; then
            echo -e "  ${GREEN}$r${NC}"
        elif [[ "$r" == FAIL* ]]; then
            echo -e "  ${RED}$r${NC}"
        else
            echo -e "  ${YELLOW}$r${NC}"
        fi
    done
    echo ""
    echo -e "Total: ${GREEN}$PASS passed${NC}, ${RED}$FAIL failed${NC}, ${YELLOW}$SKIP skipped${NC}"
    echo ""

    if [ $FAIL -gt 0 ]; then
        echo -e "${RED}Some tests failed. Check output above.${NC}"
        return 1
    else
        echo -e "${GREEN}All tests passed!${NC}"
        return 0
    fi
}

# ============================================================
# Main
# ============================================================
main() {
    echo "LLM Universal Proxy — Compatibility Test"
    echo "Target: $BASE_URL"
    echo ""

    if [ "${1:-}" = "--auto-start" ]; then
        echo "Auto-starting proxy..."
        BINARY="${BINARY:-./target/release/llm-universal-proxy}"
        CONFIG="${CONFIG:-$DEFAULT_CONFIG}"
        ENV_FILE="${ENV_FILE:-$DEFAULT_ENV_FILE}"
        if [ ! -f "$BINARY" ]; then
            echo "Building proxy..."
            cargo build --locked --release
        fi
        load_auto_start_env_file "$ENV_FILE"
        export LLM_UNIVERSAL_PROXY_AUTH_MODE="proxy_key"
        export LLM_UNIVERSAL_PROXY_KEY="$PROXY_KEY"
        render_auto_start_config "$CONFIG" "$ENV_FILE"
        RUNTIME_CONFIG="$AUTO_START_RUNTIME_CONFIG"
        $BINARY --config "$RUNTIME_CONFIG" &
        PROXY_PID=$!
        echo "Proxy PID: $PROXY_PID"
        trap 'status=$?; kill "$PROXY_PID" 2>/dev/null; cleanup_auto_start_runtime_config; echo "Proxy stopped."; exit "$status"' EXIT
        if ! wait_for_proxy; then
            echo "Failed to start proxy." >&2
            exit 1
        fi
    fi

    test_health
    test_preset_anthropic_compatible
    test_preset_openai_compatible
    test_local_qwen
    test_tool_calls

    print_summary
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi
