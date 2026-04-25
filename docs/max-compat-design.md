# Maximum Compatibility Design

Status: active  
Last updated: 2026-04-25

## Summary

`llm-universal-proxy` uses a client-first maximum compatibility posture for translated paths, while staying protocol-first in the core architecture.

This means:

- do not model `Codex`, `Claude`, and `Gemini` as first-class data-plane identities
- do model a unified `capability surface` for each local model alias
- do keep `max_compat` as an explicit runtime policy mode
- do keep provider-owned state and provider-native lifecycle features native-only
- do treat transport-only bridge artifacts as internal machinery, not as user-facing contract

Locked contract:

- The proxy must not rewrite the visible tool name supplied by the client.
- `__llmup_custom__*` is an internal transport artifact, not a public contract.
- `apply_patch` remains a public freeform tool on client-visible surfaces.
- Real-client public editing contracts preserve each client's public tool name: Codex `apply_patch`, Claude Code `Edit`, and Gemini `replace`.
- The intended translated-path bridge preserves the stable visible tool name and carries bridge provenance in request-scoped translation context.

The current system has the architectural seams for this work:

- `src/server/proxy.rs` resolves routing and computes runtime policy
- `src/translate/internal/assessment.rs` is the compatibility policy gate
- `src/translate/internal/tools.rs` already contains the custom-tool bridge machinery
- `src/server/responses_resources.rs` already protects native OpenAI Responses lifecycle ownership

## Why `client_profile` Is The Wrong Core Abstraction

The proxy already detects the client contract from request path and body shape:

- request format detection is protocol-first in `src/detect.rs`
- passthrough vs translation target selection is protocol-first in `src/discovery.rs`
- request execution is protocol-first in `src/server/proxy.rs`

So a core field like:

```text
client_profile = generic | codex | claude | gemini | auto
```

would mix two different concerns:

- protocol semantics
- wrapper and product-specific client behavior

That is too coarse for the data plane.

The correct core abstraction is:

- `compatibility_mode`
- `capability_surface`

where:

- `compatibility_mode` answers "how aggressive should compatibility shims be?"
- `capability_surface` answers "what client-visible contract should this local alias advertise and preserve?"

Client brand names can still exist in wrappers, real-client test matrix labels, and debugging, but they should not become the main policy axis inside the proxy.

## Product Direction

The product promise is bounded: protocol coverage means native passthrough for same-protocol paths and portable-core translation for mismatched paths, not full-fidelity provider equivalence.

- same-protocol paths: native passthrough within proxy routing, auth, and observability boundaries
- translated paths: `portable core` plus explicit compatibility behavior
- provider-native state and native extensions: same-provider only unless a documented shim exists

Portable core:

- text
- first-phase typed media input only when both request policy and the effective model surface allow the media kind
- system instructions
- function tools
- portable tool results
- usage and basic terminal reasons
- reasoning summaries and safe reasoning carriers

Native extensions:

- OpenAI hosted tools and Responses lifecycle state
- Anthropic server tools and pause-turn semantics
- Gemini built-in tools, caches, and interaction-specific state
- provider-owned conversation or compaction resources

## Multimodal Phase 1 Boundary

Multimodal support is currently a `max_compat` / request-policy protocol compatibility feature, not a blanket provider capability promise. The request policy gate recognizes typed media across OpenAI Chat/Responses, Anthropic Messages, and Gemini request shapes, then checks the effective `surface.modalities.input` for the routed alias. That surface value is a media-type gate only; source transport support is checked separately.

Current input modality meanings:

| Surface value | Compatibility meaning |
| --- | --- |
| `pdf` | Narrow document capability for PDF media. |
| `file` | Generic file capability and includes PDF. |
| `video` | First-phase gate for video media; Gemini video routed to non-Gemini targets must fail closed. |

Current translator boundaries:

| Path | First-phase behavior |
| --- | --- |
| OpenAI Chat/Responses images to Anthropic | Data URI images can become Anthropic base64 image blocks. HTTP(S) remote image URLs can become Anthropic `image.source.type=url`. |
| OpenAI Chat/Responses PDFs to Anthropic | PDF `file` / `input_file` data URIs can become Anthropic `document.source.type=base64`. PDF `file_data` / `file_url` HTTP(S) URLs can become Anthropic `document.source.type=url` when PDF MIME or filename provenance is available and self-consistent. |
| OpenAI Chat/Responses unsupported media to Anthropic | `input_audio`, non-PDF or generic files, unknown typed parts, provider `file_id`, and provider-native or local URIs such as `gs://`, `file://`, or `s3://` fail closed before contacting upstream. |
| Anthropic remote images to Gemini | Anthropic image blocks using remote URL sources fail closed unless a future explicit fetch/upload adapter is documented. |
| Gemini to OpenAI Chat/Responses | Gemini `inlineData` image, audio, and PDF content remains supported when the effective surface allows it. All Gemini `fileData.fileUri` sources, including HTTP(S), currently fail closed until an explicit fetch/upload adapter exists. |
| OpenAI Chat/Responses to Gemini | OpenAI-supplied file URI or HTTP(S) file references can map to Gemini `fileData` when MIME provenance is available and self-consistent. |
| Gemini video to non-Gemini | Fail closed before contacting upstream. |

Provider/model availability still comes from configuration. Do not mark a live upstream as multimodal unless that provider integration and selected model are validated for the media shape. In particular, the live MiniMax test provider should remain text-only in first-party docs; current multimodal e2e coverage uses first-party mock upstreams rather than real MiniMax.

Unsupported media and unsupported source transports are hard boundaries. HTTP(S) URLs are distinct from provider-native or local URIs: an HTTP(S) image or PDF URL may pass only on a path with an explicit target representation, while provider-owned identifiers and URIs such as `file_id`, `gs://`, `file://`, and `s3://` are not portable unless a documented adapter says otherwise. Unknown typed parts, media source forms that the target translator cannot represent, and media missing from the effective surface must be rejected before the upstream call instead of being silently dropped.

MIME provenance is part of that boundary. OpenAI Chat `file` and OpenAI Responses `input_file` parts may carry explicit `mime_type` / `mimeType`, MIME-bearing `file_data` data URIs, and filename-derived hints. The proxy treats disagreement between those sources as unsafe and rejects the request before translation, including same-format Responses passthrough. That prevents a request from passing a PDF-only surface gate while the translator later emits video, audio, image, or another concrete media type from the actual data URI.

## Recommended Runtime Policy

The runtime supports a namespace-level setting:

```yaml
compatibility_mode: max_compat
```

Available modes:

- `strict`: reject anything that is not native or safely portable
- `balanced`: keep current warning-and-drop behavior for safe degradations
- `max_compat`: prefer client usability for translated paths, while keeping hard semantic boundaries

Current default direction:

- passthrough paths remain native
- omitted configs default to `max_compat` for translated agent-facing deployments
- `strict` remains available for protocol verification, CI, and high-assurance integrations

Hard boundaries that should stay unchanged even in `max_compat`:

- the proxy does not reconstruct provider-owned state
- OpenAI Responses lifecycle resources remain native-only
- namespaced tool calls without safe portable form remain reject
- incomplete tool calls remain non-replayable

## Unified Capability Surface

Capability metadata now has a shared runtime source of truth instead of wrapper-only private defaults.

The proxy owns a unified `ModelSurface` schema and exposes one effective merged result per alias.

Current shape:

```yaml
model_aliases:
  minimax-openai:
    target: "MINIMAX:gpt-4.1-like"
    limits:
      context_window: 200000
      max_output_tokens: 128000
    surface:
      modalities:
        input: [text]
        output: [text]
      tools:
        supports_search: false
        supports_view_image: false
        apply_patch_transport: freeform
        supports_parallel_calls: false
```

Current `ModelSurface` support is limited to `limits`, `modalities`, and `tools`. Future surface fields for reasoning policy, session or compaction behavior, and transport capabilities are roadmap extensions, not current runtime contract.

This is resolved the same way `limits` are resolved:

- upstream defaults
- alias overrides
- effective merged surface at runtime

Implemented points:

- `surface_defaults` on upstream config
- `surface` on structured model aliases
- `effective_model_surface()` in config resolution
- compatibility mode and effective surface data carried into request policy
- `llmup.surface` exposed from `/openai/v1/models`, `/anthropic/v1/models`, and `/google/v1beta/models`

Wrappers consume the same effective surface and fail fast when live model profiles omit fields required for agent-client catalogs.

## Tool Identity Contract

Tool identity needs three separate concepts:

- `stable tool identity`
  - the name the client and the model should treat as the real tool name
  - examples: `apply_patch`, `code_exec`
- `transport encoding`
  - the wire representation used on a protocol hop that cannot natively express the source tool surface
- `internal provenance`
  - request-scoped metadata used by the proxy to decode bridged tool calls safely

For agent clients, stable tool identity is part of the semantic contract, not presentation detail.

That means:

- a local tool like `apply_patch` must stay `apply_patch` on all client-facing surfaces
- if the model is expected to reason about the available tool set, it must also see `apply_patch`, not a synthetic renamed tool
- synthetic transport names are acceptable only if they never become model-visible or client-visible

This creates a hard design consequence:

- encoding custom/freeform semantics by renaming the visible tool name is not acceptable on live translated paths for agent clients

## Legacy Prefix Bridge Failure Mode

The legacy prefix-based bridge rewrites:

- tool definition name
- tool choice name
- tool call name

into the reserved prefix form:

```text
__llmup_custom__<original_name>
```

with canonical bridged arguments:

```json
{ "input": "..." }
```

This works for reversible structured tool-call decoding, but it also changes the model-visible tool identity on the upstream request.

Historically, for `Codex -> OpenAI Responses -> OpenAI Chat Completions`:

- `run_codex_proxy.sh` launches Codex as a Responses client
- `responses_to_messages(..., UpstreamFormat::OpenAiCompletion)` enables `bridge_custom_responses_semantics`
- tool definitions are rewritten by `normalized_tool_definition_to_openai_with_custom_bridge(...)`
- tool choice is also rewritten to the prefixed name

That meant the upstream model was told that the tool was named `__llmup_custom__apply_patch`.

If the user asked the model "what tools do you have?", the model could truthfully answer with the synthetic prefixed name, even if later structured tool calls were decoded back to `apply_patch`.

That was a real bug:

- not because response decoding is absent
- but because request-side tool identity leaks into model-visible prompt/tool context

## `apply_patch` And Custom Tool Bridge

The existing name-based bridge is documented here only to explain why the public contract must reject or clear reserved-prefix tool names.

When an OpenAI Responses custom tool must be translated to Anthropic or OpenAI Chat style tool calling, the proxy bridges it as a synthetic function tool:

- reserved prefix: `__llmup_custom__`
- canonical bridged arguments: `{ "input": string }`

This is implemented in `src/translate/internal/tools.rs` and covered by translator tests, including the `apply_patch` grammar case.

This transport shape is useful only as internal translator machinery for structured tool-call decoding. Visible prefix-based naming is never a valid live model-visible or client-visible contract for agent clients.

The key correction was:

- the problem is not only "decode before returning to the client"
- the problem starts earlier, because the renamed tool definition is already visible to the upstream model

So the live fix could not be "rename and hide later".

The translated-path bridge contract is:

- keep the original stable tool name visible to the upstream model
- move custom/freeform bridge provenance into request-scoped translation context
- decode upstream function tool calls back to custom/freeform using that context

Current live bridge behavior:

- do not rename `apply_patch` to `__llmup_custom__apply_patch` on live translated request paths
- keep the visible upstream tool name as `apply_patch`
- continue using the canonical object wrapper `{ "input": string }` on function-only protocol hops
- use request-scoped `ToolBridgeContext` so response and streaming translators know that `apply_patch` on this request is a bridged custom/freeform tool, not an ordinary function tool
- reserve prefix-based bridge names for internal-only transport bookkeeping; public request and response paths must reject or clear them

Current behavior by policy:

- `strict`: if custom/freeform bridge would require changing the model-visible stable tool name, reject
- `balanced`: allow bridged custom/freeform transport only when stable tool name remains unchanged and replay safety is preserved
- `max_compat`: prefer bridged transport with stable tool identity preservation, warning when grammar or format constraints degrade on the target protocol

`apply_patch` specifically should remain advertised to Codex as `freeform` in the client-visible surface, while the upstream transport bridge stays internal.

## Request-Scoped Tool Bridge Context

The intended translated-path bridge preserves the stable visible tool name and carries bridge provenance in request-scoped translation context.

To preserve reversible decoding without exposing reserved prefixes, the live runtime carries a per-request bridge context.

Current conceptual shape:

```text
ToolBridgeContext
  stable_name -> {
    source_kind: custom_freeform | custom_grammar | function
    transport_kind: function_object_wrapper
    wrapper_field: "input"
    expected_canonical_shape: single_required_string
  }
```

The context is created during request translation and passed to:

- non-stream response translation
- stream translation
- any post-translation tool-result reconciliation

This lets the proxy:

- keep visible tool names stable in requests
- still decode returned `function` tool calls into `custom_tool_call`
- avoid relying on a visible reserved-name prefix to recover semantics

Additional rule:

- if one request contains both a function tool and a custom/freeform tool with the same stable name, reject the request as ambiguous

## Current Rollout State

Delivered:

- `compatibility_mode` and `ModelSurface` are in config and runtime.
- model catalog endpoints expose effective `llmup.surface` data.
- wrappers consume live/effective surface metadata instead of relying only on legacy client-specific defaults.
- live translated custom/freeform tool paths preserve stable names such as `apply_patch` and keep `__llmup_custom__*` internal.
- strict safety tests, compatibility-mode policy tests, model-surface projection tests, and focused real-client matrix checks cover the public editing tool identity contract: Codex `apply_patch`, Claude Code `Edit`, and Gemini `replace`.

Remaining work:

- broaden real-client coverage beyond the current public tool enumeration and supported workspace-edit lanes.
- extend `ModelSurface` only after the runtime supports additional reasoning, session, or transport fields.
- keep protocol baseline docs aligned with strict vs `max_compat` behavior for tools, state continuity, and streaming.
- expand arbitrary structured-tool behavior coverage without relaxing the visible tool identity contract.

## Non-Goals

- inventing cross-provider lifecycle state
- promising lossless translation for hosted or server-native tools
- hiding all degradation from the user
- turning wrapper-specific client behavior into the core routing identity
