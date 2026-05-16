# OpenAI Chat Completions API baseline

## Metadata

- `captured_at_utc`: `2026-04-17T06:59:44Z`
- `snapshot_bucket`: `2026-04-16`
- `snapshot_bucket_note`: Snapshot artifacts are stored under the `2026-04-16` bucket because this capture completed at `2026-04-16T23:59:44-07:00` in `America/Los_Angeles`.
- `online_recheck_at_utc`: `2026-05-16T00:00:00Z`
- `source_urls`:
  - `https://developers.openai.com/api/reference/resources/chat/index.md`
  - `https://developers.openai.com/api/docs/guides/prompt-caching`
  - `https://developers.openai.com/api/docs/guides/reasoning`
  - `https://developers.openai.com/api/docs/guides/streaming-responses`
  - `https://developers.openai.com/api/docs/guides/migrate-to-responses`
- `snapshot_manifest`: `docs/protocol-baselines/snapshots/2026-04-16/openai-manifest.md`
- `scope`:
  - Official HTTP wire surface for Chat Completions, including the stored-completion lifecycle.
  - Request and response shapes, streaming behavior, tool calling, audio, reasoning, prompt caching, and documented web-search controls.
- `non_goals`:
  - SDK helper APIs or language-specific ergonomics.
  - Pricing, rate limits, and model quality comparisons.
  - Realtime or WebSocket protocol design.

The captured Chat Completions reference is still current, but its own header says new projects should try Responses first. See also: [OpenAI Responses API baseline](openai-responses.md).

## Formal surface

The captured official surface includes more than the create endpoint:

- `POST /v1/chat/completions`
- `GET /v1/chat/completions`
- `GET /v1/chat/completions/{completion_id}`
- `POST /v1/chat/completions/{completion_id}`
- `DELETE /v1/chat/completions/{completion_id}`
- `GET /v1/chat/completions/{completion_id}/messages`

Stored-completion lifecycle endpoints are part of the same documented resource family.

## Request baseline

### Core envelope

The create request centers on:

- `model`
- `messages`
- `store`
- `stream`
- `metadata`
- standard sampling and output controls such as `temperature`, `top_p`, `n`, `stop`, `presence_penalty`, `frequency_penalty`, and `seed`

The captured reference also includes newer documented controls such as:

- `verbosity`
- `prediction`
- `service_tier`
- `parallel_tool_calls`
- `web_search_options`

### Message model

`messages` is an ordered conversation array. The captured role set includes:

- `developer`
- `system`
- `user`
- `assistant`
- `tool`
- `function`

Important shape rules from the captured reference:

- `developer` and `system` messages are text-only on this surface.
- `user` messages may be a plain string or an array of typed content parts.
- `assistant` messages may omit `content` when they carry `tool_calls` or legacy `function_call`.
- `tool` messages are the structured way to send tool results back to the model.
- `function` messages and `function_call` are legacy-compatible fields; `tool_calls` is the current field family.

### Multimodal input and output

The captured Chat reference documents typed content parts for input:

- `text`
- `image_url`
- `input_audio`
- `file`

Chat Completions also supports audio output:

- request `modalities: ["audio"]` or `["text", "audio"]`
- provide the `audio` output configuration
- assistant responses may then carry an `audio` object in addition to text content

### Tool calling

The captured tool surface includes:

- function tools
- custom tools
- `tools`
- `tool_choice`
- `parallel_tool_calls`

`tool_choice` is not just `auto` or `none`; the reference also documents forcing a specific tool. Tool names, tool IDs, and incremental argument fragments are part of the documented Chat wire surface.

### Built-in web search control

The saved Chat reference also documents a request-level `web_search_options` object for supported models.

`web_search_options` currently exposes:

- `search_context_size` with `low`, `medium`, and `high`
- `user_location.type`
- `user_location.approximate.city`
- `user_location.approximate.country`
- `user_location.approximate.region`
- `user_location.approximate.timezone`

When web search is used, assistant messages may include `annotations[]` entries of type `url_citation` with `start_index`, `end_index`, `title`, and `url`.

### Structured outputs

Chat Completions uses `response_format` for output shaping. The captured reference documents plain text, JSON object, and JSON schema-oriented modes.

## Response baseline

### Non-streaming shape

The non-streaming response object is `chat.completion` with the usual top-level fields:

- `id`
- `object`
- `created`
- `model`
- `choices`
- `usage`
- `system_fingerprint`

Each `choices[]` entry contains:

- `index`
- `message`
- `finish_reason`
- optional `logprobs`

The captured reference documents these `finish_reason` values:

- `stop`
- `length`
- `content_filter`
- `tool_calls`
- legacy `function_call`

### Assistant message shape

`choices[].message` may contain:

- `content`
- `tool_calls`
- `refusal`
- `audio`
- legacy `function_call`

Clients should not assume text-only output. Assistant messages may carry tools, refusals, audio, or legacy `function_call` fields.

### Usage accounting

The captured response shape exposes cache and reasoning accounting directly:

- `usage.prompt_tokens_details.cached_tokens`
- `usage.completion_tokens_details.reasoning_tokens`
- audio token counts
- prediction token details

Those fields are important protocol data, not optional decorations. They are how callers observe prompt-cache hits and reasoning-token spend on this surface.

## Streaming baseline

Chat Completions streaming remains the classic SSE shape:

- transport is SSE
- each payload chunk is a JSON object whose `object` is `chat.completion.chunk`
- the stream is data-only in the captured reference, not a family of named `event:` lines
- the terminal sentinel is still `data: [DONE]`

The client must reconstruct the assistant message by concatenating `choices[].delta` updates in order. In practice that means:

- an early chunk may establish the assistant role
- later chunks append `content`
- tool or function arguments may arrive as partial deltas
- the final semantic stop reason may arrive only near the end

The captured reference also documents `stream_options`:

- `include_obfuscation`
- `include_usage`

Important implementation details:

- obfuscation fields are documented as enabled by default and may appear on delta events
- with `include_usage: true`, one extra chunk is emitted before `data: [DONE]`
- that usage chunk always has empty `choices`
- all earlier chunks may carry `usage: null`
- if the stream is interrupted, the final usage chunk may never arrive

## Reasoning

Chat Completions exposes reasoning mostly as request policy and usage accounting, not as a first-class output item family.

The captured request field is `reasoning_effort`. Current documented values are:

- `none`
- `minimal`
- `low`
- `medium`
- `high`
- `xhigh`

The captured reference also includes model-specific notes:

- `gpt-5.1` defaults to `none` and supports `none`, `low`, `medium`, and `high`
- models before `gpt-5.1` default to `medium` and do not support `none`
- `gpt-5-pro` defaults to and only supports `high`
- `xhigh` is supported for models after `gpt-5.1-codex-max`

Two protocol consequences matter for implementations:

- `max_completion_tokens` is the authoritative output cap and includes visible output plus reasoning tokens
- reasoning spend is surfaced via `usage.completion_tokens_details.reasoning_tokens`

Unlike Responses, the captured Chat reference does not define a standalone `reasoning` output item or encrypted reasoning carry-forward field on the Chat wire format.

## Prompt caching

Both Chat Completions and Responses now expose explicit prompt-cache controls. On Chat, the relevant fields are:

- `prompt_cache_key`
- `prompt_cache_retention`

The captured Chat reference says `prompt_cache_key` replaces `user` for caching optimization. It also documents `prompt_cache_retention` values as:

- `"in-memory"`
- `"24h"`

The separately captured prompt-caching guide uses `in_memory` and `24h`. That is an official-doc inconsistency as of 2026-04-16.

Other captured prompt-caching details:

- in-memory retention is generally 5 to 10 minutes of inactivity, up to 1 hour
- extended retention can keep cached prefixes active up to 24 hours
- prompt caches are organization-scoped, not shared across orgs
- cache hits surface through `usage.prompt_tokens_details.cached_tokens`

Online recheck on 2026-05-16:

- The current prompt-caching guide still describes automatic caching for cacheable prompts and still exposes `prompt_cache_key` / `prompt_cache_retention` as the explicit controls for improving routing and retention.
- The guide still says caching is enabled for recent OpenAI models and uses a 1024-token cacheability threshold with 128-token hit increments.
- The guide continues to document `prompt_cache_retention: "in_memory"` and `"24h"` in guide examples, while the earlier captured API reference used `"in-memory"`; the proxy should keep treating the value as provider-native rather than normalizing or validating it.
- No official OpenAI-domain page found during this recheck changed the Chat prompt-cache wire shape captured on 2026-04-16.

## Conversation state and stored resources

Chat Completions has no first-class continuation resources such as:

- no `previous_response_id`
- no `conversation`
- no compaction endpoint

Multi-turn state is caller-managed by replaying `messages`.

`store: true` adds persistence, but only for the completion artifact itself. That enables:

- list stored completions
- retrieve a stored completion
- delete a stored completion
- update stored completion metadata
- retrieve stored messages for that completion

Important limitation: the captured `POST /chat/completions/{completion_id}` endpoint only supports updating `metadata`. Stored Chat Completions are retrievable artifacts, not server-managed conversation threads.

## See also

See also [OpenAI Responses API baseline](openai-responses.md) and the shared [OpenAI capture manifest](snapshots/2026-04-16/openai-manifest.md), which also inventories the separately documented legacy `/v1/completions` surface.
