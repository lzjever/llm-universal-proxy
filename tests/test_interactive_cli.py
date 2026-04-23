import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "interactive_cli.py"
WRAPPER_PATHS = (
    REPO_ROOT / "scripts" / "run_codex_proxy.sh",
    REPO_ROOT / "scripts" / "run_claude_proxy.sh",
    REPO_ROOT / "scripts" / "run_gemini_proxy.sh",
)


def load_module():
    sys.modules.pop("interactive_cli", None)
    spec = importlib.util.spec_from_file_location("interactive_cli", SCRIPT_PATH)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class InteractiveCliTests(unittest.TestCase):
    def test_parse_args_uses_resolved_default_proxy_binary(self):
        module = load_module()

        with mock.patch.object(
            module,
            "default_proxy_binary_path",
            return_value=pathlib.Path("/tmp/fresh-proxy"),
        ):
            args = module.parse_args(["--client", "codex"])

        self.assertEqual(args.binary, "/tmp/fresh-proxy")

    def test_wrapper_scripts_resolve_interactive_cli_relative_to_script_dir(self):
        for wrapper_path in WRAPPER_PATHS:
            with self.subTest(wrapper=wrapper_path.name):
                with tempfile.TemporaryDirectory() as temp_dir:
                    completed = subprocess.run(
                        ["bash", str(wrapper_path), "--help"],
                        cwd=temp_dir,
                        capture_output=True,
                        text=True,
                        check=False,
                    )

                self.assertEqual(
                    completed.returncode,
                    0,
                    msg=completed.stderr or completed.stdout,
                )
                self.assertIn("interactive_cli.py", completed.stdout)

    def test_build_interactive_command_shapes(self):
        module = load_module()
        workspace = pathlib.Path("/tmp/workspace").resolve()
        proxy_base = "http://127.0.0.1:18888"

        self.assertEqual(
            module.build_interactive_command(
                "codex",
                workspace,
                "minimax-openai",
                proxy_base,
                client_home=pathlib.Path("/tmp/codex-home"),
                model_limits=None,
            ),
            [
                "codex",
                "-C",
                str(workspace),
                "-m",
                "minimax-openai",
                "--dangerously-bypass-approvals-and-sandbox",
                "-c",
                'model_provider="proxy"',
                "-c",
                'model_providers.proxy.name="Proxy"',
                "-c",
                f'model_providers.proxy.base_url="{proxy_base}/openai/v1"',
                "-c",
                'model_providers.proxy.wire_api="responses"',
            ],
        )
        self.assertEqual(
            module.build_interactive_command(
                "claude",
                workspace,
                "claude-haiku-4-5",
                proxy_base,
                client_home=None,
                model_limits=None,
            ),
            [
                "claude",
                "--bare",
                "--setting-sources",
                "user",
                "--model",
                "claude-haiku-4-5",
                "--dangerously-skip-permissions",
                "--add-dir",
                str(workspace),
            ],
        )
        self.assertEqual(
            module.build_interactive_command(
                "gemini",
                workspace,
                "minimax-openai",
                proxy_base,
                client_home=None,
                model_limits=None,
            ),
            [
                "gemini",
                "--model",
                "minimax-openai",
                "--sandbox=false",
                "--yolo",
                "--include-directories",
                str(workspace),
            ],
        )

    def test_build_interactive_command_injects_codex_catalog_when_limits_exist(self):
        module = load_module()
        workspace = pathlib.Path("/tmp/workspace").resolve()
        with tempfile.TemporaryDirectory() as temp_dir:
            client_home = pathlib.Path(temp_dir).resolve()

            command = module.build_interactive_command(
                "codex",
                workspace,
                "minimax-openai",
                "http://127.0.0.1:18888",
                client_home=client_home,
                model_limits=module.ModelLimits(
                    context_window=200000,
                    max_output_tokens=128000,
                ),
                codex_metadata=module.CodexModelMetadata(
                    input_modalities=("text",),
                    supports_search_tool=False,
                ),
            )

            catalog = json.loads(
                (client_home / ".codex" / "catalog.json").read_text(encoding="utf-8")
            )

        joined = " ".join(command)
        self.assertIn("model_catalog_json", joined)
        self.assertIn(str(client_home / ".codex" / "catalog.json"), joined)
        self.assertIn('web_search="disabled"', joined)
        self.assertIn('tools.view_image=false', joined)
        self.assertEqual(
            catalog["models"][0]["apply_patch_tool_type"],
            "freeform",
        )

    def test_build_interactive_command_skips_view_image_disable_for_image_capable_models(self):
        module = load_module()
        workspace = pathlib.Path("/tmp/workspace").resolve()
        with tempfile.TemporaryDirectory() as temp_dir:
            client_home = pathlib.Path(temp_dir).resolve()

            command = module.build_interactive_command(
                "codex",
                workspace,
                "vision-openai",
                "http://127.0.0.1:18888",
                client_home=client_home,
                model_limits=module.ModelLimits(context_window=200000),
                codex_metadata=module.CodexModelMetadata(
                    input_modalities=("text", "image"),
                    supports_search_tool=True,
                ),
            )

        joined = " ".join(command)
        self.assertIn("model_catalog_json", joined)
        self.assertNotIn('tools.view_image=false', joined)

    def test_build_interactive_command_rejects_internal_tool_artifacts_in_public_args(self):
        module = load_module()
        workspace = pathlib.Path("/tmp/workspace").resolve()

        with tempfile.TemporaryDirectory() as temp_dir:
            client_home = pathlib.Path(temp_dir).resolve()
            with mock.patch.object(
                module,
                "build_codex_catalog_args",
                return_value=[
                    "-c",
                    'tool_identity_contract="__llmup_custom__apply_patch"',
                ],
            ):
                with self.assertRaisesRegex(ValueError, "__llmup_custom__apply_patch"):
                    module.build_interactive_command(
                        "codex",
                        workspace,
                        "minimax-openai",
                        "http://127.0.0.1:18888",
                        client_home=client_home,
                        model_limits=None,
                    )

    def test_run_with_proxy_base_does_not_call_start_proxy(self):
        module = load_module()
        live_profile = mock.Mock(
            limits=module.ModelLimits(
                context_window=200000,
                max_output_tokens=128000,
            ),
            codex_metadata=module.CodexModelMetadata(
                input_modalities=("text", "image"),
                supports_search_tool=True,
                supports_view_image=True,
                apply_patch_tool_type="freeform",
                supports_parallel_tool_calls=True,
            ),
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            with mock.patch.object(
                module, "ensure_client_binary"
            ), mock.patch.object(
                module, "start_proxy"
            ) as start_proxy, mock.patch.object(
                module, "wait_for_health"
            ) as wait_for_health, mock.patch.object(
                module, "stop_proxy"
            ) as stop_proxy, mock.patch.object(
                module,
                "fetch_live_model_profile",
                return_value=live_profile,
            ) as fetch_live_model_profile, mock.patch.object(
                module, "launch_interactive_client", return_value=0
            ) as launch_client:
                exit_code = module.run(
                    [
                        "--client",
                        "codex",
                        "--workspace",
                        temp_dir,
                        "--proxy-base",
                        "http://127.0.0.1:18888/",
                        "--proxy-health-timeout-secs",
                        "55",
                    ]
                )

        self.assertEqual(exit_code, 0)
        start_proxy.assert_not_called()
        wait_for_health.assert_called_once_with("http://127.0.0.1:18888", timeout_secs=55)
        fetch_live_model_profile.assert_called_once_with(
            "http://127.0.0.1:18888",
            "minimax-openai",
        )
        stop_proxy.assert_called_once_with(None, terminate_grace_secs=15)
        launch_args = launch_client.call_args.args
        self.assertEqual(
            launch_args[0],
            module.build_interactive_command(
                "codex",
                pathlib.Path(temp_dir).resolve(),
                "minimax-openai",
                "http://127.0.0.1:18888",
                client_home=pathlib.Path(launch_args[2]["HOME"]),
                model_limits=live_profile.limits,
                codex_metadata=live_profile.codex_metadata,
            ),
        )

    def test_run_with_proxy_base_uses_live_profile_without_reading_local_config(self):
        module = load_module()
        live_profile = mock.Mock(
            limits=module.ModelLimits(
                context_window=200000,
                max_output_tokens=128000,
            ),
            codex_metadata=module.CodexModelMetadata(
                input_modalities=("text", "image"),
                supports_search_tool=True,
                supports_view_image=False,
                apply_patch_tool_type="freeform",
                supports_parallel_tool_calls=True,
            ),
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            missing_config = pathlib.Path(temp_dir) / "missing-proxy.yaml"
            with mock.patch.object(
                module, "ensure_client_binary"
            ), mock.patch.object(
                module, "wait_for_health"
            ), mock.patch.object(
                module, "stop_proxy"
            ), mock.patch.object(
                module,
                "fetch_live_model_profile",
                return_value=live_profile,
            ) as fetch_live_model_profile, mock.patch.object(
                module, "launch_interactive_client", return_value=0
            ) as launch_client:
                exit_code = module.run(
                    [
                        "--client",
                        "codex",
                        "--workspace",
                        temp_dir,
                        "--proxy-base",
                        "http://127.0.0.1:18888/",
                        "--config-source",
                        str(missing_config),
                    ]
                )

        self.assertEqual(exit_code, 0)
        fetch_live_model_profile.assert_called_once_with(
            "http://127.0.0.1:18888",
            "minimax-openai",
        )
        launch_args = launch_client.call_args.args
        self.assertEqual(
            launch_args[0],
            module.build_interactive_command(
                "codex",
                pathlib.Path(temp_dir).resolve(),
                "minimax-openai",
                "http://127.0.0.1:18888",
                client_home=pathlib.Path(launch_args[2]["HOME"]),
                model_limits=live_profile.limits,
                codex_metadata=live_profile.codex_metadata,
            ),
        )

    def test_run_without_proxy_base_starts_waits_and_stops_proxy(self):
        module = load_module()

        class FakeProcess:
            def poll(self):
                return None

        fake_process = FakeProcess()

        with tempfile.TemporaryDirectory() as temp_dir:
            with mock.patch.object(
                module, "ensure_client_binary"
            ), mock.patch.object(
                module, "ensure_proxy_binary"
            ), mock.patch.object(
                module, "start_proxy",
                return_value=(
                    fake_process,
                    pathlib.Path(temp_dir) / "runtime-config.yaml",
                    pathlib.Path(temp_dir) / "proxy.stdout.log",
                    pathlib.Path(temp_dir) / "proxy.stderr.log",
                ),
            ) as start_proxy, mock.patch.object(
                module, "wait_for_health"
            ) as wait_for_health, mock.patch.object(
                module, "stop_proxy"
            ) as stop_proxy, mock.patch.object(
                module,
                "fetch_live_model_profile",
                return_value=mock.Mock(
                    limits=None,
                    codex_metadata=None,
                ),
            ) as fetch_live_model_profile, mock.patch.object(
                module, "launch_interactive_client", return_value=0
            ):
                exit_code = module.run(
                    [
                        "--client",
                        "claude",
                        "--workspace",
                        temp_dir,
                        "--proxy-health-timeout-secs",
                        "65",
                    ]
                )

        self.assertEqual(exit_code, 0)
        start_proxy.assert_called_once()
        wait_for_health.assert_called_once_with("http://127.0.0.1:18888", timeout_secs=65)
        fetch_live_model_profile.assert_called_once_with(
            "http://127.0.0.1:18888",
            "claude-haiku-4-5",
        )
        stop_proxy.assert_called_once_with(fake_process, terminate_grace_secs=15)

    def test_start_managed_proxy_serializes_surface_fields_into_runtime_config(self):
        module = load_module()

        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir)
            config_source = root / "proxy.yaml"
            config_source.write_text(
                """
listen: 127.0.0.1:18888
upstreams:
  MINIMAX-OPENAI:
    api_root: "https://api.minimaxi.com/v1"
    format: openai-completion
    credential_actual: "secret"
    auth_policy: force_server
    surface_defaults:
      modalities:
        input: ["text"]
        output: ["text"]
      tools:
        supports_search: false
        supports_view_image: false
        apply_patch_transport: function
        supports_parallel_calls: true
model_aliases:
  vision-openai:
    target: "MINIMAX-OPENAI:MiniMax-Vision"
    surface:
      modalities:
        input: ["text", "image"]
        output: ["text"]
      tools:
        supports_search: true
        supports_view_image: true
        apply_patch_transport: freeform
        supports_parallel_calls: false
""".lstrip(),
                encoding="utf-8",
            )
            env_file = root / ".env.test"
            env_file.write_text("", encoding="utf-8")
            args = module.parse_args(
                [
                    "--client",
                    "codex",
                    "--config-source",
                    str(config_source),
                    "--env-file",
                    str(env_file),
                    "--binary",
                    str(root / "llm-universal-proxy"),
                ]
            )

            fake_process = object()
            with mock.patch.object(
                module, "ensure_proxy_binary"
            ), mock.patch.object(
                module, "prepare_proxy_env", return_value={"PATH": os.environ.get("PATH", "")}
            ), mock.patch.object(
                module,
                "start_proxy",
                return_value=(
                    fake_process,
                    root / "runtime-config.yaml",
                    root / "proxy.stdout.log",
                    root / "proxy.stderr.log",
                ),
            ) as start_proxy:
                proxy_base, process = module.start_managed_proxy(
                    args,
                    {"PATH": os.environ.get("PATH", "")},
                    root / "runtime-root",
                )

        self.assertEqual(proxy_base, "http://127.0.0.1:18888")
        self.assertIs(process, fake_process)
        runtime_config_text = start_proxy.call_args.args[1]
        self.assertIn("surface_defaults:", runtime_config_text)
        self.assertIn('input: ["text"]', runtime_config_text)
        self.assertIn('output: ["text"]', runtime_config_text)
        self.assertIn("supports_search: false", runtime_config_text)
        self.assertIn("supports_view_image: false", runtime_config_text)
        self.assertIn("apply_patch_transport: function", runtime_config_text)
        self.assertIn("supports_parallel_calls: true", runtime_config_text)
        self.assertIn("surface:", runtime_config_text)
        self.assertIn('input: ["text", "image"]', runtime_config_text)
        self.assertIn('output: ["text"]', runtime_config_text)
        self.assertIn("supports_search: true", runtime_config_text)
        self.assertIn("supports_view_image: true", runtime_config_text)
        self.assertIn("apply_patch_transport: freeform", runtime_config_text)
        self.assertIn("supports_parallel_calls: false", runtime_config_text)

    def test_start_managed_proxy_round_trips_namespace_and_upstream_proxy_objects(self):
        module = load_module()

        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir)
            config_source = root / "proxy.yaml"
            config_source.write_text(
                """
listen: 127.0.0.1:18888
proxy:
  url: http://corp-proxy.example:8080
upstreams:
  MINIMAX-OPENAI:
    api_root: "https://api.minimaxi.com/v1"
    format: openai-completion
    credential_actual: "secret"
    auth_policy: force_server
    proxy:
      url: http://upstream-proxy.example:8080
model_aliases:
  minimax-openai: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
""".lstrip(),
                encoding="utf-8",
            )
            env_file = root / ".env.test"
            env_file.write_text("", encoding="utf-8")
            args = module.parse_args(
                [
                    "--client",
                    "codex",
                    "--config-source",
                    str(config_source),
                    "--env-file",
                    str(env_file),
                    "--binary",
                    str(root / "llm-universal-proxy"),
                ]
            )

            fake_process = object()
            with mock.patch.object(
                module, "ensure_proxy_binary"
            ), mock.patch.object(
                module, "prepare_proxy_env", return_value={"PATH": os.environ.get("PATH", "")}
            ), mock.patch.object(
                module,
                "start_proxy",
                return_value=(
                    fake_process,
                    root / "runtime-config.yaml",
                    root / "proxy.stdout.log",
                    root / "proxy.stderr.log",
                ),
            ) as start_proxy:
                module.start_managed_proxy(
                    args,
                    {"PATH": os.environ.get("PATH", "")},
                    root / "runtime-root",
                )

        runtime_config_text = start_proxy.call_args.args[1]
        reparsed = module.parse_proxy_source(runtime_config_text)

        self.assertEqual(reparsed.proxy["url"], "http://corp-proxy.example:8080")
        self.assertEqual(
            reparsed.upstreams["MINIMAX-OPENAI"]["proxy"]["url"],
            "http://upstream-proxy.example:8080",
        )

    def test_start_managed_proxy_injects_qwen_surface_defaults_when_env_enables_local_lane(self):
        module = load_module()

        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir)
            config_source = root / "proxy.yaml"
            config_source.write_text(
                """
listen: 127.0.0.1:18888
upstreams:
  MINIMAX-OPENAI:
    api_root: "https://api.minimaxi.com/v1"
    format: openai-completion
    credential_actual: "secret"
    auth_policy: force_server
    surface_defaults:
      modalities:
        input: ["text"]
        output: ["text"]
      tools:
        supports_search: false
        supports_view_image: false
        apply_patch_transport: freeform
model_aliases:
  minimax-openai: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
""".lstrip(),
                encoding="utf-8",
            )
            env_file = root / ".env.test"
            env_file.write_text(
                "\n".join(
                    [
                        "LOCAL_QWEN_BASE_URL=http://127.0.0.1:9997/v1",
                        "LOCAL_QWEN_MODEL=qwen3.5-9b-awq",
                        "LOCAL_QWEN_API_KEY=not-needed",
                        "",
                    ]
                ),
                encoding="utf-8",
            )
            args = module.parse_args(
                [
                    "--client",
                    "codex",
                    "--config-source",
                    str(config_source),
                    "--env-file",
                    str(env_file),
                    "--binary",
                    str(root / "llm-universal-proxy"),
                ]
            )

            fake_process = object()
            with mock.patch.object(
                module, "ensure_proxy_binary"
            ), mock.patch.object(
                module, "prepare_proxy_env", return_value={"PATH": os.environ.get("PATH", "")}
            ), mock.patch.object(
                module,
                "start_proxy",
                return_value=(
                    fake_process,
                    root / "runtime-config.yaml",
                    root / "proxy.stdout.log",
                    root / "proxy.stderr.log",
                ),
            ) as start_proxy:
                module.start_managed_proxy(
                    args,
                    {"PATH": os.environ.get("PATH", "")},
                    root / "runtime-root",
                )

        runtime_config_text = start_proxy.call_args.args[1]
        reparsed = module.parse_proxy_source(runtime_config_text)
        qwen_surface = reparsed.upstream_surface_defaults["LOCAL-QWEN"]

        self.assertEqual(
            reparsed.upstreams["LOCAL-QWEN"]["api_root"],
            "http://127.0.0.1:9997/v1",
        )
        self.assertEqual(qwen_surface.input_modalities, ("text",))
        self.assertFalse(qwen_surface.supports_search)

    def test_parse_args_exposes_structured_timeout_policy(self):
        module = load_module()

        args = module.parse_args(
            [
                "--client",
                "gemini",
                "--proxy-health-timeout-secs",
                "75",
                "--process-stop-grace-secs",
                "18",
            ]
        )

        self.assertEqual(args.proxy_health_timeout_secs, 75)
        self.assertEqual(args.process_stop_grace_secs, 18)

    def test_build_client_env_keeps_home_and_client_state_out_of_real_home(self):
        module = load_module()
        base_env = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": "/home/real-user",
            "OPENAI_API_KEY": "real-openai",
            "ANTHROPIC_API_KEY": "real-anthropic",
            "GEMINI_API_KEY": "real-gemini",
        }

        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir)
            codex_env = module.build_client_env(
                "codex",
                base_env,
                "http://127.0.0.1:18888",
                root / "codex-home",
            )
            claude_env = module.build_client_env(
                "claude",
                base_env,
                "http://127.0.0.1:18888",
                root / "claude-home",
            )
            gemini_env = module.build_client_env(
                "gemini",
                base_env,
                "http://127.0.0.1:18888",
                root / "gemini-home",
            )

        self.assertNotEqual(codex_env["HOME"], "/home/real-user")
        self.assertTrue(codex_env["HOME"].startswith(temp_dir))
        self.assertTrue(codex_env["CODEX_HOME"].startswith(temp_dir))
        self.assertNotEqual(claude_env["HOME"], "/home/real-user")
        self.assertTrue(claude_env["CLAUDE_CONFIG_DIR"].startswith(temp_dir))
        self.assertNotEqual(gemini_env["HOME"], "/home/real-user")
        self.assertTrue(gemini_env["HOME"].startswith(temp_dir))
        for key in (
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "TMPDIR",
        ):
            self.assertFalse(gemini_env[key].startswith("/home/real-user"))

    def test_build_client_env_reuses_host_rust_toolchain_homes_for_all_clients(self):
        module = load_module()

        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir)
            host_home = root / "host-home"
            cargo_home = host_home / ".cargo"
            rustup_home = host_home / ".rustup"
            cargo_home.mkdir(parents=True, exist_ok=True)
            rustup_home.mkdir(parents=True, exist_ok=True)
            base_env = {
                "PATH": os.environ.get("PATH", ""),
                "HOME": str(host_home),
            }

            codex_env = module.build_client_env(
                "codex",
                base_env,
                "http://127.0.0.1:18888",
                root / "codex-home",
            )
            claude_env = module.build_client_env(
                "claude",
                base_env,
                "http://127.0.0.1:18888",
                root / "claude-home",
            )
            gemini_env = module.build_client_env(
                "gemini",
                base_env,
                "http://127.0.0.1:18888",
                root / "gemini-home",
            )

            for client_env in (codex_env, claude_env, gemini_env):
                self.assertEqual(client_env["CARGO_HOME"], str(cargo_home))
                self.assertEqual(client_env["RUSTUP_HOME"], str(rustup_home))
                self.assertNotEqual(client_env["HOME"], str(host_home))


if __name__ == "__main__":
    unittest.main()
