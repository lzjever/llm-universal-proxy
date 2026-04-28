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

The GA user-entry path is provider-neutral and starts with the CLI wrappers. The recommended config source is [examples/quickstart-provider-neutral.yaml](./examples/quickstart-provider-neutral.yaml), using these stable local aliases:

- `preset-openai-compatible` for the OpenAI-compatible lane
- `preset-anthropic-compatible` for the Anthropic-compatible lane

MiniMax is only a replaceable OpenAI-compatible example, not a GA-required provider and not the mainline preset name. A concrete OpenAI + MiniMax sample remains in [examples/quickstart-openai-minimax.yaml](./examples/quickstart-openai-minimax.yaml) for users who want to replace the preset placeholders with named providers.

The provider-neutral config source is:

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

Set the preset environment variables before starting a wrapper-managed session:

```bash
git clone https://github.com/agentsmith-project/llm-universal-proxy.git
cd llm-universal-proxy
cargo build --locked --release

export PRESET_OPENAI_ENDPOINT_BASE_URL="https://openai-compatible.example/v1"
export PRESET_ANTHROPIC_ENDPOINT_BASE_URL="https://anthropic-compatible.example/v1"
export PRESET_ENDPOINT_MODEL="provider-model-id"
export PRESET_ENDPOINT_API_KEY="provider-api-key"
export LLM_UNIVERSAL_PROXY_AUTH_MODE=proxy_key
export LLM_UNIVERSAL_PROXY_KEY="local-proxy-key"
```

What those variables do:

| Variable | Used for |
| --- | --- |
| `PRESET_OPENAI_ENDPOINT_BASE_URL` | API root for the OpenAI-compatible upstream, including its version segment such as `/v1` |
| `PRESET_ANTHROPIC_ENDPOINT_BASE_URL` | API root for the Anthropic-compatible upstream |
| `PRESET_ENDPOINT_MODEL` | Provider model ID hydrated into both preset aliases |
| `PRESET_ENDPOINT_API_KEY` | Server-side provider credential used by both preset upstreams |
| `LLM_UNIVERSAL_PROXY_AUTH_MODE` | Required proxy auth mode; use `proxy_key` when the proxy holds provider keys |
| `LLM_UNIVERSAL_PROXY_KEY` | Required in `proxy_key` mode; client SDK API keys must use this value |

The `PRESET_*` values are a wrapper/config-source contract. The wrappers hydrate them into a concrete runtime config before starting the proxy. If you run `llm-universal-proxy --config` directly, replace the placeholders with concrete URLs and model names first.

Reasoning effort such as `xhigh` is a client/request-side setting, not part of the model name. Keep the alias stable and set reasoning in the request or client config.

## Compatibility Contract

`llmup` gives clients a stable local protocol surface, not unlimited provider equivalence.

- same-provider/native passthrough preserves provider-native fields and lifecycle state
- compatible same-protocol lanes promise portable core/portable fields only; they are not native provider passthrough
- translated paths target a portable core and may warn or reject non-portable provider-native features
- native extensions and provider-owned lifecycle state stay on same-provider/native paths unless a documented shim says otherwise
- Responses reasoning/compaction continuity is mode-bound: default/max_compat may drop an opaque carrier only when visible summary text or visible transcript history remains; strict/balanced fail closed; opaque-only reasoning and opaque-only compaction fail closed; same-provider/native passthrough preserves provider-owned state
- the quickstart includes conservative text-only `surface_defaults`; turn on search, image, or parallel-tool flags only when that model surface really supports them
- multimodal `surface.modalities.input` gates media types, not every source transport; HTTP(S) image/PDF URLs are distinct from provider or local URIs such as `gs://`, `s3://`, and `file://`
- Gemini `inlineData` can be preserved when translating to OpenAI Chat/Responses, but all Gemini `fileData.fileUri` sources currently fail closed until an explicit fetch/upload adapter exists
- typed media metadata must be internally consistent; conflicting MIME hints such as `mime_type` versus a `file_data` data URI are rejected before the upstream call

## Codex / Claude Code / Gemini Basic Setup

For day-to-day usage, prefer the repo's wrapper scripts instead of hand-configuring each client. They handle local environment isolation, base URL injection, preset hydration, and client-specific metadata.

The defaults in `scripts/interactive_cli.py` match the provider-neutral preset names:

| Client | Default wrapper model |
| --- | --- |
| Codex CLI | `preset-openai-compatible` |
| Claude Code | `preset-anthropic-compatible` |
| Gemini CLI | `preset-openai-compatible` |

### Codex CLI

```bash
bash scripts/run_codex_proxy.sh \
  --config-source examples/quickstart-provider-neutral.yaml \
  --workspace "$PWD" \
  --model preset-openai-compatible
```

### Claude Code

```bash
bash scripts/run_claude_proxy.sh \
  --config-source examples/quickstart-provider-neutral.yaml \
  --workspace "$PWD" \
  --model preset-anthropic-compatible
```

### Gemini CLI

```bash
bash scripts/run_gemini_proxy.sh \
  --config-source examples/quickstart-provider-neutral.yaml \
  --workspace "$PWD" \
  --model preset-openai-compatible
```

Pass `--proxy-base http://127.0.0.1:8080` when you want to attach to a proxy you started separately. When `--proxy-base` is omitted, the wrapper renders the preset config, starts the proxy, waits for `/health`, launches the client, and stops the proxy when the session exits.

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
- aliases such as `preset-openai-compatible` and `preset-anthropic-compatible` are local names; they do not need to equal the upstream model ID
- use structured aliases only when you want extra `limits` or `surface` metadata on top of `target: UPSTREAM:MODEL`
- the provider-neutral `PRESET_*` placeholders are for wrapper-rendered config sources; direct static YAML should contain concrete URLs and model IDs
- `LLM_UNIVERSAL_PROXY_AUTH_MODE` is a process-wide setting for all data-plane routes, not a per-upstream YAML or API field; see the static examples in [docs/configuration.md](./docs/configuration.md) and the runtime payload mapping in [docs/admin-dynamic-config.md](./docs/admin-dynamic-config.md)

For the full YAML reference and more examples, see [docs/configuration.md](./docs/configuration.md).

## Container Image

Release images are published at `ghcr.io/agentsmith-project/llm-universal-proxy`.
The current published container release is `v0.2.24`; Cargo package version
`0.2.25` is the next release identity, not a published container tag yet. For
production, pin `ghcr.io/agentsmith-project/llm-universal-proxy:v0.2.24` or the
published digest instead of relying on `latest`. Container usage, Docker
Compose, one-minute smoke verification, Admin Dashboard auth boundaries, and
GHCR access for authenticated or public pulls are documented in
[docs/container.md](./docs/container.md).

## Dynamic Configuration Overview

Static YAML is the default. If you need live updates, the proxy also exposes admin endpoints for reading runtime state and replacing namespace config without restarting the whole process. Admin payloads use a runtime shape, and the data-plane auth mode still comes from the process environment.

Current admin endpoints:

- `GET /admin/state`
- `GET /admin/namespaces/:namespace/state`
- `POST /admin/namespaces/:namespace/config`

That flow is documented in [docs/admin-dynamic-config.md](./docs/admin-dynamic-config.md).

## Keep Reading

- [docs/configuration.md](./docs/configuration.md): static config, alias patterns, YAML reference
- [docs/clients.md](./docs/clients.md): Codex / Claude Code / Gemini wrapper setup and base URL details
- [docs/container.md](./docs/container.md): GHCR image usage, Docker Compose, container smoke, and release policy
- [docs/admin-dynamic-config.md](./docs/admin-dynamic-config.md): admin API, live config, CAS updates
- [docs/ga-readiness-review.md](./docs/ga-readiness-review.md): GA scope, release evidence, and compatibility boundaries
- [docs/protocol-compatibility-matrix.md](./docs/protocol-compatibility-matrix.md): compatibility boundaries and portability summary
- [docs/max-compat-design.md](./docs/max-compat-design.md): deeper translated-path compatibility notes
- [docs/DESIGN.md](./docs/DESIGN.md): current architecture map
- [docs/README.md](./docs/README.md): docs index

## License

MIT License
