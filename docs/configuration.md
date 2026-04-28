# Configuration Guide

For most deployments, a static YAML file passed through `--config` is the simplest way to run `llmup`. For Codex CLI, Claude Code, and Gemini CLI, the recommended user-entry path is a wrapper-managed config source that is rendered into static YAML before the proxy starts.

Start from [examples/quickstart-provider-neutral.yaml](../examples/quickstart-provider-neutral.yaml) for the provider-neutral wrapper path. It exposes two stable local aliases:

- `preset-openai-compatible`
- `preset-anthropic-compatible`

MiniMax is only a replaceable OpenAI-compatible example, not a GA-required provider and not the main preset naming scheme. The historical concrete sample lives at [examples/quickstart-openai-minimax.yaml](../examples/quickstart-openai-minimax.yaml) for users who want to replace provider-neutral placeholders with named upstreams.

If you need to update config without restarting the process, see [Admin and Dynamic Config](./admin-dynamic-config.md).

## Quick Start

The provider-neutral quickstart config source is:

```yaml
listen: 127.0.0.1:8080
upstream_timeout_secs: 120

upstreams:
  PRESET-ANTHROPIC-COMPATIBLE:
    api_root: PRESET_ANTHROPIC_ENDPOINT_BASE_URL
    format: anthropic
    provider_key_env: PRESET_ENDPOINT_API_KEY
    limits:
      context_window: 200000
      max_output_tokens: 128000
    surface_defaults:
      modalities:
        input: ["text"]
        output: ["text"]
      tools:
        supports_search: false
        supports_view_image: false
        apply_patch_transport: freeform
        supports_parallel_calls: false

  PRESET-OPENAI-COMPATIBLE:
    api_root: PRESET_OPENAI_ENDPOINT_BASE_URL
    format: openai-completion
    provider_key_env: PRESET_ENDPOINT_API_KEY
    limits:
      context_window: 200000
      max_output_tokens: 128000
    surface_defaults:
      modalities:
        input: ["text"]
        output: ["text"]
      tools:
        supports_search: false
        supports_view_image: false
        apply_patch_transport: freeform
        supports_parallel_calls: false

model_aliases:
  preset-anthropic-compatible: "PRESET-ANTHROPIC-COMPATIBLE:PRESET_ENDPOINT_MODEL"
  preset-openai-compatible: "PRESET-OPENAI-COMPATIBLE:PRESET_ENDPOINT_MODEL"
```

The preset environment contract is:

| Variable | Meaning |
| --- | --- |
| `PRESET_OPENAI_ENDPOINT_BASE_URL` | OpenAI-compatible API root, including the version segment such as `/v1` |
| `PRESET_ANTHROPIC_ENDPOINT_BASE_URL` | Anthropic-compatible API root |
| `PRESET_ENDPOINT_MODEL` | Provider model ID hydrated into `preset-openai-compatible` and `preset-anthropic-compatible` |
| `PRESET_ENDPOINT_API_KEY` | Server-side provider credential referenced by both preset upstreams |

Minimal wrapper-managed flow:

```bash
export PRESET_OPENAI_ENDPOINT_BASE_URL="https://openai-compatible.example/v1"
export PRESET_ANTHROPIC_ENDPOINT_BASE_URL="https://anthropic-compatible.example/v1"
export PRESET_ENDPOINT_MODEL="provider-model-id"
export PRESET_ENDPOINT_API_KEY="provider-api-key"
export LLM_UNIVERSAL_PROXY_AUTH_MODE=proxy_key
export LLM_UNIVERSAL_PROXY_KEY="local-proxy-key"

./scripts/run_codex_proxy.sh \
  --config-source examples/quickstart-provider-neutral.yaml \
  --workspace "$PWD" \
  --model preset-openai-compatible
```

Reasoning effort such as `xhigh` stays on the client request; it is not part of the alias or upstream model name.

`PRESET_*` placeholders are not general Rust config interpolation. They are a provider-neutral config-source convention consumed by the wrappers and real CLI matrix. If you start `llm-universal-proxy --config` directly, use concrete `api_root` URLs and concrete `UPSTREAM:MODEL` alias targets.

## Data Plane Security

Client-facing provider/model/resource routes use a required global auth mode that is separate from the admin token. The mode is process-wide: one running proxy process uses one `LLM_UNIVERSAL_PROXY_AUTH_MODE` for all namespaces and all provider/model/resource routes. It is not a per-upstream setting, and it is not configured inside YAML. If you need a mixed deployment where some clients use a local proxy key and other clients pass provider keys directly, run separate proxy instances with different process environments.

Set `LLM_UNIVERSAL_PROXY_AUTH_MODE` to exactly one of:

- `proxy_key`: clients send the proxy key as their normal SDK API key or as `Authorization: Bearer <proxy-key>`. The proxy uses each upstream's `provider_key_env` value to read the real provider key from its own environment.
- `client_provider_key`: clients send the real provider key as their normal SDK API key or bearer token. The proxy does not need provider key env vars for those upstream calls.

`LLM_UNIVERSAL_PROXY_AUTH_MODE` and `LLM_UNIVERSAL_PROXY_KEY` are environment variables. Do not put them in YAML. `LLM_UNIVERSAL_PROXY_KEY` is required only in `proxy_key` mode. `/health` remains unauthenticated. Provider/model/resource routes reject missing client keys in both modes. Admin API routes still use `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN` and `Authorization: Bearer <admin-token>`.

`provider_key_env` is a per-upstream environment variable name. It is not the provider key itself. In `proxy_key` mode, every upstream that can receive traffic must set `provider_key_env`, and that named variable must resolve in the proxy process environment. In `client_provider_key` mode, `provider_key_env` is not required and is normally omitted; a non-empty value is not rejected just because this mode is active, but it is not used to choose the upstream credential for those requests.

### Static YAML Auth Examples

In `proxy_key` mode, the proxy owns provider credentials. Clients only see the local proxy key.

Set process environment before starting `llmup`:

```bash
export LLM_UNIVERSAL_PROXY_AUTH_MODE=proxy_key
export LLM_UNIVERSAL_PROXY_KEY="local-proxy-key"
export OPENAI_COMPATIBLE_API_KEY="real-openai-compatible-provider-key"
export ANTHROPIC_COMPATIBLE_API_KEY="real-anthropic-compatible-provider-key"
```

Static YAML:

```yaml
# Auth mode and proxy key are process environment, not YAML fields.
listen: 127.0.0.1:8080
upstream_timeout_secs: 120

upstreams:
  PROXY-KEY-OPENAI-COMPATIBLE:
    # Provider API root; include the provider's version segment.
    api_root: https://openai-compatible.example/v1
    format: openai-completion
    # Name of the env var the proxy reads for this upstream's provider key.
    provider_key_env: OPENAI_COMPATIBLE_API_KEY

  PROXY-KEY-ANTHROPIC-COMPATIBLE:
    api_root: https://anthropic-compatible.example/v1
    format: anthropic
    provider_key_env: ANTHROPIC_COMPATIBLE_API_KEY

model_aliases:
  coding-openai: "PROXY-KEY-OPENAI-COMPATIBLE:provider-openai-model"
  coding-anthropic: "PROXY-KEY-ANTHROPIC-COMPATIBLE:provider-anthropic-model"
```

In `client_provider_key` mode, clients send the real provider key through their normal SDK API key or bearer-token path. The proxy does not need server-side provider key env vars for these upstream calls, so static YAML normally leaves `provider_key_env` out.

Set process environment before starting `llmup`:

```bash
export LLM_UNIVERSAL_PROXY_AUTH_MODE=client_provider_key
```

Static YAML:

```yaml
# Auth mode is process environment. Clients provide real provider keys.
listen: 127.0.0.1:8080
upstream_timeout_secs: 120

upstreams:
  CLIENT-KEY-OPENAI-COMPATIBLE:
    api_root: https://openai-compatible.example/v1
    format: openai-completion
    # provider_key_env is normally omitted; the client key is forwarded upstream.

  CLIENT-KEY-ANTHROPIC-COMPATIBLE:
    api_root: https://anthropic-compatible.example/v1
    format: anthropic
    # provider_key_env is normally omitted here too.

model_aliases:
  bring-your-openai-key: "CLIENT-KEY-OPENAI-COMPATIBLE:provider-openai-model"
  bring-your-anthropic-key: "CLIENT-KEY-ANTHROPIC-COMPATIBLE:provider-anthropic-model"
```

CORS is off by default. To allow browser callers, set `LLM_UNIVERSAL_PROXY_CORS_ALLOWED_ORIGINS` to a comma-separated list of exact origins, for example `https://app.example,https://console.example`. Wildcard origins are not accepted, and CORS is not an auth mechanism.

## The Main Sections

### `listen`

The address the proxy binds to, for example `127.0.0.1:8080`.

### `upstream_timeout_secs`

The request timeout for upstream calls.

### `resource_limits`

Resource boundaries for client request bodies, upstream response bodies, and streaming SSE translation. All values must be greater than zero.

- `max_request_body_bytes`: maximum client JSON request body size
- `max_non_stream_response_bytes`: maximum successful non-stream upstream response body size
- `max_upstream_error_body_bytes`: maximum upstream error body captured before returning a bounded proxy error
- `max_sse_frame_bytes`: maximum size of a single upstream SSE frame
- `stream_idle_timeout_secs`: maximum idle time between upstream stream chunks
- `stream_max_duration_secs`: maximum total lifetime for an upstream stream
- `stream_max_events`: maximum upstream SSE frames processed for one stream
- `max_accumulated_stream_state_bytes`: maximum accumulated translator state for one stream

### `compatibility_mode`

Controls how aggressively the proxy tries to preserve client-facing behavior on translated paths.

- `max_compat` is the default and the normal choice for real coding clients
- `balanced` is a middle ground
- `strict` prefers hard boundaries over compatibility shims

Responses reasoning/compaction continuity has specific mode boundaries: default/max_compat may drop an opaque carrier only when visible summary text or visible transcript history remains; strict/balanced fail closed; opaque-only reasoning and opaque-only compaction fail closed; same-provider/native passthrough preserves provider-owned state.

### `upstreams`

A map of named upstreams. Each upstream defines where the real provider lives and which protocol shape the proxy should use when talking to it.

Each upstream usually needs:

- `api_root`: the provider API root, including its version segment
- `format`: the expected upstream protocol when you want to pin it
- `provider_key_env`: the environment variable that contains the provider key in `proxy_key` mode

Practical rules:

- `api_root` should point at the provider API root, not a model-specific path
- include the version segment such as `/v1` or `/v1beta`
- `upstream_headers` may add non-secret routing or tenant headers, but cannot override auth/secret headers such as `authorization`, `proxy-authorization`, `x-api-key`, `api-key`, `openai-api-key`, `x-goog-api-key`, or `anthropic-api-key`
- use `provider_key_env` when `LLM_UNIVERSAL_PROXY_AUTH_MODE=proxy_key`
- normally omit `provider_key_env` when `LLM_UNIVERSAL_PROXY_AUTH_MODE=client_provider_key` and clients provide provider keys directly; a non-empty value is accepted but unnecessary

Provider-specific static headers belong inside the upstream's `headers` field.

### `model_aliases`

`model_aliases` lets you present one stable local model name to clients even if the real upstream models change over time.

The provider-neutral preset aliases are:

```yaml
model_aliases:
  preset-anthropic-compatible: "PRESET-ANTHROPIC-COMPATIBLE:PRESET_ENDPOINT_MODEL"
  preset-openai-compatible: "PRESET-OPENAI-COMPATIBLE:PRESET_ENDPOINT_MODEL"
```

Those are local names. They do not need to match provider model IDs. After wrapper hydration, both aliases resolve to the concrete `PRESET_ENDPOINT_MODEL` value.

If you want more explicit metadata, switch to the structured alias form:

```yaml
model_aliases:
  preset-openai-compatible:
    target: PRESET-OPENAI-COMPATIBLE:provider-model-id
    limits:
      context_window: 200000
      max_output_tokens: 128000
```

Model resolution rules:

- if the client requests an alias such as `preset-openai-compatible`, the proxy resolves it through `model_aliases`
- if the client requests `UPSTREAM:MODEL`, the proxy routes directly to that named upstream and model
- if multiple upstreams exist and the requested model is neither an alias nor an explicit `UPSTREAM:MODEL`, the proxy rejects the request instead of guessing

### `surface_defaults` and alias `surface`

Use `limits` when you want the proxy and client wrappers to know things such as:

- `context_window`
- `max_output_tokens`

Use `surface_defaults` on an upstream, or `surface` on an alias, when you want to describe the client-visible behavior of a routed model.

Raw HTTP smoke tests can omit these fields, but wrapper/live-profile flows should provide at least the minimal text-only surface used by the quickstart. Add richer values only when the model surface really supports them.

Supported `surface.modalities.input` values:

| Value | Meaning |
| --- | --- |
| `text` | Plain text input. |
| `image` | Image input parts. |
| `audio` | Audio input parts. |
| `pdf` | Narrow PDF input capability. Use this when the model supports PDF documents but not arbitrary files. |
| `file` | Generic file input capability. This includes PDF; use `pdf` when you need to advertise only the narrower PDF shape. |
| `video` | Video input capability. In the current first multimodal phase, video is primarily a request-policy gate and is not a promise of cross-provider video translation. |

These values describe the proxy's protocol compatibility layer and the configured client-visible alias surface. They do not prove that a real upstream provider or model accepts that media; configure only what the selected upstream model actually supports.

`surface.modalities.input` gates media types, not every possible source transport for that media. HTTP(S) URLs are explicit remote URLs; provider-native or local URIs such as `gs://`, `s3://`, and `file://`, and provider-owned identifiers such as `file_id`, are different source identities and are not portable unless a documented adapter supports them.

Unsupported media and unsupported source transports must fail closed. The proxy should reject unsupported or unknown typed media parts instead of silently dropping them, flattening them into text, or forwarding them to an upstream surface that cannot represent them.

Media metadata must also be self-consistent. For OpenAI Chat `file` parts and OpenAI Responses `input_file` parts, the proxy compares explicit `mime_type` / `mimeType`, MIME-bearing `file_data` data URIs, and filename-derived hints. If those sources disagree, the request is rejected before any upstream call.

## Upstream Egress Proxy

The public config shape is:

- top-level `proxy`
- per-upstream `proxy`

Each `proxy` value is either:

- `direct`
- `{ url: ... }`

If both levels are omitted, the proxy falls back to the standard environment proxy variables.

Resolution order:

1. `upstreams.<NAME>.proxy`
2. top-level `proxy`
3. environment proxy settings

Supported `proxy.url` schemes:

- `http://`
- `https://`
- `socks5://`
- `socks5h://`

For a fuller proxy example, see [examples/upstream-proxy.yaml](../examples/upstream-proxy.yaml).

## Optional Hooks and Debug Trace

These are optional and should usually come after the basic routing config is already working.

### `hooks`

Use hooks when you want best-effort outbound reporting for usage or exchanges.

### `debug_trace`

Use `debug_trace` when you want a local JSONL trail for debugging request and response behavior.

For client attachment and wrapper details, see [Client Setup Guide](./clients.md).
