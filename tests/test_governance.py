import os
import pathlib
import re
import shutil
import stat
import subprocess
import tempfile
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yml"
RELEASE_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "release.yml"
GOVERNANCE_SCRIPT = REPO_ROOT / "scripts" / "check-governance.sh"
SUPPLY_CHAIN_AUDIT_SCRIPT = REPO_ROOT / "scripts" / "supply_chain_audit.sh"
SUPPLY_CHAIN_AUDIT_COMMAND = "bash scripts/supply_chain_audit.sh"
LOCKFILE_INTEGRITY_COMMAND = "cargo metadata --locked --format-version 1 --no-deps"
PYTHON_CONTRACT_TEST_COMMAND = (
    "PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p 'test*.py'"
)
CODEX_SCRIPTED_INTERACTIVE_GATE_COMMAND = (
    "PYTHONDONTWRITEBYTECODE=1 python3 -m unittest "
    "tests.test_interactive_cli.InteractiveCliTests."
    "test_codex_wrapper_executes_scripted_interactive_two_turns_hermetically"
)
PROVIDER_KEY_PATTERN_SNIPPETS = (
    "sk-cp-",
    "sk-ant-",
    "sk-proj-",
)
OFFICIAL_PROVIDER_SECRET_ENVS = (
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "GEMINI_API_KEY",
    "MINIMAX_API_KEY",
)
COMPAT_PROVIDER_SECRET_ENVS = (
    "COMPAT_PROVIDER_API_KEY",
    "COMPAT_OPENAI_API_KEY",
    "COMPAT_ANTHROPIC_API_KEY",
)
COMPAT_PROVIDER_VAR_ENVS = (
    "COMPAT_OPENAI_BASE_URL",
    "COMPAT_OPENAI_MODEL",
    "COMPAT_ANTHROPIC_BASE_URL",
    "COMPAT_ANTHROPIC_MODEL",
    "COMPAT_PROVIDER_LABEL",
)
COMPAT_PROVIDER_SMOKE_JSON = "artifacts/compatible-provider-smoke.json"
ACTIVE_DOC_PATHS = (
    REPO_ROOT / "README.md",
    REPO_ROOT / "README_CN.md",
    *sorted((REPO_ROOT / "docs").glob("*.md")),
)
AUTH_CONTRACT_SCAN_PATHS = (
    REPO_ROOT / "README.md",
    REPO_ROOT / "README_CN.md",
    *sorted((REPO_ROOT / "docs").glob("*.md")),
    *sorted((REPO_ROOT / "examples").glob("**/*")),
    *sorted((REPO_ROOT / "scripts").glob("**/*")),
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


def legacy_auth_terms() -> tuple[str, ...]:
    data_prefix = "LLM_UNIVERSAL_PROXY_" + "DATA"
    credential = "credential"
    fallback = "fallback"
    return (
        data_prefix + "_TOKEN",
        data_prefix + "_AUTH",
        "X-LLMUP-" + "Data-" + "Token",
        "_".join(("auth", "policy")),
        "_".join(("client", "or", fallback)),
        "_".join(("force", "server")),
        "_".join((credential, "env")),
        "_".join((credential, "actual")),
        "_".join((fallback, credential, "env")),
        "_".join((fallback, credential, "actual")),
        "_".join((fallback, "api", "key")),
    )


def legacy_auth_prose_pattern() -> re.Pattern[str]:
    return re.compile(r"\bdata[-_\s]+tokens?\b", re.IGNORECASE)


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


def curl_command_blocks(script: str):
    block = []
    for line in script.splitlines():
        stripped = line.lstrip()
        if not block and not re.search(r"(^|[^\w])curl\b", stripped):
            continue
        block.append(line)
        if not line.rstrip().endswith("\\"):
            yield "\n".join(block)
            block = []
    if block:
        yield "\n".join(block)


def has_compatible_provider_smoke_invocation(text: str) -> bool:
    return any(
        "python3 scripts/real_endpoint_matrix.py" in line
        and "--mode compatible-provider-smoke" in line
        and "--json-out" in line
        and COMPAT_PROVIDER_SMOKE_JSON in line
        for line in text.splitlines()
    )


def workflow_jobs(workflow_path: pathlib.Path):
    text = workflow_path.read_text(encoding="utf-8")
    matches = list(re.finditer(r"^  ([A-Za-z0-9_-]+):\n", text, re.MULTILINE))
    jobs = {}
    for index, match in enumerate(matches):
        job_name = match.group(1)
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        jobs[job_name] = text[match.start() : end]
    return jobs


def workflow_step_block(text: str, step_name: str) -> str:
    marker = f"      - name: {step_name}"
    start = text.find(marker)
    if start == -1:
        return ""
    next_step = text.find("\n      - name: ", start + len(marker))
    if next_step == -1:
        return text[start:]
    return text[start:next_step]


class GovernanceTests(unittest.TestCase):
    def test_governance_checkout_fetches_full_history_for_tag_visibility(self):
        for workflow_path in (CI_WORKFLOW, RELEASE_WORKFLOW):
            with self.subTest(workflow=workflow_path.name):
                job = workflow_jobs(workflow_path).get("governance", "")
                self.assertTrue(job, "workflow must define a governance job")
                checkout_step = workflow_step_block(job, "Checkout code")
                self.assertTrue(
                    checkout_step,
                    "governance job must checkout repository code",
                )
                self.assertIn("uses: actions/checkout@v5", checkout_step)
                self.assertIn("        with:", checkout_step)
                self.assertRegex(checkout_step, r"(?m)^          fetch-depth: 0$")

    def test_governance_fails_closed_in_github_actions_shallow_checkout(self):
        real_git = shutil.which("git")
        self.assertIsNotNone(real_git, "git must be available for governance tests")

        with tempfile.TemporaryDirectory() as temp_dir:
            fake_bin = pathlib.Path(temp_dir)
            fake_git = fake_bin / "git"
            fake_git.write_text(
                """#!/usr/bin/env python3
import os
import sys

args = sys.argv[1:]
if args[:1] == ["rev-parse"] and "--is-shallow-repository" in args:
    print("true")
    sys.exit(0)

real_git = os.environ["REAL_GIT"]
os.execv(real_git, [real_git, *args])
""",
                encoding="utf-8",
            )
            fake_git.chmod(fake_git.stat().st_mode | stat.S_IXUSR)

            env = os.environ.copy()
            env.update(
                {
                    "GITHUB_ACTIONS": "true",
                    "PATH": f"{fake_bin}{os.pathsep}{env.get('PATH', '')}",
                    "REAL_GIT": real_git,
                }
            )

            result = subprocess.run(
                ["bash", str(GOVERNANCE_SCRIPT)],
                cwd=REPO_ROOT,
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                check=False,
            )

        output = result.stdout + result.stderr
        self.assertNotEqual(
            result.returncode,
            0,
            "governance must fail closed in GitHub Actions shallow checkouts",
        )
        self.assertIn("shallow", output.lower())
        self.assertIn("fetch-depth: 0", output)
        self.assertIn("tag visibility", output)

    def test_governance_fails_when_current_version_tag_is_occupied_by_another_head(self):
        version_match = re.search(
            r'^version = "([^"]+)"',
            (REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8"),
            re.MULTILINE,
        )
        self.assertIsNotNone(version_match, "Cargo.toml must declare package version")
        version = version_match.group(1)

        real_git = shutil.which("git")
        self.assertIsNotNone(real_git, "git must be available for governance tests")

        with tempfile.TemporaryDirectory() as temp_dir:
            fake_bin = pathlib.Path(temp_dir)
            fake_git = fake_bin / "git"
            fake_git.write_text(
                """#!/usr/bin/env python3
import os
import sys

args = sys.argv[1:]
tag_ref = os.environ["FAKE_GIT_OCCUPIED_TAG_REF"]
head_sha = os.environ["FAKE_GIT_HEAD_SHA"]
tag_sha = os.environ["FAKE_GIT_TAG_SHA"]


def is_occupied_tag(value):
    return value == tag_ref or value == f"{tag_ref}^{{commit}}"


if args[:1] == ["rev-parse"]:
    if args[-1] in ("HEAD", "HEAD^{commit}"):
        print(head_sha)
        sys.exit(0)
    if any(is_occupied_tag(arg) for arg in args):
        print(tag_sha)
        sys.exit(0)

if args[:1] == ["show-ref"] and any(is_occupied_tag(arg) for arg in args):
    sys.exit(0)

if args[:1] == ["rev-list"] and any(is_occupied_tag(arg) for arg in args):
    print(tag_sha)
    sys.exit(0)

real_git = os.environ["REAL_GIT"]
os.execv(real_git, [real_git, *args])
""",
                encoding="utf-8",
            )
            fake_git.chmod(fake_git.stat().st_mode | stat.S_IXUSR)

            env = os.environ.copy()
            env.update(
                {
                    "FAKE_GIT_OCCUPIED_TAG_REF": f"refs/tags/v{version}",
                    "FAKE_GIT_HEAD_SHA": "1" * 40,
                    "FAKE_GIT_TAG_SHA": "2" * 40,
                    "GITHUB_REF": f"refs/tags/v{version}",
                    "PATH": f"{fake_bin}{os.pathsep}{env.get('PATH', '')}",
                    "REAL_GIT": real_git,
                }
            )

            result = subprocess.run(
                ["bash", str(GOVERNANCE_SCRIPT)],
                cwd=REPO_ROOT,
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                check=False,
            )

        output = result.stdout + result.stderr
        self.assertNotEqual(
            result.returncode,
            0,
            "governance must fail when the current version tag already points "
            "at a different commit",
        )
        self.assertIn(f"refs/tags/v{version}", output)
        self.assertIn("current HEAD", output)

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
            'org.opencontainers.image.source="https://github.com/agentsmith-project/llm-universal-proxy"',
            "USER llmup:llmup",
            "EXPOSE 8080",
            "HEALTHCHECK",
            "http://127.0.0.1:8080/ready",
            "/etc/llmup/config.yaml",
            "listen: 0.0.0.0:8080",
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
        self.assertIn("LLM_UNIVERSAL_PROXY_AUTH_MODE=proxy_key", script)
        self.assertIn("LLM_UNIVERSAL_PROXY_KEY=${PROXY_KEY}", script)
        self.assertIn(
            "CONTAINER_SMOKE_UPSTREAM_API_KEY=container-smoke-provider-key",
            script,
        )
        self.assertIn(
            'EXPECTED_X_API_KEY = "container-smoke-provider-key"',
            script,
        )
        self.assertIn('self.headers.get("x-api-key")', script)
        self.assertIn("unexpected upstream authorization", script)
        self.assertIn("host.docker.internal:host-gateway", script)
        self.assertIn('CONTAINER_PORT="8080"', script)
        self.assertIn("listen: 0.0.0.0:${CONTAINER_PORT}", script)
        self.assertIn("format: anthropic", script)
        self.assertIn("provider_key_env: CONTAINER_SMOKE_UPSTREAM_API_KEY", script)
        self.assertIn('-p "${HOST}:${PROXY_PORT}:${CONTAINER_PORT}"', script)
        self.assertIn("wait_for_container_healthy", script)
        self.assertIn("/ready", script)
        self.assertIn("default empty config -> admin apply -> ready", script)
        self.assertIn("Authorization: Bearer ${ADMIN_TOKEN}", script)
        self.assertIn("No bootstrap config bind mount", script)
        self.assertNotIn("listen: 0.0.0.0:${PROXY_PORT}", script)
        self.assertIn("scripts/test_container_smoke.sh", governance)

        data_route_curls = [
            block
            for block in curl_command_blocks(script)
            if "http://${HOST}:${PROXY_PORT}" in block
            and "/health" not in block
            and "/ready" not in block
            and "/admin/" not in block
        ]
        self.assertGreater(
            len(data_route_curls),
            0,
            "Container smoke must exercise at least one data-plane route",
        )
        for block in data_route_curls:
            with self.subTest(curl=block):
                self.assertIn(
                    "Authorization: Bearer ${PROXY_KEY}",
                    block,
                    "Every data-plane smoke curl must send the proxy key",
                )

    def test_binary_smoke_uses_client_provider_key_auth_contract(self):
        script = (REPO_ROOT / "scripts" / "test_binary_smoke.sh").read_text(
            encoding="utf-8"
        )
        governance = GOVERNANCE_SCRIPT.read_text(encoding="utf-8")

        self.assertIn("scripts/test_binary_smoke.sh", governance)
        self.assertIn("LLM_UNIVERSAL_PROXY_AUTH_MODE=client_provider_key", script)
        self.assertNotIn("LLM_UNIVERSAL_PROXY_KEY=", script)
        self.assertIn('SMOKE_PROVIDER_KEY="binary-smoke-provider-key"', script)
        self.assertIn(
            'python3 -u - "$MOCK_PORT_FILE" "$SMOKE_PROVIDER_KEY"',
            script,
        )
        self.assertIn("expected_provider_key = sys.argv[2]", script)
        self.assertIn('self.headers.get("x-api-key")', script)
        self.assertIn('self.headers.get("Authorization")', script)
        self.assertIn("unexpected upstream authorization", script)
        self.assertNotIn("provider_key_env:", script)

        data_route_curls = [
            block
            for block in curl_command_blocks(script)
            if "http://${HOST}:${PROXY_PORT}" in block
            and "/health" not in block
            and "/admin/" not in block
        ]
        self.assertGreater(
            len(data_route_curls),
            0,
            "Binary smoke must exercise at least one data-plane route",
        )
        for block in data_route_curls:
            with self.subTest(curl=block):
                self.assertIn(
                    "Authorization: Bearer ${SMOKE_PROVIDER_KEY}",
                    block,
                    "Every data-plane binary smoke curl must send the provider key",
                )

        for governed_snippet in (
            "LLM_UNIVERSAL_PROXY_AUTH_MODE=client_provider_key",
            'SMOKE_PROVIDER_KEY="binary-smoke-provider-key"',
            'python3 -u - "$MOCK_PORT_FILE" "$SMOKE_PROVIDER_KEY"',
            'self.headers.get("x-api-key")',
            'self.headers.get("Authorization")',
            "unexpected upstream authorization",
            "Authorization: Bearer ${SMOKE_PROVIDER_KEY}",
        ):
            with self.subTest(governed_snippet=governed_snippet):
                self.assertIn(
                    f'check_contains "scripts/test_binary_smoke.sh" \'{governed_snippet}\'',
                    governance,
                )

        self.assertIn(
            'check_absent "scripts/test_binary_smoke.sh" "provider_key_env:"',
            governance,
        )
        self.assertIn(
            'check_absent "scripts/test_binary_smoke.sh" "LLM_UNIVERSAL_PROXY_KEY="',
            governance,
        )

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
        self.assertIn("GHCR_IMAGE: ghcr.io/agentsmith-project/llm-universal-proxy", release)
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

    def test_compatible_provider_release_gate_contract_is_governed(self):
        release = (REPO_ROOT / ".github" / "workflows" / "release.yml").read_text(
            encoding="utf-8"
        )
        governance = GOVERNANCE_SCRIPT.read_text(encoding="utf-8")

        self.assertIn("compatible-provider-smoke:", release)
        self.assertIn("environment: release-compatible-provider", release)
        self.assertNotIn("Validate protected real provider secrets", release)
        run_step = workflow_step_block(release, "Run compatible provider smoke")
        self.assertTrue(run_step)
        for secret_name in COMPAT_PROVIDER_SECRET_ENVS:
            with self.subTest(secret=secret_name):
                self.assertIn(f"{secret_name}: ${{{{ secrets.{secret_name} }}}}", run_step)
                self.assertNotIn(f'test -n "${{{secret_name}:-}}"', release)
                self.assertIn(f'check_contains ".github/workflows/release.yml" \'{secret_name}: ${{{{ secrets.{secret_name} }}}}\'', governance)
                self.assertIn(f'check_absent ".github/workflows/release.yml" \'test -n "${{{secret_name}:-}}"\'', governance)
        for var_name in COMPAT_PROVIDER_VAR_ENVS:
            with self.subTest(var=var_name):
                self.assertIn(f"{var_name}: ${{{{ vars.{var_name} }}}}", run_step)
                self.assertNotIn(f'test -n "${{{var_name}:-}}"', release)
                self.assertIn(f'check_contains ".github/workflows/release.yml" \'{var_name}: ${{{{ vars.{var_name} }}}}\'', governance)
                self.assertIn(f'check_absent ".github/workflows/release.yml" \'test -n "${{{var_name}:-}}"\'', governance)
        for secret_name in OFFICIAL_PROVIDER_SECRET_ENVS:
            with self.subTest(no_official_secret=secret_name):
                self.assertNotIn(f"{secret_name}: ${{{{ secrets.{secret_name} }}}}", run_step)
        self.assertNotIn("GLM_APIKEY", run_step)
        self.assertNotIn("secrets.GLM_APIKEY", release)
        self.assertIn(
            'check_absent ".github/workflows/release.yml" \'GLM_APIKEY: ${{ secrets.GLM_APIKEY }}\'',
            governance,
        )

        self.assertTrue(has_compatible_provider_smoke_invocation(release))
        upload_step = workflow_step_block(release, "Upload compatible provider smoke result")
        self.assertTrue(upload_step)
        self.assertIn('if: ${{ always() }}', upload_step)
        self.assertIn("name: compatible-provider-smoke", upload_step)
        self.assertIn(f"path: {COMPAT_PROVIDER_SMOKE_JSON}", upload_step)
        self.assertIn("if-no-files-found: error", upload_step)

        for snippet in (
            "COMPAT_PROVIDER_SECRET_ENVS",
            "COMPAT_PROVIDER_VAR_ENVS",
            "OFFICIAL_PROVIDER_SECRET_ENVS",
            "COMPAT_PROVIDER_SMOKE_JSON",
            "check_compatible_provider_smoke_invocation",
            'check_absent ".github/workflows/release.yml" "Validate protected real provider secrets"',
            "Upload compatible provider smoke result",
            'if: ${{ always() }}',
            "name: compatible-provider-smoke",
            f"path: {COMPAT_PROVIDER_SMOKE_JSON}",
            "if-no-files-found: error",
        ):
            with self.subTest(governance_snippet=snippet):
                self.assertIn(snippet, governance)

    def test_cli_wrapper_release_gate_contract_is_governed(self):
        release = (REPO_ROOT / ".github" / "workflows" / "release.yml").read_text(
            encoding="utf-8"
        )
        governance = GOVERNANCE_SCRIPT.read_text(encoding="utf-8")

        job = workflow_jobs(RELEASE_WORKFLOW).get("cli-wrapper-matrix", "")
        self.assertTrue(job, "release workflow must define cli-wrapper-matrix")
        run_step = workflow_step_block(job, "Run CLI wrapper matrix")
        self.assertTrue(run_step)
        self.assertIn(CODEX_SCRIPTED_INTERACTIVE_GATE_COMMAND, run_step)
        self.assertIn(
            "python3 scripts/real_cli_matrix.py --test basic --skip-slow --list-matrix",
            run_step,
        )
        self.assertNotIn("--test live", run_step)
        self.assertNotIn("--mode real-provider-smoke", run_step)
        self.assertIn(CODEX_SCRIPTED_INTERACTIVE_GATE_COMMAND, release)

        for snippet in (
            "CODEX_SCRIPTED_INTERACTIVE_GATE_COMMAND",
            CODEX_SCRIPTED_INTERACTIVE_GATE_COMMAND,
            "check_cli_wrapper_matrix_contract",
            'check_contains ".github/workflows/release.yml" "$CODEX_SCRIPTED_INTERACTIVE_GATE_COMMAND"',
            'check_contains "tests/test_interactive_cli.py" "test_codex_wrapper_executes_scripted_interactive_two_turns_hermetically"',
            'check_absent ".github/workflows/release.yml" "--test live"',
        ):
            with self.subTest(governance_snippet=snippet):
                self.assertIn(snippet, governance)

    def test_release_test_gate_matches_ci_python_contract_gate(self):
        ci = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        release = (REPO_ROOT / ".github" / "workflows" / "release.yml").read_text(
            encoding="utf-8"
        )

        self.assertIn("Run Rust tests", ci)
        self.assertIn("cargo test --locked --verbose", ci)
        self.assertIn("Run Python contract tests", ci)
        self.assertIn(PYTHON_CONTRACT_TEST_COMMAND, ci)
        self.assertIn("Run Rust tests", release)
        self.assertIn("cargo test --locked --verbose", release)
        self.assertIn("Run Python contract tests", release)
        self.assertIn(PYTHON_CONTRACT_TEST_COMMAND, release)
        self.assertRegex(ci, r"(?i)secret scan")
        self.assertRegex(release, r"(?i)secret scan")

    def test_supply_chain_audit_contract_is_governed(self):
        ci = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        release = (REPO_ROOT / ".github" / "workflows" / "release.yml").read_text(
            encoding="utf-8"
        )
        governance = GOVERNANCE_SCRIPT.read_text(encoding="utf-8")

        self.assertTrue(
            SUPPLY_CHAIN_AUDIT_SCRIPT.exists(),
            "supply-chain gate must use a shared script entrypoint",
        )
        audit_script = SUPPLY_CHAIN_AUDIT_SCRIPT.read_text(encoding="utf-8")

        for label, workflow in (("ci", ci), ("release", release)):
            with self.subTest(workflow=label):
                self.assertIn(SUPPLY_CHAIN_AUDIT_COMMAND, workflow)
                self.assertNotIn("cargo audit --locked", workflow)

        self.assertIn(LOCKFILE_INTEGRITY_COMMAND, audit_script)
        self.assertIn("cargo audit", audit_script)
        self.assertNotIn("cargo audit --locked", audit_script)

        for snippet in (
            f'check_contains ".github/workflows/ci.yml" "{SUPPLY_CHAIN_AUDIT_COMMAND}"',
            f'check_contains ".github/workflows/release.yml" "{SUPPLY_CHAIN_AUDIT_COMMAND}"',
            'check_absent ".github/workflows/ci.yml" "cargo audit --locked"',
            'check_absent ".github/workflows/release.yml" "cargo audit --locked"',
            f'check_contains "scripts/supply_chain_audit.sh" "{LOCKFILE_INTEGRITY_COMMAND}"',
            'check_contains "scripts/supply_chain_audit.sh" "cargo audit"',
            'check_absent "scripts/supply_chain_audit.sh" "cargo audit --locked"',
        ):
            with self.subTest(governance_snippet=snippet):
                self.assertIn(snippet, governance)

    def test_governance_secret_scan_covers_tracked_fixtures_docs_examples_scripts(self):
        script = GOVERNANCE_SCRIPT.read_text(encoding="utf-8")
        default_config = (
            REPO_ROOT
            / "scripts"
            / "fixtures"
            / "cli_matrix"
            / "default_proxy_test_matrix.yaml"
        ).read_text(encoding="utf-8")

        self.assertIn("scan_tracked_secret_risks", script)
        self.assertIn("git ls-files", script)
        for path_prefix in ("scripts/fixtures", "docs", "examples", "scripts"):
            with self.subTest(path_prefix=path_prefix):
                self.assertIn(path_prefix, script)
        for key_pattern in PROVIDER_KEY_PATTERN_SNIPPETS:
            with self.subTest(key_pattern=key_pattern):
                self.assertIn(key_pattern, script)
                self.assertNotIn(key_pattern, default_config)
        self.assertIn("provider_key_env: PRESET_ENDPOINT_API_KEY", default_config)
        self.assertIn("PRESET-OPENAI-COMPATIBLE", default_config)
        self.assertIn("PRESET-ANTHROPIC-COMPATIBLE", default_config)
        self.assertNotIn("MINIMAX", default_config.upper())
        self.assertNotIn("provider_key_inline", default_config)

    def test_governance_bans_legacy_data_auth_contract_terms_in_active_surfaces(self):
        script = GOVERNANCE_SCRIPT.read_text(encoding="utf-8")

        for snippet in (
            "scan_tracked_auth_contract",
            '["git", "ls-files", "README.md", "README_CN.md", "docs", "examples", "scripts"]',
            'parts = ("data", "token")',
            "old_prose_re",
            "LLM_UNIVERSAL_PROXY_AUTH_MODE",
            "LLM_UNIVERSAL_PROXY_KEY",
            "provider_key_env",
            "client_provider_key",
            "proxy_key",
        ):
            with self.subTest(governance_snippet=snippet):
                self.assertIn(snippet, script)

        violations = []
        old_prose_re = legacy_auth_prose_pattern()
        for path in AUTH_CONTRACT_SCAN_PATHS:
            if (
                not path.is_file()
                or "protocol-baselines" in path.parts
                or "__pycache__" in path.parts
            ):
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
            for term in legacy_auth_terms():
                if term in text:
                    line_no = text.count("\n", 0, text.find(term)) + 1
                    violations.append(f"{path.relative_to(REPO_ROOT)}:{line_no}: {term}")
            for match in old_prose_re.finditer(text):
                line_no = text.count("\n", 0, match.start()) + 1
                violations.append(
                    f"{path.relative_to(REPO_ROOT)}:{line_no}: {match.group(0)}"
                )

        self.assertFalse(
            violations,
            "Active docs/examples/scripts must use the new auth contract:\n"
            + "\n".join(violations),
        )

    def test_governance_blocks_dummy_sdk_keys_and_stale_fallback_credential_docs(self):
        script = GOVERNANCE_SCRIPT.read_text(encoding="utf-8")
        clients_doc = (REPO_ROOT / "docs" / "clients.md").read_text(encoding="utf-8")
        admin_doc = (REPO_ROOT / "docs" / "admin-dynamic-config.md").read_text(
            encoding="utf-8"
        )

        for snippet in (
            'check_absent "docs/clients.md" "OPENAI_API_KEY=dummy"',
            'check_absent "docs/clients.md" "ANTHROPIC_API_KEY=dummy"',
            'check_absent "docs/clients.md" "GEMINI_API_KEY=dummy"',
            'check_absent "docs/admin-dynamic-config.md" "fallback credential"',
            'check_absent "docs/admin-dynamic-config.md" "fallback_credential"',
        ):
            with self.subTest(governance_snippet=snippet):
                self.assertIn(snippet, script)

        for forbidden in (
            "OPENAI_API_KEY=dummy",
            "ANTHROPIC_API_KEY=dummy",
            "GEMINI_API_KEY=dummy",
        ):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, clients_doc)

        for snippet in (
            "OPENAI_API_KEY=$LLM_UNIVERSAL_PROXY_KEY",
            "ANTHROPIC_API_KEY=$LLM_UNIVERSAL_PROXY_KEY",
            "GEMINI_API_KEY=$LLM_UNIVERSAL_PROXY_KEY",
            "`client_provider_key` mode, set these SDK keys to the real provider key",
        ):
            with self.subTest(clients_snippet=snippet):
                self.assertIn(snippet, clients_doc)

        for forbidden in ("fallback credential", "fallback_credential"):
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, admin_doc)
        self.assertIn("whether a provider key is configured", admin_doc)
        self.assertIn("provider_key_env presence", admin_doc)

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
        self.assertIn("provider_key_env: OPENAI_COMPATIBLE_API_KEY", container_config)
        self.assertIn(
            "provider_key_env: ANTHROPIC_COMPATIBLE_API_KEY",
            container_config,
        )
        self.assertNotIn("provider_key_inline", container_config)
        self.assertNotIn("MINIMAX", container_config.upper())
        self.assertNotIn("MINIMAX", compose.upper())
        self.assertNotIn("PRESET_", container_config)
        self.assertNotIn("PRESET_", compose)
        self.assertIn(
            "${OPENAI_COMPATIBLE_API_KEY:?set OPENAI_COMPATIBLE_API_KEY}",
            compose,
        )
        self.assertIn(
            "${ANTHROPIC_COMPATIBLE_API_KEY:?set ANTHROPIC_COMPATIBLE_API_KEY}",
            compose,
        )
        self.assertIn(
            "${LLM_UNIVERSAL_PROXY_ADMIN_TOKEN:?set LLM_UNIVERSAL_PROXY_ADMIN_TOKEN}",
            compose,
        )
        self.assertIn(
            "LLM_UNIVERSAL_PROXY_AUTH_MODE: proxy_key",
            compose,
        )
        self.assertIn("${LLM_UNIVERSAL_PROXY_KEY:?set LLM_UNIVERSAL_PROXY_KEY}", compose)
        self.assertNotRegex(container_config + compose, r"sk-[A-Za-z0-9]")
        self.assertIn("ghcr.io/agentsmith-project/llm-universal-proxy", container_doc)
        self.assertIn("LLM_UNIVERSAL_PROXY_AUTH_MODE=proxy_key", container_doc)
        self.assertIn("LLM_UNIVERSAL_PROXY_KEY", container_doc)
        self.assertIn("provider_key_env", container_doc)
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
