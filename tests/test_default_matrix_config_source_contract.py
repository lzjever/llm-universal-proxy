import importlib.util
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import textwrap
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "real_cli_matrix.py"
TRACKED_CONFIG_PATH = (
    REPO_ROOT
    / "scripts"
    / "fixtures"
    / "cli_matrix"
    / "default_proxy_test_matrix.yaml"
)
LEGACY_IGNORED_CONFIG_PATH = REPO_ROOT / "proxy-test-minimax-and-local.yaml"
COMPAT_SCRIPT_PATH = REPO_ROOT / "scripts" / "test_compatibility.sh"
PROVIDER_KEY_RE = re.compile(
    r"sk-(?:cp|ant|proj|live|test)-[A-Za-z0-9_-]{16,}|sk-[A-Za-z0-9_-]{32,}"
)


def load_module():
    spec = importlib.util.spec_from_file_location(
        "real_cli_matrix_default_config_source_contract",
        SCRIPT_PATH,
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class DefaultMatrixConfigSourceContractTests(unittest.TestCase):
    def test_default_config_source_uses_tracked_fixture_file(self):
        module = load_module()

        self.assertEqual(module.DEFAULT_CONFIG_SOURCE, TRACKED_CONFIG_PATH)
        self.assertTrue(TRACKED_CONFIG_PATH.exists())
        self.assertEqual(
            TRACKED_CONFIG_PATH.parent,
            REPO_ROOT / "scripts" / "fixtures" / "cli_matrix",
        )
        not_ignored = subprocess.run(
            ["git", "check-ignore", str(TRACKED_CONFIG_PATH.relative_to(REPO_ROOT))],
            cwd=REPO_ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=False,
        )
        self.assertEqual(not_ignored.returncode, 1, not_ignored.stdout + not_ignored.stderr)

    def test_default_config_source_no_longer_uses_ignored_repo_root_file(self):
        module = load_module()

        self.assertNotEqual(module.DEFAULT_CONFIG_SOURCE, LEGACY_IGNORED_CONFIG_PATH)
        ignored = subprocess.run(
            ["git", "check-ignore", str(LEGACY_IGNORED_CONFIG_PATH.relative_to(REPO_ROOT))],
            cwd=REPO_ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=False,
        )
        self.assertEqual(ignored.returncode, 0, ignored.stderr)

    def test_test_compatibility_script_defaults_to_tracked_config(self):
        script_text = COMPAT_SCRIPT_PATH.read_text(encoding="utf-8")

        self.assertIn(
            "scripts/fixtures/cli_matrix/default_proxy_test_matrix.yaml",
            script_text,
        )
        self.assertNotIn("CONFIG=\"${CONFIG:-proxy-test-minimax-and-local.yaml}\"", script_text)

    def test_test_compatibility_auto_start_renders_runtime_config_from_env_file(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            env_file = pathlib.Path(temp_dir) / ".env.test"
            env_file.write_text(
                "\n".join(
                    [
                        'export PRESET_ENDPOINT_API_KEY="proxy-only-secret"',
                        'export PRESET_OPENAI_ENDPOINT_BASE_URL="https://openai-compatible.example/v1"',
                        'export PRESET_ANTHROPIC_ENDPOINT_BASE_URL="https://anthropic-compatible.example/v1"',
                        'export PRESET_ENDPOINT_MODEL="provider-configured-model"',
                        "",
                    ]
                ),
                encoding="utf-8",
            )
            command = textwrap.dedent(
                f"""
                set -euo pipefail
                source scripts/test_compatibility.sh
                BASE_URL="http://127.0.0.1:19991"
                render_auto_start_config "{TRACKED_CONFIG_PATH}" "{env_file}"
                runtime_config="$AUTO_START_RUNTIME_CONFIG"
                runtime_dir="$AUTO_START_RUNTIME_DIR"
                test -n "$runtime_config"
                test -n "$runtime_dir"
                test -d "$runtime_dir"
                test -f "$runtime_config"
                grep -Fq "listen: 127.0.0.1:19991" "$runtime_config"
                grep -Fq "api_root: https://openai-compatible.example/v1" "$runtime_config"
                grep -Fq "api_root: https://anthropic-compatible.example/v1" "$runtime_config"
                grep -Fq '"PRESET-OPENAI-COMPATIBLE:provider-configured-model"' "$runtime_config"
                grep -Fq '"PRESET-ANTHROPIC-COMPATIBLE:provider-configured-model"' "$runtime_config"
                ! grep -Fq "api_root: PRESET_" "$runtime_config"
                ! grep -Fq "PRESET_ENDPOINT_MODEL" "$runtime_config"
                cleanup_auto_start_runtime_config
                test ! -e "$runtime_dir"
                """
            )

            completed = subprocess.run(
                ["bash", "-c", command],
                cwd=REPO_ROOT,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                check=False,
            )

        self.assertEqual(
            completed.returncode,
            0,
            completed.stdout + completed.stderr,
        )

    def test_test_compatibility_auto_start_cleans_runtime_dir_on_start_failure(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir)
            runtime_tmp = root / "tmp"
            runtime_tmp.mkdir()
            env_file = root / ".env.test"
            env_file.write_text(
                "\n".join(
                    [
                        'export PRESET_ENDPOINT_API_KEY="proxy-only-secret"',
                        'export PRESET_OPENAI_ENDPOINT_BASE_URL="https://openai-compatible.example/v1"',
                        'export PRESET_ANTHROPIC_ENDPOINT_BASE_URL="https://anthropic-compatible.example/v1"',
                        'export PRESET_ENDPOINT_MODEL="provider-configured-model"',
                        "",
                    ]
                ),
                encoding="utf-8",
            )
            fake_binary = root / "fake-proxy"
            fake_binary.write_text(
                "#!/usr/bin/env bash\n"
                "trap 'exit 0' TERM\n"
                "while :; do sleep 1; done\n",
                encoding="utf-8",
            )
            fake_binary.chmod(0o755)
            command = textwrap.dedent(
                f"""
                set -euo pipefail
                source scripts/test_compatibility.sh
                wait_for_proxy() {{ return 1; }}
                BINARY="{fake_binary}"
                CONFIG="{TRACKED_CONFIG_PATH}"
                ENV_FILE="{env_file}"
                TMPDIR="{runtime_tmp}"
                BASE_URL="http://127.0.0.1:19992"
                main --auto-start
                """
            )

            completed = subprocess.run(
                ["bash", "-c", command],
                cwd=REPO_ROOT,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                check=False,
                timeout=10,
            )
            leftovers = list(runtime_tmp.glob("llmup-compat-runtime.*"))

        self.assertEqual(completed.returncode, 1, completed.stdout + completed.stderr)
        self.assertEqual(leftovers, [], completed.stdout + completed.stderr)

    def test_test_compatibility_skips_local_qwen_without_local_qwen_env(self):
        command = textwrap.dedent(
            """
            set -euo pipefail
            source scripts/test_compatibility.sh
            test_json() {
                echo "unexpected test_json call: $1" >&2
                return 99
            }
            test_sse() {
                echo "unexpected test_sse call: $1" >&2
                return 99
            }
            output_file="$(mktemp)"
            test_local_qwen >"$output_file"
            test "$SKIP" -eq 1
            test "$PASS" -eq 0
            test "$FAIL" -eq 0
            grep -Fq "[SKIP]" "$output_file"
            grep -Fq "LOCAL_QWEN_BASE_URL" "$output_file"
            grep -Fq "LOCAL_QWEN_MODEL" "$output_file"
            """
        )

        completed = subprocess.run(
            ["bash", "-c", command],
            cwd=REPO_ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=False,
            env={
                "PATH": os.environ.get("PATH", ""),
                "PRESET_ENDPOINT_API_KEY": "proxy-only-secret",
                "PRESET_OPENAI_ENDPOINT_BASE_URL": "https://openai-compatible.example/v1",
                "PRESET_ANTHROPIC_ENDPOINT_BASE_URL": "https://anthropic-compatible.example/v1",
                "PRESET_ENDPOINT_MODEL": "provider-configured-model",
            },
        )

        self.assertEqual(
            completed.returncode,
            0,
            completed.stdout + completed.stderr,
        )

    def test_test_compatibility_runs_local_qwen_when_local_qwen_env_exists(self):
        command = textwrap.dedent(
            """
            set -euo pipefail
            source scripts/test_compatibility.sh
            JSON_CALLS=0
            SSE_CALLS=0
            test_json() {
                JSON_CALLS=$((JSON_CALLS + 1))
            }
            test_sse() {
                SSE_CALLS=$((SSE_CALLS + 1))
            }
            test_local_qwen
            test "$JSON_CALLS" -eq 3
            test "$SSE_CALLS" -eq 3
            test "$SKIP" -eq 0
            """
        )

        completed = subprocess.run(
            ["bash", "-c", command],
            cwd=REPO_ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=False,
            env={
                "PATH": os.environ.get("PATH", ""),
                "PRESET_ENDPOINT_API_KEY": "proxy-only-secret",
                "PRESET_OPENAI_ENDPOINT_BASE_URL": "https://openai-compatible.example/v1",
                "PRESET_ANTHROPIC_ENDPOINT_BASE_URL": "https://anthropic-compatible.example/v1",
                "PRESET_ENDPOINT_MODEL": "provider-configured-model",
                "LOCAL_QWEN_BASE_URL": "http://127.0.0.1:9997/v1",
                "LOCAL_QWEN_MODEL": "qwen3.5-9b-awq",
            },
        )

        self.assertEqual(
            completed.returncode,
            0,
            completed.stdout + completed.stderr,
        )

    def test_tracked_default_config_uses_env_credentials_without_provider_keys(self):
        config_text = TRACKED_CONFIG_PATH.read_text(encoding="utf-8")

        self.assertIn("credential_env: PRESET_ENDPOINT_API_KEY", config_text)
        self.assertIn("PRESET-OPENAI-COMPATIBLE", config_text)
        self.assertIn("PRESET-ANTHROPIC-COMPATIBLE", config_text)
        self.assertIn("preset-openai-compatible", config_text)
        self.assertIn("preset-anthropic-compatible", config_text)
        self.assertNotIn("MINIMAX", config_text.upper())
        self.assertNotIn("minimax-openai", config_text)
        self.assertNotIn("minimax-anth", config_text)
        self.assertNotIn("credential_actual", config_text)
        self.assertIsNone(PROVIDER_KEY_RE.search(config_text))


if __name__ == "__main__":
    unittest.main()
