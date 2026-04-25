# State Continuity Notes

- Layer: capability-diff
- Status: active
- Last refreshed: 2026-04-16
- Scope: follow-up turns, replay semantics, compaction, and durable vs request-scoped state

## Summary

State continuity is where protocol expectations diverge the most. "Conversation" can mean a replayed transcript, a server-tracked response chain, a cache handle, or a beta context-management surface depending on the provider.

## Provider comparison

| Dimension | OpenAI Responses | OpenAI Chat Completions | Anthropic Messages | Gemini `generateContent` | Proxy guidance |
| --- | --- | --- | --- | --- | --- |
| Native follow-up handle | `previous_response_id` and conversation-oriented resources | None; caller replays `messages` | No stable GA response handle in Messages create; replay is the default | SDK chat helpers replay full history behind the scenes | Never assume a cross-provider follow-up ID exists. |
| Server-side conversation resource | Yes, now first-class in OpenAI API navigation | No | Beta/adjacent context-management surfaces exist, but not as an OpenAI-style response chain | No equivalent conversation resource in core `generateContent` | Proxy should not invent resource-backed state unless it owns that state. |
| Compaction / context editing | Official `/responses/compact` plus compaction items | No native Chat equivalent | Compaction and context editing are documented as beta context-management features | No native compaction resource; caching is separate | Compaction is not a portable feature. Treat it as provider-native state transformation. |
| Long-running execution | `background` mode on Responses | No equivalent | Tool loops continue by replaying assistant output; beta containers/context management expand this story | Stateless retry/replay model, optionally aided by caches | Async continuation semantics should stay same-provider. |
| Tool-loop resume | Resource and event aware | Manual replay through messages | `pause_turn` means "send the assistant output back" | Manual replay | Resume rules must be documented per provider, not generalized. |

## Non-portable state surfaces to watch

| Surface | Why it is risky |
| --- | --- |
| OpenAI `previous_response_id`, conversations, `context_management`, and compaction | They imply upstream-managed state the proxy does not reconstruct today. |
| Anthropic `context_management`, containers, and MCP server state | These are stateful beta surfaces with no safe OpenAI or Gemini mirror. |
| Gemini `cachedContent` | It is a cache reference, not a conversation cursor. |

## Implementation stance

1. Prefer explicit transcript replay as the common denominator.
2. Preserve native state handles only on passthrough paths where routing is unambiguous.
3. Fail clearly when a request requires provider state the proxy does not own.
