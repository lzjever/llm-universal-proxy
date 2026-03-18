# LLM Universal Proxy

[中文文档](./README_CN.md)

A single-binary HTTP proxy that provides a unified interface for Large Language Model APIs. It accepts requests in multiple LLM API formats and automatically handles format conversion when needed.

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
- **Codex CLI Friendly**: Works as a Responses-compatible endpoint in front of Anthropic-compatible upstreams

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

The proxy is configured via environment variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `LISTEN` | Listen address | `0.0.0.0:8080` |
| `UPSTREAM_URL` | Upstream service base URL | `https://api.openai.com/v1` |
| `UPSTREAM_FORMAT` | Fixed upstream format (skips auto-discovery). Options: `google`, `anthropic`, `openai-completion`, `openai-responses` | *(auto-detect)* |
| `UPSTREAM_TIMEOUT_SECS` | Request timeout in seconds | `120` |
| `UPSTREAM_API_KEY` | Fallback upstream API key used when the client provides no auth header | *(unset)* |
| `UPSTREAM_HEADERS` | Static upstream headers as a JSON object, for example `{"anthropic-version":"2023-06-01"}` | *(unset)* |

Notes:
- Anthropic-compatible upstreams usually require `x-api-key` and `anthropic-version`. The proxy forwards client auth headers when present, can fall back to `UPSTREAM_API_KEY`, and injects a default `anthropic-version: 2023-06-01` header for Anthropic upstreams.
- `UPSTREAM_HEADERS` is merged on top of the defaults, so it can be used for provider-specific headers without changing clients.

## Usage

### Basic Example

```bash
# Start the proxy pointing to OpenAI
UPSTREAM_URL=https://api.openai.com/v1 ./llm-universal-proxy

# Start the proxy pointing to Anthropic Claude
UPSTREAM_URL=https://api.anthropic.com/v1 ./llm-universal-proxy

# Start the proxy pointing to Google Gemini
UPSTREAM_URL=https://generativelanguage.googleapis.com/v1beta ./llm-universal-proxy
```

### Codex CLI to an Anthropic-Compatible Upstream

This is the practical setup for tools such as Codex CLI when the real upstream speaks the Anthropic Messages API but the client expects the OpenAI Responses API.

1. Start the proxy against the Anthropic-compatible upstream:

```bash
LISTEN=127.0.0.1:8099 \
UPSTREAM_URL=https://open.bigmodel.cn/api/anthropic/v1 \
UPSTREAM_FORMAT=anthropic \
UPSTREAM_API_KEY="$GLM_APIKEY" \
./target/release/llm-universal-proxy
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
- The client talks OpenAI Responses to the proxy at `/v1/responses`; the proxy translates upstream to Anthropic Messages.
- For providers that need extra static headers beyond the Anthropic default, set `UPSTREAM_HEADERS`.

### Docker

```bash
# Build the image
docker build -t llm-universal-proxy .

# Run the container
docker run -p 8080:8080 -e UPSTREAM_URL=https://api.openai.com/v1 llm-universal-proxy
```

### API Endpoints

| Endpoint | Description |
|----------|-------------|
| `POST /v1/chat/completions` | Main endpoint accepting all 4 formats |
| `POST /v1/responses` | OpenAI Responses API endpoint |
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
