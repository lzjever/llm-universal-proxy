import importlib.util
import json
import pathlib
import sys
import tempfile
import textwrap
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "real_cli_matrix.py"
APPLY_PATCH_EXECUTION_FIXTURE_PATH = (
    REPO_ROOT
    / "scripts"
    / "fixtures"
    / "cli_matrix"
    / "smoke"
    / "apply_patch_execution_contract"
    / "task.json"
)


def load_module():
    spec = importlib.util.spec_from_file_location(
        "real_cli_matrix_apply_patch_execution_contract",
        SCRIPT_PATH,
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def make_fixture(module, verifier):
    return module.TaskFixture(
        fixture_id="apply_patch_execution_contract",
        kind="smoke",
        description="",
        prompt="Fix the regression and name the public editing tool you used.",
        verifier=verifier,
        timeout_secs=90,
        workspace_template=pathlib.Path("/tmp/workspace"),
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


def apply_patch_execution_verifier():
    return {
        "type": "all_of",
        "verifiers": [
            {
                "type": "stdout_contract",
                "contains_any": ["apply_patch"],
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
    def test_verify_fixture_output_all_of_requires_real_workspace_edit(self):
        module = load_module()
        fixture = make_fixture(module, apply_patch_execution_verifier())

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            write_buggy_workspace(workspace_dir)

            ok, message = module.verify_fixture_output(
                fixture,
                "I used apply_patch and verified the result.",
                workspace_dir,
            )

        self.assertFalse(ok)
        self.assertIn("calc.py:add", message)
        self.assertIn("return a + b", message)

    def test_verify_fixture_output_all_of_accepts_public_apply_patch_and_fix(self):
        module = load_module()
        fixture = make_fixture(module, apply_patch_execution_verifier())

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            write_fixed_workspace(workspace_dir)

            ok, message = module.verify_fixture_output(
                fixture,
                "I used apply_patch and verified the result with main.py.",
                workspace_dir,
            )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_apply_patch_execution_fixture_declares_smoke_workspace_contract(self):
        payload = json.loads(
            APPLY_PATCH_EXECUTION_FIXTURE_PATH.read_text(encoding="utf-8")
        )

        self.assertEqual(payload["id"], "apply_patch_execution_contract")
        self.assertEqual(payload["kind"], "smoke")
        self.assertEqual(payload["workspace_template"], "workspace")
        self.assertEqual(payload["unsupported_lanes"], ["qwen-local"])
        self.assertEqual(payload["verifier"]["type"], "all_of")
        self.assertEqual(
            [entry["type"] for entry in payload["verifier"]["verifiers"]],
            ["stdout_contract", "python_source_and_output"],
        )

    def test_lane_supports_fixture_respects_fixture_unsupported_lanes(self):
        module = load_module()
        fixture = make_fixture(module, apply_patch_execution_verifier())
        qwen_lane = module.Lane(
            name="qwen-local",
            required=False,
            enabled=True,
            proxy_model="qwen-local",
            upstream_name="LOCAL-QWEN",
            skip_reason=None,
        )

        self.assertFalse(module.lane_supports_fixture(qwen_lane, fixture))


if __name__ == "__main__":
    unittest.main()
