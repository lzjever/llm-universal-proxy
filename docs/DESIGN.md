# LLM Universal Proxy - Current Design

## Status

This document describes the current implementation shape at `HEAD`.

It is not the product-spec source of truth and it is not a protocol-fidelity contract:

- Product and behavioral requirements live in [PRD.md](./PRD.md) and [CONSTITUTION.md](./CONSTITUTION.md).
- Field-level portability, downgrade, and reject rules live in [protocol-compatibility-matrix.md](./protocol-compatibility-matrix.md) and the protocol baselines under [protocol-baselines/](./protocol-baselines/README.md).

Earlier versions of this document described a much smaller v0 proxy with a single upstream, a mostly single-file server, and discovery/passthrough as the dominant architectural concern. The codebase has moved well past that shape. Keeping this document as a "current architecture map" is more useful than preserving the older plan as if it were still live.

## System Shape

The current binary contains four major runtime surfaces:

1. Data plane HTTP API for OpenAI, Anthropic, and Google/Gemini-compatible clients.
2. Admin control plane for runtime namespace configuration and redacted state inspection.
3. Optional local dashboard driven from live runtime snapshots and metrics.
4. Optional observability sinks: async hooks and local debug trace.

At a high level:

```text
Client request
  -> server router
  -> namespace runtime state
  -> upstream capability + model resolution
  -> request assessment / translation if needed
  -> upstream HTTP call
  -> optional stream translation
  -> optional hooks + debug trace wrappers
  -> telemetry finalization
  -> client response

Admin client
  -> /admin router
  -> runtime state CAS update / redacted state read

Dashboard
  -> live runtime snapshot + metrics
```

## Runtime State Model

The live runtime is namespace-scoped rather than process-global.

- `AppState` holds:
  - `RuntimeState` behind `Arc<RwLock<...>>`
  - `RuntimeMetrics`
  - resolved admin access policy
- `RuntimeState` is a map of namespace name -> `RuntimeNamespaceState`
- `RuntimeNamespaceState` contains:
  - current namespace `Config`
  - resolved `UpstreamState` map
  - optional `HookDispatcher`
  - optional `DebugTraceRecorder`
  - server-owned revision used by admin writes
- `UpstreamState` tracks:
  - configured upstream info
  - discovered or fixed capability
  - availability status
  - unary HTTP client
  - streaming HTTP client
  - resolved proxy metadata used by request execution and admin state

This matters because:

- namespace-prefixed data-plane routes are first-class, not bolt-ons
- admin writes swap live namespace config without restarting the process
- dashboard and admin state reflect runtime state, not just startup config

## Data Plane and Control Plane

### Data plane

The data plane is protocol-namespaced and also supports namespace-prefixed variants:

- `/openai/v1/...`
- `/anthropic/v1/...`
- `/google/v1beta/...`
- `/namespaces/:namespace/openai/v1/...`
- `/namespaces/:namespace/anthropic/v1/...`
- `/namespaces/:namespace/google/v1beta/...`

Router assembly lives in `src/server/mod.rs`, while the behavior is split across `src/server/proxy.rs`, `src/server/responses_resources.rs`, `src/server/models.rs`, `src/server/errors.rs`, and `src/server/headers.rs`.

### Admin plane

The admin plane is intentionally separate from the data plane:

- routes live under `/admin/...`
- auth policy is bearer-token or loopback-only, with fail-closed handling for misconfiguration
- reads serialize redacted view models
- writes use server-owned revision / compare-and-swap semantics

This separation is architectural, not cosmetic: admin middleware and browser-facing data-plane behavior are intentionally not shared by accident.

## Upstream Capability and Routing

Each upstream is either:

- fixed to a configured protocol via `fixed_upstream_format`, or
- probed at runtime by `src/discovery.rs`

Discovery still exists, but it is now only one part of a larger routing system. It determines:

- which client formats can be passed through natively
- which fallback target format to use when translation is required
- whether an upstream is currently usable

Important caveat: discovery is a capability bootstrap signal, not a guarantee that every route or field is portable. Request-side assessment and route-specific rules still decide whether a request is allowed, warned, translated, or rejected.

### Upstream Transport Policy

Outbound transport is resolved per upstream, not by sharing one namespace-wide client pair.

- Each namespace config may define a top-level `proxy` default.
- Each upstream may define its own `proxy` override.
- Effective priority is `upstream > namespace > env > no proxy`.
- `proxy: direct` explicitly cuts off env proxy inheritance for that scope.
- An explicit proxy URL also cuts off env proxy inheritance for that scope and pins the upstream to that proxy.
- Explicit proxy URLs are currently validated as `http`, `https`, `socks5`, or `socks5h`.
- Discovery, normal requests, streaming requests, and OpenAI Responses resource routes reuse the same resolved per-upstream transport policy.
- The runtime resolves that policy once when building `UpstreamState`, then reuses the upstream's unary and streaming clients everywhere that upstream is called.
- Admin state exposes a redacted summary of that decision through `proxy_source`, `proxy_mode`, and `proxy_url`.

## Request Execution Flow

Most client-facing request execution lives in `src/server/proxy.rs`.

The current flow is:

1. Select namespace and load its live `RuntimeNamespaceState`.
2. Detect client format from route and request shape.
3. Resolve the requested model to an upstream/model pair.
4. Check upstream availability and capability.
5. Run request-side compatibility / portability assessment.
6. If client format is natively supported, pass through the request body.
7. Otherwise, translate the request through the translate facade.
8. Apply auth forwarding and configured upstream headers.
9. Call upstream through the selected upstream state's unary or streaming HTTP client.
10. Normalize non-stream responses or wrap stream responses in the runtime chain.

OpenAI Responses lifecycle resources are a special case. They do not use the generic "translate anything anywhere" path. `src/server/responses_resources.rs` only proxies those resource routes when the namespace can identify a unique native OpenAI Responses upstream. The server does not invent response-session ownership state.

## Translation Layer

`src/translate/` is now a facade tree rather than a single implementation file:

- `mod.rs` exposes the stable request/response API
- `assessment.rs` handles request-side translation decisions
- `internal/` contains the real protocol mapping logic
- `request/` and `response/` are facade seams

The implementation is still OpenAI-centric internally, but the important architectural fact is not "everything is a perfect pivot." The important fact is:

- request-side portability is assessed before translation
- translation is fail-closed on high-risk incompatibilities
- field-level degradations are intentionally tracked outside this document

Current structural guardrails:

- `RequestTranslationPolicy` is the runtime boundary from resolved config/model limits into the translator. Translation consumes policy defaults such as effective `max_output_tokens`; it does not reach back into config on its own.
- Only complete and trusted structured tool calls are allowed to enter replayable history. Incomplete or truncated calls are marked non-replayable and intentionally degraded on later replay/bridge paths instead of being treated as safe structured replay.
- When a bridge rewrites tool-call representation, any trusted non-replayable marker must be re-attested against the rewritten value. Literal marker copy is invalid because the signature is bound to the current `name` / raw payload.
- Visible tool identity is part of the live client contract. Request translation may adapt argument encoding and protocol field shape, but it must not change the model-visible or client-visible stable tool name supplied by the client.
- The reserved prefix `__llmup_custom__*` is internal transport machinery only. Public request translation, response translation, and client-visible output must reject or clear it rather than surface it as a live contract.
- The target direction for `max_compat` is a request-scoped tool bridge context that preserves stable tool names in the live request while still allowing non-stream and streaming response translators to decode bridged tool calls back into native custom/freeform semantics.

Locked tool identity contract:

- The proxy must not rewrite the visible tool name supplied by the client.
- `__llmup_custom__*` is an internal transport artifact, not a public contract.
- `apply_patch` remains a public freeform tool on client-visible surfaces.

For current compatibility rules, use the protocol matrix and baseline docs, not this file.

## Streaming and Transport Lifecycle

Streaming is its own subsystem under `src/streaming/`.

### Translation chain

When translation is needed, the server wraps the upstream byte stream with `TranslateSseStream`, which:

- buffers upstream bytes
- parses SSE events via `wire.rs`
- translates event-by-event
- maintains cross-frame state in `state.rs`
- closes promptly after fatal translation rejection instead of draining upstream to EOF

### Parser behavior

`src/streaming/wire.rs` is intentionally tolerant of real upstream wire behavior:

- supports LF and CRLF separators
- joins multiline `data:` payloads
- ignores blank `data:` frames instead of treating them as terminal

### Runtime transport chain

For translated or observed streams, the effective wrapper order is:

```text
upstream bytes stream
  -> TranslateSseStream (if needed)
  -> HookCaptureStream (if hooks enabled)
  -> DebugTraceStream (if debug trace enabled)
  -> TrackedBodyStream (telemetry / cancellation finalization)
  -> client body
```

This order matters:

- hooks and debug trace observe client-visible translated output, not raw upstream protocol bytes
- `TrackedBodyStream` owns success/error/cancel transport finalization
- downstream disconnect tears down the stream by dropping the wrapper chain

Each `UpstreamState` owns two HTTP clients:

- a unary client with total upstream timeout
- a streaming client with connect/setup timeout but no shared total-request timeout

That split prevents long-lived SSE streams from inheriting unary timeout behavior while still keeping discovery, request execution, and resource routes on the same upstream-scoped transport policy.

## Observability Design

### Runtime metrics

`src/telemetry.rs` tracks request counts and outcome finalization. This is always-on process/runtime telemetry, not a protocol log.

### Hooks

`src/hooks.rs` provides optional async best-effort export for:

- `usage`
- `exchange`

Important current behavior:

- delivery is bounded by `max_pending_bytes`
- `exchange` capture is bounded and may be truncated
- stream observations record both transport outcome and protocol terminal, rather than collapsing them into one bit
- heavy replay/build work happens off the live request path

This is an observability/export system, not a guarantee of lossless archival.

### Debug trace

`src/debug_trace.rs` provides an optional local JSONL trace for developer troubleshooting.

Important current behavior:

- request entries record only the new tail input for the current turn
- response entries record normalized summaries, not raw SSE dumps
- streaming summaries preserve terminal event / finish reason / normalized error
- writes go through a background bounded queue
- overflow is surfaced explicitly rather than silently pretending the trace is complete

### Dashboard

`src/dashboard.rs` consumes live runtime snapshots plus metrics. It is an implementation consumer of runtime state, not a second source of truth.

## What This Document Does Not Define

This document intentionally does not try to freeze:

- every protocol field mapping
- every portability warning string
- every dashboard UI detail
- every internal helper or module name

Use this file to understand the current runtime architecture and where behavior lives. Use the protocol docs and tests to understand exact wire semantics.

## Historical Note

The older "single upstream + one server file + discovery-first proxy" design was a valid starting point, but it is now historical context only. Current contributors should treat the server tree, namespace runtime, stateful Responses routing, bounded observability pipeline, and runtime-chain tests as the real architecture.
