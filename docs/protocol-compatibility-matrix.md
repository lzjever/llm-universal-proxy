# Protocol Compatibility Matrix

- Status: active summary
- Last refreshed: 2026-04-25
- Scope: short entrypoint into the detailed compatibility docs

This file is now the short compatibility front door. Detailed provider comparisons live under [`protocol-baselines/matrices/`](protocol-baselines/matrices/) and the dated refresh audit lives under [`protocol-baselines/audits/`](protocol-baselines/audits/).

Status cells in the quick matrix answer whether the capability is officially documented on that provider surface. Portability and downgrade guidance live in the linked detailed docs.

## Quick matrix

| Capability | OpenAI Responses | OpenAI Chat | Anthropic Messages | Gemini `generateContent` | Where to read more |
| --- | --- | --- | --- | --- | --- |
| Core text conversation | Native | Native | Native | Native | [`overview.md`](protocol-baselines/overview.md) |
| Typed multimodal input | Native | Native | Native | Native | [`provider-capability-matrix.md`](protocol-baselines/matrices/provider-capability-matrix.md) |
| Function calling | Native | Native | Native | Native | [`tools.md`](protocol-baselines/capabilities/tools.md) |
| Hosted / server tools | Native | No official surface | Native | Native | [`tools.md`](protocol-baselines/capabilities/tools.md) |
| Reasoning controls and output | Native | Limited | Native | Native | [`reasoning.md`](protocol-baselines/capabilities/reasoning.md) |
| Prompt / context caching | Native | Native | Native | Native | [`cache.md`](protocol-baselines/capabilities/cache.md) |
| Streaming delivery | Native | Native | Native | Native | [`streaming.md`](protocol-baselines/capabilities/streaming.md) |
| Rich typed streaming lifecycle | Native | Limited | Native | No official surface | [`streaming.md`](protocol-baselines/capabilities/streaming.md) |
| Provider-managed conversation state | Native | No official surface | Guide/Beta | No official surface | [`state-continuity.md`](protocol-baselines/capabilities/state-continuity.md) |

## Detailed docs

| Need | Doc |
| --- | --- |
| One-page provider comparison | [`protocol-baselines/matrices/provider-capability-matrix.md`](protocol-baselines/matrices/provider-capability-matrix.md) |
| High-risk field mappings | [`protocol-baselines/matrices/field-mapping-matrix.md`](protocol-baselines/matrices/field-mapping-matrix.md) |
| Vendor-specific facts | [`protocol-baselines/README.md`](protocol-baselines/README.md) |
| Latest refresh and implementation risks | [`protocol-baselines/audits/2026-04-16-spec-refresh.md`](protocol-baselines/audits/2026-04-16-spec-refresh.md) |

## Current compatibility posture

The GA claim is portable-core production GA. Same-provider native passthrough is
the path for preserving provider-native fields and lifecycle resources.
Cross-provider documented compatibility/fail-closed is the portability contract:
the proxy documents supported mappings and rejects high-risk unsupported fields
before contacting upstream. Compatible providers can satisfy live GA evidence by
exposing the OpenAI-compatible completions/chat-completions surface and the
Anthropic-compatible messages surface. MiniMax is only one example compatible
lane, not an OpenAI Responses certified clone and not a GA-required provider.

The proxy should treat function calling and explicit transcript replay as the common denominator. Hosted tools, provider-managed state, compaction, and cache-control semantics are increasingly vendor-specific and should be preserved only on same-provider paths or documented as intentional degradations. Same-provider/native passthrough preserves opaque provider state when the route remains native.

- First-phase multimodal support is bounded by request-policy gating and the effective `surface.modalities.input` for the selected alias. `pdf` means PDF-only, `file` means generic files including PDFs, and `video` is currently gate-first rather than a broad cross-provider translation promise. These values are media-type gates, not source transport guarantees.
- OpenAI Chat/Responses to Anthropic translates data URI images to base64 image sources, HTTP(S) image URLs to Anthropic URL image sources, PDF data URIs to base64 document sources, and PDF HTTP(S) `file_data` / `file_url` references to URL document sources when PDF MIME or filename provenance is available.
- OpenAI Chat/Responses to Anthropic rejects `input_audio`, non-PDF or generic files, unknown typed parts, provider `file_id`, and provider-native or local URIs such as `gs://`, `file://`, or `s3://` before contacting upstream.
- Anthropic remote image URLs to Gemini fail closed unless a future explicit fetch/upload adapter is documented.
- Gemini `inlineData` image, audio, and PDF content to OpenAI Chat/Responses remains supported. All Gemini `fileData.fileUri` sources, including HTTP(S), currently fail closed for OpenAI targets until an explicit fetch/upload adapter exists. OpenAI-supplied file URI or HTTP(S) file references can still map to Gemini `fileData` when MIME provenance is available.
- OpenAI `file` and Responses `input_file` MIME provenance must be self-consistent. Conflicting explicit MIME metadata, MIME-bearing data URIs, or filename hints fail closed instead of allowing the request-policy gate and downstream translator to disagree.
- Reasoning text and continuity should be preserved where possible, but request-side reasoning knobs remain vendor-specific. Translation should keep the model-visible reasoning trail when portable without implying that all providers expose equivalent control surfaces. Opaque reasoning carriers such as `reasoning.encrypted_content` and reasoning item `encrypted_content` are same-provider/native passthrough only. In default/max_compat cross-provider translation, the proxy may drop the opaque carrier without parsing or replaying it when visible summary text or visible message/tool history remains; strict and balanced modes fail closed, and opaque-only reasoning state still fails closed.
- Cross-provider request translation fails closed for high-risk provider-state, safety, and reasoning fields that the proxy cannot faithfully replay: OpenAI Responses `store: true`, stateful controls such as `previous_response_id` / `conversation` / `context_management`, native compact resources, and opaque-only reasoning or compaction state; Gemini `cachedContent` / `cached_content`, Gemini `safetySettings` / `safety_settings`, Gemini `thoughtSignature` / `thought_signature` anywhere in request content or history; Anthropic top-level `thinking` / `context_management`; and Anthropic signed, omitted, or redacted thinking blocks. Same-provider passthrough preserves native provider fields.
- `context_management`, compact resources, and provider-native state-control surfaces remain same-provider only and fail closed for cross-provider reconstruction. Request-side compaction input items do not forward `encrypted_content` or other opaque state across providers; default/max_compat may degrade to visible portable transcript or explicit visible summary context, while opaque-only compaction still fails closed. Native Responses passthrough preserves the native item unchanged.
- OpenAI Responses lifecycle and state resource endpoints, including response input items, input token counting, conversations, and conversation items, are same-provider native pass-through only. They require one available native OpenAI Responses upstream in the selected namespace and fail closed for no upstream, multiple candidate upstreams, unavailable upstreams, or cross-provider state reconstruction.
- OpenAI-to-Gemini tool-call translation does not synthesize Gemini thought signatures. Real provider `thoughtSignature` values are only preserved on Gemini passthrough, not fabricated during cross-protocol conversion.
- Anthropic `redacted_thinking` and thinking blocks that rely on provider signatures or omitted/non-string `thinking` payloads are not represented by a cross-provider standard in this compatibility layer; cross-protocol request support remains fail-closed unless a future explicit mapping is designed and documented.
- Replayable tool history requires a complete and trusted structured call. Non-replayable or truncated tool calls should intentionally degrade to text/context preservation rather than masquerade as valid structured replay across providers.
- Unsupported media, unsupported source transports, unknown typed parts, and Gemini video routed to non-Gemini targets should fail closed before contacting the upstream rather than being silently dropped.
