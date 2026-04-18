# LLM Universal Proxy - Project Map

## Status

This file is a contributor map for the current repository.

It exists to answer:

- where the real code lives
- which modules own which runtime responsibilities
- which tests protect which behaviors
- which docs are normative versus descriptive

Earlier versions of this file described a much smaller v0 layout. The current project is broader: namespace-scoped runtime state, split server modules, split translate/streaming trees, optional dashboard, hooks, debug trace, and dedicated runtime-chain coverage.

## Source Tree

```text
src/
  config.rs
  dashboard.rs
  debug_trace.rs
  detect.rs
  discovery.rs
  formats.rs
  hooks.rs
  lib.rs
  main.rs
  telemetry.rs
  upstream.rs
  server/
    admin.rs
    errors.rs
    headers.rs
    models.rs
    mod.rs
    proxy.rs
    responses_resources.rs
    state.rs
  streaming/
    anthropic_source.rs
    gemini_source.rs
    mod.rs
    openai_sink.rs
    responses_source.rs
    state.rs
    stream.rs
    wire.rs
  translate/
    assessment.rs
    internal.rs
    internal/
    mod.rs
    request/
    response/
    shared.rs

tests/
  common/
  detect_test.rs
  integration_test.rs
  reasoning_test.rs
  runtime_chain_test.rs
  test_real_cli_matrix.py

scripts/
  real_cli_matrix.py
  real_endpoint_matrix.py
  test-and-report.sh
  test_cli_clients.sh
  test_binary_smoke.sh
  test_compatibility.sh

docs/
  CONSTITUTION.md
  PRD.md
  DESIGN.md
  PROJECT.md
  protocol-compatibility-matrix.md
  protocol-baselines/
```

## Module Responsibilities

### Core runtime modules

| Path | Responsibility |
| --- | --- |
| `src/config.rs` | Config parsing, runtime/admin config payloads, defaults, validation |
| `src/formats.rs` | Protocol enum definitions and shared format naming |
| `src/detect.rs` | Client-format detection from path and request shape |
| `src/discovery.rs` | Upstream capability probing and default-target selection |
| `src/upstream.rs` | Reqwest client construction and upstream HTTP call helpers |
| `src/telemetry.rs` | Request metrics and transport outcome accounting |

### Server tree

| Path | Responsibility |
| --- | --- |
| `src/server/mod.rs` | Public server facade and router assembly |
| `src/server/state.rs` | Runtime namespace state, admin access policy, upstream resolution bootstrap |
| `src/server/proxy.rs` | Main request execution path and streaming body orchestration |
| `src/server/responses_resources.rs` | Native OpenAI Responses lifecycle resource handlers |
| `src/server/models.rs` | Model list/detail handlers across protocol namespaces |
| `src/server/headers.rs` | Auth forwarding and upstream protocol header helpers |
| `src/server/errors.rs` | Error normalization and response shaping |
| `src/server/admin.rs` | Admin auth middleware and namespace/state handlers |

### Translation and streaming

| Path | Responsibility |
| --- | --- |
| `src/translate/mod.rs` | Stable translation facade |
| `src/translate/assessment.rs` | Request-side translation decision surface |
| `src/translate/internal/` | Provider-specific request/response logic |
| `src/streaming/mod.rs` | Streaming facade and exports |
| `src/streaming/stream.rs` | SSE translation wrapper and stream runtime behavior |
| `src/streaming/wire.rs` | SSE event parsing/formatting helpers |
| `src/streaming/*_source.rs` | Provider-specific upstream-event to internal chunk mapping |
| `src/streaming/openai_sink.rs` | Client-facing stream emission helpers |
| `src/streaming/state.rs` | Cross-frame stream state and fatal rejection tracking |

### Observability

| Path | Responsibility |
| --- | --- |
| `src/hooks.rs` | Async best-effort usage/exchange hooks with bounded capture |
| `src/debug_trace.rs` | Local JSONL debug trace with bounded writer queue |
| `src/dashboard.rs` | Optional dashboard backed by live runtime snapshots |

## Where To Start By Task

### Routing, namespaces, admin writes

Start in:

- `src/server/mod.rs`
- `src/server/state.rs`
- `src/server/admin.rs`

### Generic request execution or upstream call behavior

Start in:

- `src/server/proxy.rs`
- `src/server/errors.rs`
- `src/server/headers.rs`
- `src/upstream.rs`

### Stateful OpenAI Responses resource behavior

Start in:

- `src/server/responses_resources.rs`

### Protocol portability and request/response mapping

Start in:

- `src/translate/assessment.rs`
- `src/translate/mod.rs`
- `src/translate/internal/`
- `docs/protocol-compatibility-matrix.md`
- `docs/protocol-baselines/`

### Streaming runtime-chain and SSE handling

Start in:

- `src/streaming/stream.rs`
- `src/streaming/wire.rs`
- `src/streaming/state.rs`
- `src/server/proxy.rs`

### Hooks, debug trace, and transport/teardown observability

Start in:

- `src/hooks.rs`
- `src/debug_trace.rs`
- `tests/runtime_chain_test.rs`

## Test Map

| Path | Primary purpose |
| --- | --- |
| `tests/detect_test.rs` | Request-format detection coverage |
| `tests/integration_test.rs` | End-to-end protocol routing, translation, models, admin, dashboard, and debug-trace integration |
| `tests/reasoning_test.rs` | Thinking/reasoning portability and usage mapping |
| `tests/runtime_chain_test.rs` | Cancellation propagation, teardown, namespace isolation, hooks/debug trace runtime behavior, fatal translated stream rejection |
| `tests/common/mock_upstream.rs` | Mock upstream implementations for supported protocols |
| `tests/test_real_cli_matrix.py` | Python tests for the real CLI matrix harness |

In-module tests also matter:

- `src/server/tests/` covers split server behavior in unit-style form
- `src/streaming/tests/` covers parser and stream edge cases
- `src/debug_trace.rs` and `src/hooks.rs` include focused observability tests

## Verification Harnesses

These scripts are useful, but they are not substitutes for the Rust test suite:

| Path | Purpose |
| --- | --- |
| `scripts/real_cli_matrix.py` | Real Codex / Claude / Gemini CLI matrix through the proxy |
| `scripts/real_endpoint_matrix.py` | Lower-level protocol/HTTP smoke without real CLI processes |
| `scripts/test_cli_clients.sh` | Compatibility shim around `real_cli_matrix.py` |
| `scripts/test-and-report.sh` | Local test run with report artifacts |

When changing runner behavior, also update:

- `tests/test_real_cli_matrix.py`
- relevant README sections if user-facing behavior changes

## Document Map

| Document | Role |
| --- | --- |
| `docs/CONSTITUTION.md` | High-level architectural principles and invariants |
| `docs/PRD.md` | Product and behavior requirements |
| `docs/DESIGN.md` | Current implementation architecture snapshot |
| `docs/PROJECT.md` | Current repository and maintenance map |
| `docs/protocol-compatibility-matrix.md` | Field-level portability, degrade, reject policy |
| `docs/protocol-baselines/` | Provider reference captures used for protocol work |

## Maintenance Rules

When the implementation changes, update the document that matches the change:

- Update `docs/DESIGN.md` when the runtime architecture or major execution chain changes.
- Update `docs/PROJECT.md` when the module tree, test map, or contributor entrypoints change.
- Update `docs/protocol-compatibility-matrix.md` when protocol portability or downgrade behavior changes.
- Update `README.md` / `README_CN.md` when user-visible behavior, setup, or guarantees change.

## What This File Intentionally Does Not Do

This file is not:

- a protocol specification
- a product roadmap
- a changelog
- a promise that every internal module name is stable forever

It should stay short, current, and practical for contributors navigating the repo today.
