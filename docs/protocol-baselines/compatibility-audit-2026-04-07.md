# Protocol Compatibility Audit — 2026-04-07

## Scope

This audit compares the current codebase against the latest official documentation for:

- OpenAI Chat Completions
- OpenAI Responses
- Anthropic Messages
- Google Gemini `generateContent` / `streamGenerateContent`

Raw snapshots are stored under `docs/protocol-baselines/snapshots/2026-04-07/`.

## Source set

- OpenAI Responses:
  - `https://platform.openai.com/docs/api-reference/responses`
  - `https://developers.openai.com/api/reference/resources/responses`
- OpenAI Chat:
  - `https://platform.openai.com/docs/api-reference/chat/create`
  - `https://developers.openai.com/api/reference/resources/chat`
- Anthropic Messages:
  - `https://platform.claude.com/docs/en/api/messages`
- Gemini:
  - `https://ai.google.dev/api/generate-content`

## Executive summary

The proxy is now in good shape as a compatibility-forwarding layer:

- OpenAI Chat create is supported.
- OpenAI Responses create and lifecycle routes are supported.
- Anthropic Messages create is supported.
- Gemini `generateContent` and `streamGenerateContent` are supported.
- Gemini request parsing now accepts both official camelCase and shell-style snake_case part keys.
- OpenAI tool-result translation now preserves the real Gemini `functionResponse.name`.

The main remaining boundary is intentional rather than a missing implementation:

- the proxy does not invent response-to-upstream session state
- stateful Responses features only work when the current request still carries enough routing information
- lifecycle routes fail clearly if the proxy cannot uniquely determine a native Responses upstream

That is consistent with the product's role as a protocol translation proxy rather than a session persistence layer.

## What matches well

### 1. Core endpoint families

- `POST /openai/v1/chat/completions`
- `POST /openai/v1/responses`
- `GET /openai/v1/responses/{response_id}`
- `DELETE /openai/v1/responses/{response_id}`
- `POST /openai/v1/responses/{response_id}/cancel`
- `POST /openai/v1/responses/compact`
- `POST /anthropic/v1/messages`
- `POST /google/v1beta/models/{model}:generateContent`
- `POST /google/v1beta/models/{model}:streamGenerateContent`

### 2. Translation and streaming behavior

- OpenAI/Anthropic/Gemini request and response translation is covered.
- Responses child events include `response_id` for downstream client compatibility.
- SSE parsing tolerates both `\n\n` and `\r\n\r\n`.
- gzip-compressed upstream OpenAI responses are covered by integration tests.

### 3. Compatibility signaling

- The proxy emits `x-proxy-compat-warning` headers when it must drop or approximate protocol-specific fields.
- Responses-only fields such as `previous_response_id`, `truncation`, `include`, and non-function tools are explicitly signaled during cross-protocol degradation.

## Current boundaries

### 1. Responses state continuity is not reconstructed by the proxy

Severity: medium

The proxy forwards official Responses lifecycle routes, but it does not persist `response_id -> upstream` state. That means:

- `POST /responses` must still be routable from the current request's `model` or alias
- lifecycle routes only succeed when the current request context uniquely identifies a native Responses upstream
- `previous_response_id` does not let the proxy recover an earlier upstream selection by itself

This is an intentional boundary, not a hidden best-effort feature.

### 2. Responses-only request semantics remain a compatibility subset when leaving Responses

Severity: low

Some official Responses fields are still not portable to Chat, Anthropic, or Gemini request formats:

- `previous_response_id`
- `truncation`
- `max_tool_calls`
- `include`
- `reasoning`
- `prompt_cache_key`
- non-function built-in tools

The proxy keeps function tools and portable fields, and emits compatibility warnings for the rest.

### 3. Practical OpenAI-compatible support is stronger than provider-specific certification

Severity: low

The implementation is robust for broad OpenAI-style upstreams such as vLLM-like servers because it already handles:

- CRLF SSE framing
- `[DONE]`
- gzip responses
- Chat and Responses stream normalization

But the repository still does not claim provider-certified integration for every OpenAI-like server. Compatibility should be described as broad OpenAI-compatible forwarding, not as formal certification for any specific third-party gateway unless separately tested.

## Release conclusion

Release posture: suitable for release as a compatibility-forwarding proxy.

The codebase now aligns well with the latest official protocol surfaces that it chooses to support. The remaining limitations are documented, tested as explicit boundaries, and consistent with the proxy's intended scope.
