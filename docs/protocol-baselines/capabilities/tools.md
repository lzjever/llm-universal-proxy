# Tool Capability Notes

- Layer: capability-diff
- Status: active
- Last refreshed: 2026-04-16
- Scope: function tools, hosted/server tools, MCP surfaces, and tool result portability

## Summary

Function calling is the only dependable cross-provider core. Everything else should be treated as provider-native capability, not as a guaranteed portable tool abstraction.

For agent clients, visible tool identity is part of that core contract. A proxy may bridge argument shape or result shape, but it must not rename the stable tool identity supplied by the client on any model-visible or client-visible surface.

Locked tool identity contract:

- The proxy must not rewrite the visible tool name supplied by the client.
- `__llmup_custom__*` is an internal transport artifact, not a public contract.
- `apply_patch` remains a public freeform tool on client-visible surfaces.

In the table below, provider columns describe the official tool surface. Portability judgment lives in the proxy-guidance column.

## Provider comparison

| Dimension | OpenAI Responses | OpenAI Chat Completions | Anthropic Messages | Proxy guidance |
| --- | --- | --- | --- | --- |
| Function tools | Native | Native | Native via `tools[].input_schema` | This is the portability baseline. |
| Hosted / server tools | Rich official surface: web search, file search, code interpreter, image generation, computer use, shell, MCP/connectors, and more | No official hosted/server tool family in the public Chat create surface | Server tools and MCP connector are first-class, but with Anthropic-specific blocks and stop reasons | Preserve hosted/server tools only on same-provider/native passthrough lanes or through explicit compatibility shims. Cross-provider translation should default to drop-or-warn. |
| MCP / remote tools | Remote MCP is an official Responses tool family | No portable Chat-native MCP schema | `mcp_servers` and MCP connector are Anthropic-specific beta surfaces | Never pretend these are interchangeable. |
| Tool choice | `auto`, `none`, required/forced tool variants | Similar function-tool control surface | `auto`, `none`, `any`, `tool`, plus flags like `disable_parallel_tool_use` | Map only the intent you can prove. Forced tool use is usually approximate. |
| Parallelism | Explicit `parallel_tool_calls` | Explicit `parallel_tool_calls` | Exposed as a disable flag inside tool choice | Preserve only the "allow vs disallow parallelism" intent when possible. |
| Tool result shape | Typed output items and streamed tool events | `tool_calls` plus tool-role follow-up messages | `tool_use`, `tool_result`, and server-tool blocks | Normalize around function name, arguments, and result payload only. |

Google OpenAI-compatible Gemini uses the OpenAI Chat-compatible tool surface in
active proxy behavior. Native Google/Gemini tool details are retired historical
baseline context, not active proxy support.

## What should be considered vendor-specific

| Vendor-specific area | Why it should stay vendor-specific |
| --- | --- |
| OpenAI hosted tools and include expansions | They depend on Responses-specific item types and tool event families. |
| Anthropic server tools and `pause_turn` loops | They rely on Messages block semantics and server-side iteration limits. |
| Anthropic MCP connector | It uses request fields, beta headers, and result blocks that have no OpenAI mirror. |

## Implementation stance

1. Treat function calling as the universal core.
2. Gate hosted/server tools behind same-provider/native passthrough or explicit compatibility shims.
3. Emit compatibility warnings whenever non-function tool capabilities are dropped or approximated.
4. Never rely on visible synthetic tool renaming as the primary live bridge for agent-facing translated paths. If a function-only bridge is needed, preserve the original visible tool name and carry bridge provenance in request-scoped translator context.
