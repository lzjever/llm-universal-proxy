# LLM Universal Proxy

[中文文档](./README_CN.md)

A single-binary HTTP proxy that provides a unified interface for Large Language Model APIs. It accepts requests in multiple LLM API formats, routes models to named upstreams, and automatically handles format conversion when needed.

## Features

- **Multi-Format Support**: Accepts requests in 4 different LLM API formats:
  - Google Gemini
  - Anthropic Claude
  - OpenAI Chat Completions
  - OpenAI Responses API
- **Auto-Discovery**: Automatically detects which formats the upstream service supports
- **Smart Routing**: Passes through requests when client format matches upstream capabilities (no translation overhead)
- **Format Translation**: Seamlessly converts between formats when needed
- **Streaming Support**: Handles both streaming and non-streaming responses
- **Concurrent Requests**: Asynchronous handling for high performance
- **Named Upstreams**: Route requests to multiple upstream providers from one proxy instance
- **Local Model Aliases**: Expose one unique local model name for any upstream model
- **Audit Hooks**: Optional async `exchange` / `usage` HTTP hooks for request-response capture and metering
- **Credential Policy**: Supports fallback credentials, direct configured credentials, and force-server auth
- **Codex CLI Friendly**: Works as a Responses-compatible endpoint in front of Anthropic-compatible upstreams
- **Model Unification Layer**: Map models from different providers to one stable local naming scheme, such as `opus`, `sonnet`, `haiku`, or team-specific coding aliases

## Why It Is Useful

- **One stable model namespace across providers**: You can map models from different vendors into one local naming layer. For example, different upstream models can be exposed as stable names such as `opus`, `sonnet`, `haiku`, or any team-specific alias. That makes tools that assume fixed model names easier to operate.
- **Useful for Claude Code style workflows**: If you want Claude-style routing semantics but your real upstreams come from different vendors, the proxy can present a consistent set of local model names while routing to whichever provider you choose underneath.
- **Useful for modern Codex CLI**: Newer Codex CLI versions only speak the OpenAI Responses API. This proxy lets Codex use upstreams that speak Anthropic Messages, OpenAI Chat Completions, or other non-Responses-compatible APIs. That is especially useful when you want to use coding-capable providers such as GLM, MiniMax, or Kimi behind a Responses-only client.
- **Cross-provider protocol bridge**: You can place Anthropic-compatible, OpenAI-compatible, and Gemini-style upstreams behind one consistent interface instead of teaching each client multiple protocols.
- **Built-in observability for analysis**: `usage` hooks export metering data; `exchange` hooks export full client-facing query/response pairs. That makes it practical to persist production traffic for auditing, analytics, evaluation, or later model-training pipelines.

## Installation

### Download Binary

Download the latest release from the [Releases](https://github.com/lzjever/llm-universal-proxy/releases) page.

### Build from Source

```bash
# Clone the repository
git clone https://github.com/lzjever/llm-universal-proxy.git
cd llm-universal-proxy

# Build release binary
cargo build --release

# The binary will be at ./target/release/llm-universal-proxy
```

### Using Make

```bash
make build        # Build release binary
make test         # Run all tests
make run-release  # Build and run in release mode
```

## Configuration

The proxy is configured with a YAML file passed via `--config`:

```yaml
listen: 0.0.0.0:8080
upstream_timeout_secs: 120

upstreams:
  GLM-OFFICIAL:
    base_url: https://open.bigmodel.cn/api/anthropic
    format: anthropic
    credential_env: GLM_APIKEY
    auth_policy: client_or_fallback

  OPENAI:
    base_url: https://api.openai.com
    format: openai-responses
    credential_env: OPENAI_API_KEY
    auth_policy: force_server

model_aliases:
  GLM-5: GLM-OFFICIAL:GLM-5
  gpt-4o: OPENAI:gpt-4o

hooks:
  max_pending_bytes: 104857600
  timeout_secs: 30
  failure_threshold: 3
  cooldown_secs: 300
  usage:
    url: https://example.com/hooks/usage
  exchange:
    url: https://example.com/hooks/exchange
```

Notes:
- Best practice is to keep upstream `base_url` versionless. The proxy appends `/v1` or `/v1beta` internally, but it also supports compatibility roots that already contain a version segment such as `.../api/paas/v4`.
- Anthropic-compatible upstreams usually require `x-api-key` and `anthropic-version`. The proxy forwards client auth headers when present, can fall back to the upstream's configured `credential_env`, and injects a default `anthropic-version: 2023-06-01` header for Anthropic upstreams.
- Provider-specific headers belong inside each upstream entry's `headers` object.
- `credential_env` is the environment variable name holding that upstream's fallback credential. The secret stays out of the YAML file.
- `credential_actual` can be used instead of `credential_env` when you want to place a fallback credential directly in YAML. `credential_env` and `credential_actual` are mutually exclusive.
- `auth_policy` supports `client_or_fallback` and `force_server`.
- Hooks are best-effort and asynchronous. `usage` is usually enough; `exchange` captures the full client-facing request/response pair after the request completes.

## Usage

### Multi-Upstream Example

```bash
cat > proxy.yaml <<'YAML'
listen: 0.0.0.0:8080
upstream_timeout_secs: 120

upstreams:
  GLM-OFFICIAL:
    base_url: https://open.bigmodel.cn/api/anthropic
    format: anthropic
    credential_env: GLM_APIKEY
    auth_policy: client_or_fallback

  OPENAI:
    base_url: https://api.openai.com
    format: openai-responses
    credential_env: OPENAI_API_KEY
    auth_policy: force_server

model_aliases:
  GLM-5: GLM-OFFICIAL:GLM-5
  gpt-4o: OPENAI:gpt-4o
YAML

export GLM_APIKEY="your-glm-key"
export OPENAI_API_KEY="your-openai-key"

./llm-universal-proxy --config proxy.yaml
```

Clients can then select a model in either of these ways:
- Explicit upstream selector: `GLM-OFFICIAL:GLM-5`
- Local alias: `GLM-5`

If more than one upstream is configured and a model is not an explicit `upstream:model` reference or a configured alias, the proxy returns `400`.

### Stable Local Model Names

One practical pattern is to expose a provider-neutral local naming layer and hide vendor-specific model IDs behind it:

```yaml
model_aliases:
  opus: ANTHROPIC:claude-opus-4-1
  sonnet: ANTHROPIC:claude-sonnet-4
  haiku: ANTHROPIC:claude-haiku-4
  coder-fast: GLM-OFFICIAL:GLM-4.5-Air
  coder-strong: KIMI:kimi-k2
```

Clients can then request `opus`, `sonnet`, `haiku`, `coder-fast`, or `coder-strong` without caring which upstream vendor actually serves the request.

### Codex CLI to an Anthropic-Compatible Upstream

This is the practical setup for tools such as Codex CLI when the real upstream speaks the Anthropic Messages API but the client expects the OpenAI Responses API.

1. Start the proxy against the Anthropic-compatible upstream:

```bash
cat > codex-proxy.yaml <<'YAML'
listen: 127.0.0.1:8099

upstreams:
  GLM-OFFICIAL:
    base_url: https://open.bigmodel.cn/api/anthropic
    format: anthropic
    credential_env: GLM_APIKEY

model_aliases:
  GLM-5: GLM-OFFICIAL:GLM-5
YAML

./target/release/llm-universal-proxy
  --config codex-proxy.yaml
```

2. Point Codex CLI at the local proxy with an isolated config:

```bash
HOME="$(mktemp -d)" GLM_APIKEY="your-real-key" codex exec --ephemeral \
  -c 'model="GLM-5"' \
  -c 'model_provider="glm-proxy"' \
  -c 'model_providers.glm-proxy.name="GLM Proxy"' \
  -c 'model_providers.glm-proxy.base_url="http://127.0.0.1:8099/v1"' \
  -c 'model_providers.glm-proxy.env_key="GLM_APIKEY"' \
  -c 'model_providers.glm-proxy.wire_api="responses"' \
  'Reply with exactly: codex-ok'
```

Notes:
- This does not modify your global Codex CLI configuration because it uses a temporary `HOME` and `--ephemeral`.
- The client talks OpenAI Responses to the proxy at `/v1/responses`; the proxy resolves local model `GLM-5` to `GLM-OFFICIAL:GLM-5`, then translates upstream to Anthropic Messages.
- For providers that need extra static headers beyond the Anthropic default, set the upstream's `headers` field in the matching upstream entry.

### Real Upstream Smoke Matrix

The repository includes a real smoke script that exercises Anthropic-compatible and OpenAI-compatible upstreams through the proxy:

```bash
GLM_APIKEY="your-real-key" python3 scripts/real_endpoint_matrix.py
```

It covers these client entrypoints:
- `/v1/chat/completions`
- `/v1/responses`
- `/v1/messages`

And validates both non-streaming and streaming paths against:
- Anthropic-compatible upstreams
- OpenAI-compatible upstreams

### Docker

```bash
# Build the image
docker build -t llm-universal-proxy .

# Run the container
docker run -p 8080:8080 \
  -v "$PWD/proxy.yaml:/app/proxy.yaml:ro" \
  llm-universal-proxy
  --config /app/proxy.yaml
```

### API Endpoints

| Endpoint | Description |
|----------|-------------|
| `POST /v1/chat/completions` | Main endpoint accepting all 4 formats |
| `POST /v1/responses` | OpenAI Responses API endpoint |
| `POST /v1/messages` | Anthropic Messages API endpoint |
| `GET /health` | Health check (returns `{"status":"ok"}`) |

### Example Requests

#### OpenAI Chat Completions Format

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -d '{
    "model": "gpt-4",
    "messages": [{"role": "user", "content": "Hello!"}],
    "stream": false
  }'
```

#### Anthropic Claude Format

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "x-api-key: YOUR_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "claude-3-opus-20240229",
    "messages": [{"role": "user", "content": "Hello!"}],
    "max_tokens": 1024
  }'
```

#### Google Gemini Format

```bash
curl "http://localhost:8080/v1/chat/completions?key=YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "contents": [{"parts": [{"text": "Hello!"}]}]
  }'
```

## How It Works

1. **Format Detection**: Analyzes the request path and body to determine the client's API format
2. **Capability Discovery**: Probes the upstream service to determine supported formats
3. **Smart Routing**:
   - If client format matches upstream → **Passthrough** (zero overhead)
   - If formats differ → **Translation** using OpenAI Chat Completions as pivot format
4. **Streaming Support**: Handles SSE streams with chunk-by-chunk translation

## Architecture

```
                    ┌──────────────────────┐
                    │   LLM Universal      │
   Client Request   │       Proxy          │   Upstream Request
   (Any Format) ───▶│                      │──────────────────▶
                    │  ┌────────────────┐  │   (Converted if needed)
                    │  │   Detection    │  │
                    │  └───────┬────────┘  │
                    │          │           │
                    │  ┌───────▼────────┐  │
                    │  │   Translation  │  │
                    │  └───────┬────────┘  │
                    │          │           │
                    │  ┌───────▼────────┐  │
                    │  │   Upstream     │  │
                    │  │   Client       │──┼──────▶ OpenAI / Anthropic / Google
                    │  └────────────────┘  │
                    └──────────────────────┘
```

## Supported Format Conversions

| From → To | OpenAI | Anthropic | Gemini |
|-----------|--------|-----------|--------|
| OpenAI | ✅ Passthrough | ✅ Translate | ✅ Translate |
| Anthropic | ✅ Translate | ✅ Passthrough | ✅ Translate |
| Gemini | ✅ Translate | ✅ Translate | ✅ Passthrough |

## Development

```bash
# Run tests
cargo test

# Run with detailed test report
make test-report

# Check code
cargo clippy --all-targets --all-features -- -D warnings

# Format code
cargo fmt --all -- --check
```

## License

MIT License
