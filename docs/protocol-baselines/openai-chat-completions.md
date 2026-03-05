# OpenAI Chat Completions API — protocol baseline

- **Source:** [OpenAI API Reference — Chat](https://platform.openai.com/docs/api-reference/chat/create)  
- **Streaming:** [Streaming responses](https://platform.openai.com/docs/guides/streaming-responses)  
- **Capture date / version:** 2026-03-05 (documentation as of that date; check source for latest)

---

## Endpoint

- **POST** `/chat/completions`  
- Base URL example: `https://api.openai.com/v1`

## Request (create chat completion)

Key body parameters:

- **model** (string): Model ID (e.g. `gpt-4o`).
- **messages** (array): List of messages. Each message: `role` (`"system"` | `"user"` | `"assistant"`), `content` (string or array of content parts).
- **stream** (boolean, optional): If `true`, response is SSE stream. Default behavior is non-streaming.
- **max_tokens**, **temperature**, **top_p**, **n**, **tools**, **tool_choice**, **response_format**, **logprobs**, etc. (see official reference).

Content parts for messages can include `type: "text"` with `text`, or image parts, etc.

## Response (non-streaming)

- **object:** `"chat.completion"`
- **id:** string (completion ID)
- **created:** number (Unix timestamp)
- **model:** string
- **choices:** array of:
  - **index:** number
  - **message:** object with **role** (`"assistant"`), **content** (string or array of parts), optional **tool_calls** (array of `{ id, type: "function", function: { name, arguments } }`), optional **refusal**
  - **finish_reason:** `"stop"` | `"length"` | `"tool_calls"` | `"content_filter"` | `"function_call"`
  - **logprobs:** optional
- **usage** (optional): `prompt_tokens`, `completion_tokens`, `total_tokens`

## Response (streaming, SSE)

- **Content-Type:** `text/event-stream`
- Each event: `data: <JSON>\n\n`
- Chunk object: **object** `"chat.completion.chunk"`, **choices** array with **delta** (e.g. `role`, `content`, or `tool_calls`), **finish_reason** (null until end).
- Stream end: `data: [DONE]\n\n`

## Notes for proxy

- Detect client format by path `/v1/chat/completions` and body (e.g. `messages` without `input`).
- Passthrough: forward request/response as-is when upstream is Chat Completions.
- Translation: pivot via OpenAI Chat Completions when converting to/from other formats.
