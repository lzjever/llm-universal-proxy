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
    if ! grep -Fq "$pattern" "$file"; then
        FAILURES+=("$file is missing: $pattern")
    fi
}

check_absent() {
    local file="$1"
    local pattern="$2"
    if grep -Fq "$pattern" "$file"; then
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

check_contains "Makefile" "build --locked --release"
check_contains "Makefile" "test --locked"
check_contains "Makefile" "check --locked"

check_contains "scripts/test-and-report.sh" "test --locked --no-fail-fast"
check_contains "scripts/test_cli_clients.sh" "real_cli_matrix.py"
check_contains "scripts/real_cli_matrix.py" 'DEFAULT_PROXY_BINARY = REPO_ROOT / "target" / "release" / "llm-universal-proxy"'
check_contains "scripts/test_compatibility.sh" "cargo build --locked --release"

check_contains ".github/workflows/ci.yml" "bash scripts/check-governance.sh"
check_contains ".github/workflows/ci.yml" "id: repo_meta"
check_contains ".github/workflows/ci.yml" "run: python scripts/repo_metadata.py github-output"
check_absent ".github/workflows/ci.yml" '>> "$GITHUB_OUTPUT"'
check_contains ".github/workflows/ci.yml" 'toolchain: ${{ steps.repo_meta.outputs.rust_toolchain }}'
check_contains ".github/workflows/ci.yml" "dtolnay/rust-toolchain@${TOOLCHAIN_ACTION_REF}"
check_absent ".github/workflows/ci.yml" "dtolnay/rust-toolchain@master"
check_contains ".github/workflows/ci.yml" "bash scripts/test_binary_smoke.sh"

check_contains ".github/workflows/release.yml" "bash scripts/check-governance.sh"
check_contains ".github/workflows/release.yml" "id: repo_meta"
check_contains ".github/workflows/release.yml" "run: python scripts/repo_metadata.py github-output"
check_absent ".github/workflows/release.yml" '>> "$GITHUB_OUTPUT"'
check_contains ".github/workflows/release.yml" 'toolchain: ${{ steps.repo_meta.outputs.rust_toolchain }}'
check_contains ".github/workflows/release.yml" "dtolnay/rust-toolchain@${TOOLCHAIN_ACTION_REF}"
check_absent ".github/workflows/release.yml" "dtolnay/rust-toolchain@master"
check_contains ".github/workflows/release.yml" "bash scripts/test_binary_smoke.sh"

if [[ ${#FAILURES[@]} -gt 0 ]]; then
    printf 'governance check failed:\n' >&2
    for failure in "${FAILURES[@]}"; do
        printf '  - %s\n' "$failure" >&2
    done
    exit 1
fi

printf 'governance check passed for version %s and toolchain %s\n' "$VERSION" "$TOOLCHAIN"
