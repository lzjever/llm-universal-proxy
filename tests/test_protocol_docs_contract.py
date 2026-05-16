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

    def test_prd_uses_single_maximum_safe_compatibility_boundary(self):
        text = read_doc("docs/PRD.md")

        self.assertNotIn("same-protocol paths stay native", text)
        for snippet in (
            "raw same-protocol passthrough",
            "maximum safe compatibility",
            "not a user-selectable compatibility level",
            "MUST NOT introduce a `llmup` cache store",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

    def test_prd_translation_pipeline_records_raw_lane_as_pre_ga_target(self):
        text = read_doc("docs/PRD.md")
        pipeline = text.split("### 4.2 Translation Pipeline", 1)[1].split(
            "\n### 4.3", 1
        )[0]

        for snippet in (
            "raw same-protocol passthrough execution lane",
            "avoid body mutation and response normalization",
            "single maximum-compatible translation lane",
            "Apply hard portability boundaries before upstream",
            "pre-GA implementation target and engineering lane",
            "may still pass through compatibility machinery",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, pipeline)

    def test_constitution_records_single_strategy_and_narrow_state_exception(self):
        text = read_doc("docs/CONSTITUTION.md")

        for snippet in (
            "single maximum safe compatibility",
            "Raw Passthrough Is An Execution Lane",
            "not a product tier",
            "conversation_state_bridge.mode=memory",
            "memory-only",
            "resp_llmup_*",
            "process restart",
            "external provider IDs fail closed",
            "not persistent conversation state",
            "not a response cache",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

    def test_active_docs_and_python_contracts_reject_old_tiered_language(self):
        paths = [
            REPO_ROOT / "README.md",
            REPO_ROOT / "README_CN.md",
            *sorted((REPO_ROOT / "docs").rglob("*.md")),
            REPO_ROOT / "tests" / "test_protocol_docs_contract.py",
            REPO_ROOT / "tests" / "test_docs_homepage_contract.py",
            REPO_ROOT / "tests" / "test_ga_docs_contract.py",
            REPO_ROOT / "tests" / "test_release_gates.py",
            REPO_ROOT / "tests" / "test_real_cli_matrix.py",
        ]
        forbidden = (
            "strict" + "/balanced",
            "default" + "/" + "max" + "_compat",
            "compatible " + "same-protocol lane",
            "same" + "-provider/native passthrough",
            "same" + "-provider native passthrough",
            "compatibility" + "_mode",
            "Provider" + "PromptCacheOptimized",
            "provider prompt-cache optimized " + "lane",
            "raw passthrough, provider prompt-cache optimization, "
            + "or maximum-compatible translation",
        )

        for path in paths:
            relative_path = path.relative_to(REPO_ROOT)
            text = path.read_text(encoding="utf-8")
            for snippet in forbidden:
                if (
                    snippet == "compatibility" + "_mode"
                    and str(relative_path)
                    == "docs/engineering/max-compat-development-plan.md"
                ):
                    continue
                with self.subTest(path=str(relative_path), snippet=snippet):
                    self.assertNotIn(snippet, text)

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

    def test_active_protocol_baselines_do_not_model_native_gemini_as_active_surface(self):
        active_baselines = (
            "docs/protocol-baselines/capabilities/cache.md",
            "docs/protocol-baselines/capabilities/reasoning.md",
            "docs/protocol-baselines/capabilities/state-continuity.md",
            "docs/protocol-baselines/capabilities/streaming.md",
            "docs/protocol-baselines/capabilities/tools.md",
            "docs/protocol-baselines/matrices/field-mapping-matrix.md",
            "docs/protocol-baselines/matrices/provider-capability-matrix.md",
        )
        forbidden = (
            "Gemini `generateContent`",
            "Gemini `streamGenerateContent`",
            "`functionDeclarations`",
            "`functionCall`",
            "`functionResponse`",
            "`thinkingConfig`",
            "`cachedContent`",
            "`inlineData`",
            "`fileData`",
            "`mcpServers`",
            "`toolConfig`",
            "`functionCallingConfig`",
            "Gemini video",
            "Gemini-routed",
            "Gemini-native",
            "Gemini built-in",
        )

        combined = "\n".join(read_doc(path) for path in active_baselines)
        for snippet in forbidden:
            with self.subTest(snippet=snippet):
                self.assertNotIn(snippet, combined)

        self.assertIn("Google OpenAI-compatible", combined)
        self.assertIn("retired historical", combined)

    def test_root_protocol_baselines_mark_native_gemini_support_or_routes_retired(self):
        native_gemini_signals = (
            "Gemini `generateContent`",
            "`streamGenerateContent`",
            "/google/v1beta",
            "Gemini request parsing",
            "OpenAI/Anthropic/Gemini request and response translation",
        )
        retired_or_non_active_markers = (
            "retired historical",
            "not active",
            "no longer an active",
            "removed",
            "non-active",
        )

        baseline_docs = sorted(
            (REPO_ROOT / "docs" / "protocol-baselines").glob("*.md")
        )
        for path in baseline_docs:
            text = path.read_text(encoding="utf-8")
            if not any(signal in text for signal in native_gemini_signals):
                continue

            relative_path = path.relative_to(REPO_ROOT)
            with self.subTest(path=str(relative_path)):
                lower_text = text.casefold()
                self.assertTrue(
                    any(
                        marker in lower_text
                        for marker in retired_or_non_active_markers
                    ),
                    (
                        f"{relative_path} contains native Gemini support/route "
                        "language without an explicit retired/non-active marker"
                    ),
                )

    def test_tools_baseline_preserves_hosted_tools_only_on_native_or_shim_lanes(self):
        text = read_doc("docs/protocol-baselines/capabilities/tools.md")
        row = table_row(text, "Hosted / server tools")

        self.assertNotIn("Preserve same-protocol hosted tools only", row)
        self.assertIn(
            "Preserve hosted/server tools only on raw/native passthrough lanes or through explicit compatibility shims. Cross-provider translation should warn/drop or fail closed when the target cannot represent the tool safely.",
            row,
        )
        self.assertIn(
            "Gate hosted/server tools behind raw/native passthrough or explicit compatibility shims.",
            text,
        )

    def test_maximum_compatibility_design_uses_single_strategy_and_raw_passthrough_boundary(self):
        text = read_doc("docs/max-compat-design.md")

        self.assertNotIn("same-protocol paths: native passthrough", text)
        for snippet in (
            "one client-first translation strategy: maximum safe compatibility",
            "Not Tiered",
            "raw same-protocol passthrough",
            "hard portability boundary",
            "provider prompt-cache optimization is a provider-native request-control step",
            "not a third primary lane",
            "Legacy compatibility config input is accepted only as no-op parsing compatibility",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

    def test_maximum_compatibility_records_request_translation_and_raw_passthrough_facts(self):
        text = read_doc("docs/max-compat-design.md")

        for snippet in (
            "Translated paths should use the maximum safe representation",
            "Same-format raw passthrough",
            "Native Responses passthrough preserves `context_management`",
            "`include` values such as `reasoning.encrypted_content`",
            "input reasoning and compaction items with `encrypted_content`",
            "exactly one native OpenAI Responses upstream",
            "does not reconstruct provider state",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)

    def test_maximum_compatibility_records_reasoning_and_compaction_degrade_rules(self):
        text = read_doc("docs/max-compat-design.md")

        self.assertNotIn("safe reasoning carriers", text)
        for snippet in (
            "request-side reasoning encrypted_content",
            "include `reasoning.encrypted_content`",
            "request-side compaction input",
            "maximum safe compatibility",
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

    def test_development_plan_records_removed_compatibility_policy_plumbing(self):
        text = read_doc("docs/engineering/max-compat-development-plan.md")

        for snippet in (
            "User-selectable compatibility-policy plumbing has been removed",
            "single maximum safe compatibility strategy",
            "legacy `"
            + "compatibility"
            + "_mode"
            + "` is accepted only as no-op input parsing",
            "not stored, serialized, or exposed",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, text)
        self.assertNotIn("Legacy compatibility-policy plumbing exists in the runtime", text)
        self.assertNotIn(
            "Status: delivered as internal plumbing; no longer a product-facing tier model.",
            text,
        )

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
