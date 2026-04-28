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
    CodexModelMetadata,
    DEFAULT_CONFIG_SOURCE,
    DEFAULT_ENV_FILE,
    DEFAULT_PROXY_KEY,
    ModelLimits,
    PROXY_KEY_ENV,
    add_timeout_policy_args,
    build_client_env,
    build_codex_catalog_args,
    build_codex_proxy_provider_args,
    build_runtime_config_text,
    default_proxy_binary_path,
    ensure_no_public_internal_tool_artifacts,
    fetch_live_model_profile,
    load_dotenv_file,
    merge_preset_endpoint_env,
    parse_proxy_source,
    prepare_proxy_env,
    resolve_proxy_key,
    start_proxy,
    stop_proxy,
    timeout_policy_from_args,
    wait_for_health,
)


DEFAULT_MODEL_BY_CLIENT = {
    "codex": "preset-openai-compatible",
    "claude": "preset-anthropic-compatible",
    "gemini": "preset-openai-compatible",
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
    parser.add_argument("--binary", default=str(default_proxy_binary_path()))
    parser.add_argument("--proxy-host", default="127.0.0.1")
    parser.add_argument(
        "--dangerous-harness",
        action="store_true",
        help="allow client-specific no-sandbox or permission-bypass flags",
    )
    parser.add_argument(
        "--proxy-port",
        type=int,
        default=int(os.environ.get("PROXY_PORT", "18888")),
    )
    add_timeout_policy_args(parser, include_case_thresholds=False)
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
    *,
    client_home: pathlib.Path | None = None,
    model_limits: ModelLimits | None = None,
    codex_metadata: CodexModelMetadata | None = None,
    dangerous_harness: bool = False,
) -> list[str]:
    workspace = pathlib.Path(workspace).resolve()
    proxy_base = normalize_proxy_base(proxy_base)

    if client_name == "codex":
        command = [
            "codex",
            "-C",
            str(workspace),
            "-m",
            model,
        ]
        if dangerous_harness:
            command.append("--dangerously-bypass-approvals-and-sandbox")
        else:
            command.extend(["--sandbox", "workspace-write"])
        command.extend(build_codex_proxy_provider_args(proxy_base))
        command.extend(
            build_codex_catalog_args(
                client_home, model, model_limits, codex_metadata
            )
        )
        ensure_no_public_internal_tool_artifacts(
            command, context="interactive CLI command"
        )
        return command

    if client_name == "claude":
        command = [
            "claude",
            "--bare",
            "--setting-sources",
            "user",
            "--model",
            model,
        ]
        if dangerous_harness:
            command.append("--dangerously-skip-permissions")
        command.extend(["--add-dir", str(workspace)])
        ensure_no_public_internal_tool_artifacts(
            command, context="interactive CLI command"
        )
        return command

    if client_name == "gemini":
        command = [
            "gemini",
            "--model",
            model,
        ]
        if dangerous_harness:
            command.extend(["--sandbox=false", "--yolo"])
        command.extend(["--include-directories", str(workspace)])
        ensure_no_public_internal_tool_artifacts(
            command, context="interactive CLI command"
        )
        return command

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
) -> tuple[
    str,
    subprocess.Popen[str] | None,
    pathlib.Path | None,
    pathlib.Path | None,
]:
    config_source = pathlib.Path(args.config_source)
    dotenv_env = load_dotenv_file(pathlib.Path(args.env_file))
    proxy_binary = pathlib.Path(args.binary)
    ensure_proxy_binary(proxy_binary)

    parsed_source = parse_proxy_source(config_source.read_text(encoding="utf-8"))
    dotenv_env = merge_preset_endpoint_env(parsed_source, dotenv_env, base_env)
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
    proxy_env = prepare_proxy_env(base_env, dotenv_env, runtime_root)
    process, _config_path, stdout_path, stderr_path = start_proxy(
        proxy_binary,
        runtime_config_text,
        report_dir,
        proxy_env,
    )
    return (
        f"http://{args.proxy_host}:{args.proxy_port}",
        process,
        stdout_path,
        stderr_path,
    )


def run(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    timeout_policy = timeout_policy_from_args(args)
    ensure_client_binary(args.client)

    workspace = pathlib.Path(args.workspace).resolve()
    config_source = pathlib.Path(args.config_source)
    base_env = dict(os.environ)

    with tempfile.TemporaryDirectory(prefix=f"interactive-cli-{args.client}-") as temp_dir:
        runtime_root = pathlib.Path(temp_dir)
        proxy_process: subprocess.Popen[str] | None = None
        proxy_stdout_path: pathlib.Path | None = None
        proxy_stderr_path: pathlib.Path | None = None

        try:
            if args.proxy_base:
                proxy_base = normalize_proxy_base(args.proxy_base)
                proxy_key = resolve_proxy_key(base_env)
            else:
                (
                    proxy_base,
                    proxy_process,
                    proxy_stdout_path,
                    proxy_stderr_path,
                ) = start_managed_proxy(
                    args,
                    base_env,
                    runtime_root,
                )
                proxy_key = resolve_proxy_key(
                    base_env,
                    load_dotenv_file(pathlib.Path(args.env_file)),
                )

            if proxy_process is None:
                wait_for_health(
                    proxy_base,
                    timeout_secs=timeout_policy.proxy_health_timeout_secs,
                )
            else:
                wait_for_health(
                    proxy_base,
                    timeout_secs=timeout_policy.proxy_health_timeout_secs,
                    process=proxy_process,
                    stdout_path=proxy_stdout_path,
                    stderr_path=proxy_stderr_path,
                )
            live_profile = fetch_live_model_profile(
                proxy_base,
                args.model,
                proxy_key=proxy_key,
            )

            client_home = runtime_root / "homes" / args.client
            client_base_env = dict(base_env)
            client_base_env[PROXY_KEY_ENV] = proxy_key
            client_env = build_client_env(
                args.client,
                client_base_env,
                proxy_base,
                client_home,
                model_name=args.model,
                model_limits=live_profile.limits,
            )
            command = build_interactive_command(
                args.client,
                workspace,
                args.model,
                proxy_base,
                client_home=client_home,
                model_limits=live_profile.limits,
                codex_metadata=live_profile.codex_metadata,
                dangerous_harness=args.dangerous_harness,
            )
            return launch_interactive_client(command, workspace, client_env)
        finally:
            stop_proxy(
                proxy_process,
                terminate_grace_secs=timeout_policy.process_terminate_grace_secs,
            )


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
