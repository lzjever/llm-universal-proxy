import importlib.util
import json
import pathlib
import sys
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "real_cli_matrix.py"
TOOL_IDENTITY_FIXTURE_PATH = (
    REPO_ROOT
    / "scripts"
    / "fixtures"
    / "cli_matrix"
    / "smoke"
    / "tool_identity_public_contract.json"
)


def load_module():
    spec = importlib.util.spec_from_file_location("real_cli_matrix_contracts", SCRIPT_PATH)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def make_fixture(module, verifier):
    return module.TaskFixture(
        fixture_id="tool_identity_public_contract",
        kind="smoke",
        description="",
        prompt="List public editing tools.",
        verifier=verifier,
        timeout_secs=30,
        workspace_template=None,
    )


class CliMatrixContractTests(unittest.TestCase):
    def read_text(self, relative_path: str) -> str:
        return (REPO_ROOT / relative_path).read_text(encoding="utf-8")

    def test_stdout_contract_rejects_output_that_never_confirms_public_edit_tool(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any": ["apply_patch"],
                "not_contains": ["__llmup_custom__"],
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "Editing tools are available in this environment.",
            workspace_dir=None,
        )

        self.assertFalse(ok)
        self.assertIn("apply_patch", message)

    def test_stdout_contract_accepts_output_with_public_apply_patch_name(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any": ["apply_patch"],
                "not_contains": ["__llmup_custom__"],
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "Public editing tool: apply_patch",
            workspace_dir=None,
        )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_tool_identity_fixture_requires_public_apply_patch_name(self):
        payload = json.loads(TOOL_IDENTITY_FIXTURE_PATH.read_text(encoding="utf-8"))

        self.assertEqual(payload["id"], "tool_identity_public_contract")
        self.assertEqual(payload["verifier"]["type"], "stdout_contract")
        self.assertEqual(payload["verifier"]["contains_any"], ["apply_patch"])
        self.assertIn("__llmup_custom__", payload["verifier"]["not_contains"])

    def test_clients_guide_uses_live_surface_truth_for_codex_wrapper(self):
        text = self.read_text("docs/clients.md")

        self.assertIn("live `llmup.surface` metadata", text)
        self.assertNotIn("temporary model metadata", text)

    def test_prd_and_plan_describe_current_real_cli_contract_scope(self):
        prd_text = self.read_text("docs/PRD.md")
        plan_text = self.read_text("docs/max-compat-development-plan.md")

        self.assertIn("Current real-client regression coverage is intentionally narrow", prd_text)
        self.assertIn("public tool identity", prd_text)
        self.assertIn("workspace-edit", prd_text)
        self.assertIn("public tool enumeration contract", plan_text)
        self.assertIn("workspace-edit execution", plan_text)


if __name__ == "__main__":
    unittest.main()
