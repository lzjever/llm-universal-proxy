# GA Readiness Review

- Status: baseline review
- Review date: 2026-04-25
- Release recommendation: do not GA yet
- Current posture: Beta / RC candidate

## Executive Summary

The product direction is clear and the core proxy shape is strong, but the
current implementation should not be declared Generally Available yet. The
main gap is not a single missing endpoint; it is the distance between the
documentation promises, protocol fidelity, security defaults, performance
boundaries, and release validation gates.

The project is closer to a Beta or RC posture: suitable for focused testing,
compatibility hardening, and controlled rollout, but not yet ready for a broad
GA claim.

## What Is Already Ready

- Unified proxy surfaces for OpenAI-, Anthropic-, Gemini-, and MiniMax-style
  interfaces.
- Stable model aliases and namespace endpoints.
- A portable-core philosophy with fail-closed behavior and explicit warning
  signaling when behavior is degraded.
- Multi-protocol mapping work across the major provider families.
- Early CI coverage and test matrix structure.

These are meaningful foundations for GA. The remaining work should preserve
this direction while tightening the places where the proxy can currently
overpromise, silently degrade, or expose unsafe defaults.

## GA Blockers

| Area | Finding | GA expectation |
| --- | --- | --- |
| Secrets | The tracked CLI matrix fixture now uses `credential_env` instead of inline provider credentials, and governance scans tracked fixtures, docs, examples, and scripts for provider key patterns or non-dummy `credential_actual` values. | Rotate any potentially exposed credential and keep secret scanning mandatory in CI/release. |
| Data-plane security | The data plane currently has no required auth boundary and allows permissive CORS behavior. | Require data-plane auth for production use and replace broad CORS with an explicit allowlist. |
| Server credential forwarding | `force_server` can cause server-held provider credentials to be injected into upstream requests. | Make the behavior explicit, gated, auditable, and safe by default. |
| Protocol fidelity | Public protocol claims are broader than the implemented fidelity for several provider-specific semantics. | Narrow the claim or implement/reject high-risk fields explicitly. |
| Provider semantics | Some provider-specific fields are dropped, weakened, or approximated in ways that can change meaning. | Preserve semantics on same-provider paths and fail closed when loss is high risk. |
| Memory and streaming bounds | SSE and response-body handling lack clear size and time boundaries. | Add hard request, response, stream, hook, and debug-trace limits. |
| Real-provider validation | The protected real provider smoke gate now blocks release artifacts, but real upstream validation is not yet broad enough to support a GA compatibility claim. | Broaden the real provider matrix while keeping the protected smoke gate mandatory for releases. |

## Protocol Gaps

### OpenAI Responses

OpenAI Responses support is not yet complete enough for a full lifecycle
compatibility claim. The GA line should account for create, retrieve, cancel,
delete, compact, input-item listing, streaming, conversation continuity, and
state-related behavior. Unsupported lifecycle or state features should be
documented as explicit boundaries and should fail clearly when they cannot be
routed or preserved.

High-risk Responses fields and behaviors should not be silently dropped. When
the proxy cannot safely preserve a field, it should reject the request or emit a
clear compatibility warning only for low-risk degradation.

### Anthropic Messages

Anthropic extended thinking, redacted thinking, and richer content-block
semantics are not fully preserved across provider mappings. These are not just
decorative fields: they can affect model-visible reasoning continuity, audit
behavior, and client expectations.

GA should require either faithful same-provider pass-through or explicit
fail-closed behavior for fields whose loss would change semantics.

### Gemini GenerateContent

Gemini-specific behavior such as `thoughtSignature`, `cachedContent`,
`safetySettings`, function-calling details, and related typed content semantics
are not yet fully round-tripped or preserved. These fields should be treated as
high risk unless the target provider path has a documented, tested equivalent.

For GA, Gemini fields that affect safety, cache state, tool replay, or thought
continuity should either be preserved or rejected before the upstream request is
made.

## Documentation And Contract Gaps

- The dashboard/admin boundary is documented as one admin-token boundary, including dashboard shell/admin actions.
- CLI wrapper documentation now describes safe defaults and requires `--dangerous-harness` for high-risk bypass or `yolo` style parameters.
- Performance targets are not backed by benchmark gates.
- Release workflow now makes public release artifacts depend on Rust tests,
  Python contract tests, governance and local secret scan, mock endpoint matrix,
  CLI wrapper matrix, perf gate, protected real provider smoke, container smoke,
  and supply-chain gates.
- Compatibility docs should distinguish broad OpenAI-compatible forwarding from
  provider-certified behavior.

## Recommended GA Gates

The current GA release gates are intentionally split between deterministic
local checks and protected release-environment checks. The mock endpoint matrix
and perf gate run only against a local mock upstream. The real provider smoke
gate runs only from the `release-real-providers` GitHub environment and fails
closed when the protected `GLM_APIKEY` secret is absent.

### Security

- Rotate any potentially exposed provider credential.
- Keep secret scanning in CI and pre-release checks.
- Require production data-plane auth.
- Replace permissive CORS with a configured allowlist.
- Audit and gate any `force_server` path that injects server-held credentials.

### Resource Boundaries

- Add request body size limits.
- Add response body size limits.
- Add SSE frame, event, stream duration, and idle timeout limits.
- Add hook execution size and time limits.
- Add debug trace retention and payload size limits.
- Ensure all limit failures produce predictable client errors and telemetry.

### Protocol Safety

- Reject high-risk unsupported fields instead of silently dropping them.
- Preserve provider-native fields on same-provider routes.
- Emit compatibility warnings only for low-risk, documented degradation.
- Maintain a high-risk field matrix for OpenAI Responses, Anthropic Messages,
  and Gemini GenerateContent.

### Provider Validation

- Add a real-provider matrix for OpenAI, Anthropic, Gemini, and MiniMax routes.
- Cover non-streaming and streaming paths.
- Cover tool/function calling where supported.
- Cover state, cache, reasoning, safety, and content-block edge cases where the
  provider exposes them.
- Keep fixture credentials out of source control and inject them only through
  secret-managed CI/runtime configuration.

### Release Workflow

GA release gating should include:

- Rust unit, integration, and contract tests.
- Python SDK/contract tests.
- Deterministic mock endpoint matrix over OpenAI Chat, OpenAI Responses,
  Anthropic Messages, and Gemini GenerateContent unary, stream, tool, and error
  paths.
- Real provider smoke tests from the protected `release-real-providers`
  environment.
- CLI wrapper matrix structure check.
- Deterministic local perf gate with machine-readable JSON output and threshold
  checks.
- Container image smoke tests.
- Security, secret, and supply-chain scans.
- Documentation consistency check for admin/data-plane boundaries and protocol
  compatibility claims.

## Baseline GA Definition

GA should mean that a production operator can safely deploy the proxy with
documented defaults, predictable failure modes, bounded resource usage, and
release artifacts that have been validated against both local contracts and real
provider behavior.

Until those gates are met, the recommended label is Beta or RC, with the
current findings tracked as release blockers.

## Official References

- OpenAI Responses: <https://platform.openai.com/docs/api-reference/responses>
- OpenAI Conversations: <https://platform.openai.com/docs/api-reference/conversations/create-item>
- Anthropic extended thinking: <https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking>
- Anthropic streaming: <https://docs.anthropic.com/en/api/streaming>
- Gemini GenerateContent: <https://ai.google.dev/api/generate-content>
- Gemini thought signatures: <https://ai.google.dev/gemini-api/docs/thought-signatures>
- Gemini function calling: <https://ai.google.dev/gemini-api/docs/function-calling>
