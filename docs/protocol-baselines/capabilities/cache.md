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
| Gemini | Separate named cache resources plus `cachedContent` references, alongside implicit caching on Gemini 2.5+ models |

## Provider comparison

| Dimension | OpenAI Responses / Chat | Anthropic Messages | Gemini `generateContent` | Proxy guidance |
| --- | --- | --- | --- | --- |
| How caching is enabled | Automatic on cacheable prompts; official docs expose `prompt_cache_key` and retention controls | Explicit `cache_control`, available both as a top-level automatic mode and at block level | Explicit cache resources under `cachedContents`, referenced via `cachedContent`; Gemini 2.5+ also supports implicit caching | Do not flatten these into one synthetic cross-provider feature flag. |
| What is being referenced | A cacheable prompt prefix, not a reusable named resource | A cacheable prefix breakpoint inside tools/system/messages | A named cached-content resource plus optional implicit cache hits | Gemini-style cache handles do not map to OpenAI or Anthropic. |
| Lifetime model | Provider-managed retention policy | 5-minute default TTL, optional 1-hour TTL in docs | TTL on cached resources; defaults and cost model differ | TTL cannot be normalized safely across providers. |
| Usage fields | `cached_tokens` under prompt/input token details | Separate `cache_creation_input_tokens` and `cache_read_input_tokens` | `usageMetadata.cachedContentTokenCount` and cache-related metadata | Preserve raw counters where available and describe normalized values as approximate. |
| Relation to persistence | Separate from `store` / object retention | Separate from message history replay | Separate from `store` and from file uploads | Never treat cache presence as durable conversation state. |

## High-risk misunderstandings

| Misunderstanding | Correction |
| --- | --- |
| "`cachedContent` is equivalent to `prompt_cache_key`." | It is not. Gemini exposes a named cache resource; OpenAI exposes a cache-routing hint. |
| "Anthropic cache reads are the same as OpenAI cached token counts." | Anthropic splits cache writes and reads; OpenAI collapses the prompt cache view differently. |
| "`store` means prompt caching." | It does not. Storage, retrieval, and caching are separate features in all three ecosystems. |
| "Gemini `extra_body.google.cached_content` or `extra_body.cached_content` is portable OpenAI cache control." | It is a Gemini-native extension exposed through OpenAI-library compatibility, not an OpenAI `prompt_cache_key`. |

## Implementation stance

1. Preserve cache knobs byte-for-byte on strict same-format passthroughs.
2. In translated mode, treat provider prompt-cache optimization as target-provider request synthesis, not as `llmup` caching. OpenAI cache keys, Anthropic breakpoints, and Gemini cached-content handles have different billing and lifetime effects.
3. Normalize cache usage for reporting, but keep provider-native fields available when the client understands them.
4. Document all cache downgrades explicitly, especially when dropping Gemini `cachedContent` or Anthropic `cache_control`.
5. Provider-cache auto-injection must be policy-driven and trace-visible, not an implicit side effect of translation.
6. A Gemini-native cached-content handle may be accepted on an OpenAI-shaped request only as an explicit Gemini-routed extension, such as `cached_content`, `extra_body.cached_content`, or `extra_body.google.cached_content`; it is not a cross-provider cache translation.
