#!/usr/bin/env bash
# Run all tests and generate a report.
# Usage: ./scripts/test-and-report.sh [--report-dir DIR]
# Default report dir: ./test-reports (created if missing).

set -e

REPORT_DIR="${PWD}/test-reports"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --report-dir)
      REPORT_DIR="$2"
      shift 2
      ;;
    *)
      echo "Unknown option: $1" >&2
      echo "Usage: $0 [--report-dir DIR]" >&2
      exit 1
      ;;
  esac
done

mkdir -p "$REPORT_DIR"
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
LOG_FILE="${REPORT_DIR}/test-${TIMESTAMP}.log"
REPORT_MD="${REPORT_DIR}/report-${TIMESTAMP}.md"
REPORT_JSON="${REPORT_DIR}/report-${TIMESTAMP}.json"

# Project root (parent of scripts/)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT_DIR"

# Use same env as Makefile (unset proxy etc.) and run all tests for full report
CARGO="${CARGO:-cargo}"
if [[ -x "$HOME/.cargo/bin/cargo" ]]; then
  CARGO="$HOME/.cargo/bin/cargo"
fi
export CARGO
CARGO_ENV="env -u RUSTC_WRAPPER -u http_proxy -u HTTP_PROXY -u https_proxy -u HTTPS_PROXY -u all_proxy -u ALL_PROXY"

echo "Running tests (log: $LOG_FILE) ..."
if $CARGO_ENV $CARGO test --no-fail-fast 2>&1 | tee "$LOG_FILE"; then
  TEST_EXIT=0
else
  TEST_EXIT=$?
fi

# Parse log: "running N tests" per crate, "test result: ok. X passed; Y failed"
TOTAL_PASSED=0
TOTAL_FAILED=0
FAILED_NAMES=()
CRATE_RESULTS=()

RE_RESULT_FAILED='test result:.* ([0-9]+) passed; ([0-9]+) failed'
RE_RESULT_OK='test result: ok\. ([0-9]+) passed'

while IFS= read -r line; do
  if [[ "$line" =~ running\ ([0-9]+)\ tests ]]; then
    CURRENT_CRATE_TESTS="${BASH_REMATCH[1]}"
  fi
  if [[ "$line" =~ $RE_RESULT_FAILED ]]; then
    p="${BASH_REMATCH[1]}"
    f="${BASH_REMATCH[2]}"
    TOTAL_PASSED=$((TOTAL_PASSED + p))
    TOTAL_FAILED=$((TOTAL_FAILED + f))
    CRATE_RESULTS+=("passed $p, failed $f")
  elif [[ "$line" =~ $RE_RESULT_OK ]]; then
    n="${BASH_REMATCH[1]}"
    TOTAL_PASSED=$((TOTAL_PASSED + n))
    CRATE_RESULTS+=("passed $n")
  fi
  if [[ "$line" == failures:* ]]; then
    in_failures=1
    continue
  fi
  if [[ -n "${in_failures:-}" ]] && [[ "$line" =~ ^[[:space:]]*(test[^[:space:]]*)[[:space:]]*$ ]]; then
    FAILED_NAMES+=("${BASH_REMATCH[1]}")
  fi
  if [[ "$line" =~ ^test\ result: ]]; then
    in_failures=
  fi
done < "$LOG_FILE"

TOTAL_TESTS=$((TOTAL_PASSED + TOTAL_FAILED))
if [[ $TOTAL_TESTS -eq 0 ]]; then
  TOTAL_TESTS=$(grep -c "test.*\.\.\." "$LOG_FILE" 2>/dev/null || echo "0")
fi

# Markdown report
{
  echo "# LLM Universal Proxy — Test Report"
  echo ""
  echo "- **Timestamp:** $TIMESTAMP"
  echo "- **Log file:** $(basename "$LOG_FILE")"
  echo "- **Result:** $([ $TEST_EXIT -eq 0 ] && echo "PASS" || echo "FAIL")"
  echo ""
  echo "## Summary"
  echo ""
  echo "| Metric | Value |"
  echo "|--------|-------|"
  echo "| Total passed | $TOTAL_PASSED |"
  echo "| Total failed | $TOTAL_FAILED |"
  echo "| Total tests  | $TOTAL_TESTS |"
  echo ""
  if [[ ${#FAILED_NAMES[@]} -gt 0 ]]; then
    echo "## Failed tests"
    echo ""
    for n in "${FAILED_NAMES[@]}"; do
      echo "- \`$n\`"
    done
    echo ""
  fi
  echo "## Log (tail)"
  echo ""
  echo "\`\`\`"
  tail -n 30 "$LOG_FILE"
  echo "\`\`\`"
} > "$REPORT_MD"

# JSON report (machine-readable)
{
  echo "{"
  echo "  \"timestamp\": \"$TIMESTAMP\","
  echo "  \"success\": $([ $TEST_EXIT -eq 0 ] && echo "true" || echo "false"),"
  echo "  \"passed\": $TOTAL_PASSED,"
  echo "  \"failed\": $TOTAL_FAILED,"
  echo "  \"total\": $TOTAL_TESTS,"
  echo "  \"log_file\": \"$LOG_FILE\","
  echo "  \"failed_tests\": ["
  for i in "${!FAILED_NAMES[@]}"; do
    echo -n "    \"${FAILED_NAMES[$i]}\""
    [[ $i -lt $((${#FAILED_NAMES[@]} - 1)) ]] && echo "," || echo ""
  done
  echo "  ]"
  echo "}"
} > "$REPORT_JSON"

# Symlink latest for easy access
ln -sf "$(basename "$REPORT_MD")" "${REPORT_DIR}/report-latest.md"
ln -sf "$(basename "$REPORT_JSON")" "${REPORT_DIR}/report-latest.json"
ln -sf "$(basename "$LOG_FILE")" "${REPORT_DIR}/test-latest.log"

echo ""
echo "Report:    $REPORT_MD"
echo "Latest:    ${REPORT_DIR}/report-latest.md"
echo "JSON:      $REPORT_JSON"
if [[ $TEST_EXIT -ne 0 ]]; then
  echo "Tests failed. See log and report for details."
  exit $TEST_EXIT
fi
echo "All tests passed."
