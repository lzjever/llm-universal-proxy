# Protocol Spec Online Recheck - 2026-05-16

- Layer: versioned audit
- Status: active online recheck
- Compared against: repo baselines captured under `snapshots/2026-04-16`
- Scope: official provider docs for OpenAI Responses / Chat Completions prompt caching, Anthropic Messages prompt caching, and Gemini `generateContent` caching
- Note: this recheck did not add immutable snapshot artifacts. The 2026-04-16 snapshot bucket remains the archived evidence set.

## Sources Rechecked

| Provider | Official docs |
| --- | --- |
| OpenAI | `https://platform.openai.com/docs/guides/prompt-caching`, `https://developers.openai.com/api/docs/guides/prompt-caching` |
| Anthropic | `https://docs.anthropic.com/en/api/messages`, `https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching`, `https://platform.claude.com/docs/en/build-with-claude/prompt-caching` |
| Gemini | `https://ai.google.dev/gemini-api/docs/caching`, `https://ai.google.dev/api/generate-content`, `https://ai.google.dev/api/caching` |

## Change Summary

| Area | Current official-doc signal | Local documentation action |
| --- | --- | --- |
| OpenAI prompt caching | Prompt caching remains automatic. `prompt_cache_key` and `prompt_cache_retention` remain provider-native optimization controls. The official guide still documents `in_memory` / `24h` retention and the 1024-token cacheability threshold. | Added 2026-05-16 recheck notes to both OpenAI baselines and kept the `in_memory` / `in-memory` spelling inconsistency documented as provider-native. |
| Anthropic prompt caching | Top-level automatic caching and block-level breakpoints remain documented. The pricing table now names newer Claude 4.x models. The Messages reference says `max_tokens: 0` can populate the prompt cache without generating output. | Added Anthropic recheck notes. No compatibility-matrix change: this is still native Anthropic cache behavior. |
| Gemini caching | Explicit caching remains a named `cachedContents` resource referenced by `cachedContent`. The guide states implicit caching is enabled by default for Gemini 2.5+ models and shows provider-specific `extra_body` usage through OpenAI-library compatibility. | Tightened cache docs from "newer models" to "Gemini 2.5+" and documented that OpenAI-library `extra_body.google.cached_content` / `extra_body.cached_content` are Gemini-native, not portable OpenAI cache controls. |

## Compatibility Impact

No cross-provider status cells changed. The recheck reinforces the existing proxy posture:

- Preserve provider-native cache controls on same-provider/native passthrough lanes.
- Preserve or map cache usage counters for observability where possible.
- Do not translate cache handles or TTLs across providers by default.
- Any future provider-cache auto-injection should be an explicit routing/config policy because writes can change cost and cache lifetime.
- Non-official OpenAI model/cache claims found during broader web search were excluded from this audit because the user requested official provider docs as the standard.

## Implementation Follow-Up Candidates

| Candidate | Why it may help | Guardrail |
| --- | --- | --- |
| OpenAI upstream cache policy | Auto-set `prompt_cache_key` and, where suitable, `prompt_cache_retention` for long stable prefixes when callers omit them. | Opt-in per upstream/model; preserve caller-provided fields. |
| Anthropic upstream cache policy | Add top-level automatic `cache_control` or safe breakpoints after translation into native Anthropic requests. | Opt-in only because cache writes cost more than base input tokens. |
| Gemini cache resource adapter | Proxy Gemini `cachedContents` create/list/get/patch/delete resources or expose a documented pre-warm flow. | Keep request-time `cached_content` handle forwarding provider-native and do not create caches implicitly. |
| Cache effectiveness telemetry | Aggregate normalized cache read/create counters by upstream/model and route. | Keep provider-native raw counters available because accounting differs. |
