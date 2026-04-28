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
CLI_WRAPPER_LIST_COMMAND="python3 scripts/real_cli_matrix.py --test basic --skip-slow --list-matrix"
CODEX_SCRIPTED_INTERACTIVE_GATE_COMMAND="PYTHONDONTWRITEBYTECODE=1 python3 -m unittest tests.test_interactive_cli.InteractiveCliTests.test_codex_wrapper_executes_scripted_interactive_two_turns_hermetically"
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

check_checkout_tag_visibility() {
    local is_shallow

    if ! is_shallow="$(git rev-parse --is-shallow-repository 2>/dev/null)"; then
        FAILURES+=("unable to determine checkout depth for release tag visibility; configure actions/checkout with fetch-depth: 0 so governance can see occupied tags")
        return
    fi

    case "$is_shallow" in
        false)
            ;;
        true)
            FAILURES+=("shallow checkout cannot safely enforce release tag visibility; configure actions/checkout with fetch-depth: 0 so governance can see occupied tags")
            ;;
        *)
            FAILURES+=("unexpected shallow checkout state '$is_shallow'; configure actions/checkout with fetch-depth: 0 so governance can see occupied tags")
            ;;
    esac
}

check_governance_checkout_fetch_depth() {
    local workflow="$1"
    local checkout_output

    if ! checkout_output="$(WORKFLOW_PATH="$workflow" python3 - <<'PY'
import os
import pathlib
import re
import sys


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


workflow_path = pathlib.Path(os.environ["WORKFLOW_PATH"])
workflow = workflow_path.read_text(encoding="utf-8")
job = workflow_jobs(workflow).get("governance", "")
failures = []

if not job:
    failures.append(f"{workflow_path} is missing the governance job")
else:
    checkout_step = workflow_step_block(job, "Checkout code")
    if not checkout_step:
        failures.append(f"{workflow_path} governance job is missing Checkout code step")
    else:
        if "uses: actions/checkout@v5" not in checkout_step:
            failures.append(
                f"{workflow_path} governance checkout must use actions/checkout@v5"
            )
        if not re.search(r"(?m)^        with:\s*$", checkout_step):
            failures.append(
                f"{workflow_path} governance checkout must define a with block"
            )
        if not re.search(r"(?m)^          fetch-depth:\s*0\s*$", checkout_step):
            failures.append(
                f"{workflow_path} governance checkout must set fetch-depth: 0 for tag visibility"
            )

if failures:
    print("\n".join(failures))
    sys.exit(1)
PY
    )"; then
        while IFS= read -r failure; do
            [[ -n "$failure" ]] && FAILURES+=("$failure")
        done <<< "$checkout_output"
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

if failures:
    print("\n".join(failures))
    sys.exit(1)
PY
}

scan_tracked_auth_contract() {
    python3 - <<'PY'
import pathlib
import re
import subprocess
import sys

data_prefix = "LLM_UNIVERSAL_PROXY_" + "DATA"
credential = "credential"
fallback = "fallback"
old_terms = (
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
parts = ("data", "token")
old_prose_re = re.compile(r"\b" + parts[0] + r"[-_\s]+" + parts[1] + r"s?\b", re.IGNORECASE)
required_terms = (
    "LLM_UNIVERSAL_PROXY_AUTH_MODE",
    "LLM_UNIVERSAL_PROXY_KEY",
    "provider_key_env",
    "client_provider_key",
    "proxy_key",
)

tracked_paths = subprocess.check_output(
    ["git", "ls-files", "README.md", "README_CN.md", "docs", "examples", "scripts"],
    text=True,
).splitlines()
failures = []
combined_active_text = []

for path_text in tracked_paths:
    path = pathlib.Path(path_text)
    if not path.is_file():
        continue
    if "protocol-baselines" in path.parts:
        continue

    text = path.read_text(encoding="utf-8", errors="replace")
    combined_active_text.append(text)
    for term in old_terms:
        index = text.find(term)
        if index == -1:
            continue
        line_no = text.count("\n", 0, index) + 1
        failures.append(f"{path_text}:{line_no}: legacy auth contract term is not allowed: {term}")
    for match in old_prose_re.finditer(text):
        line_no = text.count("\n", 0, match.start()) + 1
        failures.append(f"{path_text}:{line_no}: legacy auth prose is not allowed: {match.group(0)}")

active_text = "\n".join(combined_active_text)
for term in required_terms:
    if term not in active_text:
        failures.append(f"active docs/examples/scripts are missing new auth contract term: {term}")

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

check_cli_wrapper_matrix_contract() {
    check_contains ".github/workflows/release.yml" "CLI Wrapper Matrix"
    check_contains ".github/workflows/release.yml" "$CLI_WRAPPER_LIST_COMMAND"
    check_contains ".github/workflows/release.yml" "$CODEX_SCRIPTED_INTERACTIVE_GATE_COMMAND"
    check_contains "tests/test_interactive_cli.py" "test_codex_wrapper_executes_scripted_interactive_two_turns_hermetically"
    check_contains "tests/test_interactive_cli.py" "run_codex_proxy.sh"
    check_contains "tests/test_interactive_cli.py" "/openai/v1/responses"
    check_absent ".github/workflows/release.yml" "--test live"
    check_absent ".github/workflows/release.yml" "--mode real-provider-smoke"
}

if ! SECRET_SCAN_OUTPUT="$(scan_tracked_secret_risks)"; then
    while IFS= read -r failure; do
        [[ -n "$failure" ]] && FAILURES+=("$failure")
    done <<< "$SECRET_SCAN_OUTPUT"
fi

if ! AUTH_CONTRACT_OUTPUT="$(scan_tracked_auth_contract)"; then
    while IFS= read -r failure; do
        [[ -n "$failure" ]] && FAILURES+=("$failure")
    done <<< "$AUTH_CONTRACT_OUTPUT"
fi

if ! RELEASE_PUBLISH_GATE_OUTPUT="$(check_release_publish_jobs_need_ga_gates)"; then
    while IFS= read -r failure; do
        [[ -n "$failure" ]] && FAILURES+=("$failure")
    done <<< "$RELEASE_PUBLISH_GATE_OUTPUT"
fi

check_compatible_provider_smoke_invocation
check_cli_wrapper_matrix_contract

check_checkout_tag_visibility
check_governance_checkout_fetch_depth ".github/workflows/ci.yml"
check_governance_checkout_fetch_depth ".github/workflows/release.yml"

check_eq "Cargo.lock package version" "$LOCK_VERSION" "$VERSION"
check_eq "CHANGELOG latest version" "$CHANGELOG_VERSION" "$VERSION"
check_release_tag_identity

if [[ "${GITHUB_REF:-}" == refs/tags/* ]]; then
    check_eq "Git tag" "${GITHUB_REF}" "refs/tags/v${VERSION}"
fi

check_contains "Dockerfile" "ARG RUST_TOOLCHAIN=${TOOLCHAIN}"
check_contains "Dockerfile" "COPY rust-toolchain.toml"
check_contains "Dockerfile" "cargo build --locked --release"
check_contains "Dockerfile" 'org.opencontainers.image.source="https://github.com/agentsmith-project/llm-universal-proxy"'
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
check_contains "scripts/test_container_smoke.sh" "LLM_UNIVERSAL_PROXY_AUTH_MODE=proxy_key"
check_contains "scripts/test_container_smoke.sh" "LLM_UNIVERSAL_PROXY_KEY=\${PROXY_KEY}"
check_contains "scripts/test_container_smoke.sh" 'CONTAINER_SMOKE_UPSTREAM_API_KEY=container-smoke-provider-key'
check_contains "scripts/test_container_smoke.sh" 'EXPECTED_X_API_KEY = "container-smoke-provider-key"'
check_contains "scripts/test_container_smoke.sh" 'self.headers.get("x-api-key")'
check_contains "scripts/test_container_smoke.sh" "unexpected upstream authorization"
check_contains "scripts/test_container_smoke.sh" "/etc/llmup/config.yaml"
check_contains "scripts/test_container_smoke.sh" "host.docker.internal:host-gateway"
check_contains "scripts/test_container_smoke.sh" 'CONTAINER_PORT="8080"'
check_contains "scripts/test_container_smoke.sh" 'listen: 0.0.0.0:${CONTAINER_PORT}'
check_contains "scripts/test_container_smoke.sh" "format: anthropic"
check_contains "scripts/test_container_smoke.sh" "provider_key_env: CONTAINER_SMOKE_UPSTREAM_API_KEY"
check_contains "scripts/test_container_smoke.sh" '-p "${HOST}:${PROXY_PORT}:${CONTAINER_PORT}"'
check_contains "scripts/test_container_smoke.sh" "wait_for_container_healthy"
check_contains "scripts/test_binary_smoke.sh" 'LLM_UNIVERSAL_PROXY_AUTH_MODE=client_provider_key'
check_contains "scripts/test_binary_smoke.sh" 'SMOKE_PROVIDER_KEY="binary-smoke-provider-key"'
check_contains "scripts/test_binary_smoke.sh" 'python3 -u - "$MOCK_PORT_FILE" "$SMOKE_PROVIDER_KEY"'
check_contains "scripts/test_binary_smoke.sh" 'self.headers.get("x-api-key")'
check_contains "scripts/test_binary_smoke.sh" 'self.headers.get("Authorization")'
check_contains "scripts/test_binary_smoke.sh" 'unexpected upstream authorization'
check_contains "scripts/test_binary_smoke.sh" 'Authorization: Bearer ${SMOKE_PROVIDER_KEY}'
check_absent "scripts/test_binary_smoke.sh" "provider_key_env:"
check_absent "scripts/test_binary_smoke.sh" "LLM_UNIVERSAL_PROXY_KEY="
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
check_contains ".github/workflows/ci.yml" "bash scripts/supply_chain_audit.sh"
check_absent ".github/workflows/ci.yml" "cargo audit --locked"
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
check_contains ".github/workflows/release.yml" "bash scripts/supply_chain_audit.sh"
check_absent ".github/workflows/release.yml" "cargo audit --locked"
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
check_contains ".github/workflows/release.yml" "ghcr.io/agentsmith-project/llm-universal-proxy"
check_contains ".github/workflows/release.yml" "platforms: linux/amd64,linux/arm64"
check_contains ".github/workflows/release.yml" "push: true"
check_contains ".github/workflows/release.yml" '${{ env.GHCR_IMAGE }}:latest'
check_contains ".github/workflows/release.yml" 'DOCKER_BUILD_RECORD_UPLOAD: "false"'
check_contains ".github/workflows/release.yml" "pattern: llm-universal-proxy-*"
check_contains ".github/workflows/release.yml" "IMAGE=llm-universal-proxy:release-smoke bash scripts/test_container_smoke.sh"

check_contains "docs/README.md" "container.md"
check_contains "README.md" "docs/container.md"
check_contains "README_CN.md" "docs/container.md"
check_contains "README.md" "v0.2.22"
check_contains "README_CN.md" "v0.2.22"
check_contains "docs/container.md" "ghcr.io/agentsmith-project/llm-universal-proxy"
check_contains "docs/container.md" "ghcr.io/agentsmith-project/llm-universal-proxy:v0.2.22"
check_contains "docs/container.md" "ghcr.io/agentsmith-project/llm-universal-proxy@sha256:9dd52969dd30fad3a6472eb97ef5e6b231f9c51469e13e19f906c99f75ba8c89"
check_contains "docs/container.md" "docker pull ghcr.io/agentsmith-project/llm-universal-proxy:v0.2.22"
check_contains "docs/container.md" "Pin a release tag or digest for production"
check_contains "docs/container.md" 'Do not use `latest` for production pinning'
check_contains "docs/container.md" "docker login ghcr.io"
check_contains "docs/container.md" "personal access token (classic)"
check_contains "docs/container.md" "read:packages"
check_contains "docs/container.md" "GITHUB_USERNAME"
check_contains "docs/container.md" "If the package is public"
check_contains "docs/container.md" "unauthorized, 403, or package page appears 404"
check_absent "docs/container.md" "fine-grained personal access token"
check_absent "docs/container.md" '$GITHUB_ACTOR'
check_contains "docs/container.md" "LLM_UNIVERSAL_PROXY_ADMIN_TOKEN"
check_contains "docs/container.md" "LLM_UNIVERSAL_PROXY_AUTH_MODE=proxy_key"
check_contains "docs/container.md" "LLM_UNIVERSAL_PROXY_KEY"
check_contains "docs/container.md" "provider_key_env"
check_contains "docs/container.md" "Do not mount the local quickstart config unchanged for container service mode"
check_contains "docs/container.md" "Do not use the unedited example config for real provider requests"
check_absent "docs/clients.md" "OPENAI_API_KEY=dummy"
check_absent "docs/clients.md" "ANTHROPIC_API_KEY=dummy"
check_absent "docs/clients.md" "GEMINI_API_KEY=dummy"
check_contains "docs/clients.md" 'OPENAI_API_KEY=$LLM_UNIVERSAL_PROXY_KEY'
check_contains "docs/clients.md" 'ANTHROPIC_API_KEY=$LLM_UNIVERSAL_PROXY_KEY'
check_contains "docs/clients.md" 'GEMINI_API_KEY=$LLM_UNIVERSAL_PROXY_KEY'
check_contains "docs/clients.md" 'client_provider_key` mode, set these SDK keys to the real provider key'
check_contains "docs/admin-dynamic-config.md" "do not introduce a separate service key"
check_absent "docs/admin-dynamic-config.md" "fallback credential"
check_absent "docs/admin-dynamic-config.md" "fallback_credential"
check_contains "docs/admin-dynamic-config.md" "whether a provider key is configured"
check_contains "docs/admin-dynamic-config.md" "provider_key_env presence"
check_contains "examples/container-config.yaml" "listen: 0.0.0.0:8080"
check_contains "examples/container-config.yaml" "provider_key_env: OPENAI_COMPATIBLE_API_KEY"
check_contains "examples/container-config.yaml" "provider_key_env: ANTHROPIC_COMPATIBLE_API_KEY"
check_absent "examples/container-config.yaml" "MINIMAX"
check_absent "examples/container-config.yaml" "PRESET_"
check_contains "examples/docker-compose.yaml" 'OPENAI_COMPATIBLE_API_KEY: ${OPENAI_COMPATIBLE_API_KEY:?set OPENAI_COMPATIBLE_API_KEY}'
check_contains "examples/docker-compose.yaml" 'ANTHROPIC_COMPATIBLE_API_KEY: ${ANTHROPIC_COMPATIBLE_API_KEY:?set ANTHROPIC_COMPATIBLE_API_KEY}'
check_contains "examples/docker-compose.yaml" "ghcr.io/agentsmith-project/llm-universal-proxy:v0.2.22"
check_absent "examples/docker-compose.yaml" ":latest"
check_absent "examples/docker-compose.yaml" "MINIMAX"
check_absent "examples/docker-compose.yaml" "PRESET_"
check_contains "examples/docker-compose.yaml" 'LLM_UNIVERSAL_PROXY_ADMIN_TOKEN: ${LLM_UNIVERSAL_PROXY_ADMIN_TOKEN:?set LLM_UNIVERSAL_PROXY_ADMIN_TOKEN}'
check_contains "examples/docker-compose.yaml" 'LLM_UNIVERSAL_PROXY_AUTH_MODE: proxy_key'
check_contains "examples/docker-compose.yaml" 'LLM_UNIVERSAL_PROXY_KEY: ${LLM_UNIVERSAL_PROXY_KEY:?set LLM_UNIVERSAL_PROXY_KEY}'

check_contains "scripts/supply_chain_audit.sh" "cargo metadata --locked --format-version 1 --no-deps"
check_contains "scripts/supply_chain_audit.sh" "cargo audit"
check_absent "scripts/supply_chain_audit.sh" "cargo audit --locked"

if [[ ${#FAILURES[@]} -gt 0 ]]; then
    printf 'governance check failed:\n' >&2
    for failure in "${FAILURES[@]}"; do
        printf '  - %s\n' "$failure" >&2
    done
    exit 1
fi

printf 'governance check passed for version %s and toolchain %s\n' "$VERSION" "$TOOLCHAIN"
