# LLM Universal Proxy

A **single-binary** HTTP proxy that exposes one service URL and supports **four** LLM API formats on both the client and upstream sides. Clients can call in any of the four formats; the proxy forwards to a configured upstream. The proxy **discovers** which formats the upstream supports; when the client uses a format the upstream supports, it **passthroughs** (no translation) to reduce errors and improve efficiency. When the client format is not supported, the proxy **translates** to the default (most generic) upstream format. **Concurrent** requests are supported (async, non-blocking). Streaming is supported in all cases.

## Formats

| Format | Description | Typical path / shape |
|--------|-------------|------------------------|
| **Google** | Gemini-style | `contents[]`, parts |
| **Anthropic** | Claude | `/v1/messages`, `messages[]` + content blocks, `system` |
| **OpenAI completion** | Chat Completions | `/v1/chat/completions`, `messages[]`, `stream` |
| **OpenAI responses** | Responses API | `/v1/responses`, `input[]`, `instructions` |

## Requirements (summary)

- **Single binary** — `cargo build --release` → one executable.
- **Single service URL** — One listen address; one logical endpoint (e.g. `/v1/chat/completions` and `/v1/responses` both accepted).
- **Client** can send requests in **any** of the four formats (detected from path + body).
- **Upstream** is one base URL; the proxy **auto-discovers** which formats the upstream supports (or use `UPSTREAM_FORMAT` to fix one format and skip discovery).
- **Passthrough** — If the client’s format is supported by the upstream: no translation, forward request and response in that format (reduces errors and improves efficiency).
- **Streaming** — Must support streaming (SSE); when translating, convert upstream SSE chunks to client format.
- **Preserve behavior** — Minimize information loss when translating (tool calls, reasoning, usage, etc.).

## Reference

Design and conversion logic follow the **9router** reference project:

- `for-reference-only/9router/open-sse/translator/` — request/response translation, pivot via OpenAI.
- `for-reference-only/9router/open-sse/services/provider.js` — format detection.
- `for-reference-only/9router/open-sse/handlers/chatCore/` — streaming and non-streaming handling.

See [docs/DESIGN.md](docs/DESIGN.md) for the full design.

## Build and run

From a normal terminal:

```bash
cargo build --release
./target/release/llm-universal-proxy
```

From **Cursor IDE’s integrated terminal**, Cursor can set `RUSTC_WRAPPER` (or similar) so that rustup reports “unknown proxy name: 'cursor'”. Use the Makefile so cargo is run with a workaround:

```bash
make build
make test
./target/release/llm-universal-proxy
```

## Testing and reports

- **Run all tests:** `make test` (or `cargo test` with proxy env unset).
- **Run tests and generate report:** `make test-report`
  - Runs all tests with `--no-fail-fast`, writes logs and reports to `test-reports/`.
  - **Markdown:** `test-reports/report-latest.md` — summary, pass/fail counts, failed test names, log tail.
  - **JSON:** `test-reports/report-latest.json` — machine-readable (timestamp, success, passed, failed, total, failed_tests).
  - **Log:** `test-reports/test-latest.log` — full `cargo test` output.
- **Coverage:** 48 unit tests (config, detect, discovery, formats, streaming, translate), 6 detect integration tests, 18 proxy+mock integration tests (passthrough, translation, streaming, errors, health).

The Makefile uses `env -u RUSTC_WRAPPER` and prefers `$HOME/.cargo/bin/cargo` so that `make test` and `make check` work inside Cursor. See [Cursor forum](https://forum.cursor.com/t/rust-linux-error-unknown-proxy-name/19342).

Config via environment:

- `LISTEN` — Listen address (default `0.0.0.0:8080`)
- `UPSTREAM_URL` — Upstream base URL (default `https://api.openai.com/v1`)
- `UPSTREAM_FORMAT` — Optional; if set, use this format only (skip discovery). One of: `google` | `anthropic` | `openai-completion` | `openai-responses`
- `UPSTREAM_TIMEOUT_SECS` — Optional; request timeout in seconds (default 120)

Endpoints:

- `POST /v1/chat/completions` — Chat completions (and any of the four body formats)
- `POST /v1/responses` — OpenAI Responses API
- `GET /health` — Health check (returns `{"status":"ok"}`). CORS is enabled for all routes.

## Tests (TDD)

```bash
cargo test
```

Tests cover:

- Format detection (path + body).
- Request translation (client → upstream).
- Response translation (upstream → client).
- Streaming passthrough and translation (as we implement).

## License

MIT.
