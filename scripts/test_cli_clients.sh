#!/usr/bin/env bash
# ============================================================
# LLM Universal Proxy — E2E Tests with Real CLI Clients
# ============================================================
# Tests Codex CLI (Responses API) and Claude Code (Anthropic Messages API)
# through the proxy against multiple upstreams.
#
# Usage:
#   bash scripts/test_cli_clients.sh                       # run all tests
#   bash scripts/test_cli_clients.sh --test codex_basic    # run specific phase
#   bash scripts/test_cli_clients.sh --skip-slow            # skip multi-turn tests
#   bash scripts/test_cli_clients.sh --proxy-only           # just start proxy, don't test
#
# Prerequisites:
#   - codex CLI installed (npm install -g @openai/codex)
#   - claude CLI installed
#   - proxy built: cargo build --release
#   - proxy config: proxy-test-minimax-and-local.yaml
#
# Cross-protocol translation matrix exercised:
#   codex   → minimax-anth    : Responses → Anthropic   (full translation)
#   codex   → minimax-openai  : Responses → OpenAI CC  (sub-format translation)
#   codex   → qwen-local      : Responses → OpenAI CC  (translation)
#   claude  → minimax-anth    : Anthropic → Anthropic  (passthrough)
#   claude  → minimax-openai  : Anthropic → OpenAI CC  (full translation)
#   claude  → qwen-local      : Anthropic → OpenAI CC  (translation)
# ============================================================

set -euo pipefail

# ── Config ──
PROXY_HOST="127.0.0.1"
PROXY_PORT="${PROXY_PORT:-18888}"
PROXY_BASE="http://${PROXY_HOST}:${PROXY_PORT}"
PROXY_CONFIG="proxy-test-minimax-and-local.yaml"
PROXY_BIN="${PROXY_BIN:-./target/release/llm-universal-proxy}"
TIMEOUT_SINGLE=90
TIMEOUT_MULTI=180
CODEX_BASE_URL="${PROXY_BASE}/openai/v1"
# Claude Code appends /v1/messages itself, so the base must stop at /anthropic.
CLAUDE_BASE_URL="${PROXY_BASE}/anthropic"
TRACE_PATH="/tmp/llm-proxy-test-debug.jsonl"

# Models defined in proxy config
CODEX_MODELS=("minimax-anth" "minimax-openai" "qwen-local")
CLAUDE_MODELS=("sonnet" "haiku" "opus")

# ── Colors ──
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# ── Counters ──
PASS=0
FAIL=0
SKIP=0
RESULTS=()

# ── Selectors ──
RUN_PHASE="all"
SKIP_SLOW=false
PROXY_ONLY=false

# ── Valid phase names for --test ──
VALID_PHASES="all codex_basic codex_multi claude_basic claude_multi codex claude basic multi"

# ── Parse args ──
while [[ $# -gt 0 ]]; do
    case "$1" in
        --test)       RUN_PHASE="$2"; shift 2 ;;
        --skip-slow)  SKIP_SLOW=true; shift ;;
        --proxy-only) PROXY_ONLY=true; shift ;;
        --help|-h)
            echo "Usage: bash scripts/test_cli_clients.sh [--test phase] [--skip-slow] [--proxy-only]"
            echo ""
            echo "Phases: all, codex_basic, codex_multi, claude_basic, claude_multi, codex, claude, basic, multi"
            echo ""
            echo "Environment:"
            echo "  PROXY_BIN      Path to proxy binary (default: ./target/release/llm-universal-proxy)"
            echo "  PROXY_PORT     Port to use (default: 18888)"
            exit 0
            ;;
        *) echo "Unknown arg: $1"; exit 1 ;;
    esac
done

# Validate --test phase name
if [[ "$RUN_PHASE" != "all" ]] && ! echo " $VALID_PHASES " | grep -q " $RUN_PHASE "; then
    echo "Unknown test phase: $RUN_PHASE"
    echo "Valid phases: $VALID_PHASES"
    exit 1
fi

# ============================================================
# Helpers
# ============================================================

should_run() {
    local t="$1"
    [[ "$RUN_PHASE" == "all" ]] && return 0
    [[ "$RUN_PHASE" == "$t" ]] && return 0
    case "$RUN_PHASE" in
        codex_basic)  [[ "$t" == codex_*_basic ]]  && return 0 ;;
        codex_multi)  [[ "$t" == codex_*_multi ]]  && return 0 ;;
        claude_basic) [[ "$t" == claude_*_basic ]] && return 0 ;;
        claude_multi) [[ "$t" == claude_*_multi ]] && return 0 ;;
        codex)  [[ "$t" == codex_* ]]   && return 0 ;;
        claude) [[ "$t" == claude_* ]]  && return 0 ;;
        basic)  [[ "$t" == *_basic ]]   && return 0 ;;
        multi)  [[ "$t" == *_multi ]]   && return 0 ;;
    esac
    return 1
}

log_pass()  { PASS=$((PASS+1));  RESULTS+=("PASS: $1"); echo -e "  ${GREEN}[PASS]${NC} $1"; }
log_fail()  { FAIL=$((FAIL+1));  RESULTS+=("FAIL: $1 — $2"); echo -e "  ${RED}[FAIL]${NC} $1 — $2"; }
log_skip()  { SKIP=$((SKIP+1));  RESULTS+=("SKIP: $1"); echo -e "  ${YELLOW}[SKIP]${NC} $1"; }
log_header() { echo -e "\n${CYAN}${BOLD}=== $1 ===${NC}"; }

# ============================================================
# Proxy lifecycle
# ============================================================

PROXY_PID=""
PROXY_STDERR_FILE=""
CLAUDE_TEST_CONFIG_DIR=""
PROXY_RUNTIME_CONFIG=""

start_proxy() {
    if ! [ -x "$PROXY_BIN" ]; then
        echo "Building proxy..."
        cargo build --release 2>&1 | tail -5
    fi

    PROXY_STDERR_FILE=$(mktemp /tmp/proxy-test-stderr.XXXXXX)
    rm -f "$TRACE_PATH"
    PROXY_RUNTIME_CONFIG=$(mktemp /tmp/proxy-test-config.XXXXXX.yaml)

    python3 - <<'PY' "$PROXY_CONFIG" "$PROXY_RUNTIME_CONFIG" "$PROXY_HOST" "$PROXY_PORT"
import pathlib
import re
import sys

src = pathlib.Path(sys.argv[1]).read_text()
dst = pathlib.Path(sys.argv[2])
host = sys.argv[3]
port = sys.argv[4]

updated = re.sub(r'(?m)^listen:\s*.*$', f'listen: {host}:{port}', src, count=1)
if updated == src:
    raise SystemExit("failed to override listen address in proxy config")
dst.write_text(updated)
PY

    echo "Starting proxy: $PROXY_BIN --config $PROXY_RUNTIME_CONFIG"
    "$PROXY_BIN" --config "$PROXY_RUNTIME_CONFIG" > /dev/null 2>"$PROXY_STDERR_FILE" &
    PROXY_PID=$!
    echo "Proxy PID: $PROXY_PID"
}

stop_proxy() {
    if [[ -n "$PROXY_PID" ]]; then
        kill "$PROXY_PID" 2>/dev/null || true
        wait "$PROXY_PID" 2>/dev/null || true
        if [[ -s "$PROXY_STDERR_FILE" ]] && [[ $FAIL -gt 0 ]]; then
            echo -e "\n${YELLOW}Proxy stderr (last 20 lines):${NC}"
            tail -20 "$PROXY_STDERR_FILE"
        fi
        rm -f "$PROXY_STDERR_FILE"
        rm -f "$PROXY_RUNTIME_CONFIG"
        PROXY_PID=""
        PROXY_RUNTIME_CONFIG=""
    fi
}

cleanup_claude_config() {
    if [[ -n "$CLAUDE_TEST_CONFIG_DIR" ]]; then
        rm -rf "$CLAUDE_TEST_CONFIG_DIR"
        CLAUDE_TEST_CONFIG_DIR=""
    fi
}

wait_for_proxy() {
    local max_wait=30 elapsed=0
    while [[ $elapsed -lt $max_wait ]]; do
        if curl -sf "$PROXY_BASE/health" >/dev/null 2>&1; then
            echo "Proxy healthy on $PROXY_BASE"
            return 0
        fi
        # Check if process died
        if ! kill -0 "$PROXY_PID" 2>/dev/null; then
            echo "Proxy process exited unexpectedly" >&2
            cat "$PROXY_STDERR_FILE" >&2
            return 1
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    echo "Proxy did not become healthy within ${max_wait}s" >&2
    cat "$PROXY_STDERR_FILE" >&2
    return 1
}

# ============================================================
# Prerequisite checks
# ============================================================

check_prereqs() {
    local missing=()
    command -v codex >/dev/null 2>&1 || missing+=("codex CLI")
    command -v claude >/dev/null 2>&1 || missing+=("claude CLI")
    command -v curl >/dev/null 2>&1 || missing+=("curl")
    command -v python3 >/dev/null 2>&1 || missing+=("python3")
    [[ -f "$PROXY_CONFIG" ]] || missing+=("$PROXY_CONFIG")

    if [[ ${#missing[@]} -gt 0 ]]; then
        echo "Missing prerequisites: ${missing[*]}"
        echo "Install: npm install -g @openai/codex  and/or  npm install -g @anthropic-ai/claude-code"
        exit 1
    fi

    echo "Codex CLI:    $(codex --version 2>&1)"
    echo "Claude Code:  $(claude --version 2>&1)"
    echo "Proxy binary: $PROXY_BIN"
    echo "Proxy config: $PROXY_CONFIG"
}

setup_claude_config() {
    CLAUDE_TEST_CONFIG_DIR=$(mktemp -d /tmp/claude-proxy-config.XXXXXX)
    cat > "$CLAUDE_TEST_CONFIG_DIR/settings.json" <<EOF
{
  "env": {
    "ANTHROPIC_BASE_URL": "$CLAUDE_BASE_URL",
    "ANTHROPIC_API_KEY": "dummy",
    "ANTHROPIC_DEFAULT_SONNET_MODEL": "minimax-anth",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "minimax-openai",
    "ANTHROPIC_DEFAULT_OPUS_MODEL": "qwen-local",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"
  },
  "skipDangerousModePermissionPrompt": true
}
EOF
}

# ============================================================
# Test project for multi-turn tests
# ============================================================

setup_project() {
    local dir
    dir=$(mktemp -d /tmp/proxy-test-project.XXXXXX)
    cat > "$dir/calc.py" << 'PYEOF'
def add(a, b):
    return a - b  # bug: should be +

def multiply(a, b):
    return a * b
PYEOF
    cat > "$dir/main.py" << 'PYEOF'
from calc import add, multiply

print(f"2 + 3 = {add(2, 3)}")
print(f"4 * 5 = {multiply(4, 5)}")
PYEOF
    echo "$dir"
}

# ============================================================
# Codex CLI tests
# ============================================================

# Codex CLI uses the OpenAI Responses API internally.
# We point it at the proxy via OPENAI_BASE_URL + -c flags.
# The proxy receives requests at /openai/v1/responses.
# The -c flags define a custom provider with supports_websockets=false.
# This avoids WebSocket upgrade attempts and forces HTTP-only mode.

run_codex_basic() {
    local model="$1"
    local label="codex_${model//-/_}_basic"

    if ! should_run "$label"; then log_skip "$label"; return; fi

    log_header "Running: $label"
    echo "  (Codex CLI → proxy → $model)"

    local prompt="Reply with exactly the word PONG and nothing else."
    local output exit_code=0

    output=$(\
        OPENAI_BASE_URL="$CODEX_BASE_URL" \
        OPENAI_API_KEY=dummy \
        timeout "$TIMEOUT_SINGLE" \
        codex exec "$prompt" \
        -c "model=\"$model\"" \
        -c 'model_provider="proxy"' \
        -c 'model_providers.proxy.name="Proxy"' \
        -c "model_providers.proxy.base_url=\"$CODEX_BASE_URL\"" \
        -c 'model_providers.proxy.wire_api="responses"' \
        -c 'model_providers.proxy.supports_websockets=false' \
        --sandbox read-only \
        --json 2>/dev/null) || exit_code=$?

    if [[ "$exit_code" -ne 0 ]]; then
        log_fail "$label" "exit code $exit_code"
        return
    fi

    # Check for failure events in JSONL output
    if echo "$output" | grep -q '"type":"turn.failed"'; then
        local err_msg
        err_msg=$(echo "$output" | grep '"type":"turn.failed"' \
            | python3 -c "import sys,json; print(json.loads(sys.stdin.read()).get('error',{}).get('message','turn failed'))" 2>/dev/null \
            || echo "turn failed")
        log_fail "$label" "$err_msg"
        return
    fi

    # Check output contains PONG
    if echo "$output" | grep -qi "PONG"; then
        log_pass "$label"
    else
        local snippet
        snippet=$(echo "$output" | tail -5 | head -c 300)
        log_fail "$label" "expected PONG, got: ${snippet:-<empty>}"
    fi
}

run_codex_multi() {
    local model="$1"
    local label="codex_${model//-/_}_multi"

    if ! should_run "$label"; then log_skip "$label"; return; fi
    if [[ "$SKIP_SLOW" == "true" ]]; then log_skip "$label (slow)"; return; fi
    if [[ "$model" == "qwen-local" ]]; then
        log_skip "$label (local qwen alias is not reliable for multi-turn code edits)"
        return
    fi

    log_header "Running: $label"
    echo "  (Codex CLI → proxy → $model, multi-turn bug-fix task)"

    local proj_dir
    proj_dir=$(setup_project)

    local prompt="Read calc.py. There is a bug: the add function uses minus instead of plus. Fix it. Then read main.py and confirm the fix is correct."
    local output exit_code=0

    output=$(cd "$proj_dir" && \
        OPENAI_BASE_URL="$CODEX_BASE_URL" \
        OPENAI_API_KEY=dummy \
        timeout "$TIMEOUT_MULTI" \
        codex exec "$prompt" \
        -c "model=\"$model\"" \
        -c 'model_provider="proxy"' \
        -c 'model_providers.proxy.name="Proxy"' \
        -c "model_providers.proxy.base_url=\"$CODEX_BASE_URL\"" \
        -c 'model_providers.proxy.wire_api="responses"' \
        -c 'model_providers.proxy.supports_websockets=false' \
        --sandbox workspace-write \
        --skip-git-repo-check \
        --json 2>/dev/null) || exit_code=$?

    if [[ "$exit_code" -ne 0 ]]; then
        local calc_content
        calc_content=$(cat "$proj_dir/calc.py" 2>/dev/null | head -3)
        log_fail "$label" "exit code $exit_code. calc.py: ${calc_content:-<missing>}"
        rm -rf "$proj_dir"
        return
    fi

    # Check: was calc.py fixed?
    if grep -q "a + b" "$proj_dir/calc.py" 2>/dev/null; then
        log_pass "$label"
    else
        local calc_content
        calc_content=$(cat "$proj_dir/calc.py" 2>/dev/null | head -3)
        log_fail "$label" "calc.py not fixed. Content: ${calc_content:-<missing>}"
    fi

    rm -rf "$proj_dir"
}

# ============================================================
# Claude Code tests
# ============================================================

# Claude Code uses the Anthropic Messages API internally.
# We isolate it from the user's global ~/.claude configuration by creating a
# throwaway CLAUDE_CONFIG_DIR for this script run only.
#
# In --bare mode Claude requires ANTHROPIC_API_KEY-style auth, which is perfect
# for proxy testing because it avoids any dependency on the user's OAuth state
# and keeps the injected system context small enough for the local qwen model.
#
# Model routing:
#   sonnet -> minimax-anth
#   haiku  -> minimax-openai
#   opus   -> qwen-local
# The mapping lives in the temporary settings.json written by setup_claude_config().

run_claude_basic() {
    local model="$1"
    local label="claude_${model//-/_}_basic"

    if ! should_run "$label"; then log_skip "$label"; return; fi

    log_header "Running: $label"
    echo "  (Claude Code → proxy → $model)"

    local prompt="Reply with exactly the word PONG and nothing else."
    local output exit_code=0

    output=$(CLAUDE_CONFIG_DIR="$CLAUDE_TEST_CONFIG_DIR" \
        timeout "$TIMEOUT_SINGLE" \
        claude --bare --setting-sources user -p "$prompt" \
        --model "$model" \
        --max-turns 1 \
        --dangerously-skip-permissions \
        --no-session-persistence \
        --add-dir "$PWD" \
        2>/dev/null) || exit_code=$?

    if [[ "$exit_code" -ne 0 ]]; then
        log_fail "$label" "exit code $exit_code"
        return
    fi

    # Claude -p outputs plain text by default
    if echo "$output" | grep -qi "PONG"; then
        log_pass "$label"
    else
        local snippet="${output:-<empty>}"
        snippet=${snippet:0:200}
        log_fail "$label" "expected PONG, got: $snippet"
    fi
}

run_claude_multi() {
    local model="$1"
    local label="claude_${model//-/_}_multi"

    if ! should_run "$label"; then log_skip "$label"; return; fi
    if [[ "$SKIP_SLOW" == "true" ]]; then log_skip "$label (slow)"; return; fi
    if [[ "$model" == "opus" ]]; then
        log_skip "$label (local qwen alias is not reliable for multi-turn code edits)"
        return
    fi

    log_header "Running: $label"
    echo "  (Claude Code → proxy → $model, multi-turn bug-fix task)"

    local proj_dir
    proj_dir=$(setup_project)

    local prompt="Read calc.py, identify the bug in the add function (it subtracts instead of adding). Fix it by changing the minus to plus. Then read main.py and confirm the output is correct."
    local output exit_code=0

    output=$(cd "$proj_dir" && \
        CLAUDE_CONFIG_DIR="$CLAUDE_TEST_CONFIG_DIR" \
        timeout "$TIMEOUT_MULTI" \
        claude --bare --setting-sources user -p "$prompt" \
        --model "$model" \
        --max-turns 5 \
        --dangerously-skip-permissions \
        --no-session-persistence \
        --add-dir "$proj_dir" \
        2>/dev/null) || exit_code=$?

    if [[ "$exit_code" -ne 0 ]]; then
        local calc_content
        calc_content=$(cat "$proj_dir/calc.py" 2>/dev/null | head -3)
        log_fail "$label" "exit code $exit_code. calc.py: ${calc_content:-<missing>}"
        rm -rf "$proj_dir"
        return
    fi

    # Check: was calc.py fixed?
    if grep -q "a + b" "$proj_dir/calc.py" 2>/dev/null; then
        log_pass "$label"
    else
        local calc_content
        calc_content=$(cat "$proj_dir/calc.py" 2>/dev/null | head -3)
        log_fail "$label" "calc.py not fixed. Content: ${calc_content:-<missing>}"
    fi

    rm -rf "$proj_dir"
}

# ============================================================
# Summary
# ============================================================

print_summary() {
    log_header "Summary"
    echo ""
    for r in "${RESULTS[@]}"; do
        if [[ "$r" == PASS* ]]; then echo -e "  ${GREEN}$r${NC}"
        elif [[ "$r" == FAIL* ]]; then echo -e "  ${RED}$r${NC}"
        else echo -e "  ${YELLOW}$r${NC}"
        fi
    done
    echo ""
    echo -e "Total: ${GREEN}$PASS passed${NC}, ${RED}$FAIL failed${NC}, ${YELLOW}$SKIP skipped${NC}"

    # Show trace stats if available
    if [[ -f "$TRACE_PATH" ]]; then
        local req_count
        req_count=$(grep -c '"phase":"request"' "$TRACE_PATH" 2>/dev/null || echo "0")
        echo -e "\n${CYAN}Debug trace:${NC} $req_count requests logged to $TRACE_PATH"
    fi

    echo ""

    if [[ $FAIL -gt 0 ]]; then
        echo -e "${RED}Some tests failed.${NC}"
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
    echo "${BOLD}LLM Universal Proxy — E2E CLI Client Tests${NC}"
    echo "Target: $PROXY_BASE"
    echo ""

    check_prereqs

    # Start proxy
    log_header "Starting proxy"
    start_proxy
    setup_claude_config
    trap 'stop_proxy; cleanup_claude_config' EXIT

    if ! wait_for_proxy; then
        echo "${RED}Failed to start proxy${NC}"
        exit 1
    fi

    # If proxy-only mode, just wait
    if [[ "$PROXY_ONLY" == "true" ]]; then
        echo ""
        echo "Proxy is running. Press Ctrl+C to stop."
        echo "  Codex base URL:  $CODEX_BASE_URL"
        echo "  Claude base URL: $CLAUDE_BASE_URL"
        echo ""
        # Wait for SIGINT
        wait "$PROXY_PID" 2>/dev/null || true
        return 0
    fi

    # ── Codex CLI tests ──
    # Responses API → various upstream formats
    log_header "Phase 1: Codex CLI — Single-turn (Responses API client)"
    for model in "${CODEX_MODELS[@]}"; do
        run_codex_basic "$model"
    done

    log_header "Phase 2: Codex CLI — Multi-turn (Responses API client)"
    for model in "${CODEX_MODELS[@]}"; do
        run_codex_multi "$model"
    done

    # ── Claude Code tests ──
    # Anthropic Messages API → various upstream formats
    log_header "Phase 3: Claude Code — Single-turn (Anthropic Messages client)"
    for model in "${CLAUDE_MODELS[@]}"; do
        run_claude_basic "$model"
    done

    log_header "Phase 4: Claude Code — Multi-turn (Anthropic Messages client)"
    for model in "${CLAUDE_MODELS[@]}"; do
        run_claude_multi "$model"
    done

    # ── Summary ──
    print_summary
}

main "$@"
