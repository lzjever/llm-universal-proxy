import pathlib
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
GOVERNANCE_SCRIPT = REPO_ROOT / "scripts" / "check-governance.sh"


class GovernanceTests(unittest.TestCase):
    def test_governance_tracks_dynamic_proxy_binary_rule(self):
        script = GOVERNANCE_SCRIPT.read_text(encoding="utf-8")

        self.assertIn(
            'check_contains "scripts/real_cli_matrix.py" "def default_proxy_binary_path("',
            script,
        )
        self.assertIn(
            'check_contains "scripts/real_cli_matrix.py" \'DEFAULT_PROXY_BINARY = default_proxy_binary_path()\'',
            script,
        )
        self.assertIn(
            'check_contains "scripts/interactive_cli.py" \'default=str(default_proxy_binary_path())\'',
            script,
        )
        self.assertNotIn(
            'check_contains "scripts/real_cli_matrix.py" \'DEFAULT_PROXY_BINARY = REPO_ROOT / "target" / "release" / "llm-universal-proxy"\'',
            script,
        )


if __name__ == "__main__":
    unittest.main()
