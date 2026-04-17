# Provider Capability Matrix

- Layer: capability-diff matrix
- Status: active
- Last refreshed: 2026-04-16
- Scope: one-page comparison of the protocol surfaces the proxy has to reason about

Legend: provider-status cells answer only whether the capability is officially documented on that provider surface. `Native` means documented on the surface covered by the vendor baseline. `Limited` means documented on that same surface but with narrower shape, model coverage, or lifecycle semantics. `Guide/Beta` means officially documented only in adjacent guides or beta-marked surfaces. `No official surface` means the baseline for that provider does not document it on this surface. Portability guidance lives in `Proxy note`.

| Capability | OpenAI Responses | OpenAI Chat Completions | Anthropic Messages | Gemini `generateContent` | Proxy note |
| --- | --- | --- | --- | --- | --- |
| Stateless text conversation | Native | Native | Native | Native | Common portability floor |
| Typed multimodal input parts | Native | Native | Native | Native | Shapes still differ by provider |
| Function calling | Native | Native | Native | Native | Best common tool surface |
| Hosted / server tools | Native | No official surface | Native | Native | Treat as vendor-specific |
| Remote MCP tools | Native | No official surface | Guide/Beta | Native | Same-provider passthrough only |
| Tool parallelism control | Native | Native | Native | No official surface | Intent only can be preserved |
| Reasoning request control | Native | Limited | Native | Native | Non-portable request syntax |
| Reasoning output as typed structure | Native | Limited | Native | Native | Summary text is the practical common denominator |
| Cached prompt reuse | Native | Native | Native | Native | Semantics differ sharply |
| Named cache resource | No official surface | No official surface | No official surface | Native | Gemini-specific |
| Rich typed streaming lifecycle | Native | Limited | Native | No official surface | Event adapters required |
| Explicit incomplete / failed stream terminal | Native | Limited | Limited | Limited | Important for Responses-native clients |
| Native response-chain handle | Native | No official surface | No official surface | No official surface | OpenAI-specific state model |
| Native conversation resource | Native | No official surface | Guide/Beta | No official surface | Do not emulate across providers |
| Native compaction surface | Native | No official surface | Guide/Beta | No official surface | Treat as provider-native state management |
| Background / async run mode | Native | No official surface | No official surface | No official surface | Keep same-provider |
| Service tier in request surface | Native | Native | Native | Native | Usually passthrough only |
| Request-side persistence flag | Native (`store`) | Native (`store`) | No official surface | Native (`store`) | Not portable |

## Capability drill-downs

| Topic | Detailed note |
| --- | --- |
| Reasoning | [`../capabilities/reasoning.md`](../capabilities/reasoning.md) |
| Cache | [`../capabilities/cache.md`](../capabilities/cache.md) |
| Tools | [`../capabilities/tools.md`](../capabilities/tools.md) |
| Streaming | [`../capabilities/streaming.md`](../capabilities/streaming.md) |
| State continuity | [`../capabilities/state-continuity.md`](../capabilities/state-continuity.md) |
