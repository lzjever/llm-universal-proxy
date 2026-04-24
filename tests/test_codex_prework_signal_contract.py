import importlib.util
import json
import pathlib
import sys
import tempfile
import textwrap
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "real_cli_matrix.py"
CODEX_PHASE_FIXTURE_PATH = (
    REPO_ROOT
    / "scripts"
    / "fixtures"
    / "cli_matrix"
    / "smoke"
    / "codex_prework_signal_work_summary_contract"
    / "task.json"
)


def load_module():
    spec = importlib.util.spec_from_file_location(
        "real_cli_matrix_codex_phase_contract",
        SCRIPT_PATH,
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def make_fixture(module, verifier):
    return module.TaskFixture(
        fixture_id="codex_prework_signal_work_summary_contract",
        kind="smoke",
        description="",
        prompt="Fix the regression and validate it.",
        verifier=verifier,
        timeout_secs=90,
        workspace_template=pathlib.Path("/tmp/workspace"),
        supported_clients=("codex",),
        unsupported_lanes=("qwen-local",),
    )


def write_fixed_workspace(workspace_dir: pathlib.Path) -> None:
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


def codex_phase_verifier():
    return {
        "type": "all_of",
        "verifiers": [
            {
                "type": "codex_json_event_contract",
                "event_types": ["turn.started", "turn.completed"],
                "phase_contract": {
                    "require_pre_work_signal": True,
                    "pre_work_item_types": ["reasoning", "agent_message"],
                    "require_post_work_agent_message": True,
                    "work_item_types": ["command_execution", "file_change"],
                },
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


def _jsonl(events):
    return "\n".join(json.dumps(event) for event in events) + "\n"


def codex_stdout_with_only_final_agent_message() -> str:
    return _jsonl(
        [
            {"type": "thread.started", "thread_id": "thread_1"},
            {"type": "turn.started"},
            {
                "type": "item.completed",
                "item": {
                    "id": "item_0",
                    "type": "agent_message",
                    "text": "Fixed and verified successfully.",
                },
            },
            {"type": "turn.completed", "usage": {"input_tokens": 1, "output_tokens": 1}},
        ]
    )


def codex_stdout_with_prework_reasoning_and_final_but_no_work() -> str:
    return _jsonl(
        [
            {"type": "thread.started", "thread_id": "thread_1"},
            {"type": "turn.started"},
            {
                "type": "item.completed",
                "item": {
                    "id": "item_0",
                    "type": "reasoning",
                    "text": "I found the bug and know what needs to change.",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_1",
                    "type": "agent_message",
                    "text": "Everything looks good now.",
                },
            },
            {"type": "turn.completed", "usage": {"input_tokens": 1, "output_tokens": 1}},
        ]
    )


def codex_stdout_with_prework_agent_message_work_and_final() -> str:
    return _jsonl(
        [
            {"type": "thread.started", "thread_id": "thread_1"},
            {"type": "turn.started"},
            {
                "type": "item.completed",
                "item": {
                    "id": "item_0",
                    "type": "agent_message",
                    "text": "I found the bug and will fix it.",
                },
            },
            {
                "type": "item.started",
                "item": {
                    "id": "item_1",
                    "type": "command_execution",
                    "command": "/usr/bin/zsh -lc \"sed -i 's/return a - b/return a + b/' calc.py\"",
                    "status": "in_progress",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_1",
                    "type": "command_execution",
                    "command": "/usr/bin/zsh -lc \"sed -i 's/return a - b/return a + b/' calc.py\"",
                    "aggregated_output": "",
                    "exit_code": 0,
                    "status": "completed",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_2",
                    "type": "agent_message",
                    "text": "I fixed the bug and verified the output with main.py.",
                },
            },
            {"type": "turn.completed", "usage": {"input_tokens": 1, "output_tokens": 1}},
        ]
    )


def codex_stdout_with_prework_reasoning_work_and_final() -> str:
    return _jsonl(
        [
            {"type": "thread.started", "thread_id": "thread_1"},
            {"type": "turn.started"},
            {
                "type": "item.completed",
                "item": {
                    "id": "item_0",
                    "type": "reasoning",
                    "text": "I found the bug and will fix it.",
                },
            },
            {
                "type": "item.started",
                "item": {
                    "id": "item_1",
                    "type": "command_execution",
                    "command": "/usr/bin/zsh -lc \"sed -i 's/return a - b/return a + b/' calc.py\"",
                    "status": "in_progress",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_1",
                    "type": "command_execution",
                    "command": "/usr/bin/zsh -lc \"sed -i 's/return a - b/return a + b/' calc.py\"",
                    "aggregated_output": "",
                    "exit_code": 0,
                    "status": "completed",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_2",
                    "type": "agent_message",
                    "text": "I fixed the bug and verified the output with main.py.",
                },
            },
            {"type": "turn.completed", "usage": {"input_tokens": 1, "output_tokens": 1}},
        ]
    )


def codex_stdout_with_work_and_final_but_no_prework_signal() -> str:
    return _jsonl(
        [
            {"type": "thread.started", "thread_id": "thread_1"},
            {"type": "turn.started"},
            {
                "type": "item.started",
                "item": {
                    "id": "item_1",
                    "type": "command_execution",
                    "command": "/usr/bin/zsh -lc \"sed -i 's/return a - b/return a + b/' calc.py\"",
                    "status": "in_progress",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_1",
                    "type": "command_execution",
                    "command": "/usr/bin/zsh -lc \"sed -i 's/return a - b/return a + b/' calc.py\"",
                    "aggregated_output": "",
                    "exit_code": 0,
                    "status": "completed",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_2",
                    "type": "agent_message",
                    "text": "I fixed the bug and verified the output with main.py.",
                },
            },
            {"type": "turn.completed", "usage": {"input_tokens": 1, "output_tokens": 1}},
        ]
    )


def codex_stdout_with_read_only_inspect_before_prework_then_mutating_work(
    read_only_command: str = "/usr/bin/cat calc.py main.py",
) -> str:
    return _jsonl(
        [
            {"type": "thread.started", "thread_id": "thread_1"},
            {"type": "turn.started"},
            {
                "type": "item.started",
                "item": {
                    "id": "item_0",
                    "type": "command_execution",
                    "command": read_only_command,
                    "aggregated_output": "",
                    "exit_code": None,
                    "status": "in_progress",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_0",
                    "type": "command_execution",
                    "command": read_only_command,
                    "aggregated_output": "def add(a, b):\n    return a - b\n",
                    "exit_code": 0,
                    "status": "completed",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_1",
                    "type": "reasoning",
                    "text": "The add implementation subtracts and needs to be changed.",
                },
            },
            {
                "type": "item.started",
                "item": {
                    "id": "item_2",
                    "type": "command_execution",
                    "command": "/usr/bin/zsh -lc \"sed -i 's/return a - b/return a + b/' calc.py\"",
                    "status": "in_progress",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_2",
                    "type": "command_execution",
                    "command": "/usr/bin/zsh -lc \"sed -i 's/return a - b/return a + b/' calc.py\"",
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
                    "text": "I fixed calc.py and verified the output with main.py.",
                },
            },
            {"type": "turn.completed", "usage": {"input_tokens": 1, "output_tokens": 1}},
        ]
    )


def codex_stdout_with_mutating_work_before_prework_reasoning() -> str:
    return _jsonl(
        [
            {"type": "thread.started", "thread_id": "thread_1"},
            {"type": "turn.started"},
            {
                "type": "item.started",
                "item": {
                    "id": "item_0",
                    "type": "command_execution",
                    "command": "/usr/bin/zsh -lc \"sed -i 's/return a - b/return a + b/' calc.py\"",
                    "status": "in_progress",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_0",
                    "type": "command_execution",
                    "command": "/usr/bin/zsh -lc \"sed -i 's/return a - b/return a + b/' calc.py\"",
                    "aggregated_output": "",
                    "exit_code": 0,
                    "status": "completed",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_1",
                    "type": "reasoning",
                    "text": "The add implementation has been corrected.",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_2",
                    "type": "agent_message",
                    "text": "I fixed calc.py and verified the output with main.py.",
                },
            },
            {"type": "turn.completed", "usage": {"input_tokens": 1, "output_tokens": 1}},
        ]
    )


class CodexPreworkSignalContractTests(unittest.TestCase):
    def test_verify_fixture_output_rejects_single_final_agent_message_without_prework_signal_or_work(self):
        module = load_module()
        fixture = make_fixture(module, codex_phase_verifier())

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            write_fixed_workspace(workspace_dir)

            ok, message = module.verify_fixture_output(
                fixture,
                codex_stdout_with_only_final_agent_message(),
                workspace_dir,
            )

        self.assertFalse(ok)
        self.assertIn("pre-work signal", message)

    def test_verify_fixture_output_rejects_missing_observable_work_between_messages(self):
        module = load_module()
        fixture = make_fixture(module, codex_phase_verifier())

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            write_fixed_workspace(workspace_dir)

            ok, message = module.verify_fixture_output(
                fixture,
                codex_stdout_with_prework_reasoning_and_final_but_no_work(),
                workspace_dir,
            )

        self.assertFalse(ok)
        self.assertIn("observable work", message)

    def test_verify_fixture_output_rejects_work_and_final_message_without_prework_signal(self):
        module = load_module()
        fixture = make_fixture(module, codex_phase_verifier())

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            write_fixed_workspace(workspace_dir)

            ok, message = module.verify_fixture_output(
                fixture,
                codex_stdout_with_work_and_final_but_no_prework_signal(),
                workspace_dir,
            )

        self.assertFalse(ok)
        self.assertIn("pre-work signal", message)

    def test_verify_fixture_output_accepts_read_only_inspect_before_prework_then_mutating_work(self):
        module = load_module()
        fixture = make_fixture(module, codex_phase_verifier())

        read_only_commands = [
            "/usr/bin/cat calc.py main.py",
            "/usr/bin/sed -n '1,120p' calc.py",
            "/usr/bin/sed --quiet '1,120p' calc.py",
            "/usr/bin/sed -n -e '1,80p' -e '81,160p' calc.py",
            "/usr/bin/sed -n --line-length=80 1p calc.py",
            "/usr/bin/head -n 20 calc.py",
            "/usr/bin/tail -n 20 calc.py",
            "/usr/bin/ls -la",
            "/usr/bin/find . -maxdepth 2 -type f",
            "/usr/bin/rg --no-config 'return a - b' .",
            "/usr/bin/rg --no-config 'return.*b' .",
            "/usr/bin/rg --no-config --max-count=5 'return' .",
            "/usr/bin/grep -R 'return a - b' .",
            "/usr/bin/grep -E 'return[[:space:]]+a' calc.py",
            "/usr/bin/grep -F 'return a + b' calc.py",
            "/usr/bin/python3 -I -S -c \"print(open('calc.py').read())\"",
            "/usr/bin/python3 -IS -c \"print(open('calc.py', mode='r').read())\"",
        ]
        for read_only_command in read_only_commands:
            with self.subTest(read_only_command=read_only_command):
                with tempfile.TemporaryDirectory() as temp_dir:
                    workspace_dir = pathlib.Path(temp_dir)
                    write_fixed_workspace(workspace_dir)

                    ok, message = module.verify_fixture_output(
                        fixture,
                        codex_stdout_with_read_only_inspect_before_prework_then_mutating_work(
                            read_only_command
                        ),
                        workspace_dir,
                    )

                self.assertTrue(ok, message)
                self.assertEqual(message, "")

    def test_read_only_inspect_rejects_empty_quoted_zsh_equals_expansion(self):
        module = load_module()

        dangerous_commands = [
            "/usr/bin/cat ''=sh",
            '/usr/bin/cat ""=sh',
        ]
        for dangerous_command in dangerous_commands:
            with self.subTest(dangerous_command=dangerous_command):
                self.assertFalse(
                    module._codex_command_execution_is_read_only_inspect(
                        {"command": dangerous_command}
                    )
                )

    def test_verify_fixture_output_rejects_dangerous_command_before_prework_reasoning(self):
        module = load_module()
        fixture = make_fixture(module, codex_phase_verifier())

        dangerous_commands = [
            "/usr/bin/zsh -lc 'rg --no-config -z needle .'",
            "/usr/bin/zsh -lc 'rg --no-config --search-zip needle .'",
            "/usr/bin/rg --no-config -z needle .",
            "/usr/bin/rg --no-config -nz needle .",
            "/usr/bin/rg --no-config --search-zip needle .",
            "/usr/bin/rg --no-config --unknown-read-mode needle .",
            "/usr/bin/rg --no-config needle =--pre=pre.sh .",
            "/usr/bin/rg --no-config needle #safe .",
            "/usr/bin/rg needle .",
            "/usr/bin/egrep needle f",
            "/usr/bin/fgrep needle f",
            "/usr/bin/zsh -lc 'cat calc.py'",
            "/bin/bash -c 'cat calc.py'",
            "cat calc.py",
            "python -c \"print(open('calc.py').read())\"",
            "/usr/bin/python3 -c \"print(open('calc.py').read())\"",
            "/usr/bin/python3 -I -c \"print(open('calc.py').read())\"",
            "/usr/bin/python3 -S -c \"print(open('calc.py').read())\"",
            "/usr/bin/python3 -I -S -c \"open('touched.txt', mode='w').write('x')\"",
            "/usr/bin/python3 -I -S -c \"import os; print(open('calc.py').read())\"",
            "/usr/bin/python3 -I -S -c \"getattr(open('calc.py'), 'read')()\"",
            "/usr/bin/zsh -lc \"sed -n '1w touched.txt' calc.py\"",
            "/usr/bin/sed -n '1w touched.txt' calc.py",
            "/usr/bin/zsh -lc \"sed -n 's/foo/bar/w touched.txt' calc.py\"",
            "/usr/bin/zsh -lc \"sed -n '1e touch touched.txt' calc.py\"",
            "/usr/bin/zsh -lc \"sed -ni 's/a/b/' calc.py\"",
            "/usr/bin/zsh -lc \"sed -nEi 's/a/b/' calc.py\"",
            "/usr/bin/zsh -lc 'sed -n --file script.sed calc.py'",
            "/usr/bin/sed -n 's/foo/bar/w touched.txt' calc.py",
            "/usr/bin/sed -n '1e touch touched.txt' calc.py",
            "/usr/bin/sed -ni 's/a/b/' calc.py",
            "/usr/bin/zsh -lc 'cat calc.py &> touched.txt'",
            "/usr/bin/cat calc.py > touched.txt",
            "/usr/bin/cat calc.py &| /usr/bin/touch touched.txt",
            "/usr/bin/zsh -lc 'cat calc.py >& touched.txt'",
            "/usr/bin/zsh -lc 'cat calc.py >>| touched.txt'",
            "/usr/bin/zsh -lc 'cat calc.py &>| touched.txt'",
            "/usr/bin/zsh -lc 'cat calc.py &>>| touched.txt'",
            "/usr/bin/zsh -lc 'cat calc.py >&| touched.txt'",
            "/usr/bin/zsh -lc 'cat calc.py >>& touched.txt'",
            "/usr/bin/zsh -lc \"cat calc.py > '&1'\"",
            "/usr/bin/zsh -lc 'cat calc.py >\\&1'",
            "/usr/bin/zsh -lc 'cat calc.py > &1'",
            "/usr/bin/zsh -lc 'cat calc.py >>&1'",
            "/usr/bin/zsh -lc 'cat calc.py >>&-'",
            "/usr/bin/zsh -lc \"sed -n '1p' `touch touched.txt` calc.py\"",
            "/usr/bin/zsh -lc 'cat $(touch touched.txt) calc.py'",
            "/usr/bin/zsh -lc 'cat $HOME calc.py'",
            "/usr/bin/zsh -lc 'X=-i; sed -n 1p $X calc.py'",
            "/usr/bin/zsh -lc 'X=-delete; find . $X'",
            "/usr/bin/zsh -lc 'X=-delete; find . $=X'",
            "/usr/bin/zsh -lc 'X=--pre=./pre.sh; rg $X needle .'",
            "/usr/bin/zsh -lc 'X=calc.py; cat ${X}'",
            "/usr/bin/zsh -lc 'set -- -i; sed -n 1p $1 calc.py'",
            "/usr/bin/zsh -lc 'set -- calc.py; cat $@'",
            "/usr/bin/zsh -lc 'set -- calc.py; cat $*'",
            "/usr/bin/zsh -lc 'cat $? calc.py'",
            "/usr/bin/zsh -lc 'cat $# calc.py'",
            "/usr/bin/zsh -lc 'cat $- calc.py'",
            "/usr/bin/zsh -lc 'find . -fprint touched.txt'",
            "/usr/bin/zsh -lc 'find . -fls touched.txt'",
            "/usr/bin/find . -delete",
            "/usr/bin/find . =-delete",
            "/usr/bin/find . -fprint touched.txt",
            "/usr/bin/find . -fls touched.txt",
            "/usr/bin/find . ~",
            "/usr/bin/zsh -lc 'rg --pre ./pre.sh pattern .'",
            "/usr/bin/rg --no-config --pre ./pre.sh pattern .",
            "/usr/bin/zsh -lc 'rg needle .'",
            "/usr/bin/sed -n '1p' ~ calc.py",
            "/usr/bin/sed -n 1p ^safe",
            "/usr/bin/rg --no-config ~ needle .",
            "/usr/bin/zsh -lc \"sed -n '1p' {-i,calc.py}\"",
            "/usr/bin/zsh -lc 'find . -{delete,print}'",
            "/usr/bin/zsh -lc 'rg --{pre=./pre.sh,files} needle .'",
            "python -c \"from pathlib import Path; print(Path('calc.py').read_text())\"",
            "python -X pycache_prefix=. -c \"print(open('calc.py', encoding='idna').read())\"",
            "python -X perf -c \"print(open('calc.py').read())\"",
            "/usr/bin/zsh -lc \"sed -n '1p' *\"",
            "/usr/bin/zsh -lc 'rg needle *'",
            "/usr/bin/zsh -lc \"python -c \\\"open('touched.txt', mode='w').write('x')\\\"\"",
            "/usr/bin/zsh -lc \"python -c \\\"import os; getattr(os, 'system')('touch touched.txt'); print(open('calc.py').read())\\\"\"",
            "/usr/bin/zsh -lc \"python -c \\\"from os import system as s; s('touch touched.txt'); print(open('calc.py').read())\\\"\"",
            "/usr/bin/zsh -lc \"python -c \\\"from pathlib import Path; getattr(Path('touched.txt'), 'write_text')('x'); print(Path('calc.py').read_text())\\\"\"",
            "/usr/bin/zsh -lc 'python mutate.py -c \"print(open(\\\"calc.py\\\").read())\"'",
            "/usr/bin/zsh -lc 'python -m mutate -c \"print(open(\\\"calc.py\\\").read())\"'",
            "/bin/bash --norc 'cat calc.py'",
            "/usr/bin/zsh --no-rcs 'cat calc.py'",
            "/usr/bin/zsh -oappendcreate pwd",
            "/usr/bin/zsh -oclobber pwd",
            "/usr/bin/zsh -lc 'cat calc.py\ntouch touched.txt'",
            "/usr/bin/zsh -lc 'cat calc.py & touch touched.txt'",
            "/usr/bin/zsh -lc 'RIPGREP_CONFIG_PATH=./rgrc rg needle .'",
            "bash -lc 'cat calc.py'",
            "zsh -lc 'cat calc.py'",
            "./zsh -lc 'cat calc.py'",
            "/tmp/bash -lc 'cat calc.py'",
            "/bin/bash -lc 'cat </dev/tcp/127.0.0.1/1'",
            "/bin/bash -lc \"python -c \\\"print(open('calc.py').read())\\\" </dev/tcp/127.0.0.1/1\"",
            "/bin/bash -lc 'cat < calc.py'",
            "/bin/bash -lc 'cat <<EOF'",
            "/bin/bash -lc 'cat <<< hello'",
            "/bin/bash -lc 'cat <&3'",
            "/usr/bin/zsh -lc './cat calc.py'",
            "/usr/bin/zsh -lc '/tmp/cat calc.py'",
            "/usr/bin/zsh -lc 'cat calc.py' && touch touched.txt",
            "touch touched.txt; /usr/bin/zsh -lc 'cat calc.py'",
        ]
        for dangerous_command in dangerous_commands:
            with self.subTest(dangerous_command=dangerous_command):
                with tempfile.TemporaryDirectory() as temp_dir:
                    workspace_dir = pathlib.Path(temp_dir)
                    write_fixed_workspace(workspace_dir)

                    ok, message = module.verify_fixture_output(
                        fixture,
                        codex_stdout_with_read_only_inspect_before_prework_then_mutating_work(
                            dangerous_command
                        ),
                        workspace_dir,
                    )

                self.assertFalse(ok)
                self.assertIn("pre-work signal", message)

    def test_verify_fixture_output_rejects_mutating_work_before_prework_reasoning(self):
        module = load_module()
        fixture = make_fixture(module, codex_phase_verifier())

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            write_fixed_workspace(workspace_dir)

            ok, message = module.verify_fixture_output(
                fixture,
                codex_stdout_with_mutating_work_before_prework_reasoning(),
                workspace_dir,
            )

        self.assertFalse(ok)
        self.assertIn("pre-work signal", message)

    def test_verify_fixture_output_accepts_prework_reasoning_work_and_final_message_sequence(self):
        module = load_module()
        fixture = make_fixture(module, codex_phase_verifier())

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            write_fixed_workspace(workspace_dir)

            ok, message = module.verify_fixture_output(
                fixture,
                codex_stdout_with_prework_reasoning_work_and_final(),
                workspace_dir,
            )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_verify_fixture_output_accepts_prework_agent_message_work_and_final_message_sequence(self):
        module = load_module()
        fixture = make_fixture(module, codex_phase_verifier())

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            write_fixed_workspace(workspace_dir)

            ok, message = module.verify_fixture_output(
                fixture,
                codex_stdout_with_prework_agent_message_work_and_final(),
                workspace_dir,
            )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_codex_phase_fixture_declares_codex_only_prework_signal_contract(self):
        payload = json.loads(CODEX_PHASE_FIXTURE_PATH.read_text(encoding="utf-8"))

        self.assertEqual(payload["id"], "codex_prework_signal_work_summary_contract")
        self.assertEqual(payload["supported_clients"], ["codex"])
        self.assertEqual(payload["workspace_template"], "../codex_observable_edit_contract/workspace")
        self.assertEqual(
            payload["verifier"]["verifiers"][0]["phase_contract"],
            {
                "require_pre_work_signal": True,
                "pre_work_item_types": ["reasoning", "agent_message"],
                "require_post_work_agent_message": True,
                "work_item_types": ["command_execution", "file_change"],
            },
        )


if __name__ == "__main__":
    unittest.main()
