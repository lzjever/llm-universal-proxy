import importlib.util
import io
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import unittest
from unittest import mock


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "real_cli_matrix.py"


def load_module():
    spec = importlib.util.spec_from_file_location("real_cli_matrix", SCRIPT_PATH)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def make_lane(
    module,
    *,
    name="minimax-anth",
    required=True,
    enabled=True,
    proxy_model=None,
    upstream_name="MINIMAX-ANTHROPIC",
):
    return module.Lane(
        name=name,
        required=required,
        enabled=enabled,
        proxy_model=proxy_model or name,
        upstream_name=upstream_name,
        skip_reason=None,
    )


def make_fixture(
    module,
    *,
    fixture_id="smoke_pong",
    prompt="Reply with PONG",
    verifier=None,
    timeout_secs=5,
):
    return module.TaskFixture(
        fixture_id=fixture_id,
        kind="smoke",
        prompt=prompt,
        verifier=verifier or {"type": "contains", "value": "PONG"},
        timeout_secs=timeout_secs,
        workspace_template=None,
    )


def make_case(module, *, client_name, lane=None, fixture=None, case_id=None):
    lane = lane or make_lane(module)
    fixture = fixture or make_fixture(module)
    return module.MatrixCase(
        client_name=client_name,
        lane=lane,
        fixture=fixture,
        case_id=case_id or f"{client_name}__{lane.name}__{fixture.fixture_id}",
    )


class RealCliMatrixTests(unittest.TestCase):
    def test_parse_dotenv_exports_reads_export_lines(self):
        module = load_module()

        parsed = module.parse_dotenv_exports(
            textwrap.dedent(
                """
                # comment
                export PRESET_ENDPOINT_API_KEY="real-key"
                LOCAL_QWEN_BASE_URL=http://127.0.0.1:9997/v1
                LOCAL_QWEN_MODEL='qwen3.5-9b-awq'
                """
            )
        )

        self.assertEqual(parsed["PRESET_ENDPOINT_API_KEY"], "real-key")
        self.assertEqual(parsed["LOCAL_QWEN_BASE_URL"], "http://127.0.0.1:9997/v1")
        self.assertEqual(parsed["LOCAL_QWEN_MODEL"], "qwen3.5-9b-awq")

    def test_parse_proxy_source_extracts_upstreams_and_aliases(self):
        module = load_module()

        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstream_timeout_secs: 120
                upstreams:
                  MINIMAX-ANTHROPIC:
                    api_root: "https://api.minimaxi.com/anthropic/v1"
                    format: anthropic
                    credential_actual: "secret"
                    auth_policy: force_server
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-completion
                    credential_actual: "secret"
                    auth_policy: force_server
                model_aliases:
                  minimax-anth: "MINIMAX-ANTHROPIC:MiniMax-M2.7-highspeed"
                  minimax-openai: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
                  claude-opus-4-6: "LOCAL-QWEN:qwen3.5-9b-awq"
                debug_trace:
                  path: /tmp/trace.jsonl
                  max_text_chars: 16384
                """
            )
        )

        self.assertEqual(parsed.listen, "127.0.0.1:18888")
        self.assertEqual(parsed.upstream_timeout_secs, 120)
        self.assertEqual(parsed.upstreams["MINIMAX-ANTHROPIC"]["format"], "anthropic")
        self.assertEqual(
            parsed.model_aliases["minimax-openai"],
            "MINIMAX-OPENAI:MiniMax-M2.7-highspeed",
        )
        self.assertEqual(parsed.debug_trace["path"], "/tmp/trace.jsonl")

    def test_resolve_lanes_marks_qwen_optional_when_env_missing(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            (REPO_ROOT / "proxy-test-minimax-and-local.yaml").read_text(encoding="utf-8")
        )

        lanes = {lane.name: lane for lane in module.resolve_lanes(parsed, {})}

        self.assertTrue(lanes["minimax-anth"].required)
        self.assertTrue(lanes["minimax-openai"].required)
        self.assertFalse(lanes["qwen-local"].required)
        self.assertFalse(lanes["qwen-local"].enabled)
        self.assertIn("LOCAL_QWEN", lanes["qwen-local"].skip_reason)

    def test_resolve_lanes_enables_qwen_when_env_present(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            (REPO_ROOT / "proxy-test-minimax-and-local.yaml").read_text(encoding="utf-8")
        )

        lanes = {
            lane.name: lane
            for lane in module.resolve_lanes(
                parsed,
                {
                    "LOCAL_QWEN_BASE_URL": "http://127.0.0.1:9997/v1",
                    "LOCAL_QWEN_MODEL": "qwen3.5-9b-awq",
                    "LOCAL_QWEN_API_KEY": "not-needed",
                },
            )
        }

        self.assertTrue(lanes["qwen-local"].enabled)
        self.assertEqual(lanes["qwen-local"].proxy_model, "qwen-local")
        self.assertEqual(lanes["qwen-local"].upstream_name, "LOCAL-QWEN")

    def test_build_runtime_config_overrides_listen_and_injects_qwen(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            (REPO_ROOT / "proxy-test-minimax-and-local.yaml").read_text(encoding="utf-8")
        )

        rendered = module.build_runtime_config_text(
            parsed,
            {
                "LOCAL_QWEN_BASE_URL": "http://127.0.0.1:9997/v1",
                "LOCAL_QWEN_MODEL": "qwen3.5-9b-awq",
                "LOCAL_QWEN_API_KEY": "not-needed",
            },
            listen_host="127.0.0.1",
            listen_port=19999,
            trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
        )

        self.assertIn("listen: 127.0.0.1:19999", rendered)
        self.assertIn("LOCAL-QWEN:", rendered)
        self.assertIn('qwen-local: "LOCAL-QWEN:qwen3.5-9b-awq"', rendered)
        self.assertIn("path: /tmp/cli-matrix-trace.jsonl", rendered)

    def test_build_runtime_config_omits_local_qwen_aliases_when_env_missing(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            (REPO_ROOT / "proxy-test-minimax-and-local.yaml").read_text(encoding="utf-8")
        )

        rendered = module.build_runtime_config_text(
            parsed,
            {},
            listen_host="127.0.0.1",
            listen_port=19999,
            trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
        )

        self.assertNotIn("LOCAL-QWEN:", rendered)
        self.assertNotIn('qwen-local: "LOCAL-QWEN:', rendered)
        self.assertNotIn('claude-opus-4-6: "LOCAL-QWEN:', rendered)

    def test_build_runtime_config_preserves_unknown_top_level_settings(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                feature_flags:
                  responses_passthrough: true
                  allowed_clients:
                    - codex
                    - gemini
                upstream_timeout_secs: 120
                upstreams:
                  MINIMAX-ANTHROPIC:
                    api_root: "https://api.minimaxi.com/anthropic/v1"
                    format: anthropic
                    credential_actual: "secret"
                    auth_policy: force_server
                model_aliases:
                  minimax-anth: "MINIMAX-ANTHROPIC:MiniMax-M2.7-highspeed"
                debug_trace:
                  path: /tmp/trace.jsonl
                  max_text_chars: 16384
                """
            )
        )

        rendered = module.build_runtime_config_text(
            parsed,
            {},
            listen_host="127.0.0.1",
            listen_port=19999,
            trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
        )

        self.assertIn("listen: 127.0.0.1:19999", rendered)
        self.assertIn("feature_flags:", rendered)
        self.assertIn("responses_passthrough: true", rendered)
        self.assertIn("- codex", rendered)
        self.assertIn("- gemini", rendered)

    def test_build_client_env_isolates_user_state_for_all_clients(self):
        module = load_module()
        base_env = {
            "PATH": "/usr/bin",
            "HOME": "/home/user",
            "OPENAI_API_KEY": "real-openai",
            "ANTHROPIC_API_KEY": "real-anthropic",
            "GEMINI_API_KEY": "real-gemini",
        }

        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir)
            codex_env = module.build_client_env(
                "codex", base_env, "http://127.0.0.1:18888", root / "codex-home"
            )
            claude_env = module.build_client_env(
                "claude", base_env, "http://127.0.0.1:18888", root / "claude-home"
            )
            gemini_env = module.build_client_env(
                "gemini", base_env, "http://127.0.0.1:18888", root / "gemini-home"
            )

        self.assertEqual(codex_env["OPENAI_API_KEY"], "dummy")
        self.assertEqual(
            codex_env["OPENAI_BASE_URL"], "http://127.0.0.1:18888/openai/v1"
        )
        self.assertNotEqual(codex_env["HOME"], "/home/user")
        self.assertNotIn("real-openai", codex_env.values())

        self.assertEqual(claude_env["ANTHROPIC_API_KEY"], "dummy")
        self.assertEqual(
            claude_env["ANTHROPIC_BASE_URL"], "http://127.0.0.1:18888/anthropic"
        )
        self.assertIn("CLAUDE_CONFIG_DIR", claude_env)

        self.assertEqual(gemini_env["GEMINI_API_KEY"], "dummy")
        self.assertEqual(
            gemini_env["GOOGLE_GEMINI_BASE_URL"], "http://127.0.0.1:18888/google"
        )
        self.assertEqual(gemini_env["HTTP_PROXY"], "")
        self.assertEqual(gemini_env["HTTPS_PROXY"], "")

    def test_build_client_command_uses_known_good_gemini_sandbox_flag_form(self):
        module = load_module()
        lane = make_lane(module, name="minimax-openai", proxy_model="minimax-openai")
        fixture = make_fixture(module)

        command = module.build_client_command(
            "gemini",
            "http://127.0.0.1:18888",
            lane,
            fixture,
            pathlib.Path("/tmp/workspace"),
        )

        self.assertIn("--sandbox=false", command)
        self.assertNotIn("--sandbox", command)

    def test_prepare_proxy_env_keeps_dotenv_scoped_to_proxy_only(self):
        module = load_module()
        base_env = {
            "PATH": "/usr/bin",
            "HOME": "/home/user",
            "OPENAI_API_KEY": "real-openai",
        }
        dotenv_env = {
            "PRESET_ENDPOINT_API_KEY": "proxy-only-secret",
            "LOCAL_QWEN_BASE_URL": "http://127.0.0.1:9997/v1",
        }

        proxy_env = module.prepare_proxy_env(base_env, dotenv_env)

        with tempfile.TemporaryDirectory() as temp_dir:
            client_env = module.build_client_env(
                "codex",
                base_env,
                "http://127.0.0.1:18888",
                pathlib.Path(temp_dir) / "codex-home",
            )

        self.assertEqual(proxy_env["PRESET_ENDPOINT_API_KEY"], "proxy-only-secret")
        self.assertEqual(proxy_env["LOCAL_QWEN_BASE_URL"], "http://127.0.0.1:9997/v1")
        self.assertNotIn("PRESET_ENDPOINT_API_KEY", client_env)
        self.assertNotIn("LOCAL_QWEN_BASE_URL", client_env)

    def test_expand_matrix_respects_phase_and_skip_slow(self):
        module = load_module()
        lane = module.Lane(
            name="minimax-anth",
            required=True,
            enabled=True,
            proxy_model="minimax-anth",
            upstream_name="MINIMAX-ANTHROPIC",
            skip_reason=None,
        )
        fixtures = [
            module.TaskFixture(
                fixture_id="smoke_pong",
                kind="smoke",
                prompt="Reply with PONG",
                verifier={"type": "contains", "value": "PONG"},
                timeout_secs=90,
                workspace_template=None,
            ),
            module.TaskFixture(
                fixture_id="bugfix",
                kind="long_horizon",
                prompt="Fix calc.py",
                verifier={"type": "file_contains", "path": "calc.py", "needle": "a + b"},
                timeout_secs=180,
                workspace_template=pathlib.Path("bugfix"),
            ),
        ]

        cases = module.expand_matrix(
            clients=["codex", "claude", "gemini"],
            lanes=[lane],
            fixtures=fixtures,
            phase="basic",
            skip_slow=True,
        )

        self.assertEqual([case.client_name for case in cases], ["codex", "claude", "gemini"])
        self.assertTrue(all(case.fixture.kind == "smoke" for case in cases))

    def test_expand_matrix_excludes_qwen_local_from_long_horizon_cases(self):
        module = load_module()
        lanes = [
            module.Lane(
                name="minimax-anth",
                required=True,
                enabled=True,
                proxy_model="minimax-anth",
                upstream_name="MINIMAX-ANTHROPIC",
                skip_reason=None,
            ),
            module.Lane(
                name="qwen-local",
                required=False,
                enabled=True,
                proxy_model="qwen-local",
                upstream_name="LOCAL-QWEN",
                skip_reason=None,
            ),
        ]
        fixtures = [
            module.TaskFixture(
                fixture_id="smoke_pong",
                kind="smoke",
                prompt="Reply with PONG",
                verifier={"type": "contains", "value": "PONG"},
                timeout_secs=90,
                workspace_template=None,
            ),
            module.TaskFixture(
                fixture_id="python_bugfix",
                kind="long_horizon",
                prompt="Fix calc.py",
                verifier={"type": "file_contains", "path": "calc.py", "needle": "a + b"},
                timeout_secs=180,
                workspace_template=pathlib.Path("bugfix"),
            ),
        ]

        cases = module.expand_matrix(
            clients=["codex", "gemini"],
            lanes=lanes,
            fixtures=fixtures,
            phase="all",
            skip_slow=False,
        )

        self.assertIn("codex__qwen-local__smoke_pong", [case.case_id for case in cases])
        self.assertIn("gemini__qwen-local__smoke_pong", [case.case_id for case in cases])
        self.assertNotIn(
            "codex__qwen-local__python_bugfix", [case.case_id for case in cases]
        )
        self.assertNotIn(
            "gemini__qwen-local__python_bugfix", [case.case_id for case in cases]
        )
        self.assertIn(
            "codex__minimax-anth__python_bugfix", [case.case_id for case in cases]
        )
        self.assertIn(
            "gemini__minimax-anth__python_bugfix", [case.case_id for case in cases]
        )

    def test_filter_matrix_cases_supports_explicit_case_ids(self):
        module = load_module()
        lane = module.Lane(
            name="minimax-anth",
            required=True,
            enabled=True,
            proxy_model="minimax-anth",
            upstream_name="MINIMAX-ANTHROPIC",
            skip_reason=None,
        )
        fixture = module.TaskFixture(
            fixture_id="smoke_pong",
            kind="smoke",
            prompt="Reply with PONG",
            verifier={"type": "contains", "value": "PONG"},
            timeout_secs=90,
            workspace_template=None,
        )
        cases = [
            module.MatrixCase(
                client_name="codex",
                lane=lane,
                fixture=fixture,
                case_id="codex__minimax-anth__smoke_pong",
            ),
            module.MatrixCase(
                client_name="gemini",
                lane=lane,
                fixture=fixture,
                case_id="gemini__minimax-anth__smoke_pong",
            ),
        ]

        filtered = module.filter_matrix_cases(
            cases, selected_case_ids=["gemini__minimax-anth__smoke_pong"]
        )

        self.assertEqual(
            [case.case_id for case in filtered], ["gemini__minimax-anth__smoke_pong"]
        )
        with self.assertRaisesRegex(ValueError, "unknown matrix case"):
            module.filter_matrix_cases(cases, selected_case_ids=["missing-case"])

    def test_classify_lane_health_skips_optional_qwen_probe_failures(self):
        module = load_module()
        optional_lane = module.Lane(
            name="qwen-local",
            required=False,
            enabled=True,
            proxy_model="qwen-local",
            upstream_name="LOCAL-QWEN",
            skip_reason=None,
        )
        required_lane = module.Lane(
            name="minimax-anth",
            required=True,
            enabled=True,
            proxy_model="minimax-anth",
            upstream_name="MINIMAX-ANTHROPIC",
            skip_reason=None,
        )

        self.assertEqual(
            module.classify_lane_health(optional_lane, "connection refused")[0], "skipped"
        )
        self.assertEqual(
            module.classify_lane_health(required_lane, "connection refused")[0], "failed"
        )

    def test_probe_lane_accepts_valid_responses_shape_without_exact_probe_text(self):
        module = load_module()
        lane = make_lane(
            module,
            name="qwen-local",
            required=False,
            proxy_model="qwen-local",
            upstream_name="LOCAL-QWEN",
        )

        with mock.patch.object(
            module,
            "http_json",
            return_value=(
                200,
                json.dumps(
                    {
                        "id": "resp_qwen",
                        "object": "response",
                        "status": "completed",
                        "output": [
                            {
                                "type": "message",
                                "content": [
                                    {
                                        "type": "output_text",
                                        "text": "Sure, here is a semantic match.",
                                    }
                                ],
                            }
                        ],
                    }
                ),
            ),
        ):
            self.assertIsNone(module.probe_lane("http://127.0.0.1:18888", lane))

    def test_probe_lane_rejects_http_200_body_without_response_shape(self):
        module = load_module()
        lane = make_lane(
            module,
            name="qwen-local",
            required=False,
            proxy_model="qwen-local",
            upstream_name="LOCAL-QWEN",
        )

        with mock.patch.object(module, "http_json", return_value=(200, '{"ok":true}')):
            self.assertIn("valid response shape", module.probe_lane("http://127.0.0.1:18888", lane))

    def test_verify_fixture_output_rejects_comment_only_python_bugfix_edits(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="python_bugfix",
            verifier={
                "type": "python_source_and_output",
                "source": {
                    "path": "calc.py",
                    "function": "add",
                    "args": ["a", "b"],
                    "returns": {
                        "kind": "binary_op",
                        "operator": "+",
                        "left": "a",
                        "right": "b",
                    },
                },
                "entrypoint": {
                    "path": "main.py",
                    "expect_stdout_contains": ["2 + 3 = 5", "4 * 5 = 20"],
                },
            },
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            (workspace_dir / "calc.py").write_text(
                textwrap.dedent(
                    """
                    def add(a, b):
                        return a - b

                    # return a + b

                    def multiply(a, b):
                        return a * b
                    """
                ).strip()
                + "\n",
                encoding="utf-8",
            )
            (workspace_dir / "main.py").write_text(
                textwrap.dedent(
                    """
                    from calc import add, multiply

                    print(f"2 + 3 = {add(2, 3)}")
                    print(f"4 * 5 = {multiply(4, 5)}")
                    """
                ).strip()
                + "\n",
                encoding="utf-8",
            )

            ok, message = module.verify_fixture_output(fixture, "", workspace_dir)

        self.assertFalse(ok)
        self.assertIn("calc.py", message)
        self.assertIn("return a + b", message)

    def test_verify_fixture_output_checks_main_py_behavior_for_python_bugfix(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="python_bugfix",
            verifier={
                "type": "python_source_and_output",
                "source": {
                    "path": "calc.py",
                    "function": "add",
                    "args": ["a", "b"],
                    "returns": {
                        "kind": "binary_op",
                        "operator": "+",
                        "left": "a",
                        "right": "b",
                    },
                },
                "entrypoint": {
                    "path": "main.py",
                    "expect_stdout_contains": ["2 + 3 = 5", "4 * 5 = 20"],
                },
            },
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            (workspace_dir / "calc.py").write_text(
                textwrap.dedent(
                    """
                    def add(a, b):
                        return a + b

                    def multiply(a, b):
                        return a * b
                    """
                ).strip()
                + "\n",
                encoding="utf-8",
            )
            (workspace_dir / "main.py").write_text(
                'print("2 + 3 = 6")\nprint("4 * 5 = 20")\n',
                encoding="utf-8",
            )

            ok, message = module.verify_fixture_output(fixture, "", workspace_dir)

        self.assertFalse(ok)
        self.assertIn("main.py", message)
        self.assertIn("2 + 3 = 5", message)

    def test_verify_fixture_output_accepts_python_bugfix_fix_when_behavior_matches(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="python_bugfix",
            verifier={
                "type": "python_source_and_output",
                "source": {
                    "path": "calc.py",
                    "function": "add",
                    "args": ["a", "b"],
                    "returns": {
                        "kind": "binary_op",
                        "operator": "+",
                        "left": "a",
                        "right": "b",
                    },
                },
                "entrypoint": {
                    "path": "main.py",
                    "expect_stdout_contains": ["2 + 3 = 5", "4 * 5 = 20"],
                },
            },
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            (workspace_dir / "calc.py").write_text(
                textwrap.dedent(
                    """
                    def add(a, b):
                        return a + b

                    def multiply(a, b):
                        return a * b
                    """
                ).strip()
                + "\n",
                encoding="utf-8",
            )
            (workspace_dir / "main.py").write_text(
                textwrap.dedent(
                    """
                    from calc import add, multiply

                    print(f"2 + 3 = {add(2, 3)}")
                    print(f"4 * 5 = {multiply(4, 5)}")
                    """
                ).strip()
                + "\n",
                encoding="utf-8",
            )

            ok, message = module.verify_fixture_output(fixture, "", workspace_dir)

        self.assertTrue(ok, message)

    def test_run_matrix_case_feeds_claude_prompt_via_stdin(self):
        module = load_module()
        case = make_case(
            module,
            client_name="claude",
            lane=make_lane(module),
            fixture=make_fixture(module, prompt="Reply with exactly PONG"),
        )
        captured = {}

        def fake_run(command, **kwargs):
            captured["command"] = command
            captured["kwargs"] = kwargs
            return subprocess.CompletedProcess(command, 0, stdout="PONG\n", stderr="")

        with tempfile.TemporaryDirectory() as temp_dir:
            report_dir = pathlib.Path(temp_dir)
            with mock.patch.object(module.subprocess, "run", side_effect=fake_run):
                result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(result["status"], "passed")
        self.assertIn("--add-dir", captured["command"])
        self.assertNotIn(case.fixture.prompt, captured["command"])
        self.assertEqual(captured["kwargs"]["input"], case.fixture.prompt)
        self.assertNotIn("stdin", captured["kwargs"])

    def test_run_matrix_case_detaches_stdin_for_headless_gemini_runs(self):
        module = load_module()
        case = make_case(
            module,
            client_name="gemini",
            lane=make_lane(module, name="minimax-openai", proxy_model="minimax-openai"),
            fixture=make_fixture(module, timeout_secs=1),
        )
        child_code = "import sys; sys.stdin.read(); print('PONG')"
        original_stdin = os.dup(0)
        read_fd, write_fd = os.pipe()

        try:
            os.dup2(read_fd, 0)
            os.close(read_fd)
            with tempfile.TemporaryDirectory() as temp_dir:
                report_dir = pathlib.Path(temp_dir)
                with mock.patch.object(
                    module,
                    "build_client_command",
                    return_value=[sys.executable, "-c", child_code],
                ):
                    result = module.run_matrix_case(
                        case,
                        "http://127.0.0.1:18888",
                        report_dir,
                        {"PATH": os.environ.get("PATH", "")},
                    )
        finally:
            os.dup2(original_stdin, 0)
            os.close(original_stdin)
            os.close(write_fd)

        self.assertEqual(result["status"], "passed", result["message"])

    def test_run_matrix_case_reuses_runner_managed_gemini_home_across_cases(self):
        module = load_module()
        first_case = make_case(
            module,
            client_name="gemini",
            lane=make_lane(module, name="minimax-openai", proxy_model="minimax-openai"),
            case_id="gemini__minimax-openai__smoke_pong",
        )
        second_case = make_case(
            module,
            client_name="gemini",
            lane=make_lane(module, name="minimax-anth", proxy_model="minimax-anth"),
            case_id="gemini__minimax-anth__smoke_pong",
        )
        seen_homes = []

        def fake_run(command, **kwargs):
            seen_homes.append(kwargs["env"]["HOME"])
            return subprocess.CompletedProcess(command, 0, stdout="PONG\n", stderr="")

        with tempfile.TemporaryDirectory() as temp_dir:
            report_dir = pathlib.Path(temp_dir) / "reports" / "run-001"
            report_dir.mkdir(parents=True, exist_ok=True)
            with mock.patch.object(module.subprocess, "run", side_effect=fake_run):
                first_result = module.run_matrix_case(
                    first_case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )
                second_result = module.run_matrix_case(
                    second_case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(first_result["status"], "passed")
        self.assertEqual(second_result["status"], "passed")
        self.assertEqual(len(seen_homes), 2)
        self.assertEqual(seen_homes[0], seen_homes[1])
        self.assertIn("_runner_state", seen_homes[0])
        self.assertNotIn(first_case.case_id, seen_homes[0])
        self.assertNotIn(second_case.case_id, seen_homes[1])

    def test_run_matrix_case_normalizes_gemini_workspace_paths_when_report_dir_is_relative(self):
        module = load_module()
        case = make_case(
            module,
            client_name="gemini",
            lane=make_lane(module, name="minimax-openai", proxy_model="minimax-openai"),
        )
        observed = {}

        def fake_run(command, **kwargs):
            include_index = command.index("--include-directories")
            observed["include_dir"] = pathlib.Path(command[include_index + 1])
            observed["cwd"] = pathlib.Path(kwargs["cwd"]).resolve()
            observed["home"] = pathlib.Path(kwargs["env"]["HOME"]).resolve()
            return subprocess.CompletedProcess(command, 0, stdout="PONG\n", stderr="")

        with tempfile.TemporaryDirectory() as temp_dir:
            original_cwd = os.getcwd()
            os.chdir(temp_dir)
            try:
                report_dir = pathlib.Path("reports") / "run-001"
                report_dir.mkdir(parents=True, exist_ok=True)
                with mock.patch.object(module.subprocess, "run", side_effect=fake_run):
                    result = module.run_matrix_case(
                        case,
                        "http://127.0.0.1:18888",
                        report_dir,
                        {"PATH": os.environ.get("PATH", "")},
                    )
            finally:
                os.chdir(original_cwd)

        self.assertEqual(result["status"], "passed", result["message"])
        self.assertTrue(
            observed["include_dir"].is_absolute(),
            f"expected absolute --include-directories path, got {observed['include_dir']}",
        )
        self.assertEqual(observed["include_dir"], observed["cwd"])
        self.assertTrue(observed["home"].is_absolute())
        self.assertIn("_runner_state", str(observed["home"]))

    def test_run_matrix_case_extends_only_first_gemini_bootstrap_timeout(self):
        module = load_module()
        case = make_case(
            module,
            client_name="gemini",
            lane=make_lane(module, name="minimax-openai", proxy_model="minimax-openai"),
            fixture=make_fixture(module, timeout_secs=5),
        )
        observed_timeouts = []

        def fake_run(command, **kwargs):
            observed_timeouts.append(kwargs["timeout"])
            home_dir = pathlib.Path(kwargs["env"]["HOME"])
            rg_path = home_dir / ".gemini" / "tmp" / "bin" / "rg"
            rg_path.parent.mkdir(parents=True, exist_ok=True)
            rg_path.write_text("", encoding="utf-8")
            return subprocess.CompletedProcess(command, 0, stdout="PONG\n", stderr="")

        with tempfile.TemporaryDirectory() as temp_dir:
            report_dir = pathlib.Path(temp_dir) / "reports" / "run-001"
            report_dir.mkdir(parents=True, exist_ok=True)
            with mock.patch.object(module.subprocess, "run", side_effect=fake_run):
                first_result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )
                second_result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(first_result["status"], "passed")
        self.assertEqual(second_result["status"], "passed")
        self.assertEqual(
            observed_timeouts,
            [module.GEMINI_BOOTSTRAP_TIMEOUT_SECS, case.fixture.timeout_secs],
        )

    def test_run_matrix_case_uses_absolute_workspace_paths_for_gemini(self):
        module = load_module()
        case = make_case(
            module,
            client_name="gemini",
            lane=make_lane(module, name="minimax-openai", proxy_model="minimax-openai"),
        )
        observed = {}

        def fake_run(command, **kwargs):
            observed["command"] = command
            observed["cwd"] = kwargs["cwd"]
            observed["home"] = kwargs["env"]["HOME"]
            return subprocess.CompletedProcess(command, 0, stdout="PONG\n", stderr="")

        original_cwd = os.getcwd()
        with tempfile.TemporaryDirectory() as temp_dir:
            temp_root = pathlib.Path(temp_dir)
            os.chdir(temp_root)
            try:
                report_dir = pathlib.Path("reports") / "run-001"
                report_dir.mkdir(parents=True, exist_ok=True)
                with mock.patch.object(module.subprocess, "run", side_effect=fake_run):
                    result = module.run_matrix_case(
                        case,
                        "http://127.0.0.1:18888",
                        report_dir,
                        {"PATH": os.environ.get("PATH", "")},
                    )
            finally:
                os.chdir(original_cwd)

        self.assertEqual(result["status"], "passed")
        self.assertTrue(pathlib.Path(observed["cwd"]).is_absolute())
        self.assertTrue(pathlib.Path(observed["home"]).is_absolute())
        include_idx = observed["command"].index("--include-directories") + 1
        include_dir = pathlib.Path(observed["command"][include_idx])
        self.assertTrue(include_dir.is_absolute())
        self.assertEqual(include_dir, pathlib.Path(observed["cwd"]))

    def test_run_matrix_case_keeps_failures_for_enabled_optional_lane(self):
        module = load_module()
        case = make_case(
            module,
            client_name="gemini",
            lane=module.Lane(
                name="qwen-local",
                required=False,
                enabled=True,
                proxy_model="qwen-local",
                upstream_name="LOCAL-QWEN",
                skip_reason=None,
            ),
            fixture=make_fixture(module),
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            report_dir = pathlib.Path(temp_dir)
            with mock.patch.object(
                module.subprocess,
                "run",
                return_value=subprocess.CompletedProcess(
                    ["gemini", "--prompt", case.fixture.prompt],
                    7,
                    stdout="",
                    stderr="boom",
                ),
            ):
                result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(result["status"], "failed")
        self.assertEqual(result["message"], "exit code 7")

    def test_run_proxy_only_skips_client_binary_checks(self):
        module = load_module()

        class FakeProcess:
            def poll(self):
                return 0

            def wait(self, timeout=None):
                return 0

        observed = {}

        def fake_ensure_required_binaries(clients, proxy_binary):
            observed["clients"] = list(clients)
            observed["proxy_binary"] = proxy_binary

        with tempfile.TemporaryDirectory() as temp_dir:
            reports_root = pathlib.Path(temp_dir) / "reports"
            binary_path = pathlib.Path(temp_dir) / "fake-proxy"
            runtime_config_path = pathlib.Path(temp_dir) / "runtime-config.yaml"
            proxy_stdout_path = pathlib.Path(temp_dir) / "proxy.stdout.log"
            proxy_stderr_path = pathlib.Path(temp_dir) / "proxy.stderr.log"
            stdout = io.StringIO()
            with mock.patch.object(
                module, "ensure_required_binaries", side_effect=fake_ensure_required_binaries
            ), mock.patch.object(
                module,
                "selected_clients",
                side_effect=AssertionError(
                    "selected_clients should not run in proxy-only mode"
                ),
            ), mock.patch.object(
                module,
                "start_proxy",
                return_value=(
                    FakeProcess(),
                    runtime_config_path,
                    proxy_stdout_path,
                    proxy_stderr_path,
                ),
            ), mock.patch.object(module, "wait_for_health"), mock.patch.object(
                module, "stop_proxy"
            ), mock.patch(
                "sys.stdout", stdout
            ):
                exit_code = module.run(
                    [
                        "--proxy-only",
                        "--config-source",
                        str(REPO_ROOT / "proxy-test-minimax-and-local.yaml"),
                        "--env-file",
                        str(pathlib.Path(temp_dir) / "missing.env"),
                        "--fixtures-root",
                        str(REPO_ROOT / "scripts" / "fixtures" / "cli_matrix"),
                        "--reports-root",
                        str(reports_root),
                        "--binary",
                        str(binary_path),
                    ]
                )

        self.assertEqual(exit_code, 0)
        self.assertEqual(observed["clients"], [])
        self.assertEqual(observed["proxy_binary"], binary_path)
        self.assertIn("Proxy healthy at http://127.0.0.1:18888", stdout.getvalue())

    def test_resolve_cli_args_supports_list_matrix_and_case_filters(self):
        module = load_module()

        args = module.resolve_cli_args(
            [
                "--list-matrix",
                "--case",
                "codex__minimax-anth__smoke_pong",
                "--case",
                "gemini__minimax-openai__smoke_pong",
            ]
        )

        self.assertTrue(args.list_matrix)
        self.assertEqual(
            args.case,
            [
                "codex__minimax-anth__smoke_pong",
                "gemini__minimax-openai__smoke_pong",
            ],
        )

    def test_write_reports_creates_json_markdown_and_latest_symlink(self):
        module = load_module()

        with tempfile.TemporaryDirectory() as temp_dir:
            reports_root = pathlib.Path(temp_dir)
            run_dir = module.write_reports(
                reports_root,
                {
                    "started_at": "2026-04-17T00:00:00Z",
                    "finished_at": "2026-04-17T00:01:00Z",
                    "pass": 1,
                    "fail": 0,
                    "skip": 1,
                },
                [
                    {
                        "case_id": "codex__minimax-anth__smoke_pong",
                        "client": "codex",
                        "lane": "minimax-anth",
                        "fixture": "smoke_pong",
                        "status": "passed",
                        "message": "contained PONG",
                    },
                    {
                        "case_id": "gemini__qwen-local__smoke_pong",
                        "client": "gemini",
                        "lane": "qwen-local",
                        "fixture": "smoke_pong",
                        "status": "skipped",
                        "message": "optional lane unavailable",
                    },
                ],
                timestamp="20260417T000000Z",
            )

            self.assertTrue((run_dir / "report.json").exists())
            self.assertTrue((run_dir / "report.md").exists())
            self.assertTrue((run_dir / "results.jsonl").exists())
            latest = reports_root / "latest"
            self.assertTrue(latest.is_symlink())
            self.assertEqual(latest.resolve(), run_dir.resolve())

            summary = json.loads((run_dir / "report.json").read_text(encoding="utf-8"))
            self.assertEqual(summary["pass"], 1)
            self.assertIn(
                "codex__minimax-anth__smoke_pong",
                (run_dir / "results.jsonl").read_text(encoding="utf-8"),
            )


if __name__ == "__main__":
    unittest.main()
