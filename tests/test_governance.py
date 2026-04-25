import pathlib
import re
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
GOVERNANCE_SCRIPT = REPO_ROOT / "scripts" / "check-governance.sh"
PYTHON_CONTRACT_TEST_COMMAND = (
    "PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p 'test*.py'"
)
ACTIVE_DOC_PATHS = (
    REPO_ROOT / "README.md",
    REPO_ROOT / "README_CN.md",
    *sorted((REPO_ROOT / "docs").glob("*.md")),
)
BOUNDARY_LANGUAGE_RE = re.compile(
    r"\b("
    r"portab\w+|"
    r"native[- ]extension\w*|"
    r"fail[- ]warn|"
    r"warn(?:ing|ings)?|"
    r"reject(?:s|ed|ing)?|"
    r"degrad\w+|"
    r"non[- ]portable|"
    r"boundar(?:y|ies)"
    r")\b",
    re.IGNORECASE,
)
NEGATED_BOUNDARY_LANGUAGE_RE = re.compile(
    r"\b(?:without|no)\s+(?:compatibility\s+)?"
    r"(?:warnings?|warn(?:ing|ings)?|reject(?:ion|ions|s|ed|ing)?)"
    r"(?:\s+or\s+(?:warnings?|warn(?:ing|ings)?|reject(?:ion|ions|s|ed|ing)?))*\b",
    re.IGNORECASE,
)
UNBOUNDED_COMPAT_PROMISE_PATTERNS = (
    (
        "drop-in replacement",
        re.compile(r"\bdrop[- ]in replacement\b", re.IGNORECASE),
    ),
    (
        "exact preservation / zero loss",
        re.compile(r"\bexact preservation\b|\bzero loss\b", re.IGNORECASE),
    ),
    (
        "Any-to-Any",
        re.compile(r"\bAny[- ]to[- ]Any\b", re.IGNORECASE),
    ),
    (
        "full fidelity",
        re.compile(r"\bfull fidelity\b", re.IGNORECASE),
    ),
    (
        "any client / any backend",
        re.compile(
            r"\bany client\b(?:(?!\n\s*\n).){0,220}\bany (?:LLM )?backend\b",
            re.IGNORECASE | re.DOTALL,
        ),
    ),
    (
        "transparent any upstream",
        re.compile(
            r"\btransparen\w*\b(?:(?!\n\s*\n).){0,220}\bany upstream\b",
            re.IGNORECASE | re.DOTALL,
        ),
    ),
    (
        "all 16 as unconditional success",
        re.compile(
            r"\ball 16\b(?:(?!\n\s*\n).){0,180}\b("
            r"full fidelity|work(?:ing|s| correctly)?|pass(?:es|ing)?"
            r")\b",
            re.IGNORECASE | re.DOTALL,
        ),
    ),
)


def has_valid_boundary_language(unit: str) -> bool:
    boundary_text = NEGATED_BOUNDARY_LANGUAGE_RE.sub("", unit)
    return BOUNDARY_LANGUAGE_RE.search(boundary_text) is not None


def claim_units(text: str):
    for paragraph_match in re.finditer(r"(?:[^\n]|\n(?!\s*\n))+", text):
        paragraph = paragraph_match.group(0).strip("\n")
        if not paragraph.strip():
            continue

        table_lines = [
            line for line in paragraph.splitlines() if line.lstrip().startswith("|")
        ]
        if len(table_lines) > 1:
            for line in table_lines:
                yield line, paragraph_match.start() + paragraph_match.group(0).find(line)
        else:
            yield paragraph, paragraph_match.start()


class GovernanceTests(unittest.TestCase):
    def test_default_test_entries_run_python_contract_tests_without_bytecode(self):
        entrypoints = {
            ".github/workflows/ci.yml": REPO_ROOT / ".github" / "workflows" / "ci.yml",
            "Makefile": REPO_ROOT / "Makefile",
            "scripts/test-and-report.sh": REPO_ROOT / "scripts" / "test-and-report.sh",
            "scripts/check-governance.sh": GOVERNANCE_SCRIPT,
        }

        missing = []
        for label, path in entrypoints.items():
            text = path.read_text(encoding="utf-8")
            if PYTHON_CONTRACT_TEST_COMMAND not in text:
                missing.append(label)

        self.assertFalse(
            missing,
            "Default test/governance entrypoints must run Python contract "
            f"tests without writing __pycache__: {', '.join(missing)}",
        )

    def test_governance_tracks_dynamic_proxy_binary_rule(self):
        script = GOVERNANCE_SCRIPT.read_text(encoding="utf-8")

        self.assertIn(
            'check_contains "scripts/real_cli_matrix.py" "def default_proxy_binary_path("',
            script,
        )
        self.assertIn(
            'check_contains "scripts/real_cli_matrix.py" \'DEFAULT_PROXY_BINARY = default_proxy_binary_path()\'',
            script,
        )
        self.assertIn(
            'check_contains "scripts/interactive_cli.py" \'default=str(default_proxy_binary_path())\'',
            script,
        )
        self.assertNotIn(
            'check_contains "scripts/real_cli_matrix.py" \'DEFAULT_PROXY_BINARY = REPO_ROOT / "target" / "release" / "llm-universal-proxy"\'',
            script,
        )

    def test_container_image_contract_is_governed(self):
        dockerfile = (REPO_ROOT / "Dockerfile").read_text(encoding="utf-8")
        required = (
            "ARG RUST_TOOLCHAIN=",
            "FROM ${RUST_BASE_IMAGE} AS builder",
            "FROM ${RUNTIME_BASE_IMAGE}",
            "cargo build --locked --release",
            'org.opencontainers.image.source="https://github.com/lzjever/llm-universal-proxy"',
            "USER llmup:llmup",
            "EXPOSE 8080",
            "HEALTHCHECK",
            "http://127.0.0.1:8080/health",
            'CMD ["--config", "/etc/llmup/config.yaml"]',
        )

        for pattern in required:
            with self.subTest(pattern=pattern):
                self.assertIn(pattern, dockerfile)

    def test_docker_smoke_target_has_script_and_governance_coverage(self):
        makefile = (REPO_ROOT / "Makefile").read_text(encoding="utf-8")
        script = (REPO_ROOT / "scripts" / "test_container_smoke.sh").read_text(
            encoding="utf-8"
        )
        governance = GOVERNANCE_SCRIPT.read_text(encoding="utf-8")

        self.assertIn("docker-smoke", makefile)
        self.assertIn("scripts/test_container_smoke.sh", makefile)
        self.assertIn("/etc/llmup/config.yaml", script)
        self.assertIn("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN=${ADMIN_TOKEN}", script)
        self.assertIn("host.docker.internal:host-gateway", script)
        self.assertIn('CONTAINER_PORT="8080"', script)
        self.assertIn("listen: 0.0.0.0:${CONTAINER_PORT}", script)
        self.assertIn('-p "${HOST}:${PROXY_PORT}:${CONTAINER_PORT}"', script)
        self.assertIn("wait_for_container_healthy", script)
        self.assertNotIn("listen: 0.0.0.0:${PROXY_PORT}", script)
        self.assertIn("scripts/test_container_smoke.sh", governance)

    def test_ci_and_release_workflows_keep_container_publish_scope_tight(self):
        ci = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        release = (REPO_ROOT / ".github" / "workflows" / "release.yml").read_text(
            encoding="utf-8"
        )

        self.assertIn("Container Image Smoke", ci)
        self.assertIn("push: false", ci)
        self.assertIn(
            "IMAGE=llm-universal-proxy:ci bash scripts/test_container_smoke.sh",
            ci,
        )
        self.assertIn("GHCR_IMAGE: ghcr.io/lzjever/llm-universal-proxy", release)
        self.assertIn("platforms: linux/amd64,linux/arm64", release)
        self.assertIn("push: true", release)
        self.assertIn("${{ env.GHCR_IMAGE }}:latest", release)
        self.assertIn('DOCKER_BUILD_RECORD_UPLOAD: "false"', release)
        self.assertIn("pattern: llm-universal-proxy-*", release)
        self.assertIn(
            "IMAGE=llm-universal-proxy:release-smoke bash scripts/test_container_smoke.sh",
            release,
        )
        self.assertNotIn(":edge", release)

    def test_container_examples_and_docs_do_not_bake_secrets(self):
        container_config = (REPO_ROOT / "examples" / "container-config.yaml").read_text(
            encoding="utf-8"
        )
        compose = (REPO_ROOT / "examples" / "docker-compose.yaml").read_text(
            encoding="utf-8"
        )
        container_doc = (REPO_ROOT / "docs" / "container.md").read_text(
            encoding="utf-8"
        )
        admin_doc = (REPO_ROOT / "docs" / "admin-dynamic-config.md").read_text(
            encoding="utf-8"
        )

        self.assertIn("listen: 0.0.0.0:8080", container_config)
        self.assertIn("credential_env: OPENAI_API_KEY", container_config)
        self.assertNotIn("credential_actual", container_config)
        self.assertIn("${OPENAI_API_KEY:?set OPENAI_API_KEY}", compose)
        self.assertIn(
            "${LLM_UNIVERSAL_PROXY_ADMIN_TOKEN:?set LLM_UNIVERSAL_PROXY_ADMIN_TOKEN}",
            compose,
        )
        self.assertNotRegex(container_config + compose, r"sk-[A-Za-z0-9]")
        self.assertIn("ghcr.io/lzjever/llm-universal-proxy", container_doc)
        self.assertIn(
            "Do not mount the local quickstart config unchanged for container service mode",
            container_doc,
        )
        self.assertIn("do not introduce a separate service key", admin_doc)

    def test_readme_and_docs_expose_container_entrypoint_only(self):
        readme = (REPO_ROOT / "README.md").read_text(encoding="utf-8")
        readme_cn = (REPO_ROOT / "README_CN.md").read_text(encoding="utf-8")
        docs_index = (REPO_ROOT / "docs" / "README.md").read_text(encoding="utf-8")

        self.assertIn("docs/container.md", readme)
        self.assertIn("docs/container.md", readme_cn)
        self.assertIn("container.md", docs_index)

    def test_active_docs_bound_overbroad_compatibility_promises(self):
        violations = []

        for path in ACTIVE_DOC_PATHS:
            text = path.read_text(encoding="utf-8")
            for unit, start_index in claim_units(text):
                for label, pattern in UNBOUNDED_COMPAT_PROMISE_PATTERNS:
                    if pattern.search(unit) and not has_valid_boundary_language(unit):
                        line_no = text.count("\n", 0, start_index) + 1
                        excerpt = " ".join(unit.strip().split())
                        violations.append(
                            f"{path.relative_to(REPO_ROOT)}:{line_no}: "
                            f"{label}: {excerpt[:180]}"
                        )

        self.assertFalse(
            violations,
            "Unbounded compatibility promises must include same-paragraph "
            "portability/native-extension/fail-warn boundaries:\n"
            + "\n".join(violations),
        )

    def test_overbroad_compatibility_patterns_cover_high_risk_language(self):
        risky_text = "\n\n".join(
            (
                "Text content - exact preservation, zero loss.",
                "Any-to-Any: Every supported client protocol can reach every supported upstream protocol.",
                "The proxy is a drop-in replacement without warning.",
            )
        )
        detected_labels = set()

        for unit, _start_index in claim_units(risky_text):
            for label, pattern in UNBOUNDED_COMPAT_PROMISE_PATTERNS:
                if pattern.search(unit) and not has_valid_boundary_language(unit):
                    detected_labels.add(label)

        self.assertIn("exact preservation / zero loss", detected_labels)
        self.assertIn("Any-to-Any", detected_labels)
        self.assertIn("drop-in replacement", detected_labels)


if __name__ == "__main__":
    unittest.main()
