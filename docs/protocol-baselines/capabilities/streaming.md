# Streaming Capability Notes

- Layer: capability-diff
- Status: active
- Last refreshed: 2026-04-16
- Scope: transport, event taxonomy, terminal semantics, and downgrade behavior

## Summary

All four providers support streaming-like delivery, but the unit of streaming is different:

| Provider | Streaming unit |
| --- | --- |
| OpenAI Responses | Typed lifecycle events and typed output-item deltas |
| OpenAI Chat Completions | Choice deltas ending in `[DONE]` |
| Anthropic Messages | Block lifecycle SSE events plus stop reasons |
| Gemini `streamGenerateContent` | Candidate-oriented incremental output |

All four providers officially document streaming delivery. What changes is whether the stream exposes a rich named lifecycle beyond incremental output chunks.

## Provider comparison

| Dimension | OpenAI Responses | OpenAI Chat Completions | Anthropic Messages | Gemini `streamGenerateContent` | Proxy guidance |
| --- | --- | --- | --- | --- | --- |
| Streaming delivery | SSE | SSE | SSE | Streaming resource family, commonly consumed as SSE | Stream transport is portable; event schema is not. |
| Rich typed lifecycle | Text, tool args, output items, and terminal events are typed separately | Chunk objects only, not a named lifecycle event family | Block start/delta/stop plus message delta/stop | Incremental candidate content, not a named lifecycle event family | Adapters should normalize to the client's expected event family, not to an imagined universal schema. |
| Delta granularity | Text, tool args, output items, and terminal events are typed separately | One delta envelope per choice chunk | Block start/delta/stop plus message delta/stop | Incremental candidate content, not item lifecycle events | Preserve chunk ordering and candidate boundaries before trying to coerce event shapes. |
| Terminal semantics | Explicit `response.completed`, `response.incomplete`, `response.failed` | Implicit final chunk plus finish reason | Successful response still carries workflow stop reasons such as `pause_turn` and `model_context_window_exceeded` | Terminal chunk semantics differ by SDK and endpoint | Be careful not to collapse "successful but unfinished workflow" into "clean completion." |
| Tool streaming | Native and detailed | Partial via tool-call argument deltas | Native block-level tool streaming | Candidate-based function-call updates | Preserve function-call progress where possible; hosted-tool detail rarely survives translation. |
| Safety / refusal streaming | Failure and incomplete are first-class stream terminals | Model-specific finish reasons | Anthropic now documents streaming refusals explicitly | Provider-specific | Downstream clients often need proxy-normalized failure vs refusal behavior. |

## Risk hotspots

| Hotspot | Why it matters |
| --- | --- |
| `pause_turn` | Anthropic treats this as a successful workflow pause, not a final answer. |
| `response.failed` vs HTTP error | Responses clients often expect an explicit terminal event even when other providers would only return an HTTP error. |
| Context-window overflow | Anthropic may report a successful stop reason where OpenAI-style clients expect a hard failure signal. |

## Implementation stance

1. Normalize terminals to the downstream protocol, but record when the meaning changed.
2. Prefer explicit incomplete/failure events for Responses-native clients.
3. Preserve tool-call progress and usage totals ahead of provider-specific event detail.
