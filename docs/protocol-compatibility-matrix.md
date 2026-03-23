# Protocol Compatibility Matrix

- Sources:
  - OpenAI Responses API: https://platform.openai.com/docs/api-reference/responses/object
  - OpenAI Chat Completions API: https://platform.openai.com/docs/api-reference/chat/create
  - Anthropic Messages API: https://docs.anthropic.com/en/api/messages
  - Anthropic stop reasons: https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons
- Purpose: document field-level mappings, intentional degradations, and known non-1:1 cases across the proxy's three primary chat protocols.
- Updated: 2026-03-22

## Legend

| Status | Meaning |
|--------|---------|
| Exact | Proxy preserves semantics closely enough for downstream behavior to match. |
| Approx | Proxy preserves the most important downstream behavior, but the wire shape or provider semantics differ. |
| Unsupported | No safe 1:1 mapping exists today; the proxy drops or degrades the field and this is intentional. |

## Request fields

| Feature | OpenAI Responses | OpenAI Chat Completions | Anthropic Messages | Proxy behavior | Status | Notes |
|--------|-------------------|-------------------------|--------------------|----------------|--------|-------|
| Basic text input | `input` string or items | `messages` | `messages` plus top-level `system` | Mapped both directions through the chat pivot | Exact | Main conversation path. |
| System instructions | `instructions` | `system` role message | top-level `system` | Responses `instructions` maps to Chat `system`, then to Anthropic `system` | Approx | Anthropic system is top-level, not a message role. |
| Function tools | `tools` with function tool definitions | `tools[].function` | `tools[].name/input_schema` | Function tools only are mapped across all three | Approx | Non-function tools are not portable. |
| Built-in / non-function tools | Responses built-ins like web search, shell, MCP-related tools | Not native in Chat Completions | Anthropic server tools have different shapes | Dropped when converting into Chat/Anthropic request formats | Unsupported | Proxy avoids emitting invalid request shapes. |
| Tool choice: auto | `tool_choice: "auto"` | `tool_choice: "auto"` | `tool_choice: { "type": "auto" }` | Mapped | Exact | |
| Tool choice: none | `tool_choice: "none"` | `tool_choice: "none"` | `tool_choice: { "type": "none" }` | Mapped | Exact | |
| Tool choice: required / any | `tool_choice: "required"` | `tool_choice: "required"` | `tool_choice: { "type": "any" }` | Mapped | Approx | Anthropic uses `any` rather than `required`. |
| Tool choice: force one function | Responses function choice object | Chat `tool_choice.type=function` | Anthropic `tool_choice.type=tool` | Mapped by function name | Approx | Only function-name forcing is preserved. |
| `parallel_tool_calls` | Supported | Partially supported in Chat ecosystems | Anthropic uses `disable_parallel_tool_use` | `false` maps to Anthropic `disable_parallel_tool_use: true` when tool choice is present | Approx | Only the "disable parallel calls" intent is preserved. |
| `max_output_tokens` / `max_tokens` | `max_output_tokens` | `max_tokens` | `max_tokens` | Mapped | Exact | |
| `previous_response_id` | Supported | Not supported | Not supported | Dropped when leaving Responses | Unsupported | Cannot be reconstructed from stateless Chat or Anthropic requests. |
| `store` | Supported | Different / not portable | Not portable | Dropped when leaving Responses | Unsupported | Storage semantics are provider-specific. |
| `metadata` | Supported | Provider-specific | Provider-specific | Not translated today | Unsupported | Safer to drop than emit unsupported payloads. |
| `truncation` | Supported | No exact equivalent | No exact equivalent | Dropped when leaving Responses | Unsupported | The runtime truncation policy cannot be reproduced in other protocols. |
| `reasoning` request config | Supported | Provider-specific | Provider-specific extended thinking knobs | Dropped when crossing protocols | Unsupported | Only reasoning output usage is mapped today, not request policy. |
| `include` | Supported | Not equivalent | Not equivalent | Dropped when leaving Responses | Unsupported | Some included fields have no Chat or Anthropic representation. |
| `max_tool_calls` | Supported | No direct equivalent | No direct equivalent | Dropped when leaving Responses | Unsupported | Downstream cannot enforce the same global built-in-tool cap. |

## Response fields and statuses

| Feature | OpenAI Responses | OpenAI Chat Completions | Anthropic Messages | Proxy behavior | Status | Notes |
|--------|-------------------|-------------------------|--------------------|----------------|--------|-------|
| Text output | `output[].message.content[].output_text` | `choices[].message.content` | `content[].text` | Mapped | Exact | |
| Reasoning output | `output[].reasoning.summary[]` | `reasoning_content` side channel | `thinking` blocks | Mapped as summarized reasoning text | Approx | Encrypted / rich reasoning metadata is not preserved. |
| Function call output | `function_call` / `function_call_output` items | `tool_calls` / `tool` role | `tool_use` / `tool_result` blocks | Mapped | Approx | Function tools only. |
| Usage: input/output/total | `input_tokens`, `output_tokens`, `total_tokens` | `prompt_tokens`, `completion_tokens`, `total_tokens` | `input_tokens`, `output_tokens` | Mapped | Exact | |
| Usage: cached tokens | `input_tokens_details.cached_tokens` | `prompt_tokens_details.cached_tokens` | `cache_read_input_tokens` / `cache_creation_input_tokens` | Mapped to best available equivalent | Approx | Anthropic cache creation and cache read do not collapse perfectly into one Responses field. |
| Usage: reasoning tokens | `output_tokens_details.reasoning_tokens` | `completion_tokens_details.reasoning_tokens` | No exact equivalent in base Messages API | Mapped where present | Approx | Anthropic may not provide the same split. |
| Completed status | `status: completed` / `response.completed` | `finish_reason: stop` | `stop_reason: end_turn` | Mapped | Exact | |
| Incomplete due to length | `status: incomplete`, `reason: max_output_tokens` | `finish_reason: length` | `stop_reason: max_tokens` | Mapped | Exact | Responses now emits `response.incomplete` in streaming conversions. |
| Incomplete due to filtering | `status: incomplete`, `reason: content_filter` | `finish_reason: content_filter` | `stop_reason: refusal` is nearest semantic equivalent | Mapped to incomplete/content filter | Approx | Anthropic refusal is not identical to OpenAI content filtering, but this preserves downstream guardrail handling better than `stop`. |
| Context window exceeded | `response.failed` with `error.code=context_length_exceeded` | HTTP/context error or synthetic `finish_reason` in some translated streams | `stop_reason: model_context_window_exceeded` in successful responses | Proxy normalizes to Responses failure and OpenAI-style context error semantics | Approx | Anthropic treats this as a successful stop reason; proxy upgrades it into an explicit downstream error because Codex and similar clients need that behavior. |
| Provider startup error before SSE body | `response.failed` | HTTP error | HTTP error | Proxy synthesizes Responses `response.failed` for Responses clients | Approx | Improves downstream compatibility for Codex. |
| `response.failed` from Responses upstream | Explicit failed event | No exact streaming error event | No exact equivalent | Proxy converts to a final OpenAI completion chunk with best-effort finish reason | Approx | Best effort for non-Responses clients. |
| `response.incomplete` from Responses upstream | Explicit incomplete event | Final chunk with `finish_reason=length/content_filter` | Final Claude stop reason | Proxy maps to best-effort finish reason | Approx | Preserves truncation/filter behavior for downstream consumers. |
| `pause_turn` | Not a Responses concept | No exact equivalent | Anthropic successful stop reason for server-tool loops | Not fully mapped today | Unsupported | Requires a higher-level resume protocol, not just field translation. |

## Streaming event lifecycle

| Feature | Proxy behavior | Status | Notes |
|--------|----------------|--------|-------|
| Responses child `response_id` | Emitted on child events | Exact | Added for Codex compatibility. |
| Function call event metadata | `call_id` and `name` preserved on delta/done events | Exact | |
| Text part annotations | Empty `annotations: []` emitted on text parts | Approx | Matches common OpenAI Responses examples. |
| `response.completed` usage details | `total_tokens`, cached tokens, reasoning tokens preserved | Exact | |
| `response.incomplete` emission | Emitted for `length` and `content_filter` finishes | Exact | |
| `response.failed` emission for Anthropic context overflows | Emitted | Approx | Upstream Anthropic stop reason is upgraded into an error event. |

## Important non-1:1 differences

| Source feature | Why no exact mapping exists | Proxy fallback |
|---------------|-----------------------------|----------------|
| `previous_response_id` | Responses supports stateful response chaining; Chat and Anthropic are stateless request formats | Drop field when leaving Responses; caller must inline prior context explicitly. |
| `store` | Persistence model is provider-specific | Drop field when leaving Responses. |
| Responses built-in tools | Chat Completions and Anthropic tool schemas are not the same API surface | Keep function tools only; drop built-ins on cross-protocol translation. |
| `truncation` | Provider-side context management policy cannot be reproduced in another protocol | Drop field and rely on downstream model/provider defaults. |
| Anthropic `pause_turn` | It is a workflow control signal, not a normal completion state | Currently documented as unsupported. |
| Anthropic `refusal` | Closest OpenAI equivalent is `content_filter`, but semantics are not identical | Map to `content_filter` because downstream safety handling is closer. |

## Operational guidance

- Prefer passthrough whenever the client and upstream both speak the same protocol.
- For Codex and other Responses-native clients, normalize hard failures into `response.failed` and truncations into `response.incomplete`; downstream behavior is better than returning a superficially successful `response.completed`.
- When a field is dropped intentionally, prefer documenting it over inventing unsupported wire shapes.
