# OpenAI Responses API — protocol baseline

- Source:
  - https://platform.openai.com/docs/api-reference/responses
  - https://developers.openai.com/api/reference/resources/responses
- Capture date: 2026-04-07
- Local snapshot:
  - `docs/protocol-baselines/snapshots/2026-04-07/openai-responses.html`

## Canonical surface

- Primary create endpoint: `POST /v1/responses`
- Additional lifecycle endpoints documented by OpenAI:
  - `GET /v1/responses/{response_id}`
  - `DELETE /v1/responses/{response_id}`
  - `POST /v1/responses/{response_id}/cancel`
  - `POST /v1/responses/compact`
  - input-item helper endpoints under the same resource family
- Base URL example: `https://api.openai.com/v1`

## Request shape

- Core fields:
  - `model`
  - `input`
  - `instructions`
  - `stream`
  - `tools`
  - `tool_choice`
  - `metadata`
- `input` accepts either:
  - a string
  - an array of structured input items
- Structured items include at least:
  - `type: "message"` with `role` and `content`
  - tool-related items such as function-call output
  - reasoning-related items in newer response shapes

## Non-streaming response shape

- Top-level object type is `response`
- Common fields:
  - `id`
  - `object`
  - `created_at`
  - `status`
  - `output`
  - `usage`
- `output` is a typed array, commonly containing:
  - `message`
  - `function_call`
  - `reasoning`

## Streaming shape

- Transport: SSE
- Common event types documented in the current reference include:
  - `response.created`
  - `response.in_progress`
  - `response.output_text.delta`
  - `response.output_item.added`
  - `response.function_call_arguments.delta`
  - terminal events such as `response.completed`, `response.incomplete`, `response.failed`
- OpenAI also documents sequence-aware replay/continuation fields on retrieval streaming paths.

## Proxy implications

- Detect Responses clients by explicit path or by `input` without `messages`.
- Treat `POST /v1/responses` as the compatibility minimum.
- Full spec conformance requires more than the create endpoint; the lifecycle sub-resources are part of the current official surface.
