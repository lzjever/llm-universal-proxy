import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import types
import unittest
from unittest import mock


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
ENDPOINT_MATRIX_SCRIPT = REPO_ROOT / "scripts" / "real_endpoint_matrix.py"
REQUIRED_REAL_PROVIDER_ENVS = {
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "GEMINI_API_KEY",
    "MINIMAX_API_KEY",
}


def load_endpoint_matrix_module():
    spec = importlib.util.spec_from_file_location(
        "real_endpoint_matrix_contract",
        ENDPOINT_MATRIX_SCRIPT,
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class RealEndpointMatrixContractTests(unittest.TestCase):
    def test_anthropic_default_model_tracks_current_api_id(self):
        module = load_endpoint_matrix_module()

        self.assertEqual(module.REAL_ANTHROPIC_DEFAULT_MODEL, "claude-sonnet-4-6")

        cases = module.build_real_provider_matrix_cases()
        anthropic_defaults = {
            case.default_model for case in cases if case.provider == "anthropic"
        }
        self.assertEqual(anthropic_defaults, {"claude-sonnet-4-6"})

        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("ANTHROPIC_UPSTREAM_MODEL", None)
            os.environ.pop("ANTHROPIC_MODEL", None)
            args = module.parse_args(["--mode", "real-provider-smoke"])

        self.assertEqual(args.anthropic_model, "claude-sonnet-4-6")

    def test_real_provider_matrix_cases_cover_minimal_ga_tier(self):
        module = load_endpoint_matrix_module()
        cases = module.build_real_provider_matrix_cases()

        self.assertGreaterEqual(len(cases), 16)
        self.assertEqual(
            {case.provider for case in cases if case.required},
            {"openai", "anthropic", "gemini", "minimax"},
        )
        self.assertEqual(
            {case.env_var for case in cases if case.required},
            REQUIRED_REAL_PROVIDER_ENVS,
        )

        expected = {
            ("openai", "responses", "unary", "responses_unary"),
            ("openai", "responses", "stream", "responses_stream"),
            ("openai", "chat", "tool", "chat_tool"),
            ("openai", "responses", "fail_closed", "high_risk_state"),
            ("anthropic", "messages", "unary", "messages_unary"),
            ("anthropic", "messages", "stream", "messages_stream"),
            ("anthropic", "messages", "tool", "client_tool"),
            ("anthropic", "messages", "fail_closed", "high_risk_state"),
            ("gemini", "generateContent", "unary", "generate_content_unary"),
            ("gemini", "streamGenerateContent", "stream", "stream_generate_content"),
            ("gemini", "generateContent", "tool", "function_declarations"),
            ("gemini", "generateContent", "fail_closed", "high_risk_state"),
            ("minimax", "openai_chat", "unary", "chat_unary"),
            ("minimax", "openai_chat", "stream", "chat_stream"),
            ("minimax", "openai_chat", "tool", "chat_tool"),
            ("minimax", "openai_chat", "fail_closed", "unsupported_lifecycle_state"),
        }
        actual = {
            (case.provider, case.surface, case.mode, case.feature)
            for case in cases
        }
        for item in expected:
            with self.subTest(case=item):
                self.assertIn(item, actual)

        for case in cases:
            with self.subTest(case=case.case_id):
                self.assertTrue(case.case_id)
                self.assertTrue(case.path.startswith("/"))
                self.assertIsInstance(case.payload, dict)
                self.assertTrue(case.default_model)
                self.assertIsInstance(case.expected_markers, tuple)
                rendered_payload = json.dumps(case.payload)
                self.assertNotIn("sk-proj-", rendered_payload)
                self.assertNotIn("sk-ant-", rendered_payload)
                self.assertNotIn("AIza", rendered_payload)

        case_ids = [case.case_id for case in cases]
        self.assertEqual(len(case_ids), len(set(case_ids)))

    def test_perf_gate_uses_representative_mock_subset(self):
        module = load_endpoint_matrix_module()
        cases = module.build_perf_matrix_cases()

        self.assertGreater(len(cases), 1)
        self.assertNotEqual([case.case_id for case in cases], ["openai_chat_unary"])
        self.assertGreaterEqual(
            {case.surface for case in cases},
            {
                "openai_chat",
                "openai_responses",
                "anthropic_messages",
                "gemini_generate_content",
            },
        )

    def test_write_real_provider_config_uses_env_refs_without_inline_secret_values(self):
        module = load_endpoint_matrix_module()
        sentinel_by_env = {
            key: f"real-matrix-sentinel-{key.lower()}-value"
            for key in REQUIRED_REAL_PROVIDER_ENVS
        }
        args = types.SimpleNamespace(
            openai_base_url="https://openai.example/v1",
            anthropic_base_url="https://anthropic.example/v1",
            gemini_base_url="https://gemini.example/v1beta",
            minimax_base_url="https://minimax.example/v1",
            openai_model="gpt-contract",
            anthropic_model="claude-contract",
            gemini_model="gemini-contract",
            minimax_model="minimax-contract",
        )

        with tempfile.TemporaryDirectory() as temp_dir, mock.patch.dict(
            os.environ, sentinel_by_env, clear=False
        ):
            config_path = pathlib.Path(temp_dir) / "proxy.yaml"
            module.write_real_provider_config(config_path, 43210, args)
            config_text = config_path.read_text(encoding="utf-8")

        self.assertNotIn("credential_actual", config_text)
        for env_name in REQUIRED_REAL_PROVIDER_ENVS:
            self.assertIn(f"credential_env: {env_name}", config_text)
        for secret_value in sentinel_by_env.values():
            self.assertNotIn(secret_value, config_text)

    def test_cli_requires_explicit_mode_and_does_not_default_to_real(self):
        env = os.environ.copy()
        for key in REQUIRED_REAL_PROVIDER_ENVS | {"GLM_APIKEY"}:
            env.pop(key, None)

        completed = subprocess.run(
            [sys.executable, str(ENDPOINT_MATRIX_SCRIPT)],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )

        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("explicit mode", completed.stderr)
        self.assertNotIn("OPENAI_API_KEY", completed.stderr)
        self.assertNotIn("GLM_APIKEY", completed.stderr)

    def test_real_provider_smoke_missing_secret_report_redacts_present_env_secret_values(self):
        sentinel = "real-matrix-sentinel-openai-present-secret"
        env = os.environ.copy()
        for key in REQUIRED_REAL_PROVIDER_ENVS | {"GLM_APIKEY"}:
            env.pop(key, None)
        env["OPENAI_API_KEY"] = sentinel

        with tempfile.TemporaryDirectory() as temp_dir:
            json_out = pathlib.Path(temp_dir) / "real-missing.json"
            completed = subprocess.run(
                [
                    sys.executable,
                    str(ENDPOINT_MATRIX_SCRIPT),
                    "--real-provider-smoke",
                    "--json-out",
                    str(json_out),
                ],
                cwd=REPO_ROOT,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            report_text = json_out.read_text(encoding="utf-8")
            report = json.loads(report_text)

        self.assertNotEqual(completed.returncode, 0)
        self.assertEqual(
            set(report["missing_env"]),
            REQUIRED_REAL_PROVIDER_ENVS - {"OPENAI_API_KEY"},
        )
        self.assertNotIn(sentinel, report_text)
        self.assertNotIn(sentinel, completed.stdout)
        self.assertNotIn(sentinel, completed.stderr)

    def test_real_provider_smoke_missing_secrets_fails_closed_with_json(self):
        env = os.environ.copy()
        for key in REQUIRED_REAL_PROVIDER_ENVS | {"GLM_APIKEY"}:
            env.pop(key, None)

        with tempfile.TemporaryDirectory() as temp_dir:
            json_out = pathlib.Path(temp_dir) / "real-missing.json"
            completed = subprocess.run(
                [
                    sys.executable,
                    str(ENDPOINT_MATRIX_SCRIPT),
                    "--real-provider-smoke",
                    "--json-out",
                    str(json_out),
                ],
                cwd=REPO_ROOT,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )

            self.assertNotEqual(completed.returncode, 0)
            self.assertTrue(json_out.exists())
            report = json.loads(json_out.read_text(encoding="utf-8"))

        self.assertEqual(report["status"], "failed")
        self.assertEqual(report["gate"], "real-provider-smoke")
        self.assertEqual(set(report["missing_env"]), REQUIRED_REAL_PROVIDER_ENVS)
        self.assertEqual(report["passed"], 0)
        self.assertEqual(report["skipped"], 0)
        self.assertGreaterEqual(report["failed"], 16)

        required_result_fields = {
            "case_id",
            "provider",
            "surface",
            "mode",
            "feature",
            "status",
            "duration_ms",
            "error",
        }
        for result in report["results"]:
            with self.subTest(case=result["case_id"]):
                self.assertGreaterEqual(set(result), required_result_fields)
                self.assertEqual(result["status"], "failed")
                self.assertIn(result["provider"].upper(), result["error"].upper())

    def test_real_provider_startup_failure_redacts_env_secret_values_from_json_and_stderr(self):
        sentinel_by_env = {
            key: f"real-matrix-sentinel-{key.lower()}-startup-secret"
            for key in REQUIRED_REAL_PROVIDER_ENVS
        }
        env = os.environ.copy()
        env.update(sentinel_by_env)

        with tempfile.TemporaryDirectory() as temp_dir:
            temp_root = pathlib.Path(temp_dir)
            fake_binary = temp_root / "fake_proxy.py"
            fake_binary.write_text(
                textwrap.dedent(
                    r"""
                    #!/usr/bin/env python3
                    import json
                    import os
                    import re
                    import sys
                    import threading
                    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

                    EXPECTED_POSTS = 16
                    post_count = 0
                    post_lock = threading.Lock()

                    config_path = sys.argv[sys.argv.index("--config") + 1]
                    with open(config_path, encoding="utf-8") as config_file:
                        config_text = config_file.read()
                    listen_match = re.search(r"listen:\s*127\.0\.0\.1:(\d+)", config_text)
                    if not listen_match:
                        raise SystemExit("missing listen port")
                    listen_port = int(listen_match.group(1))

                    class Handler(BaseHTTPRequestHandler):
                        protocol_version = "HTTP/1.1"

                        def log_message(self, fmt, *args):
                            return

                        def _send(self, status, content_type, body):
                            data = body.encode("utf-8")
                            self.send_response(status)
                            self.send_header("Content-Type", content_type)
                            self.send_header("Content-Length", str(len(data)))
                            self.end_headers()
                            self.wfile.write(data)
                            self.wfile.flush()
                            self.close_connection = True

                        def _send_json(self, status, payload):
                            self._send(status, "application/json", json.dumps(payload))

                        def _send_sse(self, body):
                            self._send(200, "text/event-stream", body)

                        def do_GET(self):
                            if self.path == "/health":
                                self._send(200, "text/plain", "ok")
                            else:
                                self._send_json(404, {"error": "not found"})

                        def do_POST(self):
                            global post_count
                            length = int(self.headers.get("Content-Length", "0"))
                            body_text = self.rfile.read(length).decode("utf-8") if length else "{}"
                            body = json.loads(body_text or "{}")
                            path = self.path.split("?", 1)[0]

                            if "previous_response_id" in body_text:
                                self._send_json(400, {"error": {"message": "previous_response_id rejected"}})
                            elif path.endswith(":streamGenerateContent"):
                                self._send_sse('data: {"text":"OK"}\n\n')
                            elif path.endswith(":generateContent"):
                                if "functionDeclarations" in body_text:
                                    self._send_json(200, {"candidates": [{"content": {"parts": [{"functionCall": {"name": "get_weather"}}]}}]})
                                else:
                                    self._send_json(200, {"candidates": [{"content": {"parts": [{"text": "OK"}]}}]})
                            elif path == "/anthropic/v1/messages":
                                if body.get("stream"):
                                    self._send_sse("event: message_start\ndata: {}\n\nevent: message_stop\ndata: {}\n\n")
                                elif "tools" in body:
                                    self._send_json(200, {"content": [{"type": "tool_use", "name": "get_weather"}]})
                                else:
                                    self._send_json(200, {"content": [{"type": "text", "text": "OK"}]})
                            elif path == "/openai/v1/responses":
                                if body.get("stream"):
                                    self._send_sse("event: response.completed\ndata: OK\n\n")
                                elif "tools" in body:
                                    self._send_json(200, {"output": [{"type": "function_call", "name": "get_weather"}]})
                                else:
                                    self._send_json(200, {"output_text": "OK"})
                            elif path == "/openai/v1/chat/completions":
                                if body.get("stream"):
                                    self._send_sse("data: OK\n\ndata: [DONE]\n\n")
                                elif "tools" in body:
                                    self._send_json(200, {"choices": [{"message": {"tool_calls": [{"function": {"name": "get_weather"}}]}}]})
                                else:
                                    self._send_json(200, {"choices": [{"message": {"content": "OK"}}]})
                            else:
                                self._send_json(404, {"error": "not found", "path": path})

                            sys.stderr.write("proxy startup stderr copied env secret: ")
                            sys.stderr.write(os.environ["OPENAI_API_KEY"])
                            sys.stderr.write("\n")
                            sys.stderr.write(config_text)
                            sys.stderr.flush()

                            with post_lock:
                                post_count += 1
                                if post_count >= EXPECTED_POSTS:
                                    os._exit(42)

                    server = ThreadingHTTPServer(("127.0.0.1", listen_port), Handler)
                    server.serve_forever()
                    """
                ).lstrip(),
                encoding="utf-8",
            )
            fake_binary.chmod(0o755)
            json_out = temp_root / "startup-failure.json"

            completed = subprocess.run(
                [
                    sys.executable,
                    str(ENDPOINT_MATRIX_SCRIPT),
                    "--real-provider-smoke",
                    "--binary",
                    str(fake_binary),
                    "--json-out",
                    str(json_out),
                ],
                cwd=REPO_ROOT,
                env=env,
                text=True,
                capture_output=True,
                timeout=30,
                check=False,
            )
            report_text = json_out.read_text(encoding="utf-8")

        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("real-provider-smoke", report_text)
        combined = completed.stdout + completed.stderr + report_text
        for secret_value in sentinel_by_env.values():
            self.assertNotIn(secret_value, combined)


if __name__ == "__main__":
    unittest.main()
