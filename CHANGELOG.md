# Changelog

## v0.1.2 - 2026-03-18

- Tracked `Cargo.lock` in git so `cargo check/test/clippy --locked` works in CI and release jobs.
- Updated GitHub Actions checkout steps to `actions/checkout@v5` to avoid the Node.js 20 deprecation path on hosted runners.
- Kept release gating on `cargo fmt`, `cargo clippy -D warnings`, and `cargo test` before building tagged artifacts.

## v0.1.1 - 2026-03-18

- Added `UPSTREAM_API_KEY` and `UPSTREAM_HEADERS` so the proxy can authenticate to upstreams and inject required protocol headers.
- Defaulted unspecified `stream` requests to non-streaming behavior to match OpenAI Chat Completions and Responses semantics.
- Corrected OpenAI Responses usage mapping and expanded streaming lifecycle conversion for content, reasoning, and function calls.
- Switched Google Gemini SSE upstream routing to `streamGenerateContent?alt=sse` and tightened related integration coverage.
- Hardened upstream format discovery by probing with auth and protocol headers instead of treating any non-`404` response as support.
- Added real-world documentation for running Codex CLI against Anthropic-compatible upstream services through the proxy.
- Release CI now gates on `cargo fmt`, `cargo clippy -D warnings`, and `cargo test` before building tagged artifacts.
