# LLM Universal Proxy - Project Map

## Status

This file is the short contributor map for the current GA repository shape. It
points maintainers to the runtime modules, tests, scripts, and docs that usually
need to move together. It is not a protocol spec, design rationale, roadmap, or
changelog.

## Source Tree

```text
src/
  config.rs
  config/model_surface.rs
  dashboard.rs
  dashboard_logs.rs
  debug_trace.rs
  detect.rs
  discovery.rs
  downstream.rs
  formats.rs
  hooks.rs
  internal_artifacts.rs
  lib.rs
  main.rs
  telemetry.rs
  upstream.rs
  server/
    admin.rs
    body_limits.rs
    data_auth.rs
    errors.rs
    headers.rs
    models.rs
    mod.rs
    proxy.rs
    public_boundary.rs
    responses_resources.rs
    state.rs
    tracked_body.rs
    web_dashboard.rs
    web_dashboard/
  streaming/
  translate/

tests/
  common/
  integration_test.rs
  runtime_chain_test.rs
  upstream_proxy_matrix_test.rs
  test_cli_matrix_contracts.py
  test_default_matrix_surface_contract.py
  test_interactive_cli.py
  test_project_docs_contract.py
  test_real_cli_matrix.py
  test_real_endpoint_matrix.py
  test_release_gates.py

scripts/
  interactive_cli.py
  real_cli_matrix.py
  real_endpoint_matrix.py
  run_codex_proxy.sh
  run_claude_proxy.sh
  run_gemini_proxy.sh
  fixtures/cli_matrix/default_proxy_test_matrix.yaml

.github/workflows/
  ci.yml
  release.yml

docs/
  DESIGN.md
  PROJECT.md
  clients.md
  configuration.md
  container.md
  ga-readiness-review.md
  protocol-compatibility-matrix.md
  protocol-baselines/
```

## Runtime Map

| Path | Contributor entrypoint |
| --- | --- |
| `src/config.rs` | YAML/env config, static `data_auth`, upstream `provider_key.inline` / `provider_key.env` / `provider_key_env`, namespace/admin config payloads, aliases, validation, resource limits |
| `src/config/model_surface.rs` | Effective model surface vocabulary: modalities, tool flags, `apply_patch_transport`, compatibility mode |
| `src/formats.rs` | Shared client/upstream protocol names |
| `src/detect.rs` | Request-format detection by path and body shape |
| `src/discovery.rs` | Upstream capability probing and default-target selection |
| `src/upstream.rs` | Reqwest calls, upstream response reads, downstream-aware cancellation |
| `src/downstream.rs` | Downstream disconnect/cancellation tokens used by proxy and Responses resources |
| `src/telemetry.rs` | Runtime request metrics and transport accounting |
| `src/hooks.rs` | Best-effort exchange/usage hooks with bounded capture |
| `src/debug_trace.rs` | Local JSONL debug trace with bounded writer queue |
| `src/dashboard.rs` / `src/dashboard_logs.rs` | Optional live runtime dashboard process and log capture |

## Server Map

| Path | Contributor entrypoint |
| --- | --- |
| `src/server/mod.rs` | Router assembly for admin, dashboard, health, data routes, namespaces, CORS, disconnect wrapping |
| `src/server/state.rs` | Runtime namespace state, admin policy, upstream resolution bootstrap |
| `src/server/admin.rs` | Admin token middleware, namespace config/state handlers, and `GET` / `PUT` `/admin/data-auth` CAS handlers |
| `src/server/data_auth.rs` | Provider-route auth policy: static `data_auth`, `LLM_UNIVERSAL_PROXY_AUTH_MODE`, `LLM_UNIVERSAL_PROXY_KEY`, `provider_key.inline`, `provider_key.env`, `provider_key_env`, and admin separation |
| `src/server/body_limits.rs` | JSON request parsing with namespace `max_request_body_bytes` enforcement |
| `src/server/proxy.rs` | Main request execution path for OpenAI, Anthropic, and Gemini surfaces |
| `src/server/responses_resources.rs` | Native OpenAI Responses and Conversations lifecycle resource handlers |
| `src/server/models.rs` | Model list/detail handlers and effective surface exposure |
| `src/server/headers.rs` | Auth forwarding and protocol-specific upstream headers |
| `src/server/errors.rs` | Error normalization and compatibility response shaping |
| `src/server/public_boundary.rs` | Public boundary checks for request/tool artifacts |
| `src/server/tracked_body.rs` | Body wrapper for stream accounting and cancellation logging |
| `src/server/web_dashboard.rs` | Embedded dashboard asset handlers |
| `src/server/web_dashboard/index.html` / `src/server/web_dashboard/app.css` / `src/server/web_dashboard/app.js` | Static web dashboard UI assets |

## Translation And Streaming

| Path | Contributor entrypoint |
| --- | --- |
| `src/translate/assessment.rs` | Request-side translation decision surface |
| `src/translate/internal/` | Provider-specific request/response conversion, media, models, tools, regression tests |
| `src/streaming/stream.rs` | SSE translation wrapper and runtime stream behavior |
| `src/streaming/wire.rs` | SSE event parsing/formatting helpers |
| `src/streaming/*_source.rs` | Provider-specific upstream event to internal chunk mapping |
| `src/streaming/openai_sink.rs` | Client-facing stream emission helpers |
| `src/streaming/state.rs` | Cross-frame stream state and fatal rejection tracking |

## CLI And Release Harnesses

| Path | Contributor entrypoint |
| --- | --- |
| `scripts/interactive_cli.py` | Interactive CLI wrapper implementation for Codex CLI, Claude Code, and Gemini CLI |
| `scripts/run_codex_proxy.sh` / `scripts/run_claude_proxy.sh` / `scripts/run_gemini_proxy.sh` | Thin wrapper launchers that resolve `interactive_cli.py` relative to the script dir |
| `scripts/fixtures/cli_matrix/default_proxy_test_matrix.yaml` | Provider-neutral preset matrix fixture using `preset-openai-compatible` and `preset-anthropic-compatible` |
| `scripts/real_cli_matrix.py` | Deterministic real CLI matrix harness and verifier logic |
| `scripts/real_endpoint_matrix.py` | Endpoint matrix harness for mock, perf, real-provider, and `compatible-provider-smoke` modes |
| `.github/workflows/release.yml` | GA release gate wiring: Rust/Python tests, mock endpoint matrix, CLI wrapper matrix, perf, supply chain, compatible provider smoke |
| `.github/workflows/ci.yml` | CI mirror for local tests, governance, container smoke, and mock endpoint matrix |

The protected release compatible-provider smoke lives in
`.github/workflows/release.yml` as `compatible-provider-smoke`, runs in the
`release-compatible-provider` environment, invokes
`scripts/real_endpoint_matrix.py --mode compatible-provider-smoke`, and uploads
`artifacts/compatible-provider-smoke.json`.

## Where To Start By Task

| Task | Start with |
| --- | --- |
| Routing, namespaces, admin writes | `src/server/mod.rs`, `src/server/state.rs`, `src/server/admin.rs` |
| Data auth, CORS, sensitive data route access | `src/server/data_auth.rs`, `/admin/data-auth`, `docs/CONSTITUTION.md`, `docs/configuration.md`, `docs/admin-dynamic-config.md` |
| Request body size failures | `src/server/body_limits.rs`, `src/config.rs`, `tests/integration_test.rs` |
| Dashboard API/static UI | `src/dashboard.rs`, `src/server/web_dashboard.rs`, `src/server/web_dashboard/`, `src/server/tests/web_dashboard.rs` |
| Generic proxy execution | `src/server/proxy.rs`, `src/server/headers.rs`, `src/server/errors.rs`, `src/upstream.rs` |
| Downstream cancellation or disconnect behavior | `src/downstream.rs`, `src/server/tracked_body.rs`, `tests/runtime_chain_test.rs` |
| Model surface or CLI capability flags | `src/config/model_surface.rs`, `src/server/models.rs`, `scripts/interactive_cli.py`, `tests/test_default_matrix_surface_contract.py` |
| Protocol portability | `src/translate/assessment.rs`, `src/translate/internal/`, `docs/protocol-compatibility-matrix.md`, `docs/protocol-baselines/` |
| Streaming/SSE behavior | `src/streaming/stream.rs`, `src/streaming/wire.rs`, `src/streaming/state.rs`, `src/streaming/tests/` |
| Interactive CLI wrapper behavior | `scripts/interactive_cli.py`, wrapper shell scripts, `tests/test_interactive_cli.py` |
| Endpoint matrix or release compatible-provider smoke | `scripts/real_endpoint_matrix.py`, `tests/test_real_endpoint_matrix.py`, `tests/test_release_gates.py`, `.github/workflows/release.yml` |

## Test Map

| Path | Primary purpose |
| --- | --- |
| `tests/integration_test.rs` | End-to-end protocol routing, translation, models, admin, data auth, `/admin/data-auth`, body limits, dashboard, debug trace |
| `tests/runtime_chain_test.rs` | Cancellation propagation, teardown, namespace isolation, hooks/debug trace runtime behavior, fatal translated stream rejection |
| `tests/upstream_proxy_matrix_test.rs` / `tests/upstream_proxy_test.rs` | Upstream proxy behavior and matrix coverage |
| `src/server/tests/` | Split server behavior, including admin, headers, proxy, Responses resources, state, web dashboard |
| `src/streaming/tests/` | Parser, sink/source, and stream edge cases |
| `src/translate/internal/tests/` | Provider translation internals and regression coverage |
| `tests/test_interactive_cli.py` | Interactive CLI wrapper contract, provider-neutral presets, wrapper scripts, hermetic scripted interactive Codex wrapper gate |
| `tests/test_cli_matrix_contracts.py` | CLI contract verifier behavior for public tool names and debug-trace matching |
| `tests/test_real_cli_matrix.py` | Real CLI matrix harness behavior |
| `tests/test_real_endpoint_matrix.py` | Endpoint matrix case construction, modes, reports, provider-neutral compatible smoke |
| `tests/test_release_gates.py` | Release gate workflow, governance, endpoint matrix, CLI wrapper matrix, supply-chain, compatible-provider artifact contracts |
| `tests/test_default_matrix_surface_contract.py` | Provider-neutral preset matrix surface defaults for `preset-openai-compatible` and `preset-anthropic-compatible` |
| `tests/test_project_docs_contract.py` | Contract that this project map keeps the GA contributor entrypoints visible |

## Document Map

| Document | Role |
| --- | --- |
| `docs/CONSTITUTION.md` | High-level architectural principles and invariants |
| `docs/PRD.md` | Product and behavior requirements |
| `docs/DESIGN.md` | Architecture snapshot and execution-chain design notes |
| `docs/PROJECT.md` | Current repository and maintenance map |
| `docs/clients.md` | CLI/client wiring and wrapper/manual endpoint expectations |
| `docs/configuration.md` | Config syntax, auth policies, model aliases, model surface fields |
| `docs/container.md` | Container build/smoke/release path and release gate summary |
| `docs/ga-readiness-review.md` | GA posture, remaining evidence, and release-gate checklist |
| `docs/protocol-compatibility-matrix.md` | Field-level portability, degrade, reject policy |
| `docs/protocol-baselines/` | Provider reference captures used for protocol work |

## Maintenance Rules

When the implementation changes, update the document that matches the change:

- Update `docs/DESIGN.md` when runtime architecture or major execution chain changes.
- Update `docs/PROJECT.md` when module paths, tests, scripts, or contributor entrypoints change.
- Update `docs/protocol-compatibility-matrix.md` when protocol portability or downgrade behavior changes.
- Update `docs/clients.md`, `README.md`, or `README_CN.md` when user-visible setup or client behavior changes.
- Update release/container docs when `.github/workflows/release.yml`, release gate names, or artifact names change.
