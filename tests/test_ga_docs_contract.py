import json
import pathlib
import re
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
CONTAINER_IMAGE_MANIFEST = REPO_ROOT / "docs" / "release-artifacts" / "container-image.json"
IMAGE_REPO = "ghcr.io/agentsmith-project/llm-universal-proxy"


def read_doc(relative_path: str) -> str:
    return (REPO_ROOT / relative_path).read_text(encoding="utf-8")


def load_container_manifest() -> dict:
    return json.loads(CONTAINER_IMAGE_MANIFEST.read_text(encoding="utf-8"))


def cargo_package_version() -> str:
    cargo = read_doc("Cargo.toml")
    match = re.search(r'^version = "([^"]+)"', cargo, flags=re.MULTILINE)
    if match is None:
        raise AssertionError("Cargo.toml must declare package version")
    return match.group(1)


def container_refs() -> dict:
    manifest = load_container_manifest()
    published = manifest["published"]
    next_release = manifest["next_release"]
    image = manifest["image"]
    return {
        "image": image,
        "published_release_tag": published["release_tag"],
        "published_version_tag": published["version_tag"],
        "published_digest": published["digest"],
        "published_digest_ref": f'{image}@{published["digest"]}',
        "next_release_tag": next_release["release_tag"],
        "next_package_version": next_release["cargo_package_version"],
    }


def normalized_whitespace(text: str) -> str:
    return " ".join(text.split())


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


def markdown_code_blocks(text: str, language: str) -> list[str]:
    return re.findall(rf"```{re.escape(language)}\n(.*?)\n```", text, re.DOTALL)


class GaDocsContractTests(unittest.TestCase):
    def test_container_image_manifest_separates_published_image_from_next_release_identity(self):
        manifest = load_container_manifest()
        published = manifest["published"]
        next_release = manifest["next_release"]

        self.assertEqual(manifest["schema"], 1)
        self.assertEqual(manifest["image"], IMAGE_REPO)
        self.assertRegex(published["release_tag"], r"^v[0-9]+\.[0-9]+\.[0-9]+$")
        self.assertEqual(
            published["version_tag"], published["release_tag"].removeprefix("v")
        )
        self.assertRegex(published["digest"], r"^sha256:[0-9a-f]{64}$")
        self.assertIn("published_at", published)
        self.assertNotIn("released_at", published)
        self.assertEqual(next_release["cargo_package_version"], cargo_package_version())
        self.assertEqual(next_release["release_tag"], f"v{cargo_package_version()}")
        self.assertEqual(next_release["status"], "not_published")
        self.assertNotEqual(published["release_tag"], next_release["release_tag"])

    def test_published_docker_image_usage_docs_are_ga_ready(self):
        refs = container_refs()
        container_doc = read_doc("docs/container.md")

        current_release = markdown_section(container_doc, "Current Release")
        for snippet in (
            f'{refs["image"]}:{refs["published_release_tag"]}',
            f'{refs["image"]}:{refs["published_version_tag"]}',
            f'{refs["image"]}:latest',
            refs["published_digest_ref"],
            f'Cargo package version `{refs["next_package_version"]}`',
            "next release identity",
            "not a published container tag yet",
        ):
            with self.subTest(section="current_release", snippet=snippet):
                self.assertIn(snippet, current_release)

        pull = markdown_section(container_doc, "Pull")
        for snippet in (
            f'docker pull {refs["image"]}:{refs["published_release_tag"]}',
            f'docker pull {refs["published_digest_ref"]}',
        ):
            with self.subTest(section="pull", snippet=snippet):
                self.assertIn(snippet, pull)

        smoke = markdown_section(container_doc, "Verify in One Minute")
        for snippet in (
            f'docker pull {refs["image"]}:{refs["published_release_tag"]}',
            "docker run --rm",
            "127.0.0.1:8080:8080",
            "/etc/llmup/config.yaml",
            "LLM_UNIVERSAL_PROXY_ADMIN_TOKEN",
            "LLM_UNIVERSAL_PROXY_AUTH_MODE",
            "LLM_UNIVERSAL_PROXY_KEY",
            "curl -fsS http://127.0.0.1:8080/health",
            "curl -fsS http://127.0.0.1:8080/ready",
            f"The current published `{refs['published_release_tag']}` image",
            "supports no-mount API bootstrap",
        ):
            with self.subTest(section="smoke", snippet=snippet):
                self.assertIn(snippet, smoke)

        production_pinning = markdown_section(container_doc, "Production Pinning")
        for snippet in (
            f'{refs["image"]}:{refs["published_release_tag"]}',
            refs["published_digest_ref"],
            "Pin a release tag or digest for production",
            "Do not use `latest` for production pinning",
        ):
            with self.subTest(section="production_pinning", snippet=snippet):
                self.assertIn(snippet, production_pinning)

        ghcr_access = markdown_section(container_doc, "GHCR Access")
        for snippet in (
            "personal access token (classic)",
            "docker login ghcr.io",
            "read:packages",
            "GITHUB_USERNAME",
            "If the package is public",
            "unauthorized, 403, or package page appears 404",
        ):
            with self.subTest(section="ghcr_access", snippet=snippet):
                self.assertIn(snippet, ghcr_access)

        for snippet in ("fine-grained personal access token", "$GITHUB_ACTOR"):
            with self.subTest(section="container_doc_forbidden", snippet=snippet):
                self.assertNotIn(snippet, container_doc)

        smoke = markdown_section(container_doc, "Verify in One Minute")
        self.assertIn("GHCR Access", smoke)
        self.assertIn("unauthorized", smoke)

        lower_doc = container_doc.casefold()
        self.assertIn("quick trials", lower_doc)
        self.assertIn("convenience", lower_doc)
        for heading in ("Compose", "Troubleshooting"):
            with self.subTest(heading=heading):
                markdown_section(container_doc, heading)

        bootstrap = markdown_section(container_doc, "API Bootstrap")
        for snippet in (
            f"available in the current published `{refs['published_release_tag']}` image",
            "built-in empty bootstrap config",
            "No static config",
            "mount is required",
            f'docker pull {refs["image"]}:{refs["published_release_tag"]}',
            f'{refs["image"]}:{refs["published_release_tag"]}',
            "not persisted by the proxy",
            "replay the Admin API",
            "after every container restart",
            "`/health` is liveness",
            "`/ready` is readiness",
            "runtime payload shape",
        ):
            with self.subTest(section="api_bootstrap", snippet=snippet):
                self.assertIn(snippet, bootstrap)
        self.assertNotIn("llm-universal-proxy:local", bootstrap)

        run_section = markdown_section(container_doc, "Run the Release Image")
        self.assertIn(
            "Do not use the unedited example config for real provider requests",
            run_section,
        )
        self.assertIn("replace the placeholder base URLs and model aliases", run_section)
        self.assertNotIn(
            f'{refs["image"]}:{refs["next_release_tag"]}',
            container_doc,
            "docs must not bind the next release tag to the published digest",
        )
        self.assertNotRegex(
            container_doc.casefold(),
            rf"current (?:published )?(?:container )?release is `{re.escape(refs['next_release_tag'].casefold())}`",
        )

    def test_readmes_summarize_published_container_usage(self):
        refs = container_refs()
        english = normalized_whitespace(read_doc("README.md"))
        chinese = normalized_whitespace(read_doc("README_CN.md"))

        readme_expectations = {
            "README.md": (
                english,
                f'The current published container release is `{refs["published_release_tag"]}`',
                f'Cargo package version `{refs["next_package_version"]}` is the next release identity, not a published container tag yet',
                rf"current published .*`{re.escape(refs['next_release_tag'])}`",
            ),
            "README_CN.md": (
                chinese,
                f'当前已发布容器版本是 `{refs["published_release_tag"]}`',
                f'Cargo package version `{refs["next_package_version"]}` 是下一次 release identity，并不是已发布容器 tag',
                rf"当前已发布.*`{re.escape(refs['next_release_tag'])}`",
            ),
        }

        for path, (text, published_semantics, next_semantics, forbidden) in readme_expectations.items():
            with self.subTest(path=path):
                self.assertIn(refs["image"], text)
                self.assertIn(published_semantics, text)
                self.assertIn(next_semantics, text)
                self.assertIn("digest", text)
                self.assertIn("docs/container.md", text)
                self.assertNotIn(f'{refs["image"]}:{refs["next_release_tag"]}', text)
                self.assertNotRegex(text.casefold(), forbidden.casefold())

    def test_docker_compose_defaults_to_current_release_not_latest(self):
        refs = container_refs()
        compose = read_doc("examples/docker-compose.yaml")

        self.assertIn(f'{refs["image"]}:{refs["published_release_tag"]}', compose)
        self.assertNotIn(f'{refs["image"]}:{refs["next_release_tag"]}', compose)
        self.assertNotIn(":latest", compose)
        self.assertRegex(
            compose,
            rf"(?m)^\s*image:\s*\$\{{LLMUP_IMAGE:-{re.escape(refs['image'])}:{refs['published_release_tag']}\}}\s*$",
        )

    def test_governance_locks_published_docker_image_docs_contract(self):
        refs = container_refs()
        governance = read_doc("scripts/check-governance.sh")

        for snippet in (
            "CONTAINER_IMAGE_MANIFEST",
            "PUBLISHED_CONTAINER_RELEASE_TAG",
            "PUBLISHED_CONTAINER_VERSION_TAG",
            "PUBLISHED_CONTAINER_DIGEST",
            "NEXT_RELEASE_TAG",
            'check_contains "docs/container.md" "${PUBLISHED_CONTAINER_IMAGE}:${PUBLISHED_CONTAINER_RELEASE_TAG}"',
            'check_contains "docs/container.md" "${PUBLISHED_CONTAINER_DIGEST_REF}"',
            'check_contains "docs/container.md" "not a published container tag yet"',
            'check_contains "docs/container.md" "Pin a release tag or digest for production"',
            'check_contains "docs/container.md" \'Do not use `latest` for production pinning\'',
            'check_contains "docs/container.md" "docker login ghcr.io"',
            'check_contains "docs/container.md" "personal access token (classic)"',
            'check_contains "docs/container.md" "read:packages"',
            'check_contains "docs/container.md" "GITHUB_USERNAME"',
            'check_contains "docs/container.md" "If the package is public"',
            'check_contains "docs/container.md" "unauthorized, 403, or package page appears 404"',
            'check_contains "README.md" "${PUBLISHED_CONTAINER_RELEASE_TAG}"',
            'check_contains "README_CN.md" "${PUBLISHED_CONTAINER_RELEASE_TAG}"',
            'check_contains "README.md" "${NEXT_PACKAGE_VERSION}"',
            'check_contains "README_CN.md" "${NEXT_PACKAGE_VERSION}"',
            'check_readme_container_release_semantics "README.md" en',
            'check_readme_container_release_semantics "README_CN.md" zh',
            'check_absent "docs/container.md" "fine-grained personal access token"',
            'check_absent "docs/container.md" \'$GITHUB_ACTOR\'',
            'check_contains "examples/docker-compose.yaml" "${PUBLISHED_CONTAINER_IMAGE}:${PUBLISHED_CONTAINER_RELEASE_TAG}"',
            'check_absent "examples/docker-compose.yaml" ":latest"',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, governance)
        self.assertNotIn(
            refs["published_digest"],
            governance,
            "governance must read the published digest from the manifest",
        )
        self.assertNotIn(
            f'{refs["image"]}:{refs["next_release_tag"]}',
            governance,
            "governance must not hard-code the next release tag as published",
        )

    def test_prd_metadata_and_section_numbers_are_current_for_ga(self):
        prd = read_doc("docs/PRD.md")

        self.assertIn("**Last Updated**: 2026-04-28", prd)

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

        for snippet in (
            "`provider_key: { inline: \"...\" }`",
            "`provider_key: { env: \"ENV\" }`",
            "`provider_key_env: ENV`",
            "`data_auth`",
            "`GET /admin/data-auth`",
            "`PUT /admin/data-auth`",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, prd)

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
            "provider_key_env: OPENAI_COMPATIBLE_API_KEY",
            "provider_key_env: ANTHROPIC_COMPATIBLE_API_KEY",
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
            "`LLM_UNIVERSAL_PROXY_AUTH_MODE`",
            "`client_provider_key`",
            "`proxy_key`",
            "`LLM_UNIVERSAL_PROXY_KEY`",
            "`Authorization: Bearer <proxy-key>`",
            "`LLM_UNIVERSAL_PROXY_ADMIN_TOKEN`",
            "`Authorization: Bearer <admin-token>`",
            "`provider_key_env`",
            "fail closed",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, constitution)

    def test_ga_readiness_review_uses_current_auth_boundary_language(self):
        ga_review = read_doc("docs/ga-readiness-review.md")
        completed_baseline = markdown_section(ga_review, "Completed Local Baseline")

        self.assertNotRegex(ga_review, r"\bdata[-_\s]+tokens?\b")
        for snippet in (
            "`LLM_UNIVERSAL_PROXY_AUTH_MODE`",
            "`client_provider_key`",
            "`proxy_key`",
            "`LLM_UNIVERSAL_PROXY_KEY`",
            "`provider_key_env`",
            "`data_auth`",
            "`provider_key.env`",
            "`provider_key.inline`",
            "`/admin/data-auth`",
            "admin-token boundary",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, completed_baseline)

    def test_client_manual_wiring_documents_proxy_key_sdk_contract(self):
        clients = read_doc("docs/clients.md")
        manual = markdown_section(clients, "Manual Wiring Without Wrappers")

        for forbidden in (
            "OPENAI_API_KEY=dummy",
            "ANTHROPIC_API_KEY=dummy",
            "GEMINI_API_KEY=dummy",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, manual)

        for snippet in (
            "OPENAI_API_KEY=$LLM_UNIVERSAL_PROXY_KEY",
            "ANTHROPIC_API_KEY=$LLM_UNIVERSAL_PROXY_KEY",
            "GEMINI_API_KEY=$LLM_UNIVERSAL_PROXY_KEY",
            "`proxy_key` mode",
            "`client_provider_key` mode, set these SDK keys to the real provider key",
            "`provider_key_env`",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, manual)

    def test_user_entry_auth_docs_do_not_describe_env_only_auth_or_provider_keys(self):
        docs = {
            "README.md": read_doc("README.md"),
            "README_CN.md": read_doc("README_CN.md"),
            "docs/clients.md": read_doc("docs/clients.md"),
            "docs/CONSTITUTION.md": read_doc("docs/CONSTITUTION.md"),
        }

        forbidden = (
            "Required proxy auth mode",
            "必填的 proxy auth mode",
            "Provider/model/resource routes require `LLM_UNIVERSAL_PROXY_AUTH_MODE`",
            "the proxy reads the real upstream provider key from `provider_key_env`",
            "the proxy reads upstream provider keys from `provider_key_env`",
        )
        for relative_path, text in docs.items():
            for snippet in forbidden:
                with self.subTest(path=relative_path, forbidden=snippet):
                    self.assertNotIn(snippet, text)

        expectations = {
            "README.md": (
                "Prefer static `data_auth`",
                "environment fallback when `data_auth` is omitted",
                "`provider_key.inline`, `provider_key.env`, or legacy `provider_key_env`",
            ),
            "README_CN.md": (
                "优先使用静态 `data_auth`",
                "省略 `data_auth` 时的环境变量兼容 fallback",
                "`provider_key.inline`、`provider_key.env` 或 legacy `provider_key_env`",
            ),
            "docs/clients.md": (
                "Prefer static `data_auth`",
                "compatibility environment fallback when `data_auth` is omitted",
                "`provider_key.inline`, `provider_key.env`, or legacy `provider_key_env`",
            ),
            "docs/CONSTITUTION.md": (
                "Prefer static `data_auth`",
                "compatibility environment fallback",
                "`provider_key.inline`, `provider_key.env`, or legacy `provider_key_env`",
            ),
        }
        for relative_path, snippets in expectations.items():
            for snippet in snippets:
                with self.subTest(path=relative_path, snippet=snippet):
                    self.assertIn(snippet, docs[relative_path])

    def test_configuration_doc_covers_global_auth_and_static_yaml_examples(self):
        config_doc = read_doc("docs/configuration.md")
        security = markdown_section(config_doc, "Data Plane Security")
        static_examples = markdown_subsection(security, "Static YAML Auth Examples")
        yaml_blocks = markdown_code_blocks(static_examples, "yaml")

        for snippet in (
            "process-wide",
            "all namespaces",
            "all provider/model/resource routes",
            "not a per-upstream setting",
            "run separate proxy instances",
            "`data_auth`",
            "If `data_auth` is omitted",
            "environment fallback",
            "`LLM_UNIVERSAL_PROXY_AUTH_MODE`",
            "`LLM_UNIVERSAL_PROXY_KEY`",
            "`proxy_key.inline`",
            "`proxy_key.env`",
            "`provider_key.inline`",
            "`provider_key.env`",
            "`provider_key_env` is a per-upstream environment variable name",
            "`provider_key.inline`, `provider_key.env`, and `provider_key_env` are mutually exclusive",
            "Inline and env source values must be non-empty",
            "Admin read views never return inline secret values",
            "`provider_key_env` is not required",
            "normally omitted",
            "not rejected",
            "not used",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, security)

        for repeated_sentence in (
            "That one state applies to all provider/model/resource routes.",
            "That path is the environment fallback.",
            "In this mode, `provider_key_env` is not required, is normally omitted, and `provider_key.env` or `provider_key_env` are not rejected if present, but are not used.",
        ):
            with self.subTest(repeated_sentence=repeated_sentence):
                self.assertNotIn(repeated_sentence, security)

        normalized_security = normalized_whitespace(security)
        for repeated_snippet in (
            "`provider_key.inline`, `provider_key.env`, and `provider_key_env` are mutually exclusive",
            "Inline and env source values must be non-empty.",
            "Admin read views never return inline secret values.",
            "`provider_key_env` is not required",
        ):
            with self.subTest(repeated_snippet=repeated_snippet):
                self.assertEqual(normalized_security.count(repeated_snippet), 1)

        proxy_key_block = next(
            (
                block
                for block in yaml_blocks
                if "data_auth:" in block
                and "mode: proxy_key" in block
                and "provider_key:" in block
                and "env: OPENAI_COMPATIBLE_API_KEY" in block
                and "provider_key_env: ANTHROPIC_COMPATIBLE_API_KEY" in block
            ),
            None,
        )
        self.assertIsNotNone(
            proxy_key_block,
            "proxy_key static YAML example must show data_auth plus structured and legacy provider key sources",
        )
        self.assertIn("#", proxy_key_block)
        self.assertIn("format: openai-completion", proxy_key_block)
        self.assertIn("format: anthropic", proxy_key_block)
        self.assertNotIn("LLM_UNIVERSAL_PROXY_AUTH_MODE", proxy_key_block)
        self.assertIn("env: LLM_UNIVERSAL_PROXY_KEY", proxy_key_block)

        client_provider_key_block = next(
            (
                block
                for block in yaml_blocks
                if "CLIENT-KEY-OPENAI-COMPATIBLE" in block
                and "CLIENT-KEY-ANTHROPIC-COMPATIBLE" in block
                and "mode: client_provider_key" in block
            ),
            None,
        )
        self.assertIsNotNone(
            client_provider_key_block,
            "client_provider_key static YAML example must be present",
        )
        self.assertIn("#", client_provider_key_block)
        self.assertNotIn("provider_key_env:", client_provider_key_block)
        self.assertNotIn("LLM_UNIVERSAL_PROXY_AUTH_MODE", client_provider_key_block)
        self.assertNotIn("LLM_UNIVERSAL_PROXY_KEY", client_provider_key_block)

        inline_example = next(
            (
                block
                for block in yaml_blocks
                if 'inline: "DEMO_PROVIDER_KEY_DO_NOT_USE"' in block
            ),
            None,
        )
        self.assertIsNotNone(
            inline_example,
            "docs must show inline provider_key only with an obvious fake value",
        )

    def test_admin_dynamic_config_doc_covers_runtime_payload_and_field_mapping(self):
        admin_doc = read_doc("docs/admin-dynamic-config.md")
        write_section = markdown_section(admin_doc, "Update a Namespace Without Restarting")
        api_examples = markdown_subsection(write_section, "API Configuration Examples")
        mapping = markdown_section(admin_doc, "Static YAML vs Runtime Payload")
        data_auth_section = markdown_section(admin_doc, "Update Data Auth Without Restarting")
        redaction = markdown_section(admin_doc, "What the Admin Read View Redacts")
        inspect_data_auth = markdown_subsection(
            markdown_section(admin_doc, "Inspect Runtime State"),
            "Inspect data auth",
        )

        typical_responses = markdown_code_blocks(inspect_data_auth, "json")
        data_auth_response = next(
            (
                response
                for response in (json.loads(block) for block in typical_responses)
                if isinstance(response, dict)
                and "revision" in response
                and "config" in response
            ),
            None,
        )
        self.assertIsNotNone(
            data_auth_response,
            "Inspect data auth must include a data-auth snapshot JSON example",
        )
        data_auth_config = data_auth_response["config"]
        self.assertIs(data_auth_config.get("configured"), True)
        self.assertEqual(data_auth_config["mode"], "proxy_key")
        self.assertIs(data_auth_config["proxy_key"]["configured"], True)

        for snippet in (
            "process-wide",
            "`data_auth`",
            "environment fallback",
            "all namespaces",
            "not set through the namespace payload",
            "`proxy_key` mode",
            "every upstream",
            "provider credential source",
            "`provider_key.inline`",
            "`provider_key.env`",
            "`provider_key_env`",
            "`client_provider_key` mode",
            "does not require `provider_key_env`",
            "normally omitted",
            "not rejected",
            "not used",
        ):
            with self.subTest(section="write", snippet=snippet):
                self.assertIn(snippet, write_section)

        for snippet in (
            "`PRESET_*` URL/model placeholders",
            "already-hydrated concrete URL/model values",
            "`provider_key_env` remains an environment variable name",
            "`PRESET_ENDPOINT_API_KEY` is valid",
            "`llm-universal-proxy --config <config.yaml>`",
            "`--admin-bootstrap` starts with no namespaces",
            "`/ready` stays unavailable",
        ):
            with self.subTest(section="preset", snippet=snippet):
                self.assertIn(snippet, admin_doc)

        yaml_blocks = markdown_code_blocks(api_examples, "yaml")
        self.assertTrue(yaml_blocks, "admin docs must include a YAML-shaped payload example")
        yaml_payload = yaml_blocks[0]
        for snippet in (
            "- name: PRESET-OPENAI-COMPATIBLE",
            "fixed_upstream_format: openai-completion",
            "provider_key_env: PRESET_ENDPOINT_API_KEY",
            "upstream_name: PRESET-OPENAI-COMPATIBLE",
            "upstream_model: provider-model-id",
        ):
            with self.subTest(section="yaml_payload", snippet=snippet):
                self.assertIn(snippet, yaml_payload)
        self.assertIn("#", yaml_payload)
        self.assertNotIn("LLM_UNIVERSAL_PROXY_AUTH_MODE", yaml_payload)

        json_or_bash_blocks = markdown_code_blocks(api_examples, "bash") + markdown_code_blocks(
            api_examples, "json"
        )
        self.assertTrue(json_or_bash_blocks, "admin docs must include a real JSON/curl example")
        combined_blocks = "\n".join(json_or_bash_blocks)
        for snippet in (
            "curl -fsS",
            "/admin/namespaces/default/config",
            '"upstreams": [',
            '"name": "PRESET-OPENAI-COMPATIBLE"',
            '"fixed_upstream_format": "openai-completion"',
            '"provider_key_env": "PRESET_ENDPOINT_API_KEY"',
            '"upstream_name": "PRESET-OPENAI-COMPATIBLE"',
            '"upstream_model": "provider-model-id"',
        ):
            with self.subTest(section="json_payload", snippet=snippet):
                self.assertIn(snippet, combined_blocks)

        for snippet in (
            "`upstreams` named map",
            "`upstreams` list of objects",
            "`format`",
            "`fixed_upstream_format`",
            "alias string",
            "`upstream_name` and `upstream_model`",
            "`data_auth`",
            "`/admin/data-auth`",
            "not in namespace payload",
            "`provider_key` object",
            "`provider_key_env` legacy field",
        ):
            with self.subTest(section="mapping", snippet=snippet):
                self.assertIn(snippet, mapping)

        for snippet in (
            "`GET /admin/data-auth`",
            "`PUT /admin/data-auth`",
            "`Authorization: Bearer <admin-token>`",
            "`if_revision`",
            "`412 Precondition Failed`",
            "redacted snapshot",
            "`proxy_key.inline`",
            "`proxy_key.env`",
            "new requests immediately use the new proxy key",
            "mode switch",
            "rebuilds the already-loaded namespaces",
            "failure is not committed",
            "does not persist plaintext keys",
            "replay the write after restart",
        ):
            with self.subTest(section="data_auth", snippet=snippet):
                self.assertIn(snippet, data_auth_section)

        for snippet in (
            "inline upstream credentials",
            "inline `data_auth.proxy_key` values",
            "`provider_key` view reports `source`, `configured`, `redacted`, and `env_name` when applicable",
            "provider_key_env presence",
        ):
            with self.subTest(section="redaction", snippet=snippet):
                self.assertIn(snippet, redaction)

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

    def test_changelog_latest_release_records_current_release_metadata(self):
        refs = container_refs()
        changelog = read_doc("CHANGELOG.md")
        version = cargo_package_version()
        heading_match = re.search(
            rf"^## v{re.escape(version)} - \d{{4}}-\d{{2}}-\d{{2}}$",
            changelog,
            flags=re.MULTILINE,
        )
        self.assertIsNotNone(heading_match)
        latest_release = markdown_section(changelog, heading_match.group(0)[3:])

        for snippet in (
            "release identity",
            f"occupied `{refs['published_release_tag']}` tag",
            "reusing the existing tag",
            "next patch version",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, latest_release)


if __name__ == "__main__":
    unittest.main()
