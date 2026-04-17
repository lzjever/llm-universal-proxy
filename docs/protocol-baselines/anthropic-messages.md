# Anthropic Messages API — protocol baseline

## Metadata

- `captured_at_utc`: `2026-04-17T07:04:42Z`
- `snapshot_bucket`: `2026-04-16`
- `snapshot_bucket_note`: This capture completed at `2026-04-17T00:04:42-07:00` in `America/Los_Angeles` and remained in the `2026-04-16` bucket because the collection workflow grouped it with the rest of that day's snapshot batch.
- `source_urls`:
  - `https://docs.anthropic.com/en/api/messages`
  - `https://docs.anthropic.com/en/docs/build-with-claude/streaming`
  - `https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/implement-tool-use`
  - `https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking`
  - `https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching`
  - `https://docs.anthropic.com/en/docs/build-with-claude/citations`
  - `https://docs.anthropic.com/en/docs/build-with-claude/token-counting`
  - `https://docs.anthropic.com/en/api/service-tiers`
  - `https://docs.anthropic.com/en/api/beta-headers`
  - `https://docs.claude.com/en/docs/build-with-claude/context-editing`
  - `https://docs.anthropic.com/en/api/handling-stop-reasons`
  - `https://docs.anthropic.com/en/docs/build-with-claude/search-results`
  - `https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/web-search-tool`
  - `https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/code-execution-tool`
  - `https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/fine-grained-tool-streaming`
  - `https://docs.anthropic.com/en/release-notes/api`
- `snapshot_manifest`: `docs/protocol-baselines/snapshots/2026-04-16/anthropic-manifest.json`
- `scope`:
  - Native Anthropic Messages wire contract and closely coupled features that materially change request and response handling.
  - Content blocks, streaming SSE, client tools, server tools, extended thinking, prompt caching, citations, token counting, stop reasons, `context_management`, `container`, and `service_tier`.
- `non_goals`:
  - Claude Managed Agents and Sessions APIs.
  - Model catalog, pricing tables, workspace and admin operations.
  - SDK helper ergonomics that do not change the wire format.
  - Cross-vendor normalization rules.

## Canonical surface

Anthropic's native chat surface is the Messages API.

- `base_api_origin`: `https://api.anthropic.com`
- `documented_endpoints`: `POST /v1/messages`, `POST /v1/messages/count_tokens`

The captured baseline covers these primary endpoints:

| Endpoint | Purpose | Notes |
| --- | --- | --- |
| `POST /v1/messages` | Generate assistant turns | Supports streaming, tools, extended thinking, citations, prompt caching, service tiers, context management, and server tools |
| `POST /v1/messages/count_tokens` | Estimate request input tokens | Accepts the same structured prompt ingredients as `messages.create`; free but separately rate limited |

## Headers and versioning

### Required request headers

| Header | Required | Baseline behavior |
| --- | --- | --- |
| `x-api-key` | Yes | Required authentication header in official examples |
| `anthropic-version` | Yes | Current docs still show `2023-06-01` |
| `content-type: application/json` | Yes | Required for JSON bodies |
| `anthropic-beta` | Conditional | Optional comma-separated feature switches for beta features |

### Versioning rules

- `anthropic-version` is the stable API contract gate.
- `anthropic-beta` is additive. Multiple beta values are comma-separated in a single header.
- Beta names are date-stamped, so wire compatibility is feature-specific, not global.
- Invalid beta names return `invalid_request_error` with `Unsupported beta header: ...`.
- A feature can move from beta to GA and stop requiring a beta header while leaving the payload shape intact. Search results are an example of this.

## Core request contract

### Required top-level fields

| Field | Required | Notes |
| --- | --- | --- |
| `model` | Yes | Anthropic model identifier |
| `max_tokens` | Yes | Maximum assistant output tokens for the sampling call |
| `messages` | Yes | Conversation turns, each with `role` and `content` |

### Common optional top-level fields

Current docs list `system`, `stream`, `metadata`, `stop_sequences`, `temperature`, `top_p`, `top_k`, `tools`, `tool_choice`, `thinking`, `cache_control`, `service_tier`, `container`, and `context_management` as Anthropic-native top-level controls.

### Message structure

- `messages[].role` is `user` or `assistant`
- `system` is a top-level prompt field, not a `messages[].role == "system"` turn
- `messages[].content` accepts either:
  - shorthand string content
  - an array of typed content blocks
- Anthropic docs note that consecutive turns with the same role are combined during processing
- The final assistant turn can be used as a prefill / continuation hint, except when extended thinking is enabled

### Minimal native shape

```json
{
  "model": "claude-sonnet-4-6",
  "max_tokens": 1024,
  "system": "You are a precise assistant.",
  "messages": [
    {
      "role": "user",
      "content": [
        { "type": "text", "text": "Summarize this document." }
      ]
    }
  ]
}
```

## Content block taxonomy

Anthropic Messages is fundamentally a typed-content protocol. Correct implementation depends on preserving block identity and block-local fields.

### Common content block types

| Block type | Typical direction | Notes |
| --- | --- | --- |
| `text` | user and assistant | Assistant `text` blocks may carry `citations` |
| `image` | user input | Standard multimodal input block |
| `document` | user input | Source material for citations; can be text, PDF, file reference, or custom content |
| `search_result` | user input | Search-result content block for custom RAG; can include `citations: { "enabled": true }` |
| `tool_use` | assistant output | Client-tool invocation with `id`, `name`, and structured `input` |
| `tool_result` | user input | Returned to Claude in the next user turn; must point at a prior `tool_use_id` |
| `thinking` | assistant output | Extended-thinking block; includes `thinking` text and `signature` |
| `redacted_thinking` | assistant output | Preserve verbatim; appears when thinking content is not exposed in cleartext |
| `container_upload` | user input | File reference block for code execution containers |
| `server_tool_use` | assistant output | Anthropic-managed tool invocation, e.g. `web_search` or `code_execution` |
| `web_search_tool_result` | assistant output | Anthropic-managed web-search result block tied to a `server_tool_use` |

### Taxonomy notes that matter to implementers

- `tool_result.content` is not limited to plain text; it can carry structured content and must preserve `tool_use_id` linkage.
- `search_result` is a real input block type, not just a citation payload.
- `web_search_result` objects appear inside server-tool result payloads; they are not the same thing as top-level `search_result` input blocks.
- `thinking` and `redacted_thinking` are assistant-side protocol objects, not debug metadata.
- Citation payloads live on `text` blocks; they are not separate content blocks.

### Preservation rules

- Preserve unknown block `type` values and unknown fields verbatim.
- Preserve block order exactly.
- Preserve block-local metadata such as citation spans, signatures, tool IDs, source URLs, titles, and cache controls.
- Never coerce Anthropic block arrays into OpenAI's `content: string | parts[]` abstraction on the Anthropic-native path.

## Response contract

Non-streaming responses return a top-level assistant message object with the usual Anthropic fields:

- `id`
- `type: "message"`
- `role: "assistant"`
- `model`
- `content`
- `stop_reason`
- `stop_sequence`
- `usage`

### Usage object expectations

The exact `usage` shape is feature-dependent and may grow new subfields. Common fields across the captured docs include:

- `input_tokens`
- `output_tokens`
- `cache_creation_input_tokens`
- `cache_read_input_tokens`
- `service_tier`
- `server_tool_use` (for example, `web_search_requests`)

Do not assume `usage` only contains input and output token counts. In streaming responses, `stop_reason` remains `null` until `message_delta`.

## Streaming: SSE event model and delta subtypes

Anthropic streaming is Server-Sent Events, not a JSON-lines stream. Each event has an SSE event name and a JSON body whose `type` mirrors the event name.

### Canonical event flow

| Event | Meaning |
| --- | --- |
| `message_start` | Starts the stream with a `Message` object whose `content` is empty |
| `content_block_start` | Opens a content block at `index` |
| `content_block_delta` | Emits incremental data for the open block |
| `content_block_stop` | Closes the current block |
| `message_delta` | Emits final top-level deltas such as `stop_reason`, `stop_sequence`, and cumulative `usage` |
| `message_stop` | Terminates the stream |
| `ping` | Keepalive event; may appear anywhere |
| `error` | In-band stream error event, e.g. overload conditions |

Anthropic explicitly warns that new event types may appear over time. A robust parser must ignore or surface unknown events without failing the whole stream.

### Delta subtypes captured in current docs

| Delta subtype | Where it appears | Handling |
| --- | --- | --- |
| `text_delta` | text blocks | Append `delta.text` to the current text buffer |
| `input_json_delta` | `tool_use` or streamed server-tool input | Append raw `partial_json`; do not require each chunk to be valid JSON |
| `thinking_delta` | `thinking` blocks | Append `delta.thinking` |
| `signature_delta` | `thinking` blocks | Capture signature material emitted immediately before block close |
| `citations_delta` | text blocks with citations | Add citation entries to the current text block citation list |

### Streaming rules that are easy to get wrong

- `content_block_*` events are keyed by `index`, and the final `content[]` array uses the same indices.
- `message_delta` usage counts are cumulative, not per-event increments.
- `stop_reason` and `stop_sequence` stay `null` until the final `message_delta`.
- For current models, `input_json_delta` is emitted one complete key/value property at a time, so tool-input streams may pause between chunks.
- Fine-grained tool streaming can deliver invalid or partial JSON while the tool input is still being generated.
- With `thinking.display: "omitted"`, no `thinking_delta` text is sent; the block opens, gets a `signature_delta`, and closes.

### Streaming assembler requirements

1. Initialize an empty message on `message_start`.
2. Create a block slot on each `content_block_start`.
3. Append deltas by block `index`.
4. Treat `input_json_delta.partial_json` as a byte/string accumulator until block close.
5. Merge `citations_delta` into the active text block's citation array.
6. Update final `stop_reason`, `stop_sequence`, and `usage` from `message_delta`.
7. Ignore unknown event types safely.

## Tools: client tools vs. server tools

Anthropic has two distinct tool classes and they should not be conflated.

### Client tools

Client tools are declared in the request's top-level `tools` array. This includes both user-defined tools and Anthropic-schema client tools.

Each client tool definition includes:

| Field | Meaning |
| --- | --- |
| `name` | Tool name, matching Anthropic's naming constraints |
| `description` | Plaintext description of what the tool does and when to use it |
| `input_schema` | JSON Schema describing tool arguments |
| `input_examples` | Optional examples to help Claude choose and structure arguments |

Additional optional tool-definition properties referenced by Anthropic docs include:

- `cache_control`
- `strict`
- `defer_loading`
- `allowed_callers`

Important constraint:

- `input_examples` are not supported for server-side tools such as web search or code execution.

### Tool choice

Anthropic documents four `tool_choice` modes:

| `tool_choice.type` | Meaning |
| --- | --- |
| `auto` | Claude decides whether to call tools; default when tools are present |
| `any` | Claude must use one of the provided tools |
| `tool` | Claude must use the named tool |
| `none` | Disable tool use; default when no tools are present |

Behavioral notes:

- With `any` or `tool`, Anthropic prefills the assistant turn so Claude goes straight to `tool_use` rather than emitting natural-language lead-in text.
- Release notes document `disable_parallel_tool_use: true` under `tool_choice` to ensure Claude uses at most one tool in a turn.

### Server tools

Server tools are Anthropic-managed capabilities such as web search and code execution. They differ from client tools in three important ways:

1. Anthropic executes them inside the same sampling turn.
2. The assistant may emit `server_tool_use` and tool-specific result blocks such as `web_search_tool_result`.
3. The turn can end with `pause_turn`, requiring the caller to continue the turn by resubmitting the assistant output.

Current docs treat `server_tool_use` and tool-specific result blocks as assistant-side content, distinct from client-tool `tool_use` / `tool_result` loops.

### Fine-grained tool streaming

The current feature docs describe fine-grained tool streaming as available on all models and platforms.

Enable it by:

- setting `stream: true` on the request
- setting `eager_input_streaming: true` on a user-defined tool

Tool-input streams may be incomplete or temporarily invalid JSON while generation is still in progress.

## Extended thinking

Extended thinking materially changes the request contract, block taxonomy, tool constraints, and caching behavior.

### Supported configurations

Anthropic's current docs distinguish between adaptive and manual thinking:

- For Claude Opus 4.7 and later, use adaptive thinking: `thinking: { "type": "adaptive" }` with `effort`
- Manual thinking on Opus 4.7+ (`{ "type": "enabled", "budget_tokens": N }`) returns a 400
- Manual thinking is still accepted on some earlier current models but is deprecated
- Manual `budget_tokens` minimum is `1024`

### Display modes

- `display` defaults to `"omitted"`
- Use `display: "summarized"` to receive thinking summaries
- When display is omitted, streaming emits only `signature_delta` for the thinking block, not `thinking_delta`

### Compatibility constraints

Anthropic explicitly documents these constraints when thinking is enabled:

- incompatible with `temperature` modifications
- incompatible with `top_k` modifications
- forced tool use is disallowed
- only `tool_choice: { "type": "auto" }` or `{ "type": "none" }` is allowed
- `top_p` may be set only within the documented narrow range (`1` down to `0.95`)
- response prefilling is not allowed

### Tool-loop preservation rules

Anthropic's tool-use guidance for thinking is unusually strict:

- During a tool loop, pass the previous assistant turn's `thinking` blocks back unmodified.
- Treat the entire thinking-plus-tool exchange as one conceptual assistant turn.
- Do not redact, summarize, or regenerate preserved thinking blocks when replaying a tool loop.

### Counting and context-window behavior

- Previous assistant-turn thinking blocks are ignored for context-window accounting.
- The token-counting docs say previous assistant-turn thinking does not count toward input tokens.
- Current assistant-turn thinking does count toward input tokens.

### Caching interaction

- Changing thinking budget invalidates cached prompt prefixes that include messages.
- Cached system prompts and tool definitions can still remain valid when thinking parameters change.

Some SDKs require streaming when `max_tokens > 21333` to avoid client-side HTTP timeouts. That is an SDK behavior, not a wire-level API restriction.

## Prompt caching

Prompt caching is a native Anthropic feature with its own TTLs, pricing, and invalidation rules.

### Two activation modes

| Mode | How to enable | Best for |
| --- | --- | --- |
| Automatic caching | Top-level `cache_control` on the request | Long multi-turn conversations where Anthropic should move the cache breakpoint forward automatically |
| Explicit cache breakpoints | `cache_control` on specific cacheable blocks | Stable tool definitions, documents, or prompt segments you want to cache independently |

### Cache scope and TTL

- Caching operates over the full prompt prefix in Anthropic order: `tools`, `system`, then `messages`
- Default TTL is 5 minutes
- 1-hour TTL is available with `cache_control: { "type": "ephemeral", "ttl": "1h" }`
- Automatic caching consumes one of Anthropic's available cache breakpoint slots, so block-level and automatic caching can coexist

### Pricing and usage semantics from Anthropic docs

- 5-minute cache writes cost 25% more than base input tokens
- 1-hour cache writes cost 2x base input token price
- Cache reads cost 10% of base input token price
- Cache breakpoints themselves are free; cost depends on cache reads/writes plus uncached input

### Thinking and citation interactions

- Thinking blocks cannot be directly marked with `cache_control`
- Thinking blocks can still be cached indirectly when they appear in previous assistant turns
- When read from cache, those cached thinking blocks do count as input tokens
- Citation sub-content cannot be cached directly; cache the top-level document or search-result source block instead

### Cache invalidation patterns called out by Anthropic

- Cache remains valid when new user content is only tool results
- Cache invalidates when non-`tool_result` user content is added, which causes previous thinking blocks to be stripped from the cached message prefix
- This behavior matters even when callers are not manually placing cache markers on those later turns

### Operational notes

- Prompt caching is eligible for Zero Data Retention
- `usage` may include both `cache_creation_input_tokens` and `cache_read_input_tokens`

## Context management, container reuse, and service tiers

These are separate Anthropic-native controls with their own request and response fields.

### `context_management`

Context editing is currently beta and requires `anthropic-beta: context-management-2025-06-27`.

Documented strategies in the captured docs:

| Strategy | Purpose |
| --- | --- |
| `clear_tool_uses_20250919` | Clear older tool results when context grows |
| `clear_thinking_20251015` | Clear prior thinking blocks |

Response-side behavior:

- Anthropic returns `context_management.applied_edits`
- Applied edits report counts such as cleared tool uses, cleared thinking turns, and cleared input tokens

Default behavior note:

- When thinking is enabled and the caller does not explicitly configure `clear_thinking_20251015`, Anthropic defaults to keeping only thinking blocks from the most recent assistant turn
- Anthropic recommends preserving all thinking blocks with `keep: "all"` when maximizing cache hits matters

### `container`

Anthropic's `container` parameter is relevant for server tools, especially code execution.

Documented behavior from the code execution docs:

- You can reuse a container across requests by sending the container ID from a previous response
- Reused containers retain created files between requests
- Files API uploads can then be referenced from message content with `container_upload` blocks

Container IDs and container configuration are opaque Anthropic-native values; the captured docs define reuse by ID but not a separate protocol-level lifetime policy.

### `service_tier`

Anthropic service-tier selection is request-native, not account metadata.

| Value | Meaning |
| --- | --- |
| `auto` | Default; use Priority Tier if available, otherwise standard capacity |
| `standard_only` | Never consume Priority Tier capacity |

Response and header behavior:

- The actual assigned tier is returned as `usage.service_tier`
- When `service_tier = "auto"` and the request is eligible for priority commitment, Anthropic may return:
  - `anthropic-priority-input-tokens-limit`
  - `anthropic-priority-input-tokens-remaining`
  - `anthropic-priority-input-tokens-reset`
  - `anthropic-priority-output-tokens-limit`
  - `anthropic-priority-output-tokens-remaining`
  - `anthropic-priority-output-tokens-reset`

Proxy guidance:

- Preserve both the request field and any returned priority-capacity headers
- Do not remap service tier to a generic "priority" boolean

## Citations and search results

Anthropic has two related but distinct citation-bearing input families: documents and search results.

### Document citations

Enable citations on `document` blocks with:

```json
{
  "type": "document",
  "citations": { "enabled": true }
}
```

Key behaviors from the docs:

- All active models support citations except Haiku 3
- Citation spans are returned on assistant `text` blocks
- `cited_text` does not count toward output tokens
- When passed back on later turns, `cited_text` also does not count toward input tokens
- Citations are compatible with prompt caching, token counting, and batch processing
- Citations are incompatible with structured outputs (`output_config.format` or deprecated `output_format`); Anthropic returns 400
- Generated citation blocks cannot be cached directly, but source documents can
- Citations are eligible for Zero Data Retention

### Citation streaming

Streaming responses add `citations_delta`, which appends a single citation object to the current text block.

Build citations as part of the text block state machine; the captured docs do not model them as independent top-level blocks.

### Search-result content blocks

Search-result blocks let callers provide custom-RAG material while preserving Claude-style citations.

Documented schema shape:

```json
{
  "type": "search_result",
  "source": "https://example.com/article",
  "title": "Article Title",
  "content": [
    { "type": "text", "text": "Search result text..." }
  ],
  "citations": { "enabled": true }
}
```

Documented usage patterns:

- search results can come from tool calls
- search results can be included directly as top-level user message content
- search-result citations use `type: "search_result_location"`

Search-result citation payloads include fields such as:

- `source`
- `title`
- `cited_text`
- `search_result_index`
- `start_block_index`
- `end_block_index`

Release-note status:

- Search-result content blocks are GA as of the captured docs; the older `search-results-2025-06-09` beta header is no longer required.

## Token counting

Anthropic exposes token counting at `POST /v1/messages/count_tokens`.

### Contract

- Accepts the same structured prompt ingredients as Messages creation, including tools, images, PDFs, and thinking-aware message histories
- Returns the total estimated input token count
- Counts are estimates, not billing-grade exact replicas of the eventual sampling call

### Important counting rules from the docs

- Token counts may include Anthropic-added system optimization tokens
- You are not billed for those system-added tokens
- Server-tool token counts only apply to the first sampling call
- Previous assistant-turn thinking blocks are ignored
- Current assistant-turn thinking does count

### Pricing and limits

- Token counting is free to use
- It has requests-per-minute limits based on usage tier
- Anthropic documents this separately from message-creation billing and capacity semantics, so callers should not assume identical throughput constraints

Anthropic documents this endpoint separately from message creation and returns its own estimated total, including system-added optimization tokens.

## Stop reasons

Anthropic's `stop_reason` is part of successful responses and must be handled as control flow, not as an exception.

### Documented stop reasons

| Stop reason | Meaning | Required caller behavior |
| --- | --- | --- |
| `end_turn` | Claude finished naturally | Treat as a completed assistant turn |
| `max_tokens` | Hit requested output-token cap | Caller may continue with a follow-up request if more output is needed |
| `stop_sequence` | Matched a custom stop sequence | Respect returned `stop_sequence` |
| `tool_use` | Claude wants a client-side tool call | Execute tool and return `tool_result` in the next user turn |
| `pause_turn` | Anthropic paused a long-running server-tool turn | Continue by sending the assistant response back in a subsequent request |
| `refusal` | Safety refusal | Surface refusal state to caller |
| `model_context_window_exceeded` | Model exhausted context window before finishing | Treat as a partial but valid response; consider continuation or prompt reduction |

### Model-version notes

- `model_context_window_exceeded` is enabled by default on Sonnet 4.5 and newer
- Earlier models require `anthropic-beta: model-context-window-exceeded-2025-08-26`

### Streaming notes

- In streaming, `stop_reason` is `null` until the final `message_delta`
- `stop_reason` is therefore not knowable from `message_start`

### Tool-loop best practices from Anthropic docs

- Always branch on `stop_reason`
- On `pause_turn`, resend the assistant response to continue rather than treating it as failure
- On `max_tokens`, be careful with incomplete `tool_use` blocks; retry with a higher limit if the tool input was truncated
- After returning `tool_result`, do not append extra user text unless you intentionally want to change the prompt pattern; Anthropic specifically warns this can cause empty `end_turn` responses

## See also

See also [the Anthropic snapshot manifest](snapshots/2026-04-16/anthropic-manifest.json).
