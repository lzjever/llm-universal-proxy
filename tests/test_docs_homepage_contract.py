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
    "examples/quickstart-openai-minimax.yaml",
)
QUICKSTART_ALIASES = ("gpt-5-4", "gpt-5-4-mini")


def load_real_cli_matrix():
    spec = importlib.util.spec_from_file_location(
        "real_cli_matrix_docs_homepage_contract",
        REAL_CLI_MATRIX_PATH,
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


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

    def test_readmes_lock_two_upstream_alias_story_and_reasoning_note(self):
        readme = self.read_text("README.md")
        readme_cn = self.read_text("README_CN.md")

        for text in (readme, readme_cn):
            with self.subTest(language="README" if text is readme else "README_CN"):
                self.assertIn("https://api.openai.com/v1", text)
                self.assertIn("https://api.minimaxi.com/v1", text)
                self.assertIn("gpt-5-4: OPENAI:gpt-5.4", text)
                self.assertIn(
                    "gpt-5-4-mini: MINIMAX_OPENAI:MiniMax-M2.7-highspeed", text
                )
                self.assertIn(
                    "examples/quickstart-openai-minimax.yaml",
                    text,
                )
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
        self.assertIn("`gpt-5-4`", text)
        self.assertIn("`gpt-5-4-mini`", text)
        self.assertIn('`wire_api="responses"`', text)
        self.assertNotIn("/responses or /chat/completions", text)
        self.assertNotIn("/openai/v1/chat/completions", text)

    def test_configuration_guide_reuses_quickstart_aliases_and_example(self):
        text = self.read_text("docs/configuration.md")

        self.assertIn("`gpt-5-4`", text)
        self.assertIn("`gpt-5-4-mini`", text)
        self.assertIn("MiniMax-M2.7-highspeed", text)
        self.assertIn(
            "[examples/quickstart-openai-minimax.yaml](../examples/quickstart-openai-minimax.yaml)",
            text,
        )
        self.assertIn(
            "Reasoning effort such as `xhigh` stays on the client request; it is not part of the alias or upstream model name.",
            text,
        )

    def test_quickstart_example_is_user_facing_two_upstream_config(self):
        text = self.read_text("examples/quickstart-openai-minimax.yaml")

        self.assertIn("OPENAI:", text)
        self.assertIn("MINIMAX_OPENAI:", text)
        self.assertIn("https://api.openai.com/v1", text)
        self.assertIn("https://api.minimaxi.com/v1", text)
        self.assertIn("gpt-5-4: OPENAI:gpt-5.4", text)
        self.assertIn("gpt-5-4-mini: MINIMAX_OPENAI:MiniMax-M2.7-highspeed", text)
        self.assertNotIn(".env.test", text)
        self.assertNotIn("proxy-test-minimax-and-local.yaml", text)
        self.assertNotIn("MINIMAX-ANTHROPIC", text)

    def test_recommended_quickstart_config_has_cli_ready_surface_fields(self):
        for relative_path in QUICKSTART_CONFIG_PATHS:
            with self.subTest(path=relative_path):
                self.assert_cli_ready_surface_contract(
                    self.extract_quickstart_config(relative_path)
                )


if __name__ == "__main__":
    unittest.main()
