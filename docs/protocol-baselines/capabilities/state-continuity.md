# State Continuity Notes

- Layer: capability-diff
- Status: active
- Vendor snapshot/captured date: 2026-04-16
- Proxy posture updated date: 2026-05-16
- Scope: follow-up turns, replay semantics, compaction, and durable vs request-scoped state

## Summary

State continuity is where protocol expectations diverge the most. "Conversation" can mean a replayed transcript, a server-tracked response chain, a cache handle, or a beta context-management surface depending on the provider.

## Provider comparison

| Dimension | OpenAI Responses | OpenAI Chat Completions | Anthropic Messages | Proxy guidance |
| --- | --- | --- | --- | --- |
| Native follow-up handle | `previous_response_id` and conversation-oriented resources | None; caller replays `messages` | No stable GA response handle in Messages create; replay is the default | Never assume a cross-provider follow-up ID exists. The optional local memory bridge only replays llmup-owned `resp_llmup_*` IDs. |
| Server-side conversation resource | Yes, now first-class in OpenAI API navigation | No | Beta/adjacent context-management surfaces exist, but not as an OpenAI-style response chain | Proxy should not invent resource-backed state unless it owns that state. |
| Compaction / context editing | Official `/responses/compact` plus compaction items | No native Chat equivalent | Compaction and context editing are documented as beta context-management features | Compaction resources and `context_management` are provider-native state control. Native Responses passthrough preserves them; cross-provider reconstruction fails closed. Request-side compaction input items may degrade in default/max_compat only when each degraded item has explicit visible summary text, or when non-compaction visible transcript/history remains. |
| Long-running execution | `background` mode on Responses | No equivalent | Tool loops continue by replaying assistant output; beta containers/context management expand this story | Async continuation semantics should stay same-provider. |
| Tool-loop resume | Resource and event aware | Manual replay through messages | `pause_turn` means "send the assistant output back" | Resume rules must be documented per provider, not generalized. |

Google OpenAI-compatible Gemini follows OpenAI Chat-compatible replay behavior
in the active proxy surface. Native Google/Gemini state details are retired
historical baseline context.

## Non-portable state surfaces to watch

| Surface | Why it is risky |
| --- | --- |
| OpenAI provider `previous_response_id`, conversations, `context_management`, and compaction | They imply upstream-managed state the proxy does not reconstruct. Request-side compaction input items also carry opaque state such as `encrypted_content`; that carrier is not forwarded across providers. default/max_compat can warn/drop it only when visible summary text or non-compaction visible transcript/history remains. |
| llmup local `resp_llmup_*` bridge IDs | Only valid when `conversation_state_bridge.mode=memory`, only for non-streaming text OpenAI Responses translated to OpenAI Chat or Anthropic, and only while the in-memory entry remains unexpired for the same local owner. |
| Anthropic `context_management`, containers, and MCP server state | These are stateful beta surfaces with no safe OpenAI mirror. |

## Implementation stance

1. Prefer explicit transcript replay as the common denominator.
2. Preserve native state handles only on passthrough paths where routing is unambiguous.
3. Keep `context_management`, compact resources, and provider-native state-control surfaces same-provider only; cross-provider state reconstruction fails closed.
4. For request-side compaction input items, strict/balanced modes fail closed. default/max_compat may warn/drop `encrypted_content` or another opaque carrier only when the specific compaction item has explicit visible summary text, or when the request includes non-compaction visible transcript/history. Opaque-only compaction input fails closed.
5. Native Responses passthrough preserves compaction items unchanged.
6. One summarized compaction item does not permit another opaque-only compaction item in the same request to be silently dropped.
7. The local memory bridge is not a provider resource API: it does not import external `resp_*` IDs, persist across restarts, capture streams, or replay tools/reasoning/compaction/background work.
