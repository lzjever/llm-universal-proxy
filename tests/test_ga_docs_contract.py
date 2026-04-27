import pathlib
import re
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]


def read_doc(relative_path: str) -> str:
    return (REPO_ROOT / relative_path).read_text(encoding="utf-8")


def markdown_section(text: str, heading: str) -> str:
    marker = f"\n## {heading}\n"
    start = text.find(marker)
    if start == -1:
        if text.startswith(f"## {heading}\n"):
            start = 0
        else:
            raise AssertionError(f"missing section heading: {heading}")
    else:
        start += 1

    next_heading = text.find("\n## ", start + len(f"## {heading}\n"))
    return text[start:] if next_heading == -1 else text[start:next_heading]


def markdown_subsection(text: str, heading: str) -> str:
    marker = f"\n### {heading}\n"
    start = text.find(marker)
    if start == -1:
        if text.startswith(f"### {heading}\n"):
            start = 0
        else:
            raise AssertionError(f"missing subsection heading: {heading}")
    else:
        start += 1

    next_heading = text.find("\n### ", start + len(f"### {heading}\n"))
    return text[start:] if next_heading == -1 else text[start:next_heading]


class GaDocsContractTests(unittest.TestCase):
    def test_prd_metadata_and_section_numbers_are_current_for_ga(self):
        prd = read_doc("docs/PRD.md")

        self.assertIn("**Last Updated**: 2026-04-27", prd)

        headings = re.findall(r"^### 2\.(\d+) ", prd, flags=re.MULTILINE)
        numbers = [int(value) for value in headings]
        self.assertEqual(
            list(range(1, len(numbers) + 1)),
            numbers,
            "PRD functional requirement headings must be unique and sequential",
        )

        for heading in (
            "### 2.7 Upstream Configuration",
            "### 2.8 Observability",
            "### 2.9 Namespace Support",
            "### 2.10 Admin Control Plane",
            "### 2.11 Admin Config CAS Semantics",
        ):
            with self.subTest(heading=heading):
                self.assertIn(heading, prd)

    def test_prd_dashboard_scope_is_web_static_ui_with_admin_api_boundary(self):
        prd = read_doc("docs/PRD.md")
        observability = markdown_subsection(prd, "2.8 Observability")

        self.assertNotIn("Terminal UI", observability)
        for snippet in (
            "Web Admin Dashboard",
            "`/dashboard` shell and static assets are public UI resources",
            "Dashboard JavaScript sends `Authorization: Bearer <admin-token>` only when it calls existing `/admin/*` APIs",
            "live runtime state",
            "current runtime config",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, observability)

    def test_prd_success_metrics_keep_streaming_within_portability_boundaries(self):
        prd = read_doc("docs/PRD.md")
        success_metrics = markdown_section(prd, "9. Success Metrics")

        self.assertNotIn(
            "| Streaming works for all combinations | 16/16 pass |",
            success_metrics,
        )
        self.assertIn("documented portability boundaries", success_metrics)
        self.assertIn("pass, warn, or reject as specified", success_metrics)

    def test_max_compat_phase_5_is_delivered_for_ga_docs_with_maintenance_open(self):
        text = read_doc("docs/max-compat-development-plan.md")
        phase_5 = text.split("### Phase 5: Documentation Rollout", 1)[1].split(
            "\n### Phase 6:", 1
        )[0]

        self.assertNotIn("Status: in progress.", phase_5)
        self.assertIn("delivered", phase_5.casefold())
        self.assertIn("current GA docs", phase_5)
        self.assertIn("ongoing maintenance", phase_5)

    def test_container_main_path_is_provider_neutral(self):
        container_config = read_doc("examples/container-config.yaml")
        compose = read_doc("examples/docker-compose.yaml")
        container_doc = read_doc("docs/container.md")
        run_section = container_doc.split("## Run the Release Image", 1)[1].split(
            "\n## Local Build and Smoke", 1
        )[0]

        for snippet in (
            "OPENAI_COMPATIBLE",
            "ANTHROPIC_COMPATIBLE",
            "credential_env: OPENAI_COMPATIBLE_API_KEY",
            "credential_env: ANTHROPIC_COMPATIBLE_API_KEY",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, container_config)

        self.assertIn(
            "OPENAI_COMPATIBLE_API_KEY: ${OPENAI_COMPATIBLE_API_KEY:?set OPENAI_COMPATIBLE_API_KEY}",
            compose,
        )
        self.assertIn(
            "ANTHROPIC_COMPATIBLE_API_KEY: ${ANTHROPIC_COMPATIBLE_API_KEY:?set ANTHROPIC_COMPATIBLE_API_KEY}",
            compose,
        )
        for forbidden in (
            "MINIMAX",
            "PRESET_",
            "api.openai.com",
            "api.minimaxi.com",
            "OPENAI_API_KEY",
            "MINIMAX_API_KEY",
            "MiniMax-M2.7",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, container_config)

        for forbidden in ("MINIMAX", "PRESET_", "OPENAI_API_KEY", "MINIMAX_API_KEY"):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, compose)

        for snippet in (
            "OPENAI_COMPATIBLE_API_KEY",
            "ANTHROPIC_COMPATIBLE_API_KEY",
            "provider-neutral compatible upstreams",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, run_section)
        for forbidden in (
            "MINIMAX_API_KEY",
            "OPENAI_API_KEY",
            "OpenAI/MiniMax",
            "MiniMax is only an example provider choice here",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, run_section)

    def test_prd_ga_evidence_is_provider_neutral_not_fixed_live_matrix(self):
        prd = read_doc("docs/PRD.md")
        test_config = markdown_section(prd, "6. Test Configuration")

        self.assertIn("provider-neutral protected `COMPAT_*` evidence", test_config)
        self.assertIn("operator validation/example", test_config)
        self.assertIn("not portable-core GA hard dependencies", test_config)
        for snippet in (
            "`COMPAT_PROVIDER_API_KEY`",
            "`COMPAT_OPENAI_API_KEY`",
            "`COMPAT_ANTHROPIC_API_KEY`",
            "`COMPAT_OPENAI_BASE_URL`",
            "`COMPAT_OPENAI_MODEL`",
            "`COMPAT_ANTHROPIC_BASE_URL`",
            "`COMPAT_ANTHROPIC_MODEL`",
            "`COMPAT_PROVIDER_LABEL`",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, test_config)

        forbidden_fixed_matrix = (
            "The proxy MUST be tested against the following real upstream endpoints",
            "MiniMax (Anthropic)",
            "MiniMax (OpenAI)",
            "Local qwen3.5-9b-awq",
            "| Client Entry | MiniMax Anthropic | MiniMax OpenAI | Local qwen3.5 |",
        )
        for snippet in forbidden_fixed_matrix:
            with self.subTest(forbidden=snippet):
                self.assertNotIn(snippet, test_config)

    def test_constitution_records_proxy_auth_as_in_scope(self):
        constitution = read_doc("docs/CONSTITUTION.md")
        out_of_scope = markdown_section(constitution, "Scope Boundaries").split(
            "### Out of Scope", 1
        )[-1]

        self.assertNotIn("Authentication to the proxy itself", out_of_scope)
        for snippet in (
            "Proxy authentication is in scope",
            "`/health` remains unauthenticated",
            "`LLM_UNIVERSAL_PROXY_DATA_TOKEN`",
            "`X-LLMUP-Data-Token`",
            "`Authorization: Bearer <data-token>`",
            "`LLM_UNIVERSAL_PROXY_ADMIN_TOKEN`",
            "`Authorization: Bearer <admin-token>`",
            "loopback-only",
            "non-loopback",
            "fail closed",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, constitution)

    def test_constitution_separates_public_dashboard_from_admin_api_auth(self):
        constitution = read_doc("docs/CONSTITUTION.md")
        auth_scope = markdown_section(constitution, "Scope Boundaries")

        self.assertNotIn(
            "Admin-plane routes and the dashboard use `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN`",
            auth_scope,
        )
        for snippet in (
            "`/dashboard` shell and static assets are public UI resources",
            "Dashboard JavaScript sends `Authorization: Bearer <admin-token>` only when it calls existing `/admin/*` APIs",
            "Admin-plane routes use `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN`",
            "Empty or whitespace-only admin tokens are misconfiguration and fail closed",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, auth_scope)

    def test_admin_docs_separate_public_dashboard_from_admin_api_auth(self):
        docs = {
            "admin-dynamic-config": (
                read_doc("docs/admin-dynamic-config.md"),
                "Admin Dashboard Boundary",
            ),
            "container": (
                read_doc("docs/container.md"),
                "Admin Plane and Dashboard Boundary",
            ),
        }

        required_snippets = (
            "`/dashboard` shell and static assets are public UI resources",
            "Dashboard JavaScript sends `Authorization: Bearer <admin-token>` only when it calls existing `/admin/*` APIs",
            "Admin-plane routes use `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN`",
        )
        forbidden_snippets = (
            "The Web Admin Dashboard uses the same admin plane and the same `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` boundary as the endpoints below.",
            "The admin API and Web Admin Dashboard share one admin boundary:",
            "dashboard shell/admin actions are admin-plane operations",
            "dashboard shell/admin actions use the same admin-plane boundary",
            "dashboard login is admin-token based",
        )

        for name, (text, heading) in docs.items():
            with self.subTest(doc=name):
                boundary = markdown_section(text, heading)
                for snippet in required_snippets:
                    with self.subTest(doc=name, snippet=snippet):
                        self.assertIn(snippet, boundary)
                for snippet in forbidden_snippets:
                    with self.subTest(doc=name, forbidden=snippet):
                        self.assertNotIn(snippet, boundary)

    def test_changelog_separates_public_dashboard_from_admin_api_auth(self):
        changelog = read_doc("CHANGELOG.md")

        required_snippets = (
            "`/dashboard` shell and static assets are public UI resources",
            "`/admin/*` API calls require `Authorization: Bearer <admin-token>`",
            "data-plane token is separate",
        )
        forbidden_snippets = (
            "uses the existing admin API and `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` boundary",
            "The Web Admin Dashboard uses the same admin plane and the same `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` boundary as the endpoints below.",
            "The admin API and Web Admin Dashboard share one admin boundary:",
            "dashboard shell/admin actions are admin-plane operations",
            "dashboard shell/admin actions use the same admin-plane boundary",
            "dashboard login is admin-token based",
        )

        for snippet in required_snippets:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, changelog)
        for snippet in forbidden_snippets:
            with self.subTest(forbidden=snippet):
                self.assertNotIn(snippet, changelog)

    def test_ga_and_container_docs_name_protected_routes_and_artifact_boundary(self):
        docs = {
            "ga": read_doc("docs/ga-readiness-review.md"),
            "container": read_doc("docs/container.md"),
        }

        for name, text in docs.items():
            with self.subTest(doc=name):
                self.assertIn("protected `release-compatible-provider`", text)
                self.assertIn("`/openai/v1/chat/completions`", text)
                self.assertIn("`/anthropic/v1/messages`", text)
                self.assertIn("OpenAI-compatible chat-completions", text)
                self.assertIn("Anthropic-compatible messages", text)
                self.assertIn("GitHub Actions artifact", text)
                self.assertIn("not a GitHub Release asset", text)
                self.assertIn("external release evidence", text)
                self.assertNotIn("completions/chat-completions", text)
                self.assertNotIn("OpenAI-compatible completions surface", text)
                self.assertNotIn("OpenAI-compatible completions and", text)

    def test_changelog_latest_release_records_recent_user_visible_changes(self):
        changelog = read_doc("CHANGELOG.md")
        latest_release = markdown_section(changelog, "v0.2.22 - 2026-04-27")

        for snippet in (
            "provider-neutral preset naming",
            "Responses reasoning/compaction continuity degradation",
            "hermetic Codex wrapper interaction gate",
            "GA docs alignment",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, latest_release)


if __name__ == "__main__":
    unittest.main()
