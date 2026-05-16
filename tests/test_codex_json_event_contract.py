import importlib.util
import json
import pathlib
import sys
import tempfile
import textwrap
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "real_cli_matrix.py"
CODEX_OBSERVABLE_EDIT_FIXTURE_PATH = (
    REPO_ROOT
    / "scripts"
    / "fixtures"
    / "cli_matrix"
    / "smoke"
    / "codex_observable_edit_contract"
    / "task.json"
)


def load_module():
    spec = importlib.util.spec_from_file_location(
        "real_cli_matrix_codex_observable_edit_contract",
        SCRIPT_PATH,
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def make_fixture(module, verifier):
    return module.TaskFixture(
        fixture_id="codex_observable_edit_contract",
        kind="smoke",
        description="",
        prompt="Fix the regression and validate it.",
        verifier=verifier,
        timeout_secs=90,
        workspace_template=pathlib.Path("/tmp/workspace"),
        supported_clients=("codex",),
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


def codex_observable_edit_verifier():
    return {
        "type": "all_of",
        "verifiers": [
            {
                "type": "codex_json_event_contract",
                "event_types": ["turn.started", "turn.completed"],
                "work_summary_contract": {
                    "work_item_types": ["file_change", "command_execution"],
                },
                "completed_edit_targets": [
                    {
                        "path_suffix": "calc.py",
                        "kind": "update",
                    }
                ],
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


def codex_stdout_with_file_change() -> str:
    events = [
        {"type": "thread.started", "thread_id": "thread_1"},
        {"type": "turn.started"},
        {
            "type": "item.completed",
            "item": {"id": "item_1", "type": "agent_message", "text": "I'll fix the regression."},
        },
        {
            "type": "item.completed",
            "item": {
                "id": "item_2",
                "type": "file_change",
                "changes": [{"path": "/tmp/workspace/calc.py", "kind": "update"}],
                "status": "completed",
            },
        },
        {
            "type": "item.completed",
            "item": {"id": "item_3", "type": "agent_message", "text": "Validated with main.py."},
        },
        {"type": "turn.completed", "usage": {"input_tokens": 1, "output_tokens": 1}},
    ]
    return "\n".join(json.dumps(event) for event in events) + "\n"


def codex_stdout_false_positive_shell_write() -> str:
    events = [
        {"type": "thread.started", "thread_id": "thread_1"},
        {"type": "turn.started"},
        {
            "type": "item.completed",
            "item": {
                "id": "item_1",
                "type": "agent_message",
                "text": "I'll fix it using apply_patch.",
            },
        },
        {
            "type": "item.completed",
            "item": {
                "id": "item_2",
                "type": "command_execution",
                "command": "/usr/bin/zsh -lc \"cat > calc.py << 'EOF' ...\"",
                "aggregated_output": "",
                "exit_code": 0,
                "status": "completed",
            },
        },
        {
            "type": "item.completed",
            "item": {
                "id": "item_3",
                "type": "agent_message",
                "text": "The editing tool used was apply_patch.",
            },
        },
        {"type": "turn.completed", "usage": {"input_tokens": 1, "output_tokens": 1}},
    ]
    return "\n".join(json.dumps(event) for event in events) + "\n"


def codex_stdout_with_sed_edit() -> str:
    events = [
        {"type": "thread.started", "thread_id": "thread_1"},
        {"type": "turn.started"},
        {
            "type": "item.completed",
            "item": {"id": "item_1", "type": "agent_message", "text": "Found the bug."},
        },
        {
            "type": "item.completed",
            "item": {
                "id": "item_2",
                "type": "command_execution",
                "command": "/usr/bin/zsh -lc \"sed -i 's/return a - b/return a + b/' calc.py && cat calc.py\"",
                "aggregated_output": "def add(a, b):\n    return a + b\n",
                "exit_code": 0,
                "status": "completed",
            },
        },
        {
            "type": "item.completed",
            "item": {
                "id": "item_3",
                "type": "agent_message",
                "text": "I fixed calc.py and verified the output with main.py.",
            },
        },
        {"type": "turn.completed", "usage": {"input_tokens": 1, "output_tokens": 1}},
    ]
    return "\n".join(json.dumps(event) for event in events) + "\n"


def codex_stdout_with_shell_write_edit() -> str:
    events = [
        {"type": "thread.started", "thread_id": "thread_1"},
        {"type": "turn.started"},
        {
            "type": "item.completed",
            "item": {"id": "item_1", "type": "agent_message", "text": "Writing the fixed file."},
        },
        {
            "type": "item.completed",
            "item": {
                "id": "item_2",
                "type": "command_execution",
                "command": "/usr/bin/zsh -lc \"cat > calc.py << 'EOF'\ndef add(a, b):\n    return a + b\nEOF\"",
                "aggregated_output": "",
                "exit_code": 0,
                "status": "completed",
            },
        },
        {
            "type": "item.completed",
            "item": {
                "id": "item_3",
                "type": "agent_message",
                "text": "I rewrote calc.py and verified the result.",
            },
        },
        {"type": "turn.completed", "usage": {"input_tokens": 1, "output_tokens": 1}},
    ]
    return "\n".join(json.dumps(event) for event in events) + "\n"


def codex_stdout_with_completed_work_but_no_post_work_summary() -> str:
    events = [
        {"type": "thread.started", "thread_id": "thread_1"},
        {"type": "turn.started"},
        {
            "type": "item.completed",
            "item": {"id": "item_1", "type": "agent_message", "text": "I'll fix the regression."},
        },
        {
            "type": "item.completed",
            "item": {
                "id": "item_2",
                "type": "file_change",
                "changes": [{"path": "/tmp/workspace/calc.py", "kind": "update"}],
                "status": "completed",
            },
        },
        {"type": "turn.completed", "usage": {"input_tokens": 1, "output_tokens": 1}},
    ]
    return "\n".join(json.dumps(event) for event in events) + "\n"


def codex_stdout_with_read_only_commands() -> str:
    events = [
        {"type": "thread.started", "thread_id": "thread_1"},
        {"type": "turn.started"},
        {
            "type": "item.completed",
            "item": {
                "id": "item_1",
                "type": "agent_message",
                "text": "I'll fix it now.",
            },
        },
        {
            "type": "item.completed",
            "item": {
                "id": "item_2",
                "type": "command_execution",
                "command": "/usr/bin/zsh -lc 'cat calc.py main.py'",
                "aggregated_output": "def add(a, b):\n    return a + b\n",
                "exit_code": 0,
                "status": "completed",
            },
        },
        {
            "type": "item.completed",
            "item": {
                "id": "item_3",
                "type": "command_execution",
                "command": "/usr/bin/zsh -lc 'python main.py'",
                "aggregated_output": "2 + 3 = 5\n",
                "exit_code": 0,
                "status": "completed",
            },
        },
        {
            "type": "item.completed",
            "item": {
                "id": "item_4",
                "type": "agent_message",
                "text": "The editing tool used was apply_patch.",
            },
        },
        {"type": "turn.completed", "usage": {"input_tokens": 1, "output_tokens": 1}},
    ]
    return "\n".join(json.dumps(event) for event in events) + "\n"


class CodexJsonEventContractTests(unittest.TestCase):
    def test_verify_fixture_output_accepts_codex_file_change_event_and_workspace_fix(self):
        module = load_module()
        fixture = make_fixture(module, codex_observable_edit_verifier())

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            write_fixed_workspace(workspace_dir)

            ok, message = module.verify_fixture_output(
                fixture,
                codex_stdout_with_file_change(),
                workspace_dir,
            )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_verify_fixture_output_accepts_codex_sed_edit_command_and_workspace_fix(self):
        module = load_module()
        fixture = make_fixture(module, codex_observable_edit_verifier())

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            write_fixed_workspace(workspace_dir)

            ok, message = module.verify_fixture_output(
                fixture,
                codex_stdout_with_sed_edit(),
                workspace_dir,
            )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_verify_fixture_output_accepts_codex_shell_write_edit_command_and_workspace_fix(self):
        module = load_module()
        fixture = make_fixture(module, codex_observable_edit_verifier())

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            write_fixed_workspace(workspace_dir)

            ok, message = module.verify_fixture_output(
                fixture,
                codex_stdout_with_shell_write_edit(),
                workspace_dir,
            )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_verify_fixture_output_rejects_completed_work_without_post_work_summary(self):
        module = load_module()
        fixture = make_fixture(module, codex_observable_edit_verifier())

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            write_fixed_workspace(workspace_dir)

            ok, message = module.verify_fixture_output(
                fixture,
                codex_stdout_with_completed_work_but_no_post_work_summary(),
                workspace_dir,
            )

        self.assertFalse(ok)
        self.assertIn("post-work final agent_message", message)

    def test_verify_fixture_output_rejects_read_only_command_execution_false_positive(self):
        module = load_module()
        fixture = make_fixture(module, codex_observable_edit_verifier())

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            write_fixed_workspace(workspace_dir)

            ok, message = module.verify_fixture_output(
                fixture,
                codex_stdout_with_read_only_commands(),
                workspace_dir,
            )

        self.assertFalse(ok)
        self.assertIn("observable edit signal", message)

    def test_verify_fixture_output_rejects_invalid_codex_json_event_stream(self):
        module = load_module()
        fixture = make_fixture(module, codex_observable_edit_verifier())

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            write_fixed_workspace(workspace_dir)

            ok, message = module.verify_fixture_output(
                fixture,
                "not jsonl\n",
                workspace_dir,
            )

        self.assertFalse(ok)
        self.assertIn("JSONL event", message)

    def test_codex_observable_edit_fixture_declares_codex_only_event_contract(self):
        payload = json.loads(CODEX_OBSERVABLE_EDIT_FIXTURE_PATH.read_text(encoding="utf-8"))

        self.assertEqual(payload["id"], "codex_observable_edit_contract")
        self.assertEqual(payload["kind"], "smoke")
        self.assertEqual(payload["workspace_template"], "workspace")
        self.assertEqual(payload["supported_clients"], ["codex"])
        self.assertEqual(payload["unsupported_lanes"], ["qwen-local"])
        self.assertEqual(
            [entry["type"] for entry in payload["verifier"]["verifiers"]],
            ["codex_json_event_contract", "python_source_and_output"],
        )
        self.assertEqual(
            payload["verifier"]["verifiers"][0]["work_summary_contract"],
            {"work_item_types": ["file_change", "command_execution"]},
        )
        self.assertEqual(
            payload["verifier"]["verifiers"][0]["completed_edit_targets"],
            [{"path_suffix": "calc.py", "kind": "update"}],
        )

    def test_expand_matrix_filters_codex_only_fixture_from_other_clients(self):
        module = load_module()
        fixture = make_fixture(module, codex_observable_edit_verifier())
        lane = module.Lane(
            name="minimax-anth",
            required=True,
            enabled=True,
            proxy_model="minimax-anth",
            upstream_name="MINIMAX-ANTHROPIC",
            skip_reason=None,
        )

        cases = module.expand_matrix(
            clients=["codex", "claude"],
            lanes=[lane],
            fixtures=[fixture],
            phase="basic",
            skip_slow=False,
        )

        self.assertEqual([case.client_name for case in cases], ["codex"])
        self.assertEqual(
            [case.case_id for case in cases],
            ["codex__minimax-anth__codex_observable_edit_contract"],
        )


if __name__ == "__main__":
    unittest.main()
