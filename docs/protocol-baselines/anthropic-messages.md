# Anthropic Messages API — protocol baseline

- Source:
  - https://platform.claude.com/docs/en/api/messages
- Capture date: 2026-04-07
- Local snapshot:
  - `docs/protocol-baselines/snapshots/2026-04-07/anthropic-messages.html`

## Canonical surface

- Primary endpoint: `POST /v1/messages`
- Closely related current endpoints:
  - `POST /v1/messages/count_tokens`
  - models endpoints under the same API family
- Base URL example: `https://api.anthropic.com`

## Required headers

- `x-api-key`
- `anthropic-version`
- `content-type: application/json`

## Request shape

- Core fields:
  - `model`
  - `max_tokens`
  - `messages`
  - `system`
  - `stream`
  - `tools`
  - `tool_choice`
  - `metadata`
- `messages[].role` is primarily `user` or `assistant`
- `messages[].content` may be:
  - shorthand string
  - an array of typed content blocks
- Common block types in the current reference:
  - `text`
  - `image`
  - `tool_use`
  - `tool_result`
  - thinking-related blocks on supported models

## Non-streaming response shape

- Top-level `type` is `message`
- Common fields:
  - `id`
  - `type`
  - `role`
  - `model`
  - `content`
  - `stop_reason`
  - `stop_sequence`
  - `usage`

## Streaming shape

- Transport: SSE
- Core events documented by Anthropic:
  - `message_start`
  - `content_block_start`
  - `content_block_delta`
  - `content_block_stop`
  - `message_delta`
  - `message_stop`
- Tool use and thinking both appear as typed block events rather than OpenAI-style `delta.tool_calls`.

## Proxy implications

- For strict conformance, `max_tokens` and `anthropic-version` should always be present for native Messages requests.
- Mapping `parallel_tool_calls: false` from OpenAI-style clients is only an approximation; Anthropic exposes this through `disable_parallel_tool_use` inside `tool_choice`.
