# Google Gemini GenerateContent API — protocol baseline

- Source:
  - https://ai.google.dev/api/generate-content
- Capture date: 2026-04-07
- Local snapshot:
  - `docs/protocol-baselines/snapshots/2026-04-07/google-gemini-generate-content.html`

## Canonical surface

- Primary endpoint:
  - `POST https://generativelanguage.googleapis.com/v1beta/{model=models/*}:generateContent`
- Streaming companion endpoint:
  - `...:streamGenerateContent`
- Auth:
  - `x-goog-api-key`
  - or OAuth

## Request shape

- Core fields:
  - `contents[]`
  - `tools[]`
  - `toolConfig`
  - `safetySettings[]`
  - `systemInstruction`
  - `generationConfig`
  - `cachedContent`
- `contents[].parts[]` can include:
  - `text`
  - inline binary/media parts
  - `functionCall`
  - `functionResponse`

## Non-streaming response shape

- Top-level fields commonly include:
  - `candidates[]`
  - `usageMetadata`
  - `modelVersion`
  - `responseId`
- `candidates[].content.parts[]` may include:
  - text parts
  - thought/reasoning-like parts on supported models
  - `functionCall`

## Streaming shape

- Streaming uses the `streamGenerateContent` resource family and SSE transport.
- Compatibility clients also commonly consume `alt=sse`.

## Proxy implications

- Gemini is not just `contents`; newer official fields such as `toolConfig`, `cachedContent`, and `systemInstruction` are part of the practical contract.
- Function-calling compatibility requires careful handling of both `functionCall` and `functionResponse`, especially because Gemini expects a response object shape that is not identical to OpenAI tool messages.
