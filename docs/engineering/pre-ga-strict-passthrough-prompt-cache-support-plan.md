# Pre-GA Raw Provider Passthrough and Provider Prompt-Cache Support Plan

- Status: handoff-ready development plan
- Date: 2026-05-16
- Scope: raw same-protocol passthrough, provider prompt-cache request support, provider-returned cache usage observation, and compatibility simplification
- Non-scope: any `llmup`-managed cache, gateway response cache, semantic cache, cache storage, cache lifecycle management, broad fallback DSLs, pricing catalogs, guardrails, prompt management, admin UI expansion

## Plan Coordination

This plan assumes [pre-ga-remove-native-gemini-format-plan.md](./pre-ga-remove-native-gemini-format-plan.md) is accepted as the owning decision for native Gemini removal.

Active pre-GA protocol families for this plan are therefore:

- OpenAI Chat Completions
- OpenAI Responses
- Anthropic Messages

Gemini remains usable only as a provider brand behind an OpenAI-compatible upstream. That path is handled as OpenAI Chat wire protocol. This plan must not add Gemini `generateContent`, `cachedContent`, `cachedContents/*`, `thoughtSignature`, or `extra_body.google.cached_content` work. Any future Google-specific OpenAI-compatible extension must be a separate explicit plan; it is not part of this pre-GA prompt-cache hardening.

## Goal

Make the pre-GA behavior easy to reason about:

- If the downstream client protocol and upstream wire protocol are the same, and the route can avoid body mutation and response normalization, `llmup` uses a strict raw provider passthrough execution path.
- If the protocols differ, or a configured feature needs to construct or mutate the target-provider request, `llmup` uses a maximum-compatible request-construction path with documented hard portability boundaries.
- Provider-native prompt-cache request controls are either preserved as original payload in raw passthrough or added later by an explicit provider-native request-control modifier after the target request shape is known.
- The proxy does not cache responses, prompts, embeddings, tokens, KV state, or provider cache resources.
- The proxy does not invent a cross-provider cache abstraction.

Same-format means the provider-facing wire protocol, not the provider brand. An OpenAI-compatible upstream can use raw provider passthrough for OpenAI-shaped requests when the route does not require body mutation or response normalization. If a compatible upstream needs provider shims, model body rewrites, or response normalization, it is a maximum-compatible translation route rather than raw provider passthrough.

Raw provider passthrough is a strict execution path and implementation target, not a user-selectable compatibility level. Constructed or translated routes have one product behavior: maximum safe compatibility with fail-closed boundaries when semantics cannot be preserved or safely degraded.

This is a pre-GA plan. It may break current tests and route behavior where they encode non-passthrough same-format mutation.

## Design Principles

1. Raw provider passthrough is a raw execution path. Same-format request and response payloads must not be normalized, repaired, reserialized, or translated.
2. Translation and target-provider request construction must be explicit and maximum-compatible. Compatibility shims and provider-native request-control synthesis belong outside raw passthrough.
3. Cache is provider-owned. OpenAI `prompt_cache_key` / `prompt_cache_retention` and Anthropic `cache_control` are provider prompt-cache request mechanisms with different semantics, billing, retention, and lifecycle rules.
4. Preserve prefixes. Cache savings depend on stable prompt prefixes, message order, tool order, schemas, media detail, and provider-specific cache handles.
5. Keep the mental model small. Route selection should answer one question: "strict raw passthrough or maximum-compatible request construction?"
6. Stop scope creep early. Do not add `llmup` cache storage, universal cache controls, response caching, semantic caching, fallback routing languages, or synthetic provider state while hardening passthrough.

## Current Findings

### Local Code Audit

The current implementation chooses a same upstream format correctly in discovery, but the data path still flows through shared compatibility machinery.

Important mutation points:

- Request path: [src/server/proxy.rs](../../src/server/proxy.rs) calls boundary assessment and `translate_request_with_policy()` for all routes.
- Same-format translation: [src/translate/internal.rs](../../src/translate/internal.rs) still performs role normalization, translation defaults, MiniMax/provider shims, public tool validation, and bridge-context cleanup even when formats match.
- Model rewrite: [src/server/proxy.rs](../../src/server/proxy.rs) rewrites or removes the request `model` field after translation.
- Non-stream response path: [src/server/proxy.rs](../../src/server/proxy.rs) reads the whole body, parses JSON, translates, redacts, and reserializes even for same-format success responses; provider error bodies can also be wrapped into client-protocol errors.
- Stream response path: [src/streaming/stream.rs](../../src/streaming/stream.rs) uses guarded SSE parsing plus redaction rather than raw passthrough for same-format streams.
- Headers: [src/server/headers.rs](../../src/server/headers.rs) rebuilds request and response headers with allowlists and defaults.

These behaviors are valuable in the maximum-compatible request-construction path, but they are not raw provider passthrough. Several current tests also encode the old behavior by expecting same-format role repair, parallel tool defaults, MiniMax overrides, and alias model rewrites.

### External Product Patterns

Comparable gateways split the world in ways that support this plan:

- LiteLLM has OpenAI-native integrations and separate passthrough endpoints for newer or less-supported OpenAI endpoints.
- Cloudflare AI Gateway offers provider-native endpoints where a user replaces the provider base URL, and separately offers exact response caching.
- OpenRouter distinguishes provider prompt caching from its own response cache, and uses sticky provider routing to preserve provider cache warmth.
- Helicone distinguishes provider-level prompt caching from Helicone response caching, while its AI Gateway route is a broader OpenAI-compatible translation layer.
- Portkey exposes strict OpenAI compliance controls because provider-native fields can be lost when normalized into a single schema.
- Vercel AI Gateway namespaces gateway behavior in `providerOptions.gateway`, including `caching: 'auto'`, instead of pretending provider cache controls are the same.
- Envoy AI Gateway exposes a provider-agnostic `cache_control` field, but scopes it to Anthropic-compatible targets where it can be translated into native Anthropic / Vertex Claude / Bedrock Claude controls.

The common lesson for `llmup`: raw provider passthrough and maximum-compatible request construction should be separate execution paths. Provider prompt-cache request fields should remain provider-native and should not become a third primary lane. Any cache usage observation must be read-only telemetry over provider-returned usage fields, not a `llmup` cache implementation.

## Target Architecture

Introduce an explicit primary execution path plus orthogonal modifiers:

```rust
enum PrimaryExecutionPath {
    RawProviderPassthrough,
    MaximumCompatibilityTranslation,
}

struct ExecutionModifiers {
    state_bridge: StateBridgeModifier,        // off | capture_candidate | expanded
    provider_prompt_cache: PromptCacheModifier // none | preserved | synthesized
}
```

This plan owns the primary execution path decision and provider prompt-cache modifier. [pre-ga-conversation-state-bridge-plan.md](./pre-ga-conversation-state-bridge-plan.md) owns the state-bridge modifier. This avoids inventing a separate primary path for every combination such as "state bridge + translation + prompt-cache optimization", and it keeps provider prompt-cache synthesis as a provider-native request-control modifier rather than a third primary lane. When the state bridge is enabled, execution order must be:

1. Conversation state expansion.
2. Source -> target protocol translation.
3. Provider prompt-cache optimization.
4. Upstream request.

Primary execution path selection:

- `RawProviderPassthrough`: client protocol equals upstream wire protocol and no configured route feature requires body mutation, provider-native request-control synthesis, or response normalization.
- `MaximumCompatibilityTranslation`: protocols differ, or the selected route needs a compatibility shim, target-provider request construction, provider-native request-control synthesis, model alias body rewrite, provider-specific role repair, translation default injection, format conversion, or error-shape conversion. This path always follows the single maximum safe compatibility strategy.

The route decision should be visible in debug traces and metrics as `llmup.execution_path`, with modifiers such as `llmup.state_bridge` and `llmup.provider_prompt_cache`.

### Raw Provider Passthrough Contract

Allowed proxy behavior:

- Data-plane authentication and provider credential injection.
- Upstream selection, DNS/TLS, timeout, cancellation, and body/stream size limits.
- Namespace-to-upstream base URL mapping while preserving the provider path suffix, method, and query semantics.
- Hop-by-hop header stripping and configured auth/header policy.
- Trace IDs, metrics, and hooks that observe metadata.
- Fail-closed rejection if a request contains a known proxy-private artifact such as `_llmup_tool_bridge_context` or `__llmup_custom__*` in a structured public control field.

Disallowed in raw provider passthrough:

- JSON reserialization of request or success response bodies.
- Role repair, role coalescing, tool-name repair, schema repair, MiniMax/provider shims, or translation defaults.
- `stream` insertion, `parallel_tool_calls` insertion, max-token insertion, Anthropic `disable_parallel_tool_use` insertion, or other body defaults.
- `model` field rewrite/removal. If alias expansion requires body mutation, the path is not raw provider passthrough.
- Provider error wrapping into another protocol shape.
- SSE event parsing or rewriting for ordinary successful same-format streams.
- Response redaction that changes client-visible bytes. Redaction should apply to stored traces, hook payloads, and logs, not passthrough output.

Header policy:

- Request `Authorization` usually cannot be raw passthrough when `llmup` owns provider credentials. That is explicit proxy behavior, not protocol translation.
- Preserve provider protocol headers where safe. Strip hop-by-hop headers, proxy-private headers, and headers that would leak downstream credentials.
- For raw passthrough routes, do not synthesize protocol defaults such as `anthropic-version` unless the route is explicitly configured to do so; otherwise a client testing a provider's native failure mode will not see it.

### Translation Contract

The existing translation machinery should remain available, but only in `MaximumCompatibilityTranslation`.

Maximum-compatible path responsibilities:

- Convert request and response schemas across OpenAI Chat Completions, OpenAI Responses, and Anthropic Messages.
- Apply surface gates, maximum-compatible shims, warnings, and fail-closed portability boundaries.
- Translate portable tool calls, media, stop reasons, usage, and streaming lifecycle events.
- Fail closed on provider-owned lifecycle state and non-portable cache state that cannot be represented safely.

### Provider Prompt-Cache Modifier

This modifier exists to keep provider-native request-control synthesis explicit without making it a third primary lane. In `preserved` mode, raw passthrough simply carries the client's native cache fields unchanged. In `synthesized` mode, the proxy must work on a constructed target-provider request, so the route is not strict raw provider passthrough.

Allowed behavior:

- Start from the target-provider request after routing and any required translation or request construction.
- Add only provider-native prompt-cache request controls requested by explicit policy.
- Preserve all other request and response semantics as close to native as possible.
- Emit trace metadata for synthesized fields.

Disallowed behavior:

- General role repair, schema repair, model body rewrite, response normalization, or cross-protocol error shaping.
- Calling the result raw provider passthrough.

## Provider Prompt-Cache Support Strategy

### Stance

`llmup` should support provider prompt-cache mechanisms so operators can use LLMs more economically. It should not implement cache storage or cache lookup itself.

There are two different activities:

- Provider prompt-cache optimization: `llmup` may synthesize provider-native request controls that ask the selected upstream to use its own prompt-cache mechanism.
- `llmup` caching: `llmup` stores, indexes, evicts, or serves cached data itself.

This plan includes the first activity and excludes the second. A synthesized OpenAI `prompt_cache_key` or Anthropic `cache_control` is still just a request to the provider; it is not a `llmup` cache.

Add a first-class internal Provider Prompt Cache IR so the behavior is not scattered through translators:

- `target_provider`
- `mode`: `off | explicit | auto_safe`
- `source`: `native | explicit_extension | route_policy | synthesized`
- `openai.prompt_cache_key`
- `openai.prompt_cache_retention`
- `anthropic.cache_control`
- `anthropic.breakpoint_strategy`: `top_level_auto | configured_breakpoints`
- `anthropic.ttl`
- `skipped_reason`

Do:

- Preserve provider-native prompt-cache request fields in raw provider passthrough.
- Preserve known provider cache usage counters in client-visible native responses.
- Optionally observe provider-returned cache usage counters for metrics and debug traces.
- Keep translated-mode support narrow: forward or map only explicit provider-native extension fields that are documented and intentionally supported.
- In translated mode, prefer provider-supported caching controls when the user explicitly asks for them. For Anthropic, use top-level automatic `cache_control` only for full-history conversation routes, and use configured block breakpoints for stable-prefix workloads.
- Add a provider prompt-cache optimizer that can synthesize target-provider cache request controls when a route or global policy enables it.
- Keep synthesized controls visible in debug traces and compatibility warnings so users can see exactly what `llmup` added.
- Fail closed when a provider cache handle contains non-reconstructable context and the target provider cannot honor it.
- Run all synthesis after the target-provider request has been built, because provider cache matching uses the exact target-side prompt prefix, order, and parameters.

Do not:

- Add any `llmup` cache store, cache lookup, cache eviction, cache key, or cache lifecycle manager.
- Add gateway response cache in pre-GA.
- Add semantic cache.
- Add a cross-provider `cache: true` request parameter.
- Treat OpenAI `prompt_cache_key` and Anthropic `cache_control` as direct semantic equivalents.
- Auto-insert provider cache controls with no policy, trace, or way to disable them.
- Infer stable/static content from message text meaning. Heuristics must use protocol structure, configured cache groups, and deterministic fingerprints, not LLM judgment.
- Add Google/Gemini-specific cache extensions, including `cachedContent`, `cached_content`, `cachedContents/*`, or `extra_body.google.cached_content`, in this plan.

### Provider Prompt-Cache Policy

Add a small policy surface rather than scattering provider-specific heuristics through translators:

```yaml
provider_prompt_cache:
  mode: explicit        # off | explicit | auto_safe
  openai:
    prompt_cache_key: route_cache_group  # off | explicit | route_cache_group | tenant_route | stable_prefix_hash
    prompt_cache_retention: preserve     # preserve | in_memory | 24h
  anthropic:
    cache_control: configured_breakpoints # off | explicit | top_level_auto | configured_breakpoints
    ttl: preserve                        # preserve | 5m | 1h
```

Recommended defaults:

- `explicit` for initial pre-GA rollout: preserve native fields and honor explicit provider extensions.
- `auto_safe` as the cost-optimized profile once contract tests and trace visibility are complete.

`auto_safe` may synthesize request controls, but only from deterministic inputs:

- route / namespace / model alias
- upstream provider, project, region, and model
- authenticated tenant or configured cache group
- configured session/cache group identifiers that are explicitly documented as cache grouping inputs
- a hash of the stable protocol prefix when explicitly selected

It must not hash the entire request as the default OpenAI `prompt_cache_key`: the full request includes dynamic user turns and would over-partition cache routing. It also must not use `previous_response_id`, `conversation`, `resp_llmup_*`, request IDs, arbitrary metadata, or message prose as a cache key source.

### Current Functionality

Already present:

- OpenAI Chat/Responses `prompt_cache_key` and `prompt_cache_retention` are treated as OpenAI-native controls and are preserved on OpenAI-family targets when supported.
- Anthropic `cache_control` is detected as non-portable when translating away from Anthropic and is warned/dropped instead of being mapped to unrelated provider controls.
- OpenAI Chat/Responses to Anthropic does not inject `cache_control`; [tests/integration_test.rs](../../tests/integration_test.rs) has a regression test for concurrent OpenAI-to-Anthropic requests that asserts no marker injection.
- Anthropic to OpenAI translation strips `cache_control` and does not currently synthesize OpenAI `prompt_cache_key`.
- Current code still contains native Gemini cache-handle handling. That is historical implementation surface owned by the native-Gemini removal plan, not future work for this plan.
- Hooks and translation code already observe provider-returned cache usage fields such as OpenAI cached tokens and Anthropic cache read/write tokens.

Known gaps:

- OpenAI-shaped requests routed to Anthropic have no explicit way to ask for Anthropic top-level automatic `cache_control`.
- OpenAI-shaped content parts routed to Anthropic do not intentionally preserve explicit per-block Anthropic `cache_control` extension fields.
- Anthropic-shaped requests routed to OpenAI have no way to ask `llmup` to synthesize `prompt_cache_key`.
- There is no shared policy object that explains why a cache control was preserved, dropped, or synthesized.
- There is no target-provider optimization matrix that says what `llmup` should do for every source/target pair.
- Cache-aware routing is not yet modeled: a fallback or load-balanced route can scatter equivalent cacheable requests across upstream credentials/projects/regions and lose provider cache warmth.
- Raw same-protocol passthrough still needs the raw request/response work described above before native cache controls are truly byte-preserved.

### Translated-Mode Prompt-Cache Rules

Translated-mode prompt-cache support has two levels:

- `explicit`: the proxy only forwards or maps explicit provider-native extension fields.
- `auto_safe`: the proxy may synthesize target-provider prompt-cache controls from deterministic route/session/prefix information.

The proxy may help express a provider-native cache request only when the user or route configuration has made cache optimization intent visible, either through an explicit extension or through `provider_prompt_cache.mode = "auto_safe"`.

Recommended request extension pattern:

```json
{
  "extra_body": {
    "anthropic": {
      "cache_control": { "type": "ephemeral" }
    }
  }
}
```

This extension means: when the selected upstream is Anthropic, emit Anthropic top-level `cache_control` on the translated Messages request. It does not mean "enable `llmup` caching", and it must be ignored or rejected with a clear warning on non-Anthropic targets.

Allowed translated support:

- OpenAI Chat/Responses to Anthropic: map explicit `extra_body.anthropic.cache_control` to Anthropic top-level `cache_control`.
- OpenAI Chat/Responses to Anthropic: in `auto_safe`, apply the configured Anthropic strategy when no explicit Anthropic cache control is present: top-level automatic caching for full-history conversations, or configured breakpoints for stable prefix blocks.
- OpenAI Chat/Responses to Anthropic: optionally preserve explicit `cache_control` on OpenAI text content parts only when the target Anthropic block type can legally carry it. Do not infer block-level breakpoints from prose.
- Anthropic to OpenAI Chat/Responses: in `auto_safe`, synthesize OpenAI `prompt_cache_key` when no explicit OpenAI cache key is present.
- OpenAI Chat <-> OpenAI Responses: preserve OpenAI prompt-cache controls across OpenAI-family translation without changing retention spelling.

Disallowed translated support:

- Do not read an arbitrary prompt and decide which blocks are "static" from natural-language meaning.
- Do not add Anthropic block-level breakpoints based on content length, role, first message, last system message, or perceived repetition unless a future route-level policy explicitly names that behavior.
- Do not auto-upgrade Anthropic TTL to `1h`.
- Do not copy an OpenAI `prompt_cache_key` value into another provider field as if the semantics were identical. In `auto_safe`, the key can be treated only as evidence of cache intent.
- Do not support Google/Gemini `extra_body.google.cached_content` in the default OpenAI-compatible path. It is a provider-specific extension that would reintroduce native Gemini resource lifecycle scope.

Anthropic explicit block-marker shape:

- For OpenAI-shaped requests, accept `cache_control` only on content parts that translate to Anthropic cacheable blocks.
- Eligible target blocks under Anthropic's current docs: tool definitions, system text blocks, user/assistant text blocks, user image/document blocks, assistant `tool_use`, and user `tool_result`.
- For OpenAI-shaped tool calls and tool results, require an explicit extension shape that maps unambiguously to the produced Anthropic top-level block. Do not infer a marker from a surrounding assistant message when the generated `tool_use` block is only one of several blocks.
- Ineligible target blocks: thinking/redacted thinking blocks when marked directly, citation/sub-content children, empty text blocks, and any block whose target provider docs do not allow direct `cache_control`.
- Unsupported or ambiguous markers must fail closed rather than being silently dropped when the caller explicitly requested provider prompt caching.

Target-provider matrix:

| Target provider protocol | Current provider cache mechanism | What `llmup` should do |
| --- | --- | --- |
| OpenAI Chat / Responses | Automatic prompt caching; optional `prompt_cache_key` and `prompt_cache_retention` | Preserve explicit OpenAI fields. In `auto_safe`, synthesize `prompt_cache_key` from configured cache group / tenant-route / stable-prefix fingerprint. Do not set `24h` unless explicit or configured. |
| Anthropic Messages | `cache_control` top-level automatic mode or block-level breakpoints | Preserve explicit Anthropic fields. In `auto_safe`, choose a configured strategy: top-level automatic caching for growing full-history conversations, or block-level breakpoints at configured stable prefix boundaries such as tools/system/docs. |
| Google Gemini through OpenAI-compatible upstream | OpenAI Chat wire protocol plus Google-specific optional extensions | Treat as OpenAI Chat for this plan. Do not support `extra_body.google.cached_content` or native Gemini cache resources in pre-GA. |

OpenAI `prompt_cache_key` synthesis:

- Source priority: explicit OpenAI extension field, configured `cache_group`, authenticated tenant + route + model alias, stable-prefix hash when explicitly selected.
- Stable-prefix hash, when enabled, should include only translated tool definitions, system/developer instructions, and static leading context before the first dynamic user turn. It must be a hash, never raw prompt text.
- The key must be bounded and deterministic, for example `llmup:v1:{namespace}:{alias}:{cache_group_or_hash}`.
- The trace should record `provider_prompt_cache.openai.key_source`.
- The optimizer should not synthesize `prompt_cache_retention: "24h"` from Anthropic `ttl: "1h"`; those controls have different retention and billing semantics.

Anthropic `cache_control` synthesis:

- `auto_safe` must choose between top-level automatic caching and configured block-level breakpoints.
- Top-level request `cache_control` is appropriate for growing full-history conversations where the whole preceding transcript is expected to repeat and advance.
- Top-level request `cache_control` is not a safe universal default for one-shot requests with stable system/tools and varying final user text, because automatic caching writes only at the chosen breakpoint and can miss forever if the breakpoint includes a changing suffix.
- Configured block-level breakpoints are preferred for stable system prompts, tool definitions, examples, large documents, or repository context. The placement should come from explicit configuration or explicit user-provided markers, not from prose classification.
- Default TTL should be provider default / 5-minute `ephemeral`.
- `ttl: "1h"` requires explicit user input or route config because write cost is higher.
- Configured block-level marker synthesis should expose simple injection points such as `tools`, `system[last]`, `message[index]`, or `content_part[index]`, with a maximum of four Anthropic breakpoints and explicit rejection when TTL ordering would be invalid.

Google Gemini through OpenAI-compatible upstream:

- Treat the upstream as OpenAI Chat wire protocol for active pre-GA work.
- Do not synthesize, translate, or test `extra_body.google.cached_content` in this plan, even though Google documents the extension for OpenAI-compatible clients. Raw passthrough may carry unknown OpenAI-compatible fields as bytes, but `llmup` should not claim provider-cache support for them.
- If future demand proves the economics justify it, create a separate Google-specific OpenAI-compatible extension plan with explicit operator consent.

### Source To Target Coverage Matrix

This matrix is the handoff checklist for every active pre-GA protocol pair after native Gemini removal.

| Source client format | Target upstream format | Provider cache behavior |
| --- | --- | --- |
| OpenAI Chat | OpenAI Chat | Raw passthrough preserves `prompt_cache_key`, `prompt_cache_retention`, automatic prompt caching, and raw usage. Same-format synthesis, if enabled, uses the `provider_prompt_cache` modifier on a constructed target-provider request, not raw provider passthrough. |
| OpenAI Chat | OpenAI Responses | Preserve OpenAI prompt-cache controls during OpenAI-family translation. Do not alter retention spelling. Preserve cache usage mapping in the client response. |
| OpenAI Chat | Anthropic Messages | `explicit`: map `extra_body.anthropic.cache_control` to top-level `cache_control`, and preserve explicit eligible block markers. `auto_safe`: use configured Anthropic strategy: top-level for full-history conversations, or configured breakpoints for stable tools/system/docs. Do not infer block breakpoints from prose. |
| OpenAI Responses | OpenAI Chat | Preserve OpenAI prompt-cache controls during OpenAI-family translation. Do not use Responses `store` / `previous_response_id` / `conversation` as cache controls. |
| OpenAI Responses | OpenAI Responses | Raw passthrough preserves `prompt_cache_key`, `prompt_cache_retention`, automatic prompt caching, and raw usage. Same-format synthesis, if enabled, uses the `provider_prompt_cache` modifier on a constructed target-provider request, not raw provider passthrough. |
| OpenAI Responses | Anthropic Messages | Same as OpenAI Chat -> Anthropic, after Responses input is converted to the Messages pivot. Keep visible summaries/history stable, but do not translate OpenAI state controls into cache controls. |
| Anthropic Messages | OpenAI Chat | `explicit`: warn/drop Anthropic `cache_control`. `auto_safe`: synthesize OpenAI `prompt_cache_key` from policy inputs when no explicit OpenAI key exists. Never copy raw prompt text or Anthropic TTL into the key. |
| Anthropic Messages | OpenAI Responses | Same as Anthropic -> OpenAI Chat, with OpenAI Responses `prompt_cache_key` and `prompt_cache_retention` as target fields. Do not map Anthropic `max_tokens: 0` prewarm into Responses state. |
| Anthropic Messages | Anthropic Messages | Raw passthrough preserves top-level and block-level `cache_control`, TTL, `max_tokens: 0` prewarm, thinking cache behavior, and raw usage. |

Native Gemini rows are intentionally absent. `format: google`, `format: gemini`, and `/google/v1beta/*` are owned by the removal plan and must not receive new cache optimizer work.

### Optimizer Invariants

- Run optimizer logic after any conversation-state expansion and after translation has produced the target-provider request shape, because cache hits depend on the bytes/structure the target provider sees.
- Preserve target prompt prefix order: tools, system/developer instructions, static media/document context, then dynamic user content. Avoid reordering tool definitions or schema keys during optimization.
- Never use natural-language classification to decide that content is stable. Use only configured cache groups, route identity, tenant/session identifiers, and deterministic structural prefix fingerprints.
- Do not place timestamps, request IDs, random trace IDs, short-lived user text, or provider credentials inside synthesized cache keys.
- Emit trace fields for every synthesized or dropped cache control: target provider, policy mode, field, key source, TTL/retention source, and reason.
- Include a kill switch. Operators must be able to set `provider_prompt_cache.mode = "off"` and get no synthesized provider cache controls.

### Cache-Aware Routing

Cache-aware routing is provider prompt-cache support, not gateway response caching. It should keep equivalent cacheable requests on the same provider/deployment when the router otherwise has multiple equivalent choices.

Rules:

- Route priority: explicit upstream/model choice > state-bridge continuation owner > native provider state owner > cache-aware sticky routing.
- Sticky key components: tenant/auth boundary, namespace, model alias, upstream provider, credential/project/region, target model, provider prompt-cache key or stable-prefix fingerprint.
- Never override a hard provider choice or explicit upstream order.
- If a warm provider is unavailable and failover is allowed, annotate the trace with `cache_warm_provider_unavailable`.
- Do not let sticky routing create a semantic response cache. The provider still generates every response.
- For OpenAI, sticky routing complements `prompt_cache_key`; for Anthropic, it matters when multiple workspaces/regions/providers can satisfy the same route.
- Conversation-state bridge continuations must keep their originally resolved route/upstream unless an explicit route policy says failover is allowed. Cache-aware routing must not scatter a replay chain.

### Prewarm And Threshold Diagnostics

Provider prompt-cache economics depend on minimum prefix sizes and write/read timing. The optimizer should expose diagnostics without silently adding expensive work:

- OpenAI: prompt caching is automatic above the provider threshold; `prompt_cache_key` only helps route similar prefixes. The optimizer should record when it synthesizes a key and let `cached_tokens` prove effectiveness.
- Anthropic: prompts below model-specific minimum token thresholds silently receive no cache benefit. The optimizer may add top-level `cache_control` in `auto_safe`, but should surface `cache_creation_input_tokens == 0 && cache_read_input_tokens == 0` as a possible "not cached" diagnostic. A future optional `count_tokens` preflight may warn before sending, but should not be default because it adds latency and another upstream call.
- Anthropic prewarm: `max_tokens: 0` is useful only for explicit prewarm flows and has official restrictions. Cross-protocol translated prewarm should require explicit Anthropic extension or route config; it should not be inferred from an ordinary OpenAI request.

### Provider Notes

OpenAI:

- Prompt caching is automatic for cacheable prompts on recent models.
- Preserve `prompt_cache_key` and `prompt_cache_retention` on OpenAI raw passthrough.
- Track `usage.prompt_tokens_details.cached_tokens` and Responses `usage.input_tokens_details.cached_tokens`.
- Current official docs document `prompt_cache_retention` values `in_memory` and `24h`. Most models default to `in_memory`; `gpt-5.5`, `gpt-5.5-pro`, and future models default to `24h` and do not support `in_memory`.
- The optimizer must not synthesize `24h` unless route config explicitly asks for it. If the provider defaults a model to `24h`, that is provider-native behavior and should remain visible in usage/data-retention docs rather than being hidden by `llmup`.

Anthropic:

- Preserve top-level and block-level `cache_control`.
- Top-level `cache_control` is the preferred low-complexity translated-mode target only for routes that represent growing full-history conversations. For one-shot requests with a stable prefix and dynamic suffix, explicit breakpoints at the last stable block are the safer cost-control default.
- Anthropic's OpenAI SDK compatibility surface is not the same as native Messages prompt caching; `llmup` translated support should target Anthropic Messages requests, not assume Anthropic's OpenAI-compatible endpoint can honor every cache control.
- Preserve `ttl: "1h"` only as user-supplied provider-native input; do not auto-upgrade TTLs because 1-hour writes cost more.
- Preserve `max_tokens: 0` prewarm requests in Anthropic raw passthrough. Translation behavior must not inject minimum max-token defaults into this path.
- Track `cache_creation_input_tokens`, `cache_read_input_tokens`, and `cache_creation` subfields.
- Anthropic currently supports prompt caching on all active Claude models, with model-specific minimum token thresholds, a 20-block lookback window per breakpoint, and up to four breakpoints. It supports automatic top-level caching on Claude API, Claude Platform on AWS, and Microsoft Foundry; Bedrock and Vertex Claude routes require explicit block-level breakpoints.
- Tool definitions, text blocks, user image/document blocks, assistant tool-use blocks, and user tool-result blocks are cacheable. Thinking blocks cannot be directly marked, although previous thinking can be cached as part of a larger prefix.

## Development Plan

### Phase 0: Freeze The Contract

Deliverables:

- Add the raw provider passthrough contract tests before changing behavior.
- Update docs to say same-format passthrough is raw payload passthrough plus explicit proxy behavior.
- Mark current same-format mutation tests as expected-to-change.

Acceptance:

- Developers can point to one document that explains when a route is raw provider passthrough versus maximum-compatible translation.
- No new product feature is introduced in this phase.

### Phase 1: Introduce Execution Lane Plumbing

Deliverables:

- Add `PrimaryExecutionPath`, including `RawProviderPassthrough` and `MaximumCompatibilityTranslation`, plus state/cache modifier fields.
- Route discovery returns both upstream format and primary execution path.
- Debug traces, metrics, and hooks include the primary execution path and modifiers.
- Keep behavior unchanged while path selection is observable.

Acceptance:

- Unit tests prove same-format routes select `RawProviderPassthrough` unless model alias rewriting or a configured shim requires mutation or response normalization.
- Cross-format routes select `MaximumCompatibilityTranslation`.

### Phase 2: Raw Request Passthrough

Deliverables:

- Preserve raw request body bytes through routing.
- Split the request representation into `raw_bytes` plus a parsed `serde_json::Value` used only for routing/boundary decisions. Strict passthrough sends `raw_bytes`; constructed, cache-synthesized, and state-expanded paths use the parsed/mutable JSON.
- In `RawProviderPassthrough`, skip `translate_request_with_policy()`, role repair, translation defaults, MiniMax overrides, and body-level model rewrite.
- Replace body-mutating safety checks with narrow ingress checks that reject proxy-private structured artifacts without reserializing the body.
- Ensure forced streaming does not insert `stream` in raw provider passthrough.

Acceptance tests:

- Golden upstream request bodies match client request bytes for OpenAI Chat, OpenAI Responses, and Anthropic Messages.
- Golden bodies include field order, whitespace, numeric formatting, unknown provider fields, and provider error bodies where relevant.
- Native cache fields remain byte-identical in raw provider passthrough.
- Anthropic `max_tokens: 0` prewarm passes through unchanged.
- Alias routes that need `model` body rewrite are classified as `MaximumCompatibilityTranslation`, not raw provider passthrough.

### Phase 3: Raw Response And SSE Passthrough

Deliverables:

- In `RawProviderPassthrough`, forward upstream status, content type, selected safe response headers, and raw body bytes without JSON parse/translate/reserialize.
- Forward provider error bodies unchanged.
- Add a raw SSE forwarding path for raw provider passthrough streams. Keep chunking transport-flexible, but preserve event bytes and event order.
- Move redaction to trace/log/hook storage. Do not redact client-visible raw provider passthrough output.

Acceptance tests:

- Non-stream success and error response bodies match upstream bytes.
- SSE tests preserve `event`, `data`, `id`, `retry`, comments, blank lines, terminal events, and provider usage frames.
- No raw provider passthrough response calls `translate_response_with_context()`.

### Phase 4: Provider Prompt-Cache Optimizer

Deliverables:

- Add the Provider Prompt Cache IR.
- Add `provider_prompt_cache` policy parsing and effective route/model resolution.
- Add explicit OpenAI-shaped to Anthropic support for `extra_body.anthropic.cache_control` -> top-level Anthropic `cache_control`.
- Add explicit OpenAI-shaped to Anthropic support for eligible block-level `cache_control` only when the translated Anthropic block type can legally carry it.
- Add `auto_safe` Anthropic synthesis using configured strategy: top-level automatic caching for full-history routes or configured breakpoints for stable-prefix routes.
- Add `auto_safe` OpenAI `prompt_cache_key` synthesis for translated OpenAI targets when no explicit cache key is present.
- Continue warning/dropping low-risk non-portable provider cache controls when the target provider cannot honor them.
- Keep translation marker-free unless explicit extension or `auto_safe` route/global policy is present.
- Add trace/debug fields that show `provider_prompt_cache.mode`, target provider, synthesized fields, and key source.

Acceptance tests:

- Existing OpenAI-to-Anthropic requests still do not receive `cache_control` by default.
- `extra_body.anthropic.cache_control` maps exactly to Anthropic top-level `cache_control`.
- Invalid or conflicting Anthropic cache-control extension shapes fail closed before upstream.
- Explicit OpenAI content/tool `cache_control` maps only to eligible Anthropic blocks; direct thinking markers, empty text, sub-content markers, and ambiguous tool-call/tool-result marker shapes are rejected.
- Anthropic `cache_control` translated to OpenAI still warns/drops in `explicit` mode rather than becoming a synthetic OpenAI cache control.
- Anthropic-to-OpenAI in `auto_safe` synthesizes a bounded `prompt_cache_key` with the configured source and never copies raw prompt text into the key.
- OpenAI-to-Anthropic in `auto_safe` applies the configured Anthropic strategy and does not add block-level markers unless explicitly configured.
- `prompt_cache_retention: "24h"` and Anthropic `ttl: "1h"` are never synthesized from each other without explicit route config.
- `extra_body.google.cached_content` fails closed in translated paths when treated as an explicit cache request; it is not mapped to a native Gemini field.

### Phase 5: Cache-Aware Routing Guardrails

This phase is optional for the first implementation unless the deployment has multiple equivalent upstreams for the same model. It must not block raw provider passthrough or basic provider prompt-cache request support.

Deliverables:

- Add cache-aware sticky routing only for routes that already have multiple equivalent upstream choices, keyed by provider prompt-cache policy, tenant/auth boundary, namespace, model alias, upstream provider, credential/project/region, target model, and provider prompt-cache key or stable-prefix fingerprint.
- Keep hard provider overrides and explicit upstream order authoritative.
- Annotate failover from a warm route with `cache_warm_provider_unavailable`.
- Keep metrics separate from gateway response cache metrics.

Acceptance tests:

- Equivalent cacheable requests route to the same upstream when multiple equivalent upstreams exist and cache-aware routing is enabled.
- Explicit provider order disables sticky rerouting.
- State-bridge replay chains keep their originally resolved upstream unless explicit failover policy allows otherwise.
- Failover emits a cache-warmth diagnostic and does not reuse response bodies.

### Phase 6: Provider Cache Usage Observation Only

Deliverables:

- Add an internal `ProviderCacheUsage` observation struct populated from raw provider usage after the response is already decided. This struct is telemetry only and must not drive cache lookup, cache keys, eviction, or response reuse.
- Preserve raw usage in raw provider passthrough responses.
- Emit best-effort metrics:
  - `cache.read_tokens`
  - `cache.write_tokens`
  - `cache.hit_tokens`
  - `cache.provider`
  - `cache.source_field`
- Add docs that these metrics are approximate and provider-specific.

Acceptance tests:

- OpenAI cached tokens are observed without changing response body.
- Anthropic read/write counters are observed without changing response body.
- Unknown usage shapes do not fail requests.

### Phase 7: Remove Same-Format Compatibility Shims

Deliverables:

- Move same-format role repair, translation defaults, MiniMax overrides, and model rewrite tests into the maximum-compatible path or delete them.
- Rename any "passthrough" tests that actually assert compatibility mutation.
- Update compatibility docs and matrix to reflect raw provider passthrough as the intended same-format behavior when the route avoids mutation and normalization.

Acceptance:

- Test names and docs no longer use "passthrough" for a path that mutates protocol payloads.
- All same-format raw provider passthrough tests pass without hidden exceptions.

## Test Matrix

Required local tests:

| Area | Required coverage |
| --- | --- |
| Request payload | Byte-for-byte upstream body equality for OpenAI Chat, OpenAI Responses, and Anthropic Messages |
| Response payload | Byte-for-byte downstream body equality for success and provider error responses |
| Streaming | Raw SSE event preservation for ordinary success streams |
| Headers | Auth rewrite and hop-by-hop stripping are explicit; provider protocol headers are preserved where safe |
| Provider prompt-cache support | Native cache request fields and usage fields preserved; explicit translated extensions map only to their target provider; `auto_safe` synthesis is deterministic, trace-visible, and provider-scoped; optional observation does not mutate output and does not cache anything |
| Provider cache matrix | Every source/target pair in the Source To Target Coverage Matrix has at least one positive test and one non-portable/fail-closed test where applicable |
| Cache-aware routing | Sticky routing improves provider cache warmth without overriding hard provider order and without serving cached responses |
| Path selection | Same-format routes that avoid mutation and normalization choose raw provider passthrough; cross-format or shimmed routes choose maximum-compatible request construction |
| Regressions | Maximum-compatible cross-format behavior remains in the maximum-compatible path |

## Handoff Tasks

Recommended first PR stack:

1. Add `PrimaryExecutionPath` and raw provider passthrough golden tests with no behavior change.
2. Split request execution so raw provider passthrough forwards raw request bytes.
3. Split non-stream and stream response execution so raw provider passthrough forwards raw upstream bytes.
4. Add provider prompt-cache optimizer policy and explicit translated-mode provider prompt-cache request support for Anthropic and OpenAI targets.
5. Add cache-aware routing for provider prompt-cache warmth only after state-bridge route consistency is clear; this can be split into a follow-up PR if it adds routing complexity.
6. Add provider cache usage observation from raw usage while preserving native responses and without introducing any `llmup` cache.
7. Remove or rewrite old same-format mutation tests and update protocol docs.

Primary code areas:

- [src/discovery.rs](../../src/discovery.rs)
- [src/config.rs](../../src/config.rs)
- [src/server/proxy.rs](../../src/server/proxy.rs)
- [src/server/headers.rs](../../src/server/headers.rs)
- [src/streaming/stream.rs](../../src/streaming/stream.rs)
- [src/translate/internal.rs](../../src/translate/internal.rs)
- [src/translate/internal/tests/mod.rs](../../src/translate/internal/tests/mod.rs)
- [tests/integration_test.rs](../../tests/integration_test.rs)

## Explicitly Out Of Scope

- Gateway response cache.
- Semantic cache.
- Any `llmup` cache store, cache lookup, cache eviction, response-reuse cache key, or cache lifecycle manager.
- Universal cache TTL or cache key schema.
- Automatic provider cache marker insertion with no route/global policy, trace, or disable switch.
- Provider-owned state reconstruction for OpenAI Responses or Anthropic thinking/tool state.
- Google/Gemini native cache resource lifecycle management, including `cachedContent`, `cachedContents/*`, `thoughtSignature`, and `extra_body.google.cached_content`.
- Broad fallback, retry, load-balancing, budget, pricing, model catalog, virtual-key, guardrail, prompt-management, or eval features.
- Making `llmup` a LiteLLM/Portkey/OpenRouter-style universal API product.

## Reference Material

Provider official references:

- OpenAI prompt caching: <https://developers.openai.com/api/docs/guides/prompt-caching>
- OpenAI Responses create reference: <https://platform.openai.com/docs/api-reference/responses/create>
- OpenAI Chat Completions create reference: <https://platform.openai.com/docs/api-reference/chat/create>
- Anthropic prompt caching: <https://platform.claude.com/docs/en/build-with-claude/prompt-caching>
- Anthropic OpenAI SDK compatibility: <https://platform.claude.com/docs/en/api/openai-sdk>
- Google Gemini OpenAI compatibility, for migration context only: <https://ai.google.dev/gemini-api/docs/openai>

Comparable gateway references:

- LiteLLM OpenAI passthrough: <https://docs.litellm.ai/docs/pass_through/openai_passthrough>
- LiteLLM prompt caching: <https://docs.litellm.ai/docs/completion/prompt_caching>
- LiteLLM auto-inject prompt caching checkpoints: <https://docs.litellm.ai/docs/tutorials/prompt_caching>
- LiteLLM proxy caching: <https://docs.litellm.ai/docs/proxy/caching>
- Cloudflare AI Gateway provider-native endpoints: <https://developers.cloudflare.com/ai-gateway/usage/providers/>
- Cloudflare AI Gateway caching: <https://developers.cloudflare.com/ai-gateway/features/caching/>
- OpenRouter prompt caching: <https://openrouter.ai/docs/features/prompt-caching>
- OpenRouter response caching: <https://openrouter.ai/docs/guides/features/response-caching>
- Helicone AI Gateway overview: <https://docs.helicone.ai/gateway/overview>
- Helicone provider prompt caching: <https://docs.helicone.ai/gateway/concepts/prompt-caching>
- Helicone LLM caching: <https://docs.helicone.ai/features/advanced-usage/caching>
- Portkey Anthropic prompt caching: <https://portkey.ai/docs/integrations/llms/anthropic/prompt-caching>
- Portkey Bedrock prompt caching: <https://portkey.ai/docs/virtual_key_old/integrations/llms/bedrock/prompt-caching>
- Portkey Messages API provider-native cache note: <https://portkey.ai/docs/product/ai-gateway/messages-api>
- Portkey strict OpenAI compliance: <https://portkey.ai/docs/product/ai-gateway/strict-open-ai-compliance>
- Vercel AI Gateway provider options: <https://vercel.com/docs/ai-gateway/provider-options>
- Vercel AI Gateway OpenAI-compatible advanced prompt caching: <https://vercel.com/docs/ai-gateway/sdks-and-apis/openai-compat/advanced>
- Envoy AI Gateway prompt caching: <https://aigateway.envoyproxy.io/docs/capabilities/llm-integrations/prompt-caching/>
