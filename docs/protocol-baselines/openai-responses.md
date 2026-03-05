# OpenAI Responses API — protocol baseline

- **Source:** [OpenAI API Reference — Responses](https://platform.openai.com/docs/api-reference/responses), [Responses streaming](https://platform.openai.com/docs/api-reference/responses-streaming)  
- **Streaming guide:** [Streaming API responses](https://platform.openai.com/docs/guides/streaming-responses)  
- **Capture date / version:** 2026-03-05 (documentation as of that date; check source for latest)

---

## Endpoints

- **POST** `/responses` — Create a model response (main endpoint for chat-like usage)
- **GET** `/responses/{response_id}` — Get a model response
- **DELETE** `/responses/{response_id}`
- **POST** `/responses/{response_id}/cancel`
- **POST** `/responses/compact`

Base URL example: `https://api.openai.com/v1`

## Request (create response)

- **model** (optional): Model ID.
- **input** (array or string): Conversation input. Array of items (e.g. messages with `type`, `role`, `content`); or string for single text input.
- **instructions** (optional): System-level instructions.
- **stream** (boolean, optional): If `true`, response is SSE stream.
- **truncation**, **tools**, **tool_choice**, **safety_identifier**, etc. (see official reference).

Input items can be messages (`type: "message"`, `role`, `content` with e.g. `type: "input_text"`, `text`) or other item types.

## Response (non-streaming)

- **object:** `"response"`
- **id:** string
- **created_at:** number (Unix timestamp)
- **status:** e.g. `"completed"` | `"in_progress"`
- **output:** array of output items, e.g.:
  - **type:** `"message"` — **content** array with e.g. `type: "output_text"`, **text**
  - **type:** `"function_call"` — **call_id**, **name**, **arguments**
- **usage** (optional): token counts

## Response (streaming, SSE)

- **Content-Type:** `text/event-stream`
- Semantic event types (examples): `response.created`, `response.in_progress`, `response.output_text.delta`, `response.output_item.added`, `response.function_call_arguments.delta`, `response.completed`, `error`.
- Events may include **event:** line (e.g. `event: response.created`) and **data:** line with JSON.
- Key lifecycle: `response.created` → deltas (e.g. `response.output_text.delta` with **delta** text) → `response.completed`.

## Notes for proxy

- Detect client format by path `/v1/responses` or body with **input** (array or string) and no **messages**.
- Passthrough when upstream is Responses API.
- Translation: convert to/from OpenAI Chat Completions pivot; map **input** ↔ **messages**, **output** ↔ **choices[].message**.
