# Google Gemini API (generateContent) — protocol baseline

- **Source:** [Gemini API — Text generation](https://ai.google.dev/gemini-api/docs/text-generation), [Gemini API docs](https://ai.google.dev/gemini-api/docs), [REST API reference](https://ai.google.dev/api/rest/v1beta/models/generateContent)  
- **Capture date / version:** 2026-03-05 (documentation as of that date; check source for latest)

---

## Endpoint

- **POST** `https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent`  
- Path can also be expressed as base `https://generativelanguage.googleapis.com/v1beta` + path `/models/{model}:generateContent` (model in path).  
- Headers: `x-goog-api-key` (or OAuth), `Content-Type: application/json`

## Request

- **contents** (array, required): Conversation content. Each item has **role** (e.g. `"user"` | `"model"`) and **parts** (array of parts).
- **parts** (within each content item): e.g. `{ "text": "..." }`, **inline_data** (e.g. `mime_type`, `data` for images), **functionCall** / **functionResponse** for tools.
- **systemInstruction** (optional): Content (e.g. parts with text) for system instruction.
- **generationConfig** (optional): **temperature**, **topP**, **topK**, **maxOutputTokens**, **stopSequences**, **stream** (for streaming), etc.
- **tools** (optional): Function declarations for tool/function calling.
- **safetySettings** (optional): Safety thresholds.
- **cachedContent** (optional): Reference to cached context.

Example minimal request:

```json
{
  "contents": [
    {
      "parts": [{ "text": "Your prompt here" }]
    }
  ]
}
```

## Response (non-streaming)

- **candidates** (array): Each candidate:
  - **content:** **parts** (array), **role** (e.g. `"model"`)
  - **finishReason:** `"STOP"` | `"MAX_TOKENS"` | `"SAFETY"` | `"RECITATION"` | `"OTHER"`
- **usageMetadata** (optional): **promptTokenCount**, **candidatesTokenCount**, **totalTokenCount** (and optionally **thoughtsTokenCount** for thinking models).
- **modelVersion** (optional): string.
- **responseId** (optional): string.

Parts in **content.parts** can be: **text**, **thought** (reasoning, with **text**), **functionCall** (**name**, **id**, **args**).

## Response (streaming)

- **Content-Type:** `text/event-stream` (when requesting stream via **generationConfig.stream** or equivalent).
- Each chunk: SSE **data:** line with JSON object; same top-level shape as non-streaming (e.g. **candidates** with **content.parts**), possibly partial. Final chunk may include **finishReason**, **usageMetadata**.

## Notes for proxy

- Detect client format by body: **contents** (array) present.
- Passthrough when upstream is Gemini.
- Translation: **contents** ↔ **messages**; **content.parts** (text / thought / functionCall) ↔ **choices[].message** (content, reasoning_content, tool_calls); **usageMetadata** ↔ **usage**.
