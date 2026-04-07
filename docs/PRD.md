# LLM Universal Proxy — Product Requirements Document (PRD)

**Version**: 1.0
**Status**: Active
**Last Updated**: 2026-04-07

---

## 1. Product Overview

### 1.1 Product Name

LLM Universal Proxy (codename: proxec)

### 1.2 Product Definition

A single-binary HTTP proxy that acts as a universal protocol translation layer between LLM API clients and LLM backend endpoints. Clients using any of the four major LLM API protocols can transparently connect to any upstream endpoint using any of the same four protocols, with the proxy handling all format detection, request translation, response translation, and streaming adaptation in real time.

### 1.3 Problem Statement

The LLM ecosystem is fragmented across incompatible API protocols:

- **OpenAI Chat Completions** — the de facto standard for most third-party LLM providers
- **OpenAI Responses API** — used by newer tools like Codex CLI
- **Anthropic Messages** — used by Claude Code and Claude-native applications
- **Google Gemini** — used by Gemini CLI and Google-native applications

Each client tool typically speaks only one protocol. Each upstream endpoint typically supports only one or two protocols. This creates an N×M compatibility matrix that is impractical for users and tool developers to manage individually.

### 1.4 Target Users

| User | Use Case |
|------|----------|
| Developer using Codex CLI | Wants to use a non-OpenAI model (GLM, MiniMax, Kimi, local vLLM) through Codex's Responses-only interface |
| Developer using Claude Code | Wants to route Claude Code requests to a non-Anthropic upstream for cost or availability reasons |
| Developer using Gemini CLI | Wants to use Gemini CLI with a non-Google upstream endpoint |
| Team with multiple LLM providers | Wants a single stable endpoint that normalizes model naming across providers |
| Operator running local models | Wants to expose a local vLLM/Ollama instance to any LLM client tool regardless of protocol |
| AI infrastructure engineer | Wants an observability layer for request/response auditing and usage metering |

---

## 2. Functional Requirements

### 2.1 Protocol Support Matrix

The proxy MUST support bidirectional translation between all four protocols:

| # | Client Protocol | Upstream Protocol | Translation Required |
|---|----------------|-------------------|---------------------|
| 1 | OpenAI Chat Completions | OpenAI Chat Completions | No (passthrough) |
| 2 | OpenAI Chat Completions | OpenAI Responses | Yes |
| 3 | OpenAI Chat Completions | Anthropic Messages | Yes |
| 4 | OpenAI Chat Completions | Google Gemini | Yes |
| 5 | OpenAI Responses | OpenAI Chat Completions | Yes |
| 6 | OpenAI Responses | OpenAI Responses | No (passthrough) |
| 7 | OpenAI Responses | Anthropic Messages | Yes |
| 8 | OpenAI Responses | Google Gemini | Yes |
| 9 | Anthropic Messages | OpenAI Chat Completions | Yes |
| 10 | Anthropic Messages | OpenAI Responses | Yes |
| 11 | Anthropic Messages | Anthropic Messages | No (passthrough) |
| 12 | Anthropic Messages | Google Gemini | Yes |
| 13 | Google Gemini | OpenAI Chat Completions | Yes |
| 14 | Google Gemini | OpenAI Responses | Yes |
| 15 | Google Gemini | Anthropic Messages | Yes |
| 16 | Google Gemini | Google Gemini | No (passthrough) |

All 16 combinations (4 passthrough + 12 translated) MUST work correctly.

### 2.2 Client Endpoints

The proxy MUST expose the following namespaced endpoints:

| Endpoint | Protocol | Method |
|----------|----------|--------|
| `/openai/v1/chat/completions` | OpenAI Chat Completions | POST |
| `/openai/v1/responses` | OpenAI Responses | POST |
| `/openai/v1/models` | Model catalog | GET |
| `/anthropic/v1/messages` | Anthropic Messages | POST |
| `/anthropic/v1/models` | Model catalog | GET |
| `/google/v1beta/models/:id` | Gemini GenerateContent | POST |
| `/google/v1beta/models` | Model catalog | GET |

### 2.3 Streaming Support

- The proxy MUST support SSE (Server-Sent Events) streaming for all 16 protocol combinations.
- Streaming translation MUST operate chunk-by-chunk without buffering the entire response.
- Each protocol's streaming lifecycle events MUST be correctly translated:
  - **OpenAI Chat Completions**: `data:` chunks + `[DONE]`
  - **OpenAI Responses**: `response.created` → `response.in_progress` → `output_item.added` → `content_part.added` → `output_text.delta` → ... → `response.completed`
  - **Anthropic Messages**: `message_start` → `content_block_start` → `content_block_delta` → `content_block_stop` → `message_delta` → `message_stop`
  - **Google Gemini**: `candidates` chunks with `parts`

### 2.4 Request Translation

The proxy MUST translate the following request fields across protocols:

| Field | Status | Notes |
|-------|--------|-------|
| Text messages | Must | Exact preservation across all formats |
| System instructions | Must | Map between `system` role (OpenAI), top-level `system` (Anthropic), `systemInstruction` (Gemini), `instructions` (Responses) |
| Function tool definitions | Must | Map `function` tools across all formats |
| Tool choice (auto/none/required) | Must | Map between format-specific tool choice objects |
| `max_output_tokens` / `max_tokens` | Must | Normalize field names |
| `temperature`, `top_p` | Should | Pass through generation config where applicable |
| Image content | Should | Map between `image_url` (OpenAI), `source.base64` (Anthropic), `inlineData` (Gemini) |
| Thinking/reasoning config | May | Preserve where upstream supports it |
| Built-in / non-function tools | Won't | Dropped during cross-protocol translation with compat warning |

### 2.5 Response Translation

The proxy MUST translate the following response fields:

| Field | Status | Notes |
|-------|--------|-------|
| Text content | Must | Exact preservation |
| Tool calls / function calls | Must | Map between `tool_calls` (OpenAI), `tool_use` blocks (Anthropic), `functionCall` parts (Gemini) |
| Tool results | Must | Map between `tool` role messages (OpenAI), `tool_result` blocks (Anthropic), `functionResponse` parts (Gemini) |
| Usage / token metrics | Must | Map between `prompt_tokens/completion_tokens` (OpenAI), `input_tokens/output_tokens` (Anthropic), `promptTokenCount/candidatesTokenCount` (Gemini) |
| Finish reasons | Must | Map stop reasons: `stop` ↔ `end_turn` ↔ `STOP`, `tool_calls` ↔ `tool_use` ↔ `STOP`, `length` ↔ `max_tokens` ↔ `MAX_TOKENS` |
| Reasoning / thinking output | Should | Preserve as `reasoning_content` (OpenAI), `thinking` blocks (Anthropic), `thought` parts (Gemini) |
| Cached token details | Should | Map cache-related usage fields |
| Error responses | Must | Translate upstream errors into client-protocol-appropriate error shapes |

### 2.6 Upstream Configuration

The proxy MUST support:

- **Multiple named upstreams** — each with its own API root, protocol format, and credentials
- **Model aliases** — map local model names to `upstream:real_model` pairs
- **Auto-discovery** — probe upstream to detect supported protocols when format is not explicitly set
- **Credential policies**:
  - `client_or_fallback` — use client-provided auth if present, otherwise use configured fallback
  - `force_server` — always use server-side credentials, ignore client auth

### 2.7 Observability

| Feature | Priority | Description |
|---------|----------|-------------|
| Debug trace (JSONL) | Must | Per-request debug trace file for protocol troubleshooting |
| Health endpoint | Must | `GET /health` returns `{"status":"ok"}` |
| Exchange hooks | Should | Async HTTP webhook for full request/response capture |
| Usage hooks | Should | Async HTTP webhook for token usage metering |
| Dashboard | May | Terminal UI showing live request stats and upstream health |
| Compatibility warnings | Must | `x-proxy-compat-warning` response headers for degraded translations |
| Request ID tracking | Must | Every request gets a unique ID for correlation |

### 2.8 Namespace Support

The proxy MUST support namespace-prefixed routes for multi-tenant deployments:

- `/namespaces/:namespace/openai/v1/...`
- `/namespaces/:namespace/anthropic/v1/...`
- `/namespaces/:namespace/google/v1beta/...`

Runtime configuration per namespace via admin API:
- `POST /admin/namespaces/:namespace/config`

---

## 3. Non-Functional Requirements

### 3.1 Performance

| Requirement | Target |
|-------------|--------|
| Passthrough latency overhead | < 1ms added latency |
| Translation latency overhead | < 10ms added latency per request |
| Streaming chunk translation | < 1ms per chunk |
| Concurrent requests | Support 100+ concurrent requests without degradation |
| Memory per request | Bounded; no unbounded accumulation during streaming |

### 3.2 Reliability

- The proxy MUST NOT crash on malformed requests (return 4xx instead).
- The proxy MUST NOT crash on upstream failures (return 502 instead).
- Hook delivery failures MUST NOT affect request serving (circuit breaker pattern).
- The proxy MUST handle upstream timeouts gracefully (configurable timeout).

### 3.3 Compatibility

- Compatible with any HTTP client that speaks one of the four supported protocols.
- No client-side SDK changes required — the proxy is a drop-in replacement for the upstream URL.
- Works with both official vendor APIs and third-party compatible endpoints.

### 3.4 Deployment

- Single static binary (Rust, no runtime dependencies).
- YAML configuration file.
- Docker support via multi-stage build.
- Cross-platform: Linux (primary), macOS, Windows.

---

## 4. Protocol Translation Architecture

### 4.1 Pivot Format

OpenAI Chat Completions serves as the canonical intermediate format. All cross-protocol translations go through two steps:

```
Source → OpenAI Chat Completions → Target
```

This reduces the translation matrix from O(N²) to O(N) — adding a new protocol requires only two bidirectional translators (new ↔ OpenAI), not N translators.

### 4.2 Translation Pipeline

```
1. Client sends request in format A
2. Detect client format from path + body
3. Resolve model → upstream + real model name
4. If client format == upstream format → passthrough
5. Otherwise:
   a. Translate request: A → OpenAI Chat → B
   b. Send translated request to upstream
   c. Receive response from upstream
   d. Translate response: B → OpenAI Chat → A
   e. Return translated response to client
```

### 4.3 Streaming Translation

For SSE streaming, each chunk is translated individually:

```
1. Upstream sends SSE chunk in format B
2. Parse chunk according to format B semantics
3. Translate to OpenAI Chat delta format
4. Translate from OpenAI Chat delta to client format A
5. Emit translated SSE event to client
```

Stateful accumulators track message IDs, tool call IDs, content buffers, and finish reasons across chunks.

---

## 5. Supported Upstream Types

### 5.1 Official Vendor APIs

| Upstream | Protocol | Example |
|----------|----------|---------|
| OpenAI | OpenAI Responses + Chat Completions | `api.openai.com` |
| Anthropic | Anthropic Messages | `api.anthropic.com` |
| Google | Google Gemini | `generativelanguage.googleapis.com` |

### 5.2 Third-Party Compatible Endpoints

| Upstream | Primary Protocol | Notes |
|----------|-----------------|-------|
| GLM (Zhipu) | Anthropic-compatible + OpenAI-compatible | `open.bigmodel.cn` |
| MiniMax | Anthropic-compatible + OpenAI-compatible | `api.minimaxi.com` |
| Kimi (Moonshot) | OpenAI-compatible | `api.moonshot.cn` |
| DeepSeek | OpenAI-compatible | `api.deepseek.com` |
| Mistral | OpenAI-compatible | `api.mistral.ai` |

### 5.3 Self-Hosted Endpoints

| Upstream | Protocol | Notes |
|----------|----------|-------|
| vLLM | OpenAI Chat Completions | Common for serving local models |
| Ollama | OpenAI-compatible | Local model runtime |
| llama.cpp server | OpenAI-compatible | Local inference |
| Local model on custom port | OpenAI-compatible | e.g., qwen3.5-9b-awq at `http://192.168.0.220:9997/v1` |

---

## 6. Test Configuration

### 6.1 Test Environments

The proxy MUST be tested against the following real upstream endpoints:

| Name | Endpoint URL | Protocol | Model | Context |
|------|-------------|----------|-------|---------|
| MiniMax (Anthropic) | `https://api.minimaxi.com/anthropic/v1` | Anthropic Messages | `MiniMax-M2.7-highspeed` | 204,800 tokens |
| MiniMax (OpenAI) | `https://api.minimaxi.com/v1` | OpenAI Chat Completions | `MiniMax-M2.7-highspeed` | 204,800 tokens |
| Local qwen3.5-9b-awq | `http://192.168.0.220:9997/v1` | OpenAI Chat Completions | `qwen3.5-9b-awq` | 16,384 tokens |

### 6.2 Test Matrix

For each upstream, test all applicable client entrypoints:

| Client Entry | MiniMax Anthropic | MiniMax OpenAI | Local qwen3.5 |
|-------------|-------------------|----------------|---------------|
| `/openai/v1/chat/completions` (non-stream) | Translate | Passthrough | Passthrough |
| `/openai/v1/chat/completions` (stream) | Translate | Passthrough | Passthrough |
| `/openai/v1/responses` (non-stream) | Translate | Translate | Translate |
| `/openai/v1/responses` (stream) | Translate | Translate | Translate |
| `/anthropic/v1/messages` (non-stream) | Passthrough | Translate | Translate |
| `/anthropic/v1/messages` (stream) | Passthrough | Translate | Translate |
| `/google/v1beta/models/:id` (non-stream) | Translate | Translate | Translate |
| `/google/v1beta/models/:id` (stream) | Translate | Translate | Translate |

### 6.3 Client Tool Tests

| Client Tool | Client Protocol | Test Command |
|-------------|----------------|-------------|
| Codex CLI | OpenAI Responses | `codex exec --ephemeral` with proxy base URL |
| Claude Code | Anthropic Messages | `claude --print` with proxy base URL |
| Gemini CLI | Google Gemini | `gemini --prompt` with proxy base URL |
| curl (OpenAI) | OpenAI Chat Completions | Direct HTTP request |
| curl (Anthropic) | Anthropic Messages | Direct HTTP request |

---

## 7. Compatibility and Degradation Policy

### 7.1 Translation Fidelity Levels

| Level | Meaning | Example |
|-------|---------|---------|
| **Exact** | Semantics preserved closely enough for identical downstream behavior | Text content, basic tool calls |
| **Approximate** | Primary behavior preserved but wire shape differs | Tool choice mapping, cached tokens |
| **Dropped** | No safe mapping exists; field is omitted | `previous_response_id`, built-in tools |

### 7.2 Degradation Signaling

When the proxy must drop or approximate fields, it MUST:
1. Emit `x-proxy-compat-warning` response headers
2. Log the degradation to server logs
3. NOT silently pretend 1:1 fidelity

### 7.3 Known Limitations

| Limitation | Reason |
|-----------|--------|
| `previous_response_id` (Responses) | Stateful chaining cannot be reconstructed in stateless formats |
| Built-in tools (web search, etc.) | Not portable across protocol schemas |
| `truncation` policy | Provider-specific context management |
| Reasoning request config | Only reasoning output is mapped, not request policy |
| `store` / persistence | Provider-specific outside OpenAI family |

---

## 8. Future Considerations

These are NOT in scope for the current version but inform architectural decisions:

| Area | Consideration |
|------|--------------|
| Additional protocols | Ollama native, AWS Bedrock, Azure OpenAI |
| Batch processing | Translation for batch/non-real-time API calls |
| Embeddings | Protocol translation for embedding endpoints |
| Content moderation | Cross-protocol content filtering |
| A/B routing | Route different requests to different upstreams based on policy |
| Caching | Response caching for identical requests |

---

## 9. Success Metrics

| Metric | Target |
|--------|--------|
| All 16 protocol combinations working | 16/16 pass |
| Streaming works for all combinations | 16/16 pass |
| Codex CLI can use any upstream through proxy | All test upstreams work |
| Claude Code can use any upstream through proxy | All test upstreams work |
| Passthrough adds < 1ms latency | Measured |
| No silent data loss during translation | All compat warnings are emitted correctly |
