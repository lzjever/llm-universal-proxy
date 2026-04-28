#!/usr/bin/env python3
"""Real CLI matrix harness for Codex, Claude Code, and Gemini CLI."""

from __future__ import annotations

import atexit
import argparse
import ast
import collections
import dataclasses
import hashlib
import json
import os
import pathlib
import re
import secrets
import shlex
import shutil
import signal
import socket
import string
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Iterable, Sequence


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_CONFIG_SOURCE = (
    REPO_ROOT / "scripts" / "fixtures" / "cli_matrix" / "default_proxy_test_matrix.yaml"
)
DEFAULT_ENV_FILE = REPO_ROOT / ".env.test"
DEFAULT_FIXTURES_ROOT = REPO_ROOT / "scripts" / "fixtures" / "cli_matrix"
DEFAULT_REPORTS_ROOT = REPO_ROOT / "test-reports" / "cli-matrix"
DEFAULT_RELEASE_PROXY_BINARY = REPO_ROOT / "target" / "release" / "llm-universal-proxy"
DEFAULT_DEBUG_PROXY_BINARY = REPO_ROOT / "target" / "debug" / "llm-universal-proxy"
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
TRACE_CLIENT_FORMAT_BY_CLIENT = {
    "codex": "openai-responses",
    "claude": "anthropic",
    "gemini": "google",
}
TRACE_PATH_PREFIX_BY_CLIENT = {
    "codex": "/openai/",
    "claude": "/anthropic/",
    "gemini": "/google/",
}
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
RUST_TOOLCHAIN_ENV_KEYS = ("CARGO_HOME", "RUSTUP_HOME")
GEMINI_BOOTSTRAP_TIMEOUT_SECS = 180
GEMINI_RUNNER_STATE_DIRNAME = "_runner_state"
GEMINI_SHARED_HOME_DIRNAME = "gemini-home"
GEMINI_BOOTSTRAP_MARKER = ".runner-gemini-bootstrap-ready"
DEFAULT_PROXY_HEALTH_TIMEOUT_SECS = 45
DEFAULT_CASE_TIMEOUT_FLOOR_SECS = 240
DEFAULT_LONG_HORIZON_TIMEOUT_FLOOR_SECS = 420
DEFAULT_GEMINI_BOOTSTRAP_TIMEOUT_SECS = 360
DEFAULT_PROCESS_TERMINATE_GRACE_SECS = 15
DEFAULT_POST_KILL_WAIT_SECS = 2
DEFAULT_AUTO_COMPACT_RATIO = 0.85
DEFAULT_GEMINI_COMPRESSION_THRESHOLD = DEFAULT_AUTO_COMPACT_RATIO
DEFAULT_CODEX_TRUNCATION_LIMIT_BYTES = 10000
RESERVED_INTERNAL_TOOL_NAME_PREFIX = "__llmup_custom__"
INTERNAL_TOOL_ARTIFACT_PATTERN = re.compile(r"__llmup_custom__[A-Za-z0-9_:-]*")
PUBLIC_APPLY_PATCH_TOOL_NAME = "apply_patch"
PUBLIC_APPLY_PATCH_TOOL_TYPE = "freeform"
DEFAULT_CODEX_APPLY_PATCH_TOOL_TYPE = PUBLIC_APPLY_PATCH_TOOL_TYPE
PRESET_ENDPOINT_MODEL_ENV = "PRESET_ENDPOINT_MODEL"
PRESET_ENDPOINT_API_KEY_ENV = "PRESET_ENDPOINT_API_KEY"
PRESET_OPENAI_ENDPOINT_BASE_URL_ENV = "PRESET_OPENAI_ENDPOINT_BASE_URL"
PRESET_ANTHROPIC_ENDPOINT_BASE_URL_ENV = "PRESET_ANTHROPIC_ENDPOINT_BASE_URL"
PRESET_OPENAI_COMPATIBLE_LANE = "preset-openai-compatible"
PRESET_ANTHROPIC_COMPATIBLE_LANE = "preset-anthropic-compatible"
PRESET_OPENAI_COMPATIBLE_UPSTREAM = "PRESET-OPENAI-COMPATIBLE"
PRESET_ANTHROPIC_COMPATIBLE_UPSTREAM = "PRESET-ANTHROPIC-COMPATIBLE"
AUTH_MODE_ENV = "LLM_UNIVERSAL_PROXY_AUTH_MODE"
PROXY_KEY_ENV = "LLM_UNIVERSAL_PROXY_KEY"
DEFAULT_PROXY_KEY = "llmup-proxy-key"
SUPPORTED_PROMPT_TEMPLATE_FIELDS = frozenset({"client_name"})
REPLAY_MARKER_KEY_ENV = "LLMUP_INTERNAL_REPLAY_MARKER_KEY"
REPLAY_MARKER_KEY_FILENAME = ".llmup-internal-replay-marker-key"
CLIENT_RUNTIME_ROOT_PREFIX = "llmup-real-cli-runtime-"
TRACE_CASE_START_SKEW_MS = 100
TRACE_CASE_END_SKEW_MS = 5000

_CLIENT_RUNTIME_ROOTS_BY_REPORT_DIR: dict[str, pathlib.Path] = {}
_CLIENT_RUNTIME_CASE_TOKENS: dict[tuple[str, str], str] = {}
_CLIENT_RUNTIME_CASE_COUNTERS: dict[str, int] = {}
_CLIENT_RUNTIME_ROOTS_TO_CLEANUP: set[pathlib.Path] = set()
_CLIENT_RUNTIME_CLEANUP_REGISTERED = False


@dataclasses.dataclass
class ModelLimits:
    context_window: int | None = None
    max_output_tokens: int | None = None

    def merged_with(self, override: ModelLimits | None) -> ModelLimits | None:
        merged = ModelLimits(
            context_window=(
                override.context_window if override and override.context_window is not None else self.context_window
            ),
            max_output_tokens=(
                override.max_output_tokens
                if override and override.max_output_tokens is not None
                else self.max_output_tokens
            ),
        )
        if merged.context_window is None and merged.max_output_tokens is None:
            return None
        return merged


@dataclasses.dataclass
class CodexModelMetadata:
    input_modalities: tuple[str, ...] | None = None
    supports_search_tool: bool | None = None
    supports_view_image: bool | None = None
    apply_patch_tool_type: str | None = None
    supports_parallel_tool_calls: bool | None = None

    def merged_with(
        self, override: CodexModelMetadata | None
    ) -> CodexModelMetadata | None:
        merged = CodexModelMetadata(
            input_modalities=(
                override.input_modalities
                if override and override.input_modalities is not None
                else self.input_modalities
            ),
            supports_search_tool=(
                override.supports_search_tool
                if override and override.supports_search_tool is not None
                else self.supports_search_tool
            ),
            supports_view_image=(
                override.supports_view_image
                if override and override.supports_view_image is not None
                else self.supports_view_image
            ),
            apply_patch_tool_type=(
                override.apply_patch_tool_type
                if override and override.apply_patch_tool_type is not None
                else self.apply_patch_tool_type
            ),
            supports_parallel_tool_calls=(
                override.supports_parallel_tool_calls
                if override and override.supports_parallel_tool_calls is not None
                else self.supports_parallel_tool_calls
            ),
        )
        if (
            merged.input_modalities is None
            and merged.supports_search_tool is None
            and merged.supports_view_image is None
            and merged.apply_patch_tool_type is None
            and merged.supports_parallel_tool_calls is None
        ):
            return None
        return merged


@dataclasses.dataclass
class SurfaceMetadata:
    input_modalities: tuple[str, ...] | None = None
    output_modalities: tuple[str, ...] | None = None
    supports_search: bool | None = None
    supports_view_image: bool | None = None
    apply_patch_transport: str | None = None
    supports_parallel_calls: bool | None = None

    def merged_with(self, override: SurfaceMetadata | None) -> SurfaceMetadata | None:
        merged = SurfaceMetadata(
            input_modalities=(
                override.input_modalities
                if override and override.input_modalities is not None
                else self.input_modalities
            ),
            output_modalities=(
                override.output_modalities
                if override and override.output_modalities is not None
                else self.output_modalities
            ),
            supports_search=(
                override.supports_search
                if override and override.supports_search is not None
                else self.supports_search
            ),
            supports_view_image=(
                override.supports_view_image
                if override and override.supports_view_image is not None
                else self.supports_view_image
            ),
            apply_patch_transport=(
                override.apply_patch_transport
                if override and override.apply_patch_transport is not None
                else self.apply_patch_transport
            ),
            supports_parallel_calls=(
                override.supports_parallel_calls
                if override and override.supports_parallel_calls is not None
                else self.supports_parallel_calls
            ),
        )
        if (
            merged.input_modalities is None
            and merged.output_modalities is None
            and merged.supports_search is None
            and merged.supports_view_image is None
            and merged.apply_patch_transport is None
            and merged.supports_parallel_calls is None
        ):
            return None
        return merged

    def to_codex_metadata(self) -> CodexModelMetadata | None:
        metadata = CodexModelMetadata(
            input_modalities=self.input_modalities,
            supports_search_tool=self.supports_search,
            supports_view_image=self.supports_view_image,
            apply_patch_tool_type=(
                PUBLIC_APPLY_PATCH_TOOL_TYPE
                if self.apply_patch_transport is not None
                else None
            ),
            supports_parallel_tool_calls=self.supports_parallel_calls,
        )
        if (
            metadata.input_modalities is None
            and metadata.supports_search_tool is None
            and metadata.supports_view_image is None
            and metadata.apply_patch_tool_type is None
            and metadata.supports_parallel_tool_calls is None
        ):
            return None
        return metadata


DEFAULT_PROXY_CODEX_METADATA = CodexModelMetadata(
    input_modalities=("text",),
    supports_search_tool=False,
    apply_patch_tool_type=PUBLIC_APPLY_PATCH_TOOL_TYPE,
)
DEFAULT_PROXY_SURFACE_METADATA = SurfaceMetadata(
    input_modalities=("text",),
    output_modalities=("text",),
    supports_search=False,
    supports_view_image=False,
    apply_patch_transport=PUBLIC_APPLY_PATCH_TOOL_TYPE,
    supports_parallel_calls=False,
)


@dataclasses.dataclass
class LiveModelProfile:
    limits: ModelLimits | None = None
    codex_metadata: CodexModelMetadata | None = None

DEFAULT_CODEX_BASE_INSTRUCTIONS = (
    "You are Codex, a coding agent based on GPT-5. "
    "You and the user share the same workspace and collaborate "
    "to achieve the user's goals."
)


def normalize_proxy_base(proxy_base: str) -> str:
    return proxy_base.rstrip("/")


def default_proxy_binary_path(
    *,
    release_binary: pathlib.Path = DEFAULT_RELEASE_PROXY_BINARY,
    debug_binary: pathlib.Path = DEFAULT_DEBUG_PROXY_BINARY,
) -> pathlib.Path:
    candidates = [binary for binary in (release_binary, debug_binary) if binary.exists()]
    if not candidates:
        return release_binary
    return max(
        candidates,
        key=lambda binary: (
            binary.stat().st_mtime_ns,
            1 if binary == debug_binary else 0,
        ),
    )


DEFAULT_PROXY_BINARY = default_proxy_binary_path()


def _parse_surface_metadata_value(
    surface: SurfaceMetadata,
    surface_section: str,
    key: str,
    value: str,
    parsed_value: object,
) -> None:
    if surface_section == "modalities":
        if key == "input":
            surface.input_modalities = parse_string_list(value)
        elif key == "output":
            surface.output_modalities = parse_string_list(value)
        return

    if surface_section != "tools":
        return
    if key == "supports_search":
        surface.supports_search = bool(parsed_value)
    elif key == "supports_view_image":
        surface.supports_view_image = bool(parsed_value)
    elif key == "apply_patch_transport" and isinstance(parsed_value, str):
        surface.apply_patch_transport = parsed_value
    elif key == "supports_parallel_calls":
        surface.supports_parallel_calls = bool(parsed_value)


def _parse_codex_metadata_value(
    codex_metadata: CodexModelMetadata,
    key: str,
    value: str,
    parsed_value: object,
) -> None:
    if key == "supports_search_tool":
        codex_metadata.supports_search_tool = bool(parsed_value)
    elif key == "input_modalities":
        codex_metadata.input_modalities = parse_string_list(value)
    elif key == "supports_view_image":
        codex_metadata.supports_view_image = bool(parsed_value)
    elif key == "apply_patch_tool_type" and isinstance(parsed_value, str):
        codex_metadata.apply_patch_tool_type = parsed_value
    elif key == "supports_parallel_tool_calls":
        codex_metadata.supports_parallel_tool_calls = bool(parsed_value)


def default_codex_supported_reasoning_levels() -> list[dict[str, str]]:
    return [
        {
            "effort": "low",
            "description": "Fast responses with lighter reasoning",
        },
        {
            "effort": "medium",
            "description": "Balanced reasoning depth for everyday work",
        },
        {
            "effort": "high",
            "description": "Greater reasoning depth for harder problems",
        },
        {
            "effort": "xhigh",
            "description": "Maximum reasoning depth for complex problems",
        },
    ]


def find_internal_tool_artifact(value: object) -> str | None:
    if isinstance(value, str):
        text = value
    else:
        text = json.dumps(value, sort_keys=True, default=str)
    match = INTERNAL_TOOL_ARTIFACT_PATTERN.search(text)
    if match is None:
        return None
    return match.group(0)


def ensure_locked_apply_patch_public_contract() -> None:
    artifact = find_internal_tool_artifact(PUBLIC_APPLY_PATCH_TOOL_NAME)
    if artifact is not None:
        raise ValueError(
            "apply_patch public contract must not expose reserved internal tool artifact "
            f"{artifact}"
        )
    if DEFAULT_CODEX_APPLY_PATCH_TOOL_TYPE != PUBLIC_APPLY_PATCH_TOOL_TYPE:
        raise ValueError(
            "apply_patch public contract must remain freeform on client-visible surfaces"
        )


def ensure_no_public_internal_tool_artifacts(
    value: object, *, context: str
) -> None:
    ensure_locked_apply_patch_public_contract()
    artifact = find_internal_tool_artifact(value)
    if artifact is None:
        return
    raise ValueError(
        f"{context} must not expose reserved internal tool artifact {artifact}"
    )


def default_codex_catalog_entry(model_name: str) -> dict[str, object]:
    ensure_locked_apply_patch_public_contract()
    return {
        "slug": model_name,
        "display_name": model_name,
        "supported_reasoning_levels": default_codex_supported_reasoning_levels(),
        "shell_type": "shell_command",
        "visibility": "list",
        "supported_in_api": True,
        "priority": 0,
        "base_instructions": DEFAULT_CODEX_BASE_INSTRUCTIONS,
        "supports_reasoning_summaries": False,
        "support_verbosity": False,
        "truncation_policy": {
            "mode": "bytes",
            "limit": DEFAULT_CODEX_TRUNCATION_LIMIT_BYTES,
        },
        "apply_patch_tool_type": DEFAULT_CODEX_APPLY_PATCH_TOOL_TYPE,
        "supports_parallel_tool_calls": False,
        "experimental_supported_tools": [],
    }


def validate_public_apply_patch_tool_type(value: str | None) -> str | None:
    if value is None:
        return None
    if value != PUBLIC_APPLY_PATCH_TOOL_TYPE:
        raise ValueError(
            "apply_patch public contract must remain freeform on client-visible surfaces"
        )
    return value


@dataclasses.dataclass
class ParsedModelAlias:
    target: str
    limits: ModelLimits | None = None
    surface: SurfaceMetadata | None = None
    codex_metadata: CodexModelMetadata | None = None


@dataclasses.dataclass(frozen=True)
class TimeoutPolicy:
    proxy_health_timeout_secs: int = DEFAULT_PROXY_HEALTH_TIMEOUT_SECS
    case_timeout_floor_secs: int = DEFAULT_CASE_TIMEOUT_FLOOR_SECS
    long_horizon_timeout_floor_secs: int = DEFAULT_LONG_HORIZON_TIMEOUT_FLOOR_SECS
    gemini_bootstrap_timeout_secs: int = DEFAULT_GEMINI_BOOTSTRAP_TIMEOUT_SECS
    process_terminate_grace_secs: int = DEFAULT_PROCESS_TERMINATE_GRACE_SECS


DEFAULT_TIMEOUT_POLICY = TimeoutPolicy()
GEMINI_BOOTSTRAP_TIMEOUT_SECS = DEFAULT_TIMEOUT_POLICY.gemini_bootstrap_timeout_secs


@dataclasses.dataclass
class SourceConfigSection:
    key: str | None
    raw_lines: tuple[str, ...]


@dataclasses.dataclass
class ProxySourceConfig:
    listen: str
    proxy: object | None
    upstream_timeout_secs: int | None
    upstreams: collections.OrderedDict[str, collections.OrderedDict[str, object]]
    upstream_limits: collections.OrderedDict[str, ModelLimits]
    upstream_surface_defaults: collections.OrderedDict[str, SurfaceMetadata]
    upstream_codex_metadata: collections.OrderedDict[str, CodexModelMetadata]
    model_aliases: collections.OrderedDict[str, str]
    model_alias_configs: collections.OrderedDict[str, ParsedModelAlias]
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
    limits: ModelLimits | None = None
    codex_metadata: CodexModelMetadata | None = None


@dataclasses.dataclass
class TaskFixture:
    fixture_id: str
    kind: str
    prompt: str
    verifier: dict[str, object]
    timeout_secs: int
    workspace_template: pathlib.Path | None
    prompt_template: str | None = None
    description: str = ""
    supported_clients: tuple[str, ...] = ()
    unsupported_lanes: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        if self.prompt_template is None:
            return
        if not isinstance(self.prompt_template, str):
            raise ValueError(
                f"invalid prompt_template for fixture {self.fixture_id!r}: must be a string"
            )
        validate_prompt_template(self.fixture_id, self.prompt_template)


@dataclasses.dataclass
class MatrixCase:
    client_name: str
    lane: Lane
    fixture: TaskFixture
    case_id: str


@dataclasses.dataclass(frozen=True)
class VerifierContext:
    client_name: str | None = None
    case_id: str | None = None
    command: tuple[str, ...] | None = None
    home_dir: pathlib.Path | None = None
    workspace_dir: pathlib.Path | None = None
    trace_entries: tuple[dict[str, object], ...] = ()
    diagnostics: dict[str, object] | None = None
    workspace_diff: dict[str, object] | None = None


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


def parse_string_list(value: str) -> tuple[str, ...]:
    parsed = ast.literal_eval(value.strip())
    if not isinstance(parsed, list):
        raise ValueError(f"expected list literal, got {value!r}")
    return tuple(str(item) for item in parsed)


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
    proxy: object | None = None
    upstream_timeout_secs = None
    upstreams: collections.OrderedDict[str, collections.OrderedDict[str, object]] = (
        collections.OrderedDict()
    )
    upstream_limits: collections.OrderedDict[str, ModelLimits] = collections.OrderedDict()
    upstream_surface_defaults: collections.OrderedDict[str, SurfaceMetadata] = (
        collections.OrderedDict()
    )
    upstream_codex_metadata: collections.OrderedDict[str, CodexModelMetadata] = (
        collections.OrderedDict()
    )
    model_aliases: collections.OrderedDict[str, str] = collections.OrderedDict()
    model_alias_configs: collections.OrderedDict[str, ParsedModelAlias] = (
        collections.OrderedDict()
    )
    debug_trace: collections.OrderedDict[str, object] = collections.OrderedDict()

    section: str | None = None
    current_upstream: str | None = None
    current_upstream_subsection: str | None = None
    current_upstream_nested_key: str | None = None
    current_upstream_surface_section: str | None = None
    current_alias: str | None = None
    current_alias_subsection: str | None = None
    current_alias_surface_section: str | None = None

    for raw_line in text.splitlines():
        line = raw_line.split("#", 1)[0].rstrip()
        if not line.strip():
            continue
        indent = len(line) - len(line.lstrip(" "))
        stripped = line.strip()
        if indent == 0:
            current_upstream = None
            current_upstream_subsection = None
            current_upstream_nested_key = None
            current_upstream_surface_section = None
            current_alias = None
            current_alias_subsection = None
            current_alias_surface_section = None
            if stripped == "proxy:":
                section = "proxy"
                if not isinstance(proxy, collections.OrderedDict):
                    proxy = collections.OrderedDict()
                continue
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
            elif key == "proxy":
                proxy = parsed_value
            elif key == "upstream_timeout_secs":
                upstream_timeout_secs = int(parsed_value)
            continue

        if section == "proxy" and indent == 2:
            key, value = stripped.split(":", 1)
            if not isinstance(proxy, collections.OrderedDict):
                proxy = collections.OrderedDict()
            proxy[key] = parse_scalar(value)
            continue

        if section == "upstreams":
            if indent == 2 and stripped.endswith(":"):
                current_upstream = stripped[:-1]
                current_upstream_subsection = None
                current_upstream_nested_key = None
                current_upstream_surface_section = None
                upstreams[current_upstream] = collections.OrderedDict()
                continue
            if indent == 4 and current_upstream is not None and stripped == "limits:":
                current_upstream_subsection = "limits"
                current_upstream_nested_key = None
                current_upstream_surface_section = None
                upstream_limits[current_upstream] = ModelLimits()
                continue
            if (
                indent == 4
                and current_upstream is not None
                and stripped == "surface_defaults:"
            ):
                current_upstream_subsection = "surface_defaults"
                current_upstream_nested_key = None
                current_upstream_surface_section = None
                upstream_surface_defaults[current_upstream] = SurfaceMetadata()
                continue
            if indent == 4 and current_upstream is not None and stripped == "codex:":
                current_upstream_subsection = "codex"
                current_upstream_nested_key = None
                current_upstream_surface_section = None
                upstream_codex_metadata[current_upstream] = CodexModelMetadata()
                continue
            if (
                indent == 6
                and current_upstream is not None
                and current_upstream_subsection == "surface_defaults"
                and stripped in {"modalities:", "tools:"}
            ):
                current_upstream_surface_section = stripped[:-1]
                continue
            if (
                indent == 8
                and current_upstream is not None
                and current_upstream_subsection == "surface_defaults"
                and current_upstream_surface_section is not None
            ):
                key, value = stripped.split(":", 1)
                parsed_value = parse_scalar(value)
                surface_defaults = upstream_surface_defaults[current_upstream]
                _parse_surface_metadata_value(
                    surface_defaults,
                    current_upstream_surface_section,
                    key,
                    value,
                    parsed_value,
                )
                continue
            if (
                indent == 6
                and current_upstream is not None
                and current_upstream_subsection == "limits"
            ):
                key, value = stripped.split(":", 1)
                parsed_value = parse_scalar(value)
                if key == "context_window":
                    upstream_limits[current_upstream].context_window = int(parsed_value)
                elif key == "max_output_tokens":
                    upstream_limits[current_upstream].max_output_tokens = int(parsed_value)
                continue
            if (
                indent == 6
                and current_upstream is not None
                and current_upstream_subsection == "codex"
            ):
                key, value = stripped.split(":", 1)
                parsed_value = parse_scalar(value)
                _parse_codex_metadata_value(
                    upstream_codex_metadata[current_upstream],
                    key,
                    value,
                    parsed_value,
                )
                continue
            if (
                indent == 6
                and current_upstream is not None
                and current_upstream_nested_key is not None
            ):
                key, value = stripped.split(":", 1)
                nested_mapping = upstreams[current_upstream].get(current_upstream_nested_key)
                if not isinstance(nested_mapping, collections.OrderedDict):
                    nested_mapping = collections.OrderedDict()
                    upstreams[current_upstream][current_upstream_nested_key] = nested_mapping
                nested_mapping[key] = parse_scalar(value)
                continue
            if indent >= 4 and current_upstream is not None:
                current_upstream_subsection = None
                current_upstream_nested_key = None
                current_upstream_surface_section = None
                key, value = stripped.split(":", 1)
                if not value.strip():
                    current_upstream_nested_key = key
                    upstreams[current_upstream][key] = collections.OrderedDict()
                    continue
                upstreams[current_upstream][key] = parse_scalar(value)
                continue

        if section == "model_aliases":
            if indent == 2 and stripped.endswith(":"):
                current_alias = stripped[:-1]
                current_alias_subsection = None
                current_alias_surface_section = None
                model_aliases[current_alias] = ""
                model_alias_configs[current_alias] = ParsedModelAlias(target="")
                continue
            if indent == 2:
                current_alias = None
                current_alias_subsection = None
                current_alias_surface_section = None
                key, value = stripped.split(":", 1)
                target = str(parse_scalar(value))
                model_aliases[key] = target
                model_alias_configs[key] = ParsedModelAlias(target=target)
                continue
            if indent == 4 and current_alias is not None:
                if stripped == "limits:":
                    current_alias_subsection = "limits"
                    current_alias_surface_section = None
                    alias_config = model_alias_configs[current_alias]
                    if alias_config.limits is None:
                        alias_config.limits = ModelLimits()
                    continue
                if stripped == "surface:":
                    current_alias_subsection = "surface"
                    current_alias_surface_section = None
                    alias_config = model_alias_configs[current_alias]
                    if alias_config.surface is None:
                        alias_config.surface = SurfaceMetadata()
                    continue
                if stripped == "codex:":
                    current_alias_subsection = "codex"
                    current_alias_surface_section = None
                    alias_config = model_alias_configs[current_alias]
                    if alias_config.codex_metadata is None:
                        alias_config.codex_metadata = CodexModelMetadata()
                    continue
                current_alias_subsection = None
                current_alias_surface_section = None
                key, value = stripped.split(":", 1)
                parsed_value = parse_scalar(value)
                if key == "target":
                    target = str(parsed_value)
                    model_aliases[current_alias] = target
                    model_alias_configs[current_alias].target = target
                continue
            if (
                indent == 6
                and current_alias is not None
                and current_alias_subsection == "surface"
                and stripped in {"modalities:", "tools:"}
            ):
                current_alias_surface_section = stripped[:-1]
                continue
            if (
                indent == 8
                and current_alias is not None
                and current_alias_subsection == "surface"
                and current_alias_surface_section is not None
            ):
                key, value = stripped.split(":", 1)
                parsed_value = parse_scalar(value)
                surface = model_alias_configs[current_alias].surface
                if surface is None:
                    surface = SurfaceMetadata()
                    model_alias_configs[current_alias].surface = surface
                _parse_surface_metadata_value(
                    surface,
                    current_alias_surface_section,
                    key,
                    value,
                    parsed_value,
                )
                continue
            if (
                indent == 6
                and current_alias is not None
                and current_alias_subsection == "limits"
            ):
                key, value = stripped.split(":", 1)
                limits = model_alias_configs[current_alias].limits
                if limits is None:
                    limits = ModelLimits()
                    model_alias_configs[current_alias].limits = limits
                parsed_value = parse_scalar(value)
                if key == "context_window":
                    limits.context_window = int(parsed_value)
                elif key == "max_output_tokens":
                    limits.max_output_tokens = int(parsed_value)
                continue
            if (
                indent == 6
                and current_alias is not None
                and current_alias_subsection == "codex"
            ):
                key, value = stripped.split(":", 1)
                codex_metadata = model_alias_configs[current_alias].codex_metadata
                if codex_metadata is None:
                    codex_metadata = CodexModelMetadata()
                    model_alias_configs[current_alias].codex_metadata = codex_metadata
                parsed_value = parse_scalar(value)
                _parse_codex_metadata_value(
                    codex_metadata,
                    key,
                    value,
                    parsed_value,
                )
                continue

        if section == "debug_trace" and indent == 2:
            key, value = stripped.split(":", 1)
            debug_trace[key] = parse_scalar(value)

    return ProxySourceConfig(
        listen=listen,
        proxy=proxy,
        upstream_timeout_secs=upstream_timeout_secs,
        upstreams=upstreams,
        upstream_limits=upstream_limits,
        upstream_surface_defaults=upstream_surface_defaults,
        upstream_codex_metadata=upstream_codex_metadata,
        model_aliases=model_aliases,
        model_alias_configs=model_alias_configs,
        debug_trace=debug_trace,
        top_level_sections=_split_top_level_sections(text),
        raw_text=text,
    )


def _has_nonempty_env(dotenv_env: dict[str, str], key: str) -> bool:
    return bool(dotenv_env.get(key, "").strip())


def has_local_qwen(dotenv_env: dict[str, str]) -> bool:
    return all(
        _has_nonempty_env(dotenv_env, key)
        for key in (
            "LOCAL_QWEN_BASE_URL",
            "LOCAL_QWEN_MODEL",
            "LOCAL_QWEN_API_KEY",
        )
    )


def _primary_lane_specs() -> tuple[tuple[str, bool, str], ...]:
    return (
        (
            PRESET_ANTHROPIC_COMPATIBLE_LANE,
            True,
            PRESET_ANTHROPIC_COMPATIBLE_UPSTREAM,
        ),
        (
            PRESET_OPENAI_COMPATIBLE_LANE,
            True,
            PRESET_OPENAI_COMPATIBLE_UPSTREAM,
        ),
    )


def _preset_upstream_base_url_envs() -> tuple[tuple[str, str], ...]:
    return (
        (
            PRESET_OPENAI_COMPATIBLE_UPSTREAM,
            PRESET_OPENAI_ENDPOINT_BASE_URL_ENV,
        ),
        (
            PRESET_ANTHROPIC_COMPATIBLE_UPSTREAM,
            PRESET_ANTHROPIC_ENDPOINT_BASE_URL_ENV,
        ),
    )


def _preset_upstream_names() -> set[str]:
    return {upstream_name for upstream_name, _env_key in _preset_upstream_base_url_envs()}


def _append_unique(values: list[str], value: str) -> None:
    if value not in values:
        values.append(value)


def required_preset_endpoint_env_keys(config: ProxySourceConfig) -> tuple[str, ...]:
    required: list[str] = []
    for upstream_name, base_url_env in _preset_upstream_base_url_envs():
        upstream = config.upstreams.get(upstream_name)
        if upstream is None:
            continue
        if upstream.get("api_root") == base_url_env:
            _append_unique(required, base_url_env)
        if upstream.get("provider_key_env") == PRESET_ENDPOINT_API_KEY_ENV:
            _append_unique(required, PRESET_ENDPOINT_API_KEY_ENV)

    preset_upstreams = _preset_upstream_names()
    for alias_config in config.model_alias_configs.values():
        target = alias_config.target
        if ":" not in target:
            continue
        upstream_name, upstream_model = target.split(":", 1)
        if (
            upstream_name in preset_upstreams
            and upstream_model == PRESET_ENDPOINT_MODEL_ENV
        ):
            _append_unique(required, PRESET_ENDPOINT_MODEL_ENV)

    return tuple(required)


def validate_preset_endpoint_env(
    config: ProxySourceConfig, dotenv_env: dict[str, str]
) -> None:
    missing = [
        key
        for key in required_preset_endpoint_env_keys(config)
        if not dotenv_env.get(key, "").strip()
    ]
    if missing:
        raise ValueError(
            "missing required provider-neutral preset environment variables: "
            + ", ".join(missing)
        )


def merge_preset_endpoint_env(
    config: ProxySourceConfig,
    dotenv_env: dict[str, str],
    base_env: dict[str, str],
) -> dict[str, str]:
    merged = dict(dotenv_env)
    for key in required_preset_endpoint_env_keys(config):
        if base_env.get(key):
            merged[key] = base_env[key]
    return merged


def resolve_proxy_key(base_env: dict[str, str], dotenv_env: dict[str, str] | None = None) -> str:
    dotenv_env = dotenv_env or {}
    for env_source in (base_env, dotenv_env):
        value = env_source.get(PROXY_KEY_ENV, "")
        if value.strip():
            return value
    return DEFAULT_PROXY_KEY


def _hydrate_preset_endpoint_model_target(
    target: str, dotenv_env: dict[str, str]
) -> str:
    if ":" not in target:
        return target
    upstream_name, upstream_model = target.split(":", 1)
    if (
        upstream_name in _preset_upstream_names()
        and upstream_model == PRESET_ENDPOINT_MODEL_ENV
    ):
        preset_model = dotenv_env.get(PRESET_ENDPOINT_MODEL_ENV, "")
        if preset_model:
            return f"{upstream_name}:{preset_model}"
    return target


def resolve_lanes(
    config: ProxySourceConfig,
    dotenv_env: dict[str, str],
    *,
    require_preset_endpoint_env: bool = True,
) -> list[Lane]:
    if require_preset_endpoint_env:
        validate_preset_endpoint_env(config, dotenv_env)
    lane_specs = _primary_lane_specs() + (
        ("qwen-local", False, "LOCAL-QWEN"),
    )
    lanes: list[Lane] = []
    for lane_name, required, default_upstream in lane_specs:
        alias_value = config.model_aliases.get(lane_name)
        if alias_value is None and lane_name == "qwen-local" and has_local_qwen(dotenv_env):
            alias_value = f"LOCAL-QWEN:{dotenv_env['LOCAL_QWEN_MODEL']}"
        if alias_value is not None:
            alias_value = _hydrate_preset_endpoint_model_target(alias_value, dotenv_env)
        upstream_name = default_upstream
        upstream_model = None
        if alias_value and ":" in alias_value:
            upstream_name, upstream_model = alias_value.split(":", 1)
        limits = resolve_model_limits(config, lane_name)
        codex_metadata = resolve_codex_model_metadata(config, lane_name)

        enabled = upstream_name in config.upstreams
        skip_reason = None

        if lane_name == "qwen-local" and has_local_qwen(dotenv_env):
            enabled = True
            upstream_name = "LOCAL-QWEN"
            upstream_model = dotenv_env["LOCAL_QWEN_MODEL"]
            if codex_metadata is None:
                codex_metadata = _codex_metadata_from_surface(
                    _copy_surface_metadata(DEFAULT_PROXY_SURFACE_METADATA)
                )

        if not enabled:
            if lane_name == "qwen-local":
                skip_reason = (
                    "LOCAL_QWEN_BASE_URL, LOCAL_QWEN_MODEL, and LOCAL_QWEN_API_KEY "
                    "are not all configured; "
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
                limits=limits,
                codex_metadata=codex_metadata,
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
    for upstream_name, base_url_env in _preset_upstream_base_url_envs():
        base_url = dotenv_env.get(base_url_env)
        if base_url and upstream_name in upstreams:
            upstreams[upstream_name]["api_root"] = base_url
    if has_local_qwen(dotenv_env):
        qwen_upstream = collections.OrderedDict(
            [
                ("api_root", dotenv_env["LOCAL_QWEN_BASE_URL"]),
                ("format", "openai-completion"),
                ("provider_key_env", "LOCAL_QWEN_API_KEY"),
            ]
        )
        upstreams["LOCAL-QWEN"] = qwen_upstream
    return upstreams


def _runtime_upstream_surface_defaults(
    config: ProxySourceConfig, dotenv_env: dict[str, str]
) -> collections.OrderedDict[str, SurfaceMetadata]:
    surface_defaults = collections.OrderedDict(
        (name, _copy_surface_metadata(surface))
        for name, surface in config.upstream_surface_defaults.items()
        if surface is not None
    )
    if has_local_qwen(dotenv_env) and "LOCAL-QWEN" not in surface_defaults:
        surface_defaults["LOCAL-QWEN"] = _copy_surface_metadata(
            DEFAULT_PROXY_SURFACE_METADATA
        )
    return surface_defaults


def _render_model_limits(lines: list[str], indent: str, limits: ModelLimits | None) -> None:
    if limits is None:
        return
    lines.append(f"{indent}limits:")
    if limits.context_window is not None:
        lines.append(f"{indent}  context_window: {limits.context_window}")
    if limits.max_output_tokens is not None:
        lines.append(f"{indent}  max_output_tokens: {limits.max_output_tokens}")


def _render_surface_metadata(
    lines: list[str],
    indent: str,
    section_name: str,
    surface: SurfaceMetadata | None,
) -> None:
    if surface is None:
        return
    if (
        surface.input_modalities is None
        and surface.output_modalities is None
        and surface.supports_search is None
        and surface.supports_view_image is None
        and surface.apply_patch_transport is None
        and surface.supports_parallel_calls is None
    ):
        return
    lines.append(f"{indent}{section_name}:")
    if surface.input_modalities is not None or surface.output_modalities is not None:
        lines.append(f"{indent}  modalities:")
    if surface.input_modalities is not None:
        lines.append(
            f"{indent}    input: {render_scalar(list(surface.input_modalities))}"
        )
    if surface.output_modalities is not None:
        lines.append(
            f"{indent}    output: {render_scalar(list(surface.output_modalities))}"
        )
    if (
        surface.supports_search is not None
        or surface.supports_view_image is not None
        or surface.apply_patch_transport is not None
        or surface.supports_parallel_calls is not None
    ):
        lines.append(f"{indent}  tools:")
    if surface.supports_search is not None:
        lines.append(
            f"{indent}    supports_search: {render_scalar(surface.supports_search)}"
        )
    if surface.supports_view_image is not None:
        lines.append(
            f"{indent}    supports_view_image: {render_scalar(surface.supports_view_image)}"
        )
    if surface.apply_patch_transport is not None:
        lines.append(
            f"{indent}    apply_patch_transport: {render_scalar(surface.apply_patch_transport)}"
        )
    if surface.supports_parallel_calls is not None:
        lines.append(
            f"{indent}    supports_parallel_calls: {render_scalar(surface.supports_parallel_calls)}"
        )


def _render_codex_metadata(
    lines: list[str],
    indent: str,
    codex_metadata: CodexModelMetadata | None,
) -> None:
    if codex_metadata is None:
        return
    if (
        codex_metadata.input_modalities is None
        and codex_metadata.supports_search_tool is None
        and codex_metadata.supports_view_image is None
        and codex_metadata.apply_patch_tool_type is None
        and codex_metadata.supports_parallel_tool_calls is None
    ):
        return
    lines.append(f"{indent}codex:")
    if codex_metadata.input_modalities is not None:
        lines.append(
            f"{indent}  input_modalities: {render_scalar(list(codex_metadata.input_modalities))}"
        )
    if codex_metadata.supports_search_tool is not None:
        lines.append(
            f"{indent}  supports_search_tool: {render_scalar(codex_metadata.supports_search_tool)}"
        )
    if codex_metadata.supports_view_image is not None:
        lines.append(
            f"{indent}  supports_view_image: {render_scalar(codex_metadata.supports_view_image)}"
        )
    if codex_metadata.apply_patch_tool_type is not None:
        lines.append(
            f"{indent}  apply_patch_tool_type: {render_scalar(codex_metadata.apply_patch_tool_type)}"
        )
    if codex_metadata.supports_parallel_tool_calls is not None:
        lines.append(
            f"{indent}  supports_parallel_tool_calls: {render_scalar(codex_metadata.supports_parallel_tool_calls)}"
        )


def _target_upstream_name(target: str) -> str | None:
    if ":" not in target:
        return None
    upstream_name, _ = target.split(":", 1)
    return upstream_name or None


def _merged_codex_metadata(
    base: CodexModelMetadata | None, override: CodexModelMetadata | None
) -> CodexModelMetadata | None:
    if base is None:
        return override
    return base.merged_with(override)


def _effective_surface_metadata(
    upstream_surface: SurfaceMetadata | None,
    alias_surface: SurfaceMetadata | None = None,
) -> SurfaceMetadata | None:
    if upstream_surface is None:
        return alias_surface
    return upstream_surface.merged_with(alias_surface)


def _copy_surface_metadata(surface: SurfaceMetadata | None) -> SurfaceMetadata | None:
    if surface is None:
        return None
    return dataclasses.replace(surface)


def _codex_metadata_from_surface(
    surface: SurfaceMetadata | None,
) -> CodexModelMetadata | None:
    if surface is None:
        return None
    return surface.to_codex_metadata()


def resolve_model_limits(
    config: ProxySourceConfig, model_name: str
) -> ModelLimits | None:
    alias_config = config.model_alias_configs.get(model_name)
    if alias_config is None:
        upstream_name = _target_upstream_name(model_name)
        if upstream_name is None:
            return None
        return config.upstream_limits.get(upstream_name)

    upstream_name = _target_upstream_name(alias_config.target)
    base_limits = (
        config.upstream_limits.get(upstream_name)
        if upstream_name is not None
        else None
    )
    if base_limits is None:
        return alias_config.limits
    return base_limits.merged_with(alias_config.limits)


def resolve_codex_model_metadata(
    config: ProxySourceConfig, model_name: str
) -> CodexModelMetadata | None:
    alias_config = config.model_alias_configs.get(model_name)
    if alias_config is None:
        upstream_name = _target_upstream_name(model_name)
        if upstream_name is None:
            return None
        legacy_metadata = config.upstream_codex_metadata.get(upstream_name)
        surface_metadata = _codex_metadata_from_surface(
            _effective_surface_metadata(config.upstream_surface_defaults.get(upstream_name))
        )
        metadata = _merged_codex_metadata(
            legacy_metadata,
            surface_metadata,
        )
        return metadata if metadata is not None else DEFAULT_PROXY_CODEX_METADATA

    upstream_name = _target_upstream_name(alias_config.target)
    surface_metadata = _codex_metadata_from_surface(
        _effective_surface_metadata(
            config.upstream_surface_defaults.get(upstream_name)
            if upstream_name is not None
            else None,
            alias_config.surface,
        )
    )
    legacy_metadata = _merged_codex_metadata(
        config.upstream_codex_metadata.get(upstream_name)
        if upstream_name is not None
        else None,
        alias_config.codex_metadata,
    )
    metadata = _merged_codex_metadata(
        legacy_metadata,
        surface_metadata,
    )
    return metadata if metadata is not None else DEFAULT_PROXY_CODEX_METADATA


def _runtime_alias_configs(
    config: ProxySourceConfig, dotenv_env: dict[str, str]
) -> collections.OrderedDict[str, ParsedModelAlias]:
    aliases: collections.OrderedDict[str, ParsedModelAlias] = collections.OrderedDict()
    qwen_enabled = has_local_qwen(dotenv_env)
    qwen_model = dotenv_env.get("LOCAL_QWEN_MODEL", "")

    for alias_name, alias_config in config.model_alias_configs.items():
        target = _hydrate_preset_endpoint_model_target(alias_config.target, dotenv_env)
        if target.startswith("LOCAL-QWEN:"):
            if not qwen_enabled:
                continue
            aliases[alias_name] = ParsedModelAlias(
                target=f"LOCAL-QWEN:{qwen_model}",
                limits=alias_config.limits,
                surface=alias_config.surface,
                codex_metadata=alias_config.codex_metadata,
            )
            continue
        aliases[alias_name] = ParsedModelAlias(
            target=target,
            limits=alias_config.limits,
            surface=alias_config.surface,
            codex_metadata=alias_config.codex_metadata,
        )

    if qwen_enabled:
        aliases["qwen-local"] = ParsedModelAlias(target=f"LOCAL-QWEN:{qwen_model}")

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


def _render_mapping_entries(
    lines: list[str],
    indent: str,
    values: collections.abc.Mapping[str, object],
) -> None:
    for key, value in values.items():
        if isinstance(value, dict):
            lines.append(f"{indent}{key}:")
            _render_mapping_entries(lines, f"{indent}  ", value)
            continue
        lines.append(f"{indent}{key}: {render_scalar(value)}")


def _render_runtime_listen_section(listen_host: str, listen_port: int) -> list[str]:
    return [f"listen: {listen_host}:{listen_port}"]


def _render_runtime_proxy_section(config: ProxySourceConfig) -> list[str]:
    if config.proxy is None:
        return []
    if isinstance(config.proxy, dict):
        lines = ["proxy:"]
        _render_mapping_entries(lines, "  ", config.proxy)
        return lines
    return [f"proxy: {render_scalar(config.proxy)}"]


def _render_runtime_timeout_section(config: ProxySourceConfig) -> list[str]:
    if config.upstream_timeout_secs is None:
        return []
    return [f"upstream_timeout_secs: {config.upstream_timeout_secs}"]


def _render_runtime_upstreams_section(
    config: ProxySourceConfig, dotenv_env: dict[str, str]
) -> list[str]:
    lines = ["upstreams:"]
    runtime_surface_defaults = _runtime_upstream_surface_defaults(config, dotenv_env)
    for upstream_name, values in _runtime_upstreams(config, dotenv_env).items():
        lines.append(f"  {upstream_name}:")
        _render_mapping_entries(lines, "    ", values)
        _render_model_limits(lines, "    ", config.upstream_limits.get(upstream_name))
        _render_surface_metadata(
            lines,
            "    ",
            "surface_defaults",
            runtime_surface_defaults.get(upstream_name),
        )
        _render_codex_metadata(
            lines,
            "    ",
            config.upstream_codex_metadata.get(upstream_name),
        )
    return lines


def _render_runtime_aliases_section(
    config: ProxySourceConfig, dotenv_env: dict[str, str]
) -> list[str]:
    lines = ["model_aliases:"]
    for alias_name, alias_config in _runtime_alias_configs(config, dotenv_env).items():
        if (
            alias_config.limits is None
            and alias_config.surface is None
            and alias_config.codex_metadata is None
        ):
            lines.append(f"  {alias_name}: {json.dumps(alias_config.target)}")
            continue
        lines.append(f"  {alias_name}:")
        lines.append(f"    target: {json.dumps(alias_config.target)}")
        _render_model_limits(lines, "    ", alias_config.limits)
        _render_surface_metadata(lines, "    ", "surface", alias_config.surface)
        _render_codex_metadata(lines, "    ", alias_config.codex_metadata)
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
    validate_preset_endpoint_env(config, dotenv_env)
    section_renderers = collections.OrderedDict(
        [
            ("listen", lambda: _render_runtime_listen_section(listen_host, listen_port)),
            ("proxy", lambda: _render_runtime_proxy_section(config)),
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


def _prompt_template_placeholder_token(field_name: str | None) -> str:
    if field_name in (None, ""):
        return "{}"
    return "{" + field_name + "}"


def validate_prompt_template(fixture_id: str, prompt_template: str) -> None:
    try:
        parsed_fields = list(string.Formatter().parse(prompt_template))
    except ValueError as error:
        raise ValueError(
            f"invalid prompt_template for fixture {fixture_id!r}: {error}"
        ) from error

    supported_placeholders = ", ".join(
        "{" + field_name + "}" for field_name in sorted(SUPPORTED_PROMPT_TEMPLATE_FIELDS)
    )

    for _, field_name, format_spec, conversion in parsed_fields:
        if field_name is None:
            continue
        if field_name not in SUPPORTED_PROMPT_TEMPLATE_FIELDS:
            raise ValueError(
                f"invalid prompt_template for fixture {fixture_id!r}: unsupported placeholder "
                f"{_prompt_template_placeholder_token(field_name)}; supported placeholders: "
                f"{supported_placeholders}"
            )
        if conversion is not None or format_spec:
            raise ValueError(
                f"invalid prompt_template for fixture {fixture_id!r}: placeholder "
                f"{_prompt_template_placeholder_token(field_name)} must not use conversion "
                "or format specifiers"
            )


def load_fixtures(fixtures_root: pathlib.Path) -> list[TaskFixture]:
    fixtures: list[TaskFixture] = []
    for path in sorted(fixtures_root.rglob("*.json")):
        payload = json.loads(path.read_text(encoding="utf-8"))
        workspace_template = payload.get("workspace_template")
        try:
            fixtures.append(
                TaskFixture(
                    fixture_id=payload["id"],
                    kind=payload["kind"],
                    description=payload.get("description", ""),
                    prompt=payload["prompt"],
                    prompt_template=payload.get("prompt_template"),
                    verifier=payload["verifier"],
                    timeout_secs=int(payload["timeout_secs"]),
                    workspace_template=(
                        (path.parent / workspace_template).resolve()
                        if workspace_template
                        else None
                    ),
                    supported_clients=tuple(
                        str(client_name)
                        for client_name in payload.get("supported_clients", [])
                    ),
                    unsupported_lanes=tuple(
                        str(lane_name) for lane_name in payload.get("unsupported_lanes", [])
                    ),
                )
            )
        except ValueError as error:
            message = str(error)
            if "invalid prompt_template" in message:
                raise ValueError(
                    f"invalid prompt_template in fixture {path}: {message}"
                ) from error
            raise ValueError(f"invalid fixture {path}: {message}") from error
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
    if lane.name in fixture.unsupported_lanes:
        return False
    if lane.name == "qwen-local" and fixture.kind == "long_horizon":
        return False
    return True


def client_supports_fixture(client_name: str, fixture: TaskFixture) -> bool:
    if not fixture.supported_clients:
        return True
    return client_name in fixture.supported_clients


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
                if not client_supports_fixture(client_name, fixture):
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


def _resolve_host_rust_toolchain_env(base_env: dict[str, str]) -> dict[str, str]:
    resolved: dict[str, str] = {}
    for key in RUST_TOOLCHAIN_ENV_KEYS:
        value = base_env.get(key)
        if value:
            resolved[key] = value

    if len(resolved) == len(RUST_TOOLCHAIN_ENV_KEYS):
        return resolved

    host_home_value = base_env.get("HOME")
    host_home = pathlib.Path(host_home_value).expanduser() if host_home_value else pathlib.Path.home()
    default_locations = {
        "CARGO_HOME": host_home / ".cargo",
        "RUSTUP_HOME": host_home / ".rustup",
    }
    for key, candidate in default_locations.items():
        if key not in resolved and candidate.is_dir():
            resolved[key] = str(candidate)
    return resolved


def codex_available_input_budget(
    context_window: int, max_output_tokens: int | None = None
) -> int:
    if max_output_tokens is None:
        return context_window
    if max_output_tokens >= context_window:
        raise ValueError(
            "max_output_tokens must be less than context_window for Codex auto compact budgeting"
        )
    return context_window - max_output_tokens


def default_auto_compact_token_limit(
    context_window: int, max_output_tokens: int | None = None
) -> int:
    return int(
        codex_available_input_budget(context_window, max_output_tokens)
        * DEFAULT_AUTO_COMPACT_RATIO
    )


def codex_model_catalog_path(home_dir: pathlib.Path) -> pathlib.Path:
    return pathlib.Path(home_dir) / ".codex" / "catalog.json"


def gemini_settings_path(home_dir: pathlib.Path) -> pathlib.Path:
    return pathlib.Path(home_dir) / ".gemini" / "settings.json"


def replay_marker_key_path(runtime_root: pathlib.Path) -> pathlib.Path:
    return pathlib.Path(runtime_root) / REPLAY_MARKER_KEY_FILENAME


def build_codex_model_catalog(
    model_name: str,
    model_limits: ModelLimits | None,
    codex_metadata: CodexModelMetadata | None = None,
) -> dict[str, object] | None:
    has_catalog_fields = (
        codex_metadata is not None
        or (model_limits is not None and model_limits.context_window is not None)
    )
    if not has_catalog_fields:
        return None

    model_entry = default_codex_catalog_entry(model_name)
    if model_limits is not None and model_limits.context_window is not None:
        context_window = model_limits.context_window
        model_entry["context_window"] = context_window
        model_entry["auto_compact_token_limit"] = default_auto_compact_token_limit(
            context_window,
            model_limits.max_output_tokens,
        )
    if codex_metadata is not None:
        if codex_metadata.input_modalities is not None:
            model_entry["input_modalities"] = list(codex_metadata.input_modalities)
        if codex_metadata.supports_search_tool is not None:
            model_entry["supports_search_tool"] = codex_metadata.supports_search_tool
        public_apply_patch_tool_type = validate_public_apply_patch_tool_type(
            codex_metadata.apply_patch_tool_type
        )
        if public_apply_patch_tool_type is not None:
            model_entry["apply_patch_tool_type"] = public_apply_patch_tool_type
        if codex_metadata.supports_parallel_tool_calls is not None:
            model_entry["supports_parallel_tool_calls"] = (
                codex_metadata.supports_parallel_tool_calls
            )
    payload = {
        "models": [
            model_entry
        ]
    }
    ensure_no_public_internal_tool_artifacts(payload, context="codex model catalog")
    return payload


def codex_should_disable_view_image(
    codex_metadata: CodexModelMetadata | None,
) -> bool:
    if codex_metadata is None:
        return False
    if codex_metadata.supports_view_image is not None:
        return not codex_metadata.supports_view_image
    if codex_metadata.input_modalities is None:
        return False
    return "image" not in {
        modality.strip().lower() for modality in codex_metadata.input_modalities
    }


def ensure_codex_model_catalog(
    home_dir: pathlib.Path,
    model_name: str,
    model_limits: ModelLimits | None,
    codex_metadata: CodexModelMetadata | None = None,
) -> pathlib.Path | None:
    payload = build_codex_model_catalog(model_name, model_limits, codex_metadata)
    if payload is None:
        return None
    catalog_path = codex_model_catalog_path(home_dir)
    catalog_path.parent.mkdir(parents=True, exist_ok=True)
    catalog_path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    return catalog_path


def build_codex_catalog_args(
    home_dir: pathlib.Path | None,
    model_name: str,
    model_limits: ModelLimits | None,
    codex_metadata: CodexModelMetadata | None = None,
) -> list[str]:
    if home_dir is None:
        return []
    catalog_path = ensure_codex_model_catalog(
        home_dir,
        model_name,
        model_limits,
        codex_metadata,
    )
    if catalog_path is None:
        return []
    args = [
        "-c",
        f'model_catalog_json="{catalog_path}"',
    ]
    if codex_metadata is not None and codex_metadata.supports_search_tool is False:
        args.extend(
            [
                "-c",
                'web_search="disabled"',
            ]
        )
    if codex_should_disable_view_image(codex_metadata):
        args.extend(
            [
                "-c",
                "tools.view_image=false",
            ]
        )
    ensure_no_public_internal_tool_artifacts(args, context="codex catalog args")
    return args


def build_gemini_settings_payload(
    model_name: str, model_limits: ModelLimits | None
) -> dict[str, object] | None:
    if model_limits is None:
        return None

    override_generate_content_config: dict[str, object] = {}
    if model_limits.max_output_tokens is not None:
        override_generate_content_config["maxOutputTokens"] = (
            model_limits.max_output_tokens
        )
    if not override_generate_content_config and model_limits.context_window is None:
        return None

    model_definition: dict[str, object] = {
        "displayName": model_name,
        "tier": "custom",
        "family": "proxy",
        "isPreview": False,
        "isVisible": True,
        "features": {
            "thinking": True,
            "multimodalToolUse": False,
        },
    }
    if model_limits.context_window is not None:
        model_definition["dialogDescription"] = (
            f"Proxy-backed model with about {model_limits.context_window} tokens of context."
        )

    payload: dict[str, object] = {
        "model": {
            "compressionThreshold": DEFAULT_GEMINI_COMPRESSION_THRESHOLD,
        },
        "modelConfigs": {
            "modelDefinitions": {model_name: model_definition},
        },
    }
    if override_generate_content_config:
        payload["modelConfigs"]["customOverrides"] = [
            {
                "match": {"model": model_name},
                "modelConfig": {
                    "model": model_name,
                    "generateContentConfig": override_generate_content_config,
                },
            }
        ]
    return payload


def ensure_gemini_settings(
    home_dir: pathlib.Path, model_name: str, model_limits: ModelLimits | None
) -> pathlib.Path | None:
    payload = build_gemini_settings_payload(model_name, model_limits)
    if payload is None:
        return None
    settings_path = gemini_settings_path(home_dir)
    settings_path.parent.mkdir(parents=True, exist_ok=True)
    settings_path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    return settings_path


def ensure_replay_marker_key(runtime_root: pathlib.Path) -> str:
    key_path = replay_marker_key_path(runtime_root)
    if key_path.exists():
        existing_key = key_path.read_text(encoding="utf-8").strip()
        if existing_key:
            return existing_key
    marker_key = secrets.token_hex(32)
    key_path.write_text(marker_key + "\n", encoding="utf-8")
    return marker_key


def add_timeout_policy_args(
    parser: argparse.ArgumentParser, *, include_case_thresholds: bool
) -> None:
    parser.add_argument(
        "--proxy-health-timeout-secs",
        type=int,
        default=DEFAULT_TIMEOUT_POLICY.proxy_health_timeout_secs,
    )
    parser.add_argument(
        "--process-stop-grace-secs",
        type=int,
        default=DEFAULT_TIMEOUT_POLICY.process_terminate_grace_secs,
    )
    if include_case_thresholds:
        parser.add_argument(
            "--case-timeout-floor-secs",
            type=int,
            default=DEFAULT_TIMEOUT_POLICY.case_timeout_floor_secs,
        )
        parser.add_argument(
            "--long-horizon-timeout-floor-secs",
            type=int,
            default=DEFAULT_TIMEOUT_POLICY.long_horizon_timeout_floor_secs,
        )
        parser.add_argument(
            "--gemini-bootstrap-timeout-secs",
            type=int,
            default=DEFAULT_TIMEOUT_POLICY.gemini_bootstrap_timeout_secs,
        )


def timeout_policy_from_args(args: argparse.Namespace) -> TimeoutPolicy:
    return TimeoutPolicy(
        proxy_health_timeout_secs=int(args.proxy_health_timeout_secs),
        case_timeout_floor_secs=int(
            getattr(args, "case_timeout_floor_secs", DEFAULT_TIMEOUT_POLICY.case_timeout_floor_secs)
        ),
        long_horizon_timeout_floor_secs=int(
            getattr(
                args,
                "long_horizon_timeout_floor_secs",
                DEFAULT_TIMEOUT_POLICY.long_horizon_timeout_floor_secs,
            )
        ),
        gemini_bootstrap_timeout_secs=int(
            getattr(
                args,
                "gemini_bootstrap_timeout_secs",
                DEFAULT_TIMEOUT_POLICY.gemini_bootstrap_timeout_secs,
            )
        ),
        process_terminate_grace_secs=int(args.process_stop_grace_secs),
    )


def build_client_env(
    client_name: str,
    base_env: dict[str, str],
    proxy_base: str,
    home_dir: pathlib.Path,
    model_name: str | None = None,
    model_limits: ModelLimits | None = None,
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
    env.update(_resolve_host_rust_toolchain_env(base_env))
    proxy_key = resolve_proxy_key(base_env)

    if client_name == "codex":
        codex_home = home_dir / ".codex"
        codex_home.mkdir(parents=True, exist_ok=True)
        env.update(
            {
                "CODEX_HOME": str(codex_home),
                "OPENAI_API_KEY": proxy_key,
                "OPENAI_BASE_URL": f"{proxy_base}/openai/v1",
            }
        )
    elif client_name == "claude":
        claude_dir = home_dir / ".claude"
        claude_dir.mkdir(parents=True, exist_ok=True)
        env.update(
            {
                "CLAUDE_CONFIG_DIR": str(claude_dir),
                "ANTHROPIC_API_KEY": proxy_key,
                "ANTHROPIC_BASE_URL": f"{proxy_base}/anthropic",
            }
        )
    elif client_name == "gemini":
        env.update(
            {
                "GEMINI_API_KEY": proxy_key,
                "GOOGLE_GEMINI_BASE_URL": f"{proxy_base}/google",
            }
        )
        if model_name is not None:
            ensure_gemini_settings(home_dir, model_name, model_limits)
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


def _log_tail(log_path: pathlib.Path | None, max_chars: int = 4000) -> str:
    if log_path is None:
        return ""
    try:
        text = pathlib.Path(log_path).read_text(
            encoding="utf-8",
            errors="replace",
        )
    except OSError:
        return ""
    return text[-max_chars:].strip()


def _stderr_tail(stderr_path: pathlib.Path | None, max_chars: int = 4000) -> str:
    return _log_tail(stderr_path, max_chars=max_chars)


def _stderr_tail_message(stderr_path: pathlib.Path | None) -> str:
    tail = _stderr_tail(stderr_path)
    if not tail:
        return ""
    return "; stderr tail: " + " ".join(tail.split())


def _proxy_log_tail_message(
    stdout_path: pathlib.Path | None,
    stderr_path: pathlib.Path | None,
) -> str:
    parts = []
    for label, log_path in (("stdout", stdout_path), ("stderr", stderr_path)):
        tail = _log_tail(log_path)
        if tail:
            parts.append(f"{label} tail: {' '.join(tail.split())}")
    if not parts:
        return ""
    return "; " + "; ".join(parts)


def _listen_addr_from_base_url(base_url: str) -> str:
    parsed = urllib.parse.urlsplit(base_url)
    try:
        port = parsed.port
    except ValueError:
        port = None
    host = parsed.hostname
    if host and port is not None:
        if ":" in host and not host.startswith("["):
            return f"[{host}]:{port}"
        return f"{host}:{port}"
    return parsed.netloc


def _strip_ansi_codes(text: str) -> str:
    return re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", text)


def _log_has_owned_listening_proof(log_text: str, listen_addr: str) -> bool:
    if not log_text or not listen_addr:
        return False
    escaped_addr = re.escape(listen_addr)
    proof_patterns = (
        rf"\blistening\s+(?:on|at)\s+{escaped_addr}\b",
        rf"\bbound\s+(?:on|to)\s+{escaped_addr}\b",
    )
    for line in _strip_ansi_codes(log_text).splitlines():
        for pattern in proof_patterns:
            if re.search(pattern, line, flags=re.IGNORECASE):
                return True
    return False


def _owned_listening_proof_seen(
    base_url: str,
    stdout_path: pathlib.Path | None,
    stderr_path: pathlib.Path | None,
) -> bool:
    listen_addr = _listen_addr_from_base_url(base_url)
    for log_path in (stdout_path, stderr_path):
        if _log_has_owned_listening_proof(
            _log_tail(log_path, max_chars=65536),
            listen_addr,
        ):
            return True
    return False


def _raise_if_proxy_process_exited(
    process: subprocess.Popen[str] | None,
    base_url: str,
    stdout_path: pathlib.Path | None,
    stderr_path: pathlib.Path | None,
) -> None:
    if process is None:
        return
    exit_code = process.poll()
    if exit_code is None:
        return
    raise RuntimeError(
        f"proxy process exited before becoming healthy at {base_url} "
        f"(exit code {exit_code}){_proxy_log_tail_message(stdout_path, stderr_path)}"
    )


def wait_for_health(
    base_url: str,
    timeout_secs: int = DEFAULT_TIMEOUT_POLICY.proxy_health_timeout_secs,
    *,
    process: subprocess.Popen[str] | None = None,
    stdout_path: pathlib.Path | None = None,
    stderr_path: pathlib.Path | None = None,
) -> None:
    deadline = time.time() + timeout_secs
    require_owned_ready = process is not None
    owned_listening_seen = not require_owned_ready
    while time.time() < deadline:
        _raise_if_proxy_process_exited(process, base_url, stdout_path, stderr_path)
        if require_owned_ready and not owned_listening_seen:
            owned_listening_seen = _owned_listening_proof_seen(
                base_url,
                stdout_path,
                stderr_path,
            )
            if not owned_listening_seen:
                time.sleep(0.2)
                continue
        try:
            with urllib.request.urlopen(f"{base_url}/health", timeout=2) as response:
                if response.status == 200:
                    _raise_if_proxy_process_exited(
                        process,
                        base_url,
                        stdout_path,
                        stderr_path,
                    )
                    return
        except Exception:
            time.sleep(0.2)
    _raise_if_proxy_process_exited(process, base_url, stdout_path, stderr_path)
    if require_owned_ready and not owned_listening_seen:
        listen_addr = _listen_addr_from_base_url(base_url)
        raise RuntimeError(
            f"proxy at {base_url} did not become healthy in time; "
            f"missing owned listening proof for {listen_addr}"
            f"{_proxy_log_tail_message(stdout_path, stderr_path)}"
        )
    raise RuntimeError(
        f"proxy at {base_url} did not become healthy in time"
        f"{_proxy_log_tail_message(stdout_path, stderr_path)}"
    )


def _auth_headers(bearer_token: str | None) -> dict[str, str]:
    if not bearer_token:
        return {}
    return {"Authorization": f"Bearer {bearer_token}"}


def http_get_json(
    url: str,
    timeout: int = 30,
    *,
    bearer_token: str | None = None,
) -> object:
    request = urllib.request.Request(
        url,
        headers=_auth_headers(bearer_token),
        method="GET",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = response.read().decode("utf-8")
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8")
        raise RuntimeError(f"GET {url} returned HTTP {error.code}: {body[:240]}")
    except urllib.error.URLError as error:
        raise RuntimeError(f"GET {url} failed: {error}") from error

    try:
        return json.loads(body)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"GET {url} returned invalid JSON: {error}") from error


def live_model_details_url(proxy_base: str, model_name: str) -> str:
    quoted_model = urllib.parse.quote(model_name, safe=":")
    return f"{normalize_proxy_base(proxy_base)}/openai/v1/models/{quoted_model}"


def _optional_int(value: object) -> int | None:
    if isinstance(value, bool) or not isinstance(value, int):
        return None
    return int(value)


def _optional_bool(value: object) -> bool | None:
    if isinstance(value, bool):
        return value
    return None


def _optional_string(value: object) -> str | None:
    if isinstance(value, str):
        return value
    return None


def _optional_string_tuple(value: object) -> tuple[str, ...] | None:
    if not isinstance(value, list):
        return None
    return tuple(str(item) for item in value)


def _model_limits_from_live_surface(
    surface_payload: dict[str, object],
) -> ModelLimits | None:
    limit_source = surface_payload.get("limits")
    if not isinstance(limit_source, dict):
        return None

    limits = ModelLimits(
        context_window=_optional_int(limit_source.get("context_window")),
        max_output_tokens=_optional_int(limit_source.get("max_output_tokens")),
    )
    if limits.context_window is None and limits.max_output_tokens is None:
        return None
    return limits


def _surface_metadata_from_live_surface(
    surface_payload: dict[str, object],
) -> SurfaceMetadata | None:
    modalities = surface_payload.get("modalities")
    tools = surface_payload.get("tools")
    surface = SurfaceMetadata(
        input_modalities=(
            _optional_string_tuple(modalities.get("input"))
            if isinstance(modalities, dict)
            else None
        ),
        output_modalities=(
            _optional_string_tuple(modalities.get("output"))
            if isinstance(modalities, dict)
            else None
        ),
        supports_search=(
            _optional_bool(tools.get("supports_search"))
            if isinstance(tools, dict)
            else None
        ),
        supports_view_image=(
            _optional_bool(tools.get("supports_view_image"))
            if isinstance(tools, dict)
            else None
        ),
        apply_patch_transport=(
            _optional_string(tools.get("apply_patch_transport"))
            if isinstance(tools, dict)
            else None
        ),
        supports_parallel_calls=(
            _optional_bool(tools.get("supports_parallel_calls"))
            if isinstance(tools, dict)
            else None
        ),
    )
    if surface.merged_with(None) is None:
        return None
    return surface


def _validate_live_surface_codex_requirements(
    surface: SurfaceMetadata | None,
    *,
    require_tool_flags: bool = False,
) -> None:
    missing: list[str] = []
    if surface is None or surface.input_modalities is None:
        missing.append("llmup.surface.modalities.input")
    if surface is None or surface.supports_search is None:
        missing.append("llmup.surface.tools.supports_search")
    if require_tool_flags and (surface is None or surface.supports_view_image is None):
        missing.append("llmup.surface.tools.supports_view_image")
    if require_tool_flags and (
        surface is None or surface.supports_parallel_calls is None
    ):
        missing.append("llmup.surface.tools.supports_parallel_calls")
    if missing:
        raise RuntimeError(
            "live model lookup omitted critical llmup surface fields: "
            + ", ".join(missing)
        )


def fetch_live_model_profile(
    proxy_base: str,
    model_name: str,
    timeout_secs: int = 30,
    *,
    proxy_key: str | None = None,
) -> LiveModelProfile:
    payload = http_get_json(
        live_model_details_url(proxy_base, model_name),
        timeout=timeout_secs,
        bearer_token=proxy_key,
    )
    if not isinstance(payload, dict):
        raise RuntimeError("live model lookup must return a JSON object")

    llmup_payload = payload.get("llmup")
    if not isinstance(llmup_payload, dict):
        raise RuntimeError("live model lookup did not include llmup surface metadata")

    surface_payload = llmup_payload.get("surface")
    if not isinstance(surface_payload, dict):
        raise RuntimeError("live model lookup did not include llmup.surface metadata")

    limits = _model_limits_from_live_surface(surface_payload)
    live_surface_metadata = _surface_metadata_from_live_surface(surface_payload)
    _validate_live_surface_codex_requirements(
        live_surface_metadata,
        require_tool_flags=True,
    )
    codex_metadata = (
        live_surface_metadata.to_codex_metadata()
        if live_surface_metadata is not None
        else None
    )

    return LiveModelProfile(
        limits=limits,
        codex_metadata=codex_metadata,
    )


def refresh_lane_model_profiles(
    proxy_base: str,
    lanes: Iterable[Lane],
    *,
    proxy_key: str | None = None,
) -> None:
    for lane in lanes:
        if not lane.enabled:
            continue
        profile = fetch_live_model_profile(
            proxy_base,
            lane.proxy_model,
            proxy_key=proxy_key,
        )
        lane.limits = profile.limits
        lane.codex_metadata = profile.codex_metadata


def http_json(
    url: str,
    payload: dict[str, object],
    timeout: int = 60,
    *,
    bearer_token: str | None = None,
) -> tuple[int, str]:
    headers = {"Content-Type": "application/json"}
    headers.update(_auth_headers(bearer_token))
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers=headers,
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


def probe_lane(
    proxy_base: str,
    lane: Lane,
    *,
    proxy_key: str | None = None,
) -> str | None:
    status, body = http_json(
        f"{proxy_base}/openai/v1/responses",
        {"model": lane.proxy_model, "input": "Reply with exactly PROBE_OK", "stream": False},
        timeout=60,
        bearer_token=proxy_key,
    )
    if status != 200:
        return f"lane probe returned HTTP {status}: {body[:240]}"
    if "PROBE_OK" in body or probe_response_has_valid_shape(body):
        return None
    return "lane probe succeeded but did not return a valid response shape"


def render_fixture_prompt(fixture: TaskFixture, client_name: str) -> str:
    if fixture.prompt_template is None:
        return fixture.prompt
    try:
        return fixture.prompt_template.format(client_name=client_name)
    except (KeyError, ValueError) as error:
        raise ValueError(
            f"invalid prompt_template for fixture {fixture.fixture_id!r}: {error}"
        ) from error


def client_stdin_text(client_name: str, fixture: TaskFixture) -> str | None:
    if client_name == "claude":
        return render_fixture_prompt(fixture, client_name)
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


def _verify_command_success(
    workspace_dir: pathlib.Path, command_spec: dict[str, object]
) -> tuple[bool, str]:
    raw_command = command_spec.get("command")
    if not isinstance(raw_command, list) or not raw_command:
        return False, "command_success requires a non-empty command list"

    command = [str(part) for part in raw_command]
    timeout_secs = int(command_spec.get("timeout_secs", 120))
    cwd = workspace_dir

    if "cwd" in command_spec:
        relative_cwd = pathlib.Path(str(command_spec["cwd"]))
        if relative_cwd.is_absolute():
            return False, "command_success cwd must be relative to the workspace"
        cwd = workspace_dir / relative_cwd
        if not _path_is_within(cwd, workspace_dir):
            return False, "command_success cwd must stay inside the workspace"
        if not cwd.is_dir():
            return False, f"expected command_success cwd {relative_cwd} to exist"

    env = os.environ.copy()
    for key, value in dict(command_spec.get("env", {})).items():
        env[str(key)] = str(value)

    try:
        completed = subprocess.run(
            command,
            cwd=str(cwd),
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=timeout_secs,
            check=False,
        )
    except FileNotFoundError:
        return False, f"command_success executable not found: {command[0]!r}"
    except subprocess.TimeoutExpired:
        command_text = " ".join(command)
        return False, f"expected command {command_text!r} to finish within {timeout_secs}s"

    stdout_text = completed.stdout or ""
    stderr_text = completed.stderr or ""
    if completed.returncode != 0:
        diagnostic = (stderr_text or stdout_text).strip()
        if diagnostic:
            return (
                False,
                f"expected command to exit 0, got {completed.returncode}: {diagnostic[:240]}",
            )
        return False, f"expected command to exit 0, got {completed.returncode}"

    for expected_text in command_spec.get("expect_stdout_contains", []):
        if str(expected_text) not in stdout_text:
            return False, f"expected command stdout to contain {expected_text!r}"
    for expected_text in command_spec.get("expect_stderr_contains", []):
        if str(expected_text) not in stderr_text:
            return False, f"expected command stderr to contain {expected_text!r}"
    return True, ""


def _parse_jsonl_events(stdout_text: str) -> tuple[list[dict[str, object]] | None, str]:
    events: list[dict[str, object]] = []
    for line_number, raw_line in enumerate(stdout_text.splitlines(), start=1):
        line = raw_line.strip()
        if not line:
            continue
        try:
            payload = json.loads(line)
        except json.JSONDecodeError as error:
            return None, f"expected JSONL event on line {line_number}: {error.msg}"
        if not isinstance(payload, dict):
            return None, f"expected JSON object on line {line_number}"
        events.append(payload)
    if not events:
        return None, "expected at least one JSONL event"
    return events, ""


def _completed_codex_items(
    events: Iterable[dict[str, object]],
) -> list[dict[str, object]]:
    completed_items: list[dict[str, object]] = []
    for event in events:
        if event.get("type") != "item.completed":
            continue
        item = event.get("item")
        if isinstance(item, dict):
            completed_items.append(item)
    return completed_items


def _codex_file_change_matches(
    item: dict[str, object], change_spec: dict[str, object]
) -> bool:
    if item.get("type") != "file_change":
        return False
    changes = item.get("changes")
    if not isinstance(changes, list):
        return False

    required_kind = change_spec.get("kind")
    path_suffix = change_spec.get("path_suffix")

    for change in changes:
        if not isinstance(change, dict):
            continue
        if required_kind is not None and change.get("kind") != required_kind:
            continue
        change_path = change.get("path")
        if path_suffix is not None:
            if not isinstance(change_path, str) or not change_path.endswith(
                str(path_suffix)
            ):
                continue
        return True
    return False


def _verify_codex_json_event_contract(
    stdout_text: str,
) -> tuple[list[dict[str, object]] | None, list[dict[str, object]] | None, str]:
    events, message = _parse_jsonl_events(stdout_text)
    if events is None:
        return None, None, message
    return events, _completed_codex_items(events), ""


def _codex_event_item(
    event: dict[str, object],
    *,
    event_types: tuple[str, ...],
) -> dict[str, object] | None:
    if event.get("type") not in event_types:
        return None
    item = event.get("item")
    if not isinstance(item, dict):
        return None
    return item


def _codex_phase_item_indexes(
    events: Iterable[dict[str, object]],
    *,
    event_types: tuple[str, ...],
    item_types: set[str],
) -> list[int]:
    indexes: list[int] = []
    for index, event in enumerate(events):
        item = _codex_event_item(event, event_types=event_types)
        if item is None:
            continue
        item_type = item.get("type")
        if isinstance(item_type, str) and item_type in item_types:
            indexes.append(index)
    return indexes


def _codex_shell_payload(command: str) -> str:
    # Shell startup can execute rc/env hooks. Without independent environment
    # evidence, an explicit shell wrapper is not a transparent read-only layer.
    return command


def _shell_command_tokens(command: str) -> list[str] | None:
    try:
        lexer = shlex.shlex(command, posix=True, punctuation_chars=True)
        lexer.whitespace_split = True
        lexer.commenters = ""
        return list(lexer)
    except ValueError:
        return None


def _shell_payload_has_command_substitution(payload: str) -> bool:
    return "`" in payload or "$(" in payload


def _shell_payload_has_parameter_expansion(payload: str) -> bool:
    index = 0
    quote: str | None = None
    while index < len(payload):
        char = payload[index]
        if quote == "'":
            if char == "'":
                quote = None
            index += 1
            continue
        if quote == '"':
            if char == "\\":
                index += 2
                continue
            if char == '"':
                quote = None
                index += 1
                continue
        else:
            if char == "\\":
                index += 2
                continue
            if char in {"'", '"'}:
                quote = char
                index += 1
                continue

        if char == "$":
            return True
        index += 1
    return False


def _shell_payload_has_unquoted_brace(payload: str) -> bool:
    index = 0
    quote: str | None = None
    while index < len(payload):
        char = payload[index]
        if quote == "'":
            if char == "'":
                quote = None
            index += 1
            continue
        if quote == '"':
            if char == "\\":
                index += 2
                continue
            if char == '"':
                quote = None
                index += 1
                continue
            if char in {"{", "}"}:
                return True
            index += 1
            continue
        if char == "\\":
            index += 2
            continue
        if char in {"'", '"'}:
            quote = char
            index += 1
            continue
        if char in {"{", "}"}:
            return True
        index += 1
    return False


def _shell_payload_has_unquoted_glob(payload: str) -> bool:
    index = 0
    quote: str | None = None
    while index < len(payload):
        char = payload[index]
        if quote == "'":
            if char == "'":
                quote = None
            index += 1
            continue
        if quote == '"':
            if char == "\\":
                index += 2
                continue
            if char == '"':
                quote = None
            index += 1
            continue
        if char == "\\":
            index += 2
            continue
        if char in {"'", '"'}:
            quote = char
            index += 1
            continue
        if char in {"*", "?", "["}:
            return True
        index += 1
    return False


_SHELL_OPERATOR_CONTROL_CHARS = frozenset(";&|(){}<>")


def _shell_payload_has_unquoted_chars(payload: str, chars: frozenset[str]) -> bool:
    index = 0
    quote: str | None = None
    while index < len(payload):
        char = payload[index]
        if quote == "'":
            if char == "'":
                quote = None
            index += 1
            continue
        if quote == '"':
            if char == "\\":
                index += 2
                continue
            if char == '"':
                quote = None
            index += 1
            continue
        if char == "\\":
            index += 2
            continue
        if char in {"'", '"'}:
            quote = char
            index += 1
            continue
        if char in chars:
            return True
        index += 1
    return False


def _shell_payload_has_unquoted_operator_or_control(payload: str) -> bool:
    return _shell_payload_has_unquoted_chars(payload, _SHELL_OPERATOR_CONTROL_CHARS)


def _shell_payload_has_unquoted_tilde(payload: str) -> bool:
    return _shell_payload_has_unquoted_chars(payload, frozenset({"~"}))


_ZSH_UNQUOTED_EXPANSION_CHARS = frozenset({"#", "^"})


def _shell_payload_has_unquoted_zsh_expansion(payload: str) -> bool:
    index = 0
    quote: str | None = None
    at_word_start = True
    while index < len(payload):
        char = payload[index]
        if quote == "'":
            if char == "'":
                quote = None
            else:
                at_word_start = False
            index += 1
            continue
        if quote == '"':
            if char == "\\":
                index += 2
                at_word_start = False
                continue
            if char == '"':
                quote = None
                index += 1
                continue
            at_word_start = False
            index += 1
            continue
        if char == "\\":
            index += 2
            at_word_start = False
            continue
        if char in {"'", '"'}:
            quote = char
            index += 1
            continue
        if char.isspace():
            at_word_start = True
            index += 1
            continue
        if at_word_start and char == "=":
            return True
        if char in _ZSH_UNQUOTED_EXPANSION_CHARS:
            return True
        at_word_start = False
        index += 1
    return False


def _tokens_have_shell_operator_or_control_token(tokens: Sequence[str]) -> bool:
    return any(
        bool(token) and all(char in _SHELL_OPERATOR_CONTROL_CHARS for char in token)
        for token in tokens
    )


_SHELL_REDIRECTION_OPERATOR_CHARS = frozenset("<>&|")


def _is_shell_redirection_operator(token: str) -> bool:
    return (
        any(char in token for char in "<>")
        and all(char in _SHELL_REDIRECTION_OPERATOR_CHARS for char in token)
    )


def _tokens_have_unsafe_redirection(tokens: Sequence[str]) -> bool:
    return any(_is_shell_redirection_operator(token) for token in tokens)


def _shell_command_segments(tokens: Sequence[str]) -> list[list[str]] | None:
    if _tokens_have_shell_operator_or_control_token(tokens):
        return None

    return [list(tokens)] if tokens else []


def _tokens_start_with_shell_assignment(tokens: Sequence[str]) -> bool:
    return (
        bool(tokens)
        and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", tokens[0]) is not None
    )


_TRUSTED_READ_ONLY_COMMAND_NAMES = frozenset(
    {
        "cat",
        "find",
        "grep",
        "head",
        "ls",
        "pwd",
        "rg",
        "sed",
        "stat",
        "tail",
        "wc",
    }
)
_TRUSTED_PYTHON_COMMAND_NAMES = frozenset({"python3"})
_TRUSTED_READ_ONLY_COMMAND_NAMES_BY_PATH = {
    f"{prefix}/{name}": name
    for prefix in ("/bin", "/usr/bin")
    for name in _TRUSTED_READ_ONLY_COMMAND_NAMES | _TRUSTED_PYTHON_COMMAND_NAMES
}


def _command_name(token: str) -> str:
    return _TRUSTED_READ_ONLY_COMMAND_NAMES_BY_PATH.get(token, "")


_SED_SAFE_PRINT_COMMAND_PATTERN = re.compile(
    r"^\s*(?:(?:\d+|\$)\s*(?:,\s*(?:\d+|\$)\s*)?)?[pP]\s*$"
)


def _sed_inline_script_is_read_only(script: str) -> bool:
    saw_command = False
    for raw_command in re.split(r"[;\n]+", script):
        command = raw_command.strip()
        if not command or command.startswith("#"):
            continue
        if not _SED_SAFE_PRINT_COMMAND_PATTERN.match(command):
            return False
        saw_command = True
    return saw_command


def _parse_sed_short_options(
    args: Sequence[str],
    index: int,
    scripts: list[str],
) -> tuple[bool, bool, int]:
    arg = args[index]
    saw_print_only = False
    option_chars = arg[1:]
    position = 0
    while position < len(option_chars):
        option = option_chars[position]
        if option == "i" or option == "f":
            return False, saw_print_only, index + 1
        if option == "n":
            saw_print_only = True
            position += 1
            continue
        if option in {"E", "r", "s", "u", "z", "b"}:
            position += 1
            continue
        if option == "e":
            attached_script = option_chars[position + 1 :]
            if attached_script:
                scripts.append(attached_script)
                return True, saw_print_only, index + 1
            if index + 1 >= len(args):
                return False, saw_print_only, index + 1
            scripts.append(args[index + 1])
            return True, saw_print_only, index + 2
        return False, saw_print_only, index + 1
    return True, saw_print_only, index + 1


def _sed_command_is_read_only(args: Sequence[str]) -> bool:
    saw_print_only = False
    scripts: list[str] = []
    option_only_long_flags = {
        "--binary",
        "--debug",
        "--follow-symlinks",
        "--null-data",
        "--posix",
        "--regexp-extended",
        "--sandbox",
        "--separate",
        "--unbuffered",
    }
    index = 0
    while index < len(args):
        arg = args[index]
        if arg == "--":
            index += 1
            if not scripts:
                if index >= len(args):
                    return False
                scripts.append(args[index])
            break
        if arg in {"--in-place", "--file"} or arg.startswith(
            ("--in-place=", "--file=")
        ):
            return False
        if arg in {"--quiet", "--silent"}:
            saw_print_only = True
            index += 1
            continue
        if arg == "--expression":
            if index + 1 >= len(args):
                return False
            scripts.append(args[index + 1])
            index += 2
            continue
        if arg.startswith("--expression="):
            scripts.append(arg.split("=", 1)[1])
            index += 1
            continue
        if arg == "--line-length":
            if index + 1 >= len(args):
                return False
            index += 2
            continue
        if arg.startswith("--line-length=") or arg in option_only_long_flags:
            index += 1
            continue
        if arg.startswith("--"):
            return False
        if arg.startswith("-") and arg != "-":
            ok, option_saw_print_only, next_index = _parse_sed_short_options(
                args,
                index,
                scripts,
            )
            if not ok:
                return False
            saw_print_only = saw_print_only or option_saw_print_only
            index = next_index
            continue
        if not scripts:
            scripts.append(arg)
        index += 1

    return bool(scripts) and saw_print_only and all(
        _sed_inline_script_is_read_only(script) for script in scripts
    )


def _find_command_is_read_only(args: Sequence[str]) -> bool:
    value_options = frozenset(
        {
            "-maxdepth",
            "-mindepth",
            "-name",
            "-iname",
            "-path",
            "-ipath",
            "-regex",
            "-iregex",
            "-size",
            "-type",
        }
    )
    no_arg_options = frozenset({"-print", "-print0"})

    index = 0
    while index < len(args):
        arg = args[index]
        if arg == "--":
            return False
        if not arg.startswith("-"):
            index += 1
            continue
        if arg in value_options:
            if index + 1 >= len(args):
                return False
            index += 2
            continue
        if arg in no_arg_options:
            index += 1
            continue
        return False
    return True


_RG_ALLOWED_NO_ARG_LONG_OPTIONS = frozenset(
    {
        "--case-sensitive",
        "--count",
        "--count-matches",
        "--files",
        "--files-with-matches",
        "--files-without-match",
        "--fixed-strings",
        "--heading",
        "--hidden",
        "--ignore-case",
        "--json",
        "--line-number",
        "--no-config",
        "--no-heading",
        "--no-ignore",
        "--no-line-number",
        "--smart-case",
        "--stats",
        "--vimgrep",
        "--word-regexp",
    }
)
_RG_ALLOWED_VALUE_LONG_OPTIONS = frozenset(
    {
        "--after-context",
        "--before-context",
        "--context",
        "--glob",
        "--max-count",
        "--max-depth",
        "--regexp",
        "--type",
        "--type-not",
    }
)
_RG_ALLOWED_NO_ARG_SHORT_OPTIONS = frozenset(
    {"F", "H", "S", "c", "h", "i", "l", "n", "v", "w"}
)
_RG_ALLOWED_VALUE_SHORT_OPTIONS = frozenset({"A", "B", "C", "e", "g", "m", "t", "T"})


def _rg_short_options_are_read_only(
    args: Sequence[str],
    index: int,
) -> tuple[bool, int]:
    option_chars = args[index][1:]
    position = 0
    while position < len(option_chars):
        option = option_chars[position]
        rest = option_chars[position + 1 :]
        if option in _RG_ALLOWED_NO_ARG_SHORT_OPTIONS:
            position += 1
            continue
        if option in _RG_ALLOWED_VALUE_SHORT_OPTIONS:
            if rest:
                return True, index + 1
            if index + 1 >= len(args):
                return False, index + 1
            return True, index + 2
        return False, index + 1
    return True, index + 1


def _rg_command_is_read_only(args: Sequence[str]) -> bool:
    saw_no_config = False
    index = 0
    while index < len(args):
        arg = args[index]
        if arg == "--":
            break
        if not arg.startswith("-") or arg == "-":
            index += 1
            continue
        if arg in _RG_ALLOWED_NO_ARG_LONG_OPTIONS:
            saw_no_config = saw_no_config or arg == "--no-config"
            index += 1
            continue
        if any(
            arg.startswith(f"{option}=")
            for option in _RG_ALLOWED_VALUE_LONG_OPTIONS
        ):
            index += 1
            continue
        if arg in _RG_ALLOWED_VALUE_LONG_OPTIONS:
            if index + 1 >= len(args):
                return False
            index += 2
            continue
        if arg.startswith("--"):
            return False
        ok, next_index = _rg_short_options_are_read_only(args, index)
        if not ok:
            return False
        index = next_index
    return saw_no_config


_PYTHON_SAFE_OPEN_MODES = frozenset({"r", "rt", "tr", "rb", "br"})
_PYTHON_DIRECT_READ_METHODS = frozenset({"read", "readline", "readlines"})
_PYTHON_PRINT_KEYWORDS = frozenset({"sep", "end", "flush"})
_PYTHON_OPEN_KEYWORDS = frozenset({"encoding", "errors", "mode", "newline"})


def _python_string_literal(node: ast.AST) -> str | None:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    return None


def _python_path_literal(node: ast.AST) -> bool:
    return isinstance(node, ast.Constant) and isinstance(node.value, (bytes, str))


def _python_keyword_value_is_literal(node: ast.AST) -> bool:
    return isinstance(node, ast.Constant) and isinstance(
        node.value,
        (bool, bytes, int, str, type(None)),
    )


def _python_keywords_are_literals(
    keywords: Sequence[ast.keyword],
    allowed_names: frozenset[str],
) -> bool:
    for keyword in keywords:
        if keyword.arg is None or keyword.arg not in allowed_names:
            return False
        if not _python_keyword_value_is_literal(keyword.value):
            return False
    return True


def _python_open_call_is_allowed_read_source(node: ast.AST) -> bool:
    if not isinstance(node, ast.Call):
        return False
    if not isinstance(node.func, ast.Name) or node.func.id != "open":
        return False
    if not node.args or len(node.args) > 2:
        return False
    if not _python_path_literal(node.args[0]):
        return False
    if not _python_keywords_are_literals(node.keywords, _PYTHON_OPEN_KEYWORDS):
        return False

    mode = "r"
    if len(node.args) == 2:
        mode = _python_string_literal(node.args[1]) or ""
    for keyword in node.keywords:
        if keyword.arg == "mode":
            if len(node.args) == 2:
                return False
            mode = _python_string_literal(keyword.value) or ""
    return mode in _PYTHON_SAFE_OPEN_MODES


def _python_read_call_arguments_are_allowed(node: ast.Call) -> bool:
    if node.keywords:
        return False
    if not node.args:
        return True
    return (
        len(node.args) == 1
        and isinstance(node.args[0], ast.Constant)
        and isinstance(node.args[0].value, int)
    )


def _python_direct_read_call_is_allowed(node: ast.Call) -> bool:
    if not isinstance(node.func, ast.Attribute):
        return False
    return (
        node.func.attr in _PYTHON_DIRECT_READ_METHODS
        and _python_open_call_is_allowed_read_source(node.func.value)
        and _python_read_call_arguments_are_allowed(node)
    )


def _python_expression_is_allowed(node: ast.AST) -> tuple[bool, bool]:
    if isinstance(node, ast.Constant):
        return True, False
    if isinstance(node, (ast.List, ast.Set, ast.Tuple)):
        results = [_python_expression_is_allowed(element) for element in node.elts]
        return all(ok for ok, _ in results), any(evidence for _, evidence in results)
    if isinstance(node, ast.Dict):
        items = [item for pair in zip(node.keys, node.values) for item in pair if item]
        results = [_python_expression_is_allowed(item) for item in items]
        return all(ok for ok, _ in results), any(evidence for _, evidence in results)
    if not isinstance(node, ast.Call):
        return False, False
    if isinstance(node.func, ast.Name) and node.func.id == "print":
        if not _python_keywords_are_literals(node.keywords, _PYTHON_PRINT_KEYWORDS):
            return False, False
        results = [_python_expression_is_allowed(arg) for arg in node.args]
        return all(ok for ok, _ in results), True
    if _python_direct_read_call_is_allowed(node):
        return True, True
    return False, False


def _python_inline_snippet_is_read_only(code: str) -> bool:
    try:
        tree = ast.parse(code)
    except SyntaxError:
        return False

    saw_read_only_expression = False
    for statement in tree.body:
        if isinstance(statement, (ast.Import, ast.ImportFrom)):
            return False
        if not isinstance(statement, ast.Expr):
            return False
        ok, has_read_only_expression = _python_expression_is_allowed(statement.value)
        if not ok:
            return False
        saw_read_only_expression = saw_read_only_expression or has_read_only_expression

    return saw_read_only_expression


def _python_command_is_read_only(args: Sequence[str]) -> bool:
    saw_isolated = False
    saw_no_site = False
    index = 0
    while index < len(args):
        arg = args[index]
        if arg == "-" or arg == "--" or not arg.startswith("-"):
            return False
        if arg.startswith("--"):
            return False

        option_chars = arg[1:]
        char_index = 0
        while char_index < len(option_chars):
            option_char = option_chars[char_index]
            rest = option_chars[char_index + 1 :]
            if option_char == "I":
                saw_isolated = True
                char_index += 1
                continue
            if option_char == "S":
                saw_no_site = True
                char_index += 1
                continue
            if option_char == "c":
                if not saw_isolated or not saw_no_site:
                    return False
                if rest:
                    return _python_inline_snippet_is_read_only(rest)
                if index + 1 >= len(args):
                    return False
                return _python_inline_snippet_is_read_only(args[index + 1])
            return False
        index += 1
    return False


def _shell_segment_is_read_only_inspect(tokens: Sequence[str]) -> bool:
    if _tokens_start_with_shell_assignment(tokens):
        return False

    command = _command_name(tokens[0])
    args = tokens[1:]
    if command in {"cat", "head", "tail", "ls", "pwd", "stat", "wc"}:
        return True
    if command == "grep":
        return True
    if command == "rg":
        return _rg_command_is_read_only(args)
    if command == "sed":
        return _sed_command_is_read_only(args)
    if command == "find":
        return _find_command_is_read_only(args)
    if command == "python3":
        return _python_command_is_read_only(args)
    return False


def _codex_command_execution_is_read_only_inspect(item: dict[str, object]) -> bool:
    command = item.get("command")
    if not isinstance(command, str) or not command.strip():
        return False

    payload = _codex_shell_payload(command)
    if "\n" in payload or "\r" in payload:
        return False
    if _shell_payload_has_command_substitution(payload):
        return False
    if _shell_payload_has_parameter_expansion(payload):
        return False
    if _shell_payload_has_unquoted_brace(payload):
        return False
    if _shell_payload_has_unquoted_glob(payload):
        return False
    if _shell_payload_has_unquoted_operator_or_control(payload):
        return False
    if _shell_payload_has_unquoted_tilde(payload):
        return False
    if _shell_payload_has_unquoted_zsh_expansion(payload):
        return False

    tokens = _shell_command_tokens(payload)
    if not tokens or _tokens_have_unsafe_redirection(tokens):
        return False

    segments = _shell_command_segments(tokens)
    if not segments:
        return False
    return all(_shell_segment_is_read_only_inspect(segment) for segment in segments)


def _codex_phase_work_signal_indexes(
    events: Iterable[dict[str, object]],
    *,
    event_types: tuple[str, ...],
    item_types: set[str],
    ignore_read_only_command_executions: bool,
) -> list[int]:
    indexes: list[int] = []
    for index, event in enumerate(events):
        item = _codex_event_item(event, event_types=event_types)
        if item is None:
            continue
        item_type = item.get("type")
        if not isinstance(item_type, str) or item_type not in item_types:
            continue
        if (
            ignore_read_only_command_executions
            and item_type == "command_execution"
            and _codex_command_execution_is_read_only_inspect(item)
        ):
            continue
        indexes.append(index)
    return indexes


def _describe_codex_pre_work_signal(item_types: Sequence[str]) -> str:
    labels = list(dict.fromkeys(str(item_type) for item_type in item_types))
    if not labels:
        return "pre-work signal"
    if len(labels) == 1:
        return f"pre-work signal ({labels[0]})"
    if len(labels) == 2:
        return f"pre-work signal ({labels[0]} or {labels[1]})"
    return f"pre-work signal ({', '.join(labels[:-1])}, or {labels[-1]})"


def _verify_codex_phase_contract(
    events: list[dict[str, object]],
    phase_contract: dict[str, object],
) -> tuple[bool, str]:
    raw_work_item_types = phase_contract.get(
        "work_item_types", ["command_execution", "file_change"]
    )
    if not isinstance(raw_work_item_types, list) or not raw_work_item_types:
        return False, "phase_contract.work_item_types must be a non-empty list"
    work_item_types = {str(item_type) for item_type in raw_work_item_types}

    raw_pre_work_item_types = phase_contract.get("pre_work_item_types")
    if raw_pre_work_item_types is not None and (
        not isinstance(raw_pre_work_item_types, list) or not raw_pre_work_item_types
    ):
        return False, "phase_contract.pre_work_item_types must be a non-empty list"

    require_pre_work_signal = bool(phase_contract.get("require_pre_work_signal", False))
    require_pre_work_agent_message = bool(
        phase_contract.get("require_pre_work_agent_message", False)
    )
    require_post_work_agent_message = bool(
        phase_contract.get("require_post_work_agent_message", False)
    )
    raw_ignore_read_only_commands = phase_contract.get(
        "ignore_read_only_command_executions", True
    )
    if not isinstance(raw_ignore_read_only_commands, bool):
        return (
            False,
            "phase_contract.ignore_read_only_command_executions must be a boolean",
        )
    ignore_read_only_command_executions = raw_ignore_read_only_commands

    pre_work_item_types: list[str] = []
    if require_pre_work_signal:
        if raw_pre_work_item_types is None:
            pre_work_item_types = ["reasoning", "agent_message"]
        else:
            pre_work_item_types = [
                str(item_type) for item_type in raw_pre_work_item_types
            ]
    elif require_pre_work_agent_message:
        pre_work_item_types = ["agent_message"]

    pre_work_signal_label = _describe_codex_pre_work_signal(pre_work_item_types)
    agent_message_indexes = _codex_phase_item_indexes(
        events,
        event_types=("item.completed",),
        item_types={"agent_message"},
    )
    pre_work_signal_indexes = _codex_phase_item_indexes(
        events,
        event_types=("item.completed",),
        item_types=set(pre_work_item_types),
    )
    work_signal_indexes = _codex_phase_work_signal_indexes(
        events,
        event_types=("item.started", "item.completed"),
        item_types=work_item_types,
        ignore_read_only_command_executions=ignore_read_only_command_executions,
    )

    if not work_signal_indexes:
        final_agent_message_index = (
            agent_message_indexes[-1] if agent_message_indexes else None
        )
        has_pre_work_signal_before_final = final_agent_message_index is not None and any(
            index < final_agent_message_index for index in pre_work_signal_indexes
        )
        if pre_work_item_types and not has_pre_work_signal_before_final:
            return (
                False,
                f"expected codex event stream to include {pre_work_signal_label} "
                "before observable work",
            )
        return (
            False,
            "expected codex event stream to include observable work signal "
            "between pre-work signal and final agent_message",
        )

    first_work_index = work_signal_indexes[0]
    last_work_index = work_signal_indexes[-1]

    if pre_work_item_types and not any(
        index < first_work_index for index in pre_work_signal_indexes
    ):
        return (
            False,
            f"expected codex event stream to include {pre_work_signal_label} "
            "before observable work",
        )

    if require_post_work_agent_message and not any(
        index > last_work_index for index in agent_message_indexes
    ):
        return (
            False,
            "expected codex event stream to include post-work final agent_message "
            "after observable work",
        )

    return True, ""


def _verify_codex_work_summary_contract(
    events: list[dict[str, object]],
    work_summary_contract: dict[str, object],
) -> tuple[bool, str]:
    raw_work_item_types = work_summary_contract.get(
        "work_item_types", ["file_change", "command_execution"]
    )
    if not isinstance(raw_work_item_types, list) or not raw_work_item_types:
        return False, "work_summary_contract.work_item_types must be a non-empty list"
    work_item_types = {str(item_type) for item_type in raw_work_item_types}

    completed_work_indexes = _codex_phase_item_indexes(
        events,
        event_types=("item.completed",),
        item_types=work_item_types,
    )
    if not completed_work_indexes:
        return (
            False,
            "expected codex event stream to include completed work item "
            "before final agent_message",
        )

    agent_message_indexes = _codex_phase_item_indexes(
        events,
        event_types=("item.completed",),
        item_types={"agent_message"},
    )
    last_completed_work_index = completed_work_indexes[-1]
    if not any(index > last_completed_work_index for index in agent_message_indexes):
        return (
            False,
            "expected codex event stream to include post-work final agent_message "
            "after completed work",
        )

    return True, ""


def _ordered_unique_strings(values: Iterable[object]) -> list[str]:
    ordered: list[str] = []
    seen: set[str] = set()
    for value in values:
        if not isinstance(value, str):
            continue
        if value in seen:
            continue
        seen.add(value)
        ordered.append(value)
    return ordered


def _trace_entries_for_phase(
    trace_entries: Iterable[dict[str, object]], phase: str
) -> list[dict[str, object]]:
    return [entry for entry in trace_entries if entry.get("phase") == phase]


def _string_list(value: object) -> list[str]:
    if not isinstance(value, list):
        return []
    return [item for item in value if isinstance(item, str)]


def _tool_names_from_tool_value(value: object) -> list[str]:
    names: list[str] = []
    if not isinstance(value, dict):
        return names

    name = value.get("name")
    if isinstance(name, str):
        names.append(name)

    function = value.get("function")
    if isinstance(function, dict):
        function_name = function.get("name")
        if isinstance(function_name, str):
            names.append(function_name)

    declarations = value.get("functionDeclarations")
    if isinstance(declarations, list):
        for declaration in declarations:
            if not isinstance(declaration, dict):
                continue
            declaration_name = declaration.get("name")
            if isinstance(declaration_name, str):
                names.append(declaration_name)

    return names


def _tool_names_from_trace_value(value: object) -> list[str]:
    names: list[str] = []
    if isinstance(value, list):
        for item in value:
            names.extend(_tool_names_from_trace_value(item))
        return names
    if not isinstance(value, dict):
        return names

    names.extend(_string_list(value.get("tool_names")))

    tools = value.get("tools")
    if isinstance(tools, list):
        for tool in tools:
            names.extend(_tool_names_from_tool_value(tool))

    declarations = value.get("functionDeclarations")
    if isinstance(declarations, list):
        for declaration in declarations:
            names.extend(_tool_names_from_tool_value(declaration))

    for child_value in value.values():
        if child_value is tools or child_value is declarations:
            continue
        if isinstance(child_value, (dict, list)):
            names.extend(_tool_names_from_trace_value(child_value))

    return names


def _tool_selector_names_from_trace_value(value: object) -> list[str]:
    names: list[str] = []
    if isinstance(value, list):
        names.extend(_string_list(value))
        for item in value:
            names.extend(_tool_selector_names_from_trace_value(item))
        return names
    if not isinstance(value, dict):
        return names

    names.extend(_tool_names_from_tool_value(value))
    for key in (
        "allowed_tool_names",
        "allowedFunctionNames",
        "allowed_function_names",
        "tool_names",
    ):
        names.extend(_string_list(value.get(key)))

    for child_value in value.values():
        if isinstance(child_value, (dict, list)):
            names.extend(_tool_selector_names_from_trace_value(child_value))

    return names


def _trace_summary_tool_selector_names(summary: dict[str, object]) -> list[str]:
    names: list[str] = []
    for key in (
        "tool_choice",
        "allowed_tools",
        "allowed_tool_names",
        "allowedFunctionNames",
        "allowed_function_names",
        "functionCallingConfig",
        "function_calling_config",
        "toolConfig",
        "tool_config",
    ):
        names.extend(_tool_selector_names_from_trace_value(summary.get(key)))
    return names


def _trace_request_tool_names(
    trace_entries: Iterable[dict[str, object]], side: str
) -> list[str]:
    names: list[str] = []
    summary_key = f"{side}_summary"
    for entry in _trace_entries_for_phase(trace_entries, "request"):
        request = entry.get("request")
        if not isinstance(request, dict):
            continue
        summary = request.get(summary_key)
        if isinstance(summary, dict):
            names.extend(_string_list(summary.get("tool_names")))
            if side == "client":
                names.extend(_tool_names_from_trace_value(summary))
        if side == "client":
            names.extend(_tool_names_from_trace_value(request.get("new_items")))
    return _ordered_unique_strings(names)


def _trace_request_tool_selector_names(
    trace_entries: Iterable[dict[str, object]], side: str
) -> list[str]:
    names: list[str] = []
    summary_key = f"{side}_summary"
    for entry in _trace_entries_for_phase(trace_entries, "request"):
        request = entry.get("request")
        if not isinstance(request, dict):
            continue
        summary = request.get(summary_key)
        if isinstance(summary, dict):
            names.extend(_trace_summary_tool_selector_names(summary))
    return _ordered_unique_strings(names)


def _contains_case_insensitive(values: Iterable[str], expected: str) -> bool:
    expected_lower = expected.lower()
    return any(value.lower() == expected_lower for value in values)


def _matching_tool_terms(
    values: Iterable[str], expected_terms: Iterable[str]
) -> list[str]:
    matches: list[str] = []
    for expected in expected_terms:
        if _contains_case_insensitive(values, expected):
            matches.append(expected)
    return matches


def _client_specific_verifier_values(
    verifier: dict[str, object],
    key: str,
    context: VerifierContext | None,
) -> tuple[list[str] | None, str]:
    by_client_key = f"{key}_by_client"
    raw_values = verifier.get(by_client_key)
    if raw_values is None:
        return [], ""
    if not isinstance(raw_values, dict):
        return None, f"{by_client_key} must be an object"
    client_name = context.client_name if context is not None else None
    if not client_name:
        return (
            None,
            "stdout_contract requires verifier context with client_name for client-specific expectations",
        )
    client_values = raw_values.get(client_name, [])
    if not isinstance(client_values, list):
        return None, f"{by_client_key}.{client_name} must be a list"
    return [str(value) for value in client_values], ""


def _other_client_specific_verifier_values(
    verifier: dict[str, object],
    key: str,
    context: VerifierContext | None,
) -> tuple[list[str] | None, str]:
    by_client_key = f"{key}_by_client"
    raw_values = verifier.get(by_client_key)
    if raw_values is None:
        return [], ""
    if not isinstance(raw_values, dict):
        return None, f"{by_client_key} must be an object"
    client_name = context.client_name if context is not None else None
    if not client_name:
        return (
            None,
            "stdout_contract requires verifier context with client_name for client-specific expectations",
        )

    current_client_values = raw_values.get(client_name, [])
    if not isinstance(current_client_values, list):
        return None, f"{by_client_key}.{client_name} must be a list"
    current_terms = {str(value) for value in current_client_values}

    other_values: list[str] = []
    for other_client_name, other_client_values in raw_values.items():
        if other_client_name == client_name:
            continue
        if not isinstance(other_client_values, list):
            return None, f"{by_client_key}.{other_client_name} must be a list"
        for value in other_client_values:
            normalized = str(value)
            if normalized not in current_terms:
                other_values.append(normalized)
    return other_values, ""


def _client_specific_match_mode(
    verifier: dict[str, object],
    key: str,
) -> tuple[str | None, str]:
    match_mode_key = f"{key}_by_client_match_mode"
    raw_match_mode = verifier.get(match_mode_key, "token")
    if not isinstance(raw_match_mode, str):
        return None, f"{match_mode_key} must be a string"
    match_mode = raw_match_mode.strip()
    if match_mode not in {"token", "presented_tool_name", "used_tool_name_mention"}:
        return (
            None,
            f"{match_mode_key} must be one of 'token', 'presented_tool_name', "
            "'used_tool_name_mention'",
        )
    return match_mode, ""


def _client_specific_term_pattern(expected: str) -> str:
    return rf"(?<![A-Za-z0-9_:-]){re.escape(expected)}(?![A-Za-z0-9_:-])"


def _presented_tool_name_token_pattern(expected: str) -> str:
    term_pattern = _client_specific_term_pattern(expected)
    return rf"(?:[`*_]+\s*)*{term_pattern}(?:\s*[`*_]+)*"


def _presented_tool_name_list_token_pattern() -> str:
    return r"(?:[`*_]+\s*)*[A-Za-z][A-Za-z0-9_:-]*(?:\s*[`*_]+)*"


def _plain_presented_tool_name_line(line: str) -> str:
    plain_line = re.sub(r"[`*]+", "", line)
    plain_line = re.sub(r"\s+", " ", plain_line)
    return plain_line.strip()


def _displayed_presented_tool_name_token(line: str, expected: str) -> bool:
    return (
        re.search(_presented_tool_name_token_pattern(expected), line, re.IGNORECASE)
        is not None
    )


def _plain_presented_tool_name_context_patterns(expected: str) -> tuple[str, ...]:
    term_pattern = _client_specific_term_pattern(expected)
    labeled_context = (
        r"(?:exact\s+public editing tool name|public editing tool name|editing tool name|"
        r"public tool name|tool name|public editing tool|editing tool|tool used|"
        r"public editing tool used|editing tool used)"
    )
    subject_context = (
        r"(?:exact\s+public editing tool name|public editing tool name|editing tool name|"
        r"public tool name|tool name|public editing tool|editing tool)"
    )
    trailing_description = r"(?:\s*(?:\([^)]+\)|[-–—:]\s*.*))?[.!?]?\s*$"
    return (
        rf"^\s*(?:the\s+)?{labeled_context}(?:\s+i\s+(?:actually\s+)?used)?"
        rf"\s*(?::|=)\s*{term_pattern}{trailing_description}",
        rf"^\s*(?:the\s+)?{subject_context}"
        rf"(?:\s+i\s+(?:actually\s+)?used|\s+used)?"
        rf"\s+(?:was|is)\s*:?\s*{term_pattern}{trailing_description}",
        rf"^\s*(?:the\s+)?tool(?:\s+i\s+(?:actually\s+)?used|\s+used)"
        rf"\s+(?:was|is)\s*:?\s*{term_pattern}{trailing_description}",
    )


def _plain_presented_tool_name_context(line: str, expected: str) -> bool:
    plain_line = _plain_presented_tool_name_line(line)
    if not plain_line:
        return False

    plain_term = re.search(_client_specific_term_pattern(expected), plain_line, re.IGNORECASE)
    if plain_term is None:
        return False

    for pattern in _plain_presented_tool_name_context_patterns(expected):
        if re.search(pattern, plain_line, re.IGNORECASE):
            return True

    lower_line = plain_line.lower()
    context_phrases = (
        "exact public editing tool name i used",
        "exact public editing tool name used",
        "exact public editing tool name",
        "public editing tool name i used",
        "public editing tool name used",
        "public editing tool name",
        "editing tool name used",
        "editing tool name",
        "public tool name",
        "tool name",
        "public editing tool i used",
        "public editing tool used",
        "editing tool used",
        "tool used",
    )
    direct_assignment_pattern = re.compile(
        r"^\s*(?:(?::|=)\s*(?:the\s+)?|(?:was|is)\s*:?\s*(?:the\s+)?)$",
        re.IGNORECASE,
    )

    for phrase in context_phrases:
        phrase_index = lower_line.find(phrase)
        if phrase_index < 0:
            continue
        if plain_term.start() <= phrase_index + len(phrase):
            continue
        between = lower_line[phrase_index + len(phrase) : plain_term.start()]
        if direct_assignment_pattern.fullmatch(between) is not None:
            return True

    return False


def _presented_tool_name_present(stdout_text: str, expected: str) -> bool:
    normalized_text = stdout_text.replace("\\r\\n", "\n").replace("\\n", "\n")
    term_pattern = _presented_tool_name_token_pattern(expected)
    client_label_pattern = r"(?:[`*_]+\s*)*(?:codex|claude|gemini)(?:\s*[`*_]+)*"
    list_token_pattern = _presented_tool_name_list_token_pattern()

    for raw_line in normalized_text.splitlines():
        line = raw_line.strip()
        if not line:
            continue

        if _plain_presented_tool_name_context(line, expected) and _displayed_presented_tool_name_token(
            line, expected
        ):
            return True

        if re.search(
            rf"^\s*(?:the\s+)?{term_pattern}\s+tool\b"
            rf"(?:\s*(?:\([^)]+\)|[-–—:]\s*.*))?[.!?]?\s*$",
            line,
            re.IGNORECASE,
        ):
            return True

        if _displayed_presented_tool_name_token(line, expected) and re.search(
            rf"^\s*(?:[-*+]\s+|\d+[.)]\s+)?"
            rf"(?:{client_label_pattern}\s*:\s*)?"
            rf"{list_token_pattern}(?:\s*,\s*{list_token_pattern})+"
            rf"\s*[.!?]?\s*$",
            line,
            re.IGNORECASE,
        ):
            return True

        if re.search(
            rf"^\s*(?:[-*+]\s+|\d+[.)]\s+)"
            rf"(?:{client_label_pattern}\s*:\s*)?"
            rf"{term_pattern}"
            rf"(?:\s*(?:\([^)]+\)|[-–—:]\s*.*)?)?$",
            line,
            re.IGNORECASE,
        ):
            return True

        if re.search(
            rf"^\s*{term_pattern}(?:\s*(?:\([^)]+\)|[-–—:]\s*.*)?)?$",
            line,
            re.IGNORECASE,
        ):
            return True

    return False


def _used_tool_name_bare_answer_present(line: str, expected: str) -> bool:
    plain_line = _plain_presented_tool_name_line(line)
    if not plain_line:
        return False
    term_pattern = _client_specific_term_pattern(expected)
    return (
        re.search(
            rf"^\s*(?:[-*+]\s+|\d+[.)]\s+)?{term_pattern}"
            rf"(?:\s*(?:\([^)]+\)|[-–—:]\s*.*))?[.!?]?\s*$",
            plain_line,
            re.IGNORECASE,
        )
        is not None
    )


def _used_tool_name_context_patterns(expected: str) -> tuple[str, ...]:
    term_pattern = _client_specific_term_pattern(expected)
    used_tool_subject = (
        r"(?:exact\s+)?(?:(?:public\s+editing|editing|public)\s+)?"
        r"tool(?:\s+name)?"
    )
    current_surface_qualifier = (
        r"(?:\s+(?:on|in|for)\s+(?:(?:the|this)\s+)?"
        r"(?:current\s+)?client\s+surface)?"
    )
    trailing_description = r"(?:\s*(?:\([^)]+\)|[-–—:]\s*.*))?[.!?]?\s*$"
    return (
        rf"^\s*(?:the\s+)?{used_tool_subject}\s+i\s+(?:actually\s+)?used"
        rf"{current_surface_qualifier}\s+(?:was|is)\s*:?\s*(?:the\s+)?{term_pattern}"
        rf"(?:\s+tool)?{trailing_description}",
        rf"^\s*(?:the\s+)?{used_tool_subject}\s+(?:actually\s+)?used"
        rf"{current_surface_qualifier}\s*(?::|=|\s+(?:was|is)\s*:?)\s*"
        rf"(?:the\s+)?{term_pattern}(?:\s+tool)?"
        rf"{trailing_description}",
        rf"\b(?:i|we)\b[\s,:;.-]*.*?\bused\b(?:\s+(?:the\s+)?"
        rf"(?:public\s+editing\s+|editing\s+|public\s+)?tool)?[\s,:;.-]*"
        rf"(?:the\s+)?{term_pattern}(?:\s+tool)?\b",
        rf"\b(?:using|with|via)\b\s+(?:the\s+)?"
        rf"(?:public\s+editing\s+|editing\s+|public\s+)?(?:tool\s+)?"
        rf"{term_pattern}(?:\s+tool)?\b",
        rf"^\s*(?:the\s+)?(?:{term_pattern}\s+(?:public\s+editing\s+|editing\s+|public\s+)?"
        rf"tool|(?:public\s+editing\s+|editing\s+|public\s+)?tool\s+{term_pattern})"
        rf"\s+(?:was|were|has\s+been|have\s+been|had\s+been)\s+"
        rf"(?:actually\s+)?used\b",
    )


def _used_tool_name_mention_present(stdout_text: str, expected: str) -> bool:
    normalized_text = stdout_text.replace("\\r\\n", "\n").replace("\\n", "\n")
    term_pattern = _client_specific_term_pattern(expected)
    use_patterns = _used_tool_name_context_patterns(expected)

    for raw_line in normalized_text.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        plain_line = _plain_presented_tool_name_line(line)
        if re.search(term_pattern, plain_line, re.IGNORECASE) is None:
            continue
        if _used_tool_name_bare_answer_present(line, expected):
            return True
        if any(re.search(pattern, plain_line, re.IGNORECASE) for pattern in use_patterns):
            return True

    return False


def _client_specific_term_present(
    stdout_text: str,
    expected: str,
    *,
    match_mode: str = "token",
) -> bool:
    if match_mode == "presented_tool_name":
        return _presented_tool_name_present(stdout_text, expected)
    if match_mode == "used_tool_name_mention":
        return _used_tool_name_mention_present(stdout_text, expected)
    pattern = re.compile(_client_specific_term_pattern(expected))
    return pattern.search(stdout_text) is not None


def _codex_stdout_contract_visible_text(stdout_text: str) -> str | None:
    events, _ = _parse_jsonl_events(stdout_text)
    if events is None:
        return None

    messages: list[str] = []
    for item in _completed_codex_items(events):
        if item.get("type") != "agent_message":
            continue
        text = item.get("text")
        if not isinstance(text, str):
            continue
        stripped = text.strip()
        if stripped:
            messages.append(stripped)

    if not messages:
        return None
    return "\n".join(messages)


def _stdout_contract_match_text(
    stdout_text: str,
    context: VerifierContext | None,
) -> str:
    if context is not None and context.client_name == "codex":
        codex_visible_text = _codex_stdout_contract_visible_text(stdout_text)
        if codex_visible_text is not None:
            return codex_visible_text
    return stdout_text


def _stdout_contract_reject_other_client_contains_any_by_client(
    verifier: dict[str, object],
    stdout_text: str,
    context: VerifierContext | None,
    *,
    match_mode: str,
) -> tuple[bool | None, str]:
    raw_flag = verifier.get("reject_other_client_contains_any_by_client", False)
    if not isinstance(raw_flag, bool):
        return None, "reject_other_client_contains_any_by_client must be a boolean"
    if not raw_flag:
        return True, ""

    other_client_contains_any, message = _other_client_specific_verifier_values(
        verifier, "contains_any", context
    )
    if other_client_contains_any is None:
        return None, message

    forbidden_terms = sorted(
        {
            expected
            for expected in other_client_contains_any
            if (
                _client_specific_term_present(
                    stdout_text,
                    expected,
                    match_mode=match_mode,
                )
                or _client_specific_term_present(
                    stdout_text,
                    expected,
                    match_mode="presented_tool_name",
                )
                or re.search(
                    rf"[`*]+\s*{_client_specific_term_pattern(expected)}\s*[`*]+",
                    stdout_text,
                    re.IGNORECASE,
                )
                is not None
            )
        },
        key=str.lower,
    )
    if forbidden_terms:
        listed_terms = ", ".join(repr(term) for term in forbidden_terms)
        return (
            False,
            "expected output not to list public tool names from other clients: "
            f"{listed_terms}",
        )
    return True, ""


def _verify_tool_identity_trace_contract(
    verifier: dict[str, object],
    context: VerifierContext | None,
) -> tuple[bool, str]:
    client_expected_terms, message = _client_specific_verifier_values(
        verifier, "contains_any", context
    )
    if client_expected_terms is None:
        return False, message
    if not client_expected_terms:
        return False, "tool_identity_contract requires contains_any_by_client terms"

    other_client_terms, message = _other_client_specific_verifier_values(
        verifier, "contains_any", context
    )
    if other_client_terms is None:
        return False, message

    trace_entries = tuple(context.trace_entries if context is not None else ())
    if not trace_entries:
        return (
            False,
            "expected debug trace request entries for tool identity contract; "
            "no matching entries were captured after case filtering",
        )
    if not _trace_entries_for_phase(trace_entries, "request"):
        return (
            False,
            "expected debug trace request entries for tool identity contract; "
            "captured entries did not include a request phase",
        )

    client_tool_names = _trace_request_tool_names(trace_entries, "client")
    upstream_tool_names = _trace_request_tool_names(trace_entries, "upstream")
    client_selector_names = _trace_request_tool_selector_names(trace_entries, "client")
    upstream_selector_names = _trace_request_tool_selector_names(trace_entries, "upstream")

    if not client_tool_names:
        return False, "expected debug trace client tool_names for tool identity contract"
    if not upstream_tool_names:
        return False, "expected debug trace upstream tool_names for tool identity contract"

    all_trace_tool_names = client_tool_names + upstream_tool_names
    all_trace_selector_names = client_selector_names + upstream_selector_names
    all_trace_public_names = all_trace_tool_names + all_trace_selector_names
    explicit_forbidden = [str(value) for value in verifier.get("not_contains", [])]
    forbidden_terms = [
        public_name
        for public_name in all_trace_public_names
        if any(forbidden in public_name for forbidden in explicit_forbidden)
        or find_internal_tool_artifact(public_name) is not None
    ]
    if forbidden_terms:
        listed_terms = ", ".join(repr(term) for term in _ordered_unique_strings(forbidden_terms))
        return (
            False,
            "expected debug trace tool_names/tool_choice fields not to expose "
            f"{listed_terms}",
        )

    missing_sides: list[str] = []
    if not _matching_tool_terms(client_tool_names, client_expected_terms):
        missing_sides.append("client")
    if not _matching_tool_terms(upstream_tool_names, client_expected_terms):
        missing_sides.append("upstream")
    if missing_sides:
        options = ", ".join(repr(expected) for expected in client_expected_terms)
        return (
            False,
            "expected debug trace "
            + " and ".join(f"{side} tool_names" for side in missing_sides)
            + f" to include at least one of {options}",
        )

    raw_reject_flag = verifier.get("reject_other_client_contains_any_by_client", False)
    if not isinstance(raw_reject_flag, bool):
        return False, "reject_other_client_contains_any_by_client must be a boolean"
    if raw_reject_flag:
        forbidden_other_terms = _matching_tool_terms(all_trace_public_names, other_client_terms)
        if forbidden_other_terms:
            listed_terms = ", ".join(repr(term) for term in forbidden_other_terms)
            return (
                False,
                "expected debug trace not to list public tool names from other clients "
                "in tool_names/tool_choice fields: "
                f"{listed_terms}",
            )

    return True, ""


def _codex_command_targets_path(command: str, path_suffix: str) -> bool:
    quoted_patterns = (
        rf"(?<![A-Za-z0-9_./-]){re.escape(path_suffix)}(?![A-Za-z0-9_./-])",
        rf"(?<![A-Za-z0-9_./-])\./{re.escape(path_suffix)}(?![A-Za-z0-9_./-])",
    )
    return any(re.search(pattern, command) for pattern in quoted_patterns)


def _codex_command_execution_matches_edit_target(
    item: dict[str, object], target_spec: dict[str, object]
) -> bool:
    if item.get("type") != "command_execution":
        return False
    if item.get("status") != "completed":
        return False
    if item.get("exit_code") != 0:
        return False

    command = item.get("command")
    if not isinstance(command, str):
        return False

    path_suffix = str(target_spec.get("path_suffix", "")).strip()
    if not path_suffix or not _codex_command_targets_path(command, path_suffix):
        return False

    write_patterns = (
        r"\bsed\s+-i(?:[^\s\"']*)?\b",
        rf"(?:^|[\s\"'])>\s*(?:\./)?{re.escape(path_suffix)}(?:[\s\"']|$)",
        rf"(?:^|[\s\"'])>>\s*(?:\./)?{re.escape(path_suffix)}(?:[\s\"']|$)",
        rf"\btee\b[^\n]*?(?:\./)?{re.escape(path_suffix)}(?:[\s\"']|$)",
        r"\bwrite_text\s*\(",
        r"\bopen\s*\([^)]*,\s*['\"](?:w|a)",
    )
    return any(re.search(pattern, command) for pattern in write_patterns)


def _codex_item_matches_edit_target(
    item: dict[str, object], target_spec: dict[str, object]
) -> bool:
    return _codex_file_change_matches(item, target_spec) or _codex_command_execution_matches_edit_target(
        item, target_spec
    )


def _verify_verifier_output(
    verifier: dict[str, object],
    stdout_text: str,
    workspace_dir: pathlib.Path | None,
    context: VerifierContext | None = None,
) -> tuple[bool, str]:
    verifier_type = verifier["type"]
    if verifier_type == "contains":
        needle = str(verifier["value"])
        ok = needle.lower() in stdout_text.lower()
        return ok, f"expected output to contain {needle!r}"
    if verifier_type == "tool_identity_contract":
        stdout_contract_verifier = dict(verifier)
        stdout_contract_verifier["type"] = "stdout_contract"
        ok, message = _verify_verifier_output(
            stdout_contract_verifier,
            stdout_text,
            workspace_dir,
            context,
        )
        if not ok:
            return False, message
        return _verify_tool_identity_trace_contract(verifier, context)
    if verifier_type == "stdout_contract":
        client_contains, message = _client_specific_verifier_values(
            verifier, "contains", context
        )
        if client_contains is None:
            return False, message
        client_contains_match_mode, message = _client_specific_match_mode(
            verifier, "contains"
        )
        if client_contains_match_mode is None:
            return False, message
        client_contains_any, message = _client_specific_verifier_values(
            verifier, "contains_any", context
        )
        if client_contains_any is None:
            return False, message
        client_contains_any_match_mode, message = _client_specific_match_mode(
            verifier, "contains_any"
        )
        if client_contains_any_match_mode is None:
            return False, message
        client_not_contains, message = _client_specific_verifier_values(
            verifier, "not_contains", context
        )
        if client_not_contains is None:
            return False, message
        stdout_contract_text = _stdout_contract_match_text(stdout_text, context)

        contains = [str(needle) for needle in verifier.get("contains", [])]
        for expected in contains:
            if expected not in stdout_contract_text:
                return False, f"expected output to contain {expected!r}"
        for expected in client_contains:
            if not _client_specific_term_present(
                stdout_contract_text,
                expected,
                match_mode=client_contains_match_mode,
            ):
                return False, f"expected output to contain {expected!r}"
        contains_any = [str(needle) for needle in verifier.get("contains_any", [])]
        if contains_any and not any(
            expected in stdout_contract_text for expected in contains_any
        ):
            options = ", ".join(repr(expected) for expected in contains_any)
            return False, f"expected output to contain at least one of {options}"
        if client_contains_any and not any(
            _client_specific_term_present(
                stdout_contract_text,
                expected,
                match_mode=client_contains_any_match_mode,
            )
            for expected in client_contains_any
        ):
            options = ", ".join(repr(expected) for expected in client_contains_any)
            return False, f"expected output to contain at least one of {options}"
        ok, message = _stdout_contract_reject_other_client_contains_any_by_client(
            verifier,
            stdout_contract_text,
            context,
            match_mode=client_contains_any_match_mode,
        )
        if ok is None:
            return False, message
        if not ok:
            return False, message
        not_contains = [str(needle) for needle in verifier.get("not_contains", [])] + client_not_contains
        for forbidden in not_contains:
            if forbidden in stdout_text:
                return False, f"expected raw output not to contain {forbidden!r}"
        return True, ""
    if verifier_type == "file_contains":
        if workspace_dir is None:
            return False, "workspace verifier required a workspace directory"
        relative_path = pathlib.Path(str(verifier["path"]))
        target = workspace_dir / relative_path
        if not target.exists():
            return False, f"expected file {relative_path} to exist"
        needle = str(verifier["needle"])
        ok = needle in target.read_text(encoding="utf-8")
        return ok, f"expected {relative_path} to contain {needle!r}"
    if verifier_type == "file_sha256":
        if workspace_dir is None:
            return False, "workspace verifier required a workspace directory"
        relative_path = pathlib.Path(str(verifier["path"]))
        target = workspace_dir / relative_path
        if not target.exists():
            return False, f"expected file {relative_path} to exist"
        expected_digest = str(verifier["sha256"]).lower()
        actual_digest = hashlib.sha256(target.read_bytes()).hexdigest()
        ok = actual_digest == expected_digest
        return (
            ok,
            f"expected {relative_path} sha256 {expected_digest}, got {actual_digest}",
        )
    if verifier_type == "python_source_and_output":
        if workspace_dir is None:
            return False, "workspace verifier required a workspace directory"
        ok, message = _verify_python_source_contract(workspace_dir, dict(verifier["source"]))
        if not ok:
            return False, message
        return _verify_python_entrypoint(workspace_dir, dict(verifier["entrypoint"]))
    if verifier_type == "command_success":
        if workspace_dir is None:
            return False, "workspace verifier required a workspace directory"
        return _verify_command_success(workspace_dir, verifier)
    if verifier_type == "codex_json_event_contract":
        events, completed_items, message = _verify_codex_json_event_contract(stdout_text)
        if events is None or completed_items is None:
            return False, message

        for expected_type in verifier.get("event_types", []):
            event_type = str(expected_type)
            if not any(event.get("type") == event_type for event in events):
                return False, f"expected codex event stream to include {event_type!r}"

        completed_item_types = {
            str(item.get("type"))
            for item in completed_items
            if isinstance(item.get("type"), str)
        }
        for expected_type in verifier.get("completed_item_types", []):
            item_type = str(expected_type)
            if item_type not in completed_item_types:
                return (
                    False,
                    f"expected codex event stream to include completed item type {item_type!r}",
                )

        for target_spec in verifier.get("completed_edit_targets", []):
            if not isinstance(target_spec, dict):
                return False, "completed_edit_targets entries must be objects"
            if not any(
                _codex_item_matches_edit_target(item, target_spec)
                for item in completed_items
            ):
                path_suffix = target_spec.get("path_suffix", "the expected path")
                return (
                    False,
                    "expected codex event stream to include observable edit signal "
                    f"for {path_suffix!r}",
                )

        phase_contract = verifier.get("phase_contract")
        if phase_contract is not None:
            if not isinstance(phase_contract, dict):
                return False, "phase_contract must be an object"
            ok, message = _verify_codex_phase_contract(events, phase_contract)
            if not ok:
                return False, message
        work_summary_contract = verifier.get("work_summary_contract")
        if work_summary_contract is not None:
            if not isinstance(work_summary_contract, dict):
                return False, "work_summary_contract must be an object"
            ok, message = _verify_codex_work_summary_contract(
                events, work_summary_contract
            )
            if not ok:
                return False, message
        return True, ""
    if verifier_type == "all_of":
        nested_verifiers = verifier.get("verifiers", [])
        if not isinstance(nested_verifiers, list) or not nested_verifiers:
            return False, "all_of verifier requires a non-empty verifiers list"
        for index, nested in enumerate(nested_verifiers):
            if not isinstance(nested, dict):
                return False, f"all_of[{index}] must be a verifier object"
            ok, message = _verify_verifier_output(
                nested,
                stdout_text,
                workspace_dir,
                context,
            )
            if not ok:
                return False, f"all_of[{index}]: {message}"
        return True, ""
    return False, f"unsupported verifier type: {verifier_type}"


def verify_fixture_output(
    fixture: TaskFixture,
    stdout_text: str,
    workspace_dir: pathlib.Path | None,
    context: VerifierContext | None = None,
) -> tuple[bool, str]:
    return _verify_verifier_output(
        fixture.verifier,
        stdout_text,
        workspace_dir,
        context,
    )


def _path_is_within(candidate: pathlib.Path, root: pathlib.Path) -> bool:
    resolved_candidate = candidate.resolve()
    resolved_root = root.resolve()
    return resolved_candidate == resolved_root or resolved_root in resolved_candidate.parents


def _cleanup_client_runtime_roots() -> None:
    for runtime_root in list(_CLIENT_RUNTIME_ROOTS_TO_CLEANUP):
        shutil.rmtree(runtime_root, ignore_errors=True)


def _register_client_runtime_root_cleanup(runtime_root: pathlib.Path) -> None:
    global _CLIENT_RUNTIME_CLEANUP_REGISTERED
    _CLIENT_RUNTIME_ROOTS_TO_CLEANUP.add(runtime_root)
    if _CLIENT_RUNTIME_CLEANUP_REGISTERED:
        return
    atexit.register(_cleanup_client_runtime_roots)
    _CLIENT_RUNTIME_CLEANUP_REGISTERED = True


def _client_runtime_base_dir() -> pathlib.Path:
    base_dir = pathlib.Path(tempfile.gettempdir()).resolve()
    if _path_is_within(base_dir, REPO_ROOT):
        fallback_dir = pathlib.Path("/tmp")
        if fallback_dir.exists():
            base_dir = fallback_dir.resolve()
    if _path_is_within(base_dir, REPO_ROOT):
        raise RuntimeError("client runtime root must live outside the repository")
    return base_dir


def prepare_client_runtime_root(report_dir: pathlib.Path) -> pathlib.Path:
    report_dir_key = str(report_dir.resolve())
    existing_root = _CLIENT_RUNTIME_ROOTS_BY_REPORT_DIR.get(report_dir_key)
    if existing_root is not None and existing_root.exists():
        return existing_root

    runtime_root = pathlib.Path(
        tempfile.mkdtemp(
            prefix=CLIENT_RUNTIME_ROOT_PREFIX,
            dir=str(_client_runtime_base_dir()),
        )
    ).resolve()
    _CLIENT_RUNTIME_ROOTS_BY_REPORT_DIR[report_dir_key] = runtime_root
    _register_client_runtime_root_cleanup(runtime_root)
    return runtime_root


def _client_runtime_case_token(case: MatrixCase, runtime_root: pathlib.Path) -> str:
    runtime_root_key = str(runtime_root.resolve())
    cache_key = (runtime_root_key, case.case_id)
    existing_token = _CLIENT_RUNTIME_CASE_TOKENS.get(cache_key)
    if existing_token is not None:
        return existing_token

    next_index = _CLIENT_RUNTIME_CASE_COUNTERS.get(runtime_root_key, 0) + 1
    _CLIENT_RUNTIME_CASE_COUNTERS[runtime_root_key] = next_index
    token = f"case-{next_index:04d}-{secrets.token_hex(4)}"
    _CLIENT_RUNTIME_CASE_TOKENS[cache_key] = token
    return token


def prepare_workspace(
    case: MatrixCase, runtime_root: pathlib.Path
) -> pathlib.Path | None:
    workspace_dir = runtime_root / "workspaces" / _client_runtime_case_token(case, runtime_root)
    if workspace_dir.exists():
        shutil.rmtree(workspace_dir)
    if case.fixture.workspace_template is None:
        workspace_dir.mkdir(parents=True, exist_ok=True)
        return workspace_dir
    shutil.copytree(case.fixture.workspace_template, workspace_dir)
    return workspace_dir


def resolve_client_home_dir(case: MatrixCase, runtime_root: pathlib.Path) -> pathlib.Path:
    if case.client_name == "gemini":
        return runtime_root / GEMINI_RUNNER_STATE_DIRNAME / GEMINI_SHARED_HOME_DIRNAME
    return runtime_root / "homes" / _client_runtime_case_token(case, runtime_root)


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


def resolve_case_timeout_secs(
    case: MatrixCase,
    home_dir: pathlib.Path,
    timeout_policy: TimeoutPolicy = DEFAULT_TIMEOUT_POLICY,
) -> int:
    timeout_secs = max(case.fixture.timeout_secs, timeout_policy.case_timeout_floor_secs)
    if case.fixture.kind == "long_horizon":
        timeout_secs = max(
            timeout_secs, timeout_policy.long_horizon_timeout_floor_secs
        )
    if case.client_name != "gemini":
        return timeout_secs
    if gemini_bootstrap_ready(home_dir):
        return timeout_secs
    return max(timeout_secs, timeout_policy.gemini_bootstrap_timeout_secs)


def _report_path(report_dir: pathlib.Path, target: pathlib.Path) -> str:
    return os.path.relpath(target, report_dir)


def trace_file_offset(trace_path: pathlib.Path) -> int:
    try:
        return pathlib.Path(trace_path).stat().st_size
    except FileNotFoundError:
        return 0


def _parse_trace_window_lines(text: str) -> list[dict[str, object]]:
    entries: list[dict[str, object]] = []
    lines = text.splitlines()
    for index, raw_line in enumerate(lines):
        line = raw_line.strip()
        if not line:
            continue
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            if index == len(lines) - 1:
                continue
            continue
        if isinstance(payload, dict):
            entries.append(payload)
    return entries


def read_trace_entries_since(
    trace_path: pathlib.Path,
    offset: int,
    *,
    timeout_secs: float = 0.75,
) -> list[dict[str, object]]:
    trace_path = pathlib.Path(trace_path)
    deadline = time.time() + max(0.0, timeout_secs)
    previous_size: int | None = None
    stable_reads = 0
    entries: list[dict[str, object]] = []

    while True:
        if trace_path.exists():
            current_size = trace_path.stat().st_size
            with trace_path.open("rb") as handle:
                handle.seek(max(0, offset))
                window_bytes = handle.read()
            entries = _parse_trace_window_lines(
                window_bytes.decode("utf-8", errors="replace")
            )
            if current_size == previous_size:
                stable_reads += 1
            else:
                stable_reads = 0
                previous_size = current_size
            if stable_reads >= 2 or time.time() >= deadline:
                return entries
        elif time.time() >= deadline:
            return []

        if timeout_secs <= 0:
            return entries
        time.sleep(0.05)


def _trace_entry_timestamp_ms(entry: dict[str, object]) -> int | None:
    value = entry.get("timestamp_ms")
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return int(value)
    return None


def _trace_entry_within_case_window(
    entry: dict[str, object],
    started_ms: int,
    finished_ms: int,
) -> bool:
    timestamp_ms = _trace_entry_timestamp_ms(entry)
    if timestamp_ms is None:
        return True
    return (
        started_ms - TRACE_CASE_START_SKEW_MS
        <= timestamp_ms
        <= finished_ms + TRACE_CASE_END_SKEW_MS
    )


def _trace_entry_route_matches_case(entry: dict[str, object], case: MatrixCase) -> bool:
    expected_client_format = TRACE_CLIENT_FORMAT_BY_CLIENT.get(case.client_name)
    client_format = entry.get("client_format")
    if (
        expected_client_format is not None
        and isinstance(client_format, str)
        and client_format != expected_client_format
    ):
        return False

    expected_path_prefix = TRACE_PATH_PREFIX_BY_CLIENT.get(case.client_name)
    path = entry.get("path")
    if (
        expected_path_prefix is not None
        and isinstance(path, str)
        and not path.startswith(expected_path_prefix)
    ):
        return False

    client_model = entry.get("client_model")
    if isinstance(client_model, str) and client_model != case.lane.proxy_model:
        return False

    upstream_name = entry.get("upstream_name")
    if isinstance(upstream_name, str) and upstream_name != case.lane.upstream_name:
        return False

    upstream_model = entry.get("upstream_model")
    if (
        case.lane.upstream_model is not None
        and isinstance(upstream_model, str)
        and upstream_model != case.lane.upstream_model
    ):
        return False

    return True


def _trace_entry_matches_case_window_and_route(
    entry: dict[str, object],
    case: MatrixCase,
    started_ms: int,
    finished_ms: int,
) -> bool:
    return _trace_entry_within_case_window(
        entry,
        started_ms,
        finished_ms,
    ) and _trace_entry_route_matches_case(entry, case)


def filter_trace_entries_for_case(
    trace_entries: Iterable[dict[str, object]],
    case: MatrixCase,
    *,
    started_ms: int,
    finished_ms: int,
) -> list[dict[str, object]]:
    entries = list(trace_entries)
    matching_request_ids = _ordered_unique_strings(
        entry.get("request_id")
        for entry in entries
        if entry.get("phase") == "request"
        and _trace_entry_matches_case_window_and_route(
            entry,
            case,
            started_ms,
            finished_ms,
        )
    )
    if matching_request_ids:
        matching_request_id_set = set(matching_request_ids)
        return [
            entry
            for entry in entries
            if isinstance(entry.get("request_id"), str)
            and entry["request_id"] in matching_request_id_set
        ]

    return [
        entry
        for entry in entries
        if _trace_entry_matches_case_window_and_route(
            entry,
            case,
            started_ms,
            finished_ms,
        )
    ]


def _workspace_diff_empty_summary() -> dict[str, object]:
    return {
        "changed_files": [],
        "added_files": [],
        "modified_files": [],
        "removed_files": [],
    }


def _workspace_file_should_be_ignored(path: pathlib.Path) -> bool:
    ignored_parts = {"__pycache__", ".git", ".pytest_cache"}
    return any(part in ignored_parts for part in path.parts)


def snapshot_workspace(workspace_dir: pathlib.Path | None) -> dict[str, str]:
    if workspace_dir is None:
        return {}
    workspace_dir = pathlib.Path(workspace_dir)
    if not workspace_dir.exists():
        return {}

    snapshot: dict[str, str] = {}
    for path in sorted(workspace_dir.rglob("*")):
        if not path.is_file() or _workspace_file_should_be_ignored(path.relative_to(workspace_dir)):
            continue
        relative_path = path.relative_to(workspace_dir).as_posix()
        try:
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
        except OSError:
            continue
        snapshot[relative_path] = digest
    return snapshot


def summarize_workspace_diff(
    before: dict[str, str],
    after: dict[str, str],
) -> dict[str, object]:
    before_paths = set(before)
    after_paths = set(after)
    added_files = sorted(after_paths - before_paths)
    removed_files = sorted(before_paths - after_paths)
    modified_files = sorted(
        path for path in before_paths & after_paths if before[path] != after[path]
    )
    changed_files = sorted(added_files + modified_files + removed_files)
    return {
        "changed_files": changed_files,
        "added_files": added_files,
        "modified_files": modified_files,
        "removed_files": removed_files,
    }


def _codex_metadata_snapshot(
    codex_metadata: CodexModelMetadata | None,
) -> dict[str, object]:
    if codex_metadata is None:
        return {}
    snapshot: dict[str, object] = {}
    if codex_metadata.input_modalities is not None:
        snapshot["input_modalities"] = list(codex_metadata.input_modalities)
    if codex_metadata.supports_search_tool is not None:
        snapshot["supports_search_tool"] = codex_metadata.supports_search_tool
    if codex_metadata.supports_view_image is not None:
        snapshot["supports_view_image"] = codex_metadata.supports_view_image
    if codex_metadata.apply_patch_tool_type is not None:
        snapshot["apply_patch_tool_type"] = codex_metadata.apply_patch_tool_type
    if codex_metadata.supports_parallel_tool_calls is not None:
        snapshot["supports_parallel_tool_calls"] = (
            codex_metadata.supports_parallel_tool_calls
        )
    return snapshot


def _trace_route_summary(
    trace_entries: Iterable[dict[str, object]],
) -> list[dict[str, object]]:
    routes: list[dict[str, object]] = []
    for entry in _trace_entries_for_phase(trace_entries, "request"):
        route: dict[str, object] = {}
        for key in (
            "request_id",
            "path",
            "stream",
            "client_format",
            "upstream_format",
            "client_model",
            "upstream_name",
            "upstream_model",
        ):
            value = entry.get(key)
            if value is not None:
                route[key] = value
        routes.append(route)
    return routes


def build_case_diagnostics(
    case: MatrixCase,
    trace_entries: Iterable[dict[str, object]],
    workspace_diff: dict[str, object] | None = None,
) -> dict[str, object]:
    trace_entries = tuple(trace_entries)
    request_entries = _trace_entries_for_phase(trace_entries, "request")
    response_entries = _trace_entries_for_phase(trace_entries, "response")
    request_ids = _ordered_unique_strings(
        entry.get("request_id") for entry in trace_entries
    )
    route_summary = _trace_route_summary(trace_entries)
    surface_snapshot: dict[str, object] = {
        "client_model": case.lane.proxy_model,
        "lane": case.lane.name,
        "upstream_name": case.lane.upstream_name,
    }
    if case.lane.upstream_model is not None:
        surface_snapshot["upstream_model"] = case.lane.upstream_model
    surface_snapshot.update(_codex_metadata_snapshot(case.lane.codex_metadata))

    client_tool_names = _trace_request_tool_names(trace_entries, "client")
    upstream_tool_names = _trace_request_tool_names(trace_entries, "upstream")
    client_tool_selector_names = _trace_request_tool_selector_names(trace_entries, "client")
    upstream_tool_selector_names = _trace_request_tool_selector_names(trace_entries, "upstream")

    return {
        "request_id": request_ids[0] if request_ids else None,
        "request_ids": request_ids,
        "trace_entry_count": len(trace_entries),
        "trace_request_count": len(request_entries),
        "trace_response_count": len(response_entries),
        "route_summary": route_summary,
        "surface_snapshot": surface_snapshot,
        "tool_identity": {
            "client_tool_names": client_tool_names,
            "upstream_tool_names": upstream_tool_names,
            "client_tool_selector_names": client_tool_selector_names,
            "upstream_tool_selector_names": upstream_tool_selector_names,
        },
        "workspace_diff": workspace_diff or _workspace_diff_empty_summary(),
    }


def build_client_command(
    client_name: str,
    proxy_base: str,
    lane: Lane,
    fixture: TaskFixture,
    workspace_dir: pathlib.Path,
    client_home: pathlib.Path | None = None,
    dangerous_harness: bool = False,
) -> list[str]:
    prompt_text = render_fixture_prompt(fixture, client_name)
    if client_name == "codex":
        command = [
            "codex",
            "exec",
            prompt_text,
            "--model",
            lane.proxy_model,
            "--ephemeral",
            "--json",
            "--skip-git-repo-check",
        ]
        if dangerous_harness:
            command.append("--dangerously-bypass-approvals-and-sandbox")
        else:
            command.extend(["--sandbox", "workspace-write"])
        command.extend(
            [
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
        )
        command.extend(
            build_codex_catalog_args(
                client_home,
                lane.proxy_model,
                lane.limits,
                lane.codex_metadata,
            )
        )
        ensure_no_public_internal_tool_artifacts(
            command, context="real CLI command"
        )
        return command
    if client_name == "claude":
        command = [
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
        ]
        if dangerous_harness:
            command.append("--dangerously-skip-permissions")
        command.extend(["--add-dir", str(workspace_dir)])
        ensure_no_public_internal_tool_artifacts(
            command, context="real CLI command"
        )
        return command
    if client_name == "gemini":
        command = [
            "gemini",
            "--prompt",
            prompt_text,
            "--model",
            lane.proxy_model,
        ]
        if dangerous_harness:
            command.extend(["--sandbox=false", "--yolo"])
        command.extend(
            [
                "--include-directories",
                str(workspace_dir),
                "--output-format",
                "text",
            ]
        )
        ensure_no_public_internal_tool_artifacts(
            command, context="real CLI command"
        )
        return command
    raise ValueError(f"unknown client: {client_name}")


def run_matrix_case(
    case: MatrixCase,
    proxy_base: str,
    report_dir: pathlib.Path,
    base_env: dict[str, str],
    timeout_policy: TimeoutPolicy = DEFAULT_TIMEOUT_POLICY,
    dangerous_harness: bool = False,
) -> dict[str, object]:
    report_dir = report_dir.resolve()
    cases_dir = report_dir / "cases"
    cases_dir.mkdir(parents=True, exist_ok=True)

    runtime_root = prepare_client_runtime_root(report_dir).resolve()
    workspace_dir = prepare_workspace(case, runtime_root).resolve()
    home_dir = resolve_client_home_dir(case, runtime_root).resolve()
    env = build_client_env(
        case.client_name,
        base_env,
        proxy_base,
        home_dir,
        model_name=case.lane.proxy_model,
        model_limits=case.lane.limits,
    )
    command = build_client_command(
        case.client_name,
        proxy_base,
        case.lane,
        case.fixture,
        workspace_dir,
        client_home=home_dir,
        dangerous_harness=dangerous_harness,
    )
    stdin_text = client_stdin_text(case.client_name, case.fixture)
    timeout_secs = resolve_case_timeout_secs(case, home_dir, timeout_policy)
    trace_path = report_dir / "debug-trace.jsonl"
    trace_offset = trace_file_offset(trace_path)
    workspace_before = snapshot_workspace(workspace_dir)
    started = time.time()
    started_ms = int(started * 1000)
    status = "failed"
    message = ""
    stdout_text = ""
    stderr_text = ""
    trace_entries: list[dict[str, object]] = []
    workspace_diff_summary = _workspace_diff_empty_summary()
    diagnostics: dict[str, object] | None = None

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
        workspace_diff_summary = summarize_workspace_diff(
            workspace_before,
            snapshot_workspace(workspace_dir),
        )
        finished_ms = int(time.time() * 1000)
        trace_entries = filter_trace_entries_for_case(
            read_trace_entries_since(trace_path, trace_offset),
            case,
            started_ms=started_ms,
            finished_ms=finished_ms,
        )
        diagnostics = build_case_diagnostics(
            case,
            trace_entries,
            workspace_diff_summary,
        )
        if completed.returncode == 0:
            ok, verifier_message = verify_fixture_output(
                case.fixture,
                stdout_text,
                workspace_dir if case.fixture.workspace_template is not None else None,
                context=VerifierContext(
                    client_name=case.client_name,
                    case_id=case.case_id,
                    command=tuple(command),
                    home_dir=home_dir,
                    workspace_dir=workspace_dir,
                    trace_entries=tuple(trace_entries),
                    diagnostics=diagnostics,
                    workspace_diff=workspace_diff_summary,
                ),
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
        workspace_diff_summary = summarize_workspace_diff(
            workspace_before,
            snapshot_workspace(workspace_dir),
        )
        finished_ms = int(time.time() * 1000)
        trace_entries = filter_trace_entries_for_case(
            read_trace_entries_since(trace_path, trace_offset),
            case,
            started_ms=started_ms,
            finished_ms=finished_ms,
        )
        diagnostics = build_case_diagnostics(
            case,
            trace_entries,
            workspace_diff_summary,
        )
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
        "diagnostics": diagnostics
        or build_case_diagnostics(case, trace_entries, workspace_diff_summary),
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


def prepare_proxy_env(
    base_env: dict[str, str],
    dotenv_env: dict[str, str],
    runtime_root: pathlib.Path | None = None,
) -> dict[str, str]:
    proxy_env = dict(base_env)
    proxy_env.update(dotenv_env)
    proxy_env[AUTH_MODE_ENV] = "proxy_key"
    proxy_env[PROXY_KEY_ENV] = resolve_proxy_key(base_env, dotenv_env)
    if runtime_root is not None:
        proxy_env[REPLAY_MARKER_KEY_ENV] = ensure_replay_marker_key(runtime_root)
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


def stop_proxy(
    process: subprocess.Popen[str] | None,
    terminate_grace_secs: int = DEFAULT_TIMEOUT_POLICY.process_terminate_grace_secs,
) -> None:
    if process is None:
        return
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=terminate_grace_secs)
    except subprocess.TimeoutExpired:
        process.kill()
        try:
            process.wait(timeout=DEFAULT_POST_KILL_WAIT_SECS)
        except subprocess.TimeoutExpired:
            return


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
        default=None,
        help="proxy listen port; defaults to an automatically selected free port",
    )
    add_timeout_policy_args(parser, include_case_thresholds=True)
    args = parser.parse_args(argv)
    args.list = args.list_matrix
    if args.test not in VALID_PHASES:
        parser.error(f"--test must be one of: {', '.join(sorted(VALID_PHASES))}")
    env_proxy_port = os.environ.get("PROXY_PORT")
    if args.proxy_port is not None:
        args.proxy_port_source = "argument"
    elif env_proxy_port:
        try:
            args.proxy_port = int(env_proxy_port)
        except ValueError:
            parser.error("PROXY_PORT must be an integer")
        args.proxy_port_source = "environment"
    else:
        args.proxy_port_source = "auto"
    if args.proxy_port is not None and not (1 <= int(args.proxy_port) <= 65535):
        parser.error("--proxy-port must be between 1 and 65535")
    return args


def run(argv: list[str] | None = None) -> int:
    args = resolve_cli_args(argv)
    timeout_policy = timeout_policy_from_args(args)
    base_env = dict(os.environ)
    config_source = pathlib.Path(args.config_source)
    dotenv_env = load_dotenv_file(pathlib.Path(args.env_file))
    parsed_source = parse_proxy_source(config_source.read_text(encoding="utf-8"))
    dotenv_env = merge_preset_endpoint_env(parsed_source, dotenv_env, base_env)
    lanes = resolve_lanes(
        parsed_source,
        dotenv_env,
        require_preset_endpoint_env=not args.list_matrix,
    )
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
    proxy_port = int(args.proxy_port) if args.proxy_port is not None else free_port()
    runtime_config_text = build_runtime_config_text(
        parsed_source,
        dotenv_env,
        listen_host=args.proxy_host,
        listen_port=proxy_port,
        trace_path=trace_path,
    )
    proxy_env = prepare_proxy_env(base_env, dotenv_env, report_dir)
    proxy_key = proxy_env[PROXY_KEY_ENV]
    client_base_env = dict(base_env)
    client_base_env[PROXY_KEY_ENV] = proxy_key
    process = None
    results: list[dict[str, object]] = []

    try:
        process, _runtime_config_path, _stdout_path, _stderr_path = start_proxy(
            proxy_binary, runtime_config_text, report_dir, proxy_env
        )
        proxy_base = f"http://{args.proxy_host}:{proxy_port}"
        wait_for_health(
            proxy_base,
            timeout_secs=timeout_policy.proxy_health_timeout_secs,
            process=process,
            stdout_path=_stdout_path,
            stderr_path=_stderr_path,
        )
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

        refresh_lane_model_profiles(proxy_base, lanes, proxy_key=proxy_key)

        lane_probes = {
            lane.name: classify_lane_health(
                lane,
                probe_lane(proxy_base, lane, proxy_key=proxy_key),
            )
            for lane in lanes
        }
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
            results.append(
                run_matrix_case(
                    case,
                    proxy_base,
                    report_dir,
                    client_base_env,
                    timeout_policy=timeout_policy,
                    dangerous_harness=args.dangerous_harness,
                )
            )
    finally:
        stop_proxy(process, terminate_grace_secs=timeout_policy.process_terminate_grace_secs)

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
