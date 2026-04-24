import importlib.util
import json
import pathlib
import sys
import tempfile
import textwrap
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "real_cli_matrix.py"
PUBLIC_EDITING_TOOL_WORKSPACE_FIXTURE_PATH = (
    REPO_ROOT
    / "scripts"
    / "fixtures"
    / "cli_matrix"
    / "smoke"
    / "public_editing_tool_workspace_edit_contract"
    / "task.json"
)


def load_module():
    spec = importlib.util.spec_from_file_location(
        "real_cli_matrix_public_editing_tool_workspace_edit_contract",
        SCRIPT_PATH,
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def make_context(module, client_name: str):
    return module.VerifierContext(client_name=client_name)


def make_fixture(module, verifier):
    return module.TaskFixture(
        fixture_id="public_editing_tool_workspace_edit_contract",
        kind="smoke",
        description="",
        prompt="Fix the regression and include the public editing tool name if available.",
        verifier=verifier,
        timeout_secs=90,
        workspace_template=pathlib.Path("/tmp/workspace"),
        supported_clients=("claude", "gemini"),
        unsupported_lanes=("qwen-local",),
    )


def write_buggy_workspace(workspace_dir: pathlib.Path) -> None:
    (workspace_dir / "calc.py").write_text(
        textwrap.dedent(
            """
            def add(a, b):
                return a - b


            def multiply(a, b):
                return a * b
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )
    (workspace_dir / "main.py").write_text(
        textwrap.dedent(
            """
            from calc import add, multiply

            print(f"2 + 3 = {add(2, 3)}")
            print(f"-1 + 5 = {add(-1, 5)}")
            print(f"0 + 0 = {add(0, 0)}")
            print(f"4 * 5 = {multiply(4, 5)}")
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )


def write_fixed_workspace(workspace_dir: pathlib.Path) -> None:
    write_buggy_workspace(workspace_dir)
    (workspace_dir / "calc.py").write_text(
        textwrap.dedent(
            """
            def add(a, b):
                return a + b


            def multiply(a, b):
                return a * b
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )


def public_editing_tool_workspace_edit_verifier():
    return {
        "type": "all_of",
        "verifiers": [
            {
                "type": "stdout_contract",
                "contains_any_by_client": {
                    "claude": ["Edit"],
                    "gemini": ["replace"],
                },
                "contains_any_by_client_match_mode": "used_tool_name_mention",
                "not_contains": ["__llmup_custom__"],
            },
            {
                "type": "python_source_and_output",
                "source": {
                    "path": "calc.py",
                    "function": "add",
                    "args": ["a", "b"],
                    "returns": {
                        "kind": "binary_op",
                        "operator": "+",
                        "left": "a",
                        "right": "b",
                    },
                },
                "entrypoint": {
                    "path": "main.py",
                    "expect_stdout_contains": [
                        "2 + 3 = 5",
                        "-1 + 5 = 4",
                        "0 + 0 = 0",
                        "4 * 5 = 20",
                    ],
                },
            },
        ],
    }


class RealClientEditContractTests(unittest.TestCase):
    def test_public_editing_tool_workspace_fixture_prompt_template_anchors_current_client_tool_name(
        self,
    ):
        module = load_module()
        [fixture] = module.load_fixtures(PUBLIC_EDITING_TOOL_WORKSPACE_FIXTURE_PATH.parent)

        rendered_prompt = module.render_fixture_prompt(fixture, "claude")

        self.assertEqual(fixture.fixture_id, "public_editing_tool_workspace_edit_contract")
        self.assertIsNotNone(fixture.prompt_template)
        self.assertIn("Current client: claude.", rendered_prompt)
        self.assertIn(
            "exact public editing tool name you actually used on this current client surface",
            rendered_prompt,
        )
        self.assertIn("Name only the actual public tool, not a task label.", rendered_prompt)
        self.assertIn(
            "Do not answer with task IDs, fixture IDs, contract names, workspace paths, or filenames.",
            rendered_prompt,
        )
        self.assertIn("Do not mention any other clients.", rendered_prompt)
        self.assertNotIn("{client_name}", rendered_prompt)

    def test_verify_fixture_output_all_of_requires_real_workspace_edit(self):
        module = load_module()
        fixture = make_fixture(module, public_editing_tool_workspace_edit_verifier())

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            write_buggy_workspace(workspace_dir)

            ok, message = module.verify_fixture_output(
                fixture,
                "The public editing tool used was `replace`.",
                workspace_dir,
                context=make_context(module, "gemini"),
            )

        self.assertFalse(ok)
        self.assertIn("calc.py:add", message)
        self.assertIn("return a + b", message)

    def test_verify_fixture_output_all_of_accepts_client_specific_public_tool_names_and_fix(self):
        module = load_module()
        fixture = make_fixture(module, public_editing_tool_workspace_edit_verifier())

        for client_name, stdout_text in (
            (
                "claude",
                "**Public editing tool used:** The `Edit` tool.",
            ),
            (
                "gemini",
                "**Tool used:** `Replace` (workspace file editing tool)",
            ),
            (
                "gemini",
                "The tool I used was `replace`.",
            ),
            (
                "gemini",
                "I fixed the regression, validated the workspace, and used `replace`.",
            ),
        ):
            with self.subTest(client_name=client_name):
                with tempfile.TemporaryDirectory() as temp_dir:
                    workspace_dir = pathlib.Path(temp_dir)
                    write_fixed_workspace(workspace_dir)

                    ok, message = module.verify_fixture_output(
                        fixture,
                        stdout_text,
                        workspace_dir,
                        context=make_context(module, client_name),
                    )

                self.assertTrue(ok, message)
                self.assertEqual(message, "")

    def test_public_editing_tool_workspace_fixture_declares_cross_client_contract(self):
        payload = json.loads(
            PUBLIC_EDITING_TOOL_WORKSPACE_FIXTURE_PATH.read_text(encoding="utf-8")
        )

        self.assertEqual(payload["id"], "public_editing_tool_workspace_edit_contract")
        self.assertEqual(payload["kind"], "smoke")
        self.assertEqual(payload["workspace_template"], "workspace")
        self.assertEqual(payload["unsupported_lanes"], ["qwen-local"])
        self.assertEqual(payload["verifier"]["type"], "all_of")
        self.assertEqual(
            [entry["type"] for entry in payload["verifier"]["verifiers"]],
            ["stdout_contract", "python_source_and_output"],
        )
        self.assertEqual(
            payload["verifier"]["verifiers"][0]["contains_any_by_client"],
            {
                "claude": ["Edit"],
                "gemini": ["replace"],
            },
        )
        self.assertEqual(
            payload["verifier"]["verifiers"][0]["contains_any_by_client_match_mode"],
            "used_tool_name_mention",
        )
        self.assertNotIn(
            "reject_other_client_contains_any_by_client",
            payload["verifier"]["verifiers"][0],
        )
        self.assertIn("{client_name}", payload["prompt_template"])
        self.assertIn(
            "exact public editing tool name you actually used on this current client surface",
            payload["prompt_template"],
        )
        self.assertIn(
            "Do not answer with task IDs, fixture IDs, contract names, workspace paths, or filenames.",
            payload["prompt_template"],
        )
        self.assertIn("Do not mention any other clients.", payload["prompt_template"])
        self.assertEqual(payload["supported_clients"], ["claude", "gemini"])
        self.assertIn("current client surface", payload["prompt"])
        self.assertIn("Name only the actual public tool, not a task label.", payload["prompt"])
        self.assertIn(
            "Do not answer with task IDs, fixture IDs, contract names, workspace paths, or filenames.",
            payload["prompt"],
        )

    def test_lane_supports_fixture_respects_fixture_unsupported_lanes(self):
        module = load_module()
        fixture = make_fixture(module, public_editing_tool_workspace_edit_verifier())
        qwen_lane = module.Lane(
            name="qwen-local",
            required=False,
            enabled=True,
            proxy_model="qwen-local",
            upstream_name="LOCAL-QWEN",
            skip_reason=None,
        )

        self.assertFalse(module.lane_supports_fixture(qwen_lane, fixture))

    def test_client_supports_fixture_excludes_codex_for_public_editing_tool_workspace_contract(self):
        module = load_module()
        fixture = make_fixture(module, public_editing_tool_workspace_edit_verifier())

        self.assertFalse(module.client_supports_fixture("codex", fixture))
        self.assertTrue(module.client_supports_fixture("claude", fixture))
        self.assertTrue(module.client_supports_fixture("gemini", fixture))

    def test_expand_matrix_excludes_codex_for_public_editing_tool_workspace_contract(self):
        module = load_module()
        fixture = make_fixture(module, public_editing_tool_workspace_edit_verifier())
        lane = module.Lane(
            name="minimax-openai",
            required=True,
            enabled=True,
            proxy_model="minimax-openai",
            upstream_name="MINIMAX-OPENAI",
            skip_reason=None,
        )

        cases = module.expand_matrix(
            clients=["codex", "claude", "gemini"],
            lanes=[lane],
            fixtures=[fixture],
            phase="basic",
            skip_slow=False,
        )

        self.assertEqual([case.client_name for case in cases], ["claude", "gemini"])


if __name__ == "__main__":
    unittest.main()
