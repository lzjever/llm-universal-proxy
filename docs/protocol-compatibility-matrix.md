# Protocol Compatibility Matrix

- Status: active summary
- Last refreshed: 2026-04-19
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

The proxy should treat function calling and explicit transcript replay as the common denominator. Hosted tools, provider-managed state, compaction, and cache-control semantics are increasingly vendor-specific and should be preserved only on same-provider paths or documented as intentional degradations.

- First-phase multimodal support is bounded by request-policy gating and the effective `surface.modalities.input` for the selected alias. `pdf` means PDF-only, `file` means generic files including PDFs, and `video` is currently gate-first rather than a broad cross-provider translation promise.
- OpenAI/Responses to Anthropic currently translates data URI images, but remote images, audio, file parts, and unknown typed parts fail closed; OpenAI/Gemini image, audio, PDF, and `fileData` mappings remain the supported first-phase media lane.
- OpenAI `file` and Responses `input_file` MIME provenance must be self-consistent. Conflicting explicit MIME metadata, MIME-bearing data URIs, or filename hints fail closed instead of allowing the request-policy gate and downstream translator to disagree.
- Reasoning text and continuity should be preserved where possible, but request-side reasoning knobs remain vendor-specific. Translation should keep the model-visible reasoning trail when portable without implying that all providers expose equivalent control surfaces.
- Replayable tool history requires a complete and trusted structured call. Non-replayable or truncated tool calls should intentionally degrade to text/context preservation rather than masquerade as valid structured replay across providers.
- Unsupported media, unknown typed parts, and Gemini video routed to non-Gemini targets should fail closed before contacting the upstream rather than being silently dropped.
