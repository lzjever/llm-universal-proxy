# Anthropic Messages API (Claude) — protocol baseline

- **Source:** [Anthropic API — Messages](https://docs.anthropic.com/en/api/messages)  
- **Capture date / version:** 2026-03-05 (documentation as of that date; check source for latest)

---

## Endpoint

- **POST** `/v1/messages`  
- Base URL: `https://api.anthropic.com`  
- Headers: `x-api-key`, `anthropic-version` (e.g. `2023-06-01`), `content-type: application/json`

## Request

- **model** (string): Claude model ID (e.g. `claude-3-5-sonnet-20241022`).
- **max_tokens** (number, required): Maximum tokens to generate.
- **messages** (array, required): Input messages. Each message:
  - **role:** `"user"` | `"assistant"`
  - **content:** string (shorthand for one text block) or array of content blocks.
- **system** (optional): Top-level system prompt (not a message role).
- **stream** (optional): If `true`, response is SSE stream.

Content blocks (input): e.g. `{ "type": "text", "text": "..." }`, `{ "type": "image", "source": { "type": "base64", "media_type": "...", "data": "..." } }`, `type: "tool_use"` / `tool_result`.

## Response (non-streaming)

- **id:** string (message ID)
- **type:** `"message"`
- **role:** `"assistant"`
- **content:** array of content blocks, e.g.:
  - **type:** `"text"` — **text:** string
  - **type:** `"thinking"` — **thinking:** string (extended thinking)
  - **type:** `"tool_use"` — **id**, **name**, **input**
- **model:** string
- **stop_reason:** `"end_turn"` | `"max_tokens"` | `"tool_use"` | null | ...
- **stop_sequence:** optional
- **usage:** **input_tokens**, **output_tokens**

## Response (streaming, SSE)

- **Content-Type:** `text/event-stream`
- Events (with **event:** and **data:** lines):
  - **message_start** — message object with empty or initial content
  - **content_block_start** — **index**, **content_block** (e.g. `type: "text"`, `text`)
  - **content_block_delta** — **index**, **delta** (e.g. `type: "text_delta"`, **text**)
  - **content_block_stop** — **index**
  - **message_delta** — **delta** (e.g. **stop_reason**), **usage**
  - **message_stop**

## Notes for proxy

- Detect client format by body: **messages** plus **system** or **anthropic_version**, or first message **content** as array with Claude-specific block types (e.g. image with `source.type: "base64"`, tool_use / tool_result).
- Passthrough when upstream is Anthropic.
- Translation: map **messages** + **system** ↔ OpenAI **messages**; **content** blocks (text / thinking / tool_use) ↔ **choices[].message** (content, reasoning_content, tool_calls).
