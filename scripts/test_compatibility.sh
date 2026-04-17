#!/usr/bin/env bash
# ============================================================
# LLM Universal Proxy — Comprehensive Compatibility Test
# ============================================================
# Tests all protocol combinations against real LLM backends.
#
# Prerequisites:
#   1. Build the proxy: cargo build --release
#   2. Start the proxy:
#        ./target/release/llm-universal-proxy --config proxy-test-minimax-and-local.yaml
#   3. Run this script: bash scripts/test_compatibility.sh
#
# Or run with auto-start (script starts/stops proxy for you):
#   bash scripts/test_compatibility.sh --auto-start
# ============================================================

set -euo pipefail

BASE_URL="${BASE_URL:-http://127.0.0.1:18888}"
PASS=0
FAIL=0
SKIP=0
RESULTS=()

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
        -d "$data" 2>/dev/null || echo -e "\n000"
}

# --- Helper: SSE POST, return status + body ---
sse_post() {
    local url="$1"
    local data="$2"
    curl -sS -w "\n%{http_code}" -X POST "$url" \
        -H "Content-Type: application/json" \
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
# MiniMax via Anthropic-compatible upstream
# ============================================================
test_minimax_anthropic() {
    log_header "MiniMax Anthropic Upstream — OpenAI Chat Completions Client"

    test_json \
        "Non-stream: OpenAI Chat → MiniMax Anthropic" \
        "$BASE_URL/openai/v1/chat/completions" \
        '{"model":"minimax-anth","messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":false}' \
        '"content"' '"OK"'

    test_sse \
        "Stream: OpenAI Chat → MiniMax Anthropic" \
        "$BASE_URL/openai/v1/chat/completions" \
        '{"model":"minimax-anth","messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":true}' \
        "data:" "[DONE]"

    log_header "MiniMax Anthropic Upstream — OpenAI Responses Client"

    test_json \
        "Non-stream: OpenAI Responses → MiniMax Anthropic" \
        "$BASE_URL/openai/v1/responses" \
        '{"model":"minimax-anth","input":"Reply with exactly: OK","stream":false}' \
        '"text"'

    test_sse \
        "Stream: OpenAI Responses → MiniMax Anthropic" \
        "$BASE_URL/openai/v1/responses" \
        '{"model":"minimax-anth","input":"Reply with exactly: OK","stream":true}' \
        "response.completed"

    log_header "MiniMax Anthropic Upstream — Anthropic Messages Client"

    test_json \
        "Non-stream: Anthropic Messages → MiniMax Anthropic (passthrough)" \
        "$BASE_URL/anthropic/v1/messages" \
        '{"model":"minimax-anth","max_tokens":256,"messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":false}' \
        '"text"'

    test_sse \
        "Stream: Anthropic Messages → MiniMax Anthropic (passthrough)" \
        "$BASE_URL/anthropic/v1/messages" \
        '{"model":"minimax-anth","max_tokens":64,"messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":true}' \
        "message_start" "message_stop"
}

# ============================================================
# MiniMax via OpenAI-compatible upstream
# ============================================================
test_minimax_openai() {
    log_header "MiniMax OpenAI Upstream — OpenAI Chat Completions Client"

    test_json \
        "Non-stream: OpenAI Chat → MiniMax OpenAI (passthrough)" \
        "$BASE_URL/openai/v1/chat/completions" \
        '{"model":"minimax-openai","messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":false}' \
        '"content"'

    test_sse \
        "Stream: OpenAI Chat → MiniMax OpenAI (passthrough)" \
        "$BASE_URL/openai/v1/chat/completions" \
        '{"model":"minimax-openai","messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":true}' \
        "data:" "[DONE]"

    log_header "MiniMax OpenAI Upstream — OpenAI Responses Client"

    test_json \
        "Non-stream: OpenAI Responses → MiniMax OpenAI" \
        "$BASE_URL/openai/v1/responses" \
        '{"model":"minimax-openai","input":"Reply with exactly: OK","stream":false}' \
        '"text"'

    test_sse \
        "Stream: OpenAI Responses → MiniMax OpenAI" \
        "$BASE_URL/openai/v1/responses" \
        '{"model":"minimax-openai","input":"Reply with exactly: OK","stream":true}' \
        "response.completed"

    log_header "MiniMax OpenAI Upstream — Anthropic Messages Client"

    test_json \
        "Non-stream: Anthropic Messages → MiniMax OpenAI" \
        "$BASE_URL/anthropic/v1/messages" \
        '{"model":"minimax-openai","max_tokens":64,"messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":false}' \
        '"type":"message"'

    test_sse \
        "Stream: Anthropic Messages → MiniMax OpenAI" \
        "$BASE_URL/anthropic/v1/messages" \
        '{"model":"minimax-openai","max_tokens":64,"messages":[{"role":"user","content":"Reply with exactly: OK"}],"stream":true}' \
        "message_start" "message_stop"
}

# ============================================================
# Local qwen3.5-9b-awq via OpenAI-compatible upstream
# ============================================================
test_local_qwen() {
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

    # OpenAI Chat → MiniMax Anthropic (tool call)
    test_json \
        "Tool call: OpenAI Chat → MiniMax Anthropic" \
        "$BASE_URL/openai/v1/chat/completions" \
        '{"model":"minimax-anth","messages":[{"role":"user","content":"What is the weather in Tokyo?"}],"tools":[{"type":"function","function":{"name":"get_weather","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}}],"stream":false}' \
        '"tool_calls"' '"get_weather"'

    # Anthropic Messages → MiniMax OpenAI (tool call)
    test_json \
        "Tool call: Anthropic Messages → MiniMax OpenAI" \
        "$BASE_URL/anthropic/v1/messages" \
        '{"model":"minimax-openai","max_tokens":256,"messages":[{"role":"user","content":"What is the weather in Tokyo?"}],"tools":[{"name":"get_weather","input_schema":{"type":"object","properties":{"city":{"type":"string"}}}}],"stream":false}' \
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
        CONFIG="${CONFIG:-proxy-test-minimax-and-local.yaml}"
        if [ ! -f "$BINARY" ]; then
            echo "Building proxy..."
            cargo build --locked --release
        fi
        $BINARY --config "$CONFIG" &
        PROXY_PID=$!
        echo "Proxy PID: $PROXY_PID"
        trap "kill $PROXY_PID 2>/dev/null; echo 'Proxy stopped.'; exit" EXIT
        if ! wait_for_proxy; then
            echo "Failed to start proxy." >&2
            exit 1
        fi
    fi

    test_health
    test_minimax_anthropic
    test_minimax_openai
    test_local_qwen
    test_tool_calls

    print_summary
}

main "$@"
