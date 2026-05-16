# Protocol Baselines Overview

- Layer: capability-diff overview
- Status: active
- Last refreshed: 2026-05-16
- Scope: explains how to read the baseline set without duplicating vendor-specific wire docs

## Why the docs are split

| Layer | What it answers | What it should not answer |
| --- | --- | --- |
| Official baselines | "What does provider X officially document on its own wire format?" | "How should the proxy degrade this into another provider?" |
| Capability diffs | "Which features line up, drift, or stay vendor-specific?" | "What is every field in provider X's request body?" |
| Versioned audits | "What changed in the official surface since the last refresh, and where is the risk?" | "What is the evergreen compatibility story?" |

This split keeps the vendor baselines stable and readable while giving the proxy a separate place to document approximations, unsupported features, and refresh decisions.

During an in-progress refresh, it is normal for snapshot evidence to appear before every vendor baseline file is rewritten. The capability docs and audits are intentionally written to tolerate that staggered state.

The summary matrices deliberately split provider-surface coverage from portability guidance: status cells answer what the official docs expose on that surface, while notes explain what the proxy can safely preserve.

## Baseline inventory

| Provider surface | Official baseline | Primary state model | Streaming model | Tool model | Notes |
| --- | --- | --- | --- | --- | --- |
| OpenAI Responses | [`openai-responses.md`](openai-responses.md) | Resource-oriented: responses, conversations, follow-up IDs, compaction | Rich typed SSE events | Function tools plus hosted tools and remote MCP | This is the fastest-moving surface and now the main source of OpenAI-specific stateful semantics. |
| OpenAI Chat Completions | [`openai-chat-completions.md`](openai-chat-completions.md) | Stateless message replay | Delta SSE plus `[DONE]` | Mostly function-style tool schema | Best portability pivot, but no longer the feature superset in OpenAI's own docs. |
| Anthropic Messages | [`anthropic-messages.md`](anthropic-messages.md) | Stateless messages with growing beta context-management surfaces | Typed SSE block lifecycle | Client tools, server tools, MCP connector | Stop reasons and tool loops carry workflow semantics that do not map cleanly to OpenAI. |
| Google Gemini `generateContent` | [`google-gemini.md`](google-gemini.md) | Stateless request with optional cached-content references | `streamGenerateContent` family | Function declarations plus built-in/server-side tools, including code execution, search, URL context, file search, computer use, and MCP servers | Gemini mixes core request fields with adjacent caching and tool resources. |

## Cross-provider headlines

| Capability | Fast takeaway | Detailed doc |
| --- | --- | --- |
| Reasoning | All four surfaces expose reasoning signals, but only OpenAI Responses treats reasoning as a first-class typed item family. | [`capabilities/reasoning.md`](capabilities/reasoning.md) |
| Caching | OpenAI, Anthropic, and Gemini all support caching, but they model it differently: cache hint, cache breakpoint, and named cache resource. | [`capabilities/cache.md`](capabilities/cache.md) |
| Tools | All four surfaces now document more than plain function calling, but function calling is still the only stable portability core. Hosted/server tools stay vendor-specific. | [`capabilities/tools.md`](capabilities/tools.md) |
| Streaming | All four surfaces support streaming delivery, but only some expose rich named lifecycle events and their terminal semantics do not line up. | [`capabilities/streaming.md`](capabilities/streaming.md) |
| State continuity | OpenAI Responses has the richest explicit state surface. Anthropic and Gemini mostly rely on replay plus provider-specific context helpers. | [`capabilities/state-continuity.md`](capabilities/state-continuity.md) |

## Matrix entrypoints

| Need | Doc |
| --- | --- |
| One-page provider comparison | [`matrices/provider-capability-matrix.md`](matrices/provider-capability-matrix.md) |
| High-risk field mapping and downgrade rules | [`matrices/field-mapping-matrix.md`](matrices/field-mapping-matrix.md) |
| Dated refresh decisions and risks | [`audits/2026-05-16-online-recheck.md`](audits/2026-05-16-online-recheck.md) |

## Working rule for future refreshes

Refresh the vendor baselines when the official docs change shape; refresh the capability docs when the meaning of a feature changes; add a new dated audit whenever the upstream surface changes enough to affect proxy behavior, test fixtures, or compatibility claims.
