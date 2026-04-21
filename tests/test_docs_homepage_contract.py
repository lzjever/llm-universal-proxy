import pathlib
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]


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


if __name__ == "__main__":
    unittest.main()
