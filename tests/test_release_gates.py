import importlib.util
import pathlib
import re
import sys
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
RELEASE_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "release.yml"
CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yml"
GOVERNANCE_SCRIPT = REPO_ROOT / "scripts" / "check-governance.sh"
ENDPOINT_MATRIX_SCRIPT = REPO_ROOT / "scripts" / "real_endpoint_matrix.py"
PYTHON_CONTRACT_TEST_COMMAND = (
    "PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p 'test*.py'"
)
REQUIRED_RELEASE_GATE_NEEDS = (
    "mock-endpoint-matrix",
    "cli-wrapper-matrix",
    "perf-gate",
    "real-provider-smoke",
    "supply-chain",
)
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


def release_workflow_jobs():
    text = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    matches = list(re.finditer(r"^  ([A-Za-z0-9_-]+):\n", text, re.MULTILINE))
    jobs = {}
    for index, match in enumerate(matches):
        job_name = match.group(1)
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        jobs[job_name] = text[match.start() : end]
    return jobs


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


class ReleaseGateWorkflowContractTests(unittest.TestCase):
    def read_text(self, relative_path: str) -> str:
        return (REPO_ROOT / relative_path).read_text(encoding="utf-8")

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
            "Perf Gate",
            "python3 scripts/real_endpoint_matrix.py --mock --perf",
            "Supply Chain",
            "cargo audit",
            "anchore/sbom-action",
            "Real Provider Smoke",
            "environment: release-real-providers",
            "GLM_APIKEY: ${{ secrets.GLM_APIKEY }}",
            "Validate protected real provider secrets",
            'test -n "${GLM_APIKEY:-}"',
            "python3 scripts/real_endpoint_matrix.py --real-provider-smoke",
        )

        for snippet in required_snippets:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, release)

        self.assertRegex(
            release,
            r"release:\n(?:.|\n)*needs: \[[^\]]*mock-endpoint-matrix[^\]]*"
            r"cli-wrapper-matrix[^\]]*perf-gate[^\]]*real-provider-smoke[^\]]*"
            r"supply-chain[^\]]*\]",
        )

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
                missing = set(REQUIRED_RELEASE_GATE_NEEDS) - job_needs(job_block)
                self.assertFalse(
                    missing,
                    f"{job_name} publishes release artifacts before GA gates: "
                    f"{', '.join(sorted(missing))}",
                )

    def test_ci_workflow_contains_local_mock_perf_and_supply_chain_gates(self):
        ci = CI_WORKFLOW.read_text(encoding="utf-8")

        for snippet in (
            "Mock Endpoint Matrix",
            "python3 scripts/real_endpoint_matrix.py --mock",
            "Perf Gate",
            "python3 scripts/real_endpoint_matrix.py --mock --perf",
            "Supply Chain",
            "cargo audit",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, ci)

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
            "--real-provider-smoke",
            "PERF_DEFAULT_P95_MS",
            "PERF_DEFAULT_TOTAL_MS",
            "build_mock_matrix_cases",
            '"status"',
            "GLM_APIKEY is required",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, script)

        self.assertNotIn("sk-proj-", script)
        self.assertNotIn("sk-ant-", script)
        self.assertNotIn("sk-cp-", script)

    def test_governance_locks_new_release_gate_contracts(self):
        governance = GOVERNANCE_SCRIPT.read_text(encoding="utf-8")

        for snippet in (
            "python3 scripts/real_endpoint_matrix.py --mock",
            "python3 scripts/real_cli_matrix.py --test basic --skip-slow --list-matrix",
            "python3 scripts/real_endpoint_matrix.py --mock --perf",
            "python3 scripts/real_endpoint_matrix.py --real-provider-smoke",
            "environment: release-real-providers",
            "REQUIRED_RELEASE_GATE_NEEDS",
            "check_release_publish_jobs_need_ga_gates",
            "cargo audit",
            "anchore/sbom-action",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, governance)

    def test_docs_record_local_and_protected_release_gates(self):
        ga_review = self.read_text("docs/ga-readiness-review.md")
        clients = self.read_text("docs/clients.md")
        container = self.read_text("docs/container.md")

        for snippet in (
            "GA release gates",
            "mock endpoint matrix",
            "perf gate",
            "real provider smoke",
            "release-real-providers",
        ):
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, ga_review)

        self.assertIn("CLI wrapper matrix", clients)
        self.assertIn("CLI wrapper matrix", container)
        self.assertIn("mock endpoint matrix", container)
        self.assertIn("perf gate", container)
        self.assertNotIn("not yet mandatory release gates", ga_review)
        self.assertNotIn("not mandatory", ga_review)
        self.assertNotIn("not mandatory", container)


if __name__ == "__main__":
    unittest.main()
