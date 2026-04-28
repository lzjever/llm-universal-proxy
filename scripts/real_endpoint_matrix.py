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
REAL_PROVIDER_REQUIRED_ENVS = (
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "GEMINI_API_KEY",
    "MINIMAX_API_KEY",
)
REAL_PROVIDER_GATE = "real-provider-smoke"
COMPATIBLE_PROVIDER_GATE = "compatible-provider-smoke"
COMPATIBLE_PROVIDER_CLAIM_SCOPE = "compatible_provider_openai_chat_completions_and_anthropic_messages"
COMPATIBLE_PROVIDER_DEFAULT_LABEL = "compatible-provider"
COMPAT_PROVIDER_SHARED_CREDENTIAL_ENV = "COMPAT_PROVIDER_API_KEY"
COMPAT_OPENAI_CREDENTIAL_ENV = "COMPAT_OPENAI_API_KEY"
COMPAT_ANTHROPIC_CREDENTIAL_ENV = "COMPAT_ANTHROPIC_API_KEY"
COMPAT_PROVIDER_SECRET_ENVS = (
    COMPAT_PROVIDER_SHARED_CREDENTIAL_ENV,
    COMPAT_OPENAI_CREDENTIAL_ENV,
    COMPAT_ANTHROPIC_CREDENTIAL_ENV,
)
COMPAT_OPENAI_BASE_URL_ENV = "COMPAT_OPENAI_BASE_URL"
COMPAT_OPENAI_MODEL_ENV = "COMPAT_OPENAI_MODEL"
COMPAT_ANTHROPIC_BASE_URL_ENV = "COMPAT_ANTHROPIC_BASE_URL"
COMPAT_ANTHROPIC_MODEL_ENV = "COMPAT_ANTHROPIC_MODEL"
REAL_OPENAI_DEFAULT_MODEL = "gpt-5-mini"
REAL_ANTHROPIC_DEFAULT_MODEL = "claude-sonnet-4-6"
REAL_GEMINI_DEFAULT_MODEL = "gemini-2.5-flash"
REAL_MINIMAX_DEFAULT_MODEL = "MiniMax-M2.7"
SECRET_REDACTION_PLACEHOLDER_PREFIX = "[REDACTED:"
MIN_SECRET_REDACTION_LENGTH = 4
AUTH_MODE_ENV = "LLM_UNIVERSAL_PROXY_AUTH_MODE"
PROXY_KEY_ENV = "LLM_UNIVERSAL_PROXY_KEY"
DEFAULT_PROXY_KEY = "llmup-endpoint-matrix-proxy-key"
MOCK_PROVIDER_KEY_ENV = "MOCK_PROVIDER_API_KEY"


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


@dataclass(frozen=True)
class RealProviderMatrixCase:
    case_id: str
    provider: str
    surface: str
    mode: str
    feature: str
    provider_key_env: str
    required: bool
    default_model: str
    model_alias: str
    upstream_format: str
    path: str
    payload: dict[str, object]
    expected_status: int
    expected_content_type: str
    expected_markers: tuple[str, ...]


@dataclass(frozen=True)
class CompatibleProviderConfig:
    provider_label: str
    openai_base_url: str | None
    openai_model: str | None
    openai_provider_key_env: str | None
    anthropic_base_url: str | None
    anthropic_model: str | None
    anthropic_provider_key_env: str | None
    missing_config: tuple[str, ...]


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def matrix_proxy_key() -> str:
    return os.environ.get(PROXY_KEY_ENV) or DEFAULT_PROXY_KEY


def http_json(
    url: str,
    payload: dict[str, object],
    timeout: int = 60,
    *,
    proxy_key: str | None = None,
) -> tuple[int, dict[str, str], str]:
    data = json.dumps(payload).encode("utf-8")
    proxy_key = proxy_key or matrix_proxy_key()
    req = urllib.request.Request(
        url,
        data=data,
        headers={
            "content-type": "application/json",
            "Authorization": f"Bearer {proxy_key}",
        },
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


def _body_text(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True)


def _body_has_force_error(value: object) -> bool:
    return "force_error" in _body_text(value)


def _body_has_tool_request(value: object) -> bool:
    rendered = _body_text(value)
    return any(marker in rendered for marker in ("tools", "functionDeclarations"))


def _json_bytes(payload: object) -> bytes:
    return json.dumps(payload, separators=(",", ":")).encode("utf-8")


def _secret_redaction_patterns() -> list[tuple[str, str]]:
    patterns: list[tuple[str, str]] = []
    seen: set[str] = set()
    for env_name in REAL_PROVIDER_REQUIRED_ENVS + COMPAT_PROVIDER_SECRET_ENVS:
        secret = os.environ.get(env_name)
        if not secret or len(secret) < MIN_SECRET_REDACTION_LENGTH:
            continue
        placeholder = f"{SECRET_REDACTION_PLACEHOLDER_PREFIX}{env_name}]"
        for pattern in (secret, json.dumps(secret)[1:-1]):
            if pattern and pattern not in seen:
                patterns.append((pattern, placeholder))
                seen.add(pattern)
    patterns.sort(key=lambda item: len(item[0]), reverse=True)
    return patterns


def redact_real_provider_secrets(value: object) -> object:
    if isinstance(value, str):
        redacted = value
        for secret, placeholder in _secret_redaction_patterns():
            redacted = redacted.replace(secret, placeholder)
        return redacted
    if isinstance(value, dict):
        return {
            redact_real_provider_secrets(key): redact_real_provider_secrets(item)
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [redact_real_provider_secrets(item) for item in value]
    if isinstance(value, tuple):
        return tuple(redact_real_provider_secrets(item) for item in value)
    return value


def redact_real_provider_report(report: dict[str, object]) -> dict[str, object]:
    redacted = redact_real_provider_secrets(report)
    if not isinstance(redacted, dict):
        raise TypeError("redacted report must remain a dict")
    return redacted


def redact_real_provider_text(value: str) -> str:
    redacted = redact_real_provider_secrets(value)
    if not isinstance(redacted, str):
        raise TypeError("redacted text must remain a string")
    return redacted


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
        payload["tool_choice"] = {
            "type": "function",
            "function": {"name": "get_weather"},
        }
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
        payload["tool_choice"] = {
            "type": "function",
            "name": "get_weather",
        }
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
        payload["tool_choice"] = {"type": "tool", "name": "get_weather"}
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
        payload["toolConfig"] = {
            "functionCallingConfig": {
                "mode": "ANY",
                "allowedFunctionNames": ["get_weather"],
            }
        }
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


def build_perf_matrix_cases() -> list[MockMatrixCase]:
    case_by_id = {case.case_id: case for case in build_mock_matrix_cases()}
    perf_case_ids = (
        "openai_chat_unary",
        "openai_chat_stream",
        "openai_responses_unary",
        "openai_responses_tool",
        "anthropic_messages_unary",
        "gemini_generate_content_unary",
    )
    return [case_by_id[case_id] for case_id in perf_case_ids]


def _responses_high_risk_state_payload(model: str) -> dict[str, object]:
    return {
        "model": model,
        "input": "Hello",
        "previous_response_id": "resp_cross_provider_state_should_fail_closed",
    }


def build_real_provider_matrix_cases(
    *,
    openai_model: str = REAL_OPENAI_DEFAULT_MODEL,
    anthropic_model: str = REAL_ANTHROPIC_DEFAULT_MODEL,
    gemini_model: str = REAL_GEMINI_DEFAULT_MODEL,
    minimax_model: str = REAL_MINIMAX_DEFAULT_MODEL,
) -> list[RealProviderMatrixCase]:
    cases: list[RealProviderMatrixCase] = []

    def add(
        case_id: str,
        provider: str,
        surface: str,
        mode: str,
        feature: str,
        provider_key_env: str,
        default_model: str,
        model_alias: str,
        upstream_format: str,
        path: str,
        payload: dict[str, object],
        *,
        required: bool = True,
        status: int = 200,
        content_type: str = "application/json",
        markers: tuple[str, ...] = ("OK",),
    ) -> None:
        cases.append(
            RealProviderMatrixCase(
                case_id=case_id,
                provider=provider,
                surface=surface,
                mode=mode,
                feature=feature,
                provider_key_env=provider_key_env,
                required=required,
                default_model=default_model,
                model_alias=model_alias,
                upstream_format=upstream_format,
                path=path,
                payload=payload,
                expected_status=status,
                expected_content_type=content_type,
                expected_markers=markers,
            )
        )

    add(
        "openai_responses_unary",
        "openai",
        "responses",
        "unary",
        "responses_unary",
        "OPENAI_API_KEY",
        openai_model,
        "real-openai-responses",
        "openai-responses",
        "/openai/v1/responses",
        _openai_responses_payload("real-openai-responses"),
    )
    add(
        "openai_responses_stream",
        "openai",
        "responses",
        "stream",
        "responses_stream",
        "OPENAI_API_KEY",
        openai_model,
        "real-openai-responses",
        "openai-responses",
        "/openai/v1/responses",
        _openai_responses_payload("real-openai-responses", stream=True),
        content_type="text/event-stream",
        markers=("response.completed", "OK"),
    )
    add(
        "openai_chat_tool",
        "openai",
        "chat",
        "tool",
        "chat_tool",
        "OPENAI_API_KEY",
        openai_model,
        "real-openai-chat",
        "openai-completion",
        "/openai/v1/chat/completions",
        _openai_chat_payload("real-openai-chat", tool=True),
        markers=("tool_calls", "get_weather"),
    )
    add(
        "openai_responses_high_risk_state_fail_closed",
        "openai",
        "responses",
        "fail_closed",
        "high_risk_state",
        "OPENAI_API_KEY",
        openai_model,
        "real-openai-chat",
        "openai-completion",
        "/openai/v1/responses",
        _responses_high_risk_state_payload("real-openai-chat"),
        status=400,
        markers=("previous_response_id",),
    )

    add(
        "anthropic_messages_unary",
        "anthropic",
        "messages",
        "unary",
        "messages_unary",
        "ANTHROPIC_API_KEY",
        anthropic_model,
        "real-anthropic-messages",
        "anthropic",
        "/anthropic/v1/messages",
        _anthropic_payload("real-anthropic-messages"),
    )
    add(
        "anthropic_messages_stream",
        "anthropic",
        "messages",
        "stream",
        "messages_stream",
        "ANTHROPIC_API_KEY",
        anthropic_model,
        "real-anthropic-messages",
        "anthropic",
        "/anthropic/v1/messages",
        _anthropic_payload("real-anthropic-messages", stream=True),
        content_type="text/event-stream",
        markers=("message_start", "message_stop"),
    )
    add(
        "anthropic_messages_client_tool",
        "anthropic",
        "messages",
        "tool",
        "client_tool",
        "ANTHROPIC_API_KEY",
        anthropic_model,
        "real-anthropic-messages",
        "anthropic",
        "/anthropic/v1/messages",
        _anthropic_payload("real-anthropic-messages", tool=True),
        markers=("tool_use", "get_weather"),
    )
    add(
        "anthropic_responses_high_risk_state_fail_closed",
        "anthropic",
        "messages",
        "fail_closed",
        "high_risk_state",
        "ANTHROPIC_API_KEY",
        anthropic_model,
        "real-anthropic-messages",
        "anthropic",
        "/openai/v1/responses",
        _responses_high_risk_state_payload("real-anthropic-messages"),
        status=400,
        markers=("previous_response_id",),
    )

    add(
        "gemini_generate_content_unary",
        "gemini",
        "generateContent",
        "unary",
        "generate_content_unary",
        "GEMINI_API_KEY",
        gemini_model,
        "real-gemini-generate-content",
        "google",
        "/google/v1beta/models/real-gemini-generate-content:generateContent",
        _gemini_payload(),
    )
    add(
        "gemini_stream_generate_content",
        "gemini",
        "streamGenerateContent",
        "stream",
        "stream_generate_content",
        "GEMINI_API_KEY",
        gemini_model,
        "real-gemini-generate-content",
        "google",
        "/google/v1beta/models/real-gemini-generate-content:streamGenerateContent",
        _gemini_payload(),
        content_type="text/event-stream",
        markers=("data:", "OK"),
    )
    add(
        "gemini_function_declarations_tool",
        "gemini",
        "generateContent",
        "tool",
        "function_declarations",
        "GEMINI_API_KEY",
        gemini_model,
        "real-gemini-generate-content",
        "google",
        "/google/v1beta/models/real-gemini-generate-content:generateContent",
        _gemini_payload(tool=True),
        markers=("functionCall", "get_weather"),
    )
    add(
        "gemini_responses_high_risk_state_fail_closed",
        "gemini",
        "generateContent",
        "fail_closed",
        "high_risk_state",
        "GEMINI_API_KEY",
        gemini_model,
        "real-gemini-generate-content",
        "google",
        "/openai/v1/responses",
        _responses_high_risk_state_payload("real-gemini-generate-content"),
        status=400,
        markers=("previous_response_id",),
    )

    add(
        "minimax_openai_chat_unary",
        "minimax",
        "openai_chat",
        "unary",
        "chat_unary",
        "MINIMAX_API_KEY",
        minimax_model,
        "real-minimax-chat",
        "openai-completion",
        "/openai/v1/chat/completions",
        _openai_chat_payload("real-minimax-chat"),
    )
    add(
        "minimax_openai_chat_stream",
        "minimax",
        "openai_chat",
        "stream",
        "chat_stream",
        "MINIMAX_API_KEY",
        minimax_model,
        "real-minimax-chat",
        "openai-completion",
        "/openai/v1/chat/completions",
        _openai_chat_payload("real-minimax-chat", stream=True),
        content_type="text/event-stream",
        markers=("data:", "[DONE]"),
    )
    add(
        "minimax_openai_chat_tool",
        "minimax",
        "openai_chat",
        "tool",
        "chat_tool",
        "MINIMAX_API_KEY",
        minimax_model,
        "real-minimax-chat",
        "openai-completion",
        "/openai/v1/chat/completions",
        _openai_chat_payload("real-minimax-chat", tool=True),
        markers=("tool_calls", "get_weather"),
    )
    add(
        "minimax_unsupported_lifecycle_state_fail_closed",
        "minimax",
        "openai_chat",
        "fail_closed",
        "unsupported_lifecycle_state",
        "MINIMAX_API_KEY",
        minimax_model,
        "real-minimax-chat",
        "openai-completion",
        "/openai/v1/responses",
        _responses_high_risk_state_payload("real-minimax-chat"),
        status=400,
        markers=("previous_response_id",),
    )

    return cases


def build_compatible_provider_matrix_cases(
    *,
    openai_model: str,
    anthropic_model: str,
    openai_provider_key_env: str,
    anthropic_provider_key_env: str,
) -> list[RealProviderMatrixCase]:
    cases: list[RealProviderMatrixCase] = []

    def add(
        case_id: str,
        surface: str,
        mode: str,
        feature: str,
        provider_key_env: str,
        default_model: str,
        model_alias: str,
        upstream_format: str,
        path: str,
        payload: dict[str, object],
        *,
        status: int = 200,
        content_type: str = "application/json",
        markers: tuple[str, ...] = ("OK",),
    ) -> None:
        cases.append(
            RealProviderMatrixCase(
                case_id=case_id,
                provider="compatible",
                surface=surface,
                mode=mode,
                feature=feature,
                provider_key_env=provider_key_env,
                required=True,
                default_model=default_model,
                model_alias=model_alias,
                upstream_format=upstream_format,
                path=path,
                payload=payload,
                expected_status=status,
                expected_content_type=content_type,
                expected_markers=markers,
            )
        )

    add(
        "compatible_openai_chat_completions_unary",
        "openai_chat_completions",
        "unary",
        "chat_completions_unary",
        openai_provider_key_env,
        openai_model,
        "compat-openai-chat",
        "openai-completion",
        "/openai/v1/chat/completions",
        _openai_chat_payload("compat-openai-chat"),
    )
    add(
        "compatible_openai_chat_completions_stream",
        "openai_chat_completions",
        "stream",
        "chat_completions_stream",
        openai_provider_key_env,
        openai_model,
        "compat-openai-chat",
        "openai-completion",
        "/openai/v1/chat/completions",
        _openai_chat_payload("compat-openai-chat", stream=True),
        content_type="text/event-stream",
        markers=("data:",),
    )
    add(
        "compatible_anthropic_messages_unary",
        "anthropic_messages",
        "unary",
        "messages_unary",
        anthropic_provider_key_env,
        anthropic_model,
        "compat-anthropic-messages",
        "anthropic",
        "/anthropic/v1/messages",
        _anthropic_payload("compat-anthropic-messages"),
    )
    add(
        "compatible_anthropic_messages_stream",
        "anthropic_messages",
        "stream",
        "messages_stream",
        anthropic_provider_key_env,
        anthropic_model,
        "compat-anthropic-messages",
        "anthropic",
        "/anthropic/v1/messages",
        _anthropic_payload("compat-anthropic-messages", stream=True),
        content_type="text/event-stream",
        markers=("message_start", "message_stop"),
    )
    add(
        "compatible_openai_responses_state_fail_closed",
        "openai_chat_completions",
        "fail_closed",
        "responses_stateful_controls_rejected",
        openai_provider_key_env,
        openai_model,
        "compat-openai-chat",
        "openai-completion",
        "/openai/v1/responses",
        _responses_high_risk_state_payload("compat-openai-chat"),
        status=400,
        markers=("previous_response_id",),
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


def proxy_key_env() -> dict[str, str]:
    env = os.environ.copy()
    env[AUTH_MODE_ENV] = "proxy_key"
    env[PROXY_KEY_ENV] = matrix_proxy_key()
    return env


def mock_proxy_env() -> dict[str, str]:
    env = proxy_key_env()
    env[MOCK_PROVIDER_KEY_ENV] = "dummy"
    return env


def write_mock_config(path: Path, listen_port: int, mock_base_url: str) -> None:
    config = f"""
listen: 127.0.0.1:{listen_port}
upstream_timeout_secs: 10
upstreams:
  MOCK_OPENAI_CHAT:
    api_root: {json.dumps(mock_base_url + "/v1")}
    format: openai-completion
    provider_key_env: {MOCK_PROVIDER_KEY_ENV}
  MOCK_OPENAI_RESPONSES:
    api_root: {json.dumps(mock_base_url + "/v1")}
    format: openai-responses
    provider_key_env: {MOCK_PROVIDER_KEY_ENV}
  MOCK_ANTHROPIC:
    api_root: {json.dumps(mock_base_url + "/v1")}
    format: anthropic
    provider_key_env: {MOCK_PROVIDER_KEY_ENV}
  MOCK_GOOGLE:
    api_root: {json.dumps(mock_base_url + "/v1beta")}
    format: google
    provider_key_env: {MOCK_PROVIDER_KEY_ENV}
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
            env=mock_proxy_env(),
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

    cases = build_perf_matrix_cases()
    durations_ms: list[float] = []
    durations_by_case: dict[str, list[float]] = {case.case_id: [] for case in cases}

    with running_mock_proxy(binary) as base_url:
        for case in cases:
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
            for case in cases:
                result = run_mock_case(base_url, case)
                if result["status"] != "passed":
                    return {
                        "status": "failed",
                        "gate": "perf",
                        "reason": "measured request failed",
                        "result": result,
                    }
                duration_ms = float(result["duration_ms"])
                durations_ms.append(duration_ms)
                durations_by_case[case.case_id].append(duration_ms)
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
        "case_count": len(cases),
        "request_count": len(durations_ms),
        "cases": [case.case_id for case in cases],
        "surfaces": sorted({case.surface for case in cases}),
        "p95_ms": p95_ms,
        "max_ms": max_ms,
        "mean_ms": mean_ms,
        "total_ms": total_ms,
        "per_case": {
            case_id: {
                "p95_ms": round(percentile(values, 95), 3),
                "max_ms": round(max(values), 3),
                "mean_ms": round(sum(values) / len(values), 3),
            }
            for case_id, values in durations_by_case.items()
        },
        "thresholds": {
            "p95_ms": p95_threshold_ms,
            "total_ms": total_threshold_ms,
        },
        "failures": failures,
    }


def emit_machine_report(report: dict[str, object], json_out: str | None) -> None:
    report = redact_real_provider_report(report)
    if json_out:
        output_path = Path(json_out)
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(report, sort_keys=True))


def _nonempty(value: str | None) -> str | None:
    if value is None:
        return None
    value = value.strip()
    return value if value else None


def _resolve_compatible_provider_key_env(surface_env: str) -> str | None:
    if _nonempty(os.environ.get(COMPAT_PROVIDER_SHARED_CREDENTIAL_ENV)):
        return COMPAT_PROVIDER_SHARED_CREDENTIAL_ENV
    if _nonempty(os.environ.get(surface_env)):
        return surface_env
    return None


def resolve_compatible_provider_config(args: argparse.Namespace) -> CompatibleProviderConfig:
    openai_base_url = _nonempty(args.compat_openai_base_url)
    openai_model = _nonempty(args.compat_openai_model)
    anthropic_base_url = _nonempty(args.compat_anthropic_base_url)
    anthropic_model = _nonempty(args.compat_anthropic_model)
    provider_label = _nonempty(args.compat_provider_label) or COMPATIBLE_PROVIDER_DEFAULT_LABEL
    openai_provider_key_env = _resolve_compatible_provider_key_env(COMPAT_OPENAI_CREDENTIAL_ENV)
    anthropic_provider_key_env = _resolve_compatible_provider_key_env(COMPAT_ANTHROPIC_CREDENTIAL_ENV)

    missing_config: list[str] = []
    if not openai_base_url:
        missing_config.append(COMPAT_OPENAI_BASE_URL_ENV)
    if not openai_model:
        missing_config.append(COMPAT_OPENAI_MODEL_ENV)
    if not anthropic_base_url:
        missing_config.append(COMPAT_ANTHROPIC_BASE_URL_ENV)
    if not anthropic_model:
        missing_config.append(COMPAT_ANTHROPIC_MODEL_ENV)
    if not openai_provider_key_env:
        missing_config.append(f"{COMPAT_PROVIDER_SHARED_CREDENTIAL_ENV} or {COMPAT_OPENAI_CREDENTIAL_ENV}")
    if not anthropic_provider_key_env:
        missing_config.append(f"{COMPAT_PROVIDER_SHARED_CREDENTIAL_ENV} or {COMPAT_ANTHROPIC_CREDENTIAL_ENV}")

    return CompatibleProviderConfig(
        provider_label=provider_label,
        openai_base_url=openai_base_url,
        openai_model=openai_model,
        openai_provider_key_env=openai_provider_key_env,
        anthropic_base_url=anthropic_base_url,
        anthropic_model=anthropic_model,
        anthropic_provider_key_env=anthropic_provider_key_env,
        missing_config=tuple(missing_config),
    )


def compatible_provider_surfaces(config: CompatibleProviderConfig) -> list[dict[str, object]]:
    return [
        {
            "name": "openai_chat_completions",
            "format": "openai-completion",
            "base_url_env": COMPAT_OPENAI_BASE_URL_ENV,
            "model_env": COMPAT_OPENAI_MODEL_ENV,
            "provider_key_env": config.openai_provider_key_env,
            "provider_key_env_alternatives": [
                COMPAT_PROVIDER_SHARED_CREDENTIAL_ENV,
                COMPAT_OPENAI_CREDENTIAL_ENV,
            ],
            "configured": bool(
                config.openai_base_url
                and config.openai_model
                and config.openai_provider_key_env
            ),
        },
        {
            "name": "anthropic_messages",
            "format": "anthropic",
            "base_url_env": COMPAT_ANTHROPIC_BASE_URL_ENV,
            "model_env": COMPAT_ANTHROPIC_MODEL_ENV,
            "provider_key_env": config.anthropic_provider_key_env,
            "provider_key_env_alternatives": [
                COMPAT_PROVIDER_SHARED_CREDENTIAL_ENV,
                COMPAT_ANTHROPIC_CREDENTIAL_ENV,
            ],
            "configured": bool(
                config.anthropic_base_url
                and config.anthropic_model
                and config.anthropic_provider_key_env
            ),
        },
    ]


def write_compatible_provider_config(
    path: Path,
    listen_port: int,
    config: CompatibleProviderConfig,
) -> None:
    if config.missing_config:
        missing_text = ", ".join(config.missing_config)
        raise RuntimeError(f"missing compatible provider configuration: {missing_text}")
    if (
        not config.openai_base_url
        or not config.openai_model
        or not config.openai_provider_key_env
        or not config.anthropic_base_url
        or not config.anthropic_model
        or not config.anthropic_provider_key_env
    ):
        raise RuntimeError("missing compatible provider configuration")

    rendered = f"""
listen: 127.0.0.1:{listen_port}
upstream_timeout_secs: 120
upstreams:
  COMPAT_OPENAI_CHAT:
    api_root: {json.dumps(config.openai_base_url)}
    format: openai-completion
    provider_key_env: {config.openai_provider_key_env}
  COMPAT_ANTHROPIC:
    api_root: {json.dumps(config.anthropic_base_url)}
    format: anthropic
    provider_key_env: {config.anthropic_provider_key_env}
model_aliases:
  compat-openai-chat: {json.dumps(f"COMPAT_OPENAI_CHAT:{config.openai_model}")}
  compat-anthropic-messages: {json.dumps(f"COMPAT_ANTHROPIC:{config.anthropic_model}")}
"""
    path.write_text(rendered.strip() + "\n", encoding="utf-8")


def write_real_provider_config(path: Path, listen_port: int, args: argparse.Namespace) -> None:
    config = f"""
listen: 127.0.0.1:{listen_port}
upstream_timeout_secs: 120
upstreams:
  REAL_OPENAI_CHAT:
    api_root: {json.dumps(args.openai_base_url)}
    format: openai-completion
    provider_key_env: OPENAI_API_KEY
  REAL_OPENAI_RESPONSES:
    api_root: {json.dumps(args.openai_base_url)}
    format: openai-responses
    provider_key_env: OPENAI_API_KEY
  REAL_ANTHROPIC:
    api_root: {json.dumps(args.anthropic_base_url)}
    format: anthropic
    provider_key_env: ANTHROPIC_API_KEY
  REAL_GEMINI:
    api_root: {json.dumps(args.gemini_base_url)}
    format: google
    provider_key_env: GEMINI_API_KEY
  REAL_MINIMAX_CHAT:
    api_root: {json.dumps(args.minimax_base_url)}
    format: openai-completion
    provider_key_env: MINIMAX_API_KEY
model_aliases:
  real-openai-chat: {json.dumps(f"REAL_OPENAI_CHAT:{args.openai_model}")}
  real-openai-responses: {json.dumps(f"REAL_OPENAI_RESPONSES:{args.openai_model}")}
  real-anthropic-messages: {json.dumps(f"REAL_ANTHROPIC:{args.anthropic_model}")}
  real-gemini-generate-content: {json.dumps(f"REAL_GEMINI:{args.gemini_model}")}
  real-minimax-chat: {json.dumps(f"REAL_MINIMAX_CHAT:{args.minimax_model}")}
"""
    path.write_text(config.strip() + "\n", encoding="utf-8")


def _real_case_report_base(case: RealProviderMatrixCase) -> dict[str, object]:
    return {
        "case_id": case.case_id,
        "provider": case.provider,
        "surface": case.surface,
        "mode": case.mode,
        "feature": case.feature,
        "provider_key_env": case.provider_key_env,
        "required": case.required,
        "default_model": case.default_model,
        "model_alias": case.model_alias,
        "path": case.path,
    }


def _real_case_preflight_result(
    case: RealProviderMatrixCase,
    *,
    status: str,
    error: str,
) -> dict[str, object]:
    error = redact_real_provider_text(error)
    result = _real_case_report_base(case)
    result.update(
        {
            "status": status,
            "duration_ms": 0.0,
            "http_status": None,
            "content_type": None,
            "error": error,
            "failures": [error] if error else [],
        }
    )
    return result


def summarize_real_provider_results(
    results: list[dict[str, object]],
    *,
    missing_env: list[str] | None = None,
    reason: str | None = None,
    gate: str = REAL_PROVIDER_GATE,
    extra_fields: dict[str, object] | None = None,
) -> dict[str, object]:
    results = [
        redact_real_provider_report(result)
        for result in results
    ]
    passed = sum(1 for result in results if result["status"] == "passed")
    failed = sum(1 for result in results if result["status"] == "failed")
    skipped = sum(1 for result in results if result["status"] == "skipped")
    report: dict[str, object] = {
        "status": "passed" if failed == 0 and skipped == 0 else "failed",
        "gate": gate,
        "case_count": len(results),
        "passed": passed,
        "failed": failed,
        "skipped": skipped,
        "results": results,
    }
    if extra_fields:
        report.update(redact_real_provider_report(extra_fields))
    if missing_env is not None:
        report["missing_env"] = missing_env
    if reason:
        report["reason"] = redact_real_provider_text(reason)
    return redact_real_provider_report(report)


def build_real_provider_missing_secret_report(
    cases: list[RealProviderMatrixCase],
    missing_env: list[str],
) -> dict[str, object]:
    missing = set(missing_env)
    all_missing = all(case.provider_key_env in missing for case in cases if case.required)
    results: list[dict[str, object]] = []
    missing_text = ", ".join(missing_env)

    for case in cases:
        if case.required and case.provider_key_env in missing:
            results.append(
                _real_case_preflight_result(
                    case,
                    status="failed",
                    error=f"{case.provider_key_env} is required for {case.provider.upper()} provider",
                )
            )
        elif case.required and not all_missing:
            results.append(
                _real_case_preflight_result(
                    case,
                    status="skipped",
                    error=f"not run because required real provider secrets are missing: {missing_text}",
                )
            )
        else:
            results.append(
                _real_case_preflight_result(
                    case,
                    status="skipped",
                    error=f"not run because optional provider secret is missing: {case.provider_key_env}",
                )
            )

    return summarize_real_provider_results(
        results,
        missing_env=missing_env,
        reason=f"missing required real provider secrets: {missing_text}",
    )


def build_real_provider_startup_failure_report(
    cases: list[RealProviderMatrixCase],
    error: str,
) -> dict[str, object]:
    error = redact_real_provider_text(error)
    results = [
        _real_case_preflight_result(case, status="failed", error=error)
        for case in cases
    ]
    return summarize_real_provider_results(results, reason=error)


def _compatible_provider_report_fields(config: CompatibleProviderConfig) -> dict[str, object]:
    return {
        "claim_scope": COMPATIBLE_PROVIDER_CLAIM_SCOPE,
        "provider_label": config.provider_label,
        "configured_surfaces": compatible_provider_surfaces(config),
        "real_surfaces": [
            "openai_chat_completions",
            "anthropic_messages",
        ],
    }


def summarize_compatible_provider_results(
    results: list[dict[str, object]],
    config: CompatibleProviderConfig,
    *,
    missing_config: list[str] | None = None,
    reason: str | None = None,
) -> dict[str, object]:
    report = summarize_real_provider_results(
        results,
        reason=reason,
        gate=COMPATIBLE_PROVIDER_GATE,
        extra_fields=_compatible_provider_report_fields(config),
    )
    if missing_config is not None:
        report["missing_config"] = missing_config
    return redact_real_provider_report(report)


def build_compatible_provider_missing_config_report(
    cases: list[RealProviderMatrixCase],
    config: CompatibleProviderConfig,
) -> dict[str, object]:
    missing_config = list(config.missing_config)
    missing_text = ", ".join(missing_config)
    error = f"missing compatible provider configuration: {missing_text}"
    results = [
        _real_case_preflight_result(case, status="failed", error=error)
        for case in cases
    ]
    return summarize_compatible_provider_results(
        results,
        config,
        missing_config=missing_config,
        reason=error,
    )


def build_compatible_provider_startup_failure_report(
    cases: list[RealProviderMatrixCase],
    config: CompatibleProviderConfig,
    error: str,
) -> dict[str, object]:
    error = redact_real_provider_text(error)
    results = [
        _real_case_preflight_result(case, status="failed", error=error)
        for case in cases
    ]
    return summarize_compatible_provider_results(results, config, reason=error)


def run_real_provider_case(base_url: str, case: RealProviderMatrixCase) -> dict[str, object]:
    started = time.perf_counter()
    result = _real_case_report_base(case)
    try:
        status, headers, body = http_json(f"{base_url}{case.path}", case.payload, timeout=120)
    except Exception as error:
        duration_ms = round((time.perf_counter() - started) * 1000, 3)
        message = redact_real_provider_text(str(error))
        result.update(
            {
                "status": "failed",
                "http_status": None,
                "content_type": None,
                "duration_ms": duration_ms,
                "error": message,
                "failures": [message],
            }
        )
        return result

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

    result.update(
        {
            "status": "passed" if not failures else "failed",
            "http_status": status,
            "content_type": content_type,
            "duration_ms": duration_ms,
            "error": "; ".join(failures),
            "failures": failures,
        }
    )
    return redact_real_provider_report(result)


def _compatible_cases_from_config(config: CompatibleProviderConfig) -> list[RealProviderMatrixCase]:
    return build_compatible_provider_matrix_cases(
        openai_model=config.openai_model or "missing-COMPAT_OPENAI_MODEL",
        anthropic_model=config.anthropic_model or "missing-COMPAT_ANTHROPIC_MODEL",
        openai_provider_key_env=config.openai_provider_key_env or f"{COMPAT_PROVIDER_SHARED_CREDENTIAL_ENV} or {COMPAT_OPENAI_CREDENTIAL_ENV}",
        anthropic_provider_key_env=config.anthropic_provider_key_env or f"{COMPAT_PROVIDER_SHARED_CREDENTIAL_ENV} or {COMPAT_ANTHROPIC_CREDENTIAL_ENV}",
    )


def run_compatible_provider_smoke(args: argparse.Namespace) -> int:
    config = resolve_compatible_provider_config(args)
    cases = _compatible_cases_from_config(config)
    if config.missing_config:
        report = build_compatible_provider_missing_config_report(cases, config)
        emit_machine_report(report, args.json_out)
        print(
            redact_real_provider_text(
                "Missing compatible provider configuration: " + ", ".join(config.missing_config)
            ),
            file=sys.stderr,
        )
        return 2

    binary = Path(args.binary)
    if not binary.exists():
        message = f"proxy binary not found: {binary}"
        report = build_compatible_provider_startup_failure_report(cases, config, message)
        emit_machine_report(report, args.json_out)
        print(redact_real_provider_text(message), file=sys.stderr)
        return 2

    try:
        port = free_port()
    except Exception as error:
        message = f"could not allocate local proxy port: {error}"
        report = build_compatible_provider_startup_failure_report(cases, config, message)
        emit_machine_report(report, args.json_out)
        print(redact_real_provider_text(message), file=sys.stderr)
        return 2
    base_url = f"http://127.0.0.1:{port}"

    with tempfile.TemporaryDirectory(prefix="proxy-compatible-matrix-") as tempdir:
        config_path = Path(tempdir) / "proxy.yaml"
        stdout_path = Path(tempdir) / "proxy.stdout.log"
        stderr_path = Path(tempdir) / "proxy.stderr.log"
        write_compatible_provider_config(config_path, port, config)
        stdout_handle = stdout_path.open("w", encoding="utf-8")
        stderr_handle = stderr_path.open("w", encoding="utf-8")
        proc = subprocess.Popen(
            [str(binary), "--config", str(config_path)],
            stdout=stdout_handle,
            stderr=stderr_handle,
            env=proxy_key_env(),
            text=True,
        )
        try:
            try:
                wait_for_health(base_url)
                results = [run_real_provider_case(base_url, case) for case in cases]
                report = summarize_compatible_provider_results(results, config)
            except Exception as error:
                report = build_compatible_provider_startup_failure_report(cases, config, str(error))
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=5)
            stdout_handle.close()
            stderr_handle.close()

        if proc.returncode not in (0, -15) and report.get("status") == "passed":
            stderr = stderr_path.read_text(encoding="utf-8", errors="replace")
            message = stderr.strip() or f"proxy exited with {proc.returncode}"
            report = build_compatible_provider_startup_failure_report(cases, config, message)

    emit_machine_report(report, args.json_out)
    return 0 if report.get("status") == "passed" else 1


def run_real_provider_smoke(args: argparse.Namespace) -> int:
    cases = build_real_provider_matrix_cases(
        openai_model=args.openai_model,
        anthropic_model=args.anthropic_model,
        gemini_model=args.gemini_model,
        minimax_model=args.minimax_model,
    )
    missing_env = sorted(
        {
            case.provider_key_env
            for case in cases
            if case.required and not os.environ.get(case.provider_key_env)
        }
    )
    if missing_env:
        report = build_real_provider_missing_secret_report(cases, missing_env)
        emit_machine_report(report, args.json_out)
        print(
            redact_real_provider_text("Missing required real provider secrets: " + ", ".join(missing_env)),
            file=sys.stderr,
        )
        return 2

    binary = Path(args.binary)
    if not binary.exists():
        message = f"proxy binary not found: {binary}"
        report = build_real_provider_startup_failure_report(cases, message)
        emit_machine_report(report, args.json_out)
        print(redact_real_provider_text(message), file=sys.stderr)
        return 2

    try:
        port = free_port()
    except Exception as error:
        message = f"could not allocate local proxy port: {error}"
        report = build_real_provider_startup_failure_report(cases, message)
        emit_machine_report(report, args.json_out)
        print(redact_real_provider_text(message), file=sys.stderr)
        return 2
    base_url = f"http://127.0.0.1:{port}"

    with tempfile.TemporaryDirectory(prefix="proxy-real-matrix-") as tempdir:
        config_path = Path(tempdir) / "proxy.yaml"
        stdout_path = Path(tempdir) / "proxy.stdout.log"
        stderr_path = Path(tempdir) / "proxy.stderr.log"
        write_real_provider_config(config_path, port, args)
        stdout_handle = stdout_path.open("w", encoding="utf-8")
        stderr_handle = stderr_path.open("w", encoding="utf-8")
        proc = subprocess.Popen(
            [str(binary), "--config", str(config_path)],
            stdout=stdout_handle,
            stderr=stderr_handle,
            env=proxy_key_env(),
            text=True,
        )
        try:
            try:
                wait_for_health(base_url)
                results = [run_real_provider_case(base_url, case) for case in cases]
                report = summarize_real_provider_results(results)
            except Exception as error:
                report = build_real_provider_startup_failure_report(cases, str(error))
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=5)
            stdout_handle.close()
            stderr_handle.close()

        if proc.returncode not in (0, -15) and report.get("status") == "passed":
            stderr = stderr_path.read_text(encoding="utf-8", errors="replace")
            message = stderr.strip() or f"proxy exited with {proc.returncode}"
            report = build_real_provider_startup_failure_report(cases, message)

    emit_machine_report(report, args.json_out)
    return 0 if report.get("status") == "passed" else 1


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--mode",
        choices=("mock", "perf", "compatible-provider-smoke", "real-provider-smoke"),
        help="explicit gate mode; legacy --mock/--perf/--compatible-provider-smoke/--real-provider-smoke flags are still supported",
    )
    parser.add_argument("--mock", action="store_true", help="run deterministic local mock endpoint matrix")
    parser.add_argument("--perf", action="store_true", help="run local deterministic perf gate; requires --mock or --mode perf")
    parser.add_argument("--json-out", help="write machine-readable gate result JSON")
    parser.add_argument("--compatible-provider-smoke", action="store_true", help="run protected compatible-provider smoke matrix")
    parser.add_argument("--real-provider-smoke", action="store_true", help="run protected real-provider smoke matrix")
    parser.add_argument("--binary", default="./target/debug/llm-universal-proxy")
    parser.add_argument("--perf-iterations", type=int, default=PERF_DEFAULT_ITERATIONS)
    parser.add_argument("--perf-p95-ms", type=float, default=PERF_DEFAULT_P95_MS)
    parser.add_argument("--perf-total-ms", type=float, default=PERF_DEFAULT_TOTAL_MS)
    parser.add_argument(
        "--compat-openai-base-url",
        default=os.environ.get(COMPAT_OPENAI_BASE_URL_ENV),
    )
    parser.add_argument(
        "--compat-openai-model",
        default=os.environ.get(COMPAT_OPENAI_MODEL_ENV),
    )
    parser.add_argument(
        "--compat-anthropic-base-url",
        default=os.environ.get(COMPAT_ANTHROPIC_BASE_URL_ENV),
    )
    parser.add_argument(
        "--compat-anthropic-model",
        default=os.environ.get(COMPAT_ANTHROPIC_MODEL_ENV),
    )
    parser.add_argument(
        "--compat-provider-label",
        default=os.environ.get("COMPAT_PROVIDER_LABEL", COMPATIBLE_PROVIDER_DEFAULT_LABEL),
    )
    parser.add_argument(
        "--anthropic-base-url",
        default=os.environ.get("ANTHROPIC_UPSTREAM_BASE_URL", "https://api.anthropic.com/v1"),
    )
    parser.add_argument(
        "--openai-base-url",
        default=os.environ.get("OPENAI_UPSTREAM_BASE_URL", "https://api.openai.com/v1"),
    )
    parser.add_argument(
        "--gemini-base-url",
        default=os.environ.get("GEMINI_UPSTREAM_BASE_URL", "https://generativelanguage.googleapis.com/v1beta"),
    )
    parser.add_argument(
        "--minimax-base-url",
        default=os.environ.get("MINIMAX_UPSTREAM_BASE_URL", os.environ.get("MINIMAX_BASE_URL", "https://api.minimax.io/v1")),
    )
    parser.add_argument(
        "--anthropic-model",
        default=os.environ.get("ANTHROPIC_UPSTREAM_MODEL", os.environ.get("ANTHROPIC_MODEL", REAL_ANTHROPIC_DEFAULT_MODEL)),
    )
    parser.add_argument(
        "--openai-model",
        default=os.environ.get("OPENAI_UPSTREAM_MODEL", os.environ.get("OPENAI_MODEL", REAL_OPENAI_DEFAULT_MODEL)),
    )
    parser.add_argument(
        "--gemini-model",
        default=os.environ.get("GEMINI_UPSTREAM_MODEL", os.environ.get("GEMINI_MODEL", REAL_GEMINI_DEFAULT_MODEL)),
    )
    parser.add_argument(
        "--minimax-model",
        default=os.environ.get("MINIMAX_UPSTREAM_MODEL", os.environ.get("MINIMAX_MODEL", REAL_MINIMAX_DEFAULT_MODEL)),
    )
    args = parser.parse_args(argv)
    mode_sources = []
    if args.mode:
        mode_sources.append(args.mode)
    if args.compatible_provider_smoke:
        mode_sources.append("compatible-provider-smoke")
    if args.real_provider_smoke:
        mode_sources.append("real-provider-smoke")
    if args.mock:
        mode_sources.append("perf" if args.perf else "mock")
    elif args.perf:
        if args.mode != "perf":
            parser.error("--perf requires --mock or --mode perf")
        mode_sources.append("perf")

    if not mode_sources:
        parser.error(
            "explicit mode required; use --mode mock|perf|compatible-provider-smoke|real-provider-smoke "
            "or legacy --mock/--mock --perf/--compatible-provider-smoke/--real-provider-smoke"
        )
    resolved_modes = set(mode_sources)
    if len(resolved_modes) != 1:
        parser.error(f"conflicting modes requested: {', '.join(mode_sources)}")
    args.mode = mode_sources[0]
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.mode == "mock":
        report = run_mock_matrix(Path(args.binary))
        emit_machine_report(report, args.json_out)
        return 0 if report.get("status") == "passed" else 1

    if args.mode == "perf":
        report = run_perf_gate(
            Path(args.binary),
            iterations=args.perf_iterations,
            p95_threshold_ms=args.perf_p95_ms,
            total_threshold_ms=args.perf_total_ms,
        )
        emit_machine_report(report, args.json_out)
        return 0 if report.get("status") == "passed" else 1

    if args.mode == "compatible-provider-smoke":
        return run_compatible_provider_smoke(args)

    if args.mode == "real-provider-smoke":
        return run_real_provider_smoke(args)

    raise AssertionError(f"unhandled mode: {args.mode}")


if __name__ == "__main__":
    raise SystemExit(main())
