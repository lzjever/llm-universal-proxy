# Reasoning Capability Notes

- Layer: capability-diff
- Status: active
- Vendor snapshot/captured date: 2026-04-16
- Proxy posture updated date: 2026-04-26
- Scope: request knobs, reasoning output shapes, and portability constraints

## Summary

Reasoning is no longer just a model trait. It now affects request policy, streamed event families, usage accounting, and sometimes resumability. The proxy can safely preserve only a subset of that surface across providers.

## Provider comparison

| Dimension | OpenAI Responses | OpenAI Chat Completions | Anthropic Messages | Proxy guidance |
| --- | --- | --- | --- | --- |
| Request control | Native `reasoning` config on supported models; official docs also expose `include` values like `reasoning.encrypted_content` | Reasoning controls remain model-specific and Chat-oriented, not a typed reasoning object family | `thinking` is an explicit request object with token-budget semantics | Preserve vendor-native reasoning config only on passthrough paths. Cross-protocol translation should treat request knobs as non-portable. |
| Response representation | Reasoning can appear as typed output items plus summary material | Usually flattened into assistant output or model-specific side channels | Returned as `thinking` content blocks | Treat summarized text as the portability floor. Do not promise structural round-trip fidelity. |
| Encrypted / opaque reasoning state | Official docs now enumerate `reasoning.encrypted_content` in `include`; reasoning items can carry `encrypted_content` | No stable equivalent | No stable equivalent in the public Messages wire shape | Raw/native passthrough preserves the opaque carrier. For request-side continuity, maximum-compatible cross-provider translation may warn/drop the carrier only when visible summary text or visible transcript/history remains, and opaque-only reasoning fails closed. Do not synthesize fake encrypted state. Response-side reasoning encrypted_content has a separate Anthropic carrier recovery path. |
| Usage accounting | `output_tokens_details.reasoning_tokens` | `completion_tokens_details.reasoning_tokens` on supported models | Thinking budget counts toward `max_tokens`, but token accounting is not isomorphic | Preserve provider-native counters, but document them as approximate when normalized. |
| Streaming behavior | Rich reasoning-aware event family in Responses streaming | Chat streams deltas, not typed reasoning items | Thinking arrives through block events | Streaming adapters should preserve "reasoning happened" and token totals, not exact event taxonomy. |

Google OpenAI-compatible Gemini follows OpenAI Chat-compatible behavior in the
active proxy surface. Native Google/Gemini reasoning details are retired
historical baseline context.

## Main portability boundaries

| Boundary | Why it matters |
| --- | --- |
| Request policy is vendor-specific | OpenAI `reasoning` and Anthropic `thinking` differ in both syntax and semantics. |
| Opaque reasoning state is non-portable on request input | Encrypted or otherwise opaque reasoning content has no safe cross-provider request target shape. Maximum-compatible translation may keep visible summary text or visible transcript/history as ordinary context while dropping the opaque carrier; opaque-only reasoning fails closed. |
| Response carrier recovery is separate | Response-side reasoning encrypted_content can use a dedicated Anthropic carrier recovery path. Do not treat request-side continuity downgrade rules as the whole response translation policy. |
| Token counters are similar but not identical | Developers often compare reasoning token counts operationally, but the providers do not guarantee the same accounting model. |

## Implementation stance

1. Preserve reasoning request knobs only when client and upstream are the same protocol family.
2. Preserve summarized reasoning text and usage counters when possible.
3. Preserve opaque reasoning carriers only through raw/native passthrough.
4. In maximum-compatible cross-provider request translation, never forward `include: ["reasoning.encrypted_content"]`, reasoning item `encrypted_content`, or proxy-local opaque thinking carriers; warn/drop the opaque carrier only when visible summary text or visible transcript/history remains.
5. Fail closed for request-side opaque/provenance reasoning continuity fields and detailed block structures that cannot be represented portably. Ordinary reasoning knobs may map, warn/drop, or fail closed according to the target protocol. Opaque-only reasoning state always fails closed.
6. Keep response-side reasoning encrypted_content handling separate from request continuity. The Anthropic carrier recovery path is a response translation feature, not permission to replay opaque request state across providers.
