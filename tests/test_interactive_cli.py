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
    sys.modules.pop("interactive_cli", None)
    spec = importlib.util.spec_from_file_location("interactive_cli", SCRIPT_PATH)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class InteractiveCliTests(unittest.TestCase):
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
            ),
            [
                "gemini",
                "--model",
                "minimax-openai",
                "--include-directories",
                str(workspace),
            ],
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
                    ]
                )

        self.assertEqual(exit_code, 0)
        start_proxy.assert_not_called()
        wait_for_health.assert_called_once_with("http://127.0.0.1:18888")
        stop_proxy.assert_called_once_with(None)
        launch_args = launch_client.call_args.args
        self.assertEqual(
            launch_args[0],
            module.build_interactive_command(
                "codex",
                pathlib.Path(temp_dir).resolve(),
                "minimax-openai",
                "http://127.0.0.1:18888",
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
                    ]
                )

        self.assertEqual(exit_code, 0)
        start_proxy.assert_called_once()
        wait_for_health.assert_called_once_with("http://127.0.0.1:18888")
        stop_proxy.assert_called_once_with(fake_process)

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


if __name__ == "__main__":
    unittest.main()
