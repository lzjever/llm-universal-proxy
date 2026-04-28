import importlib.util
import pathlib
import re
import sys
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
RELEASE_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "release.yml"
CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yml"
GOVERNANCE_SCRIPT = REPO_ROOT / "scripts" / "check-governance.sh"
SUPPLY_CHAIN_AUDIT_SCRIPT = REPO_ROOT / "scripts" / "supply_chain_audit.sh"
SUPPLY_CHAIN_AUDIT_COMMAND = "bash scripts/supply_chain_audit.sh"
LOCKFILE_INTEGRITY_COMMAND = "cargo metadata --locked --format-version 1 --no-deps"
ENDPOINT_MATRIX_SCRIPT = REPO_ROOT / "scripts" / "real_endpoint_matrix.py"
PYTHON_CONTRACT_TEST_COMMAND = (
    "PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p 'test*.py'"
)
CODEX_SCRIPTED_INTERACTIVE_GATE_COMMAND = (
    "PYTHONDONTWRITEBYTECODE=1 python3 -m unittest "
    "tests.test_interactive_cli.InteractiveCliTests."
    "test_codex_wrapper_executes_scripted_interactive_two_turns_hermetically"
)
REQUIRED_RELEASE_GATE_NEEDS = (
    "mock-endpoint-matrix",
    "cli-wrapper-matrix",
    "perf-gate",
    "compatible-provider-smoke",
    "supply-chain",
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
RELEASE_PUBLISH_JOB_MARKERS = (
    "push: true",
    "packages: write",
    "action-gh-release",
)


def load_endpoint_matrix_module():
    spec = importlib.util.spec_from_file_location(
        "real_endpoint_matrix_release_gate_contract",
        ENDPOINT_MATRIX_SCRIPT,
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def workflow_jobs(workflow_path: pathlib.Path):
    text = workflow_path.read_text(encoding="utf-8")
    matches = list(re.finditer(r"^  ([A-Za-z0-9_-]+):\n", text, re.MULTILINE))
    jobs = {}
    for index, match in enumerate(matches):
        job_name = match.group(1)
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        jobs[job_name] = text[match.start() : end]
    return jobs


def release_workflow_jobs():
    return workflow_jobs(RELEASE_WORKFLOW)


def job_needs(job_block: str):
    match = re.search(r"^    needs:\s*(?P<value>.*)$", job_block, re.MULTILINE)
    if not match:
        return set()

    value = match.group("value").strip()
    if value.startswith("[") and value.endswith("]"):
        return {
            item.strip().strip("\"'")
            for item in value.removeprefix("[").removesuffix("]").split(",")
            if item.strip()
        }
    if value:
        return {value.strip("\"'")}

    needs = set()
    for line in job_block[match.end() :].splitlines():
        if line.startswith("    ") and not line.startswith("      "):
            break
        item_match = re.match(r"^\s*-\s*([A-Za-z0-9_-]+)\s*$", line)
        if item_match:
            needs.add(item_match.group(1))
    return needs


def compatible_provider_smoke_invocation_lines(text: str):
    return [
        line.strip()
        for line in text.splitlines()
        if "python3 scripts/real_endpoint_matrix.py" in line
        and "--mode compatible-provider-smoke" in line
    ]


def workflow_step_block(text: str, step_name: str) -> str:
    marker = f"      - name: {step_name}"
    start = text.find(marker)
    if start == -1:
        return ""
    next_step = text.find("\n      - name: ", start + len(marker))
    if next_step == -1:
        return text[start:]
    return text[start:next_step]


class ReleaseGateWorkflowContractTests(unittest.TestCase):
    def read_text(self, relative_path: str) -> str:
        return (REPO_ROOT / relative_path).read_text(encoding="utf-8")

    def assert_has_compatible_provider_smoke_invocation(self, text: str):
        invocation_lines = compatible_provider_smoke_invocation_lines(text)
        self.assertTrue(
            invocation_lines,
            "compatible provider smoke must invoke real_endpoint_matrix.py with "
            "--mode compatible-provider-smoke",
        )
        self.assertTrue(
            any("--json-out" in line and COMPAT_PROVIDER_SMOKE_JSON in line for line in invocation_lines),
            "compatible provider smoke must emit the machine-readable JSON artifact",
        )

    def test_release_workflow_contains_ga_release_gates(self):
        release = RELEASE_WORKFLOW.read_text(encoding="utf-8")

        required_snippets = (
            "Run Rust tests",
            "cargo test --locked --verbose",
            "Run Python contract tests",
            PYTHON_CONTRACT_TEST_COMMAND,
            "Check version, toolchain, and Secret Scan governance",
            "bash scripts/check-governance.sh",
            "Run container smoke",
            "bash scripts/test_container_smoke.sh",
            "Mock Endpoint Matrix",
            "python3 scripts/real_endpoint_matrix.py --mock",
            "CLI Wrapper Matrix",
            "python3 scripts/real_cli_matrix.py --test basic --skip-slow --list-matrix",
            CODEX_SCRIPTED_INTERACTIVE_GATE_COMMAND,
            "Perf Gate",
            "python3 scripts/real_endpoint_matrix.py --mock --perf",
            "Supply Chain",
            SUPPLY_CHAIN_AUDIT_COMMAND,
            "anchore/sbom-action",
            "Compatible Provider Smoke",
            "environment: release-compatible-provider",
            COMPAT_PROVIDER_SMOKE_JSON,
        )

        for snippet in required_snippets:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, release)
        self.assert_has_compatible_provider_smoke_invocation(release)

        self.assertRegex(
            release,
            r"release:\n(?:.|\n)*needs: \[[^\]]*mock-endpoint-matrix[^\]]*"
            r"cli-wrapper-matrix[^\]]*perf-gate[^\]]*compatible-provider-smoke[^\]]*"
            r"supply-chain[^\]]*\]",
        )

    def test_release_cli_wrapper_matrix_runs_structure_and_hermetic_interactive_gates(self):
        jobs = release_workflow_jobs()
        job = jobs.get("cli-wrapper-matrix", "")
        self.assertTrue(job, "release workflow must define cli-wrapper-matrix")

        run_step = workflow_step_block(job, "Run CLI wrapper matrix")
        self.assertTrue(run_step, "cli-wrapper-matrix must keep a script run step")
        self.assertIn(CODEX_SCRIPTED_INTERACTIVE_GATE_COMMAND, run_step)
        self.assertIn(
            "python3 scripts/real_cli_matrix.py --test basic --skip-slow --list-matrix",
            run_step,
        )
        self.assertIn("cli-wrapper-matrix.txt", run_step)
        self.assertNotIn("--mode real-provider-smoke", run_step)
        self.assertNotIn("--test live", run_step)

    def test_governance_checkout_fetches_full_history_for_release_tag_visibility(self):
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

    def test_release_compatible_provider_smoke_delegates_missing_secret_json_to_script(self):
        jobs = release_workflow_jobs()
        job = jobs.get("compatible-provider-smoke", "")
        self.assertTrue(job, "release workflow must define compatible-provider-smoke")

        run_step = workflow_step_block(job, "Run compatible provider smoke")
        self.assertTrue(run_step, "compatible provider smoke must have a script run step")
        for secret_name in COMPAT_PROVIDER_SECRET_ENVS:
            with self.subTest(secret=secret_name):
                self.assertIn(f"{secret_name}: ${{{{ secrets.{secret_name} }}}}", run_step)
        for var_name in COMPAT_PROVIDER_VAR_ENVS:
            with self.subTest(var=var_name):
                self.assertIn(f"{var_name}: ${{{{ vars.{var_name} }}}}", run_step)
        for secret_name in OFFICIAL_PROVIDER_SECRET_ENVS:
            with self.subTest(no_official_secret=secret_name):
                self.assertNotIn(f"{secret_name}: ${{{{ secrets.{secret_name} }}}}", job)
        self.assertNotIn("GLM_APIKEY", run_step)
        self.assertNotIn("secrets.GLM_APIKEY", job)

        invocation_lines = compatible_provider_smoke_invocation_lines(job)
        self.assertTrue(invocation_lines)
        invocation_index = job.find(invocation_lines[0])
        self.assertGreaterEqual(invocation_index, 0)
        before_invocation = job[:invocation_index]

        self.assertNotIn("Validate protected real provider secrets", before_invocation)
        self.assertNotIn("is required in the release-compatible-provider environment", before_invocation)
        self.assertNotIn("exit 1", before_invocation)
        for env_name in (*COMPAT_PROVIDER_SECRET_ENVS, *COMPAT_PROVIDER_VAR_ENVS):
            with self.subTest(no_preflight=env_name):
                self.assertNotIn(f'test -n "${{{env_name}:-}}"', before_invocation)

        self.assert_has_compatible_provider_smoke_invocation(job)

    def test_release_compatible_provider_smoke_uploads_json_artifact_always(self):
        jobs = release_workflow_jobs()
        job = jobs.get("compatible-provider-smoke", "")
        self.assertTrue(job, "release workflow must define compatible-provider-smoke")

        upload_step = workflow_step_block(job, "Upload compatible provider smoke result")
        self.assertTrue(upload_step, "compatible provider smoke JSON artifact must be uploaded")
        self.assertIn("Upload compatible provider smoke result", job)
        self.assertIn('if: ${{ always() }}', upload_step)
        self.assertIn("uses: actions/upload-artifact@v4", upload_step)
        self.assertIn("name: compatible-provider-smoke", upload_step)
        self.assertIn(f"path: {COMPAT_PROVIDER_SMOKE_JSON}", upload_step)
        self.assertIn("if-no-files-found: error", upload_step)

    def test_release_publish_jobs_need_ga_gates_before_publishing(self):
        jobs = release_workflow_jobs()
        publish_jobs = {
            name: block
            for name, block in jobs.items()
            if any(marker in block for marker in RELEASE_PUBLISH_JOB_MARKERS)
        }

        self.assertIn("container", publish_jobs)
        self.assertIn("release", publish_jobs)
        self.assertIn("${{ env.GHCR_IMAGE }}:latest", publish_jobs["container"])

        for job_name, job_block in publish_jobs.items():
            with self.subTest(job=job_name):
                needs = job_needs(job_block)
                missing = set(REQUIRED_RELEASE_GATE_NEEDS) - needs
                self.assertFalse(
                    missing,
                    f"{job_name} publishes release artifacts before GA gates: "
                    f"{', '.join(sorted(missing))}",
                )
                self.assertNotIn(
                    "real-provider-smoke",
                    needs,
                    f"{job_name} must not block GA release on the legacy four-provider smoke",
                )

    def test_release_container_job_publishes_ref_version_and_latest_tags(self):
        jobs = release_workflow_jobs()
        container = jobs.get("container", "")
        self.assertTrue(container, "release workflow must define container job")

        push_step = workflow_step_block(container, "Build and push multi-arch image")
        self.assertTrue(push_step, "container job must keep a multi-arch push step")
        for snippet in (
            "${{ env.GHCR_IMAGE }}:${{ github.ref_name }}",
            "${{ env.GHCR_IMAGE }}:${{ steps.repo_meta.outputs.version }}",
            "${{ env.GHCR_IMAGE }}:latest",
            "VERSION=${{ steps.repo_meta.outputs.version }}",
            "org.opencontainers.image.version=${{ steps.repo_meta.outputs.version }}",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, push_step)

    def test_ci_workflow_contains_local_mock_perf_and_supply_chain_gates(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")

        for snippet in (
            "Mock Endpoint Matrix",
            "python3 scripts/real_endpoint_matrix.py --mock",
            "Perf Gate",
            "python3 scripts/real_endpoint_matrix.py --mock --perf",
            "Supply Chain",
            SUPPLY_CHAIN_AUDIT_COMMAND,
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, ci)

    def test_supply_chain_audit_gate_has_central_contract(self):
        self.assertTrue(
            SUPPLY_CHAIN_AUDIT_SCRIPT.exists(),
            "supply-chain audit must have one repo-local entrypoint shared by CI and release",
        )
        audit_script = SUPPLY_CHAIN_AUDIT_SCRIPT.read_text(encoding="utf-8")

        for workflow_path in (CI_WORKFLOW, RELEASE_WORKFLOW):
            workflow = workflow_path.read_text(encoding="utf-8")
            jobs = workflow_jobs(workflow_path)
            job = jobs.get("supply-chain", "")
            with self.subTest(workflow=workflow_path.name):
                self.assertTrue(job, "workflow must define a supply-chain job")
                self.assertIn("Install cargo-audit", job)
                self.assertIn(SUPPLY_CHAIN_AUDIT_COMMAND, job)
                self.assertNotIn("cargo audit --locked", workflow)

        release_supply_chain = workflow_jobs(RELEASE_WORKFLOW).get("supply-chain", "")
        self.assertIn("anchore/sbom-action", release_supply_chain)
        self.assertIn("Upload SBOM", release_supply_chain)

        self.assertIn(LOCKFILE_INTEGRITY_COMMAND, audit_script)
        self.assertIn("cargo audit", audit_script)
        self.assertNotIn("cargo audit --locked", audit_script)

    def test_endpoint_mock_matrix_covers_public_surface_minimal_paths(self):
        module = load_endpoint_matrix_module()
        cases = module.build_mock_matrix_cases()

        expected_surfaces = {
            "openai_chat",
            "openai_responses",
            "anthropic_messages",
            "gemini_generate_content",
        }
        expected_modes = {"unary", "stream", "tool", "error"}
        actual_pairs = {(case.surface, case.mode) for case in cases}

        self.assertEqual({case.surface for case in cases}, expected_surfaces)
        self.assertEqual({case.mode for case in cases}, expected_modes)
        for surface in expected_surfaces:
            for mode in expected_modes:
                with self.subTest(surface=surface, mode=mode):
                    self.assertIn((surface, mode), actual_pairs)

        case_ids = [case.case_id for case in cases]
        self.assertEqual(len(case_ids), len(set(case_ids)))

    def test_endpoint_matrix_cli_contract_is_machine_readable_and_secret_free(self):
        script = ENDPOINT_MATRIX_SCRIPT.read_text(encoding="utf-8")

        for snippet in (
            "--mock",
            "--perf",
            "--json-out",
            "--mode",
            "PERF_DEFAULT_P95_MS",
            "PERF_DEFAULT_TOTAL_MS",
            "build_mock_matrix_cases",
            '"status"',
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, script)

        self.assertTrue(
            "--mode" in script,
            "real endpoint matrix must expose explicit CLI modes",
        )

        self.assertNotIn("sk-proj-", script)
        self.assertNotIn("sk-ant-", script)
        self.assertNotIn("sk-cp-", script)

    def test_governance_locks_new_release_gate_contracts(self):
        governance = GOVERNANCE_SCRIPT.read_text(encoding="utf-8")

        for snippet in (
            "python3 scripts/real_endpoint_matrix.py --mock",
            "python3 scripts/real_cli_matrix.py --test basic --skip-slow --list-matrix",
            "python3 scripts/real_endpoint_matrix.py --mock --perf",
            "environment: release-compatible-provider",
            "COMPAT_PROVIDER_SECRET_ENVS",
            "COMPAT_PROVIDER_VAR_ENVS",
            "COMPAT_PROVIDER_SMOKE_JSON",
            "check_compatible_provider_smoke_invocation",
            "if-no-files-found: error",
            "REQUIRED_RELEASE_GATE_NEEDS",
            "check_release_publish_jobs_need_ga_gates",
            "check_release_tag_identity",
            "refs/tags/v${VERSION}",
            "git rev-parse --verify --quiet",
            "check_governance_checkout_fetch_depth",
            "git rev-parse --is-shallow-repository",
            "fetch-depth: 0",
            "tag visibility",
            SUPPLY_CHAIN_AUDIT_COMMAND,
            LOCKFILE_INTEGRITY_COMMAND,
            "cargo audit --locked",
            "anchore/sbom-action",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, governance)
        self.assert_has_compatible_provider_smoke_invocation(governance)

    def test_docs_record_local_and_protected_release_gates(self):
        ga_review = self.read_text("docs/ga-readiness-review.md")
        clients = self.read_text("docs/clients.md")
        container = self.read_text("docs/container.md")

        for snippet in (
            "GA release gates",
            "mock endpoint matrix",
            "perf gate",
            "compatible provider smoke",
            "hermetic scripted interactive Codex wrapper gate",
            "not a full live multi-client/provider matrix",
            "release-compatible-provider",
            "portable-core production GA",
            "same-provider native passthrough",
            "cross-provider documented compatibility/fail-closed",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, ga_review)

        self.assertIn("CLI wrapper matrix", clients)
        self.assertIn("hermetic scripted interactive Codex wrapper gate", clients)
        self.assertIn("not a full live multi-client/provider matrix", clients)
        self.assertIn("MiniMax is an OpenAI-compatible lane", clients)
        self.assertIn("CLI wrapper matrix", container)
        self.assertIn("structure gate", container)
        self.assertIn("hermetic scripted interactive Codex wrapper gate", container)
        self.assertIn("not a full live multi-client/provider matrix", container)
        self.assertIn("mock endpoint matrix", container)
        self.assertIn("perf gate", container)
        self.assertIn("compatible-provider-smoke.json", container)
        self.assertNotIn("not yet mandatory release gates", ga_review)
        self.assertNotIn("not mandatory", ga_review)
        self.assertNotIn("not mandatory", container)

    def test_docs_record_opaque_reasoning_and_compaction_degrade_contract(self):
        docs = {
            "compatibility": self.read_text("docs/protocol-compatibility-matrix.md"),
            "reasoning": self.read_text("docs/protocol-baselines/capabilities/reasoning.md"),
            "state": self.read_text("docs/protocol-baselines/capabilities/state-continuity.md"),
            "field_mapping": self.read_text(
                "docs/protocol-baselines/matrices/field-mapping-matrix.md"
            ),
            "responses": self.read_text("docs/protocol-baselines/openai-responses.md"),
            "ga_review": self.read_text("docs/ga-readiness-review.md"),
        }

        required_by_doc = {
            "compatibility": (
                "default/max_compat",
                "visible summary",
                "opaque-only",
                "same-provider/native passthrough",
            ),
            "reasoning": (
                "reasoning.encrypted_content",
                "default/max_compat",
                "visible summary",
                "strict and balanced",
            ),
            "state": (
                "context_management",
                "request-side compaction input",
                "default/max_compat",
                "opaque-only",
                "native Responses passthrough",
            ),
            "field_mapping": (
                "Reasoning opaque state",
                "default/max_compat",
                "Compaction",
                "opaque-only compaction",
            ),
            "responses": (
                "context_management",
                "request-side compaction",
                "visible portable transcript",
                "Opaque-only compaction input",
                "Native OpenAI Responses passthrough",
            ),
            "ga_review": (
                "default/max_compat",
                "visible summary",
                "strict/balanced",
                "opaque-only",
                "same-provider/native passthrough",
            ),
        }

        for doc_name, snippets in required_by_doc.items():
            text = docs[doc_name].casefold()
            for snippet in snippets:
                with self.subTest(doc=doc_name, snippet=snippet):
                    self.assertIn(snippet.casefold(), text)


if __name__ == "__main__":
    unittest.main()
