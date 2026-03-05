# Protocol reference baselines

This directory holds **protocol reference baselines** for the four LLM API formats supported by the proxy. Each baseline is derived from the **official** provider documentation and is used as a reference for request/response shapes, streaming behavior, and field semantics.

**Important:** These baselines are snapshots for implementation and testing. Always refer to the official URLs below for the latest versions and full details.

| Protocol | Official source | Baseline file | Captured |
|----------|-----------------|---------------|----------|
| **OpenAI Chat Completions** | [platform.openai.com/docs/api-reference/chat](https://platform.openai.com/docs/api-reference/chat/create) | [openai-chat-completions.md](openai-chat-completions.md) | 2026-03-05 |
| **OpenAI Responses API** | [platform.openai.com/docs/api-reference/responses](https://platform.openai.com/docs/api-reference/responses), [streaming](https://platform.openai.com/docs/api-reference/responses-streaming) | [openai-responses.md](openai-responses.md) | 2026-03-05 |
| **Anthropic Messages (Claude)** | [docs.anthropic.com/en/api/messages](https://docs.anthropic.com/en/api/messages) | [anthropic-messages.md](anthropic-messages.md) | 2026-03-05 |
| **Google Gemini generateContent** | [ai.google.dev/gemini-api/docs](https://ai.google.dev/gemini-api/docs), [text-generation](https://ai.google.dev/gemini-api/docs/text-generation), [REST API](https://ai.google.dev/api/rest/v1beta/models/generateContent) | [google-gemini.md](google-gemini.md) | 2026-03-05 |

## Version and date

- **Capture date:** 2026-03-05  
- **Purpose:** Stable reference for proxy translation and mock servers.  
- **Updates:** When updating a baseline, note the new capture date and any API version (e.g. OpenAI API version, Anthropic `anthropic-version`) in the baseline file header.

## Usage

- **Implementation:** Use baselines to align request/response translation in `translate.rs` and `streaming.rs`.  
- **Mocks:** Use baselines to keep `tests/common/mock_upstream.rs` in line with official request/response and streaming formats.  
- **Changes:** If a provider changes their API, update the corresponding baseline and the proxy code, then re-run integration tests.
