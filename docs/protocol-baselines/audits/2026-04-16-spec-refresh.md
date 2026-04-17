# Protocol Spec Refresh Audit - 2026-04-16

- Layer: versioned audit
- Status: active snapshot of the docs refresh performed on 2026-04-16
- Compared against: repo baselines captured on 2026-04-07 and the in-progress 2026-04-16 vendor baseline / manifest refresh present in the repo
- Interpretation note: when a feature is documented in a guide, beta surface, or adjacent reference page rather than the narrow create endpoint, this audit labels it explicitly instead of presenting it as universal GA wire behavior

## Scope

This refresh audited the current official docs for:

- OpenAI Responses and Chat Completions
- Anthropic Messages and adjacent context/tool guides
- Google Gemini `generateContent` and adjacent caching/tool guides

The goal was not to rewrite the vendor baselines in place. The goal was to reorganize the repository docs so the vendor baselines remain factual, while capability diffs and refresh risk notes live elsewhere.

## Executive summary

The official surface has continued to widen in ways that matter for a protocol proxy:

1. OpenAI Responses is now even more clearly the stateful and tool-rich control plane, not just an alternative create endpoint.
2. Anthropic Messages now has materially more documented state and tool-adjacent behavior around prompt caching, MCP, context management, and stop-reason-driven workflow control.
3. Gemini `generateContent` still looks simple at first glance, but the practical contract now spans core request fields, cache resources, tool configuration, and usage metadata that the old repo docs barely surfaced.
4. The biggest implementation risk is no longer plain request-shape mismatch. It is pretending that provider-managed state, hosted tools, and streaming workflow semantics are portable when they are not.

## What the refresh surfaced

| Provider | Refreshed surface | What is newly prominent relative to the repo's 2026-04-07 docs | Why it matters |
| --- | --- | --- | --- |
| OpenAI Responses | Core reference plus tool, caching, streaming, background, and conversation docs | Responses create now sits alongside first-class conversations, compaction, background mode, prompt-cache controls, richer `include` expansions, and remote MCP / hosted tool families | The proxy must treat Responses as a stateful platform surface, not merely "Chat with a different request body" |
| OpenAI Chat | Current API reference and model docs | Chat still matters for compatibility, but OpenAI's own docs keep steering new builds toward Responses for advanced stateful and agentic features | Chat should remain the compatibility pivot, not the feature superset |
| Anthropic Messages | Messages, prompt caching, stop reasons, MCP connector, release notes, beta reference surfaces | Prompt caching now has top-level automatic mode, stop reasons document workflow-specific states like `pause_turn`, and beta surfaces expose `context_management`, containers, and `mcp_servers` | Messages is no longer just a stateless prompt/response format in practice |
| Gemini `generateContent` | GenerateContent reference, function-calling guide, caching guide, and discovery-backed manifests | Core request now explicitly includes `serviceTier` and `store`; the official `Tool` schema covers built-in/server-side tools such as search, URL context, file search, computer use, and `mcpServers`; usage metadata exposes cache, tool-use, and thought-token counters | Gemini compatibility needs to distinguish official surface coverage from portability and from adjacent cache/resource guides |

## Verified upstream changes that should affect our compatibility story

| Area | Verified in current official docs | Documentation impact |
| --- | --- | --- |
| OpenAI Responses state surfaces | Current OpenAI API reference navigation includes responses, conversations, items, and compaction resources; create-response docs and model docs also point to background mode and multi-turn stateful behavior | The repo needs a dedicated state-continuity note instead of burying this inside a flat compatibility matrix |
| OpenAI Responses rich tool and include surface | Current OpenAI docs enumerate hosted tools and `include` expansions such as web-search sources, code-interpreter outputs, computer-use image URLs, and `reasoning.encrypted_content` | Tool portability and reasoning portability must be documented separately from vendor baseline facts |
| Anthropic prompt caching | Current Anthropic prompt-caching docs describe automatic top-level `cache_control`, block-level breakpoints, 5-minute default TTL, optional 1-hour TTL, and read/write token accounting | Cache docs must call out that Anthropic caching is neither OpenAI prompt caching nor Gemini cached-content resources |
| Anthropic stop reasons | Current Anthropic stop-reason docs explicitly document `pause_turn`, `refusal`, and `model_context_window_exceeded`; streaming refusals are also documented | Streaming docs must distinguish "workflow pause" from "clean completion" and "successful stop" from "downstream error semantic" |
| Anthropic MCP and beta state surfaces | Current Anthropic MCP docs document a current beta header for the MCP connector, and current reference snippets expose beta fields like `context_management`, `container`, `service_tier`, and `mcp_servers` | These are implementation-risk areas and should be flagged as same-provider or beta-only paths |
| Gemini cache and usage expansion | Current Gemini docs describe `cachedContent`, explicit cache resources, implicit caching guidance, `serviceTier`, `store`, and usage metadata fields such as `cachedContentTokenCount`, `toolUsePromptTokenCount`, and `thoughtsTokenCount` | Gemini docs need a clearer split between core request compatibility and adjacent cache/resource features |

## Implementation risk register

| Risk | Severity | Why it is risky | Recommended stance |
| --- | --- | --- | --- |
| OpenAI Responses conversations, `previous_response_id`, and compaction | High | These features imply upstream-owned state. The proxy does not reconstruct that state across providers. | Passthrough when routable; otherwise fail clearly or drop with an explicit warning. |
| OpenAI hosted tools and rich `include` payloads | High | Hosted tools now produce structured artifacts that do not map to Chat, Anthropic, or Gemini function-tool payloads. | Keep function tools as the portability core; do not synthesize equivalent hosted-tool shapes. |
| OpenAI background mode | Medium | It changes lifecycle expectations and can outlive a simple request/response mental model. | Treat as same-provider behavior only. |
| Anthropic `pause_turn` and server-tool loops | High | A successful Anthropic response may still mean "continue the loop," not "assistant is done." | Preserve pause semantics for Anthropic and normalize carefully for Responses-native clients. |
| Anthropic `model_context_window_exceeded` | High | Anthropic reports it as a successful stop reason, while OpenAI-style clients often expect a hard failure semantic. | Keep the current explicit normalization and document it as approximation, not equivalence. |
| Anthropic beta context-management surfaces | High | `context_management`, containers, and MCP server fields introduce stateful behavior outside the proxy's current portability contract. | Mark as vendor-specific and beta-scoped. Do not claim cross-provider support. |
| Anthropic prompt-cache accounting | Medium | Separate read/write token counters do not collapse cleanly into OpenAI/Gemini-style cached-token views. | Preserve raw values where possible; document normalized counters as approximate. |
| Gemini `cachedContent` | High | It is a named cache resource, not a follow-up response handle or prompt-cache hint. | Drop outside Gemini passthroughs and warn. |
| Gemini usage metadata expansion | Medium | Thought tokens, cached-content tokens, and tool-use prompt tokens are useful but not directly isomorphic to OpenAI or Anthropic counters. | Preserve for Gemini clients; provide best-effort rollups elsewhere. |
| Chat as an assumed superset | Medium | Current official OpenAI docs place advanced stateful and tool-rich behavior in Responses, not Chat. | Keep Chat as a broad compatibility pivot only. |

## Documentation outcome of this refresh

| Need | New doc |
| --- | --- |
| Explain the new three-layer structure | [`../overview.md`](../overview.md) |
| Capture feature-by-feature drift | [`../capabilities/`](../capabilities/) |
| Keep a stable one-page comparison | [`../matrices/provider-capability-matrix.md`](../matrices/provider-capability-matrix.md) |
| Centralize risky field mappings | [`../matrices/field-mapping-matrix.md`](../matrices/field-mapping-matrix.md) |

## Provenance for this audit

This audit inherits upstream provenance from the vendor baselines and the 2026-04-16 manifests. Use those files for exact source URLs, snapshot inventories, and checksums instead of maintaining a second divergent source list here.

| Provider | Baseline / manifest provenance |
| --- | --- |
| OpenAI | [`../openai-responses.md`](../openai-responses.md), [`../openai-chat-completions.md`](../openai-chat-completions.md), [`../snapshots/2026-04-16/openai-manifest.md`](../snapshots/2026-04-16/openai-manifest.md) |
| Anthropic | [`../anthropic-messages.md`](../anthropic-messages.md), [`../snapshots/2026-04-16/anthropic-manifest.json`](../snapshots/2026-04-16/anthropic-manifest.json) |
| Gemini | [`../google-gemini.md`](../google-gemini.md), [`../snapshots/2026-04-16/google-manifest.json`](../snapshots/2026-04-16/google-manifest.json) |

## Bottom line

The biggest change in this refresh is structural: the repo now treats official protocol baselines, cross-provider capability differences, and dated audit findings as separate deliverables. That matches the current ecosystem, where the official APIs are adding optional fields, tool families, and state surfaces faster than a single flat matrix can explain safely.
