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
    "real-provider-smoke",
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
    missing = set(REQUIRED_RELEASE_GATE_NEEDS) - job_needs(job_block)
    if missing:
        failures.append(
            f"release workflow publishing job '{job_name}' is missing needs: "
            + ", ".join(sorted(missing))
        )

if failures:
    print("\n".join(failures))
    sys.exit(1)
PY
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

check_eq "Cargo.lock package version" "$LOCK_VERSION" "$VERSION"
check_eq "CHANGELOG latest version" "$CHANGELOG_VERSION" "$VERSION"

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
check_contains ".github/workflows/release.yml" "Real Provider Smoke"
check_contains ".github/workflows/release.yml" "environment: release-real-providers"
check_contains ".github/workflows/release.yml" 'GLM_APIKEY: ${{ secrets.GLM_APIKEY }}'
check_contains ".github/workflows/release.yml" "Validate protected real provider secrets"
check_contains ".github/workflows/release.yml" 'test -n "${GLM_APIKEY:-}"'
check_contains ".github/workflows/release.yml" "python3 scripts/real_endpoint_matrix.py --real-provider-smoke"
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
