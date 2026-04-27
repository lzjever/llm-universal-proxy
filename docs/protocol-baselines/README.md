# Protocol Baselines Docs

- Status: active
- Vendor snapshot/captured date: 2026-04-16
- Proxy posture updated date: 2026-04-26
- Audience: proxy implementers, test authors, and maintainers refreshing protocol docs
- This file is intentionally only an entrypoint and update guide.

## What lives here

| Layer | Purpose | Primary docs |
| --- | --- | --- |
| Official baselines | Vendor contract and snapshot/source facts copied from official docs, kept per protocol; proxy posture may appear only when clearly labeled | `openai-responses.md`, `openai-chat-completions.md`, `anthropic-messages.md`, `google-gemini.md` |
| Capability diffs | Cross-provider comparison of official surface coverage, degradations, proxy policy, and proxy posture for features that do not map 1:1 | `overview.md`, `capabilities/`, `matrices/` |
| Versioned audits | Date-stamped refresh notes, change detection, and implementation risk calls | `audits/2026-04-16-spec-refresh.md` |

## Metadata model

Vendor snapshot/captured date records when the source snapshot or source facts were captured. It does not imply the vendor contract was refreshed on the proxy posture updated date.

Proxy posture updated date records when this repository's compatibility policy, downgrade rules, or implementation notes were last aligned. Proxy policy is not a vendor claim, and snapshot/source facts are not proxy policy.

## Reading order

| If you need to... | Start here | Then read |
| --- | --- | --- |
| Understand the doc set | [`overview.md`](overview.md) | The relevant capability note under [`capabilities/`](capabilities/) |
| Check vendor wire facts | The vendor baseline file | The matching matrix under [`matrices/`](matrices/) |
| Refresh docs after upstream changes | This README | The newest file under [`audits/`](audits/) |
| Get the shortest compatibility summary | [`../protocol-compatibility-matrix.md`](../protocol-compatibility-matrix.md) | [`matrices/provider-capability-matrix.md`](matrices/provider-capability-matrix.md) |

## Update rules

1. Keep official baseline files vendor-specific and factual. They may record both vendor contract facts and proxy posture, but label snapshot/source facts separately from proxy policy.
2. Put broad semantic differences, degradations, and proxy guidance in `capabilities/` or `matrices/`, not in vendor baselines.
3. In summary tables, keep provider-surface facts separate from portability judgments. Provider cells should answer "is this officially documented here?"; notes or mapping-status columns should answer "is it portable?"
4. Every refresh gets a new dated audit file under `audits/`. Do not silently overwrite an older audit summary.
5. When a vendor baseline changes meaningfully, update `overview.md` and the affected matrix files in the same change.
6. Treat `snapshots/` as immutable evidence for the capture that produced them. Add new snapshots in a separate refresh, never rewrite an old capture.
7. If a feature exists only in a guide, beta surface, or model-specific page, label it that way instead of presenting it as a universal protocol guarantee.
8. During a staged refresh, snapshot files may land before the matching vendor baseline text is rewritten. That is acceptable; keep the overview, matrices, and audit date-aware without assuming every layer has the same capture date mid-refresh.
