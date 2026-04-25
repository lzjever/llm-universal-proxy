# Field Mapping Matrix

- Layer: capability-diff matrix
- Status: active
- Last refreshed: 2026-04-16
- Scope: high-risk field mappings, intentional drops, and downgrade notes

Legend: `Exact` means the proxy can preserve intent closely. `Approx` means important behavior is preserved but not the exact provider contract. `Drop` means there is no safe cross-provider translation; high-risk request inputs should be rejected rather than silently omitted.

Provider columns name the official field family on that surface. `Mapping status` is where cross-provider portability is judged.

| Intent | OpenAI Responses | OpenAI Chat Completions | Anthropic Messages | Gemini `generateContent` | Mapping status | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| Core conversation input | `input` | `messages` | `messages` plus top-level `system` | `contents` plus `systemInstruction` | Approx | Same concept, different wire shape |
| Typed media input parts | `input_image`, `input_audio`, `input_file` | image, audio, and file content parts | image and document blocks | `inlineData` and `fileData` parts | Approx / Drop | Translate only supported media in the effective surface. Unsupported media, unknown typed parts, Gemini video to non-Gemini targets, and conflicting MIME provenance fail closed. |
| System / developer instruction | `instructions` or high-priority input roles | `system` message | top-level `system` | `systemInstruction` | Approx | Hierarchy and placement differ |
| Function tool definitions | `tools` | `tools` | `tools` with `input_schema` | `tools.functionDeclarations` | Approx | Function-only portability |
| Hosted / server tool definitions | Rich Responses tool families | No official hosted/server tool family on Chat create | Server tools and MCP connector | Official built-in/server-side `Tool` branches plus `toolConfig` and `mcpServers` | Drop | Keep same-provider only |
| Tool choice: auto / none | Native strings | Native strings | Object form | Function-calling mode | Approx | Intent can be preserved, schema cannot |
| Tool choice: required / any / forced tool | Native | Native | `any` or `tool` | Function-calling mode `ANY` is closest | Approx | Forced-tool semantics are not identical |
| Parallel tool use | `parallel_tool_calls` | `parallel_tool_calls` | `disable_parallel_tool_use` | No global equivalent | Approx | Inversion on Anthropic side |
| Reasoning request policy | `reasoning` | Model-specific | `thinking` | `thinkingConfig` | Drop | Same idea, different execution contract |
| Reasoning output | Typed reasoning items / summaries | Model-specific output fields | `thinking` blocks | Thought-bearing candidate content | Approx | Preserve summaries and usage where possible |
| Reasoning opaque state | `reasoning.encrypted_content` | No stable equivalent | No stable equivalent | No stable equivalent | Drop | Never synthesize |
| Prompt-cache control | `prompt_cache_key`, retention policy | `prompt_cache_key` on supported surfaces | `cache_control` | `cachedContent` or cache API | Drop | Not the same primitive |
| Cached-token usage | `cached_tokens` | `cached_tokens` | cache read/write token fields | `cachedContentTokenCount` | Approx | Accounting models differ |
| Follow-up response handle | `previous_response_id`, conversations | No stable equivalent | No stable equivalent | No stable equivalent | Drop | Requires provider-owned state |
| Compaction | `/responses/compact`, compaction items | No stable equivalent | beta `context_management` compaction | No stable equivalent | Drop | Provider-native state transform |
| Stream failure / incomplete terminal | `response.failed`, `response.incomplete` | finish reason or HTTP error | stop reason plus HTTP error | endpoint-specific terminal behavior | Approx | Normalize for downstream needs |
| Context-window overflow signal | explicit failure shape | error / finish reason | `model_context_window_exceeded` stop reason | provider-specific | Approx | Semantics differ materially |
| Metadata | `metadata` | `metadata` on compatible implementations | `metadata` | no direct portable equivalent | Approx | Safe only within compatible families |
| Storage / persistence | `store` plus stored response resources | Official `store` field for stored completion artifacts | No official request-side persistence flag | Official `store` request flag | Drop | Storage semantics differ |
| Service tier | `service_tier` | Official `service_tier` request field | Official `service_tier` request field on current Messages surfaces | `serviceTier` | Drop | Passthrough only; tier semantics are vendor-specific |

## Use this matrix with

| If you are deciding... | Read |
| --- | --- |
| Whether a feature should warn, drop, or normalize | [`../audits/2026-04-16-spec-refresh.md`](../audits/2026-04-16-spec-refresh.md) |
| Whether a capability is broadly portable at all | [`provider-capability-matrix.md`](provider-capability-matrix.md) |
