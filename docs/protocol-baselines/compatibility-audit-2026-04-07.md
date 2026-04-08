# Protocol Compatibility Audit — 2026-04-07

## Scope

This audit compares the current codebase against the latest official documentation for:

- OpenAI Chat Completions
- OpenAI Responses
- Anthropic Messages
- Google Gemini `generateContent` / `streamGenerateContent`

It also evaluates practical compatibility posture for OpenAI-like servers such as vLLM and Xinference.

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

Raw snapshots are stored under `docs/protocol-baselines/snapshots/2026-04-07/`.

## Executive summary

The proxy is strong on the core data plane:

- native OpenAI Chat create
- native OpenAI Responses create
- native Anthropic Messages create
- native Gemini `generateContent` / `streamGenerateContent`
- bidirectional message translation
- SSE translation
- several practical compatibility features such as CRLF SSE parsing and gzip handling

It is not fully spec-complete for all four official APIs.

The largest hard gap is OpenAI Responses surface coverage: the server currently exposes only `POST /openai/v1/responses`, while the current official Responses API also includes retrieval, delete, cancel, compact, and input-item helper endpoints.

There is also one meaningful translation-quality gap in OpenAI-to-Gemini tool-result mapping: Gemini `functionResponse.name` is currently populated from `tool_call_id`, which is not the same thing as the function name.

There is a second Gemini compatibility gap for translated requests: the Gemini-to-OpenAI conversion path currently reads camelCase keys like `inlineData`, `functionCall`, and `functionResponse`, but the current official Google shell examples also show snake_case JSON such as `inline_data`.

For vLLM and Xinference, the implementation is promising as an OpenAI-compatible proxy, but the repo does not yet show explicit Xinference-specific handling or integration tests. Compatibility is therefore best described as broad OpenAI-style compatibility, not proven full-provider conformance.

## What already matches well

### 1. Core endpoint families

- OpenAI Chat create route exists.
- OpenAI Responses create route exists.
- Anthropic Messages create route exists.
- Gemini model action routing accepts `:generateContent` and `:streamGenerateContent`.

Relevant code:

- `src/server.rs`

### 2. Stream translation robustness

- SSE parser accepts both `\n\n` and `\r\n\r\n`, which directly helps with vLLM/uvicorn-style streams.
- OpenAI `[DONE]` handling exists.
- Anthropic event families and Responses event families are translated with explicit state machines.

Relevant code:

- `src/streaming.rs`

### 3. Practical upstream compatibility

- gzip-compressed upstream OpenAI responses are tested.
- Anthropic `anthropic-version` injection is tested.
- context-window overflow is normalized across protocol families.
- Google streaming URLs use `:streamGenerateContent?alt=sse`, which aligns with real Gemini deployments.

Relevant code:

- `src/config.rs`
- `src/server.rs`
- `tests/integration_test.rs`

### 4. Compatibility signaling

- The server already emits `x-proxy-compat-warning` headers when a request uses fields that cannot be preserved cleanly across protocol families.
- This is especially useful for Responses-only features such as `previous_response_id`, `truncation`, `include`, and non-function tools.

Relevant code:

- `src/server.rs`

## Findings

### Finding 1: OpenAI Responses support is only partially spec-complete

Severity: medium

The official Responses API currently documents more than just create. The proxy only exposes:

- `POST /openai/v1/responses`

Missing official resource coverage includes at least:

- `GET /v1/responses/{response_id}`
- `DELETE /v1/responses/{response_id}`
- `POST /v1/responses/{response_id}/cancel`
- `POST /v1/responses/compact`
- input-item helper endpoints

Why this matters:

- The current implementation is compatible with create-style clients and streaming create calls.
- It is not fully conformant for SDKs or tools that rely on response lifecycle APIs.

Evidence:

- `src/server.rs:171-220`

### Finding 2: OpenAI tool results degrade when translated to Gemini

Severity: medium

When converting an OpenAI `tool` message to Gemini, the proxy currently emits:

- `functionResponse.id = tool_call_id`
- `functionResponse.name = tool_call_id`

But Gemini function responses are semantically keyed by function name, not by OpenAI call ID alone.

Why this matters:

- Some Gemini-compatible consumers may tolerate this.
- Others may require a correct function name for strict tool-call correlation.
- This is the biggest current cross-protocol fidelity gap in the translation layer.

Evidence:

- `src/translate.rs:2240-2258`

Recommended fix:

- Preserve function name earlier in the OpenAI tool-call lifecycle.
- Carry a hidden correlation map from tool call ID to function name.
- Emit Gemini `functionResponse.name` from that map instead of reusing `tool_call_id`.

### Finding 3: Native endpoint validation is intentionally lenient

Severity: low

The proxy acts mostly as a translating proxy, not a full schema validator.

Examples:

- Anthropic officially requires `max_tokens`, but the native Anthropic route does not appear to reject missing `max_tokens` before forwarding.
- Several protocol-specific optional fields are dropped or approximated during translation rather than hard-failed.

Why this matters:

- This is good for compatibility.
- It means "strict conformance" and "best-effort interoperability" are not the same thing in the current implementation.

Evidence:

- `src/translate.rs` defaults `max_tokens` to `4096` when translating OpenAI to Anthropic.
- `src/server.rs` emits compatibility warnings rather than strict validation failures.

### Finding 4: Responses-to-other-format translation is explicitly a compatibility subset

Severity: low

The code already acknowledges that some current Responses fields are not portable:

- `previous_response_id`
- `truncation`
- `max_tool_calls`
- `include`
- `reasoning`
- `prompt_cache_key`
- `store`
- non-function Responses tools

Why this matters:

- This is not a bug in itself.
- It means the proxy is not a full semantic superset of the latest Responses API when targeting Anthropic, Gemini, or OpenAI Chat upstreams.

Evidence:

- `src/server.rs:1514-1569`

### Finding 5: vLLM compatibility is real; Xinference compatibility is not yet proven

Severity: low

What is clearly present:

- CRLF SSE tolerance for vLLM/uvicorn-style streams
- OpenAI Chat-style routing and stream handling
- gzip response handling

What is missing:

- explicit `xinference` support logic
- explicit Xinference integration tests
- provider-specific handling for non-standard OpenAI-compatible quirks beyond the generic layer

Why this matters:

- The proxy likely works with many Xinference setups that adhere closely to OpenAI Chat.
- The repo does not currently justify claiming robust, provider-proven Xinference compatibility.

Evidence:

- `src/streaming.rs:123-147`
- `tests/integration_test.rs:363-389`
- no explicit `xinference` references found in the source tree during audit

### Finding 6: Gemini translation accepts only camelCase part keys

Severity: low

`convert_gemini_content_to_openai()` currently looks for:

- `inlineData`
- `functionCall`
- `functionResponse`

However, Google's current official shell examples also show snake_case request JSON like:

- `inline_data`
- `mime_type`

Why this matters:

- Native Gemini passthrough is unaffected when the upstream itself accepts those payloads.
- Translation from Gemini clients to OpenAI or Anthropic upstreams may silently lose image or tool parts if the client follows the shell-example naming.

Evidence:

- `src/translate.rs:2086-2112`
- `docs/protocol-baselines/snapshots/2026-04-07/google-gemini-generate-content.html:2583-2589`

## Overall assessment by protocol

### OpenAI Chat Completions

- Status: good
- Notes:
  - create endpoint supported
  - streaming shape supported
  - practical OpenAI-compatible compatibility is strongest here

### OpenAI Responses

- Status: partial
- Notes:
  - create endpoint supported
  - streaming create path supported
  - lifecycle sub-resources not yet implemented

### Anthropic Messages

- Status: good with translation caveats
- Notes:
  - native route exists
  - streaming event mapping is solid
  - strict validation is looser than the native spec
  - parallel tool control is approximated from OpenAI semantics

### Gemini generateContent

- Status: good with tool-response caveat
- Notes:
  - native route exists
  - streaming route exists
  - request/response translation is broadly correct
  - function-response naming fidelity is currently weak

## Recommended follow-up work

1. Add the missing OpenAI Responses lifecycle endpoints.
2. Fix OpenAI-to-Gemini tool-result naming by preserving function name across the OpenAI tool-call chain.
3. Add explicit compatibility tests for:
   - vLLM
   - Xinference
   - one strict Anthropic-compatible provider
   - one strict Gemini-compatible provider or mock with stricter tool-response validation
4. Decide whether native protocol routes should be:
   - strict validators
   - or permissive proxy surfaces with documented best-effort behavior
5. Extend baseline docs on every release or compatibility-focused change.
