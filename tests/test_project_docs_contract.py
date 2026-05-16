import pathlib
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
PROJECT_DOC = REPO_ROOT / "docs" / "PROJECT.md"


class ProjectDocsContractTests(unittest.TestCase):
    def read_project_doc(self) -> str:
        return PROJECT_DOC.read_text(encoding="utf-8")

    def assert_doc_mentions(self, snippets: tuple[str, ...]) -> None:
        text = self.read_project_doc()
        for snippet in snippets:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

    def test_project_map_covers_ga_runtime_entrypoints(self):
        self.assert_doc_mentions(
            (
                "`src/config/model_surface.rs`",
                "`src/downstream.rs`",
                "`src/server/data_auth.rs`",
                "`src/server/body_limits.rs`",
                "`src/server/web_dashboard.rs`",
                "`src/server/web_dashboard/index.html`",
                "`LLM_UNIVERSAL_PROXY_AUTH_MODE`",
                "`LLM_UNIVERSAL_PROXY_KEY`",
                "`provider_key_env`",
                "`provider_key.inline`",
                "`provider_key.env`",
                "`data_auth`",
                "`/admin/data-auth`",
                "`max_request_body_bytes`",
            )
        )

    def test_project_map_covers_ga_cli_and_release_gates(self):
        self.assert_doc_mentions(
            (
                "`scripts/interactive_cli.py`",
                "`scripts/run_codex_proxy.sh`",
                "`scripts/run_claude_proxy.sh`",
                "`scripts/fixtures/cli_matrix/default_proxy_test_matrix.yaml`",
                "`preset-openai-compatible`",
                "`preset-anthropic-compatible`",
                "`scripts/real_endpoint_matrix.py`",
                "`compatible-provider-smoke`",
                "`release-compatible-provider`",
                "`artifacts/compatible-provider-smoke.json`",
            )
        )

    def test_project_map_covers_ga_test_contracts(self):
        self.assert_doc_mentions(
            (
                "`tests/test_interactive_cli.py`",
                "`tests/test_cli_matrix_contracts.py`",
                "`tests/test_real_endpoint_matrix.py`",
                "`tests/test_release_gates.py`",
                "`tests/test_default_matrix_surface_contract.py`",
                "`tests/test_project_docs_contract.py`",
                "hermetic scripted interactive Codex wrapper gate",
                "endpoint matrix",
                "CLI contract",
                "release gate",
            )
        )


if __name__ == "__main__":
    unittest.main()
