# Field Mapping Matrix

- Layer: capability-diff matrix
- Status: active
- Vendor snapshot/captured date: 2026-04-16
- Proxy posture updated date: 2026-04-26
- Scope: high-risk field mappings, intentional drops, and downgrade notes

Legend: `Exact` means the proxy can preserve intent closely. `Approx` means important behavior is preserved but not the exact provider contract. `Fail-closed` means there is no safe cross-provider translation and the proxy rejects the request before contacting upstream. `Warn/drop opaque carrier` means the proxy may warn and remove an opaque provider-owned carrier only when visible portable context remains. `Native-only` means the field is preserved only on raw/native passthrough.

Provider columns name the official field family on that surface. `Mapping status` is where cross-provider portability is judged.

| Intent | OpenAI Responses | OpenAI Chat Completions | Anthropic Messages | Mapping status | Notes |
| --- | --- | --- | --- | --- | --- |
| Core conversation input | `input` | `messages` | `messages` plus top-level `system` | Approx | Same concept, different wire shape |
| Typed media input parts | `input_image`, `input_audio`, `input_file` | image, audio, and file content parts | image and document blocks | Approx / Fail-closed | Translate only supported media in the effective surface and only source transports the target can represent. HTTP(S) URLs are distinct from provider-native or local URIs such as `gs://`, `s3://`, and `file://`; unsupported media or source forms, provider `file_id`, unknown typed parts, and conflicting MIME provenance fail closed. |
| System / developer instruction | `instructions` or high-priority input roles | `system` message | top-level `system` | Approx | Hierarchy and placement differ |
| Function tool definitions | `tools` | `tools` | `tools` with `input_schema` | Approx | Function-only portability |
| Hosted / server tool definitions | Rich Responses tool families | No official hosted/server tool family on Chat create | Server tools and MCP connector | Native-only / Fail-closed | Keep raw/native only |
| Tool choice: auto / none | Native strings | Native strings | Object form | Approx | Intent can be preserved, schema cannot |
| Tool choice: required / any / forced tool | Native | Native | `any` or `tool` | Approx | Forced-tool semantics are not identical |
| Parallel tool use | `parallel_tool_calls` | `parallel_tool_calls` | `disable_parallel_tool_use` | Approx | Inversion on Anthropic side |
| Reasoning request policy | `reasoning` | Model-specific | `thinking` | Fail-closed | Same idea, different execution contract |
| Reasoning output | Typed reasoning items / summaries | Model-specific output fields | `thinking` blocks | Approx | Preserve summaries and usage where possible |
| Reasoning opaque state | `reasoning.encrypted_content`, reasoning item `encrypted_content` | No stable equivalent | No stable equivalent | Native-only / Warn/drop opaque carrier / Fail-closed | Raw/native passthrough preserves the carrier. In maximum-compatible request translation, warn/drop opaque carrier fields only when visible summary or visible transcript/history remains; opaque-only reasoning state fails closed. Never synthesize. Response-side reasoning encrypted_content has a separate Anthropic carrier recovery path. |
| Prompt-cache control | `prompt_cache_key`, retention policy | `prompt_cache_key` on supported surfaces | `cache_control` | Native-only / Fail-closed | Not the same primitive |
| Cached-token usage | `cached_tokens` | `cached_tokens` | cache read/write token fields | Approx | Accounting models differ |
| Follow-up response handle | `previous_response_id`, conversations | No stable equivalent | No stable equivalent | Native-only / Fail-closed | Requires provider-owned state |
| Compaction | `context_management`, `/responses/compact`, compaction items | No stable equivalent | beta `context_management` compaction | Native-only / Warn/drop opaque carrier / Fail-closed | Native state surfaces stay raw/native only. In maximum-compatible request translation, request-side compaction input items may warn/drop opaque carrier fields only when each degraded item has explicit visible summary text, or when non-compaction visible transcript/history remains; opaque-only compaction still fails closed, and one summarized compaction item does not permit another opaque-only compaction item to be silently dropped. Native Responses passthrough preserves compaction items unchanged. |
| Stream failure / incomplete terminal | `response.failed`, `response.incomplete` | finish reason or HTTP error | stop reason plus HTTP error | Approx | Normalize for downstream needs |
| Context-window overflow signal | explicit failure shape | error / finish reason | `model_context_window_exceeded` stop reason | Approx | Semantics differ materially |
| Metadata | `metadata` | `metadata` on compatible implementations | `metadata` | Approx | Safe only within compatible families |
| Storage / persistence | `store` plus stored response resources | Official `store` field for stored completion artifacts | No official request-side persistence flag | Native-only / Fail-closed | Storage semantics differ |
| Service tier | `service_tier` | Official `service_tier` request field | Official `service_tier` request field on current Messages surfaces | Native-only | Passthrough only; tier semantics are vendor-specific |

Google OpenAI-compatible Gemini is treated as an OpenAI Chat-compatible upstream
in this active matrix. Native Google/Gemini mappings are retained only as
retired historical baseline material, not as an active proxy capability.

## Use this matrix with

| If you are deciding... | Read |
| --- | --- |
| Whether a feature should warn, drop, or normalize | [`../audits/2026-04-16-spec-refresh.md`](../audits/2026-04-16-spec-refresh.md) |
| Whether a capability is broadly portable at all | [`provider-capability-matrix.md`](provider-capability-matrix.md) |
