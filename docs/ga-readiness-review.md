# GA Readiness Review

- Status: converged GA scope review
- Review date: 2026-04-25
- Release recommendation: portable-core production GA after external release prerequisites are completed
- Current posture: local gates, security defaults, limits, and release-gate structure are complete; protected provider-neutral compatible live-smoke evidence is still pending

## Executive Summary

The GA claim is intentionally narrow: portable-core production GA. That means
the proxy is suitable for production deployment when operators use the
documented secure defaults, bounded resource behavior, release gates, and
compatibility boundaries.

This is not a provider-certified compatibility claim. The compatibility promise
is same-provider native passthrough for native fields and lifecycle resources,
and cross-provider documented compatibility/fail-closed for portability:
supported mappings are documented, high-risk unsupported fields fail before
upstream calls, and low-risk degradation must be visible rather than silent.

## Completed Local Baseline

- Admin and data-plane boundaries are documented and covered by local
  governance checks.
- Data routes require the data-token boundary for non-loopback production use,
  while admin routes remain behind the admin-token boundary.
- CORS is opt-in by exact origin rather than broadly emitted by default.
- Server-held credential forwarding is explicit through configured
  `credential_env` and `auth_policy` behavior.
- Local limit work is represented in the gate set and compatibility contracts:
  request, response, stream, hook, and trace paths must fail predictably when
  they exceed supported bounds.
- GA release gates now cover Rust tests, Python contract tests, governance and
  local secret scan, mock endpoint matrix, CLI wrapper matrix, perf gate, a
  protected compatible provider smoke slot, container smoke, and supply-chain
  checks.

## Remaining External Prerequisites

| Area | Required before final GA release | Non-claim until complete |
| --- | --- | --- |
| Release environment wiring | Configure the protected `release-compatible-provider` environment for a provider-neutral compatible smoke. If one compatible provider exposes both required surfaces, use `COMPAT_PROVIDER_API_KEY`; if the surfaces use separate credentials, use `COMPAT_OPENAI_API_KEY` and `COMPAT_ANTHROPIC_API_KEY`. In both cases set `COMPAT_OPENAI_BASE_URL`, `COMPAT_OPENAI_MODEL`, `COMPAT_ANTHROPIC_BASE_URL`, and `COMPAT_ANTHROPIC_MODEL`; `COMPAT_PROVIDER_LABEL` is optional. | Do not require MiniMax, GLM, or a fixed four-provider credential set for the GA gate. |
| Compatible provider run | Execute the protected provider-neutral compatible live smoke and retain the uploaded `artifacts/compatible-provider-smoke.json` artifact with the release evidence. The required live coverage is the OpenAI-compatible completions/chat-completions surface plus the Anthropic-compatible messages surface. | Do not call the release provider-certified or fully cross-provider certified from local mocks alone. |
| External credential rotation | Rotate any credential that may have existed outside the secret manager and record the operator-side rotation evidence. | Do not claim external credential rotation has already been completed by repository changes. |

## Compatibility Boundaries

### OpenAI Responses

OpenAI Responses lifecycle and state resource endpoints are same-provider native
passthrough only. Cross-provider reconstruction of provider-managed state,
conversation continuity, encrypted reasoning, or opaque lifecycle resources
must fail closed unless a future mapping is explicitly designed and tested.

### Anthropic Messages

Anthropic extended thinking, redacted thinking, and provider-signature behavior
are native semantics. They are preserved on same-provider routes and rejected on
cross-provider routes when the target cannot faithfully carry them.

### Gemini GenerateContent

Gemini `thoughtSignature`, `cachedContent`, `safetySettings`, and similar
provider-managed fields remain high-risk semantics. Same-provider Gemini paths
preserve native fields; cross-provider paths fail closed when the proxy cannot
replay them safely.

### Compatible Provider Lane

MiniMax is only an example of an OpenAI-compatible lane chosen by a user, not a
GA-required provider and not an OpenAI Responses certified clone. Release smoke
evidence should prefer provider-neutral `COMPAT_*` configuration and prove the
compatible OpenAI completions/chat-completions and Anthropic messages surfaces
without naming a specific provider as the GA requirement.

## GA Release Gates

The GA release gates are split between deterministic local checks and protected
release-environment checks. The mock endpoint matrix and perf gate run against
local mock upstreams. The compatible provider smoke gate runs only from the
`release-compatible-provider` GitHub environment, uses provider-neutral
`COMPAT_*` configuration, and emits the
`artifacts/compatible-provider-smoke.json` artifact.

GA release gating includes:

- Rust unit, integration, and contract tests.
- Python SDK/contract tests.
- Deterministic mock endpoint matrix over OpenAI Chat, OpenAI Responses,
  Anthropic Messages, and Gemini GenerateContent unary, stream, tool, and error
  paths.
- CLI wrapper matrix structure check.
- Deterministic local perf gate with machine-readable JSON output and threshold
  checks.
- Compatible provider smoke tests from the protected `release-compatible-provider`
  environment, covering a compatible OpenAI completions/chat-completions surface
  and a compatible Anthropic messages surface.
- Container image smoke tests.
- Security, secret, and supply-chain scans.
- Documentation consistency checks for admin/data-plane boundaries and protocol
  compatibility claims.

Official OpenAI Responses live smoke, official Gemini live smoke, and broader
four-provider real smoke are optional extended evidence. They can strengthen a
release record, but they do not block portable-core GA when the provider-neutral
compatible live smoke and deterministic contract/mock/structure gates pass.

## Baseline GA Definition

GA means a production operator can deploy the portable core with documented
defaults, predictable failure modes, bounded resource usage, secret-managed
provider credentials, and release artifacts validated by both local contracts
and the protected provider-neutral compatible live smoke.

It does not mean every provider-specific feature is equivalent across every
target. The promises are same-provider native passthrough and cross-provider
documented compatibility/fail-closed.

## Official References

- OpenAI Responses: <https://platform.openai.com/docs/api-reference/responses>
- OpenAI Conversations: <https://platform.openai.com/docs/api-reference/conversations/create-item>
- Anthropic extended thinking: <https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking>
- Anthropic streaming: <https://docs.anthropic.com/en/api/streaming>
- Gemini GenerateContent: <https://ai.google.dev/api/generate-content>
- Gemini thought signatures: <https://ai.google.dev/gemini-api/docs/thought-signatures>
- Gemini function calling: <https://ai.google.dev/gemini-api/docs/function-calling>
