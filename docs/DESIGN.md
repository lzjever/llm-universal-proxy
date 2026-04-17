# LLM Universal Proxy — Design

## Goal

Single-binary HTTP proxy that:

1. **Separated planes** — One data plane for client traffic (for example `http://localhost:8080/openai/v1/...`) and one admin control plane under `http://localhost:8080/admin/...`.
2. **Four client request formats** — Clients can send requests in any of:
   - **Google (Gemini)** — e.g. `/google/v1beta/models/:id`, `contents[]`, `generateContent`-style.
   - **Anthropic (Claude)** — e.g. `/anthropic/v1/messages`, `messages[]` with content blocks, `system`.
   - **OpenAI Chat Completions** — `/openai/v1/chat/completions`, `messages[]`, `stream`, `temperature`, etc.
   - **OpenAI Responses API** — `/openai/v1/responses`, `input[]`, `instructions`.
3. **Upstream formats** — The proxy connects to one **upstream** base URL. The upstream may support one or more of the four formats; the proxy **discovers** which formats are supported and uses the **most generic** as the default conversion target when translation is needed.
4. **Concurrency** — The proxy must support **concurrent requests**. Handlers are async and non-blocking; Axum’s default concurrency is used; no shared mutable state that would serialize requests.
5. **Passthrough** — If the client’s request format is **supported by the upstream**, the proxy forwards the request and response in that format (no translation). This reduces errors and improves efficiency. When the client format is not supported, the proxy translates to the default (most generic) upstream format.
6. **Streaming** — Must support streaming (SSE) in all cases: passthrough = pipe bytes; otherwise = translate stream chunks from upstream format to client format.
7. **Minimal loss** — When translating, preserve as much as possible: tool calls, thinking/reasoning, usage, finish reasons, and content structure.

### Admin control plane boundary

- Admin routes are independent from the data-plane router so that browser-facing data-plane middleware does not leak onto `/admin/...`.
- The data plane keeps the permissive global CORS layer; the admin plane does not inherit that CORS layer.
- Admin access policy is intentionally narrow:
  - if `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` is set, admin requests must present `Authorization: Bearer <token>`
  - otherwise admin is available only to loopback clients (`127.0.0.1` / `::1`)
- Admin writes keep the existing runtime config payload shape.
- Admin reads use a dedicated redacted view model:
  - upstream fallback credentials are exposed only as metadata such as `fallback_credential_env` and `fallback_credential_configured`
  - hook authorization headers are exposed only as `authorization_configured`
  - plaintext `fallback_credential_actual` and hook `authorization` values are never serialized in admin state responses

### Upstream format discovery and passthrough

- **Discovery**: The proxy probes the upstream (e.g. per-format path or minimal request) to determine which of the four formats are supported, and caches this set (e.g. at startup or on first request, with optional TTL).
- **Default conversion target**: Among the supported formats, the proxy chooses the **most generic** (e.g. OpenAI Chat Completions, then OpenAI Responses, then Anthropic, then Google) as the default. When the client sends a format not supported by upstream, the proxy translates request and response to this default.
- **Passthrough when client format is supported**: If the client sends format F and the upstream supports F, the proxy does **not** translate — it forwards the request in format F and returns the response in format F. This reduces translation errors and improves efficiency.

## Reference: 9router

Logic is inspired by the **9router** reference project (in this workspace at `for-reference-only/9router`):

- **Format detection**: `open-sse/translator/formats.js`, `open-sse/services/provider.js` (`detectFormat`, `detectFormatByEndpoint`).
- **Translation**: `open-sse/translator/index.js` — pivot via **OpenAI Chat Completions**; `translateRequest(source → openai → target)`, `translateResponse(target → openai → source)`.
- **Streaming**: `open-sse/handlers/chatCore/streamingHandler.js` — if same format, passthrough; else SSE transform that converts chunks (target → openai → source).
- **Request translators**: `open-sse/translator/request/*` (e.g. `openai-to-claude.js`, `openai-to-gemini.js`, `openai-responses.js`).
- **Response translators**: `open-sse/translator/response/*` (e.g. `claude-to-openai.js`, `gemini-to-openai.js`, `openai-responses.js`).

## Architecture

```
Client (any of 4 formats)  →  Data Plane  →  Upstream (supports one or more formats)
Admin client               →  Admin Plane →  Runtime namespace state/config
```

- **Detection**: From request path + body shape, infer **client format**.
- **Config**: **Upstream URL**; optional **UPSTREAM_FORMAT** (if set, no discovery; otherwise proxy discovers supported formats).
- **Discovery**: Proxy determines **supported upstream formats** and **default conversion target** (most generic among supported).
- **Request path**: The data plane is namespaced by protocol (`/openai/v1/...`, `/anthropic/v1/...`, `/google/v1beta/...`). The admin plane is `/admin/...`.

### Request flow

1. Parse body as JSON.
2. `client_format = detect(path, body)`.
3. **Upstream format** = client format if client format is in **supported set**, else **default conversion target**.
4. If client format == upstream format: **passthrough** (forward body as-is to upstream).
5. Else: `body' = translate_request(client_format → upstream_format, body)`; send `body'` to upstream.

### Response flow (non-streaming)

1. If passthrough: return upstream response as-is (status, headers, body).
2. Else: read full upstream JSON, `body_out = translate_response(upstream_format → client_format, body_in)`, return `body_out`.

### Response flow (streaming)

1. If passthrough: pipe upstream response stream to client (same Content-Type, e.g. `text/event-stream`).
2. Else: consume upstream SSE stream; for each chunk, parse by upstream format, convert to OpenAI-style chunk, then to client format; send converted chunks to client as SSE. Maintain streaming state (e.g. message id, tool_calls map, finish_reason) for multi-chunk semantics.

## Format matrix

| Client \ Upstream | Google | Anthropic | OpenAI Completion | OpenAI Responses |
|-------------------|--------|-----------|-------------------|------------------|
| Google            | Pass   | Trans     | Trans             | Trans            |
| Anthropic         | Trans  | Pass      | Trans             | Trans            |
| OpenAI Completion | Trans  | Trans     | Pass              | Trans            |
| OpenAI Responses  | Trans  | Trans     | Trans             | Pass             |

(Pass = passthrough; Trans = translate request + response.)

## Pivot format

Use **OpenAI Chat Completions** as the internal pivot (same as 9router):

- Request: `client_format → openai_completion → upstream_format`.
- Response: `upstream_format → openai_completion → client_format`.

This minimizes the number of translators (N formats → 2×(N-1) request/response mappers instead of N×(N-1)).

## Implementation notes (Rust)

- **Single binary**: One crate, `cargo build --release` → one executable.
- **Config**: Env or config file: `UPSTREAM_URL`, optional `UPSTREAM_FORMAT` (if set, skip discovery; else discover supported formats), `LISTEN`.
- **HTTP**: Axum (or similar) with one POST route; path can be `/v1/chat/completions` or `/v1/responses` (path used for detection).
- **Streaming**: Use `reqwest` with `.stream()` for upstream; `axum::response::sse` or body stream for client; transform stream in the middle when translating.
- **TDD**: Tests first for format detection, request translation, response translation, and integration (e.g. mock upstream, assert request/response shape).

## Out of scope (for v0)

- Authentication (API key injection, multiple keys).
- Multiple upstreams / routing by model.
- Usage/cost tracking.
- Combo/fallback chains.

Focus: **format conversion + single upstream + streaming + passthrough**.
