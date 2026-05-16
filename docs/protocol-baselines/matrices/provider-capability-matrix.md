# Provider Capability Matrix

- Layer: capability-diff matrix
- Status: active
- Vendor snapshot/captured date: 2026-04-16
- Latest online recheck date: 2026-05-16
- Proxy posture updated date: 2026-04-26
- Scope: one-page comparison of the protocol surfaces the proxy has to reason about

Legend: provider-status cells answer only whether the capability is officially documented on that provider surface. `Native` means documented on the surface covered by the vendor baseline. `Limited` means documented on that same surface but with narrower shape, model coverage, or lifecycle semantics. `Guide/Beta` means officially documented only in adjacent guides or beta-marked surfaces. `No official surface` means the baseline for that provider does not document it on this surface. Portability guidance lives in `Proxy note`.

Provider columns are vendor contract snapshot/source facts. `Proxy note` is proxy policy/proxy posture and may be updated without claiming a new vendor refresh.

| Capability | OpenAI Responses | OpenAI Chat Completions | Anthropic Messages | Proxy note |
| --- | --- | --- | --- | --- |
| Stateless text conversation | Native | Native | Native | Common portability floor |
| Typed multimodal input parts | Native | Native | Native | First-phase proxy support is gated by `surface.modalities.input`; model/provider availability is not implied |
| PDF, generic file, and video inputs | Native | Native | Limited | `pdf` is narrow, `file` includes PDF, and video currently stays gate-only |
| Function calling | Native | Native | Native | Best common tool surface |
| Hosted / server tools | Native | No official surface | Native | Treat as vendor-specific |
| Remote MCP tools | Native | No official surface | Guide/Beta | Same-provider passthrough only |
| Tool parallelism control | Native | Native | Native | Intent only can be preserved |
| Reasoning request control | Native | Limited | Native | Non-portable request syntax; request-side opaque reasoning carriers follow the default/max_compat vs strict/balanced downgrade rules in the reasoning note |
| Reasoning output as typed structure | Native | Limited | Native | Summary text is the practical common denominator |
| Cached prompt reuse | Native | Native | Native | Semantics differ sharply |
| Rich typed streaming lifecycle | Native | Limited | Native | Event adapters required |
| Explicit incomplete / failed stream terminal | Native | Limited | Limited | Important for Responses-native clients |
| Native response-chain handle | Native | No official surface | No official surface | OpenAI-specific state model |
| Native conversation resource | Native | No official surface | Guide/Beta | Do not emulate across providers |
| Native compaction surface | Native | No official surface | Guide/Beta | Treat as provider-native state management; request-side compaction input follows the visible summary/history downgrade rules in the state-continuity note |
| Background / async run mode | Native | No official surface | No official surface | Keep same-provider |
| Service tier in request surface | Native | Native | Native | Usually passthrough only |
| Request-side persistence flag | Native (`store`) | Native (`store`) | No official surface | Not portable |

Google OpenAI-compatible Gemini follows the OpenAI Chat-compatible column for
active proxy behavior. Native Google/Gemini capability comparisons live only in
retired historical baseline material.

## Capability drill-downs

| Topic | Detailed note |
| --- | --- |
| Reasoning | [`../capabilities/reasoning.md`](../capabilities/reasoning.md) |
| Cache | [`../capabilities/cache.md`](../capabilities/cache.md) |
| Tools | [`../capabilities/tools.md`](../capabilities/tools.md) |
| Streaming | [`../capabilities/streaming.md`](../capabilities/streaming.md) |
| State continuity | [`../capabilities/state-continuity.md`](../capabilities/state-continuity.md) |
