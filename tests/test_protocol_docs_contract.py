import pathlib
import re
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]


def read_doc(relative_path: str) -> str:
    return (REPO_ROOT / relative_path).read_text(encoding="utf-8")


def table_row(text: str, first_cell: str) -> str:
    pattern = re.compile(rf"^\|\s*{re.escape(first_cell)}\s*\|.*$", re.MULTILINE)
    match = pattern.search(text)
    if match is None:
        raise AssertionError(f"missing table row for {first_cell!r}")
    return match.group(0)


class ProtocolDocsContractTests(unittest.TestCase):
    def test_protocol_matrix_names_compatible_provider_ga_routes_precisely(self):
        text = read_doc("docs/protocol-compatibility-matrix.md")

        for snippet in (
            "OpenAI-compatible chat-completions route `/openai/v1/chat/completions`",
            "Anthropic-compatible messages route `/anthropic/v1/messages`",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)
        for forbidden in (
            "OpenAI-compatible completions/chat-completions",
            "OpenAI-compatible completions surface",
            "OpenAI-compatible completions and",
            "Gemini `generateContent` | Where to read more",
            "OpenAI-to-Gemini tool-call translation",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, text)
        self.assertIn("Google OpenAI-compatible upstream", text)
        self.assertIn("`format: openai-completion`", text)

    def test_prd_uses_same_provider_native_boundary(self):
        text = read_doc("docs/PRD.md")

        self.assertNotIn("same-protocol paths stay native", text)
        for snippet in (
            "same-provider/native passthrough",
            "compatible same-protocol lanes preserve only portable core/portable fields",
            "not native provider passthrough",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

    def test_active_docs_do_not_promise_native_gemini_wire_format(self):
        active_docs = (
            "docs/README.md",
            "docs/PRD.md",
            "docs/CONSTITUTION.md",
            "docs/DESIGN.md",
            "docs/PROJECT.md",
            "docs/ga-readiness-review.md",
            "docs/max-compat-design.md",
            "docs/container.md",
        )
        forbidden = (
            "Gemini wrapper setup plus common client notes",
            "Google Gemini should be able to route",
            "all four protocols",
            "| 4 | OpenAI Chat Completions | Google Gemini |",
            "| 13 | Google Gemini | OpenAI Chat Completions |",
            "All 16 combinations",
            "**Google Gemini**: `candidates` chunks",
            "`systemInstruction` (Gemini)",
            "`functionCall` parts (Gemini)",
            "`functionResponse` parts (Gemini)",
            "`promptTokenCount/candidatesTokenCount`",
            "`thought` parts (Gemini)",
            "Data plane HTTP API for OpenAI, Anthropic, and Google/Gemini-compatible clients",
            "Main request execution path for OpenAI, Anthropic, and Gemini surfaces",
            "Gemini GenerateContent unary",
            "official Gemini live smoke",
            "Gemini `replace`",
            "Gemini request shapes",
            "Gemini to OpenAI Chat/Responses",
            "OpenAI Chat/Responses to Gemini",
        )

        combined = "\n".join(read_doc(path) for path in active_docs)
        for snippet in forbidden:
            with self.subTest(snippet=snippet):
                self.assertNotIn(snippet, combined)

        self.assertIn("Google OpenAI-compatible Gemini", combined)
        self.assertIn("`format: openai-completion`", combined)

    def test_tools_baseline_preserves_hosted_tools_only_on_native_or_shim_lanes(self):
        text = read_doc("docs/protocol-baselines/capabilities/tools.md")
        row = table_row(text, "Hosted / server tools")

        self.assertNotIn("Preserve same-protocol hosted tools only", row)
        self.assertIn(
            "Preserve hosted/server tools only on same-provider/native passthrough lanes or through explicit compatibility shims. Cross-provider translation should default to drop-or-warn.",
            row,
        )
        self.assertIn(
            "Gate hosted/server tools behind same-provider/native passthrough or explicit compatibility shims.",
            text,
        )

    def test_max_compat_uses_same_provider_native_boundary(self):
        text = read_doc("docs/max-compat-design.md")

        self.assertNotIn("same-protocol paths: native passthrough", text)
        for snippet in (
            "same-provider/native passthrough",
            "compatible same-protocol lane",
            "portable fields",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

    def test_max_compat_records_request_translation_and_native_passthrough_facts(self):
        text = read_doc("docs/max-compat-design.md")

        for snippet in (
            "RequestTranslationPolicy::default() is `max_compat`",
            "`translate_request()` defaults to `max_compat`",
            "same-format request translation passthrough",
            "Native Responses passthrough preserves `context_management`",
            "`include` values such as `reasoning.encrypted_content`",
            "input reasoning and compaction items with `encrypted_content`",
            "exactly one native OpenAI Responses upstream",
            "does not reconstruct provider state",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

    def test_max_compat_records_reasoning_and_compaction_degrade_rules(self):
        text = read_doc("docs/max-compat-design.md")

        self.assertNotIn("safe reasoning carriers", text)
        for snippet in (
            "request-side reasoning encrypted_content",
            "include `reasoning.encrypted_content`",
            "request-side compaction input",
            "default/max_compat",
            "strict/balanced",
            "visible summary",
            "visible transcript/history",
            "opaque-only reasoning",
            "opaque-only compaction",
            "one summarized compaction item does not permit another opaque-only compaction item",
            "native Responses passthrough",
            "response-side reasoning encrypted_content",
            "Anthropic carrier recovery path",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

    def test_field_mapping_drop_statuses_are_not_ambiguous(self):
        text = read_doc("docs/protocol-baselines/matrices/field-mapping-matrix.md")

        for snippet in (
            "Fail-closed",
            "Warn/drop opaque carrier",
            "Native-only",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

        for first_cell in ("Reasoning opaque state", "Compaction"):
            row = table_row(text, first_cell)
            with self.subTest(row=first_cell):
                self.assertIn("Warn/drop opaque carrier", row)
                self.assertNotRegex(row, r"\|\s*Drop\s*\|")

    def test_baseline_readme_separates_vendor_contract_from_proxy_policy(self):
        readme = read_doc("docs/protocol-baselines/README.md")

        for snippet in (
            "vendor contract",
            "proxy posture",
            "snapshot/source facts",
            "proxy policy",
            "vendor snapshot/captured date",
            "proxy posture updated date",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet.casefold(), readme.casefold())

        responses = read_doc("docs/protocol-baselines/openai-responses.md")
        self.assertIn("`captured_at_utc`: `2026-04-17T06:59:44Z`", responses)
        self.assertIn("`snapshot_bucket`: `2026-04-16`", responses)
        self.assertIn("`proxy_posture_updated`: `2026-04-26`", responses)

    def test_ga_source_spot_check_is_recorded_without_full_snapshot_refresh(self):
        audit_path = (
            REPO_ROOT
            / "docs"
            / "protocol-baselines"
            / "audits"
            / "2026-04-27-ga-source-spot-check.md"
        )
        self.assertTrue(audit_path.exists(), "GA source spot-check audit is missing")
        audit = audit_path.read_text(encoding="utf-8")
        readme = read_doc("docs/protocol-baselines/README.md")
        overview = read_doc("docs/protocol-baselines/overview.md")

        self.assertIn(
            "audits/2026-04-27-ga-source-spot-check.md",
            readme + overview,
        )
        for snippet in (
            "not a full recertification",
            "not a full snapshot refresh",
            "2026-04-16 snapshot bucket",
            "captured baseline",
            "2026-04-27 spot-check",
            "GA portability contract",
            "OpenAI Responses",
            "Conversations",
            "compact",
            "Gemini generateContent",
            "Anthropic 2026-04-23/24 release notes",
            "Rate Limits API",
            "Managed Agents memory",
            "portable-core contract",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, audit)

    def test_development_plan_records_interactive_codex_wrapper_gate(self):
        text = read_doc("docs/engineering/max-compat-development-plan.md")

        self.assertIn("hermetic scripted interactive Codex wrapper gate", text)

    def test_reasoning_docs_do_not_overstate_opaque_continuity_rules(self):
        for relative_path in (
            "docs/max-compat-design.md",
            "docs/protocol-baselines/capabilities/reasoning.md",
        ):
            text = read_doc(relative_path)
            with self.subTest(relative_path=relative_path):
                self.assertNotIn("safe reasoning carriers", text)
                self.assertNotIn(
                    "provider-specific effort controls across protocol boundaries",
                    text,
                )


if __name__ == "__main__":
    unittest.main()
