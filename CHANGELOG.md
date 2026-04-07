# Changelog

## v0.2.3 - 2026-04-07

- Added a real-client E2E smoke script at `scripts/test_cli_clients.sh` that exercises the proxy with actual `codex` and `claude` CLI processes against mixed upstream aliases.
- Verified cross-protocol routing for Codex CLI over OpenAI Responses and Claude Code over Anthropic Messages, including Anthropic-compatible and OpenAI-compatible upstreams behind one proxy.
- Isolated Claude Code smoke tests from the user's global configuration by running them with a temporary `CLAUDE_CONFIG_DIR` and `--bare` mode, without modifying `~/.claude/settings.json`.
- Fixed the Claude smoke-test base URL handling so the client points at `/anthropic` and lets Claude append `/v1/messages` itself.
- Fixed Codex multi-turn smoke tests in temporary workspaces by adding `--skip-git-repo-check`.
- Marked the local `qwen-local` alias as an intentional skip for multi-turn code-edit tasks where the model is not reliable enough, while keeping its single-turn smoke coverage enabled.
- Documented the new smoke script and its constraints in `README.md`, `README_CN.md`, and the archived `HANDOFF.md`.

## v0.2.2 - 2026-03-20

- Fixed streaming request telemetry so downstream client disconnects are recorded as `cancelled` instead of being misreported as `500` errors.
- Added `cancelled` counts to the dashboard and per-upstream traffic panels, and excluded cancelled requests from error-rate accounting.
- Extended `usage` and `exchange` hook payloads with `cancelled_by_client`, `partial`, and `termination_reason` to make interrupted streaming requests observable without draining upstream generation.
- Added regression coverage for request tracker cancellation, hook stream-drop finalization, and early client disconnects against a slow SSE mock.

## v0.2.1 - 2026-03-20

- Added a protocol-namespaced API surface as the formal public interface: `/openai/v1/...`, `/anthropic/v1/...`, and `/google/v1beta/...`.
- Removed the legacy mixed `/v1/...` downstream routes to reduce code complexity and user-facing ambiguity.
- Added local model catalog endpoints under each protocol namespace, including list and retrieve operations.
- Added a terminal dashboard powered by `ratatui` / `crossterm` to expose runtime configuration, request activity, hook state, routing, and upstream traffic at a glance.
- Added isolated CLI examples for Codex CLI, Claude Code, and Gemini CLI, with temporary-home patterns that avoid modifying user-level configuration.
- Fixed Gemini-to-OpenAI request translation so Gemini-style `contents` blocks without an explicit `role` still preserve input text correctly.
- Expanded test coverage for namespaced routing, model catalog endpoints, dashboard plumbing, and Gemini request translation edge cases.

## v0.2.0 - 2026-03-20

- Added product-grade multi-upstream audit hooks with asynchronous `exchange` and `usage` delivery, request/response capture, normalized usage reporting, credential fingerprinting, and per-hook circuit breaker / pending-byte protections.
- Added upstream credential policy controls: `credential_actual`, `auth_policy`, and force-server credential enforcement.
- Added first-class Anthropic Messages client support at `POST /v1/messages`, including request detection, translation, and streaming.
- Improved upstream URL construction to support both versionless roots and versioned compatibility bases such as `.../api/paas/v4`.
- Expanded reasoning / thinking support across OpenAI Chat, OpenAI Responses, Anthropic Messages, and Gemini translation paths, including non-streaming response mapping and streaming lifecycle conversion.
- Added extensive regression coverage for reasoning/thinking, streaming, hooks, and protocol translation, plus a real upstream smoke script covering Anthropic-compatible and OpenAI-compatible services.

## v0.1.4 - 2026-03-20

- Fixed `run_with_listener()` to validate config before serving, which prevents invalid-config startup hangs and unblocks the `missing_upstreams_config_is_rejected` integration test in CI.
- Re-ran the release gate after the YAML/CLI configuration work: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --locked`.

## v0.1.3 - 2026-03-20

- Replaced environment-variable-based proxy configuration with a YAML config file loaded via `--config` / `-c`.
- Added named multi-upstream routing, local unique model aliases, and per-upstream fallback credential env support in the YAML schema.
- Standardized upstream base URLs on versionless roots and moved `/v1` / `/v1beta` path composition into the proxy.
- Removed the legacy single-upstream configuration path to reduce user-facing configuration ambiguity.
- Added strict tests for YAML parsing, config-file loading, CLI argument parsing, multi-upstream routing, alias resolution, fallback credentials, and startup failure when no upstreams are configured.

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
