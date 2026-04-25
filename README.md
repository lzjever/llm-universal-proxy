# LLM Universal Proxy

[中文文档](./README_CN.md) · [Documentation](./docs/README.md)

`llmup` is a single-binary LLM HTTP proxy. Put it between your client and your real model provider, and it gives you one stable local entrypoint even when the client protocol and upstream protocol do not match.

It is most useful when you want to:

- use non-native models behind Codex CLI
- route Claude Code or Gemini CLI through one local proxy
- expose stable local model aliases instead of vendor model IDs

> [!IMPORTANT]
> `llmup` is designed for provider APIs and compatible endpoints. It is not a bridge into vendor first-party app subscriptions or bundled first-party CLI entitlements unless that vendor explicitly documents that kind of third-party access.

![LLMUP dashboard](./docs/images/dashboard.png)

The optional local dashboard helps you inspect routing, streaming, cancellation, upstream state, and hook activity while the proxy is running.

## Quick Start

This homepage path shows two upstreams directly:

- official OpenAI API routing to `gpt-5.4`
- MiniMax's OpenAI-compatible endpoint routing to `MiniMax-M2.7-highspeed`

Start from [examples/quickstart-openai-minimax.yaml](./examples/quickstart-openai-minimax.yaml). The file contents are:

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

What those aliases mean:

- `gpt-5-4` is your stable local alias for OpenAI `gpt-5.4`
- `gpt-5-4-mini` is also a local alias; in this example it routes to MiniMax `MiniMax-M2.7-highspeed`

Build and start the proxy:

```bash
git clone https://github.com/lzjever/llm-universal-proxy.git
cd llm-universal-proxy
cargo build --locked --release

export OPENAI_API_KEY="your-openai-key"
export MINIMAX_API_KEY="your-minimax-key"

./target/release/llm-universal-proxy --config examples/quickstart-openai-minimax.yaml
```

Check health:

```bash
curl -fsS http://127.0.0.1:8080/health && echo
```

Try both aliases through the same local OpenAI-style client surface:

```bash
curl http://127.0.0.1:8080/openai/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-5-4",
    "input": "Reply with pong."
  }'
```

```bash
curl http://127.0.0.1:8080/openai/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-5-4-mini",
    "input": "Reply with pong."
  }'
```

Reasoning effort such as `xhigh` is a client/request-side setting, not part of the model name. Keep the alias stable and set reasoning in the request or client config.

## Compatibility Contract

`llmup` gives clients a stable local protocol surface, not unlimited provider equivalence.

- same-protocol paths stay native when possible
- translated paths target a portable core and may warn or reject non-portable provider-native features
- native extensions and provider-owned lifecycle state stay on same-provider paths unless a documented shim says otherwise
- the quickstart includes conservative text-only `surface_defaults`; turn on search, image, or parallel-tool flags only when that model surface really supports them
- typed media metadata must be internally consistent; conflicting MIME hints such as `mime_type` versus a `file_data` data URI are rejected before the upstream call

## Codex / Claude Code / Gemini Basic Setup

For day-to-day usage, prefer the repo's wrapper scripts instead of hand-configuring each client. They handle local environment isolation, base URL injection, and client-specific metadata.

With the quickstart config above, start with `--model gpt-5-4`. Swap to `--model gpt-5-4-mini` when you want the MiniMax lane instead.

### Codex CLI

```bash
bash scripts/run_codex_proxy.sh \
  --proxy-base http://127.0.0.1:8080 \
  --config-source examples/quickstart-openai-minimax.yaml \
  --workspace "$PWD" \
  --model gpt-5-4
```

### Claude Code

```bash
bash scripts/run_claude_proxy.sh \
  --proxy-base http://127.0.0.1:8080 \
  --config-source examples/quickstart-openai-minimax.yaml \
  --workspace "$PWD" \
  --model gpt-5-4
```

### Gemini CLI

```bash
bash scripts/run_gemini_proxy.sh \
  --proxy-base http://127.0.0.1:8080 \
  --config-source examples/quickstart-openai-minimax.yaml \
  --workspace "$PWD" \
  --model gpt-5-4
```

Wrapper base URL and actual proxy endpoint are related but not identical.

For Codex specifically, the wrapper currently fixes `wire_api="responses"`, so Codex uses the Responses route:

| Client | Wrapper-configured base URL | Client appends | Proxy endpoint actually hit |
| --- | --- | --- | --- |
| Codex CLI | `OPENAI_BASE_URL=<proxy>/openai/v1` | `/responses` | `/openai/v1/responses` |
| Claude Code | `ANTHROPIC_BASE_URL=<proxy>/anthropic` | `/v1/messages` | `/anthropic/v1/messages` |
| Gemini CLI | `GOOGLE_GEMINI_BASE_URL=<proxy>/google` | `/v1beta/models/...` | `/google/v1beta/models/...` |

Codex especially benefits from the wrapper because it injects temporary model metadata for proxy-backed aliases. For more detail, see [docs/clients.md](./docs/clients.md).

## Most Common Static Configuration

The static YAML story is intentionally small:

| Field | Purpose |
| --- | --- |
| `listen` | Proxy listen address |
| `upstream_timeout_secs` | Upstream request timeout |
| `upstreams` | Named upstream API roots, formats, and credential policy |
| `model_aliases` | Stable local names mapped to `UPSTREAM:MODEL` |
| `surface_defaults` / `surface` | Optional client-visible capability metadata for wrappers and model catalogs |
| `proxy` | Optional default upstream egress proxy |
| `hooks` | Optional usage / exchange export hooks |
| `debug_trace` | Optional local debug trace |

Practical rules:

- `api_root` should be the provider API root and include its version segment, such as `.../v1` or `.../v1beta`
- `format` pins the upstream protocol: `openai-responses`, `openai-completion`, `anthropic`, or `google`
- aliases such as `gpt-5-4` and `gpt-5-4-mini` are local names; they do not need to equal the upstream model ID
- use structured aliases only when you want extra `limits` or `surface` metadata on top of `target: UPSTREAM:MODEL`

For the full YAML reference and more examples, see [docs/configuration.md](./docs/configuration.md).

## Dynamic Configuration Overview

Static YAML is the default. If you need live updates, the proxy also exposes admin endpoints for reading runtime state and replacing namespace config without restarting the whole process.

Current admin endpoints:

- `GET /admin/state`
- `GET /admin/namespaces/:namespace/state`
- `POST /admin/namespaces/:namespace/config`

That flow is documented in [docs/admin-dynamic-config.md](./docs/admin-dynamic-config.md).

## Keep Reading

- [docs/configuration.md](./docs/configuration.md): static config, alias patterns, YAML reference
- [docs/clients.md](./docs/clients.md): Codex / Claude Code / Gemini wrapper setup and base URL details
- [docs/admin-dynamic-config.md](./docs/admin-dynamic-config.md): admin API, live config, CAS updates
- [docs/protocol-compatibility-matrix.md](./docs/protocol-compatibility-matrix.md): compatibility boundaries and portability summary
- [docs/max-compat-design.md](./docs/max-compat-design.md): deeper translated-path compatibility notes
- [docs/DESIGN.md](./docs/DESIGN.md): current architecture map
- [docs/README.md](./docs/README.md): docs index

## License

MIT License
