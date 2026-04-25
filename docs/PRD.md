# LLM Universal Proxy — Product Requirements Document (PRD)

**Version**: 1.0
**Status**: Active
**Last Updated**: 2026-04-23

---

## 1. Product Overview

### 1.1 Product Name

LLM Universal Proxy (public short name: llmup)

### 1.2 Product Definition

A single-binary HTTP proxy that provides protocol-namespaced entrypoints and translation between supported LLM API surfaces. Clients using OpenAI Chat Completions, OpenAI Responses, Anthropic Messages, or Google Gemini can route through one stable proxy to configured upstream endpoints; same-protocol paths stay native where possible, while translated paths preserve the portable core and warn or reject non-portable provider-native features.

### 1.3 Problem Statement

The LLM ecosystem is fragmented across incompatible API protocols:

- **OpenAI Chat Completions** — the de facto standard for most third-party LLM providers
- **OpenAI Responses API** — used by newer tools like Codex CLI
- **Anthropic Messages** — used by Claude Code and Claude-native applications
- **Google Gemini** — used by Gemini CLI and Google-native applications

Each client tool typically speaks only one protocol. Each upstream endpoint typically supports only one or two protocols. This creates an N×M compatibility matrix that is impractical for users and tool developers to manage individually.

### 1.4 Target Users

| User | Use Case |
|------|----------|
| Developer using Codex CLI | Wants to use a non-OpenAI model (GLM, MiniMax, Kimi, local vLLM) through Codex's Responses-only interface |
| Developer using Claude Code | Wants to route Claude Code requests to a non-Anthropic upstream for cost or availability reasons |
| Developer using Gemini CLI | Wants to use Gemini CLI with a non-Google upstream endpoint |
| Team with multiple LLM providers | Wants a single stable endpoint that normalizes model naming across providers |
| Operator running local models | Wants to expose a local vLLM/Ollama instance to any LLM client tool regardless of protocol |
| AI infrastructure engineer | Wants an observability layer for request/response auditing and usage metering |

---

## 2. Functional Requirements

### 2.1 Protocol Support Matrix

The proxy MUST support bidirectional translation between all four protocols:

| # | Client Protocol | Upstream Protocol | Translation Required |
|---|----------------|-------------------|---------------------|
| 1 | OpenAI Chat Completions | OpenAI Chat Completions | No (passthrough) |
| 2 | OpenAI Chat Completions | OpenAI Responses | Yes |
| 3 | OpenAI Chat Completions | Anthropic Messages | Yes |
| 4 | OpenAI Chat Completions | Google Gemini | Yes |
| 5 | OpenAI Responses | OpenAI Chat Completions | Yes |
| 6 | OpenAI Responses | OpenAI Responses | No (passthrough) |
| 7 | OpenAI Responses | Anthropic Messages | Yes |
| 8 | OpenAI Responses | Google Gemini | Yes |
| 9 | Anthropic Messages | OpenAI Chat Completions | Yes |
| 10 | Anthropic Messages | OpenAI Responses | Yes |
| 11 | Anthropic Messages | Anthropic Messages | No (passthrough) |
| 12 | Anthropic Messages | Google Gemini | Yes |
| 13 | Google Gemini | OpenAI Chat Completions | Yes |
| 14 | Google Gemini | OpenAI Responses | Yes |
| 15 | Google Gemini | Anthropic Messages | Yes |
| 16 | Google Gemini | Google Gemini | No (passthrough) |

All 16 combinations (4 passthrough + 12 translated) MUST be supported within documented portability boundaries. Passthrough remains lossless; translated paths may warn or reject non-portable semantics rather than silently approximating them.

### 2.2 Client Endpoints

The proxy MUST expose the following namespaced endpoints:

| Endpoint | Protocol | Method |
|----------|----------|--------|
| `/openai/v1/chat/completions` | OpenAI Chat Completions | POST |
| `/openai/v1/responses` | OpenAI Responses | POST |
| `/openai/v1/responses/:response_id` | OpenAI Responses lifecycle | GET, DELETE |
| `/openai/v1/responses/:response_id/cancel` | OpenAI Responses lifecycle | POST |
| `/openai/v1/responses/compact` | OpenAI Responses lifecycle | POST |
| `/openai/v1/models` | Model catalog | GET |
| `/anthropic/v1/messages` | Anthropic Messages | POST |
| `/anthropic/v1/models` | Model catalog | GET |
| `/google/v1beta/models/:id` | Gemini GenerateContent | POST |
| `/google/v1beta/models` | Model catalog | GET |

### 2.3 Streaming Support

- The proxy MUST support SSE (Server-Sent Events) streaming for all 16 protocol combinations within the same portability, warning, and reject boundaries as non-streaming translation.
- Streaming translation MUST operate chunk-by-chunk without buffering the entire response.
- Each protocol's streaming lifecycle events MUST be correctly translated:
  - **OpenAI Chat Completions**: `data:` chunks + `[DONE]`
  - **OpenAI Responses**: `response.created` → `response.in_progress` → `output_item.added` → `content_part.added` → `output_text.delta` → ... → `response.completed`
  - **Anthropic Messages**: `message_start` → `content_block_start` → `content_block_delta` → `content_block_stop` → `message_delta` → `message_stop`
  - **Google Gemini**: `candidates` chunks with `parts`

### 2.4 Request Translation

The proxy MUST translate the following request fields across protocols:

| Field | Status | Notes |
|-------|--------|-------|
| Text messages | Must | Preserve portable text content across supported formats |
| System instructions | Must | Map between `system` role (OpenAI), top-level `system` (Anthropic), `systemInstruction` (Gemini), `instructions` (Responses) |
| Function tool definitions | Must | Map portable `function` tools across supported formats |
| Visible tool identity | Must | The stable tool name supplied by the client is part of the semantic contract and must not be rewritten on model-visible or client-visible surfaces |
| Tool choice (auto/none/required) | Must | Map between format-specific tool choice objects |
| `max_output_tokens` / `max_tokens` | Must | Normalize field names |
| `temperature`, `top_p` | Should | Pass through generation config where applicable |
| Image content | Should | Map portable image parts between OpenAI, Anthropic, and Gemini when the effective surface allows `image`; OpenAI HTTP(S) image URLs can map to Anthropic URL image sources, but Anthropic remote image URLs to Gemini fail closed without an explicit fetch/upload adapter |
| PDF content | Should | Map PDF data URIs and PDF HTTP(S) file references when PDF MIME or filename provenance is available and the effective surface allows `pdf` or `file` |
| Typed media source boundary | Must | Treat `surface.modalities.input` as a media-type gate, not a source transport promise; provider `file_id` and provider-native or local URIs such as `gs://`, `s3://`, and `file://` fail closed unless a documented adapter supports them |
| Typed media MIME provenance | Must | Reject conflicting MIME hints before routing so surface gates and translators cannot disagree about the actual media kind |
| Thinking/reasoning config | May | Preserve where upstream supports it |
| Built-in / non-function tools | Won't | Dropped during cross-protocol translation with compat warning unless a documented bridge can preserve the original visible tool identity |

### 2.5 Response Translation

The proxy MUST translate the following response fields:

| Field | Status | Notes |
|-------|--------|-------|
| Text content | Must | Preserve portable text content; warn or reject when a response field cannot be represented safely |
| Tool calls / function calls | Must | Map between `tool_calls` (OpenAI), `tool_use` blocks (Anthropic), `functionCall` parts (Gemini) |
| Tool results | Must | Map between `tool` role messages (OpenAI), `tool_result` blocks (Anthropic), `functionResponse` parts (Gemini) |
| Usage / token metrics | Must | Map between `prompt_tokens/completion_tokens` (OpenAI), `input_tokens/output_tokens` (Anthropic), `promptTokenCount/candidatesTokenCount` (Gemini) |
| Finish reasons | Must | Map stop reasons: `stop` ↔ `end_turn` ↔ `STOP`, `tool_calls` ↔ `tool_use` ↔ `STOP`, `length` ↔ `max_tokens` ↔ `MAX_TOKENS` |
| Reasoning / thinking output | Should | Preserve as `reasoning_content` (OpenAI), `thinking` blocks (Anthropic), `thought` parts (Gemini) |
| Cached token details | Should | Map cache-related usage fields |
| Error responses | Must | Translate upstream errors into client-protocol-appropriate error shapes |

### 2.6 Compatibility Modes

The proxy MUST support explicit compatibility modes for translated paths:

| Mode | Goal | Expected behavior |
|------|------|-------------------|
| `strict` | High-assurance protocol safety | Reject any translation path that would require visible tool renaming, provider-state reconstruction, or other unsafe semantic approximation |
| `balanced` | Safe interoperability | Preserve portable core semantics, emit compat warnings for allowed degradations, and keep current fail-closed behavior for high-risk surfaces |
| `max_compat` | Agent-client usability | Prefer client-usable translated paths, but still preserve stable tool identity and never expose proxy-generated synthetic tool names as live tool contracts |

Locked tool identity contract:

- The proxy must not rewrite the visible tool name supplied by the client.
- `__llmup_custom__*` is an internal transport artifact, not a public contract.
- `apply_patch` remains a public freeform tool on client-visible surfaces.
- Public editing tool identity is per client: Codex exposes `apply_patch`, Claude Code exposes `Edit`, and Gemini exposes `replace`; the proxy must not rewrite those public names.

### 2.7 Upstream Configuration

The proxy MUST support:

- **Multiple named upstreams** — each with its own API root, protocol format, and credentials
- **Model aliases** — map local model names to `upstream:real_model` pairs
- **Auto-discovery** — probe upstream to detect supported protocols when format is not explicitly set
- **Credential policies**:
  - `client_or_fallback` — use client-provided auth if present, otherwise use configured fallback
  - `force_server` — always use server-side credentials, ignore client auth
- **Discovery availability split**:
  - fixed-format upstreams are immediately available
  - auto-discovered upstreams are available only when discovery returns at least one supported protocol
  - empty discovery results MUST be treated as unavailable, not silently downgraded to another protocol

### 2.7 Observability

| Feature | Priority | Description |
|---------|----------|-------------|
| Debug trace (JSONL) | Must | Per-request debug trace file for protocol troubleshooting |
| Health endpoint | Must | `GET /health` returns `{"status":"ok"}` |
| Exchange hooks | Should | Async HTTP webhook for full request/response capture |
| Usage hooks | Should | Async HTTP webhook for token usage metering |
| Dashboard | May | Terminal UI showing live request stats and upstream health |
| Compatibility warnings | Must | `x-proxy-compat-warning` response headers for degraded translations |
| Request ID tracking | Must | Every request gets a unique ID for correlation |

Dashboard requirements:

- The dashboard MUST render from live runtime state rather than startup-time static config snapshots.
- The dashboard's default namespace view MUST reflect the current runtime config, upstream availability, and hook state after admin writes are applied.

### 2.8 Namespace Support

The proxy MUST support namespace-prefixed routes for multi-tenant deployments:

- `/namespaces/:namespace/openai/v1/...`
- `/namespaces/:namespace/anthropic/v1/...`
- `/namespaces/:namespace/google/v1beta/...`

Runtime configuration per namespace via admin API:
- `POST /admin/namespaces/:namespace/config`
- `GET /admin/state`
- `GET /admin/namespaces/:namespace/state`

OpenAI Responses lifecycle routes MUST select only upstreams that are both:

- available
- natively support OpenAI Responses

Selection rules:

- 0 matches => `503 Service Unavailable`
- 1 match => use that upstream
- multiple matches => `400 Bad Request` ambiguous

`GET /openai/v1/responses/:response_id?stream=true` MUST be treated as a native streaming resource request. The proxy MUST forward it only to the selected native OpenAI Responses upstream, send `Accept: text/event-stream`, stream the upstream SSE response through the public-boundary guard, and fail closed if a successful upstream response is not SSE.

### 2.9 Admin Control Plane

The proxy MUST expose a separate admin control plane with these properties:

- Admin routes are structurally separated from data-plane routes.
- Admin routes MUST NOT inherit the proxy's global data-plane CORS policy.
- If `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` is set, admin access MUST require `Authorization: Bearer <token>`.
- If `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` is set to an empty or whitespace-only value, admin auth MUST be treated as misconfigured and admin requests MUST fail closed.
- Bearer token parsing MUST accept `Bearer ` and `bearer `, but MUST reject empty or whitespace-only bearer tokens.
- If `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` is not set, admin access MUST be restricted to loopback clients only.
- In loopback-only mode, admin requests carrying explicit proxy forwarding headers such as `Forwarded`, `X-Forwarded-For`, `X-Forwarded-Host`, `X-Forwarded-Proto`, or `X-Real-IP` MUST be rejected.
- Admin read endpoints MUST use a dedicated read/view model rather than serializing the internal runtime `Config`.
- Admin state responses MUST redact secrets and MUST NOT return upstream `fallback_credential_actual` or hook `authorization` values in plaintext.
- Admin state responses SHOULD expose non-secret presence signals instead, such as whether a fallback credential or hook authorization is configured.
- Admin state responses MUST sanitize URLs before returning them and MUST NOT return userinfo.
- Admin state URLs are sanitized display values and MUST NOT include userinfo, query, or fragment components.
- Config validation MUST reject upstream `api_root` values or hook URLs that contain userinfo.
- Admin state responses MUST NOT return sensitive static upstream header values in plaintext.
- Header redaction MUST use a conservative rule set that covers at least `authorization`, `proxy-authorization`, `cookie`, `set-cookie`, and header names containing `token`, `secret`, `credential`, `api-key`, or `apikey`.

### 2.10 Admin Config CAS Semantics

Admin namespace writes MUST use server-owned revisions with exact compare-and-swap semantics.

- Primary request shape: `{ "if_revision"?: string | null, "config": ... }`
- Successful response shape remains `{ "namespace": string, "revision": string, "status": "applied" }`
- The returned `revision` MUST be generated by the server for every successful write.
- `GET /admin/state` and `GET /admin/namespaces/:namespace/state` MUST continue returning the current server revision.
- If the namespace does not exist, create is allowed only when `if_revision` is omitted or `null`.
- If the namespace already exists, `if_revision` MUST be present and MUST exactly match the current revision.
- CAS failures MUST return `412 Precondition Failed` and MUST include machine-readable `current_revision`.
- The legacy write body `{ "revision": string, "config": ... }` is no longer supported and MUST return `400 Bad Request`.
- Requests that contain a `revision` field in the write body, whether alone or alongside `if_revision`, MUST return `400 Bad Request`.
- Startup-created namespaces, including `default`, MUST also participate in server-owned revision/CAS behavior; the implementation MUST NOT rely on string ordering special cases such as `startup`.

---

## 3. Non-Functional Requirements

### 3.1 Performance

| Requirement | Target |
|-------------|--------|
| Passthrough latency overhead | < 1ms added latency |
| Translation latency overhead | < 10ms added latency per request |
| Streaming chunk translation | < 1ms per chunk |
| Concurrent requests | Support 100+ concurrent requests without degradation |
| Memory per request | Bounded; no unbounded accumulation during streaming |

### 3.2 Reliability

- The proxy MUST NOT crash on malformed requests (return 4xx instead).
- The proxy MUST NOT crash on upstream failures (return 502 instead).
- Hook delivery failures MUST NOT affect request serving (circuit breaker pattern).
- The proxy MUST handle upstream timeouts gracefully (configurable timeout).

### 3.3 Compatibility

- Compatible with HTTP clients that speak one of the four supported protocol surfaces and can use a configured proxy base URL.
- No SDK changes are required for portable request/response fields; provider-native extensions may require same-provider paths or a documented shim.
- Works with official vendor APIs and third-party compatible endpoints within documented portability boundaries.

### 3.4 Deployment

- Single static binary (Rust, no runtime dependencies).
- YAML configuration file.
- Docker support via multi-stage build.
- Cross-platform: Linux (primary), macOS, Windows.

---

## 4. Protocol Translation Architecture

### 4.1 Pivot Format

OpenAI Chat Completions serves as the canonical intermediate format. All cross-protocol translations go through two steps:

```
Source → OpenAI Chat Completions → Target
```

This reduces the translation matrix from O(N²) to O(N) — adding a new protocol requires only two bidirectional translators (new ↔ OpenAI), not N translators.

### 4.2 Translation Pipeline

```
1. Client sends request in format A
2. Detect client format from path + body
3. Resolve model → upstream + real model name
4. If client format == upstream format → passthrough
5. Otherwise:
   a. Translate request: A → OpenAI Chat → B
   b. Send translated request to upstream
   c. Receive response from upstream
   d. Translate response: B → OpenAI Chat → A
   e. Return translated response to client
```

### 4.3 Streaming Translation

For SSE streaming, each chunk is translated individually:

```
1. Upstream sends SSE chunk in format B
2. Parse chunk according to format B semantics
3. Translate to OpenAI Chat delta format
4. Translate from OpenAI Chat delta to client format A
5. Emit translated SSE event to client
```

Stateful accumulators track message IDs, tool call IDs, content buffers, and finish reasons across chunks.

---

## 5. Supported Upstream Types

### 5.1 Official Vendor APIs

| Upstream | Protocol | Example |
|----------|----------|---------|
| OpenAI | OpenAI Responses + Chat Completions | `api.openai.com` |
| Anthropic | Anthropic Messages | `api.anthropic.com` |
| Google | Google Gemini | `generativelanguage.googleapis.com` |

### 5.2 Third-Party Compatible Endpoints

| Upstream | Primary Protocol | Notes |
|----------|-----------------|-------|
| GLM (Zhipu) | Anthropic-compatible + OpenAI-compatible | `open.bigmodel.cn` |
| MiniMax | Anthropic-compatible + OpenAI-compatible | `api.minimaxi.com` |
| Kimi (Moonshot) | OpenAI-compatible | `api.moonshot.cn` |
| DeepSeek | OpenAI-compatible | `api.deepseek.com` |
| Mistral | OpenAI-compatible | `api.mistral.ai` |

### 5.3 Self-Hosted Endpoints

| Upstream | Protocol | Notes |
|----------|----------|-------|
| vLLM | OpenAI Chat Completions | Common for serving local models |
| Ollama | OpenAI-compatible | Local model runtime |
| llama.cpp server | OpenAI-compatible | Local inference |
| Local model on custom port | OpenAI-compatible | e.g., qwen3.5-9b-awq at `http://192.168.0.220:9997/v1` |

---

## 6. Test Configuration

### 6.1 Test Environments

The proxy MUST be tested against the following real upstream endpoints:

| Name | Endpoint URL | Protocol | Model | Context |
|------|-------------|----------|-------|---------|
| MiniMax (Anthropic) | `https://api.minimaxi.com/anthropic/v1` | Anthropic Messages | `MiniMax-M2.7-highspeed` | 204,800 tokens |
| MiniMax (OpenAI) | `https://api.minimaxi.com/v1` | OpenAI Chat Completions | `MiniMax-M2.7-highspeed` | 204,800 tokens |
| Local qwen3.5-9b-awq | `http://192.168.0.220:9997/v1` | OpenAI Chat Completions | `qwen3.5-9b-awq` | 16,384 tokens |

### 6.2 Test Matrix

For each upstream, test all applicable client entrypoints:

| Client Entry | MiniMax Anthropic | MiniMax OpenAI | Local qwen3.5 |
|-------------|-------------------|----------------|---------------|
| `/openai/v1/chat/completions` (non-stream) | Translate | Passthrough | Passthrough |
| `/openai/v1/chat/completions` (stream) | Translate | Passthrough | Passthrough |
| `/openai/v1/responses` (non-stream) | Translate | Translate | Translate |
| `/openai/v1/responses` (stream) | Translate | Translate | Translate |
| `/anthropic/v1/messages` (non-stream) | Passthrough | Translate | Translate |
| `/anthropic/v1/messages` (stream) | Passthrough | Translate | Translate |
| `/google/v1beta/models/:id` (non-stream) | Translate | Translate | Translate |
| `/google/v1beta/models/:id` (stream) | Translate | Translate | Translate |

### 6.3 Client Tool Tests

| Client Tool | Client Protocol | Test Command |
|-------------|----------------|-------------|
| Codex CLI | OpenAI Responses | `codex exec --ephemeral` with proxy base URL |
| Claude Code | Anthropic Messages | `claude --print` with proxy base URL |
| Gemini CLI | Google Gemini | `gemini --prompt` with proxy base URL |
| curl (OpenAI) | OpenAI Chat Completions | Direct HTTP request |
| curl (Anthropic) | Anthropic Messages | Direct HTTP request |

Current real-client regression coverage is intentionally narrow: smoke cases assert public tool identity as a per-client public editing tool contract by requiring Codex `apply_patch`, Claude Code `Edit`, or Gemini `replace` on the matching client surface, rejecting other clients' public tool names, and keeping `__llmup_custom__*` absent from public output. Long-horizon cases validate workspace-edit execution on supported lanes. This is not yet a full matrix of arbitrary structured tool behavior.

---

## 7. Compatibility and Degradation Policy

### 7.1 Translation Fidelity Levels

| Level | Meaning | Example |
|-------|---------|---------|
| **Exact** | Semantics preserved closely enough for identical downstream behavior | Text content, basic tool calls |
| **Approximate** | Primary behavior preserved but wire shape differs | Tool choice mapping, cached tokens |
| **Dropped** | No safe mapping exists; field is omitted | `previous_response_id`, built-in tools |

### 7.2 Degradation Signaling

When the proxy must drop or approximate fields, it MUST:
1. Emit `x-proxy-compat-warning` response headers
2. Log the degradation to server logs
3. NOT silently pretend 1:1 fidelity

### 7.3 Known Limitations

| Limitation | Reason |
|-----------|--------|
| `previous_response_id` (Responses) | Stateful chaining cannot be reconstructed in stateless formats |
| Responses lifecycle routing | Retrieve/delete/cancel requests do not carry a routable model, and the proxy does not persist response-to-upstream session state |
| Built-in tools (web search, etc.) | Not portable across protocol schemas |
| `truncation` policy | Provider-specific context management |
| Reasoning request config | Only reasoning output is mapped, not request policy |
| `store` / persistence | Provider-specific outside OpenAI family |
| Conflicting typed-media MIME hints | Unsafe request semantics; reject before upstream routing instead of guessing |

---

## 8. Future Considerations

These are NOT in scope for the current version but inform architectural decisions:

| Area | Consideration |
|------|--------------|
| Additional protocols | Ollama native, AWS Bedrock, Azure OpenAI |
| Batch processing | Translation for batch/non-real-time API calls |
| Embeddings | Protocol translation for embedding endpoints |
| Content moderation | Cross-protocol content filtering |
| A/B routing | Route different requests to different upstreams based on policy |
| Caching | Response caching for identical requests |

---

## 9. Success Metrics

| Metric | Target |
|--------|--------|
| All 16 protocol combinations within documented portability boundaries | 16/16 assessed as pass, warn, or reject as specified |
| Streaming works for all combinations | 16/16 pass |
| Codex CLI works through configured proxy lanes | Required test upstreams pass within the wrapper surface contract |
| Claude Code works through configured proxy lanes | Required test upstreams pass within the wrapper surface contract |
| Passthrough adds < 1ms latency | Measured |
| No silent data loss during translation | All compat warnings are emitted correctly |
