import importlib.util
import os
import pathlib
import sys
import tempfile
import unittest
from unittest import mock


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "interactive_cli.py"


def load_module():
    spec = importlib.util.spec_from_file_location("interactive_cli", SCRIPT_PATH)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class InteractiveCliTests(unittest.TestCase):
    def test_build_interactive_client_command_routes_all_clients_through_proxy(self):
        module = load_module()
        workspace_dir = pathlib.Path("/tmp/workspace")
        proxy_base = "http://127.0.0.1:18888"

        codex_command = module.build_interactive_client_command(
            "codex",
            proxy_base,
            "minimax-anth",
            workspace_dir,
            extra_args=["--no-alt-screen"],
        )
        claude_command = module.build_interactive_client_command(
            "claude",
            proxy_base,
            "claude-sonnet-4-6",
            workspace_dir,
        )
        gemini_command = module.build_interactive_client_command(
            "gemini",
            proxy_base,
            "minimax-openai",
            workspace_dir,
        )

        self.assertEqual(codex_command[0], "codex")
        self.assertIn("--model", codex_command)
        self.assertIn("minimax-anth", codex_command)
        self.assertIn('model_provider="proxy"', codex_command)
        self.assertIn(
            f'model_providers.proxy.base_url="{proxy_base}/openai/v1"',
            codex_command,
        )
        self.assertIn("--no-alt-screen", codex_command)

        self.assertEqual(claude_command[0], "claude")
        self.assertIn("--model", claude_command)
        self.assertIn("claude-sonnet-4-6", claude_command)
        self.assertIn("--bare", claude_command)
        self.assertIn("--add-dir", claude_command)
        self.assertIn(str(workspace_dir), claude_command)

        self.assertEqual(gemini_command[0], "gemini")
        self.assertIn("--model", gemini_command)
        self.assertIn("minimax-openai", gemini_command)
        self.assertIn("--include-directories", gemini_command)
        self.assertIn(str(workspace_dir), gemini_command)
        self.assertIn("--sandbox=false", gemini_command)

    def test_build_client_launch_uses_runner_managed_home(self):
        module = load_module()

        with tempfile.TemporaryDirectory() as temp_dir:
            state_root = pathlib.Path(temp_dir) / "state"
            launch = module.build_client_launch(
                client_name="codex",
                model="minimax-anth",
                proxy_base="http://127.0.0.1:18888",
                workspace_dir=pathlib.Path(temp_dir),
                base_env={
                    "PATH": os.environ.get("PATH", ""),
                    "HOME": "/home/real-user",
                    "OPENAI_API_KEY": "real-secret",
                },
                state_root=state_root,
                extra_args=[],
            )

        self.assertEqual(launch.client_name, "codex")
        self.assertEqual(launch.model, "minimax-anth")
        self.assertNotEqual(launch.env["HOME"], "/home/real-user")
        self.assertTrue(str(launch.home_dir).startswith(str(state_root)))
        self.assertEqual(launch.env["OPENAI_API_KEY"], "dummy")
        self.assertEqual(
            launch.env["OPENAI_BASE_URL"],
            "http://127.0.0.1:18888/openai/v1",
        )

    def test_run_attaches_to_existing_proxy_without_starting_proxy(self):
        module = load_module()

        with tempfile.TemporaryDirectory() as temp_dir:
            state_root = pathlib.Path(temp_dir) / "state"
            with mock.patch.object(
                module, "ensure_client_binary"
            ), mock.patch.object(
                module, "ensure_proxy_binary"
            ) as ensure_proxy_binary, mock.patch.object(
                module.matrix, "wait_for_health"
            ) as wait_for_health, mock.patch.object(
                module.matrix,
                "start_proxy",
                side_effect=AssertionError("start_proxy should not run"),
            ), mock.patch.object(
                module, "launch_client_interactive", return_value=0
            ) as launch_client:
                exit_code = module.run(
                    [
                        "--client",
                        "codex",
                        "--proxy-base",
                        "http://127.0.0.1:18888",
                        "--workspace",
                        temp_dir,
                        "--state-root",
                        str(state_root),
                    ]
                )

        self.assertEqual(exit_code, 0)
        ensure_proxy_binary.assert_not_called()
        wait_for_health.assert_called_once_with("http://127.0.0.1:18888")
        launch = launch_client.call_args.args[0]
        self.assertEqual(launch.proxy_base, "http://127.0.0.1:18888")

    def test_run_autostart_derives_runtime_config_and_skips_optional_qwen_when_missing(self):
        module = load_module()

        class FakeProcess:
            def poll(self):
                return None

            def wait(self, timeout=None):
                return 0

            def terminate(self):
                return None

            def kill(self):
                return None

        with tempfile.TemporaryDirectory() as temp_dir:
            state_root = pathlib.Path(temp_dir) / "state"
            binary_path = pathlib.Path(temp_dir) / "llm-universal-proxy"
            binary_path.write_text("", encoding="utf-8")
            missing_env = pathlib.Path(temp_dir) / "missing.env"
            observed = {}

            def fake_start_proxy(proxy_binary, runtime_config_text, report_dir, proxy_env):
                observed["proxy_binary"] = proxy_binary
                observed["runtime_config_text"] = runtime_config_text
                observed["report_dir"] = report_dir
                observed["proxy_env"] = proxy_env
                return (
                    FakeProcess(),
                    report_dir / "runtime-config.yaml",
                    report_dir / "proxy.stdout.log",
                    report_dir / "proxy.stderr.log",
                )

            with mock.patch.object(
                module, "ensure_client_binary"
            ), mock.patch.object(
                module, "ensure_proxy_binary"
            ), mock.patch.object(
                module.matrix, "start_proxy", side_effect=fake_start_proxy
            ), mock.patch.object(
                module.matrix, "wait_for_health"
            ) as wait_for_health, mock.patch.object(
                module.matrix, "stop_proxy"
            ) as stop_proxy, mock.patch.object(
                module, "launch_client_interactive", return_value=0
            ) as launch_client:
                exit_code = module.run(
                    [
                        "--client",
                        "gemini",
                        "--workspace",
                        temp_dir,
                        "--state-root",
                        str(state_root),
                        "--binary",
                        str(binary_path),
                        "--config-source",
                        str(REPO_ROOT / "proxy-test-minimax-and-local.yaml"),
                        "--env-file",
                        str(missing_env),
                    ]
                )

        self.assertEqual(exit_code, 0)
        self.assertEqual(observed["proxy_binary"], binary_path)
        self.assertNotIn("LOCAL-QWEN:", observed["runtime_config_text"])
        self.assertNotIn('qwen-local: "LOCAL-QWEN:', observed["runtime_config_text"])
        wait_for_health.assert_called_once_with("http://127.0.0.1:18888")
        stop_proxy.assert_called_once()
        launch = launch_client.call_args.args[0]
        self.assertEqual(launch.client_name, "gemini")
        self.assertEqual(launch.model, "minimax-openai")


if __name__ == "__main__":
    unittest.main()
