#!/usr/bin/env python3
from __future__ import annotations

import argparse
import contextlib
import json
import math
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Iterator


PERF_DEFAULT_ITERATIONS = 30
PERF_DEFAULT_P95_MS = 750.0
PERF_DEFAULT_TOTAL_MS = 15_000.0


@dataclass(frozen=True)
class MockMatrixCase:
    case_id: str
    surface: str
    mode: str
    path: str
    payload: dict[str, object]
    expected_status: int
    expected_content_type: str
    expected_markers: tuple[str, ...]


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def http_json(
    url: str,
    payload: dict[str, object],
    timeout: int = 60,
) -> tuple[int, dict[str, str], str]:
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={"content-type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            body = resp.read().decode("utf-8")
            headers = {key.lower(): value for key, value in resp.headers.items()}
            return resp.status, headers, body
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8")
        headers = {key.lower(): value for key, value in error.headers.items()}
        return error.code, headers, body


def wait_for_health(base_url: str, timeout_secs: int = 15) -> None:
    deadline = time.time() + timeout_secs
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"{base_url}/health", timeout=2) as resp:
                if resp.status == 200:
                    return
        except Exception:
            time.sleep(0.2)
    raise RuntimeError("proxy did not become healthy in time")


def expect_json_case(
    base_url: str,
    path: str,
    payload: dict[str, object],
    expected_markers,
    label: str,
) -> None:
    status, headers, body = http_json(f"{base_url}{path}", payload)
    assert status == 200, f"{label}: unexpected status {status}, body={body}"
    assert "application/json" in headers.get("content-type", ""), (
        f"{label}: unexpected content-type {headers.get('content-type')}"
    )
    parsed = json.loads(body)
    markers = expected_markers if isinstance(expected_markers, list) else [expected_markers]
    rendered = json.dumps(parsed, ensure_ascii=False)
    for marker in markers:
        assert marker in rendered, f"{label}: missing marker {marker!r}, body={body}"
    print(f"[ok] {label}")


def expect_sse_case(
    base_url: str,
    path: str,
    payload: dict[str, object],
    expected_markers: list[str],
    label: str,
) -> None:
    status, headers, body = http_json(f"{base_url}{path}", payload)
    assert status == 200, f"{label}: unexpected status {status}, body={body}"
    assert "text/event-stream" in headers.get("content-type", ""), (
        f"{label}: unexpected content-type {headers.get('content-type')}"
    )
    for marker in expected_markers:
        assert marker in body, f"{label}: missing marker {marker!r}, body={body}"
    print(f"[ok] {label}")


def _body_text(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True)


def _body_has_force_error(value: object) -> bool:
    return "force_error" in _body_text(value)


def _body_has_tool_request(value: object) -> bool:
    rendered = _body_text(value)
    return any(marker in rendered for marker in ("tools", "functionDeclarations"))


def _json_bytes(payload: object) -> bytes:
    return json.dumps(payload, separators=(",", ":")).encode("utf-8")


def _openai_chat_payload(model: str, *, stream: bool = False, tool: bool = False, error: bool = False) -> dict[str, object]:
    payload: dict[str, object] = {
        "model": model,
        "messages": [{"role": "user", "content": "Reply with exactly OK"}],
        "stream": stream,
    }
    if tool:
        payload["messages"] = [{"role": "user", "content": "Weather in Tokyo?"}]
        payload["tools"] = [
            {
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "parameters": {
                        "type": "object",
                        "properties": {"city": {"type": "string"}},
                    },
                },
            }
        ]
    if error:
        payload["messages"] = [{"role": "user", "content": "force_error"}]
    return payload


def _openai_responses_payload(model: str, *, stream: bool = False, tool: bool = False, error: bool = False) -> dict[str, object]:
    payload: dict[str, object] = {
        "model": model,
        "input": "Reply with exactly OK",
        "stream": stream,
    }
    if tool:
        payload["input"] = "Weather in Tokyo?"
        payload["tools"] = [
            {
                "type": "function",
                "name": "get_weather",
                "parameters": {
                    "type": "object",
                    "properties": {"city": {"type": "string"}},
                },
            }
        ]
    if error:
        payload["input"] = "force_error"
    return payload


def _anthropic_payload(model: str, *, stream: bool = False, tool: bool = False, error: bool = False) -> dict[str, object]:
    payload: dict[str, object] = {
        "model": model,
        "max_tokens": 64,
        "messages": [{"role": "user", "content": "Reply with exactly OK"}],
        "stream": stream,
    }
    if tool:
        payload["messages"] = [{"role": "user", "content": "Weather in Tokyo?"}]
        payload["tools"] = [
            {
                "name": "get_weather",
                "input_schema": {
                    "type": "object",
                    "properties": {"city": {"type": "string"}},
                },
            }
        ]
    if error:
        payload["messages"] = [{"role": "user", "content": "force_error"}]
    return payload


def _gemini_payload(*, tool: bool = False, error: bool = False) -> dict[str, object]:
    payload: dict[str, object] = {
        "contents": [
            {
                "role": "user",
                "parts": [{"text": "Reply with exactly OK"}],
            }
        ]
    }
    if tool:
        payload["contents"] = [
            {
                "role": "user",
                "parts": [{"text": "Weather in Tokyo?"}],
            }
        ]
        payload["tools"] = [
            {
                "functionDeclarations": [
                    {
                        "name": "get_weather",
                        "parameters": {
                            "type": "object",
                            "properties": {"city": {"type": "string"}},
                        },
                    }
                ]
            }
        ]
    if error:
        payload["contents"] = [
            {
                "role": "user",
                "parts": [{"text": "force_error"}],
            }
        ]
    return payload


def build_mock_matrix_cases() -> list[MockMatrixCase]:
    cases: list[MockMatrixCase] = []

    def add(
        case_id: str,
        surface: str,
        mode: str,
        path: str,
        payload: dict[str, object],
        *,
        status: int = 200,
        content_type: str = "application/json",
        markers: tuple[str, ...] = ("OK",),
    ) -> None:
        cases.append(
            MockMatrixCase(
                case_id=case_id,
                surface=surface,
                mode=mode,
                path=path,
                payload=payload,
                expected_status=status,
                expected_content_type=content_type,
                expected_markers=markers,
            )
        )

    add(
        "openai_chat_unary",
        "openai_chat",
        "unary",
        "/openai/v1/chat/completions",
        _openai_chat_payload("mock-openai-chat"),
        markers=("OK from OpenAI chat mock",),
    )
    add(
        "openai_chat_stream",
        "openai_chat",
        "stream",
        "/openai/v1/chat/completions",
        _openai_chat_payload("mock-openai-chat", stream=True),
        content_type="text/event-stream",
        markers=("data:", "[DONE]"),
    )
    add(
        "openai_chat_tool",
        "openai_chat",
        "tool",
        "/openai/v1/chat/completions",
        _openai_chat_payload("mock-openai-chat", tool=True),
        markers=("tool_calls", "get_weather"),
    )
    add(
        "openai_chat_error",
        "openai_chat",
        "error",
        "/openai/v1/chat/completions",
        _openai_chat_payload("mock-openai-chat", error=True),
        status=503,
        markers=("forced mock error",),
    )

    add(
        "openai_responses_unary",
        "openai_responses",
        "unary",
        "/openai/v1/responses",
        _openai_responses_payload("mock-openai-responses"),
        markers=("OK from Responses mock",),
    )
    add(
        "openai_responses_stream",
        "openai_responses",
        "stream",
        "/openai/v1/responses",
        _openai_responses_payload("mock-openai-responses", stream=True),
        content_type="text/event-stream",
        markers=("response.output_text.delta", "response.completed"),
    )
    add(
        "openai_responses_tool",
        "openai_responses",
        "tool",
        "/openai/v1/responses",
        _openai_responses_payload("mock-openai-responses", tool=True),
        markers=("function_call", "get_weather"),
    )
    add(
        "openai_responses_error",
        "openai_responses",
        "error",
        "/openai/v1/responses",
        _openai_responses_payload("mock-openai-responses", error=True),
        status=503,
        markers=("forced mock error",),
    )

    add(
        "anthropic_messages_unary",
        "anthropic_messages",
        "unary",
        "/anthropic/v1/messages",
        _anthropic_payload("mock-anthropic"),
        markers=("OK from Anthropic mock",),
    )
    add(
        "anthropic_messages_stream",
        "anthropic_messages",
        "stream",
        "/anthropic/v1/messages",
        _anthropic_payload("mock-anthropic", stream=True),
        content_type="text/event-stream",
        markers=("message_start", "message_stop"),
    )
    add(
        "anthropic_messages_tool",
        "anthropic_messages",
        "tool",
        "/anthropic/v1/messages",
        _anthropic_payload("mock-anthropic", tool=True),
        markers=("tool_use", "get_weather"),
    )
    add(
        "anthropic_messages_error",
        "anthropic_messages",
        "error",
        "/anthropic/v1/messages",
        _anthropic_payload("mock-anthropic", error=True),
        status=503,
        markers=("forced mock error",),
    )

    add(
        "gemini_generate_content_unary",
        "gemini_generate_content",
        "unary",
        "/google/v1beta/models/mock-gemini:generateContent",
        _gemini_payload(),
        markers=("OK from Gemini mock",),
    )
    add(
        "gemini_generate_content_stream",
        "gemini_generate_content",
        "stream",
        "/google/v1beta/models/mock-gemini:streamGenerateContent",
        _gemini_payload(),
        content_type="text/event-stream",
        markers=("data:", "OK from Gemini mock"),
    )
    add(
        "gemini_generate_content_tool",
        "gemini_generate_content",
        "tool",
        "/google/v1beta/models/mock-gemini:generateContent",
        _gemini_payload(tool=True),
        markers=("functionCall", "get_weather"),
    )
    add(
        "gemini_generate_content_error",
        "gemini_generate_content",
        "error",
        "/google/v1beta/models/mock-gemini:generateContent",
        _gemini_payload(error=True),
        status=503,
        markers=("forced mock error",),
    )

    return cases


class MockProviderHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        return

    def _read_json(self) -> object:
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length) if length else b"{}"
        return json.loads(raw or b"{}")

    def _send_json(self, status: int, payload: object) -> None:
        body = _json_bytes(payload)
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()

    def _send_sse(self, payload: str) -> None:
        body = payload.encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()

    def do_POST(self) -> None:
        body = self._read_json()
        path = self.path.split("?", 1)[0]

        if _body_has_force_error(body):
            self._send_json(
                503,
                {
                    "error": {
                        "message": "forced mock error",
                        "type": "mock_error",
                    }
                },
            )
            return

        if path.endswith("/chat/completions"):
            self._handle_openai_chat(body)
            return

        if path.endswith("/responses"):
            self._handle_openai_responses(body)
            return

        if path.endswith("/messages"):
            self._handle_anthropic_messages(body)
            return

        if (
            path.endswith(":generateContent")
            or path.endswith(":streamGenerateContent")
            or path.endswith("/generateContent")
        ):
            self._handle_gemini_generate_content(path, body)
            return

        self._send_json(404, {"error": "not found", "path": path})

    def _handle_openai_chat(self, body: object) -> None:
        model = body.get("model", "mock-openai-chat") if isinstance(body, dict) else "mock-openai-chat"
        if isinstance(body, dict) and body.get("stream"):
            self._send_sse(
                'data: {"id":"chatcmpl_mock","object":"chat.completion.chunk",'
                '"choices":[{"index":0,"delta":{"content":"OK from OpenAI chat mock"}}]}\n\n'
                "data: [DONE]\n\n"
            )
            return
        if _body_has_tool_request(body):
            self._send_json(
                200,
                {
                    "id": "chatcmpl_mock",
                    "object": "chat.completion",
                    "model": model,
                    "choices": [
                        {
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": None,
                                "tool_calls": [
                                    {
                                        "id": "call_mock",
                                        "type": "function",
                                        "function": {
                                            "name": "get_weather",
                                            "arguments": "{\"city\":\"Tokyo\"}",
                                        },
                                    }
                                ],
                            },
                            "finish_reason": "tool_calls",
                        }
                    ],
                },
            )
            return
        self._send_json(
            200,
            {
                "id": "chatcmpl_mock",
                "object": "chat.completion",
                "model": model,
                "choices": [
                    {
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "OK from OpenAI chat mock",
                        },
                        "finish_reason": "stop",
                    }
                ],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            },
        )

    def _handle_openai_responses(self, body: object) -> None:
        model = body.get("model", "mock-openai-responses") if isinstance(body, dict) else "mock-openai-responses"
        if isinstance(body, dict) and body.get("stream"):
            self._send_sse(
                'event: response.output_text.delta\n'
                'data: {"type":"response.output_text.delta","delta":"OK from Responses mock"}\n\n'
                'event: response.completed\n'
                'data: {"type":"response.completed","response":{"id":"resp_mock","status":"completed"}}\n\n'
            )
            return
        if _body_has_tool_request(body):
            self._send_json(
                200,
                {
                    "id": "resp_mock",
                    "object": "response",
                    "model": model,
                    "status": "completed",
                    "output": [
                        {
                            "type": "function_call",
                            "call_id": "call_mock",
                            "name": "get_weather",
                            "arguments": "{\"city\":\"Tokyo\"}",
                        }
                    ],
                },
            )
            return
        self._send_json(
            200,
            {
                "id": "resp_mock",
                "object": "response",
                "model": model,
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [
                            {
                                "type": "output_text",
                                "text": "OK from Responses mock",
                            }
                        ],
                    }
                ],
            },
        )

    def _handle_anthropic_messages(self, body: object) -> None:
        model = body.get("model", "mock-anthropic") if isinstance(body, dict) else "mock-anthropic"
        if isinstance(body, dict) and body.get("stream"):
            self._send_sse(
                'event: message_start\n'
                'data: {"type":"message_start","message":{"id":"msg_mock","type":"message",'
                '"role":"assistant","content":[],"model":"mock-anthropic","stop_reason":null,'
                '"usage":{"input_tokens":1,"output_tokens":0}}}\n\n'
                'event: content_block_delta\n'
                'data: {"type":"content_block_delta","index":0,'
                '"delta":{"type":"text_delta","text":"OK from Anthropic mock"}}\n\n'
                'event: message_stop\n'
                'data: {"type":"message_stop"}\n\n'
            )
            return
        if _body_has_tool_request(body):
            self._send_json(
                200,
                {
                    "id": "msg_mock",
                    "type": "message",
                    "role": "assistant",
                    "model": model,
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "toolu_mock",
                            "name": "get_weather",
                            "input": {"city": "Tokyo"},
                        }
                    ],
                    "stop_reason": "tool_use",
                    "usage": {"input_tokens": 1, "output_tokens": 1},
                },
            )
            return
        self._send_json(
            200,
            {
                "id": "msg_mock",
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [{"type": "text", "text": "OK from Anthropic mock"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 1, "output_tokens": 1},
            },
        )

    def _handle_gemini_generate_content(self, path: str, body: object) -> None:
        if path.endswith(":streamGenerateContent"):
            self._send_sse(
                'data: {"candidates":[{"content":{"parts":[{"text":"OK from Gemini mock"}],'
                '"role":"model"},"finishReason":"STOP"}],"modelVersion":"gemini-mock"}\n\n'
            )
            return
        if _body_has_tool_request(body):
            self._send_json(
                200,
                {
                    "candidates": [
                        {
                            "content": {
                                "role": "model",
                                "parts": [
                                    {
                                        "functionCall": {
                                            "id": "call_mock",
                                            "name": "get_weather",
                                            "args": {"city": "Tokyo"},
                                        }
                                    }
                                ],
                            },
                            "finishReason": "STOP",
                        }
                    ],
                    "modelVersion": "gemini-mock",
                },
            )
            return
        self._send_json(
            200,
            {
                "candidates": [
                    {
                        "content": {
                            "role": "model",
                            "parts": [{"text": "OK from Gemini mock"}],
                        },
                        "finishReason": "STOP",
                    }
                ],
                "usageMetadata": {
                    "promptTokenCount": 1,
                    "candidatesTokenCount": 1,
                    "totalTokenCount": 2,
                },
            },
        )


def start_mock_provider() -> tuple[ThreadingHTTPServer, threading.Thread, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), MockProviderHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    base_url = f"http://127.0.0.1:{server.server_port}"
    return server, thread, base_url


def write_mock_config(path: Path, listen_port: int, mock_base_url: str) -> None:
    config = f"""
listen: 127.0.0.1:{listen_port}
upstream_timeout_secs: 10
upstreams:
  MOCK_OPENAI_CHAT:
    api_root: {json.dumps(mock_base_url + "/v1")}
    format: openai-completion
    credential_actual: dummy
    auth_policy: force_server
  MOCK_OPENAI_RESPONSES:
    api_root: {json.dumps(mock_base_url + "/v1")}
    format: openai-responses
    credential_actual: dummy
    auth_policy: force_server
  MOCK_ANTHROPIC:
    api_root: {json.dumps(mock_base_url + "/v1")}
    format: anthropic
    credential_actual: dummy
    auth_policy: force_server
  MOCK_GOOGLE:
    api_root: {json.dumps(mock_base_url + "/v1beta")}
    format: google
    credential_actual: dummy
    auth_policy: force_server
model_aliases:
  mock-openai-chat: "MOCK_OPENAI_CHAT:gpt-mock"
  mock-openai-responses: "MOCK_OPENAI_RESPONSES:gpt-mock"
  mock-anthropic: "MOCK_ANTHROPIC:claude-mock"
  mock-gemini: "MOCK_GOOGLE:gemini-mock"
"""
    path.write_text(config.strip() + "\n", encoding="utf-8")


@contextlib.contextmanager
def running_mock_proxy(binary: Path) -> Iterator[str]:
    if not binary.exists():
        raise RuntimeError(f"proxy binary not found: {binary}")

    server, _thread, mock_base_url = start_mock_provider()
    process: subprocess.Popen[str] | None = None
    stdout_handle = None
    stderr_handle = None

    with tempfile.TemporaryDirectory(prefix="proxy-mock-matrix-") as tempdir:
        temp_root = Path(tempdir)
        config_path = temp_root / "proxy.yaml"
        stdout_path = temp_root / "proxy.stdout.log"
        stderr_path = temp_root / "proxy.stderr.log"
        proxy_port = free_port()
        write_mock_config(config_path, proxy_port, mock_base_url)

        stdout_handle = stdout_path.open("w", encoding="utf-8")
        stderr_handle = stderr_path.open("w", encoding="utf-8")
        process = subprocess.Popen(
            [str(binary), "--config", str(config_path)],
            stdout=stdout_handle,
            stderr=stderr_handle,
            text=True,
        )
        base_url = f"http://127.0.0.1:{proxy_port}"

        try:
            wait_for_health(base_url)
            yield base_url
        except Exception as error:
            if stderr_path.exists():
                stderr = stderr_path.read_text(encoding="utf-8", errors="replace")
                if stderr:
                    print("--- proxy stderr ---", file=sys.stderr)
                    print(stderr[-4000:], file=sys.stderr)
            raise error
        finally:
            if process is not None and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
            if stdout_handle is not None:
                stdout_handle.close()
            if stderr_handle is not None:
                stderr_handle.close()
            server.shutdown()
            server.server_close()


def run_mock_case(base_url: str, case: MockMatrixCase) -> dict[str, object]:
    started = time.perf_counter()
    status, headers, body = http_json(f"{base_url}{case.path}", case.payload)
    duration_ms = round((time.perf_counter() - started) * 1000, 3)
    content_type = headers.get("content-type", "")
    failures = []

    if status != case.expected_status:
        failures.append(f"expected status {case.expected_status}, got {status}")
    if case.expected_content_type not in content_type:
        failures.append(
            f"expected content-type containing {case.expected_content_type!r}, got {content_type!r}"
        )
    for marker in case.expected_markers:
        if marker not in body:
            failures.append(f"missing marker {marker!r}")

    return {
        "case_id": case.case_id,
        "surface": case.surface,
        "mode": case.mode,
        "status": "passed" if not failures else "failed",
        "http_status": status,
        "content_type": content_type,
        "duration_ms": duration_ms,
        "failures": failures,
    }


def run_mock_matrix(binary: Path) -> dict[str, object]:
    cases = build_mock_matrix_cases()
    with running_mock_proxy(binary) as base_url:
        results = [run_mock_case(base_url, case) for case in cases]
    failed = [result for result in results if result["status"] != "passed"]
    return {
        "status": "passed" if not failed else "failed",
        "gate": "mock-endpoint-matrix",
        "case_count": len(results),
        "failed": len(failed),
        "results": results,
    }


def percentile(values: list[float], percent: float) -> float:
    ordered = sorted(values)
    index = max(0, math.ceil((percent / 100.0) * len(ordered)) - 1)
    return ordered[index]


def run_perf_gate(
    binary: Path,
    *,
    iterations: int = PERF_DEFAULT_ITERATIONS,
    p95_threshold_ms: float = PERF_DEFAULT_P95_MS,
    total_threshold_ms: float = PERF_DEFAULT_TOTAL_MS,
) -> dict[str, object]:
    if iterations <= 0:
        raise RuntimeError("--perf-iterations must be greater than zero")

    case = next(
        case for case in build_mock_matrix_cases() if case.case_id == "openai_chat_unary"
    )
    durations_ms: list[float] = []

    with running_mock_proxy(binary) as base_url:
        for _ in range(3):
            warmup = run_mock_case(base_url, case)
            if warmup["status"] != "passed":
                return {
                    "status": "failed",
                    "gate": "perf",
                    "reason": "warmup request failed",
                    "warmup": warmup,
                }

        total_started = time.perf_counter()
        for _ in range(iterations):
            result = run_mock_case(base_url, case)
            if result["status"] != "passed":
                return {
                    "status": "failed",
                    "gate": "perf",
                    "reason": "measured request failed",
                    "result": result,
                }
            durations_ms.append(float(result["duration_ms"]))
        total_ms = round((time.perf_counter() - total_started) * 1000, 3)

    p95_ms = round(percentile(durations_ms, 95), 3)
    max_ms = round(max(durations_ms), 3)
    mean_ms = round(sum(durations_ms) / len(durations_ms), 3)
    failures = []
    if p95_ms > p95_threshold_ms:
        failures.append(f"p95_ms {p95_ms} exceeded threshold {p95_threshold_ms}")
    if total_ms > total_threshold_ms:
        failures.append(f"total_ms {total_ms} exceeded threshold {total_threshold_ms}")

    return {
        "status": "passed" if not failures else "failed",
        "gate": "perf",
        "iterations": iterations,
        "p95_ms": p95_ms,
        "max_ms": max_ms,
        "mean_ms": mean_ms,
        "total_ms": total_ms,
        "thresholds": {
            "p95_ms": p95_threshold_ms,
            "total_ms": total_threshold_ms,
        },
        "failures": failures,
    }


def emit_machine_report(report: dict[str, object], json_out: str | None) -> None:
    if json_out:
        output_path = Path(json_out)
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(report, sort_keys=True))


def build_config(
    path: Path,
    listen_port: int,
    anthropic_key: str,
    openai_key: str,
    anthropic_base: str,
    openai_base: str,
    anthropic_model: str,
    openai_model: str,
) -> None:
    config = f"""
listen: 127.0.0.1:{listen_port}
upstream_timeout_secs: 120
upstreams:
  GLM-ANTHROPIC:
    api_root: {json.dumps(anthropic_base)}
    format: anthropic
    credential_actual: {json.dumps(anthropic_key)}
    auth_policy: force_server
  GLM-OPENAI:
    api_root: {json.dumps(openai_base)}
    format: openai-completion
    credential_actual: {json.dumps(openai_key)}
    auth_policy: force_server
model_aliases:
  glm-anthropic: {json.dumps(f"GLM-ANTHROPIC:{anthropic_model}")}
  glm-openai: {json.dumps(f"GLM-OPENAI:{openai_model}")}
"""
    path.write_text(config.strip() + "\n", encoding="utf-8")


def run_matrix(base_url: str) -> None:
    expect_json_case(
        base_url,
        "/openai/v1/chat/completions",
        {"model": "glm-anthropic", "messages": [{"role": "user", "content": "Reply with exactly OK"}], "stream": False},
        '"content": "OK',
        "anthropic upstream via chat completions",
    )
    expect_json_case(
        base_url,
        "/openai/v1/responses",
        {"model": "glm-anthropic", "input": "Reply with exactly OK", "stream": False},
        '"text": "OK',
        "anthropic upstream via responses",
    )
    expect_json_case(
        base_url,
        "/anthropic/v1/messages",
        {"model": "glm-anthropic", "max_tokens": 32, "messages": [{"role": "user", "content": "Reply with exactly OK"}], "stream": False},
        '"text": "OK',
        "anthropic upstream via messages",
    )
    expect_sse_case(
        base_url,
        "/openai/v1/responses",
        {"model": "glm-anthropic", "input": "Reply with exactly OK", "stream": True},
        ["response.completed", "OK"],
        "anthropic upstream via responses stream",
    )
    expect_sse_case(
        base_url,
        "/anthropic/v1/messages",
        {"model": "glm-anthropic", "max_tokens": 32, "messages": [{"role": "user", "content": "Reply with exactly OK"}], "stream": True},
        ["message_start", "message_stop"],
        "anthropic upstream via messages stream",
    )

    expect_json_case(
        base_url,
        "/openai/v1/chat/completions",
        {"model": "glm-openai", "messages": [{"role": "user", "content": "Reply with exactly OK"}], "stream": False},
        '"content": "OK',
        "openai upstream via chat completions",
    )
    expect_json_case(
        base_url,
        "/openai/v1/responses",
        {"model": "glm-openai", "input": "Reply with exactly OK", "stream": False},
        '"text": "OK',
        "openai upstream via responses",
    )
    expect_json_case(
        base_url,
        "/anthropic/v1/messages",
        {"model": "glm-openai", "max_tokens": 32, "messages": [{"role": "user", "content": "Reply with exactly OK"}], "stream": False},
        ['"type": "message"', '"role": "assistant"'],
        "openai upstream via messages",
    )
    expect_sse_case(
        base_url,
        "/openai/v1/chat/completions",
        {"model": "glm-openai", "messages": [{"role": "user", "content": "Reply with exactly OK"}], "stream": True},
        ["data:", "[DONE]"],
        "openai upstream via chat completions stream",
    )
    expect_sse_case(
        base_url,
        "/openai/v1/responses",
        {"model": "glm-openai", "input": "Reply with exactly OK", "stream": True},
        ["response.completed", "OK"],
        "openai upstream via responses stream",
    )
    expect_sse_case(
        base_url,
        "/anthropic/v1/messages",
        {"model": "glm-openai", "max_tokens": 32, "messages": [{"role": "user", "content": "Reply with exactly OK"}], "stream": True},
        ["message_start", "message_stop"],
        "openai upstream via messages stream",
    )


def run_real_provider_smoke(args: argparse.Namespace) -> int:
    api_key = os.environ.get("GLM_APIKEY")
    if not api_key:
        print("GLM_APIKEY is required", file=sys.stderr)
        return 2

    binary = Path(args.binary)
    if not binary.exists():
        print(f"proxy binary not found: {binary}", file=sys.stderr)
        return 2

    port = free_port()
    base_url = f"http://127.0.0.1:{port}"

    with tempfile.TemporaryDirectory(prefix="proxy-real-matrix-") as tempdir:
        config_path = Path(tempdir) / "proxy.yaml"
        build_config(
            config_path,
            port,
            api_key,
            api_key,
            args.anthropic_base_url,
            args.openai_base_url,
            args.anthropic_model,
            args.openai_model,
        )

        proc = subprocess.Popen(
            [str(binary), "--config", str(config_path)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            wait_for_health(base_url)
            run_matrix(base_url)
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=5)
        if proc.returncode not in (0, -15):
            stderr = proc.stderr.read() if proc.stderr else ""
            print(stderr, file=sys.stderr)
            return 1
    return 0


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mock", action="store_true", help="run deterministic local mock endpoint matrix")
    parser.add_argument("--perf", action="store_true", help="run local deterministic perf gate; requires --mock")
    parser.add_argument("--json-out", help="write machine-readable gate result JSON")
    parser.add_argument("--real-provider-smoke", action="store_true", help="run protected real-provider smoke matrix")
    parser.add_argument("--binary", default="./target/debug/llm-universal-proxy")
    parser.add_argument("--perf-iterations", type=int, default=PERF_DEFAULT_ITERATIONS)
    parser.add_argument("--perf-p95-ms", type=float, default=PERF_DEFAULT_P95_MS)
    parser.add_argument("--perf-total-ms", type=float, default=PERF_DEFAULT_TOTAL_MS)
    parser.add_argument("--anthropic-base-url", default=os.environ.get("ANTHROPIC_UPSTREAM_BASE_URL", "https://open.bigmodel.cn/api/anthropic/v1"))
    parser.add_argument("--openai-base-url", default=os.environ.get("OPENAI_UPSTREAM_BASE_URL", "https://open.bigmodel.cn/api/paas/v4"))
    parser.add_argument("--anthropic-model", default=os.environ.get("ANTHROPIC_UPSTREAM_MODEL", "GLM-5"))
    parser.add_argument("--openai-model", default=os.environ.get("OPENAI_UPSTREAM_MODEL", "glm-4.7-flash"))
    args = parser.parse_args(argv)
    if args.perf and not args.mock:
        parser.error("--perf requires --mock")
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.mock:
        if args.perf:
            report = run_perf_gate(
                Path(args.binary),
                iterations=args.perf_iterations,
                p95_threshold_ms=args.perf_p95_ms,
                total_threshold_ms=args.perf_total_ms,
            )
        else:
            report = run_mock_matrix(Path(args.binary))
        emit_machine_report(report, args.json_out)
        return 0 if report.get("status") == "passed" else 1

    return run_real_provider_smoke(args)


if __name__ == "__main__":
    raise SystemExit(main())
