# Maximum Compatibility Development Plan

Status: active  
Last updated: 2026-04-20

## Goal

Move translated agent-facing paths toward `max_compat` while enforcing two hard rules:

- the proxy must not rewrite the visible tool name supplied by the client
- the proxy must not reconstruct provider-owned lifecycle state

Locked contract:

- The proxy must not rewrite the visible tool name supplied by the client.
- `__llmup_custom__*` is an internal transport artifact, not a public contract.
- `apply_patch` remains a public freeform tool on client-visible surfaces.

Phase 0 and Phase 1 together define the intended translated-path bridge: preserve the stable visible tool name on live requests and carry bridge provenance in request-scoped translation context.

## Phase 0: Reserved Prefix Enforcement

Goal:

- enforce that reserved names such as `__llmup_custom__apply_patch` are rejected or cleared before they reach any client-visible or model-visible surface

Work items:

- audit every live path where Responses custom tools can be renamed to `__llmup_custom__*`
- split legacy stateless bridge helpers from live request translation helpers
- make live `Responses -> OpenAI Completions` translation preserve the visible tool name
- add regression tests that fail whenever translated Codex + `minimax-openai` surfaces the reserved prefix publicly

Exit criteria:

- translated live upstream request no longer contains reserved-prefix tool names in public `tools` or `tool_choice`
- asking the model what tools are available no longer surfaces the reserved prefix

## Phase 1: Request-Scoped Tool Bridge Context

Goal:

- preserve stable tool identity while still allowing reversible bridge decoding on function-only protocol hops

Work items:

- extend request translation output to carry `ToolBridgeContext`
- thread that context through non-stream response translation
- thread that context through streaming translation
- define conflict rules for same-name function and custom/freeform tools

Exit criteria:

- structured function tool calls on translated paths decode back to native `custom_tool_call` or equivalent without visible tool renaming
- ambiguous same-name function/custom definitions reject clearly

## Phase 2: Compatibility Mode Plumbing

Goal:

- make `strict`, `balanced`, and `max_compat` explicit runtime behavior rather than implied translator behavior

Work items:

- add `compatibility_mode` to runtime config
- extend `RequestTranslationPolicy`
- make `assessment` mode-aware
- document allow/warn/reject behavior per mode

Exit criteria:

- the same request can be evaluated differently under `strict` and `max_compat`
- visible tool identity rule is enforced in all modes

## Phase 3: Unified Capability Surface

Goal:

- replace wrapper-only client metadata with a shared runtime truth source

Work items:

- add `surface_defaults` to upstream config
- add `surface` to alias config
- implement `effective_model_surface()`
- expose `llmup.surface` from model catalog endpoints

Exit criteria:

- wrappers and `/models` consume the same effective surface data
- Codex/Gemini metadata is no longer defined only in Python-side parsing logic

## Phase 4: Wrapper Alignment

Goal:

- remove split-brain between live proxy behavior and wrapper-generated client metadata

Work items:

- update `interactive_cli.py` and `real_cli_matrix.py` to consume unified surface data
- keep Codex catalog generation aligned with runtime surface
- keep Gemini settings generation aligned with runtime surface

Exit criteria:

- wrapper-generated metadata matches live proxy model metadata
- wrapper no longer needs private brand-specific truth beyond client launch mechanics

## Phase 5: Documentation Rollout

Goal:

- make the new compatibility contract explicit across project docs

Work items:

- update `README.md`
- update `docs/DESIGN.md`
- update `docs/PRD.md`
- update `docs/CONSTITUTION.md`
- update protocol baseline capability notes

Exit criteria:

- all core docs agree on portable core vs native extensions
- all core docs agree that visible tool names are immutable

## Phase 6: Test Expansion

Goal:

- prevent regressions in tool identity, compatibility mode behavior, and real-client compatibility

Work items:

- add translator tests for visible tool identity preservation
- add streaming tests for bridge decoding with request-scoped context
- add integration tests for translated live request shapes
- add real-client matrix coverage for tool enumeration and structured tool execution
- keep a runnable CLI smoke verifier that fails if public output surfaces `__llmup_custom__*`

Exit criteria:

- the prefixed bridge-name leak is covered by automated tests
- translated `apply_patch` remains usable on supported live paths

## Suggested Delivery Order

1. Phase 0
2. Phase 1
3. Phase 6 translator/integration coverage for the new bridge context
4. Phase 2
5. Phase 3
6. Phase 4
7. Phase 5 final doc sweep

## Suggested Milestones

- Milestone A: bug fixed, live tool names stable again
- Milestone B: request-scoped bridge context landed for non-stream and stream
- Milestone C: compatibility mode is explicit and testable
- Milestone D: unified capability surface powers wrappers and model catalogs
