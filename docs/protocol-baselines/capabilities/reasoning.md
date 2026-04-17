# Reasoning Capability Notes

- Layer: capability-diff
- Status: active
- Last refreshed: 2026-04-16
- Scope: request knobs, reasoning output shapes, and portability constraints

## Summary

Reasoning is no longer just a model trait. It now affects request policy, streamed event families, usage accounting, and sometimes resumability. The proxy can safely preserve only a subset of that surface across providers.

## Provider comparison

| Dimension | OpenAI Responses | OpenAI Chat Completions | Anthropic Messages | Gemini `generateContent` | Proxy guidance |
| --- | --- | --- | --- | --- | --- |
| Request control | Native `reasoning` config on supported models; official docs also expose `include` values like `reasoning.encrypted_content` | Reasoning controls remain model-specific and Chat-oriented, not a typed reasoning object family | `thinking` is an explicit request object with token-budget semantics | `thinkingConfig` is part of generation config on supported models | Preserve vendor-native reasoning config only on passthrough paths. Cross-protocol translation should treat request knobs as non-portable. |
| Response representation | Reasoning can appear as typed output items plus summary material | Usually flattened into assistant output or model-specific side channels | Returned as `thinking` content blocks | Returned through candidate content plus usage metadata for thought tokens | Treat summarized text as the portability floor. Do not promise structural round-trip fidelity. |
| Encrypted / opaque reasoning state | Official docs now enumerate `reasoning.encrypted_content` in `include` | No stable equivalent | No stable equivalent in the public Messages wire shape | No stable equivalent | Drop or warn when leaving Responses. Do not synthesize fake encrypted state. |
| Usage accounting | `output_tokens_details.reasoning_tokens` | `completion_tokens_details.reasoning_tokens` on supported models | Thinking budget counts toward `max_tokens`, but token accounting is not isomorphic | `usageMetadata.thoughtsTokenCount` | Preserve provider-native counters, but document them as approximate when normalized. |
| Streaming behavior | Rich reasoning-aware event family in Responses streaming | Chat streams deltas, not typed reasoning items | Thinking arrives through block events | Streaming shape is candidate-based, not item-based | Streaming adapters should preserve "reasoning happened" and token totals, not exact event taxonomy. |

## Main portability boundaries

| Boundary | Why it matters |
| --- | --- |
| Request policy is vendor-specific | OpenAI `reasoning`, Anthropic `thinking`, and Gemini `thinkingConfig` differ in both syntax and semantics. |
| Opaque reasoning state is non-portable | OpenAI's encrypted reasoning content has no safe Anthropic or Gemini target shape. |
| Token counters are similar but not identical | Developers often compare reasoning token counts operationally, but the providers do not guarantee the same accounting model. |

## Implementation stance

1. Preserve reasoning request knobs only when client and upstream are the same protocol family.
2. Preserve summarized reasoning text and usage counters when possible.
3. Treat encrypted reasoning state, detailed block structure, and provider-specific effort controls as unsupported across protocol boundaries.
