# OpenAI Chat Completions API — protocol baseline

- Source:
  - https://platform.openai.com/docs/api-reference/chat/create
  - https://developers.openai.com/api/reference/resources/chat
- Capture date: 2026-04-07
- Local snapshot:
  - `docs/protocol-baselines/snapshots/2026-04-07/openai-chat-completions.html`

## Canonical surface

- Primary endpoint: `POST /v1/chat/completions`
- Base URL example: `https://api.openai.com/v1`

## Request shape

- Core fields:
  - `model`
  - `messages`
  - `stream`
  - `max_tokens`
  - `temperature`
  - `top_p`
  - `tools`
  - `tool_choice`
  - `response_format`
  - `logprobs`
- `messages` is an ordered array of role/content items.
- Content may be:
  - a plain string
  - an array of typed parts such as text and image input

## Non-streaming response shape

- Top-level object type: `chat.completion`
- Common fields:
  - `id`
  - `object`
  - `created`
  - `model`
  - `choices`
  - `usage`
- `choices[].message` may carry:
  - `content`
  - `tool_calls`
  - refusal-related fields on supported models

## Streaming shape

- Transport: SSE
- Each chunk is a `chat.completion.chunk`
- Chunks carry `choices[].delta`
- End-of-stream sentinel remains `data: [DONE]`

## Proxy implications

- This is still the best pivot format for broad OpenAI-compatible ecosystems such as vLLM and Xinference.
- Robust compatibility requires tolerating:
  - SSE `\n\n` and `\r\n\r\n` separators
  - partial tool-call argument deltas
  - gzip-compressed upstream responses
