#!/usr/bin/env python3
"""Real CLI matrix harness for Codex, Claude Code, and Gemini CLI."""

from __future__ import annotations

import argparse
import ast
import collections
import dataclasses
import json
import os
import pathlib
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from typing import Iterable


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_CONFIG_SOURCE = REPO_ROOT / "proxy-test-minimax-and-local.yaml"
DEFAULT_ENV_FILE = REPO_ROOT / ".env.test"
DEFAULT_FIXTURES_ROOT = REPO_ROOT / "scripts" / "fixtures" / "cli_matrix"
DEFAULT_REPORTS_ROOT = REPO_ROOT / "test-reports" / "cli-matrix"
DEFAULT_PROXY_BINARY = REPO_ROOT / "target" / "release" / "llm-universal-proxy"
VALID_PHASES = {
    "all",
    "basic",
    "multi",
    "codex",
    "codex_basic",
    "codex_multi",
    "claude",
    "claude_basic",
    "claude_multi",
    "gemini",
    "gemini_basic",
    "gemini_multi",
}
CLIENT_NAMES = ("codex", "claude", "gemini")
SAFE_ENV_KEYS = (
    "PATH",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "SYSTEMROOT",
    "TMP",
    "TEMP",
    "TMPDIR",
    "PATHEXT",
    "COMSPEC",
    "WINDIR",
)
GEMINI_BOOTSTRAP_TIMEOUT_SECS = 180
GEMINI_RUNNER_STATE_DIRNAME = "_runner_state"
GEMINI_SHARED_HOME_DIRNAME = "gemini-home"
GEMINI_BOOTSTRAP_MARKER = ".runner-gemini-bootstrap-ready"


@dataclasses.dataclass
class SourceConfigSection:
    key: str | None
    raw_lines: tuple[str, ...]


@dataclasses.dataclass
class ProxySourceConfig:
    listen: str
    upstream_timeout_secs: int | None
    upstreams: collections.OrderedDict[str, collections.OrderedDict[str, object]]
    model_aliases: collections.OrderedDict[str, str]
    debug_trace: collections.OrderedDict[str, object]
    top_level_sections: tuple[SourceConfigSection, ...]
    raw_text: str


@dataclasses.dataclass
class Lane:
    name: str
    required: bool
    enabled: bool
    proxy_model: str
    upstream_name: str
    skip_reason: str | None
    upstream_model: str | None = None


@dataclasses.dataclass
class TaskFixture:
    fixture_id: str
    kind: str
    prompt: str
    verifier: dict[str, object]
    timeout_secs: int
    workspace_template: pathlib.Path | None
    description: str = ""


@dataclasses.dataclass
class MatrixCase:
    client_name: str
    lane: Lane
    fixture: TaskFixture
    case_id: str


def parse_dotenv_exports(text: str) -> dict[str, str]:
    values: dict[str, str] = {}
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("export "):
            line = line[len("export ") :].strip()
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        value = value.strip()
        if not key:
            continue
        if value and value[0] == value[-1] and value[0] in ("'", '"'):
            value = value[1:-1]
        values[key] = value
    return values


def load_dotenv_file(path: pathlib.Path) -> dict[str, str]:
    if not path.exists():
        return {}
    return parse_dotenv_exports(path.read_text(encoding="utf-8"))


def parse_scalar(value: str) -> object:
    value = value.strip()
    if value.startswith('"') and value.endswith('"'):
        return json.loads(value)
    if value.startswith("'") and value.endswith("'"):
        return value[1:-1]
    if re.fullmatch(r"-?\d+", value):
        return int(value)
    if value in {"true", "false"}:
        return value == "true"
    return value


def _top_level_section_key(raw_line: str) -> str | None:
    stripped = raw_line.strip()
    if not stripped or raw_line.startswith(" ") or stripped.startswith("#"):
        return None
    key, separator, _rest = stripped.partition(":")
    if not separator:
        return None
    return key.strip() or None


def _split_top_level_sections(text: str) -> tuple[SourceConfigSection, ...]:
    sections: list[SourceConfigSection] = []
    current_key: str | None = None
    current_lines: list[str] = []

    for raw_line in text.splitlines():
        section_key = _top_level_section_key(raw_line)
        if section_key is not None:
            if current_lines:
                sections.append(
                    SourceConfigSection(current_key, tuple(current_lines))
                )
            current_key = section_key
            current_lines = [raw_line]
            continue
        current_lines.append(raw_line)

    if current_lines:
        sections.append(SourceConfigSection(current_key, tuple(current_lines)))
    return tuple(sections)


def parse_proxy_source(text: str) -> ProxySourceConfig:
    listen = ""
    upstream_timeout_secs = None
    upstreams: collections.OrderedDict[str, collections.OrderedDict[str, object]] = (
        collections.OrderedDict()
    )
    model_aliases: collections.OrderedDict[str, str] = collections.OrderedDict()
    debug_trace: collections.OrderedDict[str, object] = collections.OrderedDict()

    section: str | None = None
    current_upstream: str | None = None

    for raw_line in text.splitlines():
        line = raw_line.split("#", 1)[0].rstrip()
        if not line.strip():
            continue
        indent = len(line) - len(line.lstrip(" "))
        stripped = line.strip()
        if indent == 0:
            current_upstream = None
            if stripped == "upstreams:":
                section = "upstreams"
                continue
            if stripped == "model_aliases:":
                section = "model_aliases"
                continue
            if stripped == "debug_trace:":
                section = "debug_trace"
                continue
            section = None
            key, value = stripped.split(":", 1)
            parsed_value = parse_scalar(value)
            if key == "listen":
                listen = str(parsed_value)
            elif key == "upstream_timeout_secs":
                upstream_timeout_secs = int(parsed_value)
            continue

        if section == "upstreams":
            if indent == 2 and stripped.endswith(":"):
                current_upstream = stripped[:-1]
                upstreams[current_upstream] = collections.OrderedDict()
                continue
            if indent >= 4 and current_upstream is not None:
                key, value = stripped.split(":", 1)
                upstreams[current_upstream][key] = parse_scalar(value)
                continue

        if section == "model_aliases" and indent == 2:
            key, value = stripped.split(":", 1)
            model_aliases[key] = str(parse_scalar(value))
            continue

        if section == "debug_trace" and indent == 2:
            key, value = stripped.split(":", 1)
            debug_trace[key] = parse_scalar(value)

    return ProxySourceConfig(
        listen=listen,
        upstream_timeout_secs=upstream_timeout_secs,
        upstreams=upstreams,
        model_aliases=model_aliases,
        debug_trace=debug_trace,
        top_level_sections=_split_top_level_sections(text),
        raw_text=text,
    )


def has_local_qwen(dotenv_env: dict[str, str]) -> bool:
    return bool(
        dotenv_env.get("LOCAL_QWEN_BASE_URL") and dotenv_env.get("LOCAL_QWEN_MODEL")
    )


def resolve_lanes(
    config: ProxySourceConfig, dotenv_env: dict[str, str]
) -> list[Lane]:
    lane_specs = (
        ("minimax-anth", True, "MINIMAX-ANTHROPIC"),
        ("minimax-openai", True, "MINIMAX-OPENAI"),
        ("qwen-local", False, "LOCAL-QWEN"),
    )
    lanes: list[Lane] = []
    for lane_name, required, default_upstream in lane_specs:
        alias_value = config.model_aliases.get(lane_name)
        if alias_value is None and lane_name == "qwen-local" and has_local_qwen(dotenv_env):
            alias_value = f"LOCAL-QWEN:{dotenv_env['LOCAL_QWEN_MODEL']}"
        upstream_name = default_upstream
        upstream_model = None
        if alias_value and ":" in alias_value:
            upstream_name, upstream_model = alias_value.split(":", 1)

        enabled = upstream_name in config.upstreams
        skip_reason = None

        if lane_name == "qwen-local" and has_local_qwen(dotenv_env):
            enabled = True
            upstream_name = "LOCAL-QWEN"
            upstream_model = dotenv_env["LOCAL_QWEN_MODEL"]

        if not enabled:
            if lane_name == "qwen-local":
                skip_reason = (
                    "LOCAL_QWEN_BASE_URL and LOCAL_QWEN_MODEL are not both configured; "
                    "optional qwen-local lane will be skipped"
                )
            else:
                skip_reason = f"missing required routing for lane {lane_name}"

        lanes.append(
            Lane(
                name=lane_name,
                required=required,
                enabled=enabled,
                proxy_model=lane_name,
                upstream_name=upstream_name,
                upstream_model=upstream_model,
                skip_reason=skip_reason,
            )
        )
    return lanes


def _runtime_upstreams(
    config: ProxySourceConfig, dotenv_env: dict[str, str]
) -> collections.OrderedDict[str, collections.OrderedDict[str, object]]:
    upstreams = collections.OrderedDict(
        (name, collections.OrderedDict(values))
        for name, values in config.upstreams.items()
    )
    if has_local_qwen(dotenv_env):
        upstreams["LOCAL-QWEN"] = collections.OrderedDict(
            [
                ("api_root", dotenv_env["LOCAL_QWEN_BASE_URL"]),
                ("format", "openai-completion"),
                ("credential_actual", dotenv_env.get("LOCAL_QWEN_API_KEY", "not-needed")),
                ("auth_policy", "force_server"),
            ]
        )
    return upstreams


def _runtime_aliases(
    config: ProxySourceConfig, dotenv_env: dict[str, str]
) -> collections.OrderedDict[str, str]:
    aliases = collections.OrderedDict()
    qwen_enabled = has_local_qwen(dotenv_env)
    qwen_model = dotenv_env.get("LOCAL_QWEN_MODEL", "")

    for alias_name, target in config.model_aliases.items():
        if target.startswith("LOCAL-QWEN:"):
            if not qwen_enabled:
                continue
            aliases[alias_name] = f"LOCAL-QWEN:{qwen_model}"
            continue
        aliases[alias_name] = target

    if qwen_enabled:
        aliases["qwen-local"] = f"LOCAL-QWEN:{qwen_model}"

    return aliases


def render_scalar(value: object) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, str):
        if re.fullmatch(r"[A-Za-z0-9_./:-]+", value):
            return value
        return json.dumps(value)
    return json.dumps(value)


def _render_runtime_listen_section(listen_host: str, listen_port: int) -> list[str]:
    return [f"listen: {listen_host}:{listen_port}"]


def _render_runtime_timeout_section(config: ProxySourceConfig) -> list[str]:
    if config.upstream_timeout_secs is None:
        return []
    return [f"upstream_timeout_secs: {config.upstream_timeout_secs}"]


def _render_runtime_upstreams_section(
    config: ProxySourceConfig, dotenv_env: dict[str, str]
) -> list[str]:
    lines = ["upstreams:"]
    for upstream_name, values in _runtime_upstreams(config, dotenv_env).items():
        lines.append(f"  {upstream_name}:")
        for key, value in values.items():
            lines.append(f"    {key}: {render_scalar(value)}")
    return lines


def _render_runtime_aliases_section(
    config: ProxySourceConfig, dotenv_env: dict[str, str]
) -> list[str]:
    lines = ["model_aliases:"]
    for alias_name, target in _runtime_aliases(config, dotenv_env).items():
        lines.append(f"  {alias_name}: {json.dumps(target)}")
    return lines


def _render_runtime_debug_trace_section(
    config: ProxySourceConfig, trace_path: pathlib.Path
) -> list[str]:
    debug_trace = collections.OrderedDict(config.debug_trace.items())
    debug_trace["path"] = str(trace_path)
    lines = ["debug_trace:"]
    for key, value in debug_trace.items():
        lines.append(f"  {key}: {render_scalar(value)}")
    return lines


def _section_suffix_lines(raw_lines: tuple[str, ...]) -> list[str]:
    suffix: list[str] = []
    for raw_line in reversed(raw_lines):
        stripped = raw_line.strip()
        if not stripped:
            suffix.append(raw_line)
            continue
        if not raw_line.startswith(" ") and stripped.startswith("#"):
            suffix.append(raw_line)
            continue
        break
    return list(reversed(suffix))


def build_runtime_config_text(
    config: ProxySourceConfig,
    dotenv_env: dict[str, str],
    listen_host: str,
    listen_port: int,
    trace_path: pathlib.Path,
) -> str:
    section_renderers = collections.OrderedDict(
        [
            ("listen", lambda: _render_runtime_listen_section(listen_host, listen_port)),
            ("upstream_timeout_secs", lambda: _render_runtime_timeout_section(config)),
            ("upstreams", lambda: _render_runtime_upstreams_section(config, dotenv_env)),
            ("model_aliases", lambda: _render_runtime_aliases_section(config, dotenv_env)),
            ("debug_trace", lambda: _render_runtime_debug_trace_section(config, trace_path)),
        ]
    )
    rendered_keys: set[str] = set()
    lines: list[str] = []

    for section in config.top_level_sections:
        if section.key is None:
            lines.extend(section.raw_lines)
            continue
        renderer = section_renderers.get(section.key)
        if renderer is None:
            lines.extend(section.raw_lines)
            continue
        replacement_lines = renderer()
        lines.extend(replacement_lines)
        lines.extend(_section_suffix_lines(section.raw_lines))
        rendered_keys.add(section.key)

    for section_key, renderer in section_renderers.items():
        if section_key in rendered_keys:
            continue
        replacement_lines = renderer()
        if not replacement_lines:
            continue
        if lines and lines[-1].strip():
            lines.append("")
        lines.extend(replacement_lines)

    return "\n".join(lines) + "\n"


def load_fixtures(fixtures_root: pathlib.Path) -> list[TaskFixture]:
    fixtures: list[TaskFixture] = []
    for path in sorted(fixtures_root.rglob("*.json")):
        payload = json.loads(path.read_text(encoding="utf-8"))
        workspace_template = payload.get("workspace_template")
        fixtures.append(
            TaskFixture(
                fixture_id=payload["id"],
                kind=payload["kind"],
                description=payload.get("description", ""),
                prompt=payload["prompt"],
                verifier=payload["verifier"],
                timeout_secs=int(payload["timeout_secs"]),
                workspace_template=(
                    (path.parent / workspace_template).resolve()
                    if workspace_template
                    else None
                ),
            )
        )
    return fixtures


def phase_matches(client_name: str, fixture_kind: str, phase: str) -> bool:
    if phase == "all":
        return True
    if phase == "basic":
        return fixture_kind == "smoke"
    if phase == "multi":
        return fixture_kind == "long_horizon"
    if phase == client_name:
        return True
    if phase == f"{client_name}_basic":
        return fixture_kind == "smoke"
    if phase == f"{client_name}_multi":
        return fixture_kind == "long_horizon"
    return False


def lane_supports_fixture(lane: Lane, fixture: TaskFixture) -> bool:
    if lane.name == "qwen-local" and fixture.kind == "long_horizon":
        return False
    return True


def expand_matrix(
    clients: Iterable[str],
    lanes: Iterable[Lane],
    fixtures: Iterable[TaskFixture],
    phase: str,
    skip_slow: bool,
) -> list[MatrixCase]:
    cases: list[MatrixCase] = []
    for client_name in clients:
        for lane in lanes:
            for fixture in fixtures:
                if skip_slow and fixture.kind != "smoke":
                    continue
                if not phase_matches(client_name, fixture.kind, phase):
                    continue
                if not lane_supports_fixture(lane, fixture):
                    continue
                case_id = f"{client_name}__{lane.name}__{fixture.fixture_id}"
                cases.append(
                    MatrixCase(
                        client_name=client_name,
                        lane=lane,
                        fixture=fixture,
                        case_id=case_id,
                    )
                )
    return cases


def filter_matrix_cases(
    cases: Iterable[MatrixCase], selected_case_ids: Iterable[str] | None = None
) -> list[MatrixCase]:
    case_list = list(cases)
    wanted = [case_id for case_id in (selected_case_ids or []) if case_id]
    if not wanted:
        return case_list

    known_ids = {case.case_id for case in case_list}
    unknown = [case_id for case_id in wanted if case_id not in known_ids]
    if unknown:
        raise ValueError("unknown matrix case: " + ", ".join(unknown))

    wanted_set = set(wanted)
    return [case for case in case_list if case.case_id in wanted_set]


def classify_lane_health(lane: Lane, probe_error: str | None) -> tuple[str, str | None]:
    if not lane.enabled:
        status = "failed" if lane.required else "skipped"
        return status, lane.skip_reason
    if probe_error is None:
        return "ready", None
    return ("failed", probe_error) if lane.required else ("skipped", probe_error)


def _safe_base_env(base_env: dict[str, str]) -> dict[str, str]:
    return {key: base_env[key] for key in SAFE_ENV_KEYS if key in base_env}


def build_client_env(
    client_name: str, base_env: dict[str, str], proxy_base: str, home_dir: pathlib.Path
) -> dict[str, str]:
    home_dir = pathlib.Path(home_dir)
    xdg_config = home_dir / ".config"
    xdg_cache = home_dir / ".cache"
    xdg_data = home_dir / ".local" / "share"
    xdg_state = home_dir / ".local" / "state"
    temp_dir = home_dir / ".tmp"
    for path in (home_dir, xdg_config, xdg_cache, xdg_data, xdg_state, temp_dir):
        path.mkdir(parents=True, exist_ok=True)

    env = _safe_base_env(base_env)
    env.update(
        {
            "HOME": str(home_dir),
            "XDG_CONFIG_HOME": str(xdg_config),
            "XDG_CACHE_HOME": str(xdg_cache),
            "XDG_DATA_HOME": str(xdg_data),
            "XDG_STATE_HOME": str(xdg_state),
            "TMPDIR": str(temp_dir),
            "HTTP_PROXY": "",
            "HTTPS_PROXY": "",
            "http_proxy": "",
            "https_proxy": "",
            "ALL_PROXY": "",
            "all_proxy": "",
            "NO_PROXY": "127.0.0.1,localhost",
            "no_proxy": "127.0.0.1,localhost",
        }
    )

    if client_name == "codex":
        codex_home = home_dir / ".codex"
        codex_home.mkdir(parents=True, exist_ok=True)
        env.update(
            {
                "CODEX_HOME": str(codex_home),
                "OPENAI_API_KEY": "dummy",
                "OPENAI_BASE_URL": f"{proxy_base}/openai/v1",
            }
        )
    elif client_name == "claude":
        claude_dir = home_dir / ".claude"
        claude_dir.mkdir(parents=True, exist_ok=True)
        env.update(
            {
                "CLAUDE_CONFIG_DIR": str(claude_dir),
                "ANTHROPIC_API_KEY": "dummy",
                "ANTHROPIC_BASE_URL": f"{proxy_base}/anthropic",
            }
        )
    elif client_name == "gemini":
        env.update(
            {
                "GEMINI_API_KEY": "dummy",
                "GOOGLE_GEMINI_BASE_URL": f"{proxy_base}/google",
            }
        )
    else:
        raise ValueError(f"unknown client: {client_name}")
    return env


def prepare_report_dir(
    reports_root: pathlib.Path, timestamp: str | None = None
) -> pathlib.Path:
    reports_root.mkdir(parents=True, exist_ok=True)
    if timestamp is None:
        timestamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    run_dir = reports_root / timestamp
    suffix = 1
    while run_dir.exists():
        run_dir = reports_root / f"{timestamp}-{suffix:02d}"
        suffix += 1
    run_dir.mkdir(parents=True, exist_ok=False)
    latest = reports_root / "latest"
    if latest.exists() or latest.is_symlink():
        latest.unlink()
    latest.symlink_to(run_dir.name)
    return run_dir


def render_summary_markdown(summary: dict[str, object], results: list[dict[str, object]]) -> str:
    lines = [
        "# CLI Matrix Report",
        "",
        f"- Started: {summary.get('started_at', '')}",
        f"- Finished: {summary.get('finished_at', '')}",
        f"- Passed: {summary.get('pass', 0)}",
        f"- Failed: {summary.get('fail', 0)}",
        f"- Skipped: {summary.get('skip', 0)}",
        "",
        "| Case | Status | Message |",
        "| --- | --- | --- |",
    ]
    for result in results:
        lines.append(
            f"| {result['case_id']} | {result['status']} | {result.get('message', '')} |"
        )
    lines.append("")
    return "\n".join(lines)


def write_reports(
    reports_root: pathlib.Path,
    summary: dict[str, object],
    results: list[dict[str, object]],
    timestamp: str | None = None,
) -> pathlib.Path:
    run_dir = prepare_report_dir(pathlib.Path(reports_root), timestamp=timestamp)
    _write_reports_to_dir(run_dir, summary, results)
    return run_dir


def _write_reports_to_dir(
    run_dir: pathlib.Path,
    summary: dict[str, object],
    results: list[dict[str, object]],
) -> None:
    full_summary = dict(summary)
    full_summary["results"] = results
    report_json = json.dumps(full_summary, indent=2, sort_keys=True) + "\n"
    report_md = render_summary_markdown(summary, results)
    (run_dir / "report.json").write_text(report_json, encoding="utf-8")
    (run_dir / "report.md").write_text(report_md, encoding="utf-8")
    (run_dir / "summary.json").write_text(report_json, encoding="utf-8")
    (run_dir / "summary.md").write_text(report_md, encoding="utf-8")
    with (run_dir / "results.jsonl").open("w", encoding="utf-8") as handle:
        for result in results:
            handle.write(json.dumps(result, sort_keys=True) + "\n")


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def wait_for_health(base_url: str, timeout_secs: int = 30) -> None:
    deadline = time.time() + timeout_secs
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"{base_url}/health", timeout=2) as response:
                if response.status == 200:
                    return
        except Exception:
            time.sleep(0.2)
    raise RuntimeError(f"proxy at {base_url} did not become healthy in time")


def http_json(url: str, payload: dict[str, object], timeout: int = 60) -> tuple[int, str]:
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, response.read().decode("utf-8")
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8")
        return error.code, body


def probe_response_has_valid_shape(body: str) -> bool:
    try:
        payload = json.loads(body)
    except json.JSONDecodeError:
        return False

    if not isinstance(payload, dict):
        return False
    if isinstance(payload.get("output_text"), str):
        return True

    output = payload.get("output")
    if not isinstance(output, list) or not output:
        return False

    for item in output:
        if not isinstance(item, dict):
            continue
        content = item.get("content")
        if not isinstance(content, list):
            continue
        for part in content:
            if not isinstance(part, dict):
                continue
            if part.get("type") in {"output_text", "text"} and isinstance(
                part.get("text"), str
            ):
                return True

    return payload.get("object") == "response"


def probe_lane(proxy_base: str, lane: Lane) -> str | None:
    status, body = http_json(
        f"{proxy_base}/openai/v1/responses",
        {"model": lane.proxy_model, "input": "Reply with exactly PROBE_OK", "stream": False},
        timeout=60,
    )
    if status != 200:
        return f"lane probe returned HTTP {status}: {body[:240]}"
    if "PROBE_OK" in body or probe_response_has_valid_shape(body):
        return None
    return "lane probe succeeded but did not return a valid response shape"


def client_stdin_text(client_name: str, fixture: TaskFixture) -> str | None:
    if client_name == "claude":
        return fixture.prompt
    return None


def _workspace_file_text(
    workspace_dir: pathlib.Path, relative_path: pathlib.Path
) -> tuple[pathlib.Path | None, str | None, str | None]:
    target = workspace_dir / relative_path
    if not target.exists():
        return None, None, f"expected file {relative_path} to exist"
    try:
        return target, target.read_text(encoding="utf-8"), None
    except OSError as error:
        return None, None, f"failed to read {relative_path}: {error}"


def _python_return_description(return_spec: dict[str, object]) -> str:
    if return_spec.get("kind") == "binary_op":
        return (
            f"return {return_spec['left']} "
            f"{return_spec['operator']} "
            f"{return_spec['right']}"
        )
    return "the expected return expression"


def _matches_python_return(node: ast.AST | None, return_spec: dict[str, object]) -> bool:
    if return_spec.get("kind") != "binary_op" or not isinstance(node, ast.BinOp):
        return False

    operator_map: dict[str, type[ast.operator]] = {
        "+": ast.Add,
        "-": ast.Sub,
        "*": ast.Mult,
        "/": ast.Div,
    }
    operator = operator_map.get(str(return_spec.get("operator")))
    if operator is None or not isinstance(node.op, operator):
        return False
    if not isinstance(node.left, ast.Name) or not isinstance(node.right, ast.Name):
        return False
    return (
        node.left.id == str(return_spec.get("left"))
        and node.right.id == str(return_spec.get("right"))
    )


def _function_return_nodes(function_node: ast.FunctionDef | ast.AsyncFunctionDef) -> Iterable[ast.Return]:
    stack = list(reversed(function_node.body))
    while stack:
        node = stack.pop()
        if isinstance(node, ast.Return):
            yield node
            continue
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda)):
            continue
        stack.extend(reversed(list(ast.iter_child_nodes(node))))


def _verify_python_source_contract(
    workspace_dir: pathlib.Path, source_spec: dict[str, object]
) -> tuple[bool, str]:
    relative_path = pathlib.Path(str(source_spec["path"]))
    _target, source_text, error_message = _workspace_file_text(workspace_dir, relative_path)
    if error_message is not None or source_text is None:
        return False, error_message or f"expected file {relative_path} to exist"

    try:
        tree = ast.parse(source_text, filename=str(relative_path))
    except SyntaxError as error:
        return False, f"expected {relative_path} to parse successfully: {error}"

    function_name = str(source_spec["function"])
    function_node = next(
        (
            node
            for node in tree.body
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
            and node.name == function_name
        ),
        None,
    )
    if function_node is None:
        return False, f"expected {relative_path} to define {function_name}()"

    expected_args = [str(value) for value in source_spec.get("args", [])]
    if expected_args:
        actual_args = [arg.arg for arg in function_node.args.args]
        if actual_args != expected_args:
            return (
                False,
                f"expected {relative_path}:{function_name} to accept {expected_args}, got {actual_args}",
            )

    return_spec = dict(source_spec["returns"])
    if any(_matches_python_return(node.value, return_spec) for node in _function_return_nodes(function_node)):
        return True, ""

    return (
        False,
        f"expected {relative_path}:{function_name} to include {_python_return_description(return_spec)}",
    )


def _verify_python_entrypoint(
    workspace_dir: pathlib.Path, entrypoint_spec: dict[str, object]
) -> tuple[bool, str]:
    relative_path = pathlib.Path(str(entrypoint_spec["path"]))
    target, _source_text, error_message = _workspace_file_text(workspace_dir, relative_path)
    if error_message is not None or target is None:
        return False, error_message or f"expected file {relative_path} to exist"

    timeout_secs = int(entrypoint_spec.get("timeout_secs", 10))
    try:
        completed = subprocess.run(
            [sys.executable, str(target)],
            cwd=str(workspace_dir),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=timeout_secs,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return False, f"expected {relative_path} to finish within {timeout_secs}s"

    if completed.returncode != 0:
        stderr_text = (completed.stderr or "").strip()
        if stderr_text:
            return False, f"expected {relative_path} to exit 0, got: {stderr_text[:240]}"
        return False, f"expected {relative_path} to exit 0"

    stdout_text = completed.stdout or ""
    for expected_text in entrypoint_spec.get("expect_stdout_contains", []):
        if str(expected_text) not in stdout_text:
            return False, f"expected {relative_path} stdout to contain {expected_text!r}"
    return True, ""


def verify_fixture_output(
    fixture: TaskFixture, stdout_text: str, workspace_dir: pathlib.Path | None
) -> tuple[bool, str]:
    verifier_type = fixture.verifier["type"]
    if verifier_type == "contains":
        needle = str(fixture.verifier["value"])
        ok = needle.lower() in stdout_text.lower()
        return ok, f"expected output to contain {needle!r}"
    if verifier_type == "file_contains":
        if workspace_dir is None:
            return False, "workspace verifier required a workspace directory"
        relative_path = pathlib.Path(str(fixture.verifier["path"]))
        target = workspace_dir / relative_path
        if not target.exists():
            return False, f"expected file {relative_path} to exist"
        needle = str(fixture.verifier["needle"])
        ok = needle in target.read_text(encoding="utf-8")
        return ok, f"expected {relative_path} to contain {needle!r}"
    if verifier_type == "python_source_and_output":
        if workspace_dir is None:
            return False, "workspace verifier required a workspace directory"
        ok, message = _verify_python_source_contract(
            workspace_dir, dict(fixture.verifier["source"])
        )
        if not ok:
            return False, message
        return _verify_python_entrypoint(
            workspace_dir, dict(fixture.verifier["entrypoint"])
        )
    return False, f"unsupported verifier type: {verifier_type}"


def prepare_workspace(
    case: MatrixCase, workspaces_root: pathlib.Path
) -> pathlib.Path | None:
    workspace_dir = workspaces_root / case.case_id
    if workspace_dir.exists():
        shutil.rmtree(workspace_dir)
    if case.fixture.workspace_template is None:
        workspace_dir.mkdir(parents=True, exist_ok=True)
        return workspace_dir
    shutil.copytree(case.fixture.workspace_template, workspace_dir)
    return workspace_dir


def resolve_client_home_dir(case: MatrixCase, report_dir: pathlib.Path) -> pathlib.Path:
    if case.client_name == "gemini":
        return report_dir.parent / GEMINI_RUNNER_STATE_DIRNAME / GEMINI_SHARED_HOME_DIRNAME
    return report_dir / "homes" / case.case_id


def _gemini_bootstrap_bin_dir(home_dir: pathlib.Path) -> pathlib.Path:
    return home_dir / ".gemini" / "tmp" / "bin"


def gemini_bootstrap_ready(home_dir: pathlib.Path) -> bool:
    bin_dir = _gemini_bootstrap_bin_dir(home_dir)
    marker_path = home_dir / GEMINI_BOOTSTRAP_MARKER
    return marker_path.exists() or any(
        (bin_dir / candidate).exists() for candidate in ("rg", "rg.exe")
    )


def mark_gemini_bootstrap_ready(home_dir: pathlib.Path) -> None:
    marker_path = home_dir / GEMINI_BOOTSTRAP_MARKER
    marker_path.parent.mkdir(parents=True, exist_ok=True)
    marker_path.write_text("ready\n", encoding="utf-8")


def resolve_case_timeout_secs(case: MatrixCase, home_dir: pathlib.Path) -> int:
    if case.client_name != "gemini":
        return case.fixture.timeout_secs
    if gemini_bootstrap_ready(home_dir):
        return case.fixture.timeout_secs
    return max(case.fixture.timeout_secs, GEMINI_BOOTSTRAP_TIMEOUT_SECS)


def _report_path(report_dir: pathlib.Path, target: pathlib.Path) -> str:
    return os.path.relpath(target, report_dir)


def build_client_command(
    client_name: str, proxy_base: str, lane: Lane, fixture: TaskFixture, workspace_dir: pathlib.Path
) -> list[str]:
    if client_name == "codex":
        command = [
            "codex",
            "exec",
            fixture.prompt,
            "--model",
            lane.proxy_model,
            "--ephemeral",
            "--json",
            "--skip-git-repo-check",
            "--sandbox",
            "workspace-write" if fixture.kind == "long_horizon" else "read-only",
            "-C",
            str(workspace_dir),
            "-c",
            'model_provider="proxy"',
            "-c",
            'model_providers.proxy.name="Proxy"',
            "-c",
            f'model_providers.proxy.base_url="{proxy_base}/openai/v1"',
            "-c",
            'model_providers.proxy.wire_api="responses"',
            "-c",
            "model_providers.proxy.supports_websockets=false",
        ]
        return command
    if client_name == "claude":
        return [
            "claude",
            "--bare",
            "--print",
            "--output-format",
            "text",
            "--setting-sources",
            "user",
            "--model",
            lane.proxy_model,
            "--no-session-persistence",
            "--dangerously-skip-permissions",
            "--add-dir",
            str(workspace_dir),
        ]
    if client_name == "gemini":
        return [
            "gemini",
            "--prompt",
            fixture.prompt,
            "--model",
            lane.proxy_model,
            "--sandbox=false",
            "--yolo",
            "--include-directories",
            str(workspace_dir),
            "--output-format",
            "text",
        ]
    raise ValueError(f"unknown client: {client_name}")


def run_matrix_case(
    case: MatrixCase,
    proxy_base: str,
    report_dir: pathlib.Path,
    base_env: dict[str, str],
) -> dict[str, object]:
    cases_dir = report_dir / "cases"
    workspaces_root = report_dir / "workspaces"
    cases_dir.mkdir(parents=True, exist_ok=True)
    workspaces_root.mkdir(parents=True, exist_ok=True)

    workspace_dir = prepare_workspace(case, workspaces_root)
    home_dir = resolve_client_home_dir(case, report_dir)
    env = build_client_env(case.client_name, base_env, proxy_base, home_dir)
    command = build_client_command(
        case.client_name, proxy_base, case.lane, case.fixture, workspace_dir
    )
    stdin_text = client_stdin_text(case.client_name, case.fixture)
    timeout_secs = resolve_case_timeout_secs(case, home_dir)
    started = time.time()
    status = "failed"
    message = ""

    try:
        run_kwargs: dict[str, object] = {
            "cwd": str(workspace_dir),
            "env": env,
            "stdout": subprocess.PIPE,
            "stderr": subprocess.PIPE,
            "text": True,
            "timeout": timeout_secs,
            "check": False,
        }
        if stdin_text is not None:
            run_kwargs["input"] = stdin_text
        else:
            run_kwargs["stdin"] = subprocess.DEVNULL
        completed = subprocess.run(command, **run_kwargs)
        if case.client_name == "gemini" and (
            completed.returncode == 0 or gemini_bootstrap_ready(home_dir)
        ):
            mark_gemini_bootstrap_ready(home_dir)
        stdout_text = completed.stdout or ""
        stderr_text = completed.stderr or ""
        if completed.returncode == 0:
            ok, verifier_message = verify_fixture_output(
                case.fixture,
                stdout_text,
                workspace_dir if case.fixture.workspace_template is not None else None,
            )
            if ok:
                status = "passed"
                message = "verifier passed"
            else:
                status = "failed"
                message = verifier_message
        else:
            message = f"exit code {completed.returncode}"
    except subprocess.TimeoutExpired as error:
        stdout_text = error.stdout or ""
        stderr_text = error.stderr or ""
        message = f"timed out after {timeout_secs}s"

    duration_secs = round(time.time() - started, 3)
    stdout_path = cases_dir / f"{case.case_id}.stdout.txt"
    stderr_path = cases_dir / f"{case.case_id}.stderr.txt"
    stdout_path.write_text(stdout_text, encoding="utf-8")
    stderr_path.write_text(stderr_text, encoding="utf-8")

    return {
        "case_id": case.case_id,
        "client": case.client_name,
        "lane": case.lane.name,
        "fixture": case.fixture.fixture_id,
        "status": status,
        "message": message,
        "duration_secs": duration_secs,
        "stdout_path": _report_path(report_dir, stdout_path),
        "stderr_path": _report_path(report_dir, stderr_path),
        "workspace_path": _report_path(report_dir, workspace_dir),
        "home_path": _report_path(report_dir, home_dir),
        "command": command,
    }


def print_case_list(cases: list[MatrixCase]) -> None:
    for case in cases:
        print(
            f"{case.case_id}\tclient={case.client_name}\tlane={case.lane.name}"
            f"\tkind={case.fixture.kind}"
        )


def ensure_required_binaries(clients: Iterable[str], proxy_binary: pathlib.Path) -> None:
    missing = []
    for client_name in clients:
        if shutil.which(client_name) is None:
            missing.append(client_name)
    if not proxy_binary.exists():
        missing.append(str(proxy_binary))
    if missing:
        raise RuntimeError("missing prerequisites: " + ", ".join(missing))


def prepare_proxy_env(base_env: dict[str, str], dotenv_env: dict[str, str]) -> dict[str, str]:
    proxy_env = dict(base_env)
    proxy_env.update(dotenv_env)
    return proxy_env


def start_proxy(
    proxy_binary: pathlib.Path,
    runtime_config_text: str,
    report_dir: pathlib.Path,
    proxy_env: dict[str, str],
) -> tuple[subprocess.Popen[str], pathlib.Path, pathlib.Path, pathlib.Path]:
    runtime_config_path = report_dir / "runtime-config.yaml"
    runtime_config_path.write_text(runtime_config_text, encoding="utf-8")
    stdout_path = report_dir / "proxy.stdout.log"
    stderr_path = report_dir / "proxy.stderr.log"
    stdout_handle = stdout_path.open("w", encoding="utf-8")
    stderr_handle = stderr_path.open("w", encoding="utf-8")
    process = subprocess.Popen(
        [str(proxy_binary), "--config", str(runtime_config_path)],
        cwd=str(REPO_ROOT),
        env=proxy_env,
        stdout=stdout_handle,
        stderr=stderr_handle,
        text=True,
    )
    return process, runtime_config_path, stdout_path, stderr_path


def stop_proxy(process: subprocess.Popen[str] | None) -> None:
    if process is None:
        return
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def summarize_results(results: list[dict[str, object]]) -> tuple[int, int, int]:
    passed = sum(1 for item in results if item["status"] == "passed")
    failed = sum(1 for item in results if item["status"] == "failed")
    skipped = sum(1 for item in results if item["status"] == "skipped")
    return passed, failed, skipped


def selected_clients(cases: Iterable[MatrixCase]) -> list[str]:
    ordered: list[str] = []
    seen: set[str] = set()
    for case in cases:
        if case.client_name in seen:
            continue
        seen.add(case.client_name)
        ordered.append(case.client_name)
    return ordered


def resolve_cli_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run the real CLI matrix through llm-universal-proxy"
    )
    parser.add_argument("--test", default="all", help="phase selector")
    parser.add_argument("--skip-slow", action="store_true", help="skip long-horizon tasks")
    parser.add_argument("--proxy-only", action="store_true", help="start proxy and wait")
    parser.add_argument(
        "--list",
        "--list-matrix",
        dest="list_matrix",
        action="store_true",
        help="list matrix cases and exit (--list is kept as a compatibility alias)",
    )
    parser.add_argument(
        "--case",
        action="append",
        default=[],
        help="run or list only the specified case id; repeat to select multiple cases",
    )
    parser.add_argument("--config-source", default=str(DEFAULT_CONFIG_SOURCE))
    parser.add_argument("--env-file", default=str(DEFAULT_ENV_FILE))
    parser.add_argument("--fixtures-root", default=str(DEFAULT_FIXTURES_ROOT))
    parser.add_argument("--reports-root", default=str(DEFAULT_REPORTS_ROOT))
    parser.add_argument("--binary", default=str(DEFAULT_PROXY_BINARY))
    parser.add_argument("--proxy-host", default="127.0.0.1")
    parser.add_argument("--proxy-port", type=int, default=int(os.environ.get("PROXY_PORT", "18888")))
    args = parser.parse_args(argv)
    args.list = args.list_matrix
    if args.test not in VALID_PHASES:
        parser.error(f"--test must be one of: {', '.join(sorted(VALID_PHASES))}")
    return args


def run(argv: list[str] | None = None) -> int:
    args = resolve_cli_args(argv)
    base_env = dict(os.environ)
    config_source = pathlib.Path(args.config_source)
    dotenv_env = load_dotenv_file(pathlib.Path(args.env_file))
    parsed_source = parse_proxy_source(config_source.read_text(encoding="utf-8"))
    lanes = resolve_lanes(parsed_source, dotenv_env)
    fixtures = load_fixtures(pathlib.Path(args.fixtures_root))
    cases = expand_matrix(
        clients=CLIENT_NAMES,
        lanes=lanes,
        fixtures=fixtures,
        phase=args.test,
        skip_slow=args.skip_slow,
    )
    cases = filter_matrix_cases(cases, selected_case_ids=args.case)

    if args.list_matrix:
        print_case_list(cases)
        return 0
    if not cases and not args.proxy_only:
        raise RuntimeError("no matrix cases selected")

    proxy_binary = pathlib.Path(args.binary)
    required_clients: list[str] = [] if args.proxy_only else selected_clients(cases)
    ensure_required_binaries(required_clients, proxy_binary)

    started_at = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    report_dir = prepare_report_dir(pathlib.Path(args.reports_root))
    trace_path = report_dir / "debug-trace.jsonl"
    runtime_config_text = build_runtime_config_text(
        parsed_source,
        dotenv_env,
        listen_host=args.proxy_host,
        listen_port=args.proxy_port,
        trace_path=trace_path,
    )
    proxy_env = prepare_proxy_env(base_env, dotenv_env)
    process = None
    results: list[dict[str, object]] = []

    try:
        process, _runtime_config_path, _stdout_path, _stderr_path = start_proxy(
            proxy_binary, runtime_config_text, report_dir, proxy_env
        )
        proxy_base = f"http://{args.proxy_host}:{args.proxy_port}"
        wait_for_health(proxy_base)
        if args.proxy_only:
            print(f"Proxy healthy at {proxy_base}")
            print(f"OpenAI base: {proxy_base}/openai/v1")
            print(f"Anthropic base: {proxy_base}/anthropic")
            print(f"Gemini base: {proxy_base}/google")
            try:
                process.wait()
            except KeyboardInterrupt:
                pass
            return 0

        lane_probes = {lane.name: classify_lane_health(lane, probe_lane(proxy_base, lane)) for lane in lanes}
        for case in cases:
            lane_status, lane_message = lane_probes[case.lane.name]
            if lane_status != "ready":
                results.append(
                    {
                        "case_id": case.case_id,
                        "client": case.client_name,
                        "lane": case.lane.name,
                        "fixture": case.fixture.fixture_id,
                        "status": lane_status,
                        "message": lane_message or "",
                    }
                )
                continue
            print(f"[run] {case.case_id}")
            results.append(run_matrix_case(case, proxy_base, report_dir, base_env))
    finally:
        stop_proxy(process)

    finished_at = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    passed, failed, skipped = summarize_results(results)
    _write_reports_to_dir(
        report_dir,
        {
            "started_at": started_at,
            "finished_at": finished_at,
            "pass": passed,
            "fail": failed,
            "skip": skipped,
            "phase": args.test,
            "report_dir": str(report_dir),
        },
        results,
    )
    print(f"Report: {report_dir}")
    print(f"Passed: {passed}  Failed: {failed}  Skipped: {skipped}")
    return 1 if failed else 0


def main() -> int:
    try:
        return run()
    except KeyboardInterrupt:
        print("Interrupted", file=sys.stderr)
        return 130
    except Exception as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
