# Protocol Compatibility Audit — 2026-04-07

- Status: retired historical audit
- Proxy posture: this file records the pre-removal state captured on 2026-04-07. It is not an active protocol baseline or support commitment. Native Gemini `generateContent` / `streamGenerateContent` wire-format support and `/google/v1beta/*` proxy routes have been removed; Gemini models should use Google's OpenAI-compatible endpoint with `format: openai-completion`.

## Scope

This retired audit compared the then-current codebase against the latest official documentation available on 2026-04-07 for:

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

Historical note: the bullets below describe repository behavior at the time of this retired audit, before native Gemini wire-format support was removed. They are not active support claims.

At the time, the proxy was in good shape as a compatibility-forwarding layer:

- OpenAI Chat create is supported.
- OpenAI Responses create and lifecycle routes are supported.
- Anthropic Messages create is supported.
- Gemini `generateContent` and `streamGenerateContent` were supported.
- Gemini request parsing accepted both official camelCase and shell-style snake_case part keys.
- OpenAI tool-result translation preserved the real Gemini `functionResponse.name`.

The main remaining boundary is intentional rather than a missing implementation:

- the proxy does not invent response-to-upstream session state
- stateful Responses features only work when the current request still carries enough routing information
- lifecycle routes fail clearly if the proxy cannot uniquely determine a native Responses upstream

That was consistent with the product's role as a protocol translation proxy rather than a session persistence layer.

## What matched well at the time

### 1. Core endpoint families recorded in this retired audit

- `POST /openai/v1/chat/completions`
- `POST /openai/v1/responses`
- `GET /openai/v1/responses/{response_id}`
- `DELETE /openai/v1/responses/{response_id}`
- `POST /openai/v1/responses/{response_id}/cancel`
- `POST /openai/v1/responses/compact`
- `POST /anthropic/v1/messages`
- Retired native Gemini route: `POST /google/v1beta/models/{model}:generateContent`
- Retired native Gemini route: `POST /google/v1beta/models/{model}:streamGenerateContent`

### 2. Translation and streaming behavior

- OpenAI/Anthropic/Gemini request and response translation was covered.
- Responses child events include `response_id` for downstream client compatibility.
- SSE parsing tolerates both `\n\n` and `\r\n\r\n`.
- gzip-compressed upstream OpenAI responses are covered by integration tests.

### 3. Compatibility signaling

- The proxy emits `x-proxy-compat-warning` headers when it must drop or approximate protocol-specific fields.
- Responses-only fields such as `previous_response_id`, `truncation`, `include`, and non-function tools are explicitly signaled during cross-protocol degradation.

## Boundaries recorded in this retired audit

### 1. Responses state continuity is not reconstructed by the proxy

Severity: medium

The proxy forwards official Responses lifecycle routes, but it does not persist `response_id -> upstream` state. That means:

- `POST /responses` must still be routable from the current request's `model` or alias
- lifecycle routes only succeed when the current request context uniquely identifies a native Responses upstream
- `previous_response_id` does not let the proxy recover an earlier upstream selection by itself

This is an intentional boundary, not a hidden best-effort feature.

### 2. Responses-only request semantics remain a compatibility subset when leaving Responses

Severity: low

At the time, some official Responses fields were not portable to Chat, Anthropic, or Gemini request formats:

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

## Historical release conclusion

Retired release posture recorded on 2026-04-07: suitable for release as a compatibility-forwarding proxy.

This conclusion is superseded by the later removal of native Gemini wire-format support. The current active proxy posture is documented in the maintained protocol baselines and compatibility matrix.
