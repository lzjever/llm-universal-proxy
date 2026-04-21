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
        stop_proxy.assert_called_once_with(None, terminate_grace_secs=15)
        launch_args = launch_client.call_args.args
        parsed_source = module.parse_proxy_source(
            pathlib.Path(module.DEFAULT_CONFIG_SOURCE).read_text(encoding="utf-8")
        )
        self.assertEqual(
            launch_args[0],
            module.build_interactive_command(
                "codex",
                pathlib.Path(temp_dir).resolve(),
                "minimax-openai",
                "http://127.0.0.1:18888",
                client_home=pathlib.Path(launch_args[2]["HOME"]),
                model_limits=module.resolve_model_limits(parsed_source, "minimax-openai"),
                codex_metadata=module.resolve_codex_model_metadata(
                    parsed_source, "minimax-openai"
                ),
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
        stop_proxy.assert_called_once_with(fake_process, terminate_grace_secs=15)

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
