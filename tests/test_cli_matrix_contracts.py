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


def make_trace_context(module, client_name: str, trace_entries):
    return module.VerifierContext(
        client_name=client_name,
        trace_entries=tuple(trace_entries),
    )


def trace_tool_identity_verifier():
    return {
        "type": "tool_identity_contract",
        "contains_any_by_client": {
            "codex": ["apply_patch"],
            "claude": ["Edit"],
        },
        "contains_any_by_client_match_mode": "presented_tool_name",
        "reject_other_client_contains_any_by_client": True,
        "not_contains": ["__llmup_custom__"],
    }


def fake_request_trace_entry(
    *,
    request_id="req_tool_identity",
    client_tool_names=None,
    upstream_tool_names=None,
    client_tool_choice=None,
    upstream_tool_choice=None,
):
    client_summary = {
        "tool_names": client_tool_names if client_tool_names is not None else ["Edit"],
    }
    if client_tool_choice is not None:
        client_summary["tool_choice"] = client_tool_choice
    upstream_summary = {
        "tool_names": upstream_tool_names if upstream_tool_names is not None else ["Edit"],
    }
    if upstream_tool_choice is not None:
        upstream_summary["tool_choice"] = upstream_tool_choice
    return {
        "timestamp_ms": 1,
        "request_id": request_id,
        "phase": "request",
        "path": "/anthropic/v1/messages",
        "stream": True,
        "client_format": "anthropic",
        "upstream_format": "openai-completion",
        "client_model": "minimax-anth",
        "upstream_name": "MINIMAX-ANTHROPIC",
        "upstream_model": "MiniMax-M2.7-highspeed",
        "request": {
            "client_summary": client_summary,
            "upstream_summary": upstream_summary,
        },
    }


class CliMatrixContractTests(unittest.TestCase):
    def read_text(self, relative_path: str) -> str:
        return (REPO_ROOT / relative_path).read_text(encoding="utf-8")

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

    def test_stdout_contract_rejects_codex_raw_json_internal_artifact_even_when_final_message_is_public(
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
                "not_contains": ["__llmup_custom__"],
            },
        )
        stdout_text = "\n".join(
            [
                json.dumps({"type": "turn.started"}),
                json.dumps(
                    {
                        "type": "item.completed",
                        "item": {
                            "id": "item_tool",
                            "type": "custom_tool_call",
                            "name": "__llmup_custom__apply_patch",
                        },
                    }
                ),
                json.dumps(
                    {
                        "type": "item.completed",
                        "item": {
                            "id": "item_final",
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

        self.assertFalse(ok)
        self.assertIn("__llmup_custom__", message)

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

    def test_tool_identity_contract_requires_stdout_public_contract_even_with_debug_trace(self):
        module = load_module()
        fixture = make_fixture(module, trace_tool_identity_verifier())

        ok, message = module.verify_fixture_output(
            fixture,
            "Editing tools are available in this environment.",
            workspace_dir=None,
            context=make_trace_context(
                module,
                "claude",
                [fake_request_trace_entry()],
            ),
        )

        self.assertFalse(ok)
        self.assertIn("Edit", message)

    def test_tool_identity_contract_rejects_internal_trace_tool_artifacts(self):
        module = load_module()
        fixture = make_fixture(module, trace_tool_identity_verifier())

        ok, message = module.verify_fixture_output(
            fixture,
            "Public editing tool: `Edit`.",
            workspace_dir=None,
            context=make_trace_context(
                module,
                "claude",
                [
                    fake_request_trace_entry(
                        upstream_tool_names=["Edit", "__llmup_custom__Edit"],
                    )
                ],
            ),
        )

        self.assertFalse(ok)
        self.assertIn("__llmup_custom__Edit", message)

    def test_tool_identity_contract_rejects_internal_tool_choice_trace_artifacts(self):
        module = load_module()
        fixture = make_fixture(module, trace_tool_identity_verifier())

        ok, message = module.verify_fixture_output(
            fixture,
            "Public editing tool: `Edit`.",
            workspace_dir=None,
            context=make_trace_context(
                module,
                "claude",
                [
                    fake_request_trace_entry(
                        upstream_tool_choice={
                            "type": "function",
                            "function": {"name": "__llmup_custom__Edit"},
                        },
                    )
                ],
            ),
        )

        self.assertFalse(ok)
        self.assertIn("tool_choice", message)
        self.assertIn("__llmup_custom__Edit", message)

    def test_tool_identity_contract_rejects_other_client_tool_names_in_trace(self):
        module = load_module()
        fixture = make_fixture(module, trace_tool_identity_verifier())

        ok, message = module.verify_fixture_output(
            fixture,
            "Public editing tool: `Edit`.",
            workspace_dir=None,
            context=make_trace_context(
                module,
                "claude",
                [fake_request_trace_entry(upstream_tool_names=["Edit", "apply_patch"])],
            ),
        )

        self.assertFalse(ok)
        self.assertIn("other clients", message)
        self.assertIn("apply_patch", message)

    def test_tool_identity_contract_rejects_other_client_allowed_tool_choice_names_in_trace(self):
        module = load_module()
        fixture = make_fixture(module, trace_tool_identity_verifier())

        ok, message = module.verify_fixture_output(
            fixture,
            "Public editing tool: `Edit`.",
            workspace_dir=None,
            context=make_trace_context(
                module,
                "claude",
                [
                    fake_request_trace_entry(
                        upstream_tool_choice={
                            "type": "allowed_tools",
                            "allowed_tools": {
                                "mode": "required",
                                "tools": [
                                    {
                                        "type": "function",
                                        "function": {"name": "apply_patch"},
                                    }
                                ],
                            },
                        },
                    )
                ],
            ),
        )

        self.assertFalse(ok)
        self.assertIn("other clients", message)
        self.assertIn("apply_patch", message)

    def test_tool_identity_contract_requires_client_and_upstream_trace_tool_names(self):
        module = load_module()
        fixture = make_fixture(module, trace_tool_identity_verifier())

        ok, message = module.verify_fixture_output(
            fixture,
            "Public editing tool: `Edit`.",
            workspace_dir=None,
            context=make_trace_context(
                module,
                "claude",
                [fake_request_trace_entry(upstream_tool_names=[])],
            ),
        )

        self.assertFalse(ok)
        self.assertIn("upstream tool_names", message)

    def test_tool_identity_contract_fails_closed_when_trace_window_is_empty(self):
        module = load_module()
        fixture = make_fixture(module, trace_tool_identity_verifier())

        ok, message = module.verify_fixture_output(
            fixture,
            "The public editing tool is `Edit`.",
            workspace_dir=None,
            context=make_context(module, "claude"),
        )

        self.assertFalse(ok)
        self.assertIn("debug trace", message)

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

    def test_stdout_contract_rejects_used_tool_name_mention_mode_for_reserved_prefix_tool_name(
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
                "contains_any_by_client_match_mode": "used_tool_name_mention",
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "The `__llmup_custom__apply_patch` tool was used to fix the regression.",
            workspace_dir=None,
            context=make_context(module, "codex"),
        )

        self.assertFalse(ok)
        self.assertIn("apply_patch", message)

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

    def test_clients_guide_uses_live_surface_truth_for_codex_wrapper(self):
        text = self.read_text("docs/clients.md")

        self.assertIn("live `llmup.surface` metadata", text)
        self.assertNotIn("temporary model metadata", text)

    def test_prd_and_plan_describe_current_real_cli_contract_scope(self):
        prd_text = self.read_text("docs/PRD.md")
        plan_text = self.read_text("docs/engineering/max-compat-development-plan.md")

        self.assertIn("Current real-client regression coverage is intentionally narrow", prd_text)
        self.assertIn("public tool identity", prd_text)
        self.assertIn("workspace-edit", prd_text)
        self.assertIn("public tool enumeration contract", plan_text)
        self.assertIn("workspace-edit execution", plan_text)


if __name__ == "__main__":
    unittest.main()
