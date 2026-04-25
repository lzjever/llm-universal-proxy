# Google Gemini GenerateContent / streamGenerateContent - protocol baseline

## Metadata

- `captured_at_utc`: `2026-04-17T07:00:00Z`
- `snapshot_bucket`: `2026-04-16`
- `snapshot_bucket_note`: This capture completed at `2026-04-17T00:00:00-07:00` in `America/Los_Angeles` and remained in the `2026-04-16` bucket because the collection workflow grouped it with the rest of that day's snapshot batch.
- `source_urls`:
  - `https://ai.google.dev/api`
  - `https://ai.google.dev/api/generate-content`
  - `https://ai.google.dev/gemini-api/docs/text-generation`
  - `https://ai.google.dev/gemini-api/docs/function-calling`
  - `https://ai.google.dev/gemini-api/docs/thinking`
  - `https://ai.google.dev/gemini-api/docs/thought-signatures`
  - `https://ai.google.dev/api/caching`
  - `https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta`
- `snapshot_manifest`: `docs/protocol-baselines/snapshots/2026-04-16/google-manifest.json`
- `scope`:
  - Raw REST wire contract for `generateContent` and `streamGenerateContent`, plus `Content` / `Part`, function calling, `toolConfig`, thinking, safety, `generationConfig`, `usageMetadata`, finish reasons, cached contents, and JSON naming normalization.
- `non_goals`:
  - Live API, batch APIs, embeddings, file upload protocol, pricing and quotas, model inventory, and SDK-only ergonomics unless they change the wire contract.

## Normative reading order

1. Treat the v1beta discovery document and the REST reference as the field-level source of truth for request / response names, enum values, and union members.
2. Treat the guide pages as the source of truth for behavior that schemas do not fully express: SSE streaming, function-response turn construction, thought signatures, tool mixing, and cache lifecycle.
3. Where official docs conflict, this baseline prefers the newer or more concrete source.

| Official inconsistency | Baseline decision |
| --- | --- |
| `model` exists both as a path parameter and as a property in `GenerateContentRequest` within discovery JSON. | Treat the URL path parameter as the canonical REST contract. Official REST examples place the model in the URL, not the JSON body. |
| `GenerateContentRequest.tools` short description still says supported tools are only `Function` and `codeExecution`. | Treat the concrete `Tool` schema and the function-calling guide as authoritative. Current discovery exposes additional built-in and server-side tools. |
| `Tool.functionDeclarations` description still mentions a next-turn `Content.role` of `"function"`. | Treat the current `Content` schema and official examples as authoritative: the client sends `functionResponse` inside a `user` content block. |
| Official examples mix `camelCase` and `snake_case` JSON field spellings. | This baseline treats `camelCase` as canonical for raw REST because discovery JSON and most raw REST examples use it, while `snake_case` appears in SDK-oriented or older examples. |

## Canonical surface

- Base URL: `https://generativelanguage.googleapis.com/`
- Non-streaming endpoint: `POST /v1beta/{model=models/*}:generateContent`
- Streaming endpoint: `POST /v1beta/{model=models/*}:streamGenerateContent`
- Canonical model format: `models/{model}`
- Discovery revision captured: `20260415`
- Current public docs emphasize API-key auth via `x-goog-api-key`.

Canonical REST shape:

```http
POST https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent
x-goog-api-key: $GEMINI_API_KEY
Content-Type: application/json
```

Canonical request body skeleton:

```json
{
  "contents": [
    {
      "role": "user",
      "parts": [
        {
          "text": "Hello"
        }
      ]
    }
  ],
  "systemInstruction": {
    "parts": [
      {
        "text": "You are helpful."
      }
    ]
  },
  "tools": [],
  "toolConfig": {},
  "safetySettings": [],
  "generationConfig": {},
  "cachedContent": "cachedContents/abc123",
  "serviceTier": "standard",
  "store": false
}
```

## `generateContent` vs `streamGenerateContent`

Both methods consume the same `GenerateContentRequest` schema and both conceptually yield `GenerateContentResponse`.

| Method | Transport | Response model | Practical use |
| --- | --- | --- | --- |
| `generateContent` | Standard HTTP response | One complete `GenerateContentResponse` | Batch or non-interactive calls where waiting for the full answer is acceptable. |
| `streamGenerateContent` | SSE | Incremental `GenerateContentResponse` chunks | Chat, low-latency UX, and tool / agent flows that benefit from partial output. |

Streaming details from the official docs:

- Google documents `streamGenerateContent` as SSE.
- REST examples append `?alt=sse`.
- REST examples use `curl --no-buffer`.
- SDK examples iterate `GenerateContentResponse` chunks and expose helpers such as `chunk.text`.

Inference from the official docs: because the guides say streaming returns `GenerateContentResponse` instances incrementally, and the REST examples use SSE rather than a separate event taxonomy, a raw SSE consumer should treat each `data:` frame as a partial `GenerateContentResponse` and aggregate by `candidate.index`, content order, and part order. The official pages do not document OpenAI-style named event types for this endpoint.

Operational implications:

- Do not assume every stream chunk carries final `usageMetadata`, `finishReason`, or a fully assembled candidate.
- Preserve chunk order.
- Aggregate per candidate index instead of concatenating blindly across candidates.
- If you expose chat history after a stream finishes, note that Google's SDK docs say history is updated after stream consumption using the aggregated final response.

## Top-level request contract

`GenerateContentRequest` currently exposes these top-level fields:

| Field | Type | Notes |
| --- | --- | --- |
| `contents` | `Content[]` | Required. Full conversation history plus the latest user turn. Single-turn requests still use an array. |
| `tools` | `Tool[]` | Optional. Custom and built-in tool declarations. |
| `toolConfig` | `ToolConfig` | Optional. Global tool behavior, including function-calling mode. |
| `safetySettings` | `SafetySetting[]` | Optional. One unique entry per category; overrides defaults for the listed categories. |
| `systemInstruction` | `Content` | Optional. Developer instruction. Current docs say text only. |
| `generationConfig` | `GenerationConfig` | Optional. Sampling, structure, thinking, and modality controls. |
| `cachedContent` | `string` | Optional. Cache reference. Format: `cachedContents/{cachedContent}`. |
| `serviceTier` | `enum` | Optional. Current discovery values: `unspecified`, `standard`, `flex`, `priority`. |
| `store` | `boolean` | Optional. Request-level logging behavior override. |

Notes:

- Treat the URL `model` path parameter as canonical for raw REST even though discovery also includes `model` in the request schema.
- `systemInstruction` is separate from `contents`; do not synthesize it into a fake first user message.
- `cachedContent` is a resource reference only. Cache creation and mutation use the separate `cachedContents` resource family.

## `Content` and `Part`

### `Content`

Current schema:

```json
{
  "role": "user|model",
  "parts": [ { "...Part..." } ]
}
```

Key rules:

- `parts` is ordered and must be preserved exactly.
- `role` is optional, but multi-turn requests should set it.
- Current `Content` schema only documents `user` and `model`.
- Official function-calling examples send `functionResponse` from the client inside `role: "user"` content blocks.

### `Part`

`Part` is a tagged union. A single part may contain only one data branch.

| Part branch | Direction | Notes |
| --- | --- | --- |
| `text` | input or output | Inline text. |
| `inlineData` | input or output | Raw bytes plus `mimeType`. Use for images, audio, PDFs, and other supported media. |
| `fileData` | input | URI-based file reference with `fileUri` and optional `mimeType`. |
| `functionCall` | output | Predicted client-side function call with `name`, `args`, and optional `id`. |
| `functionResponse` | input | Client-supplied result for a previous `functionCall`. |
| `toolCall` | output | Predicted server-side built-in tool invocation. |
| `toolResponse` | input | Client echo of a previously returned `toolCall` plus the tool result. |
| `executableCode` | output | Code generated for the `codeExecution` tool. |
| `codeExecutionResult` | output | Result returned by server-side code execution. |
| `thought` | output metadata | Boolean marker that this part is model thought. |
| `thoughtSignature` | Gemini output or trusted replay input | Opaque Gemini bytes that must sometimes be returned verbatim in later turns. Do not fabricate them. |
| `partMetadata` | both | Opaque metadata; docs mention file/source identity and multiplexing use cases. Preserve it. |
| `mediaResolution` | input | Input media resolution hint. |
| `videoMetadata` | input | Only valid when the part carries video data. |

Media sub-objects:

- `inlineData`: `{ "mimeType": "...", "data": "<base64>" }`
- `fileData`: `{ "fileUri": "...", "mimeType": "..." }`

Proxy rules:

- Preserve part order exactly.
- On Gemini-native traffic and same-provider passthrough, preserve unknown or opaque fields such as `partMetadata` and real Gemini `thoughtSignature` values.
- Cross-protocol request translators must not synthesize or replay Gemini `thoughtSignature` / `thought_signature` values. If those fields appear anywhere in translated request content or history, fail closed instead of guessing provenance or part placement.
- Do not coerce a union branch into another branch.
- Do not flatten multimodal messages into plain text.

## Function calling and tools

### `Tool`

Current discovery schema exposes more than just plain function declarations:

| Tool field | Purpose |
| --- | --- |
| `functionDeclarations` | Client-executed custom functions. |
| `codeExecution` | Server-side code execution. |
| `googleSearch` | Built-in Google Search tool. |
| `googleSearchRetrieval` | Search-powered retrieval variant still present in discovery. |
| `urlContext` | URL context retrieval. |
| `googleMaps` | Geospatial grounding. |
| `fileSearch` | Retrieval from File Search stores. |
| `mcpServers` | Model Context Protocol servers. |
| `computerUse` | Browser-style computer-use tool. |

Important: the short field description on `GenerateContentRequest.tools` is stale and narrower than the actual `Tool` schema.

### `FunctionDeclaration`

`FunctionDeclaration` is the custom-function schema used inside `Tool.functionDeclarations[]`.

| Field | Notes |
| --- | --- |
| `name` | Required. Allowed characters are broader here than on `FunctionCall`: letters, digits, underscores, colons, dots, and dashes, max length 128. |
| `description` | Required. |
| `parameters` | Optional OpenAPI 3.0.3 subset schema. |
| `parametersJsonSchema` | Optional JSON Schema alternative. Mutually exclusive with `parameters`. |
| `response` | Optional OpenAPI-style output schema. |
| `responseJsonSchema` | Optional JSON Schema output alternative. |
| `behavior` | Present in schema but currently documented as Bidi-only, not part of normal `generateContent` behavior. |

The function-calling guide also warns that `ANY` mode may reject very large or deeply nested schemas.

### `ToolConfig`

`ToolConfig` is request-scoped and currently contains:

| Field | Notes |
| --- | --- |
| `functionCallingConfig` | Mode selection plus optional allowlist. |
| `retrievalConfig` | User language / location hints for retrieval-capable tools. |
| `includeServerSideToolInvocations` | If `true`, include server-side `toolCall` and `toolResponse` parts in the returned `Content`. |

`retrievalConfig` currently includes:

- `languageCode`
- `latLng`

### `FunctionCallingConfig`

| Mode | Meaning |
| --- | --- |
| `AUTO` | Model chooses between natural language and a function call. Discovery says this is the default if unspecified. |
| `ANY` | Model must produce a function call and is constrained to the declared schema. |
| `NONE` | Function calls are disabled. Equivalent to omitting function declarations. |
| `VALIDATED` | Model may choose natural language or a function call, but function calls are validated with constrained decoding. |

Default-mode nuance from the official guide:

- `AUTO` is the default when only `functionDeclarations` are enabled.
- `VALIDATED` is the documented default when function declarations are combined with built-in tools or structured outputs.

`allowedFunctionNames`:

- Only valid with `ANY` or `VALIDATED`.
- Limits callable functions to the listed names.

### Function round-trip contract

Canonical function-calling loop:

1. User sends a normal `contents` turn.
2. Model responds with `candidate.content.parts[*].functionCall`.
3. Client executes the function.
4. Next request appends the model `Content` exactly as returned.
5. Client appends a new `user` `Content` containing `functionResponse`.

Canonical pattern:

```json
[
  {
    "role": "user",
    "parts": [
      {
        "text": "Get Boston weather"
      }
    ]
  },
  {
    "role": "model",
    "parts": [
      {
        "functionCall": {
          "id": "fc_123",
          "name": "get_weather",
          "args": {
            "city": "Boston"
          }
        },
        "thoughtSignature": "BASE64_SIGNATURE"
      }
    ]
  },
  {
    "role": "user",
    "parts": [
      {
        "functionResponse": {
          "id": "fc_123",
          "name": "get_weather",
          "response": {
            "result": {
              "temp_c": 12
            }
          }
        }
      }
    ]
  }
]
```

Round-trip rules:

- Prefer echoing the `functionCall.id` back in `functionResponse.id`.
- Official docs say Gemini 3 model APIs now generate a unique function-call ID for every call.
- Preserve the full model `Content`, not just the extracted `functionCall`.
- Do not invent `role: "function"` for the client reply; current examples use `role: "user"`.

Parallel-call caveats from the thought-signature guide:

- If the model returns parallel calls, the first `functionCall` in that response carries the signature.
- When sending the next turn back, keep the returned `functionCall` parts together before the corresponding `functionResponse` parts.
- Interleaving as `FC1, FR1, FC2, FR2` is explicitly documented as a 400-producing mistake; use `FC1, FC2, FR1, FR2`.

### Server-side tool interleaving

When custom functions are combined with built-in tools, Google explicitly warns that a single returned `parts[]` array may contain a mix of:

- `functionCall`
- `toolCall`
- `toolResponse`

Do not assume the last part is the function call. Always scan the whole `parts[]` array.

### Multimodal `functionResponse`

`FunctionResponse` can include:

- `response`: required structured JSON object
- `parts`: optional `FunctionResponsePart[]`

Current docs say:

- Each `FunctionResponsePart` carries `inlineData`.
- If the structured `response` object references a multimodal part, it does so as `{ "$ref": "<displayName>" }`.
- Each `displayName` can only be referenced once in the structured `response`.

`FunctionResponse.willContinue` and `FunctionResponse.scheduling` exist in schema, but are only meaningful for non-blocking / generator-style function calls and are ignored otherwise.

## Thinking and thought signatures

### `ThinkingConfig`

Current `generationConfig.thinkingConfig` fields:

| Field | Notes |
| --- | --- |
| `thinkingBudget` | Integer count of thought tokens to generate. |
| `includeThoughts` | If `true`, thought parts are returned when available. |
| `thinkingLevel` | `MINIMAL`, `LOW`, `MEDIUM`, `HIGH`. Docs say the default is `HIGH` and this option is recommended for Gemini 3+; earlier models may reject it. |

Important wire observations:

- Thought content is represented as normal `Part` objects with `thought: true`.
- Thought signatures are carried in `Part.thoughtSignature` as opaque bytes.
- `usageMetadata.thoughtsTokenCount` tracks thought-token usage when available.

### Signature rules

Officially documented behavior:

- Single function call: the `functionCall` part contains a `thought_signature`.
- Parallel function calls in one response: only the first `functionCall` part carries the signature.
- You must return the signature in the exact part position where it was received.
- Validation is strict only for the current turn.
- If the first `functionCall` in a step is missing its signature on replay, the request fails with HTTP 400.

Non-function-call signatures:

- The final non-function part in a model response may also carry a `thought_signature`.
- Returning those signatures is recommended for reasoning quality.
- Validation is not strict for those non-function signatures; omission does not block the request.

Proxy portability rules:

- OpenAI-to-Gemini conversion must not add synthetic `thoughtSignature` values to tool-call history, reasoning parts, or replayed assistant turns.
- A real Gemini `thoughtSignature` may be preserved only on Gemini-native / same-provider passthrough traffic.
- Cross-protocol Gemini requests containing `thoughtSignature` or `thought_signature`, including nested `history[].parts[]` or function-response payloads, fail closed.
- Documented dummy validator signatures are not part of the current proxy behavior. They must not be generated as a compatibility shortcut for foreign or manually constructed traces.

## Safety settings and blocking

### `safetySettings`

`safetySettings[]` is request-scoped and applies to both:

- `GenerateContentRequest.contents`
- `GenerateContentResponse.candidates`

Rules:

- At most one setting per `SafetyCategory`.
- The listed settings override defaults for those categories only.
- Current Gemini request docs explicitly call out these Gemini categories: `HARM_CATEGORY_HATE_SPEECH`, `HARM_CATEGORY_SEXUALLY_EXPLICIT`, `HARM_CATEGORY_DANGEROUS_CONTENT`, `HARM_CATEGORY_HARASSMENT`, `HARM_CATEGORY_CIVIC_INTEGRITY`.
- Discovery marks `HARM_CATEGORY_CIVIC_INTEGRITY` as deprecated and points implementors to `enableEnhancedCivicAnswers`.
- Discovery still contains older PaLM-era enum values; treat the Gemini categories above as the practical baseline for current Gemini traffic.

Threshold values:

- `HARM_BLOCK_THRESHOLD_UNSPECIFIED`
- `BLOCK_LOW_AND_ABOVE`
- `BLOCK_MEDIUM_AND_ABOVE`
- `BLOCK_ONLY_HIGH`
- `BLOCK_NONE`
- `OFF`

`BLOCK_NONE` means all content is allowed. `OFF` disables the safety filter entirely.

### Prompt vs candidate blocking

Prompt-time block:

- No candidates are returned.
- Inspect `promptFeedback.blockReason` and `promptFeedback.safetyRatings`.

Candidate-time block:

- Candidates may be present.
- Inspect `candidate.finishReason`, `candidate.finishMessage`, and `candidate.safetyRatings`.

`PromptFeedback.blockReason` values:

| Value | Meaning |
| --- | --- |
| `BLOCK_REASON_UNSPECIFIED` | Unused default. |
| `SAFETY` | Prompt blocked for safety. |
| `OTHER` | Prompt blocked for another reason. |
| `BLOCKLIST` | Prompt blocked due to a terminology blocklist. |
| `PROHIBITED_CONTENT` | Prompt blocked for prohibited content. |
| `IMAGE_SAFETY` | Prompt blocked for unsafe image-generation content. |

## `generationConfig`

`generationConfig` contains most sampling, response-shape, and thinking controls.

| Field | Notes |
| --- | --- |
| `candidateCount` | Defaults to `1`. Docs note it does not work on Gemini 1.0 family models. |
| `temperature` | Float in `[0.0, 2.0]`; default varies by model. |
| `topP` | Nucleus sampling control; default varies by model. |
| `topK` | Top-k control; some models do not allow it. |
| `maxOutputTokens` | Maximum response tokens. |
| `stopSequences` | Up to 5 stop strings. |
| `seed` | Decoding seed. |
| `presencePenalty` | Binary reuse penalty. |
| `frequencyPenalty` | Repetition penalty proportional to prior usage. |
| `responseLogprobs` | If `true`, expose logprob results. |
| `logprobs` | Number of top logprobs to return when `responseLogprobs` is enabled. |
| `responseMimeType` | Default `text/plain`. Also supports `application/json` and `text/x.enum`. |
| `responseSchema` | OpenAPI 3.0.3 subset. Requires compatible `responseMimeType`, typically `application/json`. |
| `responseJsonSchema` | Preferred JSON Schema alternative. Requires `responseMimeType`. |
| `_responseJsonSchema` | Present in schema but docs label it internal detail; use `responseJsonSchema` instead. |
| `thinkingConfig` | Thinking controls. |
| `responseModalities` | Exact set of modalities the response may contain. |
| `speechConfig` | Speech-generation options for supported models. |
| `imageConfig` | Image-generation options for supported models. |
| `mediaResolution` | Input media resolution hint. |
| `enableEnhancedCivicAnswers` | Civic-answering behavior toggle where supported. |

Schema constraints worth preserving:

- `responseSchema` is an OpenAPI-subset schema object.
- `responseJsonSchema` is a limited JSON Schema subset.
- Docs list supported JSON Schema keywords including `$id`, `$defs`, `$ref`, `$anchor`, `type`, `format`, `title`, `description`, `enum`, `items`, `prefixItems`, `minItems`, `maxItems`, `minimum`, `maximum`, `anyOf`, `oneOf`, `properties`, `additionalProperties`, `required`, plus non-standard `propertyOrdering`.

## Response contract

### Top-level `GenerateContentResponse`

| Field | Notes |
| --- | --- |
| `candidates` | Generated candidates. |
| `promptFeedback` | Prompt-level safety / block result. |
| `usageMetadata` | Token accounting. |
| `modelVersion` | Output-only model version used for generation. |
| `responseId` | Output-only response identifier. |
| `modelStatus` | Output-only model stage / retirement status. |

`modelStatus` is optional and includes:

- `modelStage`
- `message`
- `retirementTime`

### `Candidate`

| Field | Notes |
| --- | --- |
| `index` | Candidate index. |
| `content` | Generated `Content`. |
| `finishReason` | Output-only stop reason enum. |
| `finishMessage` | Human-readable explanation when `finishReason` is set. |
| `safetyRatings` | Candidate-level safety ratings. |
| `tokenCount` | Candidate token count. |
| `avgLogprobs` | Average log probability. |
| `logprobsResult` | Token-level logprob detail when requested. |
| `citationMetadata` | May carry recitation / citation info. |
| `groundingMetadata` | Grounding details for grounded responses. |
| `urlContextMetadata` | URL-context tool metadata. |

## `usageMetadata`

`usageMetadata` is output-only and currently includes:

| Field | Meaning |
| --- | --- |
| `promptTokenCount` | Effective prompt size. If `cachedContent` is used, this still includes cached tokens. |
| `cachedContentTokenCount` | Number of tokens that came from the referenced cache. |
| `candidatesTokenCount` | Total generated tokens across candidates. |
| `totalTokenCount` | Prompt plus candidate tokens. |
| `thoughtsTokenCount` | Thought-token count for thinking models. |
| `promptTokensDetails` | Per-modality breakdown for request input. |
| `cacheTokensDetails` | Per-modality breakdown for cached input. |
| `candidatesTokensDetails` | Per-modality breakdown for returned output. |
| `toolUsePromptTokenCount` | Tokens present in tool-use prompts. |
| `toolUsePromptTokensDetails` | Per-modality breakdown for tool-use prompts. |

Per-modality counters use `ModalityTokenCount` with current modalities:

- `TEXT`
- `IMAGE`
- `VIDEO`
- `AUDIO`
- `DOCUMENT`

## Finish reasons

Current `Candidate.finishReason` enum set from discovery revision `20260415`:

| Finish reason | Meaning |
| --- | --- |
| `FINISH_REASON_UNSPECIFIED` | Unused default. |
| `STOP` | Natural stop point or matched stop sequence. |
| `MAX_TOKENS` | Request token cap reached. |
| `SAFETY` | Candidate blocked for safety. |
| `RECITATION` | Candidate blocked for recitation. |
| `LANGUAGE` | Candidate used an unsupported language. |
| `OTHER` | Other reason. |
| `BLOCKLIST` | Candidate contained forbidden terms. |
| `PROHIBITED_CONTENT` | Candidate potentially contained prohibited content. |
| `SPII` | Candidate potentially contained sensitive PII. |
| `MALFORMED_FUNCTION_CALL` | Generated function call was invalid. |
| `IMAGE_SAFETY` | Generated image violated safety rules. |
| `IMAGE_PROHIBITED_CONTENT` | Generated image had other prohibited content. |
| `IMAGE_OTHER` | Generated image failed for another reason. |
| `NO_IMAGE` | Model was expected to generate an image but did not. |
| `IMAGE_RECITATION` | Image generation stopped due to recitation. |
| `UNEXPECTED_TOOL_CALL` | Model called a tool even though no tools were enabled. |
| `TOO_MANY_TOOL_CALLS` | Tool-calling loop exceeded limits. |
| `MISSING_THOUGHT_SIGNATURE` | Request replay was missing a required thought signature. |
| `MALFORMED_RESPONSE` | Response was malformed. |

`finishMessage` may provide extra detail when `finishReason` is present.

## Cached contents

Google documents cache management as a separate resource family:

| Method | Endpoint |
| --- | --- |
| `cachedContents.create` | `POST /v1beta/cachedContents` |
| `cachedContents.list` | `GET /v1beta/cachedContents` |
| `cachedContents.get` | `GET /v1beta/{name=cachedContents/*}` |
| `cachedContents.patch` | `PATCH /v1beta/{name=cachedContents/*}` |
| `cachedContents.delete` | `DELETE /v1beta/{name=cachedContents/*}` |

`CachedContent` fields relevant to `generateContent`:

| Field | Notes |
| --- | --- |
| `name` | Output-only. Format `cachedContents/{id}`. |
| `model` | Required and immutable. Cache is bound to a specific model. |
| `contents` | Immutable content payload stored in the cache. |
| `systemInstruction` | Optional, immutable, text-only. |
| `tools` | Optional, immutable. |
| `toolConfig` | Optional, immutable. |
| `displayName` | Optional human label. |
| `ttl` | Input-only duration. |
| `expireTime` | Always returned on output. |
| `usageMetadata.totalTokenCount` | Total tokens consumed by the cached content. |

GenerateContent interaction:

- `GenerateContentRequest.cachedContent` points at a `CachedContent.name`.
- Prompt token accounting still includes the cache in `promptTokenCount`.
- `cachedContentTokenCount` breaks out the cached subset.

The captured docs separate cache creation from request-time cache references: reusable `systemInstruction`, `tools`, and `toolConfig` can live on `CachedContent`, while `GenerateContentRequest.cachedContent` is only a reference.

## `camelCase` vs `snake_case` in official docs

Google's current documentation mixes JSON naming styles.

Observed patterns:

- Discovery JSON and most JS / raw REST reference snippets use `camelCase`.
- Python SDK examples use `snake_case` because they are SDK arguments, not raw JSON.
- Some `curl` snippets embedded in the docs still show `snake_case` JSON payloads because they come from older or SDK-adjacent sample sources.

Important examples:

| Canonical wire field | Official snake_case variant seen in docs |
| --- | --- |
| `systemInstruction` | `system_instruction` |
| `cachedContent` | `cached_content` |
| `generationConfig` | `generation_config` |
| `thinkingConfig` | `thinking_config` |
| `responseMimeType` | `response_mime_type` |
| `responseSchema` | `response_schema` |
| `functionCallingConfig` | `function_calling_config` |
| `allowedFunctionNames` | `allowed_function_names` |
| `thoughtSignature` | `thought_signature` |
| `usageMetadata` | `usage_metadata` |

This baseline refers to raw REST field names in `camelCase` because discovery JSON and most raw REST examples use that spelling. The captured docs also show `snake_case` in SDK-oriented or older examples, and `thoughtSignature` remains an opaque byte field regardless of spelling.

## See also

See also [the Google snapshot manifest](snapshots/2026-04-16/google-manifest.json).
