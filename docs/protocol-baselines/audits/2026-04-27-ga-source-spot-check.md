# GA Source Spot-Check Audit - 2026-04-27

- Status: GA documentation spot-check
- Scope: targeted source spot-check for the current GA portability contract
- Snapshot posture: not a full recertification and not a full snapshot refresh
- Baseline posture: the 2026-04-16 snapshot bucket remains the captured baseline for vendor docs and manifests

## Summary

This audit records a narrow 2026-04-27 spot-check before GA docs signoff. It does
not replace the dated 2026-04-16 captured baseline, does not update snapshot
artifacts, and does not claim complete source revalidation. The check looked only
for obvious current-source changes that would affect the proxy portable-core
contract already documented in the protocol matrices.

Result: no obvious source change found in this spot-check requires changing the
current GA portability contract.

## Sources Checked

| Provider | Source | Spot-check read |
| --- | --- | --- |
| OpenAI | https://developers.openai.com/api/reference/resources/responses/methods/compact | Responses compact API reference still presents `/responses/compact` as a Responses-family state/compaction surface. |
| OpenAI | https://developers.openai.com/api/docs/guides/conversation-state | Conversation state docs still position Responses, Conversations, previous response chaining, and compaction as OpenAI state-continuity surfaces. |
| Google Gemini | https://ai.google.dev/api | Gemini API reference still lists standard `generateContent` and streaming `streamGenerateContent` as primary content-generation surfaces. |
| Anthropic | https://platform.claude.com/docs/en/release-notes/overview | Anthropic 2026-04-23/24 release notes were checked for platform changes that would affect the proxy portable-core contract. |

## Findings

OpenAI Responses, Conversations, and compact main surface checks did not show an
obvious change that alters the current GA contract. The existing proxy posture
still treats OpenAI state continuity, previous response IDs, Conversations, and
compaction as native/stateful surfaces that are portable only where explicitly
documented or preserved by raw/native passthrough.

Gemini generateContent remains the main REST content-generation surface for the
current GA docs. The spot-check did not identify a change that affects the proxy
portable-core treatment of Gemini request/response translation, streaming, or
model catalog boundaries.

Anthropic 2026-04-23/24 release notes covered the Rate Limits API and Managed
Agents memory. Managed Agents memory is a platform/agent feature, not a change
to the Anthropic Messages portable-core contract used by this proxy. These notes
do not change the current portable-core contract for message text, tool calls,
usage, finish reasons, streaming, or compatibility warnings.

## Decision

Keep the 2026-04-16 snapshot bucket as the captured baseline for GA. Do not
refresh snapshots in this change. The 2026-04-27 spot-check is a governance note
that the currently documented GA portability contract was not obviously affected
by the checked OpenAI, Gemini, or Anthropic source surfaces.

Future changes that alter a provider request/response schema, streaming
lifecycle, tool contract, state-continuity behavior, or portability warning
policy still require a normal dated refresh audit and, when needed, new captured
snapshot evidence.
