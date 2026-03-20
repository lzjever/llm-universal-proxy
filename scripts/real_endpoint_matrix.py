#!/usr/bin/env python3
import argparse
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def http_json(url: str, payload: dict, timeout: int = 60) -> tuple[int, dict, str]:
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        body = resp.read().decode("utf-8")
        headers = {key.lower(): value for key, value in resp.headers.items()}
        return resp.status, headers, body


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


def expect_json_case(base_url: str, path: str, payload: dict, expected_markers, label: str) -> None:
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


def expect_sse_case(base_url: str, path: str, payload: dict, expected_markers: list[str], label: str) -> None:
    status, headers, body = http_json(f"{base_url}{path}", payload)
    assert status == 200, f"{label}: unexpected status {status}, body={body}"
    assert "text/event-stream" in headers.get("content-type", ""), (
        f"{label}: unexpected content-type {headers.get('content-type')}"
    )
    for marker in expected_markers:
        assert marker in body, f"{label}: missing marker {marker!r}, body={body}"
    print(f"[ok] {label}")


def build_config(path: Path, listen_port: int, anthropic_key: str, openai_key: str, anthropic_base: str, openai_base: str, anthropic_model: str, openai_model: str) -> None:
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


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="./target/debug/llm-universal-proxy")
    parser.add_argument("--anthropic-base-url", default=os.environ.get("ANTHROPIC_UPSTREAM_BASE_URL", "https://open.bigmodel.cn/api/anthropic/v1"))
    parser.add_argument("--openai-base-url", default=os.environ.get("OPENAI_UPSTREAM_BASE_URL", "https://open.bigmodel.cn/api/paas/v4"))
    parser.add_argument("--anthropic-model", default=os.environ.get("ANTHROPIC_UPSTREAM_MODEL", "GLM-5"))
    parser.add_argument("--openai-model", default=os.environ.get("OPENAI_UPSTREAM_MODEL", "glm-4.7-flash"))
    args = parser.parse_args()

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


if __name__ == "__main__":
    raise SystemExit(main())
