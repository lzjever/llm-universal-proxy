#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

meta() {
    python3 scripts/repo_metadata.py get "$1"
}

VERSION="$(meta version)"
LOCK_VERSION="$(meta lock_version)"
CHANGELOG_VERSION="$(meta changelog_version)"
TOOLCHAIN="$(meta rust_toolchain)"
TOOLCHAIN_ACTION_REF="$(meta rust_toolchain_action_ref)"
PYTHON_CONTRACT_TEST_COMMAND="PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p 'test*.py'"
OFFICIAL_PROVIDER_SECRET_ENVS=(
    "OPENAI_API_KEY"
    "ANTHROPIC_API_KEY"
    "GEMINI_API_KEY"
    "MINIMAX_API_KEY"
)
COMPAT_PROVIDER_SECRET_ENVS=(
    "COMPAT_PROVIDER_API_KEY"
    "COMPAT_OPENAI_API_KEY"
    "COMPAT_ANTHROPIC_API_KEY"
)
COMPAT_PROVIDER_VAR_ENVS=(
    "COMPAT_OPENAI_BASE_URL"
    "COMPAT_OPENAI_MODEL"
    "COMPAT_ANTHROPIC_BASE_URL"
    "COMPAT_ANTHROPIC_MODEL"
    "COMPAT_PROVIDER_LABEL"
)
COMPAT_PROVIDER_SMOKE_JSON="artifacts/compatible-provider-smoke.json"

FAILURES=()

check_eq() {
    local label="$1"
    local actual="$2"
    local expected="$3"
    if [[ "$actual" != "$expected" ]]; then
        FAILURES+=("$label mismatch: expected '$expected', got '$actual'")
    fi
}

check_contains() {
    local file="$1"
    local pattern="$2"
    if ! grep -Fq -- "$pattern" "$file"; then
        FAILURES+=("$file is missing: $pattern")
    fi
}

check_absent() {
    local file="$1"
    local pattern="$2"
    if grep -Fq -- "$pattern" "$file"; then
        FAILURES+=("$file still contains forbidden pattern: $pattern")
    fi
}

check_release_tag_identity() {
    local release_tag="refs/tags/v${VERSION}"
    local current_head
    local tag_head

    if ! current_head="$(git rev-parse HEAD 2>/dev/null)"; then
        FAILURES+=("unable to resolve current HEAD for release tag identity check")
        return
    fi

    if tag_head="$(git rev-parse --verify --quiet "${release_tag}^{commit}" 2>/dev/null)"; then
        if [[ "$tag_head" != "$current_head" ]]; then
            FAILURES+=("${release_tag} already points to ${tag_head}, not current HEAD ${current_head}; bump the package version instead of reusing or moving an existing tag")
        fi
    fi
}

scan_tracked_secret_risks() {
    python3 - <<'PY'
import pathlib
import re
import subprocess
import sys

provider_key_patterns = ("sk-cp-", "sk-ant-", "sk-proj-", "sk-live-", "sk-test-")
provider_key_re = re.compile(
    r"sk-(?:cp|ant|proj|live|test)-[A-Za-z0-9_-]{16,}|sk-[A-Za-z0-9_-]{32,}"
)
credential_actual_re = re.compile(r"^\s*credential_actual:\s*(?P<value>.+)")
dummy_credential_values = {
    "dummy",
    "dummy-key",
    "example",
    "example-key",
    "not-needed",
    "placeholder",
    "redacted",
    "test",
    "test-key",
}

# Keep the scan limited to git ls-files under tracked fixtures, docs, examples, and scripts.
tracked_paths = subprocess.check_output(
    ["git", "ls-files", "scripts/fixtures", "docs", "examples", "scripts"],
    text=True,
).splitlines()
failures = []

for path_text in tracked_paths:
    path = pathlib.Path(path_text)
    if not path.is_file():
        continue
    text = path.read_text(encoding="utf-8", errors="replace")
    for match in provider_key_re.finditer(text):
        line_no = text.count("\n", 0, match.start()) + 1
        failures.append(f"{path_text}:{line_no}: provider key pattern detected")
    for line_no, line in enumerate(text.splitlines(), start=1):
        match = credential_actual_re.search(line)
        if not match:
            continue
        value = match.group("value").split("#", 1)[0].strip().strip('"').strip("'")
        if not value or value.startswith(("{", "$")):
            continue
        if value not in dummy_credential_values:
            failures.append(
                f"{path_text}:{line_no}: non-dummy credential_actual is not allowed"
            )

if failures:
    print("\n".join(failures))
    sys.exit(1)
PY
}

check_release_publish_jobs_need_ga_gates() {
    python3 - <<'PY'
import pathlib
import re
import sys

REQUIRED_RELEASE_GATE_NEEDS = (
    "mock-endpoint-matrix",
    "cli-wrapper-matrix",
    "perf-gate",
    "compatible-provider-smoke",
    "supply-chain",
)
RELEASE_PUBLISH_JOB_MARKERS = (
    "push: true",
    "packages: write",
    "action-gh-release",
)


def workflow_jobs(text):
    matches = list(re.finditer(r"^  ([A-Za-z0-9_-]+):\n", text, re.MULTILINE))
    jobs = {}
    for index, match in enumerate(matches):
        name = match.group(1)
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        jobs[name] = text[match.start() : end]
    return jobs


def job_needs(job_block):
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


workflow_path = pathlib.Path(".github/workflows/release.yml")
jobs = workflow_jobs(workflow_path.read_text(encoding="utf-8"))
publish_jobs = {
    name: block
    for name, block in jobs.items()
    if any(marker in block for marker in RELEASE_PUBLISH_JOB_MARKERS)
}

failures = []
for expected_job in ("container", "release"):
    if expected_job not in publish_jobs:
        failures.append(f"release workflow publishing job not found: {expected_job}")

container = jobs.get("container", "")
if "push: true" not in container:
    failures.append("release workflow container job must remain a GHCR push boundary")
if "${{ env.GHCR_IMAGE }}:latest" not in container:
    failures.append("release workflow container job must govern the GHCR latest tag")

for job_name, job_block in sorted(publish_jobs.items()):
    needs = job_needs(job_block)
    missing = set(REQUIRED_RELEASE_GATE_NEEDS) - needs
    if missing:
        failures.append(
            f"release workflow publishing job '{job_name}' is missing needs: "
            + ", ".join(sorted(missing))
        )
    if "real-provider-smoke" in needs:
        failures.append(
            f"release workflow publishing job '{job_name}' still blocks on legacy real-provider-smoke"
        )

if failures:
    print("\n".join(failures))
    sys.exit(1)
PY
}

check_compatible_provider_smoke_invocation() {
    local workflow=".github/workflows/release.yml"
    local mode_invocation="python3 scripts/real_endpoint_matrix.py --mode compatible-provider-smoke --binary ./target/release/llm-universal-proxy --json-out artifacts/compatible-provider-smoke.json"
    local contract_output

    if ! grep -Fq -- "$mode_invocation" "$workflow"; then
        FAILURES+=("$workflow is missing an explicit compatible provider smoke invocation with --mode compatible-provider-smoke and --json-out ${COMPAT_PROVIDER_SMOKE_JSON}")
    fi

    check_contains "$workflow" "Upload compatible provider smoke result"
    check_contains "$workflow" "uses: actions/upload-artifact@v4"
    check_contains "$workflow" "name: compatible-provider-smoke"
    check_contains "$workflow" "path: ${COMPAT_PROVIDER_SMOKE_JSON}"
    check_contains "$workflow" "if-no-files-found: error"

    if ! contract_output="$(python3 - <<'PY'
import pathlib
import re
import sys

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


def workflow_jobs(text):
    matches = list(re.finditer(r"^  ([A-Za-z0-9_-]+):\n", text, re.MULTILINE))
    jobs = {}
    for index, match in enumerate(matches):
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        jobs[match.group(1)] = text[match.start() : end]
    return jobs


def workflow_step_block(text, step_name):
    marker = f"      - name: {step_name}"
    start = text.find(marker)
    if start == -1:
        return ""
    next_step = text.find("\n      - name: ", start + len(marker))
    if next_step == -1:
        return text[start:]
    return text[start:next_step]


workflow = pathlib.Path(".github/workflows/release.yml").read_text(encoding="utf-8")
job = workflow_jobs(workflow).get("compatible-provider-smoke", "")
failures = []

if not job:
    failures.append("release workflow is missing compatible-provider-smoke job")
else:
    if "environment: release-compatible-provider" not in job:
        failures.append("compatible-provider-smoke job must use release-compatible-provider environment")
    if "environment: release-real-providers" in job:
        failures.append("compatible-provider-smoke job must not use release-real-providers environment")

run_step = workflow_step_block(job, "Run compatible provider smoke")
if not run_step:
    failures.append("compatible-provider-smoke job is missing the script run step")
else:
    for secret_name in COMPAT_PROVIDER_SECRET_ENVS:
        expected = f"{secret_name}: ${{{{ secrets.{secret_name} }}}}"
        if expected not in run_step:
            failures.append(
                "compatible provider smoke must inject provider-neutral secret env into the script step: "
                + secret_name
            )
    for var_name in COMPAT_PROVIDER_VAR_ENVS:
        expected = f"{var_name}: ${{{{ vars.{var_name} }}}}"
        if expected not in run_step:
            failures.append(
                "compatible provider smoke must inject provider-neutral variable env into the script step: "
                + var_name
            )
    for secret_name in OFFICIAL_PROVIDER_SECRET_ENVS:
        forbidden = f"{secret_name}: ${{{{ secrets.{secret_name} }}}}"
        if forbidden in run_step:
            failures.append(
                "compatible provider smoke must not inject official provider quorum secret: "
                + secret_name
            )
    if "GLM_APIKEY" in run_step or "secrets.GLM_APIKEY" in job:
        failures.append(
            "compatible provider smoke must not inject legacy GLM_APIKEY into the script step"
        )

invocation_lines = [
    line.strip()
    for line in job.splitlines()
    if "python3 scripts/real_endpoint_matrix.py" in line
    and "--mode compatible-provider-smoke" in line
]
if not invocation_lines:
    failures.append("compatible provider smoke must invoke scripts/real_endpoint_matrix.py")
elif not any("--json-out" in line and COMPAT_PROVIDER_SMOKE_JSON in line for line in invocation_lines):
    failures.append("compatible provider smoke invocation must write compatible-provider-smoke JSON")
else:
    invocation_index = job.find(invocation_lines[0])
    before_invocation = job[:invocation_index]
    for forbidden in (
        "Validate protected real provider secrets",
        "is required in the release-compatible-provider environment",
        "exit 1",
    ):
        if forbidden in before_invocation:
            failures.append(
                "compatible provider smoke must not fail before real_endpoint_matrix.py can write JSON: "
                + forbidden
            )
    for env_name in (*COMPAT_PROVIDER_SECRET_ENVS, *COMPAT_PROVIDER_VAR_ENVS):
        shell_check = f'test -n "${{{env_name}:-}}"'
        if shell_check in before_invocation:
            failures.append(
                "missing compatible provider configuration checks must be delegated to real_endpoint_matrix.py: "
                + env_name
            )

upload_step = workflow_step_block(job, "Upload compatible provider smoke result")
if not upload_step:
    failures.append("compatible-provider-smoke job must upload the machine-readable JSON result")
else:
    for expected in (
        "if: ${{ always() }}",
        "uses: actions/upload-artifact@v4",
        "name: compatible-provider-smoke",
        f"path: {COMPAT_PROVIDER_SMOKE_JSON}",
        "if-no-files-found: error",
    ):
        if expected not in upload_step:
            failures.append(
                "compatible provider smoke upload artifact step is missing: " + expected
            )

if failures:
    print("\n".join(failures))
    sys.exit(1)
PY
    )"; then
        while IFS= read -r failure; do
            [[ -n "$failure" ]] && FAILURES+=("$failure")
        done <<< "$contract_output"
    fi
}

if ! SECRET_SCAN_OUTPUT="$(scan_tracked_secret_risks)"; then
    while IFS= read -r failure; do
        [[ -n "$failure" ]] && FAILURES+=("$failure")
    done <<< "$SECRET_SCAN_OUTPUT"
fi

if ! RELEASE_PUBLISH_GATE_OUTPUT="$(check_release_publish_jobs_need_ga_gates)"; then
    while IFS= read -r failure; do
        [[ -n "$failure" ]] && FAILURES+=("$failure")
    done <<< "$RELEASE_PUBLISH_GATE_OUTPUT"
fi

check_compatible_provider_smoke_invocation

check_eq "Cargo.lock package version" "$LOCK_VERSION" "$VERSION"
check_eq "CHANGELOG latest version" "$CHANGELOG_VERSION" "$VERSION"
check_release_tag_identity

if [[ "${GITHUB_REF:-}" == refs/tags/* ]]; then
    check_eq "Git tag" "${GITHUB_REF}" "refs/tags/v${VERSION}"
fi

check_contains "Dockerfile" "ARG RUST_TOOLCHAIN=${TOOLCHAIN}"
check_contains "Dockerfile" "COPY rust-toolchain.toml"
check_contains "Dockerfile" "cargo build --locked --release"
check_contains "Dockerfile" 'org.opencontainers.image.source="https://github.com/lzjever/llm-universal-proxy"'
check_contains "Dockerfile" "USER llmup:llmup"
check_contains "Dockerfile" 'CMD ["--config", "/etc/llmup/config.yaml"]'
check_contains "Dockerfile" "HEALTHCHECK"
check_contains "Dockerfile" "http://127.0.0.1:8080/health"

check_contains "Makefile" "build --locked --release"
check_contains "Makefile" "test --locked"
check_contains "Makefile" "check --locked"
check_contains "Makefile" "$PYTHON_CONTRACT_TEST_COMMAND"
check_contains "Makefile" "docker-build"
check_contains "Makefile" "docker-smoke"
check_contains "Makefile" "scripts/test_container_smoke.sh"

check_contains "scripts/test-and-report.sh" "test --locked --no-fail-fast"
check_contains "scripts/test-and-report.sh" "$PYTHON_CONTRACT_TEST_COMMAND"
check_contains "scripts/test_container_smoke.sh" "LLM_UNIVERSAL_PROXY_ADMIN_TOKEN=\${ADMIN_TOKEN}"
check_contains "scripts/test_container_smoke.sh" "/etc/llmup/config.yaml"
check_contains "scripts/test_container_smoke.sh" "host.docker.internal:host-gateway"
check_contains "scripts/test_container_smoke.sh" 'CONTAINER_PORT="8080"'
check_contains "scripts/test_container_smoke.sh" 'listen: 0.0.0.0:${CONTAINER_PORT}'
check_contains "scripts/test_container_smoke.sh" '-p "${HOST}:${PROXY_PORT}:${CONTAINER_PORT}"'
check_contains "scripts/test_container_smoke.sh" "wait_for_container_healthy"
check_contains "scripts/test_cli_clients.sh" "real_cli_matrix.py"
check_contains "scripts/real_cli_matrix.py" "def default_proxy_binary_path("
check_contains "scripts/real_cli_matrix.py" 'DEFAULT_RELEASE_PROXY_BINARY = REPO_ROOT / "target" / "release" / "llm-universal-proxy"'
check_contains "scripts/real_cli_matrix.py" 'DEFAULT_DEBUG_PROXY_BINARY = REPO_ROOT / "target" / "debug" / "llm-universal-proxy"'
check_contains "scripts/real_cli_matrix.py" 'DEFAULT_PROXY_BINARY = default_proxy_binary_path()'
check_contains "scripts/real_cli_matrix.py" 'default=str(default_proxy_binary_path())'
check_contains "scripts/interactive_cli.py" 'default=str(default_proxy_binary_path())'
check_absent "scripts/real_cli_matrix.py" 'DEFAULT_PROXY_BINARY = REPO_ROOT / "target" / "release" / "llm-universal-proxy"'
check_contains "scripts/test_compatibility.sh" "cargo build --locked --release"

check_contains ".github/workflows/ci.yml" "bash scripts/check-governance.sh"
check_contains ".github/workflows/ci.yml" "Secret Scan"
check_contains ".github/workflows/ci.yml" "id: repo_meta"
check_contains ".github/workflows/ci.yml" "run: python scripts/repo_metadata.py github-output"
check_absent ".github/workflows/ci.yml" '>> "$GITHUB_OUTPUT"'
check_contains ".github/workflows/ci.yml" 'toolchain: ${{ steps.repo_meta.outputs.rust_toolchain }}'
check_contains ".github/workflows/ci.yml" "dtolnay/rust-toolchain@${TOOLCHAIN_ACTION_REF}"
check_absent ".github/workflows/ci.yml" "dtolnay/rust-toolchain@master"
check_contains ".github/workflows/ci.yml" 'if: ${{ always() }}'
check_contains ".github/workflows/ci.yml" "$PYTHON_CONTRACT_TEST_COMMAND"
check_contains ".github/workflows/ci.yml" "bash scripts/test_binary_smoke.sh"
check_contains ".github/workflows/ci.yml" "Mock Endpoint Matrix"
check_contains ".github/workflows/ci.yml" "python3 scripts/real_endpoint_matrix.py --mock"
check_contains ".github/workflows/ci.yml" "Perf Gate"
check_contains ".github/workflows/ci.yml" "python3 scripts/real_endpoint_matrix.py --mock --perf"
check_contains ".github/workflows/ci.yml" "Supply Chain"
check_contains ".github/workflows/ci.yml" "cargo audit"
check_contains ".github/workflows/ci.yml" "Container Image Smoke"
check_contains ".github/workflows/ci.yml" "push: false"
check_contains ".github/workflows/ci.yml" "IMAGE=llm-universal-proxy:ci bash scripts/test_container_smoke.sh"

check_contains ".github/workflows/release.yml" "bash scripts/check-governance.sh"
check_contains ".github/workflows/release.yml" "Secret Scan"
check_contains ".github/workflows/release.yml" "id: repo_meta"
check_contains ".github/workflows/release.yml" "run: python scripts/repo_metadata.py github-output"
check_absent ".github/workflows/release.yml" '>> "$GITHUB_OUTPUT"'
check_contains ".github/workflows/release.yml" 'toolchain: ${{ steps.repo_meta.outputs.rust_toolchain }}'
check_contains ".github/workflows/release.yml" "dtolnay/rust-toolchain@${TOOLCHAIN_ACTION_REF}"
check_absent ".github/workflows/release.yml" "dtolnay/rust-toolchain@master"
check_contains ".github/workflows/release.yml" "Run Rust tests"
check_contains ".github/workflows/release.yml" "cargo test --locked --verbose"
check_contains ".github/workflows/release.yml" "Run Python contract tests"
check_contains ".github/workflows/release.yml" "$PYTHON_CONTRACT_TEST_COMMAND"
check_contains ".github/workflows/release.yml" 'if: ${{ always() }}'
check_contains ".github/workflows/release.yml" "bash scripts/test_binary_smoke.sh"
check_contains ".github/workflows/release.yml" "Mock Endpoint Matrix"
check_contains ".github/workflows/release.yml" "python3 scripts/real_endpoint_matrix.py --mock"
check_contains ".github/workflows/release.yml" "CLI Wrapper Matrix"
check_contains ".github/workflows/release.yml" "python3 scripts/real_cli_matrix.py --test basic --skip-slow --list-matrix"
check_contains ".github/workflows/release.yml" "Perf Gate"
check_contains ".github/workflows/release.yml" "python3 scripts/real_endpoint_matrix.py --mock --perf"
check_contains ".github/workflows/release.yml" "Supply Chain"
check_contains ".github/workflows/release.yml" "cargo audit"
check_contains ".github/workflows/release.yml" "anchore/sbom-action"
check_contains ".github/workflows/release.yml" "Compatible Provider Smoke"
check_contains ".github/workflows/release.yml" "environment: release-compatible-provider"
check_contains ".github/workflows/release.yml" 'COMPAT_PROVIDER_API_KEY: ${{ secrets.COMPAT_PROVIDER_API_KEY }}'
check_contains ".github/workflows/release.yml" 'COMPAT_OPENAI_API_KEY: ${{ secrets.COMPAT_OPENAI_API_KEY }}'
check_contains ".github/workflows/release.yml" 'COMPAT_ANTHROPIC_API_KEY: ${{ secrets.COMPAT_ANTHROPIC_API_KEY }}'
check_contains ".github/workflows/release.yml" 'COMPAT_OPENAI_BASE_URL: ${{ vars.COMPAT_OPENAI_BASE_URL }}'
check_contains ".github/workflows/release.yml" 'COMPAT_OPENAI_MODEL: ${{ vars.COMPAT_OPENAI_MODEL }}'
check_contains ".github/workflows/release.yml" 'COMPAT_ANTHROPIC_BASE_URL: ${{ vars.COMPAT_ANTHROPIC_BASE_URL }}'
check_contains ".github/workflows/release.yml" 'COMPAT_ANTHROPIC_MODEL: ${{ vars.COMPAT_ANTHROPIC_MODEL }}'
check_contains ".github/workflows/release.yml" 'COMPAT_PROVIDER_LABEL: ${{ vars.COMPAT_PROVIDER_LABEL }}'
check_absent ".github/workflows/release.yml" 'GLM_APIKEY: ${{ secrets.GLM_APIKEY }}'
check_absent ".github/workflows/release.yml" "Validate protected real provider secrets"
check_absent ".github/workflows/release.yml" 'test -n "${COMPAT_PROVIDER_API_KEY:-}"'
check_absent ".github/workflows/release.yml" 'test -n "${COMPAT_OPENAI_API_KEY:-}"'
check_absent ".github/workflows/release.yml" 'test -n "${COMPAT_ANTHROPIC_API_KEY:-}"'
check_absent ".github/workflows/release.yml" 'test -n "${COMPAT_OPENAI_BASE_URL:-}"'
check_absent ".github/workflows/release.yml" 'test -n "${COMPAT_OPENAI_MODEL:-}"'
check_absent ".github/workflows/release.yml" 'test -n "${COMPAT_ANTHROPIC_BASE_URL:-}"'
check_absent ".github/workflows/release.yml" 'test -n "${COMPAT_ANTHROPIC_MODEL:-}"'
check_absent ".github/workflows/release.yml" 'test -n "${COMPAT_PROVIDER_LABEL:-}"'
check_contains ".github/workflows/release.yml" "Upload compatible provider smoke result"
check_contains ".github/workflows/release.yml" "path: ${COMPAT_PROVIDER_SMOKE_JSON}"
check_contains ".github/workflows/release.yml" "path: artifacts/compatible-provider-smoke.json"
check_contains ".github/workflows/release.yml" "if-no-files-found: error"
check_contains ".github/workflows/release.yml" "ghcr.io/lzjever/llm-universal-proxy"
check_contains ".github/workflows/release.yml" "platforms: linux/amd64,linux/arm64"
check_contains ".github/workflows/release.yml" "push: true"
check_contains ".github/workflows/release.yml" '${{ env.GHCR_IMAGE }}:latest'
check_contains ".github/workflows/release.yml" 'DOCKER_BUILD_RECORD_UPLOAD: "false"'
check_contains ".github/workflows/release.yml" "pattern: llm-universal-proxy-*"
check_contains ".github/workflows/release.yml" "IMAGE=llm-universal-proxy:release-smoke bash scripts/test_container_smoke.sh"

check_contains "docs/README.md" "container.md"
check_contains "README.md" "docs/container.md"
check_contains "README_CN.md" "docs/container.md"
check_contains "docs/container.md" "ghcr.io/lzjever/llm-universal-proxy"
check_contains "docs/container.md" "LLM_UNIVERSAL_PROXY_ADMIN_TOKEN"
check_contains "docs/container.md" "Do not mount the local quickstart config unchanged for container service mode"
check_contains "docs/admin-dynamic-config.md" "do not introduce a separate service key"
check_contains "examples/container-config.yaml" "listen: 0.0.0.0:8080"
check_contains "examples/container-config.yaml" "credential_env: OPENAI_API_KEY"
check_contains "examples/docker-compose.yaml" 'OPENAI_API_KEY: ${OPENAI_API_KEY:?set OPENAI_API_KEY}'
check_contains "examples/docker-compose.yaml" 'LLM_UNIVERSAL_PROXY_ADMIN_TOKEN: ${LLM_UNIVERSAL_PROXY_ADMIN_TOKEN:?set LLM_UNIVERSAL_PROXY_ADMIN_TOKEN}'
check_absent "examples/container-config.yaml" "credential_actual"

if [[ ${#FAILURES[@]} -gt 0 ]]; then
    printf 'governance check failed:\n' >&2
    for failure in "${FAILURES[@]}"; do
        printf '  - %s\n' "$failure" >&2
    done
    exit 1
fi

printf 'governance check passed for version %s and toolchain %s\n' "$VERSION" "$TOOLCHAIN"
