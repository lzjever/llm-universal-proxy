# LLM Universal Proxy — Project Constitution

## Fundamental Purpose

LLM Universal Proxy is a **format-agnostic protocol translation middleware** for Large Language Model APIs. Its mission is to serve as a maximally compatible bridge layer between configured LLM clients and backends, with explicit portability boundaries when protocols differ.

## Mission Statement

**Enable supported LLM API clients to reach configured backends through one stable proxy, with explicit portability boundaries.**

A client using OpenAI Chat Completions, OpenAI Responses, Anthropic Messages, or Google Gemini should be able to route to configured upstreams through a matching local namespace. When protocols differ, the proxy translates portable core semantics and warns or rejects non-portable native extensions instead of hiding the mismatch.

## Core Principles

### 1. Universal Interoperability

The proxy must support bidirectional translation between all major LLM API protocols:

| Client Protocol | Upstream Protocol | Proxy Role |
|----------------|-------------------|------------|
| OpenAI Chat Completions | Supported upstream protocol | Translate if needed within the portability contract |
| OpenAI Responses API | Supported upstream protocol | Translate if needed within the portability contract |
| Anthropic Messages | Supported upstream protocol | Translate if needed within the portability contract |
| Google Gemini | Supported upstream protocol | Translate if needed within the portability contract |

The proxy is not opinionated about which protocol is "best." It treats all four as first-class citizens.

### 2. Maximally Faithful Translation

When translating between protocols, the proxy must preserve as much semantic fidelity as possible:

- **Text content** — preserve portable user-visible text content; warn or reject when a field cannot be represented safely
- **Tool calls / function calling** — preserve portable function definitions, arguments, and results across supported protocols
- **Tool identity** — preserve the stable visible tool name supplied by the client; internal bridge names must never become the live model-visible or client-visible contract
- **Media identity** — typed media hints and source identities must be self-consistent; conflicting MIME provenance or unsupported provider/local URI sources must fail closed instead of being normalized by guesswork
- **Thinking / reasoning** — preserve reasoning output in whatever form the upstream provides
- **Usage / token counting** — map token metrics to the client's expected format
- **Stop reasons / finish reasons** — map between protocol-specific stop reason semantics
- **Streaming** — translate SSE chunk streams in real time with correct lifecycle events

When exact 1:1 mapping is impossible, the proxy must degrade gracefully and signal the degradation via `x-proxy-compat-warning` headers rather than silently losing information.

### 3. Same-Provider Native Passthrough

When the route is same-provider/native, the proxy must forward requests and responses with **zero translation overhead** apart from explicit proxy behavior such as routing, authentication policy, headers, and observability. A compatible endpoint that speaks the same wire protocol is a same-format lane: it preserves portable core fields unless an explicit compatibility shim says otherwise, but it must not be treated as native provider passthrough.

### 4. Protocol-Agnostic Client Interface

The proxy exposes namespaced endpoints for each client protocol:

- `/openai/v1/...` — for OpenAI Chat Completions and Responses clients
- `/anthropic/v1/...` — for Anthropic Messages clients
- `/google/v1beta/...` — for Google Gemini clients

Clients choose the namespace that matches their native protocol. The proxy handles the rest.

### 5. Single Binary, Minimal Dependencies

The proxy compiles to a single static binary with no runtime dependencies beyond the OS. Configuration is a single YAML file. No databases, no external services, no daemon supervisors required.

### 6. Upstream Agnosticism

The proxy does not favor any particular LLM provider. It works equally well with:

- Official vendor APIs (OpenAI, Anthropic, Google)
- Third-party compatible endpoints (MiniMax, GLM, Kimi, DeepSeek, Mistral, etc.)
- Self-hosted local models (vLLM, Ollama, llama.cpp, etc.)
- Other endpoints that implement one of the supported protocols, within documented portability boundaries

## Invariants

These are non-negotiable properties that all future development must preserve:

1. **Supported protocol routing**: Every supported client protocol must be able to reach every supported upstream protocol within documented portability boundaries.
2. **Passthrough preserves native semantics**: Same-provider/native routes should avoid translation while still allowing explicit proxy behavior such as routing, auth policy, headers, and observability. Compatible same-protocol lanes preserve portable fields but are not native provider passthrough.
3. **Translated responses keep the client protocol shape**: The response must conform to the client's expected protocol shape, and any non-portable degradation must remain visible through warnings or rejection.
4. **Visible tool identity is preserved**: The proxy must never change the stable tool name supplied by the client on model-visible or client-visible surfaces.
5. **Streaming is first-class**: Streaming (SSE) support is mandatory for supported protocol pairs within the same portability and reject rules as non-streaming translation.
6. **Backward compatibility**: Adding a new protocol or feature must not break existing client-upstream combinations.
7. **Degradation is visible**: When the proxy must drop or approximate request/response fields, it must emit compatibility warnings rather than silently failing.
8. **Typed media fails closed on conflicting identity**: If MIME provenance disagrees across explicit metadata, data URIs, or filename hints, the proxy must reject before contacting the upstream.

Locked tool identity contract:

- The proxy must not rewrite the visible tool name supplied by the client.
- `__llmup_custom__*` is an internal transport artifact, not a public contract.
- `apply_patch` remains a public freeform tool on client-visible surfaces.

## Scope Boundaries

### In Scope

- Protocol format detection and translation (request + response, streaming + non-streaming)
- Multi-upstream routing with model aliases
- Credential management (client auth passthrough, server-side fallback, force-server)
- Auto-discovery of upstream protocol capabilities
- Observability (debug traces, hooks, dashboard)
- Tool/function call translation across protocols
- Reasoning/thinking output preservation
- Usage/token metric normalization
- Capability-surface projection for real agent clients and compatibility modes
- Proxy authentication boundaries for health, data-plane, and admin-plane routes

Proxy authentication is in scope:

- `/health` remains unauthenticated so process and container health checks can run without secrets.
- Data-plane provider/model/resource routes require `LLM_UNIVERSAL_PROXY_DATA_TOKEN` for shared or remote service use. Clients may send it as `X-LLMUP-Data-Token` or `Authorization: Bearer <data-token>`, and the proxy strips that token before upstream calls and hook payloads.
- `/dashboard` shell and static assets are public UI resources. Dashboard JavaScript sends `Authorization: Bearer <admin-token>` only when it calls existing `/admin/*` APIs.
- Admin-plane routes use `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` when it is configured, sent as `Authorization: Bearer <admin-token>`. Empty or whitespace-only admin tokens are misconfiguration and fail closed.
- When an admin or data token is not configured, that plane defaults to loopback-only access and rejects proxy-forwarding headers in loopback-only mode.
- A non-loopback listener with server-held provider credentials, sensitive upstream headers, or `auth_policy: force_server` must fail closed unless the data token boundary is configured.

### Out of Scope

- LLM inference execution — the proxy does not run models
- Prompt engineering or content modification
- Persistent conversation state (the proxy is stateless per request)
- Provider-owned lifecycle state reconstruction
- Rate limiting or quota management (delegated to upstream providers)
- Training data collection

## Design Philosophy

The proxy follows a **pivot-based translation** architecture: OpenAI Chat Completions serves as the canonical intermediate format. All cross-protocol conversions go through two steps:

```
Source Format → OpenAI Chat Completions → Target Format
```

This means adding a new protocol requires only two translators (new ↔ OpenAI), not N translators (new ↔ every other protocol). This keeps the translation matrix manageable as the protocol count grows.

## Reference

The design draws inspiration from the 9router project (`/home/percy/works/mbos-v1/9router`), which implements a similar hub-and-spoke translation model in Node.js with OpenAI as the pivot format, and supports 12+ formats including OpenAI, Anthropic, Gemini, Codex, Cursor, Kiro, Ollama, and others.
