#!/usr/bin/env python3
"""Launch Codex, Claude, or Gemini in interactive mode through the proxy."""

from __future__ import annotations

import argparse
import dataclasses
import os
import pathlib
import shutil
import subprocess
import sys


SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import real_cli_matrix as matrix


REPO_ROOT = matrix.REPO_ROOT
DEFAULT_STATE_ROOT = REPO_ROOT / "test-reports" / "interactive-cli"
DEFAULT_MODEL_BY_CLIENT = {
    "codex": "minimax-anth",
    "claude": "claude-sonnet-4-6",
    "gemini": "minimax-openai",
}


@dataclasses.dataclass
class ClientLaunch:
    client_name: str
    model: str
    proxy_base: str
    workspace_dir: pathlib.Path
    state_root: pathlib.Path
    home_dir: pathlib.Path
    env: dict[str, str]
    command: list[str]


def normalize_proxy_base(proxy_base: str) -> str:
    return proxy_base.rstrip("/")


def default_model_for_client(client_name: str) -> str:
    try:
        return DEFAULT_MODEL_BY_CLIENT[client_name]
    except KeyError as error:
        raise ValueError(f"unknown client: {client_name}") from error


def resolve_cli_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Start an interactive CLI session through llm-universal-proxy"
    )
    parser.add_argument("--client", choices=matrix.CLIENT_NAMES, required=True)
    parser.add_argument("--model", help="proxy model alias to use")
    parser.add_argument("--workspace", default=os.getcwd())
    parser.add_argument(
        "--proxy-base",
        help="reuse an already-running proxy, for example http://127.0.0.1:18888",
    )
    parser.add_argument("--config-source", default=str(matrix.DEFAULT_CONFIG_SOURCE))
    parser.add_argument("--env-file", default=str(matrix.DEFAULT_ENV_FILE))
    parser.add_argument("--state-root", default=str(DEFAULT_STATE_ROOT))
    parser.add_argument("--binary", default=str(matrix.DEFAULT_PROXY_BINARY))
    parser.add_argument("--proxy-host", default="127.0.0.1")
    parser.add_argument(
        "--proxy-port",
        type=int,
        default=int(os.environ.get("PROXY_PORT", "18888")),
    )
    parser.add_argument(
        "client_args",
        nargs=argparse.REMAINDER,
        help="extra args for the client; use -- before them",
    )
    args = parser.parse_args(argv)
    if args.client_args and args.client_args[0] == "--":
        args.client_args = args.client_args[1:]
    if not args.model:
        args.model = default_model_for_client(args.client)
    return args


def ensure_client_binary(client_name: str) -> None:
    if shutil.which(client_name) is None:
        raise RuntimeError(f"missing prerequisite: {client_name}")


def ensure_proxy_binary(proxy_binary: pathlib.Path) -> None:
    if not proxy_binary.exists():
        raise RuntimeError(f"missing prerequisite: {proxy_binary}")


def resolve_client_home_dir(client_name: str, state_root: pathlib.Path) -> pathlib.Path:
    return state_root / "homes" / client_name


def build_interactive_client_command(
    client_name: str,
    proxy_base: str,
    model: str,
    workspace_dir: pathlib.Path,
    extra_args: list[str] | None = None,
) -> list[str]:
    workspace_dir = pathlib.Path(workspace_dir).resolve()
    proxy_base = normalize_proxy_base(proxy_base)
    command: list[str]

    if client_name == "codex":
        command = [
            "codex",
            "--model",
            model,
            "-C",
            str(workspace_dir),
            "-c",
            'model_provider="proxy"',
            "-c",
            'model_providers.proxy.name="Proxy"',
            "-c",
            f'model_providers.proxy.base_url="{proxy_base}/openai/v1"',
            "-c",
            'model_providers.proxy.env_key="OPENAI_API_KEY"',
            "-c",
            'model_providers.proxy.wire_api="responses"',
            "-c",
            "model_providers.proxy.supports_websockets=false",
        ]
    elif client_name == "claude":
        command = [
            "claude",
            "--model",
            model,
            "--setting-sources",
            "user",
            "--bare",
            "--add-dir",
            str(workspace_dir),
        ]
    elif client_name == "gemini":
        command = [
            "gemini",
            "--model",
            model,
            "--sandbox=false",
            "--include-directories",
            str(workspace_dir),
        ]
    else:
        raise ValueError(f"unknown client: {client_name}")

    command.extend(extra_args or [])
    return command


def build_client_launch(
    client_name: str,
    model: str,
    proxy_base: str,
    workspace_dir: pathlib.Path,
    base_env: dict[str, str],
    state_root: pathlib.Path,
    extra_args: list[str] | None = None,
) -> ClientLaunch:
    state_root = pathlib.Path(state_root).resolve()
    workspace_dir = pathlib.Path(workspace_dir).resolve()
    home_dir = resolve_client_home_dir(client_name, state_root).resolve()
    env = matrix.build_client_env(
        client_name,
        base_env,
        normalize_proxy_base(proxy_base),
        home_dir,
    )
    command = build_interactive_client_command(
        client_name,
        proxy_base,
        model,
        workspace_dir,
        extra_args=extra_args,
    )
    return ClientLaunch(
        client_name=client_name,
        model=model,
        proxy_base=normalize_proxy_base(proxy_base),
        workspace_dir=workspace_dir,
        state_root=state_root,
        home_dir=home_dir,
        env=env,
        command=command,
    )


def launch_client_interactive(launch: ClientLaunch) -> int:
    completed = subprocess.run(
        launch.command,
        cwd=str(launch.workspace_dir),
        env=launch.env,
        check=False,
    )
    return int(completed.returncode)


def start_proxy_for_interactive_run(
    args: argparse.Namespace,
    base_env: dict[str, str],
) -> tuple[str, subprocess.Popen[str], pathlib.Path]:
    config_source = pathlib.Path(args.config_source)
    dotenv_env = matrix.load_dotenv_file(pathlib.Path(args.env_file))
    parsed_source = matrix.parse_proxy_source(config_source.read_text(encoding="utf-8"))

    runs_root = pathlib.Path(args.state_root).resolve() / "runs"
    run_dir = matrix.prepare_report_dir(runs_root)
    trace_path = run_dir / "debug-trace.jsonl"
    runtime_config_text = matrix.build_runtime_config_text(
        parsed_source,
        dotenv_env,
        listen_host=args.proxy_host,
        listen_port=args.proxy_port,
        trace_path=trace_path,
    )
    proxy_env = matrix.prepare_proxy_env(base_env, dotenv_env)
    proxy_binary = pathlib.Path(args.binary)

    ensure_proxy_binary(proxy_binary)
    process, _runtime_config_path, _stdout_path, _stderr_path = matrix.start_proxy(
        proxy_binary,
        runtime_config_text,
        run_dir,
        proxy_env,
    )
    proxy_base = f"http://{args.proxy_host}:{args.proxy_port}"
    return proxy_base, process, run_dir


def run(argv: list[str] | None = None) -> int:
    args = resolve_cli_args(argv)
    base_env = dict(os.environ)
    ensure_client_binary(args.client)

    proxy_process = None
    proxy_base = normalize_proxy_base(args.proxy_base) if args.proxy_base else ""

    try:
        if proxy_base:
            matrix.wait_for_health(proxy_base)
        else:
            proxy_base, proxy_process, _run_dir = start_proxy_for_interactive_run(
                args, base_env
            )
            matrix.wait_for_health(proxy_base)

        launch = build_client_launch(
            client_name=args.client,
            model=args.model,
            proxy_base=proxy_base,
            workspace_dir=pathlib.Path(args.workspace),
            base_env=base_env,
            state_root=pathlib.Path(args.state_root),
            extra_args=args.client_args,
        )
        return launch_client_interactive(launch)
    finally:
        matrix.stop_proxy(proxy_process)


def main() -> int:
    try:
        return run()
    except KeyboardInterrupt:
        return 130
    except Exception as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
