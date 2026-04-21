# Maximum Compatibility Design

Status: active  
Last updated: 2026-04-20

## Summary

`llm-universal-proxy` should move toward a client-first maximum compatibility posture for translated paths, but it should stay protocol-first in the core architecture.

This means:

- do not model `Codex`, `Claude`, and `Gemini` as first-class data-plane identities
- do model a unified `capability surface` for each local model alias
- do make `max_compat` a real runtime policy mode
- do keep provider-owned state and provider-native lifecycle features native-only
- do treat transport-only bridge artifacts as internal machinery, not as user-facing contract

Locked contract:

- The proxy must not rewrite the visible tool name supplied by the client.
- `__llmup_custom__*` is an internal transport artifact, not a public contract.
- `apply_patch` remains a public freeform tool on client-visible surfaces.
- The intended translated-path bridge preserves the stable visible tool name and carries bridge provenance in request-scoped translation context.

The current system already has the right architectural seam for this work:

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

The product promise should be updated from "all 16 combinations work correctly" to a more precise contract:

- same-protocol paths: native passthrough, as faithful as possible
- translated paths: `portable core` plus explicit compatibility behavior
- provider-native state and native extensions: same-provider only unless a documented shim exists

Portable core:

- text
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

## Recommended Runtime Policy

Add a namespace-level and alias-level setting:

```yaml
compatibility_mode: max_compat
```

Recommended modes:

- `strict`: reject anything that is not native or lossless
- `balanced`: keep current warning-and-drop behavior for safe degradations
- `max_compat`: prefer client usability for translated paths, while keeping hard semantic boundaries

Recommended default direction:

- passthrough paths remain native
- translated paths should move toward `max_compat` by default for agent-facing deployments
- `strict` remains available for protocol verification, CI, and high-assurance integrations

Hard boundaries that should stay unchanged even in `max_compat`:

- the proxy does not reconstruct provider-owned state
- OpenAI Responses lifecycle resources remain native-only
- namespaced tool calls without safe portable form remain reject
- incomplete tool calls remain non-replayable

## Unified Capability Surface

Today, capability metadata is split across Rust runtime config and Python wrapper logic. That split should be removed.

The proxy should own a unified `ModelSurface` schema and expose one effective merged result per alias.

Recommended first-phase shape:

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
      tools:
        supports_search: false
        supports_view_image: false
        apply_patch_transport: freeform
        supports_parallel_calls: false
      reasoning:
        supported_levels: [low, medium, high, xhigh]
      session:
        compaction_mode: local_summary
      transport:
        supports_websockets: false
```

This should be resolved the same way `limits` are resolved today:

- upstream defaults
- alias overrides
- effective merged surface at runtime

Recommended implementation points:

- add `surface_defaults` to `UpstreamConfig`
- add `surface` to `ModelAlias`
- add `effective_model_surface()` to config resolution
- extend `RequestTranslationPolicy` to carry compatibility and surface data
- expose `proxec.surface` from `/openai/v1/models`, `/anthropic/v1/models`, and `/google/v1beta/models`

Wrappers should then consume the same effective surface instead of re-deriving private client metadata from source YAML.

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

For `Codex -> OpenAI Responses -> OpenAI Chat Completions`:

- `run_codex_proxy.sh` launches Codex as a Responses client
- `responses_to_messages(..., UpstreamFormat::OpenAiCompletion)` enables `bridge_custom_responses_semantics`
- tool definitions are rewritten by `normalized_tool_definition_to_openai_with_custom_bridge(...)`
- tool choice is also rewritten to the prefixed name

That means the upstream model is told that the tool is named `__llmup_custom__apply_patch`.

If the user asks the model "what tools do you have?", the model can truthfully answer with the synthetic prefixed name, even if later structured tool calls are decoded back to `apply_patch`.

So this is a real bug:

- not because response decoding is absent
- but because request-side tool identity leaks into model-visible prompt/tool context

## `apply_patch` And Custom Tool Bridge

The existing name-based bridge is documented here only to explain why the public contract must reject or clear reserved-prefix tool names.

When an OpenAI Responses custom tool must be translated to Anthropic or OpenAI Chat style tool calling, the proxy bridges it as a synthetic function tool:

- reserved prefix: `__llmup_custom__`
- canonical bridged arguments: `{ "input": string }`

This is implemented in `src/translate/internal/tools.rs` and covered by translator tests, including the `apply_patch` grammar case.

This transport shape is useful only as internal translator machinery for structured tool-call decoding. Visible prefix-based naming is never a valid live model-visible or client-visible contract for agent clients.

The key correction is:

- the problem is not only "decode before returning to the client"
- the problem starts earlier, because the renamed tool definition is already visible to the upstream model

So the live fix cannot be "rename and hide later".

The intended translated-path bridge is:

- keep the original stable tool name visible to the upstream model
- move custom/freeform bridge provenance into request-scoped translation context
- decode upstream function tool calls back to custom/freeform using that context

Recommended redesign:

- stop renaming `apply_patch` to `__llmup_custom__apply_patch` on live translated request paths
- keep the visible upstream tool name as `apply_patch`
- continue using the canonical object wrapper `{ "input": string }` on function-only protocol hops
- introduce request-scoped `ToolBridgeContext` so response and streaming translators know that `apply_patch` on this request is a bridged custom/freeform tool, not an ordinary function tool
- reserve prefix-based bridge names for internal-only transport bookkeeping; public request and response paths must reject or clear them

Recommended behavior by policy:

- `strict`: if custom/freeform bridge would require changing the model-visible stable tool name, reject
- `balanced`: allow bridged custom/freeform transport only when stable tool name remains unchanged and replay safety is preserved
- `max_compat`: prefer bridged transport with stable tool identity preservation, warning when grammar or format constraints degrade on the target protocol

`apply_patch` specifically should remain advertised to Codex as `freeform` in the client-visible surface, while the upstream transport bridge stays internal.

## Request-Scoped Tool Bridge Context

The intended translated-path bridge preserves the stable visible tool name and carries bridge provenance in request-scoped translation context.

To preserve reversible decoding without exposing reserved prefixes, the live runtime should carry a per-request bridge context.

Recommended first-phase shape:

```text
ToolBridgeContext
  stable_name -> {
    source_kind: custom_freeform | custom_grammar | function
    transport_kind: function_object_wrapper
    wrapper_field: "input"
    expected_canonical_shape: single_required_string
  }
```

The context should be created during request translation and passed to:

- non-stream response translation
- stream translation
- any post-translation tool-result reconciliation

This lets the proxy:

- keep visible tool names stable in requests
- still decode returned `function` tool calls into `custom_tool_call`
- avoid relying on a visible reserved-name prefix to recover semantics

Additional rule:

- if one request contains both a function tool and a custom/freeform tool with the same stable name, reject the request as ambiguous

## Documentation Changes

The following docs should be updated after the schema and policy are introduced:

- `README.md`
  - add `Compatibility Modes`
  - add `Capability Surface`
  - add `Custom Tool Bridge And apply_patch`
  - explain wrapper metadata as part of compatibility, not as optional helper logic
- `docs/DESIGN.md`
  - add compatibility subsystem
  - add capability surface truth model
  - add internal bridge artifact rules
- `docs/PRD.md`
  - replace unconditional any-to-any wording with `portable core + native extensions`
  - state that all translated combinations are supported within documented portability boundaries
- `docs/CONSTITUTION.md`
  - keep protocol-first design
  - keep visible degradation
  - explicitly forbid provider-state reconstruction
- `docs/protocol-baselines/*`
  - add `strict vs max_compat` notes for tools, state continuity, and streaming

## Test Plan

Keep the current strict safety tests.

Add these layers:

- capability-surface unit tests
  - merge rules
  - `/models` projection
  - wrapper consumption against the same truth source
- compatibility-mode policy tests
  - `strict`, `balanced`, `max_compat` decision tables
- custom-tool bridge tests
  - visible tool name stays `apply_patch` on translated live requests
  - upstream request tools and `tool_choice` do not contain `__llmup_custom__apply_patch` on live agent paths
  - structured tool calls still decode back to `custom_tool_call`
  - replay safety markers are re-attested after bridge rewrites
  - ambiguous same-name function/custom definitions reject
- real-client matrix tests
  - Codex catalog and proxy surface stay aligned
  - asking "what tools are available?" must not surface `__llmup_custom__apply_patch`
  - translated `apply_patch` remains usable without exposing internal bridge names
  - wrapper-derived metadata matches live proxy metadata

## Rollout Order

1. Introduce `compatibility_mode` and `ModelSurface` in config and runtime.
2. Extend `/models` to expose effective surface data.
3. Add request-scoped `ToolBridgeContext` to live runtime request/response/stream translation.
4. Stop visible custom-tool renaming on live agent-facing translated paths.
5. Refactor wrappers to consume the unified surface instead of private parsing rules.
6. Update README, DESIGN, PRD, CONSTITUTION, and protocol-baseline docs.
7. Expand tests before enabling `max_compat` as the common translated-path default.

## Non-Goals

- inventing cross-provider lifecycle state
- promising lossless translation for hosted or server-native tools
- hiding all degradation from the user
- turning wrapper-specific client behavior into the core routing identity
