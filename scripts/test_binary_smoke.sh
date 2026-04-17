#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# Override this when reusing an already-built target binary, for example from CI packaging jobs.
PROXY_BIN="${PROXY_BIN:-./target/release/llm-universal-proxy}"
HOST="127.0.0.1"
PROXY_PORT="${PROXY_PORT:-}"

TMP_DIR=""
MOCK_PID=""
PROXY_PID=""
MOCK_PORT=""
MOCK_PORT_FILE=""
PROXY_CONFIG=""
MOCK_STDERR=""
PROXY_STDERR=""
RESP_HEADERS=""
RESP_BODY=""

log() {
    printf '[smoke] %s\n' "$*"
}

print_usage() {
    cat <<'EOF'
Usage: bash scripts/test_binary_smoke.sh

Runs a small smoke suite against an already-built llm-universal-proxy release binary.

Environment:
  PROXY_BIN   Path to the binary to test
  PROXY_PORT  Fixed local port for the proxy; defaults to an ephemeral free port
EOF
}

terminate_pid() {
    local pid="${1:-}"
    if [[ -z "$pid" ]]; then
        return
    fi
    if kill -0 "$pid" 2>/dev/null; then
        kill "$pid" 2>/dev/null || true
        for _ in $(seq 1 20); do
            if ! kill -0 "$pid" 2>/dev/null; then
                break
            fi
            sleep 0.1
        done
        if kill -0 "$pid" 2>/dev/null; then
            kill -9 "$pid" 2>/dev/null || true
        fi
    fi
    wait "$pid" 2>/dev/null || true
}

dump_logs() {
    if [[ -n "$MOCK_STDERR" && -s "$MOCK_STDERR" ]]; then
        echo "--- mock upstream stderr ---" >&2
        tail -50 "$MOCK_STDERR" >&2 || true
    fi
    if [[ -n "$PROXY_STDERR" && -s "$PROXY_STDERR" ]]; then
        echo "--- proxy stderr ---" >&2
        tail -50 "$PROXY_STDERR" >&2 || true
    fi
    if [[ -n "$RESP_HEADERS" && -s "$RESP_HEADERS" ]]; then
        echo "--- response headers ---" >&2
        tr -d '\r' <"$RESP_HEADERS" >&2 || true
    fi
    if [[ -n "$RESP_BODY" && -s "$RESP_BODY" ]]; then
        echo "--- response body ---" >&2
        cat "$RESP_BODY" >&2 || true
    fi
}

fail() {
    echo "[smoke] ERROR: $*" >&2
    dump_logs
    exit 1
}

cleanup() {
    terminate_pid "$PROXY_PID"
    terminate_pid "$MOCK_PID"
    if [[ -n "$TMP_DIR" && -d "$TMP_DIR" ]]; then
        rm -rf "$TMP_DIR"
    fi
}

trap cleanup EXIT
trap 'exit 1' INT TERM

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || fail "Missing required command: $1"
}

find_free_port() {
    python3 - <<'PY'
import socket

sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
}

wait_for_file() {
    local path="$1"
    local label="$2"
    local pid="${3:-}"

    for _ in $(seq 1 100); do
        if [[ -s "$path" ]]; then
            return 0
        fi
        if [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null; then
            fail "$label exited before becoming ready"
        fi
        sleep 0.05
    done
    fail "Timed out waiting for $label"
}

wait_for_http_ok() {
    local url="$1"
    local label="$2"
    local pid="${3:-}"

    for _ in $(seq 1 150); do
        if curl -sf "$url" >/dev/null 2>&1; then
            return 0
        fi
        if [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null; then
            fail "$label exited unexpectedly"
        fi
        sleep 0.1
    done
    fail "$label did not become ready at $url"
}

assert_eq() {
    local actual="$1"
    local expected="$2"
    local message="$3"

    if [[ "$actual" != "$expected" ]]; then
        fail "$message (expected: $expected, got: $actual)"
    fi
}

start_mock_upstream() {
    log "Starting mock upstream"
    python3 -u - "$MOCK_PORT_FILE" >/dev/null 2>"$MOCK_STDERR" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port_file = sys.argv[1]


def anthropic_stream(model: str) -> bytes:
    events = [
        (
            "message_start",
            {
                "type": "message_start",
                "message": {
                    "id": "msg_1",
                    "type": "message",
                    "role": "assistant",
                    "model": model,
                    "content": [],
                    "stop_reason": None,
                    "stop_sequence": None,
                    "usage": {"input_tokens": 0, "output_tokens": 0},
                },
            },
        ),
        (
            "content_block_start",
            {
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""},
            },
        ),
        (
            "content_block_delta",
            {
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "Hi"},
            },
        ),
        (
            "content_block_stop",
            {"type": "content_block_stop", "index": 0},
        ),
        (
            "message_delta",
            {
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn"},
                "usage": {"input_tokens": 1, "output_tokens": 1},
            },
        ),
        ("message_stop", {"type": "message_stop"}),
    ]
    chunks = []
    for event_name, payload in events:
        chunks.append(f"event: {event_name}\n")
        chunks.append("data: " + json.dumps(payload, separators=(",", ":")) + "\n\n")
    return "".join(chunks).encode("utf-8")


def openai_completion(model: str) -> dict:
    return {
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 1735689600,
        "model": model,
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": "Hi from OpenAI mock"},
                "finish_reason": "stop",
            }
        ],
        "usage": {
            "prompt_tokens": 1,
            "completion_tokens": 4,
            "total_tokens": 5,
        },
    }


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        sys.stderr.write((fmt % args) + "\n")

    def _read_json(self):
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length) if length else b"{}"
        return json.loads(raw or b"{}")

    def _send_json(self, status: int, payload):
        body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()

    def do_POST(self):
        body = self._read_json()
        model = body.get("model", "missing-model")

        if self.path in ("/v1/messages", "/messages"):
            stream = bool(body.get("stream"))

            if stream:
                payload = anthropic_stream(model)
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Cache-Control", "no-cache")
                self.send_header("Connection", "close")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
                self.wfile.flush()
                return

            self._send_json(
                200,
                {
                    "id": "msg_1",
                    "type": "message",
                    "role": "assistant",
                    "model": model,
                    "content": [{"type": "text", "text": "Hi"}],
                    "stop_reason": "end_turn",
                    "stop_sequence": None,
                    "usage": {"input_tokens": 1, "output_tokens": 1},
                },
            )
            return

        if self.path in ("/v1/chat/completions", "/chat/completions"):
            self._send_json(200, openai_completion(model))
            return

        self._send_json(404, {"error": "not found"})


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w", encoding="utf-8") as handle:
    handle.write(str(server.server_port))
    handle.flush()
server.serve_forever()
PY
    MOCK_PID=$!
    wait_for_file "$MOCK_PORT_FILE" "mock upstream" "$MOCK_PID"
    MOCK_PORT="$(<"$MOCK_PORT_FILE")"
}

write_proxy_config() {
    if [[ -z "$PROXY_PORT" ]]; then
        PROXY_PORT="$(find_free_port)"
    fi

    cat >"$PROXY_CONFIG" <<EOF
listen: ${HOST}:${PROXY_PORT}
upstream_timeout_secs: 10
upstreams:
  default:
    api_root: http://${HOST}:${MOCK_PORT}/v1
    format: anthropic
EOF
}

write_empty_proxy_config() {
    if [[ -z "$PROXY_PORT" ]]; then
        PROXY_PORT="$(find_free_port)"
    fi

    cat >"$PROXY_CONFIG" <<EOF
listen: ${HOST}:${PROXY_PORT}
EOF
}

start_proxy() {
    log "Starting proxy binary"
    "$PROXY_BIN" --config "$PROXY_CONFIG" >/dev/null 2>"$PROXY_STDERR" &
    PROXY_PID=$!
    wait_for_http_ok "http://${HOST}:${PROXY_PORT}/health" "proxy" "$PROXY_PID"
}

stop_proxy() {
    terminate_pid "$PROXY_PID"
    PROXY_PID=""
}

reset_response_artifacts() {
    : >"$RESP_HEADERS"
    : >"$RESP_BODY"
}

run_responses_anthropic_sse_smoke() {
    local http_code
    local content_type

    log "Smoke 1/2: Responses -> Anthropic SSE"
    write_proxy_config
    start_proxy
    reset_response_artifacts

    log "Requesting streaming /openai/v1/responses"
    if ! http_code="$(
        curl -sS --http1.1 -N \
            -D "$RESP_HEADERS" \
            -o "$RESP_BODY" \
            -X POST "http://${HOST}:${PROXY_PORT}/openai/v1/responses" \
            -H "Accept: text/event-stream" \
            -H "Content-Type: application/json" \
            --data '{"model":"GLM-5","input":"Hi","stream":true}' \
            -w '%{http_code}'
    )"; then
        fail "Streaming request failed"
    fi

    content_type="$(
        tr -d '\r' <"$RESP_HEADERS" |
            awk 'BEGIN{IGNORECASE=1} /^Content-Type:/ {sub(/^[^:]+:[[:space:]]*/, ""); print; exit}'
    )"

    assert_eq "$http_code" "200" "Expected HTTP 200 from /openai/v1/responses"
    assert_eq "${content_type,,}" "text/event-stream" "Expected SSE content type"
    if ! grep -Fq 'response.completed' "$RESP_BODY"; then
        fail "Missing response.completed in response stream"
    fi

    stop_proxy
}

run_empty_start_admin_apply_then_request_smoke() {
    local admin_http_code
    local request_http_code
    local admin_payload

    log "Smoke 2/2: empty start -> admin apply -> namespace request"
    write_empty_proxy_config
    start_proxy

    admin_payload="$(
        cat <<EOF
{"if_revision":null,"config":{"listen":"127.0.0.1:0","upstream_timeout_secs":10,"upstreams":[{"name":"default","api_root":"http://${HOST}:${MOCK_PORT}/v1","fixed_upstream_format":"openai-completion"}]}}
EOF
    )"

    reset_response_artifacts
    log "Applying runtime namespace config via /admin/namespaces/demo/config"
    if ! admin_http_code="$(
        curl -sS \
            -D "$RESP_HEADERS" \
            -o "$RESP_BODY" \
            -X POST "http://${HOST}:${PROXY_PORT}/admin/namespaces/demo/config" \
            -H "Content-Type: application/json" \
            --data "$admin_payload" \
            -w '%{http_code}'
    )"; then
        fail "Admin config apply request failed"
    fi

    assert_eq "$admin_http_code" "200" "Expected HTTP 200 from /admin/namespaces/demo/config"
    python3 - "$RESP_BODY" <<'PY' || fail "Admin config apply response assertion failed"
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    body = json.load(handle)

assert body["status"] == "applied", body
assert body["namespace"] == "demo", body
assert isinstance(body.get("revision"), str) and body["revision"], body
PY

    reset_response_artifacts
    log "Requesting /namespaces/demo/openai/v1/chat/completions"
    if ! request_http_code="$(
        curl -sS \
            -D "$RESP_HEADERS" \
            -o "$RESP_BODY" \
            -X POST "http://${HOST}:${PROXY_PORT}/namespaces/demo/openai/v1/chat/completions" \
            -H "Content-Type: application/json" \
            --data '{"model":"gpt-4","messages":[{"role":"user","content":"Hi"}],"stream":false}' \
            -w '%{http_code}'
    )"; then
        fail "Namespace chat completions request failed"
    fi

    assert_eq "$request_http_code" "200" "Expected HTTP 200 from namespace chat completions"
    python3 - "$RESP_BODY" <<'PY' || fail "Namespace chat completions response assertion failed"
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    body = json.load(handle)

assert body["object"] == "chat.completion", body
assert body["choices"][0]["message"]["role"] == "assistant", body
assert isinstance(body["choices"][0]["message"]["content"], str), body
PY

    stop_proxy
}

main() {
    if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
        print_usage
        exit 0
    fi
    if [[ $# -ne 0 ]]; then
        print_usage >&2
        fail "Unexpected arguments: $*"
    fi

    require_cmd curl
    require_cmd python3
    [[ -x "$PROXY_BIN" ]] || fail "Missing proxy binary: $PROXY_BIN"

    TMP_DIR="$(mktemp -d /tmp/llm-universal-proxy-binary-smoke.XXXXXX)"
    MOCK_PORT_FILE="$TMP_DIR/mock.port"
    PROXY_CONFIG="$TMP_DIR/proxy.yaml"
    MOCK_STDERR="$TMP_DIR/mock.stderr.log"
    PROXY_STDERR="$TMP_DIR/proxy.stderr.log"
    RESP_HEADERS="$TMP_DIR/response.headers"
    RESP_BODY="$TMP_DIR/response.body"

    start_mock_upstream
    run_responses_anthropic_sse_smoke
    run_empty_start_admin_apply_then_request_smoke

    log "Binary smoke passed"
}

main "$@"
