# Maximum Compatibility Development Plan

Status: active  
Last updated: 2026-04-23

## Goal

Keep translated agent-facing paths on `max_compat` while enforcing two hard rules:

- the proxy must not rewrite the visible tool name supplied by the client
- the proxy must not reconstruct provider-owned lifecycle state

Locked contract:

- The proxy must not rewrite the visible tool name supplied by the client.
- `__llmup_custom__*` is an internal transport artifact, not a public contract.
- `apply_patch` remains a public freeform tool on client-visible surfaces.
- Real-client public editing contracts preserve each client's public tool name: Codex `apply_patch`, Claude Code `Edit`, and Gemini `replace`; the proxy does not rewrite them to a shared proxy name.

Phase 0 and Phase 1 together define the intended translated-path bridge: preserve the stable visible tool name on live requests and carry bridge provenance in request-scoped translation context.

## Current Baseline

- `compatibility_mode` exists in config and defaults omitted configs to `max_compat`.
- `surface_defaults`, alias `surface`, and `effective_model_surface()` exist as the shared model-surface truth chain.
- model catalog endpoints expose effective `llmup.surface` metadata for wrappers and clients.
- wrappers consume live/effective surface metadata and fail fast when critical agent-client fields are missing.
- supported live custom/freeform bridge paths preserve visible tool names such as `apply_patch` and keep `__llmup_custom__*` internal.
- Responses lifecycle resources remain native-only; the proxy does not invent provider-owned response/session state.

## Delivered Phases

### Phase 0: Reserved Prefix Enforcement

Status: delivered for supported live request/response paths.

Current contract:

- reserved names such as `__llmup_custom__apply_patch` must not appear on public client-visible or model-visible surfaces
- translated live requests preserve stable tool names in public `tools` and `tool_choice`
- real-client smoke coverage asserts the per-client public editing tool names: Codex `apply_patch`, Claude Code `Edit`, and Gemini `replace`, while omitting reserved prefixes

### Phase 1: Request-Scoped Tool Bridge Context

Status: delivered for the supported custom/freeform bridge paths.

Current contract:

- live translation carries bridge provenance in request-scoped context instead of recovering semantics from visible reserved prefixes
- structured function-only protocol hops can still decode back to native custom/freeform semantics
- ambiguous same-name function/custom definitions reject clearly

### Phase 2: Compatibility Mode Plumbing

Status: delivered.

Current contract:

- `strict`, `balanced`, and `max_compat` are explicit policy modes
- the same request can be evaluated differently by mode
- visible tool identity is enforced in all modes
- `max_compat` may warn and bridge portable semantics, but it still rejects provider-state reconstruction and unsafe non-portable shapes

### Phase 3: Unified Capability Surface

Status: delivered.

Current contract:

- upstream `surface_defaults` and alias `surface` merge into one effective model surface
- `/openai/v1/models`, `/anthropic/v1/models`, and `/google/v1beta/models` expose `llmup.surface`
- `apply_patch_transport` remains an internal transport description; the public Codex catalog still advertises `apply_patch` as `freeform`

### Phase 4: Wrapper Alignment

Status: delivered for the current Codex, Claude Code, and Gemini wrappers.

Current contract:

- wrapper-generated metadata follows the effective surface truth chain
- live model profile lookup fails fast if required `llmup.surface` fields are absent
- wrapper launch mechanics may remain client-specific, but model capability truth should not fork into private brand-specific defaults

### Phase 5: Documentation Rollout

Status: in progress.

Current contract:

- core docs must describe compatibility as portable core plus native-extension boundaries
- README quickstart examples must include enough surface metadata for wrapper live-profile flows
- docs must not reintroduce unbounded claims such as "drop-in replacement" or "any client to any backend" without same-paragraph portability, native-extension, warning, or reject boundaries

### Phase 6: Test Expansion

Status: partially delivered.

Current real-client matrix coverage is intentionally narrow: the public tool enumeration contract proves each client's stable public editing tool name is surfaced without proxy rewriting, and workspace-edit execution proves the edit path still works on supported lanes. It is not yet a full behavioral matrix for arbitrary structured tool use.

Delivered coverage:

- translator and proxy tests for visible tool identity preservation
- compatibility-mode policy tests
- model-surface merge and catalog projection tests
- wrapper parsing/runtime-config tests for `surface_defaults` and alias `surface`
- live surface fail-fast tests for critical `llmup.surface.tools` fields
- CLI smoke verifier coverage that fails if public output omits the current client's public editing tool name, mentions another client's public editing tool name, or surfaces `__llmup_custom__*`

## Remaining Roadmap

1. Broaden structured-tool behavior coverage beyond the current public tool enumeration and supported workspace-edit lanes.
2. Keep protocol baseline docs aligned with strict vs `max_compat` behavior for tools, state continuity, and streaming.
3. Add more translated streaming regressions where bridge context, terminal events, and tool-call finalization interact.
4. Continue tightening docs/example contract tests when wrapper live-profile requirements evolve.
5. Preserve the fail-warn posture: translated paths should warn or reject non-portable native extensions rather than silently approximating them.

## Guardrails

- Do not make client brand names a core data-plane policy axis.
- Do not expose reserved bridge prefixes as public tool names.
- Do not reconstruct provider-owned state for Responses lifecycle, Anthropic pause-turn state, Gemini caches, or provider-managed compaction.
- Do not describe translated paths as full-fidelity provider equivalence; describe portable behavior and native-extension boundaries explicitly.
