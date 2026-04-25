# Configuration Guide

For most deployments, a static YAML file passed through `--config` is the simplest way to run `llmup`.

This guide keeps the same homepage story on purpose:

- one official OpenAI upstream
- one MiniMax OpenAI-compatible upstream
- two stable local aliases, `gpt-5-4` and `gpt-5-4-mini`

Start from [examples/quickstart-openai-minimax.yaml](../examples/quickstart-openai-minimax.yaml) if you want the shortest path to a working config.

If you need to update config without restarting the process, see [Admin and Dynamic Config](./admin-dynamic-config.md).

## Quick Start

The homepage quickstart config is also the recommended baseline here:

```yaml
listen: 127.0.0.1:8080
upstream_timeout_secs: 120

upstreams:
  OPENAI:
    api_root: https://api.openai.com/v1
    format: openai-responses
    credential_env: OPENAI_API_KEY
    auth_policy: force_server
    surface_defaults:
      modalities:
        input: ["text"]
        output: ["text"]
      tools:
        supports_search: false
        supports_view_image: false
        apply_patch_transport: freeform
        supports_parallel_calls: false

  MINIMAX_OPENAI:
    api_root: https://api.minimaxi.com/v1
    format: openai-completion
    credential_env: MINIMAX_API_KEY
    auth_policy: force_server
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
  gpt-5-4: OPENAI:gpt-5.4
  gpt-5-4-mini: MINIMAX_OPENAI:MiniMax-M2.7-highspeed
```

This gives you:

- one local alias that routes to official OpenAI `gpt-5.4`
- one local alias that routes to MiniMax `MiniMax-M2.7-highspeed`
- one stable model naming layer that clients can keep using even if you later swap upstreams

Reasoning effort such as `xhigh` stays on the client request; it is not part of the alias or upstream model name.

Minimal startup flow:

```bash
export OPENAI_API_KEY="your-openai-key"
export MINIMAX_API_KEY="your-minimax-key"

./target/release/llm-universal-proxy --config examples/quickstart-openai-minimax.yaml
```

## The Main Sections

### `listen`

The address the proxy binds to, for example `127.0.0.1:8080`.

### `upstream_timeout_secs`

The request timeout for upstream calls.

### `compatibility_mode`

Controls how aggressively the proxy tries to preserve client-facing behavior on translated paths.

- `max_compat` is the default and the normal choice for real coding clients
- `balanced` is a middle ground
- `strict` prefers hard boundaries over compatibility shims

If you are not actively debugging protocol edge cases, leave this at the default.

### `upstreams`

A map of named upstreams. Each upstream defines where the real provider lives and which protocol shape the proxy should use when talking to it.

### `model_aliases`

A map from one stable local model name to one concrete upstream model using `UPSTREAM:MODEL`.

### `proxy`

The default forward proxy policy for upstream egress in this namespace.

### `hooks` and `debug_trace`

Optional observability features. They are useful, but they are not required for the proxy to work.

## Defining Upstreams

Each upstream usually needs:

- `api_root`: the provider API root, including its version segment
- `format`: the expected upstream protocol when you want to pin it
- `credential_env` or `credential_actual`: the server-side fallback credential
- `auth_policy`: whether the client may bring its own credential

Example with the same two-upstream homepage story:

```yaml
upstreams:
  OPENAI:
    api_root: https://api.openai.com/v1
    format: openai-responses
    credential_env: OPENAI_API_KEY
    auth_policy: force_server

  MINIMAX_OPENAI:
    api_root: https://api.minimaxi.com/v1
    format: openai-completion
    credential_env: MINIMAX_API_KEY
    auth_policy: force_server
```

Practical rules:

- `api_root` should point at the provider API root, not a model-specific path
- include the version segment such as `/v1` or `/v1beta`
- use `credential_env` when you want secrets outside the YAML file
- use `auth_policy: force_server` when the proxy should always use the server-side credential
- use `auth_policy: client_or_fallback` when the client may provide its own auth and the proxy only falls back when needed

Provider-specific static headers belong inside the upstream's `headers` field.

## Stable Model Aliases

`model_aliases` lets you present one stable local model name to clients even if the real upstream models change over time.

The homepage example intentionally uses:

```yaml
model_aliases:
  gpt-5-4: OPENAI:gpt-5.4
  gpt-5-4-mini: MINIMAX_OPENAI:MiniMax-M2.7-highspeed
```

Those are local names, not vendor guarantees. In other words, `gpt-5-4-mini` is allowed to route to MiniMax because the alias is yours, not the provider's.

If you want more explicit metadata, switch to the structured alias form:

```yaml
model_aliases:
  gpt-5-4-mini:
    target: MINIMAX_OPENAI:MiniMax-M2.7-highspeed
    limits:
      context_window: 204800
      max_output_tokens: 16384
```

Model resolution rules:

- if the client requests an alias such as `gpt-5-4`, the proxy resolves it through `model_aliases`
- if the client requests `UPSTREAM:MODEL`, the proxy routes directly to that named upstream and model
- if multiple upstreams exist and the requested model is neither an alias nor an explicit `UPSTREAM:MODEL`, the proxy rejects the request instead of guessing

## Limits and Client-Visible Surface

You can attach defaults at either the upstream level or the alias level.

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

These values describe the proxy's protocol compatibility layer and the configured client-visible alias surface. They do not prove that a real upstream provider or model accepts that media; configure only what the selected upstream model actually supports. The live MiniMax quickstart/provider profile should remain text-only unless a future MiniMax integration is explicitly validated for multimodal input; current multimodal e2e coverage uses first-party mock upstreams rather than the live MiniMax provider.

Unsupported media must fail closed. The proxy should reject unsupported or unknown typed media parts instead of silently dropping them, flattening them into text, or forwarding them to an upstream surface that cannot represent them.

Example:

```yaml
upstreams:
  MINIMAX_OPENAI:
    api_root: https://api.minimaxi.com/v1
    format: openai-completion
    credential_env: MINIMAX_API_KEY
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
  gpt-5-4-mini:
    target: MINIMAX_OPENAI:MiniMax-M2.7-highspeed
    limits:
      context_window: 204800
      max_output_tokens: 16384
```

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

Example:

```yaml
proxy:
  url: http://corp-proxy.example:8080

upstreams:
  OPENAI:
    api_root: https://api.openai.com/v1
    format: openai-responses
    credential_env: OPENAI_API_KEY

  MINIMAX_OPENAI:
    api_root: https://api.minimaxi.com/v1
    format: openai-completion
    credential_env: MINIMAX_API_KEY
    proxy: direct
```

What that means:

- `OPENAI` uses the namespace default proxy
- `MINIMAX_OPENAI` bypasses both the namespace proxy and environment proxy fallback

Supported `proxy.url` schemes:

- `http://`
- `https://`
- `socks5://`
- `socks5h://`

For a fuller proxy example, see [examples/upstream-proxy.yaml](../examples/upstream-proxy.yaml).

That example intentionally focuses on raw HTTP egress proxy behavior. If you adapt it for Codex, Claude Code, or Gemini wrapper/live-profile flows, keep the surface guidance above.

## Optional Hooks and Debug Trace

These are optional and should usually come after the basic routing config is already working.

### `hooks`

Use hooks when you want best-effort outbound reporting for usage or exchanges.

Example:

```yaml
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

### `debug_trace`

Use `debug_trace` when you want a local JSONL trail for debugging request and response behavior.

Example:

```yaml
debug_trace:
  path: /tmp/llmup-debug.jsonl
  max_text_chars: 16384
```

## Choosing Static vs Dynamic Config

Use static YAML when:

- you have one local or server deployment
- you are iterating on a config file by hand
- you want the simplest operating model

Use admin-driven dynamic config when:

- you need to update routing without restarting the process
- you want a runtime view of resolved upstream state
- you need revision-checked config writes

See [Admin and Dynamic Config](./admin-dynamic-config.md) for that flow.
