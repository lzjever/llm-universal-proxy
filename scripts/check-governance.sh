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
check_contains ".github/workflows/ci.yml" "id: repo_meta"
check_contains ".github/workflows/ci.yml" "run: python scripts/repo_metadata.py github-output"
check_absent ".github/workflows/ci.yml" '>> "$GITHUB_OUTPUT"'
check_contains ".github/workflows/ci.yml" 'toolchain: ${{ steps.repo_meta.outputs.rust_toolchain }}'
check_contains ".github/workflows/ci.yml" "dtolnay/rust-toolchain@${TOOLCHAIN_ACTION_REF}"
check_absent ".github/workflows/ci.yml" "dtolnay/rust-toolchain@master"
check_contains ".github/workflows/ci.yml" 'if: ${{ always() }}'
check_contains ".github/workflows/ci.yml" "$PYTHON_CONTRACT_TEST_COMMAND"
check_contains ".github/workflows/ci.yml" "bash scripts/test_binary_smoke.sh"
check_contains ".github/workflows/ci.yml" "Container Image Smoke"
check_contains ".github/workflows/ci.yml" "push: false"
check_contains ".github/workflows/ci.yml" "IMAGE=llm-universal-proxy:ci bash scripts/test_container_smoke.sh"

check_contains ".github/workflows/release.yml" "bash scripts/check-governance.sh"
check_contains ".github/workflows/release.yml" "id: repo_meta"
check_contains ".github/workflows/release.yml" "run: python scripts/repo_metadata.py github-output"
check_absent ".github/workflows/release.yml" '>> "$GITHUB_OUTPUT"'
check_contains ".github/workflows/release.yml" 'toolchain: ${{ steps.repo_meta.outputs.rust_toolchain }}'
check_contains ".github/workflows/release.yml" "dtolnay/rust-toolchain@${TOOLCHAIN_ACTION_REF}"
check_absent ".github/workflows/release.yml" "dtolnay/rust-toolchain@master"
check_contains ".github/workflows/release.yml" "bash scripts/test_binary_smoke.sh"
check_contains ".github/workflows/release.yml" "ghcr.io/lzjever/llm-universal-proxy"
check_contains ".github/workflows/release.yml" "platforms: linux/amd64,linux/arm64"
check_contains ".github/workflows/release.yml" "push: true"
check_contains ".github/workflows/release.yml" '${{ env.GHCR_IMAGE }}:latest'
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
