# LLM Universal Proxy — build and test
#
# CARGO: use explicit path when available so that when run from Cursor's integrated
# terminal, rustup does not see the wrong proxy name ("cursor"). See:
# https://forum.cursor.com/t/rust-linux-error-unknown-proxy-name/19342
CARGO := $(if $(wildcard $(HOME)/.cargo/bin/cargo),$(HOME)/.cargo/bin/cargo,cargo)
# Unset RUSTC_WRAPPER so rustup does not reject it (Cursor may set it to "cursor").
# Unset proxy so cargo/git reach tuna mirror and crate hosts directly (avoids TLS/SSL errors).
CARGO_ENV := env -u RUSTC_WRAPPER -u http_proxy -u HTTP_PROXY -u https_proxy -u HTTPS_PROXY -u all_proxy -u ALL_PROXY
PYTHON_CONTRACT_TEST := PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p 'test*.py'

.PHONY: build test check run run-release test-report test-binary-smoke governance

build:
	$(CARGO_ENV) $(CARGO) build --locked --release

test:
	@status=0; \
	echo "$(CARGO_ENV) $(CARGO) test --locked"; \
	$(CARGO_ENV) $(CARGO) test --locked || status=$$?; \
	echo "$(PYTHON_CONTRACT_TEST)"; \
	$(PYTHON_CONTRACT_TEST) || status=$$?; \
	exit $$status

# Build the release binary and run the local binary smoke script.
test-binary-smoke: build
	bash scripts/test_binary_smoke.sh

# Run all tests (no-fail-fast), generate report in test-reports/
test-report:
	@bash scripts/test-and-report.sh --report-dir "$(CURDIR)/test-reports"

check:
	$(CARGO_ENV) $(CARGO) check --locked && $(CARGO_ENV) $(CARGO) test --locked

run:
	$(CARGO_ENV) $(CARGO) run --locked

run-release:
	$(CARGO_ENV) $(CARGO) run --locked --release

governance:
	@bash scripts/check-governance.sh
