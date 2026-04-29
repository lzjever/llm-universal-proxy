import importlib.util
import pathlib
import re
import sys
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
REAL_CLI_MATRIX_PATH = REPO_ROOT / "scripts" / "real_cli_matrix.py"
QUICKSTART_CONFIG_PATHS = (
    "README.md",
    "README_CN.md",
    "examples/quickstart-provider-neutral.yaml",
)
QUICKSTART_ALIASES = (
    "preset-openai-compatible",
    "preset-anthropic-compatible",
)
PRESET_ENV_KEYS = (
    "PRESET_OPENAI_ENDPOINT_BASE_URL",
    "PRESET_ANTHROPIC_ENDPOINT_BASE_URL",
    "PRESET_ENDPOINT_MODEL",
    "PRESET_ENDPOINT_API_KEY",
)
USER_ENTRY_DOCS = (
    "README.md",
    "README_CN.md",
    "docs/configuration.md",
    "docs/clients.md",
)
REASONING_COMPACTION_BOUNDARY_SNIPPETS = (
    "default/max_compat",
    "visible summary",
    "visible transcript",
    "strict/balanced",
    "opaque-only reasoning",
    "opaque-only compaction",
    "same-provider/native passthrough",
    "provider-owned state",
)


def load_real_cli_matrix():
    spec = importlib.util.spec_from_file_location(
        "real_cli_matrix_docs_homepage_contract",
        REAL_CLI_MATRIX_PATH,
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def normalized_whitespace(text: str) -> str:
    return " ".join(text.split())


class DocsHomepageContractTests(unittest.TestCase):
    def read_text(self, relative_path: str) -> str:
        return (REPO_ROOT / relative_path).read_text(encoding="utf-8")

    def assert_in_order(self, text: str, snippets: tuple[str, ...]) -> None:
        cursor = -1
        for snippet in snippets:
            next_index = text.find(snippet)
            self.assertNotEqual(next_index, -1, f"missing snippet: {snippet}")
            self.assertGreater(
                next_index,
                cursor,
                f"expected `{snippet}` after `{snippets[snippets.index(snippet) - 1]}`",
            )
            cursor = next_index

    def extract_quickstart_config(self, relative_path: str) -> str:
        text = self.read_text(relative_path)
        if relative_path.endswith(".yaml"):
            return text

        match = re.search(
            r"```yaml\n(?P<config>listen: 127\.0\.0\.1:8080\n.*?)\n```",
            text,
            re.DOTALL,
        )
        self.assertIsNotNone(match, f"missing quickstart YAML block in {relative_path}")
        return match.group("config")

    def assert_cli_ready_surface_contract(self, config_text: str) -> None:
        module = load_real_cli_matrix()
        parsed = module.parse_proxy_source(config_text)

        for alias in QUICKSTART_ALIASES:
            with self.subTest(alias=alias):
                alias_config = parsed.model_alias_configs.get(alias)
                self.assertIsNotNone(alias_config, f"missing alias: {alias}")
                upstream_name = module._target_upstream_name(alias_config.target)
                self.assertIsNotNone(upstream_name, f"missing target upstream for {alias}")
                surface = module._effective_surface_metadata(
                    parsed.upstream_surface_defaults.get(upstream_name),
                    alias_config.surface,
                )
                module._validate_live_surface_codex_requirements(
                    surface,
                    require_tool_flags=True,
                )
                self.assertEqual(surface.input_modalities, ("text",))
                self.assertFalse(surface.supports_search)
                self.assertFalse(surface.supports_view_image)
                self.assertEqual(surface.apply_patch_transport, "freeform")
                self.assertFalse(surface.supports_parallel_calls)

    def assert_provider_neutral_preset_contract(self, config_text: str) -> None:
        module = load_real_cli_matrix()
        parsed = module.parse_proxy_source(config_text)

        required_env = set(module.required_preset_endpoint_env_keys(parsed))
        self.assertEqual(required_env, set(PRESET_ENV_KEYS))

        self.assertEqual(
            parsed.upstreams["PRESET-OPENAI-COMPATIBLE"]["api_root"],
            "PRESET_OPENAI_ENDPOINT_BASE_URL",
        )
        self.assertEqual(
            parsed.upstreams["PRESET-ANTHROPIC-COMPATIBLE"]["api_root"],
            "PRESET_ANTHROPIC_ENDPOINT_BASE_URL",
        )
        self.assertEqual(
            parsed.upstreams["PRESET-OPENAI-COMPATIBLE"]["provider_key_env"],
            "PRESET_ENDPOINT_API_KEY",
        )
        self.assertEqual(
            parsed.upstreams["PRESET-ANTHROPIC-COMPATIBLE"]["provider_key_env"],
            "PRESET_ENDPOINT_API_KEY",
        )
        self.assertEqual(
            parsed.model_aliases["preset-openai-compatible"],
            "PRESET-OPENAI-COMPATIBLE:PRESET_ENDPOINT_MODEL",
        )
        self.assertEqual(
            parsed.model_aliases["preset-anthropic-compatible"],
            "PRESET-ANTHROPIC-COMPATIBLE:PRESET_ENDPOINT_MODEL",
        )

    def test_readmes_follow_product_homepage_section_order(self):
        self.assert_in_order(
            self.read_text("README.md"),
            (
                "## Quick Start",
                "## Codex / Claude Code / Gemini Basic Setup",
                "## Most Common Static Configuration",
                "## Dynamic Configuration Overview",
            ),
        )
        self.assert_in_order(
            self.read_text("README_CN.md"),
            (
                "## Quick Start",
                "## Codex / Claude Code / Gemini 基本接法",
                "## 最常用静态配置",
                "## 动态配置概要",
            ),
        )

    def test_readmes_make_provider_neutral_presets_the_homepage_story(self):
        readme = self.read_text("README.md")
        readme_cn = self.read_text("README_CN.md")

        for text in (readme, readme_cn):
            with self.subTest(language="README" if text is readme else "README_CN"):
                self.assertIn(
                    "examples/quickstart-provider-neutral.yaml",
                    text,
                )
                self.assertIn("preset-openai-compatible", text)
                self.assertIn("preset-anthropic-compatible", text)
                self.assertIn("PRESET_OPENAI_ENDPOINT_BASE_URL", text)
                self.assertIn("PRESET_ANTHROPIC_ENDPOINT_BASE_URL", text)
                self.assertIn("PRESET_ENDPOINT_MODEL", text)
                self.assertIn("PRESET_ENDPOINT_API_KEY", text)
                self.assertIn("--model preset-openai-compatible", text)
                self.assertIn("--model preset-anthropic-compatible", text)
                self.assertNotIn("gpt-5-4", text)
                self.assertNotIn("MiniMax-M2.7-highspeed", text)
                self.assertNotIn("proxy.yaml", text)

        self.assertNotIn("### Which endpoint should clients use?", readme)
        self.assertNotIn("### 客户端应该连哪个入口", readme_cn)
        self.assertNotIn("| Codex CLI | `/openai/v1` |", readme)
        self.assertNotIn("| Codex CLI | `/openai/v1` |", readme_cn)

        self.assertIn(
            "Reasoning effort such as `xhigh` is a client/request-side setting, not part of the model name.",
            readme,
        )
        self.assertIn(
            "像 `xhigh` 这样的 reasoning effort 是客户端/请求侧设置，不是模型名的一部分。",
            readme_cn,
        )

    def test_user_entry_docs_explain_minimax_as_replaceable_example_only(self):
        expectations = {
            "README.md": (
                "MiniMax is only a replaceable OpenAI-compatible example",
                "not a GA-required provider",
            ),
            "README_CN.md": (
                "MiniMax 只是一个可替换的 OpenAI-compatible 示例",
                "不是 GA 必需 provider",
            ),
            "docs/configuration.md": (
                "MiniMax is only a replaceable OpenAI-compatible example",
                "not a GA-required provider",
            ),
            "docs/clients.md": (
                "MiniMax is only a replaceable OpenAI-compatible example",
                "not a GA-required provider",
            ),
        }

        for relative_path, snippets in expectations.items():
            with self.subTest(path=relative_path):
                text = self.read_text(relative_path)
                for snippet in snippets:
                    self.assertIn(snippet, text)

    def test_user_entry_docs_explain_reasoning_compaction_continuity_boundary(self):
        for relative_path in USER_ENTRY_DOCS:
            text = self.read_text(relative_path)
            for snippet in REASONING_COMPACTION_BOUNDARY_SNIPPETS:
                with self.subTest(path=relative_path, snippet=snippet):
                    self.assertIn(snippet, text)

    def test_readmes_use_same_provider_native_not_same_protocol_native_boundary(self):
        readme = self.read_text("README.md")
        readme_cn = self.read_text("README_CN.md")

        self.assertNotIn("same-protocol paths stay native when possible", readme)
        self.assertNotIn("同协议路径尽量保持 native passthrough", readme_cn)

        english_snippets = (
            "same-provider/native passthrough preserves provider-native fields and lifecycle state",
            "compatible same-protocol lanes promise portable core/portable fields only",
            "not native provider passthrough",
        )
        for snippet in english_snippets:
            with self.subTest(language="README", snippet=snippet):
                self.assertIn(snippet, readme)

        chinese_snippets = (
            "same-provider/native passthrough 才保留 provider-native 字段和 lifecycle state",
            "compatible same-protocol lane 只承诺 portable core/portable fields",
            "不等同于 native provider passthrough",
        )
        for snippet in chinese_snippets:
            with self.subTest(language="README_CN", snippet=snippet):
                self.assertIn(snippet, readme_cn)

    def test_docs_index_and_readmes_link_ga_readiness_review(self):
        for relative_path in ("README.md", "README_CN.md", "docs/README.md"):
            with self.subTest(path=relative_path):
                self.assertIn("docs/ga-readiness-review.md", self.read_text(relative_path))

    def test_readmes_and_docs_index_surface_data_auth_admin_entrypoint(self):
        expectations = {
            "README.md": (
                "`data_auth`",
                "`GET /admin/data-auth`",
                "`PUT /admin/data-auth`",
            ),
            "README_CN.md": (
                "`data_auth`",
                "`GET /admin/data-auth`",
                "`PUT /admin/data-auth`",
            ),
            "docs/README.md": (
                "static `data_auth`",
                "`/admin/data-auth`",
            ),
        }
        for relative_path, snippets in expectations.items():
            text = self.read_text(relative_path)
            for snippet in snippets:
                with self.subTest(path=relative_path, snippet=snippet):
                    self.assertIn(snippet, text)

    def test_clients_guide_matches_wrapper_base_urls_and_proxy_endpoints(self):
        text = self.read_text("docs/clients.md")

        self.assertIn(
            "The wrapper configures the client base URL, and the client appends its own protocol path on top.",
            text,
        )
        self.assertIn("`OPENAI_BASE_URL=<proxy>/openai/v1`", text)
        self.assertIn("`ANTHROPIC_BASE_URL=<proxy>/anthropic`", text)
        self.assertIn("`GOOGLE_GEMINI_BASE_URL=<proxy>/google`", text)
        self.assertIn("`/openai/v1/responses`", text)
        self.assertIn("`/anthropic/v1/messages`", text)
        self.assertIn("`/google/v1beta/models/...`", text)
        self.assertIn("`preset-openai-compatible`", text)
        self.assertIn("`preset-anthropic-compatible`", text)
        self.assertIn("examples/quickstart-provider-neutral.yaml", text)
        self.assertIn('`wire_api="responses"`', text)
        self.assertNotIn("/responses or /chat/completions", text)
        self.assertNotIn("/openai/v1/chat/completions", text)

    def test_configuration_guide_reuses_provider_neutral_presets_and_example(self):
        text = self.read_text("docs/configuration.md")

        self.assertIn("`preset-openai-compatible`", text)
        self.assertIn("`preset-anthropic-compatible`", text)
        for env_key in PRESET_ENV_KEYS:
            with self.subTest(env_key=env_key):
                self.assertIn(env_key, text)
        self.assertIn(
            "[examples/quickstart-provider-neutral.yaml](../examples/quickstart-provider-neutral.yaml)",
            text,
        )
        self.assertIn(
            normalized_whitespace(
                "Reasoning effort such as `xhigh` stays on the client request; "
                "it is not part of the alias or upstream model name."
            ),
            normalized_whitespace(text),
        )

    def test_provider_neutral_quickstart_example_matches_cli_matrix_contract(self):
        text = self.read_text("examples/quickstart-provider-neutral.yaml")

        self.assertIn("PRESET-OPENAI-COMPATIBLE:", text)
        self.assertIn("PRESET-ANTHROPIC-COMPATIBLE:", text)
        for env_key in PRESET_ENV_KEYS:
            with self.subTest(env_key=env_key):
                self.assertIn(env_key, text)
        self.assertIn(
            'preset-openai-compatible: "PRESET-OPENAI-COMPATIBLE:PRESET_ENDPOINT_MODEL"',
            text,
        )
        self.assertIn(
            'preset-anthropic-compatible: "PRESET-ANTHROPIC-COMPATIBLE:PRESET_ENDPOINT_MODEL"',
            text,
        )
        self.assertNotIn("MINIMAX_OPENAI", text)
        self.assertNotIn("MiniMax-M2", text)
        self.assertNotIn("gpt-5-4", text)

    def test_recommended_quickstart_config_has_cli_ready_surface_fields(self):
        for relative_path in QUICKSTART_CONFIG_PATHS:
            with self.subTest(path=relative_path):
                self.assert_cli_ready_surface_contract(
                    self.extract_quickstart_config(relative_path)
                )
                self.assert_provider_neutral_preset_contract(
                    self.extract_quickstart_config(relative_path)
                )

    def test_provider_neutral_quickstart_hydrates_with_preset_env(self):
        module = load_real_cli_matrix()
        parsed = module.parse_proxy_source(
            self.read_text("examples/quickstart-provider-neutral.yaml")
        )

        runtime_config = module.build_runtime_config_text(
            parsed,
            {
                "PRESET_OPENAI_ENDPOINT_BASE_URL": "https://openai-compatible.example/v1",
                "PRESET_ANTHROPIC_ENDPOINT_BASE_URL": "https://anthropic-compatible.example/v1",
                "PRESET_ENDPOINT_MODEL": "provider-configured-model",
                "PRESET_ENDPOINT_API_KEY": "proxy-only-secret",
            },
            listen_host="127.0.0.1",
            listen_port=18080,
            trace_path=pathlib.Path("/tmp/llmup-docs-contract-trace.jsonl"),
        )

        self.assertIn("api_root: https://openai-compatible.example/v1", runtime_config)
        self.assertIn("api_root: https://anthropic-compatible.example/v1", runtime_config)
        self.assertIn(
            'preset-openai-compatible: "PRESET-OPENAI-COMPATIBLE:provider-configured-model"',
            runtime_config,
        )
        self.assertIn(
            'preset-anthropic-compatible: "PRESET-ANTHROPIC-COMPATIBLE:provider-configured-model"',
            runtime_config,
        )
        runtime_body = "\n".join(
            line for line in runtime_config.splitlines() if not line.lstrip().startswith("#")
        )
        self.assertNotIn("api_root: PRESET_", runtime_body)
        self.assertNotIn("PRESET_ENDPOINT_MODEL", runtime_body)
        self.assertNotIn("proxy-only-secret", runtime_config)

    def test_openai_minimax_example_is_not_the_recommended_ga_preset(self):
        text = self.read_text("examples/quickstart-openai-minimax.yaml")

        self.assertIn("MiniMax is a replaceable OpenAI-compatible example", text)
        self.assertNotIn("preset-openai-compatible", text)
        self.assertNotIn("preset-anthropic-compatible", text)
        self.assertNotIn("gpt-5-4", text)


if __name__ == "__main__":
    unittest.main()
