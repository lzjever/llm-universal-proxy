import importlib.util
import io
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest
from unittest import mock


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
DOC_CONTRACT_PATHS = (
    REPO_ROOT / "README.md",
    REPO_ROOT / "docs" / "DESIGN.md",
    REPO_ROOT / "docs" / "PRD.md",
    REPO_ROOT / "docs" / "CONSTITUTION.md",
    REPO_ROOT / "docs" / "protocol-baselines" / "capabilities" / "tools.md",
    REPO_ROOT / "docs" / "max-compat-design.md",
    REPO_ROOT / "docs" / "max-compat-development-plan.md",
)
LOCKED_TOOL_CONTRACT_LINES = (
    "The proxy must not rewrite the visible tool name supplied by the client.",
    "`__llmup_custom__*` is an internal transport artifact, not a public contract.",
    "`apply_patch` remains a public freeform tool on client-visible surfaces.",
)
FORBIDDEN_LEGACY_TOOL_IDENTITY_LANGUAGE = {
    REPO_ROOT / "README.md": (
        "Important current limitation:",
        "The current Responses custom-tool bridge still uses reserved synthetic names such as `__llmup_custom__apply_patch` on some translated live paths.",
        "there is also a known current limitation: Responses custom tools may still be bridged with reserved proxy names visible to the upstream model.",
    ),
    REPO_ROOT / "docs" / "DESIGN.md": (
        "It is acceptable only as an internal or stateless fallback",
    ),
    REPO_ROOT / "docs" / "max-compat-design.md": (
        "The current live bridge for OpenAI Responses custom tools rewrites:",
        "the current name-based encoding is not valid as a live model-visible contract for agent clients.",
        "Current behavior is intentional.",
        "keep reserved-prefix bridge names only as legacy/stateless fallback machinery",
    ),
}
REQUIRED_TOOL_BRIDGE_DIRECTION_LANGUAGE = {
    REPO_ROOT / "docs" / "max-compat-design.md": (
        "The intended translated-path bridge preserves the stable visible tool name and carries bridge provenance in request-scoped translation context.",
    ),
    REPO_ROOT / "docs" / "max-compat-development-plan.md": (
        "Phase 0 and Phase 1 together define the intended translated-path bridge: preserve the stable visible tool name on live requests and carry bridge provenance in request-scoped translation context.",
    ),
}


def load_module():
    spec = importlib.util.spec_from_file_location("real_cli_matrix", SCRIPT_PATH)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def codex_catalog_probe(catalog_payload, timeout_secs=2):
    if shutil.which("codex") is None:
        raise unittest.SkipTest("codex binary is not available")

    with tempfile.TemporaryDirectory() as temp_dir:
        root = pathlib.Path(temp_dir)
        catalog_path = root / "catalog.json"
        catalog_path.write_text(
            json.dumps(catalog_payload, indent=2) + "\n",
            encoding="utf-8",
        )
        env = dict(os.environ)
        env["HOME"] = str(root / "home")
        command = [
            "codex",
            "exec",
            "reply with ok",
            "--model",
            "minimax-openai",
            "--ephemeral",
            "--json",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "-C",
            "/tmp",
            "-c",
            'model_provider="proxy"',
            "-c",
            'model_providers.proxy.name="Proxy"',
            "-c",
            'model_providers.proxy.base_url="http://127.0.0.1:1/openai/v1"',
            "-c",
            'model_providers.proxy.wire_api="responses"',
            "-c",
            f'model_catalog_json="{catalog_path}"',
            "-c",
            'web_search="disabled"',
        ]
        try:
            completed = subprocess.run(
                command,
                env=env,
                input="",
                text=True,
                capture_output=True,
                timeout=timeout_secs,
            )
            return completed.returncode, completed.stdout + completed.stderr, False
        except subprocess.TimeoutExpired as error:
            stdout = error.stdout.decode() if isinstance(error.stdout, bytes) else (error.stdout or "")
            stderr = error.stderr.decode() if isinstance(error.stderr, bytes) else (error.stderr or "")
            return 124, stdout + stderr, True


def make_lane(
    module,
    *,
    name="minimax-anth",
    required=True,
    enabled=True,
    proxy_model=None,
    upstream_name="MINIMAX-ANTHROPIC",
):
    return module.Lane(
        name=name,
        required=required,
        enabled=enabled,
        proxy_model=proxy_model or name,
        upstream_name=upstream_name,
        skip_reason=None,
    )


def make_fixture(
    module,
    *,
    fixture_id="smoke_pong",
    prompt="Reply with PONG",
    verifier=None,
    timeout_secs=5,
):
    return module.TaskFixture(
        fixture_id=fixture_id,
        kind="smoke",
        prompt=prompt,
        verifier=verifier or {"type": "contains", "value": "PONG"},
        timeout_secs=timeout_secs,
        workspace_template=None,
    )


def make_case(module, *, client_name, lane=None, fixture=None, case_id=None):
    lane = lane or make_lane(module)
    fixture = fixture or make_fixture(module)
    return module.MatrixCase(
        client_name=client_name,
        lane=lane,
        fixture=fixture,
        case_id=case_id or f"{client_name}__{lane.name}__{fixture.fixture_id}",
    )


class RealCliMatrixTests(unittest.TestCase):
    def test_parse_dotenv_exports_reads_export_lines(self):
        module = load_module()

        parsed = module.parse_dotenv_exports(
            textwrap.dedent(
                """
                # comment
                export PRESET_ENDPOINT_API_KEY="real-key"
                LOCAL_QWEN_BASE_URL=http://127.0.0.1:9997/v1
                LOCAL_QWEN_MODEL='qwen3.5-9b-awq'
                """
            )
        )

        self.assertEqual(parsed["PRESET_ENDPOINT_API_KEY"], "real-key")
        self.assertEqual(parsed["LOCAL_QWEN_BASE_URL"], "http://127.0.0.1:9997/v1")
        self.assertEqual(parsed["LOCAL_QWEN_MODEL"], "qwen3.5-9b-awq")

    def test_parse_proxy_source_extracts_upstreams_and_aliases(self):
        module = load_module()

        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstream_timeout_secs: 120
                upstreams:
                  MINIMAX-ANTHROPIC:
                    api_root: "https://api.minimaxi.com/anthropic/v1"
                    format: anthropic
                    credential_actual: "secret"
                    auth_policy: force_server
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-completion
                    credential_actual: "secret"
                    auth_policy: force_server
                model_aliases:
                  minimax-anth: "MINIMAX-ANTHROPIC:MiniMax-M2.7-highspeed"
                  minimax-openai: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
                  claude-opus-4-6: "LOCAL-QWEN:qwen3.5-9b-awq"
                debug_trace:
                  path: /tmp/trace.jsonl
                  max_text_chars: 16384
                """
            )
        )

        self.assertEqual(parsed.listen, "127.0.0.1:18888")
        self.assertEqual(parsed.upstream_timeout_secs, 120)
        self.assertEqual(parsed.upstreams["MINIMAX-ANTHROPIC"]["format"], "anthropic")
        self.assertEqual(
            parsed.model_aliases["minimax-openai"],
            "MINIMAX-OPENAI:MiniMax-M2.7-highspeed",
        )
        self.assertEqual(parsed.debug_trace["path"], "/tmp/trace.jsonl")

    def test_parse_proxy_source_extracts_model_alias_limits(self):
        module = load_module()

        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-completion
                    credential_actual: "secret"
                    auth_policy: force_server
                    limits:
                      context_window: 200000
                      max_output_tokens: 128000
                model_aliases:
                  minimax-openai:
                    target: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
                    limits:
                      max_output_tokens: 64000
                    codex:
                      input_modalities: ["text"]
                      supports_search_tool: false
                """
            )
        )

        self.assertEqual(
            parsed.model_aliases["minimax-openai"],
            "MINIMAX-OPENAI:MiniMax-M2.7-highspeed",
        )
        self.assertEqual(parsed.upstream_limits["MINIMAX-OPENAI"].context_window, 200000)
        self.assertEqual(parsed.upstream_limits["MINIMAX-OPENAI"].max_output_tokens, 128000)
        self.assertEqual(
            parsed.model_alias_configs["minimax-openai"].limits.max_output_tokens,
            64000,
        )
        self.assertEqual(
            parsed.model_alias_configs["minimax-openai"].codex_metadata.input_modalities,
            ("text",),
        )
        self.assertFalse(
            parsed.model_alias_configs["minimax-openai"].codex_metadata.supports_search_tool
        )

    def test_resolve_lanes_marks_qwen_optional_when_env_missing(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            (REPO_ROOT / "proxy-test-minimax-and-local.yaml").read_text(encoding="utf-8")
        )

        lanes = {lane.name: lane for lane in module.resolve_lanes(parsed, {})}

        self.assertTrue(lanes["minimax-anth"].required)
        self.assertTrue(lanes["minimax-openai"].required)
        self.assertFalse(lanes["qwen-local"].required)
        self.assertFalse(lanes["qwen-local"].enabled)
        self.assertIn("LOCAL_QWEN", lanes["qwen-local"].skip_reason)
        self.assertEqual(lanes["minimax-openai"].limits.context_window, 200000)
        self.assertEqual(lanes["minimax-openai"].limits.max_output_tokens, 128000)
        self.assertEqual(
            lanes["minimax-openai"].codex_metadata.input_modalities,
            ("text",),
        )
        self.assertFalse(lanes["minimax-openai"].codex_metadata.supports_search_tool)

    def test_resolve_lanes_enables_qwen_when_env_present(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            (REPO_ROOT / "proxy-test-minimax-and-local.yaml").read_text(encoding="utf-8")
        )

        lanes = {
            lane.name: lane
            for lane in module.resolve_lanes(
                parsed,
                {
                    "LOCAL_QWEN_BASE_URL": "http://127.0.0.1:9997/v1",
                    "LOCAL_QWEN_MODEL": "qwen3.5-9b-awq",
                    "LOCAL_QWEN_API_KEY": "not-needed",
                },
            )
        }

        self.assertTrue(lanes["qwen-local"].enabled)
        self.assertEqual(lanes["qwen-local"].proxy_model, "qwen-local")
        self.assertEqual(lanes["qwen-local"].upstream_name, "LOCAL-QWEN")

    def test_build_runtime_config_overrides_listen_and_injects_qwen(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            (REPO_ROOT / "proxy-test-minimax-and-local.yaml").read_text(encoding="utf-8")
        )

        rendered = module.build_runtime_config_text(
            parsed,
            {
                "LOCAL_QWEN_BASE_URL": "http://127.0.0.1:9997/v1",
                "LOCAL_QWEN_MODEL": "qwen3.5-9b-awq",
                "LOCAL_QWEN_API_KEY": "not-needed",
            },
            listen_host="127.0.0.1",
            listen_port=19999,
            trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
        )

        self.assertIn("listen: 127.0.0.1:19999", rendered)
        self.assertIn("LOCAL-QWEN:", rendered)
        self.assertIn('qwen-local: "LOCAL-QWEN:qwen3.5-9b-awq"', rendered)
        self.assertIn("path: /tmp/cli-matrix-trace.jsonl", rendered)

    def test_build_runtime_config_preserves_structured_model_alias_limits(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-completion
                    credential_actual: "secret"
                    auth_policy: force_server
                    limits:
                      context_window: 200000
                      max_output_tokens: 128000
                model_aliases:
                  minimax-openai:
                    target: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
                    limits:
                      max_output_tokens: 64000
                debug_trace:
                  path: /tmp/trace.jsonl
                  max_text_chars: 16384
                """
            )
        )

        rendered = module.build_runtime_config_text(
            parsed,
            {},
            listen_host="127.0.0.1",
            listen_port=19999,
            trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
        )

        self.assertIn("minimax-openai:", rendered)
        self.assertIn('target: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"', rendered)
        self.assertIn("limits:", rendered)
        self.assertIn("context_window: 200000", rendered)
        self.assertIn("max_output_tokens: 64000", rendered)

    def test_resolve_model_limits_inherits_upstream_defaults_and_alias_overrides(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-completion
                    credential_actual: "secret"
                    auth_policy: force_server
                    limits:
                      context_window: 200000
                      max_output_tokens: 128000
                model_aliases:
                  minimax-openai:
                    target: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
                    limits:
                      max_output_tokens: 64000
                """
            )
        )

        limits = module.resolve_model_limits(parsed, "minimax-openai")

        self.assertEqual(limits.context_window, 200000)
        self.assertEqual(limits.max_output_tokens, 64000)

    def test_resolve_model_limits_supports_direct_upstream_model_targets(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-completion
                    credential_actual: "secret"
                    auth_policy: force_server
                    limits:
                      context_window: 200000
                      max_output_tokens: 128000
                model_aliases:
                  minimax-openai: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
                """
            )
        )

        limits = module.resolve_model_limits(
            parsed, "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
        )

        self.assertEqual(limits.context_window, 200000)
        self.assertEqual(limits.max_output_tokens, 128000)

    def test_resolve_codex_model_metadata_defaults_proxy_models_to_text_only_and_search_disabled(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-completion
                    credential_actual: "secret"
                    auth_policy: force_server
                    limits:
                      context_window: 200000
                model_aliases:
                  minimax-openai: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
                """
            )
        )

        metadata = module.resolve_codex_model_metadata(parsed, "minimax-openai")

        self.assertEqual(metadata.input_modalities, ("text",))
        self.assertFalse(metadata.supports_search_tool)

    def test_resolve_codex_model_metadata_supports_upstream_defaults_and_alias_overrides(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                upstreams:
                  MINIMAX-OPENAI:
                    api_root: "https://api.minimaxi.com/v1"
                    format: openai-completion
                    credential_actual: "secret"
                    auth_policy: force_server
                    codex:
                      input_modalities: ["text"]
                      supports_search_tool: false
                model_aliases:
                  vision-openai:
                    target: "MINIMAX-OPENAI:MiniMax-Vision"
                    codex:
                      input_modalities: ["text", "image"]
                      supports_search_tool: true
                """
            )
        )

        metadata = module.resolve_codex_model_metadata(parsed, "vision-openai")

        self.assertEqual(metadata.input_modalities, ("text", "image"))
        self.assertTrue(metadata.supports_search_tool)

    def test_build_runtime_config_omits_local_qwen_aliases_when_env_missing(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            (REPO_ROOT / "proxy-test-minimax-and-local.yaml").read_text(encoding="utf-8")
        )

        rendered = module.build_runtime_config_text(
            parsed,
            {},
            listen_host="127.0.0.1",
            listen_port=19999,
            trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
        )

        self.assertNotIn("LOCAL-QWEN:", rendered)
        self.assertNotIn('qwen-local: "LOCAL-QWEN:', rendered)
        self.assertNotIn('claude-opus-4-6: "LOCAL-QWEN:', rendered)

    def test_build_runtime_config_preserves_unknown_top_level_settings(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            textwrap.dedent(
                """
                listen: 127.0.0.1:18888
                feature_flags:
                  responses_passthrough: true
                  allowed_clients:
                    - codex
                    - gemini
                upstream_timeout_secs: 120
                upstreams:
                  MINIMAX-ANTHROPIC:
                    api_root: "https://api.minimaxi.com/anthropic/v1"
                    format: anthropic
                    credential_actual: "secret"
                    auth_policy: force_server
                model_aliases:
                  minimax-anth: "MINIMAX-ANTHROPIC:MiniMax-M2.7-highspeed"
                debug_trace:
                  path: /tmp/trace.jsonl
                  max_text_chars: 16384
                """
            )
        )

        rendered = module.build_runtime_config_text(
            parsed,
            {},
            listen_host="127.0.0.1",
            listen_port=19999,
            trace_path=pathlib.Path("/tmp/cli-matrix-trace.jsonl"),
        )

        self.assertIn("listen: 127.0.0.1:19999", rendered)
        self.assertIn("feature_flags:", rendered)
        self.assertIn("responses_passthrough: true", rendered)
        self.assertIn("- codex", rendered)
        self.assertIn("- gemini", rendered)

    def test_build_client_env_isolates_user_state_for_all_clients(self):
        module = load_module()
        base_env = {
            "PATH": "/usr/bin",
            "HOME": "/home/user",
            "OPENAI_API_KEY": "real-openai",
            "ANTHROPIC_API_KEY": "real-anthropic",
            "GEMINI_API_KEY": "real-gemini",
        }

        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir)
            codex_env = module.build_client_env(
                "codex", base_env, "http://127.0.0.1:18888", root / "codex-home"
            )
            claude_env = module.build_client_env(
                "claude", base_env, "http://127.0.0.1:18888", root / "claude-home"
            )
            gemini_env = module.build_client_env(
                "gemini", base_env, "http://127.0.0.1:18888", root / "gemini-home"
            )

        self.assertEqual(codex_env["OPENAI_API_KEY"], "dummy")
        self.assertEqual(
            codex_env["OPENAI_BASE_URL"], "http://127.0.0.1:18888/openai/v1"
        )
        self.assertNotEqual(codex_env["HOME"], "/home/user")
        self.assertNotIn("real-openai", codex_env.values())

        self.assertEqual(claude_env["ANTHROPIC_API_KEY"], "dummy")
        self.assertEqual(
            claude_env["ANTHROPIC_BASE_URL"], "http://127.0.0.1:18888/anthropic"
        )
        self.assertIn("CLAUDE_CONFIG_DIR", claude_env)

        self.assertEqual(gemini_env["GEMINI_API_KEY"], "dummy")
        self.assertEqual(
            gemini_env["GOOGLE_GEMINI_BASE_URL"], "http://127.0.0.1:18888/google"
        )
        self.assertEqual(gemini_env["HTTP_PROXY"], "")
        self.assertEqual(gemini_env["HTTPS_PROXY"], "")

    def test_build_client_env_writes_gemini_capacity_settings(self):
        module = load_module()
        base_env = {"PATH": "/usr/bin", "HOME": "/home/user"}

        with tempfile.TemporaryDirectory() as temp_dir:
            home_dir = pathlib.Path(temp_dir) / "gemini-home"
            gemini_env = module.build_client_env(
                "gemini",
                base_env,
                "http://127.0.0.1:18888",
                home_dir,
                model_name="minimax-openai",
                model_limits=module.ModelLimits(
                    context_window=200000,
                    max_output_tokens=128000,
                ),
            )
            settings_path = home_dir / ".gemini" / "settings.json"
            self.assertTrue(settings_path.exists())
            settings = json.loads(settings_path.read_text(encoding="utf-8"))

        self.assertEqual(gemini_env["HOME"], str(home_dir))
        self.assertEqual(settings["model"]["compressionThreshold"], 0.85)
        self.assertEqual(
            settings["modelConfigs"]["customOverrides"][0]["match"]["model"],
            "minimax-openai",
        )
        self.assertEqual(
            settings["modelConfigs"]["customOverrides"][0]["modelConfig"][
                "generateContentConfig"
            ]["maxOutputTokens"],
            128000,
        )
        self.assertIn("minimax-openai", settings["modelConfigs"]["modelDefinitions"])

    def test_build_client_env_preserves_explicit_rust_toolchain_homes(self):
        module = load_module()
        base_env = {
            "PATH": "/usr/bin",
            "HOME": "/home/user",
            "CARGO_HOME": "/opt/rust/cargo-home",
            "RUSTUP_HOME": "/opt/rust/rustup-home",
        }

        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir)
            codex_env = module.build_client_env(
                "codex", base_env, "http://127.0.0.1:18888", root / "codex-home"
            )
            claude_env = module.build_client_env(
                "claude", base_env, "http://127.0.0.1:18888", root / "claude-home"
            )
            gemini_env = module.build_client_env(
                "gemini", base_env, "http://127.0.0.1:18888", root / "gemini-home"
            )

        for client_env in (codex_env, claude_env, gemini_env):
            self.assertEqual(client_env["CARGO_HOME"], "/opt/rust/cargo-home")
            self.assertEqual(client_env["RUSTUP_HOME"], "/opt/rust/rustup-home")

    def test_build_client_env_skips_missing_host_rust_toolchain_homes(self):
        module = load_module()

        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir)
            host_home = root / "host-home"
            host_home.mkdir(parents=True, exist_ok=True)
            base_env = {
                "PATH": "/usr/bin",
                "HOME": str(host_home),
            }

            codex_env = module.build_client_env(
                "codex", base_env, "http://127.0.0.1:18888", root / "codex-home"
            )
            claude_env = module.build_client_env(
                "claude", base_env, "http://127.0.0.1:18888", root / "claude-home"
            )
            gemini_env = module.build_client_env(
                "gemini", base_env, "http://127.0.0.1:18888", root / "gemini-home"
            )

        for client_env in (codex_env, claude_env, gemini_env):
            self.assertNotIn("CARGO_HOME", client_env)
            self.assertNotIn("RUSTUP_HOME", client_env)

    def test_build_codex_model_catalog_includes_capacity_and_structured_metadata(self):
        module = load_module()

        payload = module.build_codex_model_catalog(
            "minimax-openai",
            module.ModelLimits(context_window=200000, max_output_tokens=128000),
            module.CodexModelMetadata(
                input_modalities=("text",),
                supports_search_tool=False,
            ),
        )

        model_entry = payload["models"][0]
        self.assertEqual(model_entry["slug"], "minimax-openai")
        self.assertEqual(model_entry["display_name"], "minimax-openai")
        self.assertIn("supported_reasoning_levels", model_entry)
        self.assertEqual(model_entry["shell_type"], "shell_command")
        self.assertEqual(model_entry["visibility"], "list")
        self.assertTrue(model_entry["supported_in_api"])
        self.assertEqual(model_entry["priority"], 0)
        self.assertIn("base_instructions", model_entry)
        self.assertFalse(model_entry["supports_reasoning_summaries"])
        self.assertFalse(model_entry["support_verbosity"])
        self.assertEqual(
            model_entry["truncation_policy"],
            {"mode": "bytes", "limit": 10000},
        )
        self.assertEqual(model_entry["apply_patch_tool_type"], "freeform")
        self.assertFalse(model_entry["supports_parallel_tool_calls"])
        self.assertEqual(model_entry["experimental_supported_tools"], [])
        self.assertEqual(model_entry["context_window"], 200000)
        self.assertEqual(model_entry["auto_compact_token_limit"], 61200)
        self.assertEqual(model_entry["input_modalities"], ["text"])
        self.assertFalse(model_entry["supports_search_tool"])

    def test_build_codex_model_catalog_keeps_85_percent_of_context_when_output_limit_missing(self):
        module = load_module()

        payload = module.build_codex_model_catalog(
            "vision-openai",
            module.ModelLimits(context_window=200000),
            module.CodexModelMetadata(
                input_modalities=("text", "image"),
                supports_search_tool=True,
            ),
        )

        model_entry = payload["models"][0]
        self.assertEqual(model_entry["apply_patch_tool_type"], "freeform")
        self.assertEqual(model_entry["context_window"], 200000)
        self.assertEqual(model_entry["auto_compact_token_limit"], 170000)
        self.assertEqual(model_entry["input_modalities"], ["text", "image"])
        self.assertTrue(model_entry["supports_search_tool"])

    def test_build_codex_model_catalog_rejects_non_positive_input_budget(self):
        module = load_module()

        with self.assertRaisesRegex(
            ValueError, "max_output_tokens must be less than context_window"
        ):
            module.build_codex_model_catalog(
                "broken-openai",
                module.ModelLimits(context_window=200000, max_output_tokens=200000),
                module.CodexModelMetadata(
                    input_modalities=("text",),
                    supports_search_tool=False,
                ),
            )

    def test_build_codex_model_catalog_can_emit_metadata_without_context_window(self):
        module = load_module()

        payload = module.build_codex_model_catalog(
            "minimax-openai",
            module.ModelLimits(max_output_tokens=128000),
            module.CodexModelMetadata(
                input_modalities=("text",),
                supports_search_tool=False,
            ),
        )

        model_entry = payload["models"][0]
        self.assertEqual(model_entry["apply_patch_tool_type"], "freeform")
        self.assertNotIn("context_window", model_entry)
        self.assertNotIn("auto_compact_token_limit", model_entry)
        self.assertEqual(model_entry["input_modalities"], ["text"])
        self.assertFalse(model_entry["supports_search_tool"])

    def test_build_codex_model_catalog_rejects_internal_tool_artifacts_in_public_payload(self):
        module = load_module()
        bad_entry = module.default_codex_catalog_entry("minimax-openai")
        bad_entry["experimental_supported_tools"] = ["__llmup_custom__apply_patch"]

        with mock.patch.object(module, "default_codex_catalog_entry", return_value=bad_entry):
            with self.assertRaisesRegex(ValueError, "__llmup_custom__apply_patch"):
                module.build_codex_model_catalog(
                    "minimax-openai",
                    module.ModelLimits(context_window=200000, max_output_tokens=128000),
                    module.CodexModelMetadata(
                        input_modalities=("text",),
                        supports_search_tool=False,
                    ),
                )

    def test_real_codex_rejects_previous_minimal_catalog_shape(self):
        _module = load_module()

        returncode, output, timed_out = codex_catalog_probe(
            {
                "models": [
                    {
                        "slug": "minimax-openai",
                        "display_name": "minimax-openai",
                        "context_window": 200000,
                        "auto_compact_token_limit": 170000,
                        "input_modalities": ["text"],
                        "supports_search_tool": False,
                    }
                ]
            }
        )

        self.assertFalse(timed_out)
        self.assertEqual(returncode, 1)
        self.assertIn("failed to parse model_catalog_json", output)
        self.assertIn("as JSON", output)

    def test_real_codex_accepts_generated_catalog_shape(self):
        module = load_module()

        payload = module.build_codex_model_catalog(
            "minimax-openai",
            module.ModelLimits(context_window=200000, max_output_tokens=128000),
            module.CodexModelMetadata(
                input_modalities=("text",),
                supports_search_tool=False,
            ),
        )
        returncode, output, timed_out = codex_catalog_probe(payload)

        self.assertTrue(timed_out or returncode != 1, output)
        self.assertNotIn("failed to parse model_catalog_json", output)
        self.assertNotIn("missing field `", output)

    def test_real_codex_accepts_metadata_only_catalog_shape(self):
        module = load_module()

        payload = module.build_codex_model_catalog(
            "minimax-openai",
            module.ModelLimits(max_output_tokens=128000),
            module.CodexModelMetadata(
                input_modalities=("text",),
                supports_search_tool=False,
            ),
        )
        returncode, output, timed_out = codex_catalog_probe(payload)

        self.assertTrue(timed_out or returncode != 1, output)
        self.assertNotIn("failed to parse model_catalog_json", output)
        self.assertNotIn("missing field `", output)

    def test_default_config_codex_catalog_injects_85_percent_compact_limit(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            (REPO_ROOT / "proxy-test-minimax-and-local.yaml").read_text(encoding="utf-8")
        )
        model_limits = module.resolve_model_limits(parsed, "minimax-openai")
        codex_metadata = module.resolve_codex_model_metadata(parsed, "minimax-openai")

        with tempfile.TemporaryDirectory() as temp_dir:
            home_dir = pathlib.Path(temp_dir)
            args = module.build_codex_catalog_args(
                home_dir,
                "minimax-openai",
                model_limits,
                codex_metadata,
            )
            catalog_path = home_dir / ".codex" / "catalog.json"
            payload = json.loads(catalog_path.read_text(encoding="utf-8"))

        self.assertIn("model_catalog_json", " ".join(args))
        self.assertIn('web_search="disabled"', " ".join(args))
        self.assertIn('tools.view_image=false', " ".join(args))
        self.assertEqual(
            payload["models"][0]["context_window"],
            200000,
        )
        self.assertEqual(
            payload["models"][0]["auto_compact_token_limit"],
            61200,
        )
        self.assertEqual(payload["models"][0]["apply_patch_tool_type"], "freeform")
        self.assertEqual(payload["models"][0]["input_modalities"], ["text"])
        self.assertFalse(payload["models"][0]["supports_search_tool"])

    def test_default_config_codex_catalog_compacts_before_observed_live_failure_tokens(self):
        module = load_module()
        parsed = module.parse_proxy_source(
            (REPO_ROOT / "proxy-test-minimax-and-local.yaml").read_text(encoding="utf-8")
        )
        model_limits = module.resolve_model_limits(parsed, "minimax-openai")
        codex_metadata = module.resolve_codex_model_metadata(parsed, "minimax-openai")

        payload = module.build_codex_model_catalog(
            "minimax-openai",
            model_limits,
            codex_metadata,
        )

        compact_limit = payload["models"][0]["auto_compact_token_limit"]
        observed_live_failure_input_tokens = 133603
        self.assertLess(compact_limit, observed_live_failure_input_tokens)
        self.assertEqual(compact_limit, 61200)

    def test_build_client_command_uses_known_good_gemini_sandbox_flag_form(self):
        module = load_module()
        lane = make_lane(module, name="minimax-openai", proxy_model="minimax-openai")
        fixture = make_fixture(module)

        command = module.build_client_command(
            "gemini",
            "http://127.0.0.1:18888",
            lane,
            fixture,
            pathlib.Path("/tmp/workspace"),
        )

        self.assertIn("--sandbox=false", command)
        self.assertNotIn("--sandbox", command)

    def test_build_client_command_injects_codex_model_catalog_for_capacity_aware_alias(self):
        module = load_module()
        lane = make_lane(module, name="minimax-openai", proxy_model="minimax-openai")
        lane.limits = module.ModelLimits(
            context_window=200000,
            max_output_tokens=128000,
        )
        lane.codex_metadata = module.CodexModelMetadata(
            input_modalities=("text",),
            supports_search_tool=False,
        )
        fixture = make_fixture(module)
        workspace = pathlib.Path("/tmp/workspace").resolve()
        home_dir = pathlib.Path("/tmp/codex-home").resolve()

        command = module.build_client_command(
            "codex",
            "http://127.0.0.1:18888",
            lane,
            fixture,
            workspace,
            client_home=home_dir,
        )

        joined = " ".join(command)
        self.assertIn("model_catalog_json", joined)
        self.assertIn(str(home_dir / ".codex" / "catalog.json"), joined)
        self.assertIn('web_search="disabled"', joined)
        self.assertIn('tools.view_image=false', joined)

    def test_build_client_command_respects_codex_metadata_search_override(self):
        module = load_module()
        lane = make_lane(module, name="vision-openai", proxy_model="vision-openai")
        lane.limits = module.ModelLimits(context_window=200000)
        lane.codex_metadata = module.CodexModelMetadata(
            input_modalities=("text", "image"),
            supports_search_tool=True,
        )
        fixture = make_fixture(module)

        command = module.build_client_command(
            "codex",
            "http://127.0.0.1:18888",
            lane,
            fixture,
            pathlib.Path("/tmp/workspace").resolve(),
            client_home=pathlib.Path("/tmp/codex-home").resolve(),
        )

        joined = " ".join(command)
        self.assertIn("model_catalog_json", joined)
        self.assertNotIn('web_search="disabled"', joined)
        self.assertNotIn('tools.view_image=false', joined)

    def test_build_client_command_rejects_internal_tool_artifacts_in_public_args(self):
        module = load_module()
        lane = make_lane(module, name="minimax-openai", proxy_model="minimax-openai")
        fixture = make_fixture(module)

        with mock.patch.object(
            module,
            "build_codex_catalog_args",
            return_value=[
                "-c",
                'tool_identity_contract="__llmup_custom__apply_patch"',
            ],
        ):
            with self.assertRaisesRegex(ValueError, "__llmup_custom__apply_patch"):
                module.build_client_command(
                    "codex",
                    "http://127.0.0.1:18888",
                    lane,
                    fixture,
                    pathlib.Path("/tmp/workspace").resolve(),
                    client_home=pathlib.Path("/tmp/codex-home").resolve(),
                )

    def test_prepare_proxy_env_keeps_dotenv_scoped_to_proxy_only(self):
        module = load_module()
        base_env = {
            "PATH": "/usr/bin",
            "HOME": "/home/user",
            "OPENAI_API_KEY": "real-openai",
        }
        dotenv_env = {
            "PRESET_ENDPOINT_API_KEY": "proxy-only-secret",
            "LOCAL_QWEN_BASE_URL": "http://127.0.0.1:9997/v1",
        }

        proxy_env = module.prepare_proxy_env(base_env, dotenv_env)

        with tempfile.TemporaryDirectory() as temp_dir:
            client_env = module.build_client_env(
                "codex",
                base_env,
                "http://127.0.0.1:18888",
                pathlib.Path(temp_dir) / "codex-home",
            )

        self.assertEqual(proxy_env["PRESET_ENDPOINT_API_KEY"], "proxy-only-secret")
        self.assertEqual(proxy_env["LOCAL_QWEN_BASE_URL"], "http://127.0.0.1:9997/v1")
        self.assertNotIn("PRESET_ENDPOINT_API_KEY", client_env)
        self.assertNotIn("LOCAL_QWEN_BASE_URL", client_env)

    def test_prepare_proxy_env_persists_replay_marker_key_within_run_root(self):
        module = load_module()
        base_env = {"PATH": "/usr/bin", "HOME": "/home/user"}

        with tempfile.TemporaryDirectory() as temp_dir:
            runtime_root = pathlib.Path(temp_dir)
            first_env = module.prepare_proxy_env(base_env, {}, runtime_root)
            second_env = module.prepare_proxy_env(base_env, {}, runtime_root)
            key_path = runtime_root / module.REPLAY_MARKER_KEY_FILENAME

            self.assertTrue(key_path.exists())
            self.assertEqual(
                first_env[module.REPLAY_MARKER_KEY_ENV],
                second_env[module.REPLAY_MARKER_KEY_ENV],
            )
            self.assertEqual(
                key_path.read_text(encoding="utf-8").strip(),
                first_env[module.REPLAY_MARKER_KEY_ENV],
            )

    def test_ensure_replay_marker_key_regenerates_blank_file(self):
        module = load_module()

        with tempfile.TemporaryDirectory() as temp_dir:
            runtime_root = pathlib.Path(temp_dir)
            key_path = runtime_root / module.REPLAY_MARKER_KEY_FILENAME
            key_path.write_text("   \n", encoding="utf-8")

            marker_key = module.ensure_replay_marker_key(runtime_root)

            self.assertEqual(
                key_path.read_text(encoding="utf-8").strip(),
                marker_key,
            )

        self.assertTrue(marker_key)
        self.assertNotEqual(marker_key, "   ")

    def test_stop_proxy_uses_short_fixed_tail_wait_after_kill(self):
        module = load_module()

        class FakeProcess:
            def __init__(self):
                self.wait_timeouts = []

            def poll(self):
                return None

            def terminate(self):
                return None

            def kill(self):
                return None

            def wait(self, timeout=None):
                self.wait_timeouts.append(timeout)
                if len(self.wait_timeouts) == 1:
                    raise subprocess.TimeoutExpired(cmd="proxy", timeout=timeout)
                return 0

        process = FakeProcess()

        module.stop_proxy(process, terminate_grace_secs=15)

        self.assertEqual(
            process.wait_timeouts,
            [15, module.DEFAULT_POST_KILL_WAIT_SECS],
        )

    def test_expand_matrix_respects_phase_and_skip_slow(self):
        module = load_module()
        lane = module.Lane(
            name="minimax-anth",
            required=True,
            enabled=True,
            proxy_model="minimax-anth",
            upstream_name="MINIMAX-ANTHROPIC",
            skip_reason=None,
        )
        fixtures = [
            module.TaskFixture(
                fixture_id="smoke_pong",
                kind="smoke",
                prompt="Reply with PONG",
                verifier={"type": "contains", "value": "PONG"},
                timeout_secs=90,
                workspace_template=None,
            ),
            module.TaskFixture(
                fixture_id="bugfix",
                kind="long_horizon",
                prompt="Fix calc.py",
                verifier={"type": "file_contains", "path": "calc.py", "needle": "a + b"},
                timeout_secs=180,
                workspace_template=pathlib.Path("bugfix"),
            ),
        ]

        cases = module.expand_matrix(
            clients=["codex", "claude", "gemini"],
            lanes=[lane],
            fixtures=fixtures,
            phase="basic",
            skip_slow=True,
        )

        self.assertEqual([case.client_name for case in cases], ["codex", "claude", "gemini"])
        self.assertTrue(all(case.fixture.kind == "smoke" for case in cases))

    def test_expand_matrix_excludes_qwen_local_from_long_horizon_cases(self):
        module = load_module()
        lanes = [
            module.Lane(
                name="minimax-anth",
                required=True,
                enabled=True,
                proxy_model="minimax-anth",
                upstream_name="MINIMAX-ANTHROPIC",
                skip_reason=None,
            ),
            module.Lane(
                name="qwen-local",
                required=False,
                enabled=True,
                proxy_model="qwen-local",
                upstream_name="LOCAL-QWEN",
                skip_reason=None,
            ),
        ]
        fixtures = [
            module.TaskFixture(
                fixture_id="smoke_pong",
                kind="smoke",
                prompt="Reply with PONG",
                verifier={"type": "contains", "value": "PONG"},
                timeout_secs=90,
                workspace_template=None,
            ),
            module.TaskFixture(
                fixture_id="python_bugfix",
                kind="long_horizon",
                prompt="Fix calc.py",
                verifier={"type": "file_contains", "path": "calc.py", "needle": "a + b"},
                timeout_secs=180,
                workspace_template=pathlib.Path("bugfix"),
            ),
        ]

        cases = module.expand_matrix(
            clients=["codex", "gemini"],
            lanes=lanes,
            fixtures=fixtures,
            phase="all",
            skip_slow=False,
        )

        self.assertIn("codex__qwen-local__smoke_pong", [case.case_id for case in cases])
        self.assertIn("gemini__qwen-local__smoke_pong", [case.case_id for case in cases])
        self.assertNotIn(
            "codex__qwen-local__python_bugfix", [case.case_id for case in cases]
        )
        self.assertNotIn(
            "gemini__qwen-local__python_bugfix", [case.case_id for case in cases]
        )
        self.assertIn(
            "codex__minimax-anth__python_bugfix", [case.case_id for case in cases]
        )
        self.assertIn(
            "gemini__minimax-anth__python_bugfix", [case.case_id for case in cases]
        )

    def test_filter_matrix_cases_supports_explicit_case_ids(self):
        module = load_module()
        lane = module.Lane(
            name="minimax-anth",
            required=True,
            enabled=True,
            proxy_model="minimax-anth",
            upstream_name="MINIMAX-ANTHROPIC",
            skip_reason=None,
        )
        fixture = module.TaskFixture(
            fixture_id="smoke_pong",
            kind="smoke",
            prompt="Reply with PONG",
            verifier={"type": "contains", "value": "PONG"},
            timeout_secs=90,
            workspace_template=None,
        )
        cases = [
            module.MatrixCase(
                client_name="codex",
                lane=lane,
                fixture=fixture,
                case_id="codex__minimax-anth__smoke_pong",
            ),
            module.MatrixCase(
                client_name="gemini",
                lane=lane,
                fixture=fixture,
                case_id="gemini__minimax-anth__smoke_pong",
            ),
        ]

        filtered = module.filter_matrix_cases(
            cases, selected_case_ids=["gemini__minimax-anth__smoke_pong"]
        )

        self.assertEqual(
            [case.case_id for case in filtered], ["gemini__minimax-anth__smoke_pong"]
        )
        with self.assertRaisesRegex(ValueError, "unknown matrix case"):
            module.filter_matrix_cases(cases, selected_case_ids=["missing-case"])

    def test_classify_lane_health_skips_optional_qwen_probe_failures(self):
        module = load_module()
        optional_lane = module.Lane(
            name="qwen-local",
            required=False,
            enabled=True,
            proxy_model="qwen-local",
            upstream_name="LOCAL-QWEN",
            skip_reason=None,
        )
        required_lane = module.Lane(
            name="minimax-anth",
            required=True,
            enabled=True,
            proxy_model="minimax-anth",
            upstream_name="MINIMAX-ANTHROPIC",
            skip_reason=None,
        )

        self.assertEqual(
            module.classify_lane_health(optional_lane, "connection refused")[0], "skipped"
        )
        self.assertEqual(
            module.classify_lane_health(required_lane, "connection refused")[0], "failed"
        )

    def test_probe_lane_accepts_valid_responses_shape_without_exact_probe_text(self):
        module = load_module()
        lane = make_lane(
            module,
            name="qwen-local",
            required=False,
            proxy_model="qwen-local",
            upstream_name="LOCAL-QWEN",
        )

        with mock.patch.object(
            module,
            "http_json",
            return_value=(
                200,
                json.dumps(
                    {
                        "id": "resp_qwen",
                        "object": "response",
                        "status": "completed",
                        "output": [
                            {
                                "type": "message",
                                "content": [
                                    {
                                        "type": "output_text",
                                        "text": "Sure, here is a semantic match.",
                                    }
                                ],
                            }
                        ],
                    }
                ),
            ),
        ):
            self.assertIsNone(module.probe_lane("http://127.0.0.1:18888", lane))

    def test_probe_lane_rejects_http_200_body_without_response_shape(self):
        module = load_module()
        lane = make_lane(
            module,
            name="qwen-local",
            required=False,
            proxy_model="qwen-local",
            upstream_name="LOCAL-QWEN",
        )

        with mock.patch.object(module, "http_json", return_value=(200, '{"ok":true}')):
            self.assertIn("valid response shape", module.probe_lane("http://127.0.0.1:18888", lane))

    def test_verify_fixture_output_rejects_comment_only_python_bugfix_edits(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="python_bugfix",
            verifier={
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
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
            (workspace_dir / "calc.py").write_text(
                textwrap.dedent(
                    """
                    def add(a, b):
                        return a - b

                    # return a + b

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

            ok, message = module.verify_fixture_output(fixture, "", workspace_dir)

        self.assertFalse(ok)
        self.assertIn("calc.py", message)
        self.assertIn("return a + b", message)

    def test_verify_fixture_output_checks_main_py_behavior_for_python_bugfix(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="python_bugfix",
            verifier={
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
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
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
                'print("2 + 3 = 6")\nprint("-1 + 5 = 4")\nprint("0 + 0 = 0")\nprint("4 * 5 = 20")\n',
                encoding="utf-8",
            )

            ok, message = module.verify_fixture_output(fixture, "", workspace_dir)

        self.assertFalse(ok)
        self.assertIn("main.py", message)
        self.assertIn("2 + 3 = 5", message)

    def test_verify_fixture_output_accepts_python_bugfix_fix_when_behavior_matches(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="python_bugfix",
            verifier={
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
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            workspace_dir = pathlib.Path(temp_dir)
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

            ok, message = module.verify_fixture_output(fixture, "", workspace_dir)

        self.assertTrue(ok, message)

    def test_verify_fixture_output_supports_stdout_contract_without_internal_tool_artifacts(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="tool_identity_public_contract",
            verifier={
                "type": "stdout_contract",
                "not_contains": ["__llmup_custom__"],
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "Public tools include apply_patch.",
            workspace_dir=None,
        )

        self.assertTrue(ok, message)
        self.assertEqual(message, "")

    def test_verify_fixture_output_rejects_stdout_contract_internal_tool_artifacts(self):
        module = load_module()
        fixture = make_fixture(
            module,
            fixture_id="tool_identity_public_contract",
            verifier={
                "type": "stdout_contract",
                "not_contains": ["__llmup_custom__"],
            },
        )

        ok, message = module.verify_fixture_output(
            fixture,
            "Available tool: __llmup_custom__apply_patch",
            workspace_dir=None,
        )

        self.assertFalse(ok)
        self.assertIn("__llmup_custom__", message)

    def test_tool_identity_fixture_is_present_and_uses_stdout_contract(self):
        payload = json.loads(TOOL_IDENTITY_FIXTURE_PATH.read_text(encoding="utf-8"))

        self.assertEqual(payload["id"], "tool_identity_public_contract")
        self.assertEqual(payload["kind"], "smoke")
        self.assertEqual(payload["verifier"]["type"], "stdout_contract")
        self.assertIn("__llmup_custom__", payload["verifier"]["not_contains"])
        self.assertIn("apply_patch", payload["prompt"])

    def test_run_matrix_case_feeds_claude_prompt_via_stdin(self):
        module = load_module()
        case = make_case(
            module,
            client_name="claude",
            lane=make_lane(module),
            fixture=make_fixture(module, prompt="Reply with exactly PONG"),
        )
        captured = {}

        def fake_run(command, **kwargs):
            captured["command"] = command
            captured["kwargs"] = kwargs
            return subprocess.CompletedProcess(command, 0, stdout="PONG\n", stderr="")

        with tempfile.TemporaryDirectory() as temp_dir:
            report_dir = pathlib.Path(temp_dir)
            with mock.patch.object(module.subprocess, "run", side_effect=fake_run):
                result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(result["status"], "passed")
        self.assertIn("--add-dir", captured["command"])
        self.assertNotIn(case.fixture.prompt, captured["command"])
        self.assertEqual(captured["kwargs"]["input"], case.fixture.prompt)
        self.assertNotIn("stdin", captured["kwargs"])

    def test_run_matrix_case_detaches_stdin_for_headless_gemini_runs(self):
        module = load_module()
        case = make_case(
            module,
            client_name="gemini",
            lane=make_lane(module, name="minimax-openai", proxy_model="minimax-openai"),
            fixture=make_fixture(module, timeout_secs=1),
        )
        child_code = "import sys; sys.stdin.read(); print('PONG')"
        original_stdin = os.dup(0)
        read_fd, write_fd = os.pipe()

        try:
            os.dup2(read_fd, 0)
            os.close(read_fd)
            with tempfile.TemporaryDirectory() as temp_dir:
                report_dir = pathlib.Path(temp_dir)
                with mock.patch.object(
                    module,
                    "build_client_command",
                    return_value=[sys.executable, "-c", child_code],
                ):
                    result = module.run_matrix_case(
                        case,
                        "http://127.0.0.1:18888",
                        report_dir,
                        {"PATH": os.environ.get("PATH", "")},
                    )
        finally:
            os.dup2(original_stdin, 0)
            os.close(original_stdin)
            os.close(write_fd)

        self.assertEqual(result["status"], "passed", result["message"])

    def test_run_matrix_case_reuses_runner_managed_gemini_home_across_cases(self):
        module = load_module()
        first_case = make_case(
            module,
            client_name="gemini",
            lane=make_lane(module, name="minimax-openai", proxy_model="minimax-openai"),
            case_id="gemini__minimax-openai__smoke_pong",
        )
        second_case = make_case(
            module,
            client_name="gemini",
            lane=make_lane(module, name="minimax-anth", proxy_model="minimax-anth"),
            case_id="gemini__minimax-anth__smoke_pong",
        )
        seen_homes = []

        def fake_run(command, **kwargs):
            seen_homes.append(kwargs["env"]["HOME"])
            return subprocess.CompletedProcess(command, 0, stdout="PONG\n", stderr="")

        with tempfile.TemporaryDirectory() as temp_dir:
            report_dir = pathlib.Path(temp_dir) / "reports" / "run-001"
            report_dir.mkdir(parents=True, exist_ok=True)
            with mock.patch.object(module.subprocess, "run", side_effect=fake_run):
                first_result = module.run_matrix_case(
                    first_case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )
                second_result = module.run_matrix_case(
                    second_case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(first_result["status"], "passed")
        self.assertEqual(second_result["status"], "passed")
        self.assertEqual(len(seen_homes), 2)
        self.assertEqual(seen_homes[0], seen_homes[1])
        self.assertIn("_runner_state", seen_homes[0])
        self.assertNotIn(first_case.case_id, seen_homes[0])
        self.assertNotIn(second_case.case_id, seen_homes[1])

    def test_run_matrix_case_normalizes_gemini_workspace_paths_when_report_dir_is_relative(self):
        module = load_module()
        case = make_case(
            module,
            client_name="gemini",
            lane=make_lane(module, name="minimax-openai", proxy_model="minimax-openai"),
        )
        observed = {}

        def fake_run(command, **kwargs):
            include_index = command.index("--include-directories")
            observed["include_dir"] = pathlib.Path(command[include_index + 1])
            observed["cwd"] = pathlib.Path(kwargs["cwd"]).resolve()
            observed["home"] = pathlib.Path(kwargs["env"]["HOME"]).resolve()
            return subprocess.CompletedProcess(command, 0, stdout="PONG\n", stderr="")

        with tempfile.TemporaryDirectory() as temp_dir:
            original_cwd = os.getcwd()
            os.chdir(temp_dir)
            try:
                report_dir = pathlib.Path("reports") / "run-001"
                report_dir.mkdir(parents=True, exist_ok=True)
                with mock.patch.object(module.subprocess, "run", side_effect=fake_run):
                    result = module.run_matrix_case(
                        case,
                        "http://127.0.0.1:18888",
                        report_dir,
                        {"PATH": os.environ.get("PATH", "")},
                    )
            finally:
                os.chdir(original_cwd)

        self.assertEqual(result["status"], "passed", result["message"])
        self.assertTrue(
            observed["include_dir"].is_absolute(),
            f"expected absolute --include-directories path, got {observed['include_dir']}",
        )
        self.assertEqual(observed["include_dir"], observed["cwd"])
        self.assertTrue(observed["home"].is_absolute())
        self.assertIn("_runner_state", str(observed["home"]))

    def test_run_matrix_case_extends_only_first_gemini_bootstrap_timeout(self):
        module = load_module()
        case = make_case(
            module,
            client_name="gemini",
            lane=make_lane(module, name="minimax-openai", proxy_model="minimax-openai"),
            fixture=make_fixture(module, timeout_secs=5),
        )
        observed_timeouts = []

        def fake_run(command, **kwargs):
            observed_timeouts.append(kwargs["timeout"])
            home_dir = pathlib.Path(kwargs["env"]["HOME"])
            rg_path = home_dir / ".gemini" / "tmp" / "bin" / "rg"
            rg_path.parent.mkdir(parents=True, exist_ok=True)
            rg_path.write_text("", encoding="utf-8")
            return subprocess.CompletedProcess(command, 0, stdout="PONG\n", stderr="")

        with tempfile.TemporaryDirectory() as temp_dir:
            report_dir = pathlib.Path(temp_dir) / "reports" / "run-001"
            report_dir.mkdir(parents=True, exist_ok=True)
            with mock.patch.object(module.subprocess, "run", side_effect=fake_run):
                first_result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )
                second_result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(first_result["status"], "passed")
        self.assertEqual(second_result["status"], "passed")
        self.assertEqual(
            observed_timeouts,
            [
                module.DEFAULT_TIMEOUT_POLICY.gemini_bootstrap_timeout_secs,
                module.DEFAULT_TIMEOUT_POLICY.case_timeout_floor_secs,
            ],
        )

    def test_default_timeout_policy_uses_generous_real_cli_thresholds(self):
        module = load_module()

        self.assertGreater(module.DEFAULT_TIMEOUT_POLICY.case_timeout_floor_secs, 120)
        self.assertGreater(
            module.DEFAULT_TIMEOUT_POLICY.long_horizon_timeout_floor_secs, 120
        )
        self.assertGreater(
            module.DEFAULT_TIMEOUT_POLICY.gemini_bootstrap_timeout_secs, 120
        )

    def test_resolve_case_timeout_secs_uses_structured_floors(self):
        module = load_module()
        short_case = make_case(
            module,
            client_name="codex",
            fixture=make_fixture(module, timeout_secs=30),
        )
        long_case = make_case(
            module,
            client_name="codex",
            fixture=module.TaskFixture(
                fixture_id="python_bugfix",
                kind="long_horizon",
                prompt="Fix calc.py",
                verifier={"type": "contains", "value": "ok"},
                timeout_secs=60,
                workspace_template=None,
            ),
        )
        policy = module.TimeoutPolicy(
            proxy_health_timeout_secs=45,
            case_timeout_floor_secs=240,
            long_horizon_timeout_floor_secs=420,
            gemini_bootstrap_timeout_secs=360,
            process_terminate_grace_secs=15,
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            home_dir = pathlib.Path(temp_dir)
            self.assertEqual(
                module.resolve_case_timeout_secs(short_case, home_dir, policy),
                240,
            )
            self.assertEqual(
                module.resolve_case_timeout_secs(long_case, home_dir, policy),
                420,
            )

    def test_run_matrix_case_uses_absolute_workspace_paths_for_gemini(self):
        module = load_module()
        case = make_case(
            module,
            client_name="gemini",
            lane=make_lane(module, name="minimax-openai", proxy_model="minimax-openai"),
        )
        observed = {}

        def fake_run(command, **kwargs):
            observed["command"] = command
            observed["cwd"] = kwargs["cwd"]
            observed["home"] = kwargs["env"]["HOME"]
            return subprocess.CompletedProcess(command, 0, stdout="PONG\n", stderr="")

        original_cwd = os.getcwd()
        with tempfile.TemporaryDirectory() as temp_dir:
            temp_root = pathlib.Path(temp_dir)
            os.chdir(temp_root)
            try:
                report_dir = pathlib.Path("reports") / "run-001"
                report_dir.mkdir(parents=True, exist_ok=True)
                with mock.patch.object(module.subprocess, "run", side_effect=fake_run):
                    result = module.run_matrix_case(
                        case,
                        "http://127.0.0.1:18888",
                        report_dir,
                        {"PATH": os.environ.get("PATH", "")},
                    )
            finally:
                os.chdir(original_cwd)

        self.assertEqual(result["status"], "passed")
        self.assertTrue(pathlib.Path(observed["cwd"]).is_absolute())
        self.assertTrue(pathlib.Path(observed["home"]).is_absolute())
        include_idx = observed["command"].index("--include-directories") + 1
        include_dir = pathlib.Path(observed["command"][include_idx])
        self.assertTrue(include_dir.is_absolute())
        self.assertEqual(include_dir, pathlib.Path(observed["cwd"]))

    def test_run_matrix_case_keeps_failures_for_enabled_optional_lane(self):
        module = load_module()
        case = make_case(
            module,
            client_name="gemini",
            lane=module.Lane(
                name="qwen-local",
                required=False,
                enabled=True,
                proxy_model="qwen-local",
                upstream_name="LOCAL-QWEN",
                skip_reason=None,
            ),
            fixture=make_fixture(module),
        )

        with tempfile.TemporaryDirectory() as temp_dir:
            report_dir = pathlib.Path(temp_dir)
            with mock.patch.object(
                module.subprocess,
                "run",
                return_value=subprocess.CompletedProcess(
                    ["gemini", "--prompt", case.fixture.prompt],
                    7,
                    stdout="",
                    stderr="boom",
                ),
            ):
                result = module.run_matrix_case(
                    case,
                    "http://127.0.0.1:18888",
                    report_dir,
                    {"PATH": os.environ.get("PATH", "")},
                )

        self.assertEqual(result["status"], "failed")
        self.assertEqual(result["message"], "exit code 7")

    def test_run_proxy_only_skips_client_binary_checks(self):
        module = load_module()

        class FakeProcess:
            def poll(self):
                return 0

            def wait(self, timeout=None):
                return 0

        observed = {}

        def fake_ensure_required_binaries(clients, proxy_binary):
            observed["clients"] = list(clients)
            observed["proxy_binary"] = proxy_binary

        with tempfile.TemporaryDirectory() as temp_dir:
            reports_root = pathlib.Path(temp_dir) / "reports"
            binary_path = pathlib.Path(temp_dir) / "fake-proxy"
            runtime_config_path = pathlib.Path(temp_dir) / "runtime-config.yaml"
            proxy_stdout_path = pathlib.Path(temp_dir) / "proxy.stdout.log"
            proxy_stderr_path = pathlib.Path(temp_dir) / "proxy.stderr.log"
            stdout = io.StringIO()
            with mock.patch.object(
                module, "ensure_required_binaries", side_effect=fake_ensure_required_binaries
            ), mock.patch.object(
                module,
                "selected_clients",
                side_effect=AssertionError(
                    "selected_clients should not run in proxy-only mode"
                ),
            ), mock.patch.object(
                module,
                "start_proxy",
                return_value=(
                    FakeProcess(),
                    runtime_config_path,
                    proxy_stdout_path,
                    proxy_stderr_path,
                ),
            ), mock.patch.object(module, "wait_for_health"), mock.patch.object(
                module, "stop_proxy"
            ), mock.patch(
                "sys.stdout", stdout
            ):
                exit_code = module.run(
                    [
                        "--proxy-only",
                        "--config-source",
                        str(REPO_ROOT / "proxy-test-minimax-and-local.yaml"),
                        "--env-file",
                        str(pathlib.Path(temp_dir) / "missing.env"),
                        "--fixtures-root",
                        str(REPO_ROOT / "scripts" / "fixtures" / "cli_matrix"),
                        "--reports-root",
                        str(reports_root),
                        "--binary",
                        str(binary_path),
                    ]
                )

        self.assertEqual(exit_code, 0)
        self.assertEqual(observed["clients"], [])
        self.assertEqual(observed["proxy_binary"], binary_path)
        self.assertIn("Proxy healthy at http://127.0.0.1:18888", stdout.getvalue())

    def test_resolve_cli_args_supports_list_matrix_and_case_filters(self):
        module = load_module()

        args = module.resolve_cli_args(
            [
                "--list-matrix",
                "--case",
                "codex__minimax-anth__smoke_pong",
                "--case",
                "gemini__minimax-openai__smoke_pong",
            ]
        )

        self.assertTrue(args.list_matrix)
        self.assertEqual(
            args.case,
            [
                "codex__minimax-anth__smoke_pong",
                "gemini__minimax-openai__smoke_pong",
            ],
        )

    def test_write_reports_creates_json_markdown_and_latest_symlink(self):
        module = load_module()

        with tempfile.TemporaryDirectory() as temp_dir:
            reports_root = pathlib.Path(temp_dir)
            run_dir = module.write_reports(
                reports_root,
                {
                    "started_at": "2026-04-17T00:00:00Z",
                    "finished_at": "2026-04-17T00:01:00Z",
                    "pass": 1,
                    "fail": 0,
                    "skip": 1,
                },
                [
                    {
                        "case_id": "codex__minimax-anth__smoke_pong",
                        "client": "codex",
                        "lane": "minimax-anth",
                        "fixture": "smoke_pong",
                        "status": "passed",
                        "message": "contained PONG",
                    },
                    {
                        "case_id": "gemini__qwen-local__smoke_pong",
                        "client": "gemini",
                        "lane": "qwen-local",
                        "fixture": "smoke_pong",
                        "status": "skipped",
                        "message": "optional lane unavailable",
                    },
                ],
                timestamp="20260417T000000Z",
            )

            self.assertTrue((run_dir / "report.json").exists())
            self.assertTrue((run_dir / "report.md").exists())
            self.assertTrue((run_dir / "results.jsonl").exists())
            latest = reports_root / "latest"
            self.assertTrue(latest.is_symlink())
            self.assertEqual(latest.resolve(), run_dir.resolve())

            summary = json.loads((run_dir / "report.json").read_text(encoding="utf-8"))
            self.assertEqual(summary["pass"], 1)
            self.assertIn(
                "codex__minimax-anth__smoke_pong",
                (run_dir / "results.jsonl").read_text(encoding="utf-8"),
            )

    def test_docs_publish_the_locked_tool_identity_contract(self):
        for path in DOC_CONTRACT_PATHS:
            with self.subTest(path=path.name):
                text = path.read_text(encoding="utf-8")
                for line in LOCKED_TOOL_CONTRACT_LINES:
                    self.assertIn(line, text)

    def test_docs_do_not_describe_reserved_prefix_bridge_as_current_limitation_or_fallback(self):
        for path, forbidden_snippets in FORBIDDEN_LEGACY_TOOL_IDENTITY_LANGUAGE.items():
            with self.subTest(path=path.name):
                text = path.read_text(encoding="utf-8")
                for snippet in forbidden_snippets:
                    self.assertNotIn(snippet, text)

    def test_max_compat_docs_publish_stable_name_request_scoped_bridge_direction(self):
        for path, required_snippets in REQUIRED_TOOL_BRIDGE_DIRECTION_LANGUAGE.items():
            with self.subTest(path=path.name):
                text = path.read_text(encoding="utf-8")
                for snippet in required_snippets:
                    self.assertIn(snippet, text)


if __name__ == "__main__":
    unittest.main()
