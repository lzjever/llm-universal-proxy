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


def make_context(module, client_name: str):
    return module.VerifierContext(client_name=client_name)


class CliMatrixContractTests(unittest.TestCase):
    def read_text(self, relative_path: str) -> str:
        return (REPO_ROOT / relative_path).read_text(encoding="utf-8")

    def test_stdout_contract_rejects_output_that_never_confirms_client_public_edit_tool(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "codex": ["apply_patch"],
                    "claude": ["Edit"],
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
                "not_contains": ["__llmup_custom__"],
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "Editing tools are available in this environment.",
            workspace_dir=None,
            context=make_context(module, "claude"),
        )

        self.assertFalse(ok)
        self.assertIn("Edit", message)

    def test_stdout_contract_accepts_output_with_client_specific_public_tool_name(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "codex": ["apply_patch"],
                    "claude": ["Edit"],
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
                "not_contains": ["__llmup_custom__"],
            },
        )

        for client_name, stdout_text in (
            ("codex", "**Public editing tool used:** `apply_patch`"),
            ("claude", "The `Edit` tool."),
            ("gemini", "**Tool used:** `Replace` (workspace file editing tool)"),
        ):
            with self.subTest(client_name=client_name):
                ok, message = module.verify_fixture_output(
                    fixture,
                    stdout_text,
                    workspace_dir=None,
                    context=make_context(module, client_name),
                )

                self.assertTrue(ok, message)
                self.assertEqual(message, "")

    def test_stdout_contract_accepts_codex_json_completed_agent_message_for_presented_tool_name(
        self,
    ):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "codex": ["apply_patch"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )
        stdout_text = "\n".join(
            [
                json.dumps({"type": "turn.started"}),
                json.dumps(
                    {
                        "type": "item.completed",
                        "item": {
                            "id": "item_1",
                            "type": "agent_message",
                            "text": "apply_patch",
                        },
                    }
                ),
                json.dumps({"type": "turn.completed"}),
            ]
        )

        ok, message = module.verify_fixture_output(
            fixture,
            stdout_text,
            workspace_dir=None,
            context=make_context(module, "codex"),
        )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_stdout_contract_accepts_listed_tool_name_under_presented_tool_name_mode(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "The editing tools available are:\n\n1. `Replace`\n2. `write_file`\n",
            workspace_dir=None,
            context=make_context(module, "gemini"),
        )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_stdout_contract_accepts_single_line_backticked_tool_list_under_presented_tool_name_mode(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "`replace`, `write_file`",
            workspace_dir=None,
            context=make_context(module, "gemini"),
        )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_stdout_contract_accepts_client_labeled_listed_tool_name_under_presented_tool_name_mode(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "claude": ["Edit"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "Based on the public contract definition in this environment, the exact public names of the editing tools are:\n\n- **codex**: `apply_patch`\n- **claude**: `Edit`\n- **gemini**: `replace`\n",
            workspace_dir=None,
            context=make_context(module, "claude"),
        )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_stdout_contract_accepts_bold_tool_name_with_description_under_presented_tool_name_mode(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "The editing tools available are:\n\n1. **replace** - Replaces text within a file\n2. **write_file** - Writes content to a specified file\n",
            workspace_dir=None,
            context=make_context(module, "gemini"),
        )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_stdout_contract_accepts_markdown_wrapped_public_tool_name_context(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "claude": ["Edit"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "The **public editing tool name I used was `Edit`**.",
            workspace_dir=None,
            context=make_context(module, "claude"),
        )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_stdout_contract_accepts_public_tool_i_used_context_with_bold_term(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "The public editing tool I used is: **replace**",
            workspace_dir=None,
            context=make_context(module, "gemini"),
        )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_stdout_contract_accepts_public_editing_tool_label_with_backticked_term(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "Public editing tool: `replace`",
            workspace_dir=None,
            context=make_context(module, "gemini"),
        )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_stdout_contract_accepts_public_tool_i_actually_used_with_bold_term(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "The public editing tool I actually used was **replace**.",
            workspace_dir=None,
            context=make_context(module, "gemini"),
        )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_stdout_contract_accepts_plain_tool_i_used_was_or_is_with_tool_name(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        for stdout_text in (
            "The tool I used was `replace`.",
            "The tool I used is `replace`.",
        ):
            with self.subTest(stdout_text=stdout_text):
                ok, message = module.verify_fixture_output(
                    fixture,
                    stdout_text,
                    workspace_dir=None,
                    context=make_context(module, "gemini"),
                )

                self.assertTrue(ok, message)
                self.assertEqual(message, "")

    def test_stdout_contract_accepts_used_tool_name_mention_mode_for_narrative_use(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "used_tool_name_mention",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "I fixed the regression, validated the workspace, and used `replace`.",
            workspace_dir=None,
            context=make_context(module, "gemini"),
        )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_stdout_contract_rejects_other_client_public_tool_names_when_strict_client_scope_enabled(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "codex": ["apply_patch"],
                    "claude": ["Edit"],
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
                "reject_other_client_contains_any_by_client": True,
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "The exact public editing tools visible here are:\n\n- `Edit`\n- `apply_patch`\n- `replace`\n",
            workspace_dir=None,
            context=make_context(module, "claude"),
        )

        self.assertFalse(ok)
        self.assertIn("other clients", message)
        self.assertIn("apply_patch", message)
        self.assertIn("replace", message)

    def test_stdout_contract_rejects_other_client_public_tool_names_from_codex_json_agent_message(
        self,
    ):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "codex": ["apply_patch"],
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
                "reject_other_client_contains_any_by_client": True,
            },
        )
        stdout_text = "\n".join(
            [
                json.dumps({"type": "turn.started"}),
                json.dumps(
                    {
                        "type": "item.completed",
                        "item": {
                            "id": "item_1",
                            "type": "agent_message",
                            "text": "apply_patch, replace",
                        },
                    }
                ),
                json.dumps({"type": "turn.completed"}),
            ]
        )

        ok, message = module.verify_fixture_output(
            fixture,
            stdout_text,
            workspace_dir=None,
            context=make_context(module, "codex"),
        )

        self.assertFalse(ok)
        self.assertIn("other clients", message)
        self.assertIn("replace", message)

    def test_stdout_contract_rejects_plain_verb_use_under_presented_tool_name_mode(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "I will replace the line and then verify the result.",
            workspace_dir=None,
            context=make_context(module, "gemini"),
        )

        self.assertFalse(ok)
        self.assertIn("replace", message)

    def test_stdout_contract_rejects_backticked_verb_in_plain_sentence_under_presented_tool_name_mode(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "I will `replace` the line and then verify the result.",
            workspace_dir=None,
            context=make_context(module, "gemini"),
        )

        self.assertFalse(ok)
        self.assertIn("replace", message)

    def test_stdout_contract_rejects_generic_tool_explanation_under_presented_tool_name_mode(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "claude": ["Edit"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "The editing tool is used to edit files safely.",
            workspace_dir=None,
            context=make_context(module, "claude"),
        )

        self.assertFalse(ok)
        self.assertIn("Edit", message)

    def test_stdout_contract_rejects_public_tool_usage_explanation_for_verb_named_tool(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "The public editing tool is used to replace text within a file.",
            workspace_dir=None,
            context=make_context(module, "gemini"),
        )

        self.assertFalse(ok)
        self.assertIn("replace", message)

    def test_stdout_contract_rejects_public_tool_name_usage_explanation_for_verb_named_tool(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "The public editing tool name is used to replace text within a file.",
            workspace_dir=None,
            context=make_context(module, "gemini"),
        )

        self.assertFalse(ok)
        self.assertIn("replace", message)

    def test_stdout_contract_rejects_used_tool_name_mention_mode_for_backticked_verb_intent(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "used_tool_name_mention",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "I still need to `replace` the broken line and validate the workspace.",
            workspace_dir=None,
            context=make_context(module, "gemini"),
        )

        self.assertFalse(ok)
        self.assertIn("replace", message)

    def test_stdout_contract_rejects_plain_tool_i_used_was_to_verb_phrase(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "The tool I used was to replace the broken line.",
            workspace_dir=None,
            context=make_context(module, "gemini"),
        )

        self.assertFalse(ok)
        self.assertIn("replace", message)

    def test_stdout_contract_uses_token_boundaries_for_client_specific_tool_names(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "claude": ["Edit"],
                },
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "Editing tools are available in this environment.",
            workspace_dir=None,
            context=make_context(module, "claude"),
        )

        self.assertFalse(ok)
        self.assertIn("Edit", message)

    def test_stdout_contract_requires_client_context_for_client_specific_expectations(self):
        module = load_module()
        fixture = make_fixture(
            module,
            {
                "type": "stdout_contract",
                "contains_any_by_client": {"codex": ["apply_patch"]},
                "contains_any_by_client_match_mode": "presented_tool_name",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "**Public editing tool used:** `apply_patch`",
            workspace_dir=None,
        )

        self.assertFalse(ok)
        self.assertIn("client_name", message)

    def test_tool_identity_fixture_requires_client_specific_public_tool_names(self):
        payload = json.loads(TOOL_IDENTITY_FIXTURE_PATH.read_text(encoding="utf-8"))

        self.assertEqual(payload["id"], "tool_identity_public_contract")
        self.assertEqual(payload["verifier"]["type"], "stdout_contract")
        self.assertEqual(
            payload["verifier"]["contains_any_by_client"],
            {
                "codex": ["apply_patch"],
                "claude": ["Edit"],
                "gemini": ["replace"],
            },
        )
        self.assertEqual(
            payload["verifier"]["contains_any_by_client_match_mode"],
            "presented_tool_name",
        )
        self.assertTrue(payload["verifier"]["reject_other_client_contains_any_by_client"])
        self.assertIn("{client_name}", payload["prompt_template"])
        self.assertIn("exactly one line", payload["prompt_template"])
        self.assertIn("Do not mention any other clients", payload["prompt_template"])
        self.assertIn("do not use any client names as answers", payload["prompt_template"])
        self.assertIn(
            "Do not answer with task IDs, fixture IDs, contract names, workspace/path words, or filenames.",
            payload["prompt_template"],
        )
        self.assertIn("__llmup_custom__", payload["verifier"]["not_contains"])
        self.assertIn("current client surface", payload["prompt"])
        self.assertIn("Do not list tools from other clients", payload["prompt"])
        self.assertIn("Do not use any client names as answers", payload["prompt"])
        self.assertIn(
            "Do not answer with task IDs, fixture IDs, contract names, workspace/path words, or filenames.",
            payload["prompt"],
        )
        self.assertIn("you cannot actually use here", payload["prompt"])
        self.assertIn("example/counterexample names", payload["prompt"])

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
