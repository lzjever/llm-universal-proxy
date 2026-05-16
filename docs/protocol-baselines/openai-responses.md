# OpenAI Responses API baseline

## Metadata

- `captured_at_utc`: `2026-04-17T06:59:44Z`
- `snapshot_bucket`: `2026-04-16`
- `snapshot_bucket_note`: Snapshot artifacts are stored under the `2026-04-16` bucket because this capture completed at `2026-04-16T23:59:44-07:00` in `America/Los_Angeles`.
- `proxy_posture_updated`: `2026-04-26`
- `online_recheck_at_utc`: `2026-05-16T00:00:00Z`
- `source_urls`:
  - `https://developers.openai.com/api/reference/resources/responses/index.md`
  - `https://developers.openai.com/api/reference/resources/conversations/index.md`
  - `https://developers.openai.com/api/docs/guides/conversation-state`
  - `https://developers.openai.com/api/docs/guides/prompt-caching`
  - `https://developers.openai.com/api/docs/guides/reasoning`
  - `https://developers.openai.com/api/docs/guides/streaming-responses`
  - `https://developers.openai.com/api/docs/guides/migrate-to-responses`
- `snapshot_manifest`: `docs/protocol-baselines/snapshots/2026-04-16/openai-manifest.md`
- `scope`:
  - Official HTTP wire surface for Responses and the linked Conversations resource family.
  - Request and response shapes, typed items, streaming event families, reasoning, prompt caching, and persistence.
- `non_goals`:
  - SDK helper APIs or language-specific ergonomics.
  - Pricing, rate limits, and model quality comparisons.
  - Realtime transport internals beyond the captured HTTP and SSE semantics.

Responses is the primary OpenAI surface for new builds. The captured migration guide describes it as a superset of Chat Completions, and the streaming guide explicitly recommends it over Chat for semantic, type-safe streaming. See also: [OpenAI Chat Completions API baseline](openai-chat-completions.md).

## Formal surface

The captured Responses baseline spans both `/responses` and `/conversations` resources.

### Response resources

- `POST /v1/responses`
- `GET /v1/responses/{response_id}`
- `DELETE /v1/responses/{response_id}`
- `POST /v1/responses/{response_id}/cancel`
- `POST /v1/responses/compact`
- `GET /v1/responses/{response_id}/input_items`
- `POST /v1/responses/input_tokens`

### Conversation resources

- `POST /v1/conversations`
- `GET /v1/conversations/{conversation_id}`
- `POST /v1/conversations/{conversation_id}`
- `DELETE /v1/conversations/{conversation_id}`
- `POST /v1/conversations/{conversation_id}/items`
- `GET /v1/conversations/{conversation_id}/items`
- `GET /v1/conversations/{conversation_id}/items/{item_id}`
- `DELETE /v1/conversations/{conversation_id}/items/{item_id}`

This captured baseline includes the response lifecycle resources and the linked conversation resources, not just `POST /v1/responses`.

Proxy support posture: these lifecycle and state resources are supported on `/openai/v1/...` and `/namespaces/{namespace}/openai/v1/...` only as native OpenAI Responses pass-through. The namespace must resolve to exactly one available upstream that natively supports OpenAI Responses. The proxy preserves method, query, JSON body, and forwardable protocol/auth headers, percent-encodes resource ID path segments before upstream forwarding, and fails closed rather than reconstructing response, conversation, or item ownership across providers.

## Request baseline

### Core envelope

The top-level request surface centers on:

- `model`
- `input`
- `instructions`
- `store`
- `stream`
- `background`
- `metadata`

The captured reference also includes operational controls such as:

- `parallel_tool_calls`
- `max_output_tokens`
- `max_tool_calls`
- `temperature`
- `top_p`
- `truncation`
- `stream_options.include_obfuscation`
- `service_tier`
- `safety_identifier`
- `prompt`

### Input model

`input` may be:

- a plain string
- an array of typed items

The typed-item surface is one of the defining formal differences from Chat. Common item families in the captured reference include:

- `message`
- `function_call_output`
- `custom_tool_call_output`
- prior assistant messages
- item references
- reasoning items when you manually manage context

Message content can itself be multimodal, including:

- `input_text`
- `input_image`
- `input_audio`
- `input_file`

### Instructions and state

Responses splits "what the user said" from "how the model should behave":

- `input` is the turn payload
- `instructions` is the system or developer directive inserted into context

The captured create reference documents a subtle but important behavior:

- when you continue via `previous_response_id`, prior `instructions` are not automatically carried into the next response
- this makes it easy to swap system or developer instructions between turns

State controls are first-class:

- `previous_response_id` creates stateless continuation chains
- `conversation` binds the request to a server-side conversation object
- `previous_response_id` and `conversation` cannot be used together

The captured `conversation` semantics are explicit:

- items from the conversation are prepended to the request
- input and output items from the response are appended back into that conversation when the response completes

### Context management and compaction

Responses adds explicit context management features absent from Chat:

- `context_management`
- `truncation`
- `POST /v1/responses/compact`

In the captured create reference, `context_management` currently supports compaction entries with:

- `type`
- `compact_threshold`

The captured compact endpoint returns a compacted response object. This is part of the formal surface, not just a guide-only idea.

Proxy posture: `context_management` and compact resources are native OpenAI Responses state surfaces. Native OpenAI Responses passthrough preserves them, including compaction input items with `encrypted_content`. Cross-provider request translation in strict/balanced modes fails closed for request-side compaction input. In the default/max_compat lane, request-side compaction input items degrade only when each degraded compaction item has explicit summary text, or when the request contains non-compaction visible portable transcript/history. The proxy warns and drops provider-owned opaque fields such as `encrypted_content` without parsing, decrypting, forwarding, or synthesizing them, and preserves explicit summary text or visible portable transcript as ordinary context. Opaque-only compaction input still fails closed, and one summarized compaction item does not permit another opaque-only compaction item to be silently dropped.

### Tool surface

The captured Responses tool surface includes both user-defined tools and built-in tools.

User-defined:

- function tools
- custom tools
- namespaced tool groups

Built-in:

- `file_search`
- `web_search` and preview/versioned variants
- `code_interpreter`
- `computer_use_preview` and related computer-use variants
- `image_generation`
- remote `mcp`

Important control fields:

- `tools`
- `tool_choice`
- `parallel_tool_calls`
- `max_tool_calls`

Compared with Chat, Responses exposes much richer built-in tool state directly in typed output items and streaming events.

### Reasoning and output shaping

Responses reasoning is first-class. The captured request field is `reasoning`, with:

- `effort`
- `summary`
- deprecated `generate_summary`

Current documented `reasoning.effort` values:

- `none`
- `minimal`
- `low`
- `medium`
- `high`
- `xhigh`

Model notes captured in the reference:

- `gpt-5.1` defaults to `none` and supports `none`, `low`, `medium`, and `high`
- models before `gpt-5.1` default to `medium` and do not support `none`
- `gpt-5-pro` defaults to and only supports `high`
- `xhigh` is supported for models after `gpt-5.1-codex-max`

`max_output_tokens` includes visible output tokens and reasoning tokens.

Output shaping uses Responses-native controls:

- `text`
- `include`

The captured `text` object is not just a format wrapper. It includes:

- `format`
- `verbosity`

The captured `include` surface is especially important because it gates extra fields that are otherwise absent from typed items. Examples include:

- `reasoning.encrypted_content`
- `message.output_text.logprobs`
- `file_search_call.results`
- `web_search_call.action.sources`
- `message.input_image.image_url`
- `computer_call_output.output.image_url`
- `code_interpreter_call.outputs`

Proxy posture: `reasoning.encrypted_content` is opaque reasoning-continuity state. Native OpenAI Responses passthrough preserves `include: ["reasoning.encrypted_content"]` and request input reasoning items with `encrypted_content` exactly. Cross-provider request translation in strict/balanced modes fails closed for request-side reasoning encrypted_content and for `include: ["reasoning.encrypted_content"]`. In the default/max_compat lane, cross-provider `include: ["reasoning.encrypted_content"]` is warned and dropped. Reasoning item `encrypted_content` may be dropped without parsing, decoding, or replaying it only when visible summary text or visible transcript/history remains; if the reasoning item has `summary`, only that summary is reused as unsigned reasoning/thinking. Opaque-only reasoning fails closed even in default/max_compat. Proxy-local carrier strings that encode Anthropic signed or omitted thinking provenance are never replayed into another provider's request history.

## Response baseline

### Top-level object

The non-streaming response object is `response` with fields including:

- `id`
- `object`
- `created_at`
- `status`
- `error`
- `incomplete_details`
- `output`
- `usage`
- optional `conversation`

This is not a chat-style `choices[]` envelope. Clients are expected to interpret `output[]` by item type.

### Typed output items

The captured Responses reference defines a broad typed item graph. Common output item families include:

- `message`
- `reasoning`
- `function_call`
- `function_call_output`
- `custom_tool_call`
- `custom_tool_call_output`
- `file_search_call`
- `web_search_call`
- `code_interpreter_call`
- `mcp_call`
- `image_generation_call`
- `compaction`

Important reasoning-specific behavior:

- reasoning items can carry `summary`
- reasoning items can carry `encrypted_content` when explicitly requested via `include`
- the reference says encrypted reasoning enables reasoning items to be reused across turns when managing context statelessly

### Usage accounting

Responses exposes prompt-cache and reasoning accounting under a different shape than Chat:

- `usage.input_tokens_details.cached_tokens`
- `usage.output_tokens_details.reasoning_tokens`

Those fields expose cache effectiveness and reasoning-token consumption on this surface.

### Retrieval helpers

The formal response surface includes retrieval helpers that matter for correctness:

- `GET /v1/responses/{response_id}/input_items` returns the normalized input items associated with a response
- `POST /v1/responses/input_tokens` estimates token usage for the Responses input surface, including conversation-aware requests
- `GET /v1/responses/{response_id}?stream=true` retrieves a stored or in-progress response as semantic SSE; `starting_after` and `include_obfuscation` are stream-specific query controls

## Streaming events

Responses streaming is semantic SSE, not chat-style chunk aggregation.

The captured reference includes a canonical event sequence:

- `response.created`
- `response.in_progress`
- `response.output_item.added`
- `response.content_part.added`
- `response.output_text.delta`
- `response.output_text.done`
- `response.content_part.done`
- `response.output_item.done`
- `response.completed`

Terminal outcomes are semantic:

- `response.completed`
- `response.incomplete`
- `response.failed`

The separately captured streaming guide also documents a generic `error` event in streaming examples.

The current reference enumerates many more event families. Important groups include:

- output assembly: `response.output_item.*`, `response.content_part.*`, `response.output_text.*`
- function and tool arguments: `response.function_call_arguments.*`, `response.custom_tool_call_input.*`, `response.mcp_call_arguments.*`
- reasoning: `response.reasoning_summary_part.*`, `response.reasoning_summary_text.*`, `response.reasoning_text.*`
- refusal and audio: `response.refusal.*`, `response.audio.*`, `response.audio.transcript.*`
- built-in tool progress: `response.file_search_call.*`, `response.web_search_call.*`, `response.code_interpreter_call.*`, `response.image_generation_call.*`, `response.mcp_call.*`
- lifecycle and compaction: `response.created`, `response.in_progress`, `response.completed`, `response.incomplete`, `response.failed`, `response.compaction`

Protocol consequences:

- do not collapse the event stream into Chat-style anonymous deltas
- preserve exact event names and typed payloads
- tolerate additional event families without failing closed
- there is no documented `data: [DONE]` sentinel in the captured Responses reference; termination is semantic via terminal event and stream completion

Like Chat, Responses also supports `stream_options.include_obfuscation`, and the docs say obfuscation fields are included by default unless disabled.

## Conversation state

The captured conversation-state guide makes Responses state management much more explicit than Chat.

There are three practical modes:

1. Fully manual: resend the full input history yourself.
2. Chained stateless mode: send `previous_response_id`.
3. Server-managed mode: bind requests to a `conversation`.

Key semantics from the captured official docs:

- `previous_response_id` cannot be combined with `conversation`
- when using `previous_response_id`, earlier input tokens are still billed as input tokens
- response objects are saved for 30 days by default
- `store: false` disables that default response-object retention
- conversation objects and conversation items are not subject to the 30-day TTL
- any response attached to a conversation has its items persisted as conversation items with no 30-day TTL

Responses can store a response object, participate in a continuation chain, or bind to a persistent conversation resource.

## Prompt caching

Responses uses the same core prompt-cache controls as Chat:

- `prompt_cache_key`
- `prompt_cache_retention`

The captured create reference documents `prompt_cache_retention` values as:

- `"in-memory"`
- `"24h"`

The separately captured prompt-caching guide uses `in_memory` and `24h`. As with Chat, that is an official-doc inconsistency in the captured sources.

Useful guide-level details for implementers and operators:

- in-memory retention is generally 5 to 10 minutes of inactivity, up to 1 hour
- extended retention keeps cached prefixes active up to 24 hours
- repeated requests for the same prefix and cache key can overflow to additional machines at higher rates, reducing hit effectiveness
- prompt caches are scoped to an organization

Measure cache behavior via `usage.input_tokens_details.cached_tokens`.

Online recheck on 2026-05-16:

- The current prompt-caching guide still treats caching as automatic and exposes `prompt_cache_key` / `prompt_cache_retention` as provider-native optimization controls on OpenAI request surfaces.
- The guide still says caching is enabled for recent OpenAI models and uses a 1024-token cacheability threshold with 128-token hit increments.
- The guide still uses `prompt_cache_retention: "in_memory"` and `"24h"` while the captured create reference used `"in-memory"`; the proxy should preserve the caller-provided spelling and avoid translating this into a cross-provider TTL.
- No official OpenAI-domain page found during this recheck changed the Responses prompt-cache wire shape captured on 2026-04-16.

## Stored resources

Responses has several distinct persistence layers. Keeping them separate avoids design mistakes.

### Stored response objects

Controlled by `store` on the response request. These are retrievable and deletable through `/v1/responses/{response_id}` and may be cancellable while in progress.

### Conversations and items

Managed through `/v1/conversations` and `/v1/conversations/{conversation_id}/items`. These outlive the default 30-day response-object TTL and are the formal stateful conversation surface.

### Referenced external resources

Some Responses tools point at separately managed resources rather than embedding all state inline, for example:

- `vector_store_ids` for file search
- file identifiers or URLs for input files and images
- remote MCP server descriptors and auth material

These resources are referenced by the Responses surface rather than embedded into response or conversation storage.

## See also

See also [OpenAI Chat Completions API baseline](openai-chat-completions.md) and the shared [OpenAI capture manifest](snapshots/2026-04-16/openai-manifest.md), which also inventories the separately documented legacy `/v1/completions` surface.
