# Cache Capability Notes

- Layer: capability-diff
- Status: active
- Last refreshed: 2026-05-16
- Scope: prompt caching, cache handles, cache accounting, and non-portable cache semantics

## Summary

All major providers now expose some form of cache-aware generation, but the contract is different in each case:

| Provider | Mental model |
| --- | --- |
| OpenAI | Automatic prompt caching with optional routing hints like `prompt_cache_key` and retention policy controls |
| Anthropic | Cache breakpoints over prompt prefixes using `cache_control`, with read/write token accounting |

Google OpenAI-compatible Gemini is handled as the OpenAI Chat wire protocol in
the active proxy surface. Native Gemini cache behavior is retained only in the
retired historical Google Gemini baseline; it is not an active proxy capability.

## Provider comparison

| Dimension | OpenAI Responses / Chat | Anthropic Messages | Proxy guidance |
| --- | --- | --- | --- |
| How caching is enabled | Automatic on cacheable prompts; official docs expose `prompt_cache_key` and retention controls | Explicit `cache_control`, available both as a top-level automatic mode and at block level | Do not flatten these into one synthetic cross-provider feature flag. |
| What is being referenced | A cacheable prompt prefix, not a reusable named resource | A cacheable prefix breakpoint inside tools/system/messages | Provider cache handles do not map across protocol families. |
| Lifetime model | Provider-managed retention policy | 5-minute default TTL, optional 1-hour TTL in docs | TTL cannot be normalized safely across providers. |
| Usage fields | `cached_tokens` under prompt/input token details | Separate `cache_creation_input_tokens` and `cache_read_input_tokens` | Preserve raw counters where available and describe normalized values as approximate. |
| Relation to persistence | Separate from `store` / object retention | Separate from message history replay | Never treat cache presence as durable conversation state. |

## High-risk misunderstandings

| Misunderstanding | Correction |
| --- | --- |
| "Anthropic cache reads are the same as OpenAI cached token counts." | Anthropic splits cache writes and reads; OpenAI collapses the prompt cache view differently. |
| "`store` means prompt caching." | It does not. Storage, retrieval, and caching are separate features. |

## Implementation stance

1. Preserve cache knobs byte-for-byte on raw same-protocol passthroughs.
2. In translated mode, treat provider prompt-cache optimization as target-provider request synthesis, not as `llmup` caching. OpenAI cache keys and Anthropic breakpoints have different billing and lifetime effects.
3. Normalize cache usage for reporting, but keep provider-native fields available when the client understands them.
4. Document all cache downgrades explicitly, especially when dropping Anthropic `cache_control`.
5. Provider-cache auto-injection must be policy-driven and trace-visible, not an implicit side effect of translation.
