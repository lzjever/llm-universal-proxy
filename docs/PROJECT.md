# Project institution

## Scope

- **In scope**: Single binary; one listen URL; **concurrent requests** (async, non-blocking); client can send any of 4 formats; **upstream format auto-discovery** (probe upstream, cache supported formats, default conversion target = most generic); **passthrough when client format is supported by upstream** (no translation; reduces errors and improves efficiency); translate only when client format not supported; streaming; request/response translation with minimal loss.
- **Out of scope (v0)**: Auth, multiple upstreams, usage tracking, combo/fallback.

## Crate layout

```
llm-universal-proxy/
├── Cargo.toml
├── src/
│   ├── main.rs          # Binary entry, runs server
│   ├── lib.rs           # Library root, re-exports
│   ├── config.rs        # Config and upstream format
│   ├── discovery.rs     # (optional) Upstream format discovery; supported set + default target
│   ├── detect.rs        # Client format detection (path + body)
│   ├── formats.rs       # UpstreamFormat enum
│   ├── translate.rs     # Request/response translation (pivot: OpenAI)
│   ├── streaming.rs     # SSE chunk translation and passthrough
│   └── server.rs        # Axum routes, handler, discovery, upstream call
├── tests/
│   ├── common/
│   │   ├── mod.rs
│   │   └── mock_upstream.rs   # Mock servers per protocol (OpenAI, Anthropic, Google, Responses)
│   ├── detect_test.rs        # Format detection
│   └── integration_test.rs   # Proxy + mock: passthrough, translation, streaming
└── docs/
    ├── DESIGN.md
    ├── PROJECT.md (this file)
    └── protocol-baselines/     # Official protocol reference baselines (source + date in each file)
        ├── README.md           # Index, source URLs, capture date
        ├── openai-chat-completions.md
        ├── openai-responses.md
        ├── anthropic-messages.md
        └── google-gemini.md
```

## TDD approach

1. **Detection**: Tests in `detect.rs` (unit) and `tests/detect_test.rs` (integration) — path and body → format.
2. **Discovery**: Tests for probe logic and default target selection (supported set → most generic); add in `discovery.rs` or `upstream.rs` and tests.
3. **Translation**: Tests for each direction (e.g. OpenAI ↔ Anthropic, OpenAI ↔ Responses, OpenAI ↔ Google) — request and response; add in `translate.rs` and `tests/translate_test.rs`.
4. **Streaming**: Tests for passthrough (bytes unchanged) and for chunk conversion (upstream chunk → client chunk); add in `streaming.rs` and tests.
5. **Server**: Integration test with mock upstream: POST with each format, assert forwarded request shape and response shape; test passthrough when client format is in supported set.

We write or extend tests first, then implement to pass.

## Integration tests and mock upstreams

- **Location**: `tests/common/mock_upstream.rs` (mock servers), `tests/integration_test.rs` (tests).
- **Mock protocols**: Each mock implements the **official** request/response shapes for one provider:
  - **OpenAI Chat Completions**: [platform.openai.com/docs/api-reference/chat](https://platform.openai.com/docs/api-reference/chat/create) — POST `/chat/completions`, non-streaming `ChatCompletion` JSON, streaming SSE with `data:` chunks and `[DONE]`.
  - **Anthropic Messages**: [docs.anthropic.com/en/api/messages](https://docs.anthropic.com/en/api/messages) — POST `/messages`, non-streaming `content` array (text blocks), streaming SSE with `message_start`, `content_block_*`, `message_delta`, `message_stop`.
  - **Google Gemini generateContent**: [ai.google.dev/api/rest/v1beta/models/generateContent](https://ai.google.dev/api/rest/v1beta/models/generateContent) — POST `/generateContent`, non-streaming `candidates` + `usageMetadata`, streaming SSE with `candidates[].content.parts`.
  - **OpenAI Responses API**: [platform.openai.com/docs/api-reference/responses-streaming](https://platform.openai.com/docs/api-reference/responses-streaming) — POST `/responses`, non-streaming `output` array, streaming SSE with `response.created`, `response.output_text.delta`, `response.completed`.
- **Coverage**: For each upstream format, tests exercise **passthrough** (client and upstream same format) and **translation** (client OpenAI → upstream Anthropic/Google/Responses and vice versa), both non-streaming and streaming where applicable. Health endpoint is tested. Additional tests cover errors (invalid JSON, empty body, upstream unreachable → 502, nonexistent path → 404).

## Test report script

- **Script:** `scripts/test-and-report.sh` — runs `cargo test --no-fail-fast` (with proxy env unset), writes full log and generates:
  - **Markdown report:** `test-reports/report-<timestamp>.md` and `test-reports/report-latest.md` (summary table, failed test names, log tail).
  - **JSON report:** `test-reports/report-<timestamp>.json` and `test-reports/report-latest.json` (timestamp, success, passed, failed, total, failed_tests).
- **Make target:** `make test-report` runs the script. Use for CI or local verification with a single report artifact.

## Rust version

- **Edition**: 2021 (works on stable without 2024).
- Prefer latest stable Rust; optional `rust-toolchain.toml` for 2024 when desired.

## Cargo mirror (tuna) and proxy

When using the Tsinghua tuna mirror (`mirrors.tuna.tsinghua.edu.cn`) for crates and you have a global git/http proxy set:

1. **Git (index fetch)** — disable proxy for the mirror host:
   ```bash
   git config --global http.https://mirrors.tuna.tsinghua.edu.cn.proxy ""
   git config --global https.https://mirrors.tuna.tsinghua.edu.cn.proxy ""
   ```

2. **Cargo (crate downloads)** — the Makefile unsets `http_proxy`, `HTTP_PROXY`, `https_proxy`, `HTTPS_PROXY`, `all_proxy`, `ALL_PROXY` when invoking `cargo`, so builds run without proxy. If you still see SSL errors when downloading crates, retry later or try a different network; the mirror or crate host can be flaky.

This applies to any repo that uses `replace-with = 'tuna'` and `registry = "https://mirrors.tuna.tsinghua.edu.cn/git/crates.io-index.git"` in `~/.cargo/config.toml`.
