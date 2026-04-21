# Changelog

## Unreleased

## v0.2.8 - 2026-04-20

- Switched the default compatibility posture to `max_compat`, so translated agent-facing paths now prefer the safer client-usable bridge behavior by default while still keeping `strict` and `balanced` available for tighter boundary control.
- Preserved stable public tool identity across translated live paths: client-visible and model-visible surfaces now keep original tool names such as `apply_patch` instead of exposing proxy-private `__llmup_custom__*` transport artifacts, with matching smoke and regression coverage around that contract.
- Stopped trusting client-supplied `_llmup_tool_bridge_context` payloads at proxy ingress; the bridge context is now enforced as an internal-only request-scoped field so external callers cannot spoof custom/freeform tool decoding state.
- Synced Codex-facing model surface metadata from the same source-of-truth model surface used by the proxy, so generated catalogs and wrapper defaults stay aligned on text-only modalities, search support, and `apply_patch` tool surfacing instead of drifting through parallel metadata paths.
- Fixed OpenAI Responses tool-call sink finalization so pending translated tool calls emit consistent `done` / `response.output_item.done` events with the correct payload and proxied tool metadata during flush and teardown paths.
- Removed the dead Claude-to-OpenAI single-argument conversion wrapper after the last test-only caller moved to the real implementation, clearing the remaining Rust `dead_code` warning without changing translation behavior.

## v0.2.7 - 2026-04-20

- Routed dashboard-mode runtime logs into an in-memory TUI log buffer and rendered them inside a new `Runtime Logs` panel, so live `warn!` / `info!` / `error!` output no longer overwrites the alternate-screen dashboard while the proxy is serving traffic.
- Rebalanced the dashboard activity area so recent latency is shown as a compact trend sparkline and recent-request tables stay readable across tighter terminal heights while sharing space cleanly with live runtime logs.
- Fixed OpenAI Responses sink tool-stream finalization so pending tool calls emit the expected done events promptly on tool-call finishes instead of leaving completion semantics to later terminal cleanup paths.
- Improved the Codex interactive wrapper flow by generating explicit `apply_patch_tool_type: freeform` metadata in temporary catalogs, disabling `view_image` for text-only lanes, and refreshing the English / Chinese manual-testing docs around those safer defaults.

## v0.2.6 - 2026-04-19

- Propagated configured `max_output_tokens` defaults from resolved model limits into request translation when clients omit an explicit output cap, so Anthropic, OpenAI Completions, and sibling target protocols no longer silently fall back to incorrect hard-coded defaults such as `4096`.
- Updated the generated Codex custom model catalog to follow the current real schema and to compute default `auto_compact_token_limit` from available input budget: `0.85 * (context_window - max_output_tokens)` when both limits are known, while keeping the older `0.85 * context_window` fallback only when no output budget is available.
- Hardened long-session tool replay boundaries by marking incomplete or truncated tool calls as non-replayable and intentionally degrading later replay / bridge paths instead of pretending the partial call is still valid structured history.
- Fixed custom-tool bridge trust transfer so representation rewrites re-sign non-replayable markers after verification instead of literally copying a marker that no longer matches the bridged `name` / raw payload.
- Verified fixes against known failures observed on the Anthropic and OpenAI-completions Codex yolo mainlines, including long-session translation, replay, and compaction regressions, without claiming cross-provider full fidelity for every long-horizon path.
- Tightened cross-protocol compatibility handling so more non-portable request and typed-item semantics fail closed or surface explicit compatibility warnings instead of silently widening behavior across OpenAI, Responses, Anthropic, and Gemini paths.
- Hardened runtime-chain observability during streaming teardown: hooks and `debug_trace` now preserve protocol-level terminal outcomes through disconnects and error endings, while bounded background capture paths surface explicit truncation / overflow accounting instead of silently dropping data or accumulating unbounded exchange payloads.
- Expanded `debug_trace` coverage for Google / Gemini client-format streaming so traces record protocol-level terminal, error, text, and tool-call summaries rather than only the final transport outcome.
- Normalized Gemini CLI matrix runner workspace handling so smoke and long-horizon cases use stable absolute `--include-directories` / isolated runner-state paths even when reports are launched from relative directories.

## v0.2.5 - 2026-04-17

- Added `scripts/real_cli_matrix.py` as the reusable real-client CLI matrix runner for repeatable end-to-end proxy testing across real `codex`, `claude`, and `gemini` processes, including stable matrix listing and case targeting.
- Kept `scripts/test_cli_clients.sh` as a compatibility shim for older local flows and wrappers by forwarding it directly to the Python runner.
- Isolated Codex, Claude Code, and Gemini runs with runner-managed home/config/cache state and per-client environment wiring, while reusing a runner-managed Gemini bootstrap home instead of the user's normal profile.
- Added timestamped report artifacts under `test-reports/cli-matrix/`, including JSON and Markdown summaries, per-case logs, captured workspaces, and a `latest` symlink for quick inspection.
- Tightened long-horizon verification so the Python bugfix fixture must both repair the `calc.py` implementation and preserve the expected `main.py` behavior, rejecting comment-only or non-functional edits.
- Kept `qwen-local` as optional coverage enabled only when local env is configured, with the default matrix limiting it to smoke coverage and excluding long-horizon code-edit cases.

## v0.2.4 - 2026-04-07

- Added `rust-toolchain.toml` as the repository's pinned Rust toolchain source and wired CI / release jobs to install Rust from that value instead of implicit `stable`.
- Added repository governance checks for `Cargo.toml` / `Cargo.lock` / `CHANGELOG.md` version alignment, `--locked` cargo usage, Dockerfile toolchain parity, and workflow smoke wiring.
- Added lightweight binary smoke coverage to CI and the Linux release build so tagged artifacts are exercised before packaging.
- Normalized repository-wide Rust formatting so the release `cargo fmt --check` gate passes again.
- Fixed `clippy -D warnings` failures in the debug trace helpers, request handler linting, and shared test mock utilities.
- Re-ran the local release gate successfully with:
  - `cargo fmt --all -- --check`
  - `cargo clippy --locked --all-targets --all-features -- -D warnings`
  - `cargo test --locked --verbose`
- Carries forward the real CLI E2E smoke-script work documented in `v0.2.3`, including real `codex` / `claude` proxy smoke coverage and release-note documentation updates.

## v0.2.3 - 2026-04-07

- Added a real-client E2E smoke script at `scripts/test_cli_clients.sh` that exercises the proxy with actual `codex` and `claude` CLI processes against mixed upstream aliases.
- Verified cross-protocol routing for Codex CLI over OpenAI Responses and Claude Code over Anthropic Messages, including Anthropic-compatible and OpenAI-compatible upstreams behind one proxy.
- Isolated Claude Code smoke tests from the user's global configuration by running them with a temporary `CLAUDE_CONFIG_DIR` and `--bare` mode, without modifying `~/.claude/settings.json`.
- Fixed the Claude smoke-test base URL handling so the client points at `/anthropic` and lets Claude append `/v1/messages` itself.
- Fixed Codex multi-turn smoke tests in temporary workspaces by adding `--skip-git-repo-check`.
- Marked the local `qwen-local` alias as an intentional skip for multi-turn code-edit tasks where the model is not reliable enough, while keeping its single-turn smoke coverage enabled.
- Documented the new smoke script and its constraints in `README.md` and `README_CN.md`.

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
