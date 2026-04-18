#!/usr/bin/env python3
"""Launch Codex, Claude, or Gemini in interactive mode through the proxy."""

from __future__ import annotations

import argparse
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile


SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from real_cli_matrix import (  # noqa: E402
    CLIENT_NAMES,
    DEFAULT_CONFIG_SOURCE,
    DEFAULT_ENV_FILE,
    DEFAULT_PROXY_BINARY,
    build_client_env,
    build_runtime_config_text,
    load_dotenv_file,
    parse_proxy_source,
    prepare_proxy_env,
    start_proxy,
    stop_proxy,
    wait_for_health,
)


DEFAULT_MODEL_BY_CLIENT = {
    "codex": "minimax-openai",
    "claude": "claude-haiku-4-5",
    "gemini": "minimax-openai",
}


def normalize_proxy_base(proxy_base: str) -> str:
    return proxy_base.rstrip("/")


def default_model_for_client(client_name: str) -> str:
    try:
        return DEFAULT_MODEL_BY_CLIENT[client_name]
    except KeyError as error:
        raise ValueError(f"unknown client: {client_name}") from error


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Start an interactive CLI session through llm-universal-proxy"
    )
    parser.add_argument("--client", choices=CLIENT_NAMES, required=True)
    parser.add_argument("--model")
    parser.add_argument("--workspace", default=os.getcwd())
    parser.add_argument("--proxy-base")
    parser.add_argument("--config-source", default=str(DEFAULT_CONFIG_SOURCE))
    parser.add_argument("--env-file", default=str(DEFAULT_ENV_FILE))
    parser.add_argument("--binary", default=str(DEFAULT_PROXY_BINARY))
    parser.add_argument("--proxy-host", default="127.0.0.1")
    parser.add_argument(
        "--proxy-port",
        type=int,
        default=int(os.environ.get("PROXY_PORT", "18888")),
    )
    args = parser.parse_args(argv)
    if not args.model:
        args.model = default_model_for_client(args.client)
    return args


def ensure_client_binary(client_name: str) -> None:
    if shutil.which(client_name) is None:
        raise RuntimeError(f"missing prerequisite: {client_name}")


def ensure_proxy_binary(proxy_binary: pathlib.Path) -> None:
    if not proxy_binary.exists():
        raise RuntimeError(f"missing prerequisite: {proxy_binary}")


def build_interactive_command(
    client_name: str,
    workspace: pathlib.Path,
    model: str,
    proxy_base: str,
) -> list[str]:
    workspace = pathlib.Path(workspace).resolve()
    proxy_base = normalize_proxy_base(proxy_base)

    if client_name == "codex":
        return [
            "codex",
            "-C",
            str(workspace),
            "-m",
            model,
            "-c",
            'model_provider="proxy"',
            "-c",
            'model_providers.proxy.name="Proxy"',
            "-c",
            f'model_providers.proxy.base_url="{proxy_base}/openai/v1"',
            "-c",
            'model_providers.proxy.wire_api="responses"',
        ]

    if client_name == "claude":
        return [
            "claude",
            "--bare",
            "--setting-sources",
            "user",
            "--model",
            model,
            "--add-dir",
            str(workspace),
        ]

    if client_name == "gemini":
        return [
            "gemini",
            "--model",
            model,
            "--include-directories",
            str(workspace),
        ]

    raise ValueError(f"unknown client: {client_name}")


def launch_interactive_client(
    command: list[str],
    workspace: pathlib.Path,
    env: dict[str, str],
) -> int:
    process = subprocess.Popen(
        command,
        cwd=str(pathlib.Path(workspace).resolve()),
        env=env,
    )
    return int(process.wait())


def start_managed_proxy(
    args: argparse.Namespace,
    base_env: dict[str, str],
    runtime_root: pathlib.Path,
) -> tuple[str, subprocess.Popen[str] | None]:
    config_source = pathlib.Path(args.config_source)
    dotenv_env = load_dotenv_file(pathlib.Path(args.env_file))
    proxy_binary = pathlib.Path(args.binary)
    ensure_proxy_binary(proxy_binary)

    parsed_source = parse_proxy_source(config_source.read_text(encoding="utf-8"))
    report_dir = runtime_root / "proxy"
    report_dir.mkdir(parents=True, exist_ok=True)
    trace_path = report_dir / "debug-trace.jsonl"
    runtime_config_text = build_runtime_config_text(
        parsed_source,
        dotenv_env,
        listen_host=args.proxy_host,
        listen_port=args.proxy_port,
        trace_path=trace_path,
    )
    proxy_env = prepare_proxy_env(base_env, dotenv_env)
    process, _config_path, _stdout_path, _stderr_path = start_proxy(
        proxy_binary,
        runtime_config_text,
        report_dir,
        proxy_env,
    )
    return f"http://{args.proxy_host}:{args.proxy_port}", process


def run(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    ensure_client_binary(args.client)

    workspace = pathlib.Path(args.workspace).resolve()
    base_env = dict(os.environ)

    with tempfile.TemporaryDirectory(prefix=f"interactive-cli-{args.client}-") as temp_dir:
        runtime_root = pathlib.Path(temp_dir)
        proxy_process: subprocess.Popen[str] | None = None

        try:
            if args.proxy_base:
                proxy_base = normalize_proxy_base(args.proxy_base)
            else:
                proxy_base, proxy_process = start_managed_proxy(
                    args,
                    base_env,
                    runtime_root,
                )

            wait_for_health(proxy_base)

            client_home = runtime_root / "homes" / args.client
            client_env = build_client_env(
                args.client,
                base_env,
                proxy_base,
                client_home,
            )
            command = build_interactive_command(
                args.client,
                workspace,
                args.model,
                proxy_base,
            )
            return launch_interactive_client(command, workspace, client_env)
        finally:
            stop_proxy(proxy_process)


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
