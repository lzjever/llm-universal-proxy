#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

IMAGE="${IMAGE:-llm-universal-proxy:local}"
HOST="127.0.0.1"
CONTAINER_PORT="8080"
PROXY_PORT="${PROXY_PORT:-}"
ADMIN_TOKEN="container-smoke-token"
PROXY_KEY="container-smoke-proxy-key"

TMP_DIR=""
MOCK_PID=""
MOCK_PORT=""
MOCK_PORT_FILE=""
PROXY_CONFIG=""
MOCK_STDERR=""
RESP_HEADERS=""
RESP_BODY=""
CONTAINER_NAME=""

log() {
    printf '[container-smoke] %s\n' "$*"
}

print_usage() {
    cat <<'EOF'
Usage: bash scripts/test_container_smoke.sh

Runs a small smoke suite against a locally available llmup container image.

Environment:
  IMAGE       Container image to test; defaults to llm-universal-proxy:local
  PROXY_PORT Fixed host/container port for the proxy; defaults to an ephemeral free port
EOF
}

fail() {
    echo "[container-smoke] ERROR: $*" >&2
    dump_logs
    exit 1
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || fail "Missing required command: $1"
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

cleanup() {
    if [[ -n "$CONTAINER_NAME" ]]; then
        docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
    fi
    terminate_pid "$MOCK_PID"
    if [[ -n "$TMP_DIR" && -d "$TMP_DIR" ]]; then
        rm -rf "$TMP_DIR"
    fi
}

trap cleanup EXIT
trap 'exit 1' INT TERM

dump_logs() {
    if [[ -n "$CONTAINER_NAME" ]]; then
        echo "--- container logs ---" >&2
        docker logs --tail 80 "$CONTAINER_NAME" >&2 2>/dev/null || true
    fi
    if [[ -n "$MOCK_STDERR" && -s "$MOCK_STDERR" ]]; then
        echo "--- mock upstream stderr ---" >&2
        tail -50 "$MOCK_STDERR" >&2 || true
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

container_is_running() {
    [[ "$(docker inspect --format '{{.State.Running}}' "$CONTAINER_NAME" 2>/dev/null || true)" == "true" ]]
}

wait_for_http_ok() {
    local url="$1"
    local label="$2"

    for _ in $(seq 1 150); do
        if curl -sf "$url" >/dev/null 2>&1; then
            return 0
        fi
        if ! container_is_running; then
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
EXPECTED_X_API_KEY = "container-smoke-provider-key"


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
        ("content_block_stop", {"type": "content_block_stop", "index": 0}),
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
        if self.headers.get("x-api-key") != EXPECTED_X_API_KEY:
            self._send_json(401, {"error": "unexpected upstream authorization"})
            return
        if self.path in ("/v1/messages", "/messages"):
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
        self._send_json(404, {"error": "not found"})


server = ThreadingHTTPServer(("0.0.0.0", 0), Handler)
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
listen: 0.0.0.0:${CONTAINER_PORT}
upstream_timeout_secs: 10
upstreams:
  default:
    api_root: http://host.docker.internal:${MOCK_PORT}/v1
    format: anthropic
    provider_key_env: CONTAINER_SMOKE_UPSTREAM_API_KEY
EOF
}

assert_image_contract() {
    local image_user
    local healthcheck

    log "Checking image metadata"
    image_user="$(docker image inspect "$IMAGE" --format '{{.Config.User}}')"
    if [[ -z "$image_user" || "$image_user" == "0" || "$image_user" == "root" ]]; then
        fail "Image must declare a non-root user, got: ${image_user:-<empty>}"
    fi

    healthcheck="$(docker image inspect "$IMAGE" --format '{{json .Config.Healthcheck}}')"
    if [[ "$healthcheck" == "null" ]]; then
        fail "Image must declare a Docker HEALTHCHECK"
    fi
    if [[ "$healthcheck" != *"http://127.0.0.1:8080/ready"* ]]; then
        fail "Image HEALTHCHECK must target readiness on fixed container port 8080"
    fi
}

start_container() {
    CONTAINER_NAME="llmup-container-smoke-$(date +%s)-$$"
    log "Starting container $CONTAINER_NAME from $IMAGE"
    docker run -d --rm \
        --name "$CONTAINER_NAME" \
        --add-host=host.docker.internal:host-gateway \
        -e "LLM_UNIVERSAL_PROXY_ADMIN_TOKEN=${ADMIN_TOKEN}" \
        -e "LLM_UNIVERSAL_PROXY_AUTH_MODE=proxy_key" \
        -e "LLM_UNIVERSAL_PROXY_KEY=${PROXY_KEY}" \
        -e "CONTAINER_SMOKE_UPSTREAM_API_KEY=container-smoke-provider-key" \
        --health-interval=2s \
        --health-timeout=2s \
        --health-retries=15 \
        --health-start-period=1s \
        -p "${HOST}:${PROXY_PORT}:${CONTAINER_PORT}" \
        -v "${PROXY_CONFIG}:/etc/llmup/config.yaml:ro" \
        "$IMAGE" >/dev/null
    wait_for_http_ok "http://${HOST}:${PROXY_PORT}/health" "container proxy"
    wait_for_http_ok "http://${HOST}:${PROXY_PORT}/ready" "container proxy readiness"
    wait_for_container_healthy
}

start_bootstrap_container() {
    CONTAINER_NAME="llmup-container-bootstrap-smoke-$(date +%s)-$$"
    log "Starting container $CONTAINER_NAME from $IMAGE"
    log "No bootstrap config bind mount; using image default /etc/llmup/config.yaml"
    docker run -d --rm \
        --name "$CONTAINER_NAME" \
        --add-host=host.docker.internal:host-gateway \
        -e "LLM_UNIVERSAL_PROXY_ADMIN_TOKEN=${ADMIN_TOKEN}" \
        -e "LLM_UNIVERSAL_PROXY_AUTH_MODE=proxy_key" \
        -e "LLM_UNIVERSAL_PROXY_KEY=${PROXY_KEY}" \
        -e "CONTAINER_SMOKE_UPSTREAM_API_KEY=container-smoke-provider-key" \
        --health-interval=2s \
        --health-timeout=2s \
        --health-retries=15 \
        --health-start-period=1s \
        -p "${HOST}:${PROXY_PORT}:${CONTAINER_PORT}" \
        "$IMAGE" >/dev/null
    wait_for_http_ok "http://${HOST}:${PROXY_PORT}/health" "bootstrap container liveness"
}

stop_container() {
    if [[ -n "$CONTAINER_NAME" ]]; then
        docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
        CONTAINER_NAME=""
    fi
}

wait_for_container_healthy() {
    local status

    log "Waiting for Docker health status"
    for _ in $(seq 1 60); do
        status="$(
            docker inspect \
                --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}missing{{end}}' \
                "$CONTAINER_NAME" 2>/dev/null || true
        )"
        case "$status" in
            healthy)
                return 0
                ;;
            unhealthy)
                fail "Container healthcheck reported unhealthy"
                ;;
            missing)
                fail "Container is missing Docker health state"
                ;;
        esac
        if ! container_is_running; then
            fail "container proxy exited before healthcheck became healthy"
        fi
        sleep 0.5
    done
    fail "Timed out waiting for Docker healthcheck to become healthy"
}

reset_response_artifacts() {
    : >"$RESP_HEADERS"
    : >"$RESP_BODY"
}

run_admin_token_smoke() {
    local unauth_code
    local auth_code

    log "Smoke 1/4: admin token boundary"
    reset_response_artifacts
    unauth_code="$(
        curl -sS \
            -D "$RESP_HEADERS" \
            -o "$RESP_BODY" \
            "http://${HOST}:${PROXY_PORT}/admin/state" \
            -w '%{http_code}'
    )"
    assert_eq "$unauth_code" "401" "Expected admin request without token to be rejected"

    reset_response_artifacts
    auth_code="$(
        curl -sS \
            -D "$RESP_HEADERS" \
            -o "$RESP_BODY" \
            -H "Authorization: Bearer ${ADMIN_TOKEN}" \
            "http://${HOST}:${PROXY_PORT}/admin/state" \
            -w '%{http_code}'
    )"
    assert_eq "$auth_code" "200" "Expected admin request with token to succeed"
}

run_responses_smoke() {
    local http_code
    local content_type
    local label="${1:-Smoke 2/4: Responses -> Anthropic SSE}"

    log "$label"
    reset_response_artifacts
    http_code="$(
        curl -sS --http1.1 -N \
            -D "$RESP_HEADERS" \
            -o "$RESP_BODY" \
            -X POST "http://${HOST}:${PROXY_PORT}/openai/v1/responses" \
            -H "Accept: text/event-stream" \
            -H "Content-Type: application/json" \
            -H "Authorization: Bearer ${PROXY_KEY}" \
            --data '{"model":"GLM-5","input":"Hi","stream":true}' \
            -w '%{http_code}'
    )"

    content_type="$(
        tr -d '\r' <"$RESP_HEADERS" |
            awk 'BEGIN{IGNORECASE=1} /^Content-Type:/ {sub(/^[^:]+:[[:space:]]*/, ""); print; exit}'
    )"

    assert_eq "$http_code" "200" "Expected HTTP 200 from /openai/v1/responses"
    if [[ "${content_type,,}" != text/event-stream* ]]; then
        fail "Expected SSE content type, got: ${content_type:-<missing>}"
    fi
    if ! grep -Fq 'response.completed' "$RESP_BODY"; then
        fail "Missing response.completed in response stream"
    fi
}

run_bootstrap_apply_smoke() {
    local ready_code
    local admin_payload
    local admin_http_code

    log "Smoke 3/4: default empty config -> admin apply -> ready"

    reset_response_artifacts
    ready_code="$(
        curl -sS \
            -D "$RESP_HEADERS" \
            -o "$RESP_BODY" \
            "http://${HOST}:${PROXY_PORT}/ready" \
            -w '%{http_code}'
    )"
    assert_eq "$ready_code" "503" "Expected empty bootstrap config to start not ready"

    admin_payload="$(
        cat <<EOF
{"if_revision":null,"config":{"listen":"0.0.0.0:8080","upstream_timeout_secs":10,"upstreams":[{"name":"default","api_root":"http://host.docker.internal:${MOCK_PORT}/v1","fixed_upstream_format":"anthropic","provider_key_env":"CONTAINER_SMOKE_UPSTREAM_API_KEY"}]}}
EOF
    )"

    reset_response_artifacts
    admin_http_code="$(
        curl -sS \
            -D "$RESP_HEADERS" \
            -o "$RESP_BODY" \
            -X POST "http://${HOST}:${PROXY_PORT}/admin/namespaces/default/config" \
            -H "Authorization: Bearer ${ADMIN_TOKEN}" \
            -H "Content-Type: application/json" \
            --data "$admin_payload" \
            -w '%{http_code}'
    )"
    assert_eq "$admin_http_code" "200" "Expected admin bootstrap config apply to succeed"
    wait_for_http_ok "http://${HOST}:${PROXY_PORT}/ready" "bootstrap container readiness"
    wait_for_container_healthy
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
    require_cmd docker
    require_cmd python3

    TMP_DIR="$(mktemp -d /tmp/llm-universal-proxy-container-smoke.XXXXXX)"
    MOCK_PORT_FILE="$TMP_DIR/mock.port"
    PROXY_CONFIG="$TMP_DIR/proxy.yaml"
    MOCK_STDERR="$TMP_DIR/mock.stderr.log"
    RESP_HEADERS="$TMP_DIR/response.headers"
    RESP_BODY="$TMP_DIR/response.body"

    assert_image_contract
    start_mock_upstream
    write_proxy_config
    start_container
    run_admin_token_smoke
    run_responses_smoke
    stop_container

    start_bootstrap_container
    run_bootstrap_apply_smoke
    run_responses_smoke "Smoke 4/4: bootstrap Responses -> Anthropic SSE"

    log "Container smoke passed"
}

main "$@"
