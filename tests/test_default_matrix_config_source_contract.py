import importlib.util
import pathlib
import subprocess
import sys
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


if __name__ == "__main__":
    unittest.main()
