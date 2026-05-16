# Protocol Compatibility Matrix

- Status: active summary
- Vendor snapshot/captured date: 2026-04-16
- Latest online recheck date: 2026-05-16
- Proxy posture updated date: 2026-04-26
- Scope: short entrypoint into the detailed compatibility docs

This file is now the short compatibility front door. Detailed provider comparisons live under [`protocol-baselines/matrices/`](protocol-baselines/matrices/) and the dated refresh audit lives under [`protocol-baselines/audits/`](protocol-baselines/audits/).

Status cells in the quick matrix answer whether the capability is officially documented on that provider surface. Portability and downgrade guidance live in the linked detailed docs.

## Quick matrix

| Capability | OpenAI Responses | OpenAI Chat | Anthropic Messages | Where to read more |
| --- | --- | --- | --- | --- |
| Core text conversation | Native | Native | Native | [`overview.md`](protocol-baselines/overview.md) |
| Typed multimodal input | Native | Native | Native | [`provider-capability-matrix.md`](protocol-baselines/matrices/provider-capability-matrix.md) |
| Function calling | Native | Native | Native | [`tools.md`](protocol-baselines/capabilities/tools.md) |
| Hosted / server tools | Native | No official surface | Native | [`tools.md`](protocol-baselines/capabilities/tools.md) |
| Reasoning controls and output | Native | Limited | Native | [`reasoning.md`](protocol-baselines/capabilities/reasoning.md) |
| Prompt / context caching | Native | Native | Native | [`cache.md`](protocol-baselines/capabilities/cache.md) |
| Streaming delivery | Native | Native | Native | [`streaming.md`](protocol-baselines/capabilities/streaming.md) |
| Rich typed streaming lifecycle | Native | Limited | Native | [`streaming.md`](protocol-baselines/capabilities/streaming.md) |
| Provider-managed conversation state | Native | No official surface | Guide/Beta | [`state-continuity.md`](protocol-baselines/capabilities/state-continuity.md) |

## Detailed docs

| Need | Doc |
| --- | --- |
| One-page provider comparison | [`protocol-baselines/matrices/provider-capability-matrix.md`](protocol-baselines/matrices/provider-capability-matrix.md) |
| High-risk field mappings | [`protocol-baselines/matrices/field-mapping-matrix.md`](protocol-baselines/matrices/field-mapping-matrix.md) |
| Vendor-specific facts | [`protocol-baselines/README.md`](protocol-baselines/README.md) |
| Latest refresh and implementation risks | [`protocol-baselines/audits/2026-05-16-online-recheck.md`](protocol-baselines/audits/2026-05-16-online-recheck.md) |

## Current compatibility posture

The GA claim is maximum safe compatibility across the active protocol set, within
hard portability boundaries. Raw same-protocol passthrough is the intended
pre-GA execution lane for preserving provider-native fields and lifecycle
resources when no body mutation or response normalization is required. Until
that lane lands, same-protocol routes may still pass through compatibility
machinery. Cross-provider documented translation/fail-closed behavior is the
portability contract: the proxy documents supported mappings and rejects
high-risk unsupported fields before contacting upstream. Compatible providers can satisfy live GA evidence by
exposing the OpenAI-compatible chat-completions route `/openai/v1/chat/completions`
and the Anthropic-compatible messages route `/anthropic/v1/messages`. MiniMax is
only one example compatible lane, not an OpenAI Responses certified clone and
not a GA-required provider.

Compatibility is not tiered as a product behavior. Same-format routes should use raw passthrough when the proxy can avoid body mutation and response normalization, and translated routes should use maximum safe compatibility. A same-protocol route that requires shims, model body rewrites, or response normalization is an implementation translation lane rather than raw native passthrough.

The proxy should treat function calling and explicit transcript replay as the common denominator. Hosted tools, provider-managed state, compaction, and cache-control semantics are increasingly vendor-specific and should be preserved only on raw/native passthrough lanes or documented as intentional degradations. Raw/native passthrough is the intended lane for preserving opaque provider state when the route remains native and avoids mutation.

- First-phase multimodal support is bounded by request-policy gating and the effective `surface.modalities.input` for the selected alias. `pdf` means PDF-only, `file` means generic files including PDFs, and `video` is currently gate-first rather than a broad cross-provider translation promise. These values are media-type gates, not source transport guarantees.
- OpenAI Chat/Responses to Anthropic translates data URI images to base64 image sources, HTTP(S) image URLs to Anthropic URL image sources, PDF data URIs to base64 document sources, and PDF HTTP(S) `file_data` / `file_url` references to URL document sources when PDF MIME or filename provenance is available.
- OpenAI Chat/Responses to Anthropic rejects `input_audio`, non-PDF or generic files, unknown typed parts, provider `file_id`, and provider-native or local URIs such as `gs://`, `file://`, or `s3://` before contacting upstream.
- OpenAI `file` and Responses `input_file` MIME provenance must be self-consistent. Conflicting explicit MIME metadata, MIME-bearing data URIs, or filename hints fail closed instead of allowing the request-policy gate and downstream translator to disagree.
- Reasoning text and continuity should be preserved where possible, but request-side reasoning knobs remain vendor-specific. Translation should keep the model-visible reasoning trail when portable without implying that all providers expose equivalent control surfaces. Opaque reasoning carriers such as `reasoning.encrypted_content` and reasoning item `encrypted_content` are raw/native passthrough only. Cross-provider translation may warn/drop carriers only when visible summary text or visible transcript/history remains. Opaque-only reasoning state always fails closed.
- Response-side reasoning encrypted_content has a dedicated Anthropic carrier recovery path. The request-side continuity rules above are not a blanket rule for all response translation.
- Cross-provider request translation fails closed for high-risk provider-state, safety, and reasoning fields that the proxy cannot faithfully replay: OpenAI Responses provider `previous_response_id`, `conversation` / `context_management`, native compact resources, and opaque-only reasoning or compaction state; Anthropic top-level `thinking` / `context_management`; and Anthropic signed, omitted, or redacted thinking blocks. Raw/native passthrough is the intended lane for native provider fields when the route avoids mutation. The optional `conversation_state_bridge.mode=memory` path is narrower: non-streaming text-only translated Responses requests may save and replay llmup-owned `resp_llmup_*` transcript state, while `store:false`, unknown/expired IDs, and external provider IDs still fail closed for replay.
- Provider prompt-cache optimization is target-provider request synthesis, not cross-provider state reconstruction: a policy-enabled translated route may add OpenAI `prompt_cache_key` or Anthropic top-level `cache_control`, but it must not fabricate unrelated provider resources or treat provider cache controls as semantically identical.
- `context_management`, compact resources, and provider-native state-control surfaces remain raw/native only and fail closed for cross-provider reconstruction. Request-side compaction input items do not forward `encrypted_content` or other opaque state across providers. A compaction item may warn/drop opaque carrier fields only when that item has explicit visible summary text, or when the request contains non-compaction visible portable transcript/history. Opaque-only compaction always fails closed, and one summarized compaction item does not permit another opaque-only compaction item to be silently dropped. Native Responses passthrough should preserve the native item unchanged when the raw/native lane is available.
- OpenAI Responses lifecycle and state resource endpoints, including response input items, input token counting, conversations, and conversation items, target raw/native passthrough only when implemented and the route can avoid mutation. They require one available native OpenAI Responses upstream in the selected namespace and fail closed for no upstream, multiple candidate upstreams, unavailable upstreams, or cross-provider state reconstruction.
- Anthropic `redacted_thinking` and thinking blocks that rely on provider signatures or omitted/non-string `thinking` payloads are not represented by a cross-provider standard in this compatibility layer; cross-protocol request support remains fail-closed unless a future explicit mapping is designed and documented.
- Replayable tool history requires a complete and trusted structured call. Non-replayable or truncated tool calls should intentionally degrade to text/context preservation rather than masquerade as valid structured replay across providers.
- Unsupported media, unsupported source transports, and unknown typed parts should fail closed before contacting the upstream rather than being silently dropped.

Gemini remains usable as a Google OpenAI-compatible upstream by configuring `api_root: https://generativelanguage.googleapis.com/v1beta/openai` with `format: openai-completion`. Native Gemini `generateContent` is retired from the active proxy compatibility surface.
