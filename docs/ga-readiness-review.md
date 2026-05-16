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
is single maximum safe compatibility with hard portability boundaries:
supported mappings are documented, high-risk unsupported fields fail before
upstream calls, and low-risk degradation must be visible rather than silent.
Raw/native passthrough is an intended pre-GA execution lane for preserving
native fields and lifecycle resources when the route can avoid body mutation and
response normalization; until that lane lands, same-protocol traffic may still
pass through compatibility machinery.

## Completed Local Baseline

- Admin and data-plane boundaries are documented and covered by local
  governance checks.
- Provider/model/resource routes require `LLM_UNIVERSAL_PROXY_AUTH_MODE` for
  non-loopback production use when static `data_auth` is omitted. Static
  `data_auth` is the preferred process-wide config: in `proxy_key` mode clients
  authenticate with the configured proxy key, commonly
  `LLM_UNIVERSAL_PROXY_KEY` through `proxy_key.env`, and upstream credentials
  come from `provider_key.env`, `provider_key.inline`, or legacy
  `provider_key_env`; in `client_provider_key` mode clients send provider keys
  directly and `provider_key.inline` is rejected. Admin routes remain behind the
  admin-token boundary, including `/admin/data-auth`.
- CORS is opt-in by exact origin rather than broadly emitted by default.
- Server-held provider-key forwarding is explicit through configured
  `provider_key.env`, `provider_key.inline`, or `provider_key_env` in
  `proxy_key` mode, and admin reads redact inline values.
- Local limit work is represented in the gate set and compatibility contracts:
  request, response, stream, hook, and trace paths must fail predictably when
  they exceed supported bounds.
- GA release gates now cover Rust tests, Python contract tests, governance and
  local secret scan, mock endpoint matrix, CLI wrapper matrix plus a hermetic
  scripted interactive Codex wrapper gate, perf gate, a protected compatible
  provider smoke slot, container smoke, and supply-chain checks.

## Remaining External Prerequisites

| Area | Required before final GA release | Non-claim until complete |
| --- | --- | --- |
| Release environment wiring | Configure the protected `release-compatible-provider` environment for a provider-neutral compatible smoke. If one compatible provider exposes both required surfaces, use `COMPAT_PROVIDER_API_KEY`; if the surfaces use separate credentials, use `COMPAT_OPENAI_API_KEY` and `COMPAT_ANTHROPIC_API_KEY`. In both cases set `COMPAT_OPENAI_BASE_URL`, `COMPAT_OPENAI_MODEL`, `COMPAT_ANTHROPIC_BASE_URL`, and `COMPAT_ANTHROPIC_MODEL`; `COMPAT_PROVIDER_LABEL` is optional. | Do not require MiniMax, GLM, or any fixed provider credential set for the GA gate. |
| Compatible provider run | Execute the protected provider-neutral compatible live smoke from the protected `release-compatible-provider` environment and retain the uploaded `artifacts/compatible-provider-smoke.json` GitHub Actions artifact with the external release evidence. It is not a GitHub Release asset in the current workflow. The required live coverage is the OpenAI-compatible chat-completions route `/openai/v1/chat/completions` plus the Anthropic-compatible messages route `/anthropic/v1/messages`. | Do not call the release provider-certified or fully cross-provider certified from local mocks alone. |
| External credential rotation | Rotate any credential that may have existed outside the secret manager and record the operator-side rotation evidence. | Do not claim external credential rotation has already been completed by repository changes. |

## Compatibility Boundaries

### OpenAI Responses

OpenAI Responses lifecycle and state resource endpoints target raw/native passthrough only when the lane is implemented and the route can avoid mutation.
Cross-provider reconstruction of provider-managed state,
conversation continuity, `context_management`, compact resources, or opaque
lifecycle resources must fail closed unless a future mapping is explicitly
designed and tested.

Request-side opaque reasoning and compaction input items follow the single
maximum safe compatibility strategy: opaque carriers such as
`encrypted_content` may be warned and dropped only when visible summary text or
visible transcript history remains. Opaque-only reasoning or compaction state
always fails closed, and native Responses passthrough should preserve the native
item unchanged when the raw/native lane is available.

### Anthropic Messages

Anthropic extended thinking, redacted thinking, and provider-signature behavior
are native semantics. They should be preserved on raw/native routes that avoid
mutation and rejected on cross-provider routes when the target cannot
faithfully carry them.

### Google OpenAI-Compatible Gemini

Gemini models remain in scope only through Google's OpenAI-compatible endpoint.
Native Gemini `generateContent` state such as `thoughtSignature`,
`cachedContent`, and `safetySettings` is retired from the active proxy surface.

### Compatible Provider Lane

MiniMax is only an example of an OpenAI-compatible lane chosen by a user, not a
GA-required provider and not an OpenAI Responses certified clone. Release smoke
evidence should prefer provider-neutral `COMPAT_*` configuration and prove the
OpenAI-compatible chat-completions route `/openai/v1/chat/completions` and the
Anthropic-compatible messages route `/anthropic/v1/messages` without naming a
specific provider as the GA requirement.

## GA Release Gates

The GA release gates are split between deterministic local checks and protected
release-environment checks. The mock endpoint matrix and perf gate run against
local mock upstreams. The compatible provider smoke gate runs only from the
protected `release-compatible-provider` GitHub environment, uses provider-neutral
`COMPAT_*` configuration, and uploads
`artifacts/compatible-provider-smoke.json` as a GitHub Actions artifact for
external release evidence. It is not a GitHub Release asset unless the workflow
is changed to attach it to the release.

GA release gating includes:

- Rust unit, integration, and contract tests.
- Python SDK/contract tests.
- Deterministic mock endpoint matrix over OpenAI Chat, OpenAI Responses, and
  Anthropic Messages unary, stream, tool, and error paths.
- CLI wrapper matrix structure check plus a hermetic scripted interactive Codex wrapper gate.
- Deterministic local perf gate with machine-readable JSON output and threshold
  checks.
- Compatible provider smoke tests from the protected `release-compatible-provider`
  environment, covering the OpenAI-compatible chat-completions route
  `/openai/v1/chat/completions` and the Anthropic-compatible messages route
  `/anthropic/v1/messages`.
- Container image smoke tests.
- Security, secret, and supply-chain scans.
- Documentation consistency checks for admin/data-plane boundaries and protocol
  compatibility claims.

The CLI wrapper gate is not a full live multi-client/provider matrix; final
real live client evidence remains GA/operator validation. Official OpenAI
Responses live smoke and broader compatible-provider smoke are optional extended
evidence. They can strengthen a release record, but they do not block
portable-core GA when the provider-neutral compatible live smoke and
deterministic contract/mock/structure gates pass.

## Baseline GA Definition

GA means a production operator can deploy the portable core with documented
defaults, predictable failure modes, bounded resource usage, secret-managed
provider credentials, and release artifacts validated by both local contracts
and the protected provider-neutral compatible live smoke.

It does not mean every provider-specific feature is equivalent across every
target. The promise is maximum safe compatibility with hard fail-closed
boundaries, plus raw/native passthrough as the intended execution lane when a
route can avoid mutation and normalization.

## Official References

- OpenAI Responses: <https://platform.openai.com/docs/api-reference/responses>
- OpenAI Conversations: <https://platform.openai.com/docs/api-reference/conversations/create-item>
- Anthropic extended thinking: <https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking>
- Anthropic streaming: <https://docs.anthropic.com/en/api/streaming>
- Google OpenAI-compatible Gemini: <https://ai.google.dev/gemini-api/docs/openai>
