# Client Setup Guide

This guide explains how to connect Codex CLI, Claude Code, and Gemini CLI to `llmup`.

Use the wrapper scripts first. They are the least fragile path because they isolate local client state, hydrate provider-neutral preset variables, inject the correct base URL, and add client-specific metadata where needed.

The quickstart config source used throughout this guide is [examples/quickstart-provider-neutral.yaml](../examples/quickstart-provider-neutral.yaml). Its stable aliases are:

- `preset-openai-compatible` for the OpenAI-compatible lane
- `preset-anthropic-compatible` for the Anthropic-compatible lane

MiniMax is an OpenAI-compatible lane when a user chooses it. MiniMax is only a replaceable OpenAI-compatible example, not a GA-required provider and not the main CLI-wrapper preset path. Release/GA live evidence should use provider-neutral compatible configuration rather than treating any named provider as required.

## Preset Environment

Before running a managed wrapper session, export:

```bash
export PRESET_OPENAI_ENDPOINT_BASE_URL="https://openai-compatible.example/v1"
export PRESET_ANTHROPIC_ENDPOINT_BASE_URL="https://anthropic-compatible.example/v1"
export PRESET_ENDPOINT_MODEL="provider-model-id"
export PRESET_ENDPOINT_API_KEY="provider-api-key"
```

`PRESET_OPENAI_ENDPOINT_BASE_URL` is the OpenAI-compatible API root, `PRESET_ANTHROPIC_ENDPOINT_BASE_URL` is the Anthropic-compatible API root, `PRESET_ENDPOINT_MODEL` is the real provider model ID hydrated into both preset aliases, and `PRESET_ENDPOINT_API_KEY` is the server-side provider credential. These placeholders are rendered by the wrapper before proxy startup; a directly loaded static YAML file should use concrete URL and model values.

## Recommended Path: Start With the Wrapper Scripts

Use the wrapper scripts in `scripts/`:

- `scripts/run_codex_proxy.sh`
- `scripts/run_claude_proxy.sh`
- `scripts/run_gemini_proxy.sh`

Each wrapper supports two modes:

- connect to an already running proxy with `--proxy-base`
- let the wrapper start and stop the proxy for you by omitting `--proxy-base`

If you already have a proxy process running, pass `--proxy-base`. If you omit it, the wrapper starts the proxy, waits for `/health`, launches the client, and stops the proxy when the session exits.

Wrapper commands are safe by default: they do not pass no-sandbox, `yolo`, or permission-bypass flags. Disposable local harness runs that intentionally need client-specific bypass behavior must opt in with `--dangerous-harness`.

## Basic Client Commands

### Codex CLI

Managed mode, where the wrapper renders the preset config and starts the proxy:

```bash
./scripts/run_codex_proxy.sh \
  --config-source examples/quickstart-provider-neutral.yaml \
  --workspace "$PWD" \
  --model preset-openai-compatible
```

Connect Codex to an already running proxy:

```bash
./scripts/run_codex_proxy.sh \
  --proxy-base http://127.0.0.1:8080 \
  --config-source examples/quickstart-provider-neutral.yaml \
  --workspace "$PWD" \
  --model preset-openai-compatible
```

Codex benefits the most from the wrapper because it fetches live `llmup.surface` metadata from the proxy model catalog and writes the temporary catalog payload from that runtime truth, instead of relying on legacy hard-coded Codex assumptions or the unknown-model fallback path.

For throwaway harness work only, `--dangerous-harness` allows the wrapper to pass Codex's bypass flag. Leave it off for normal use.

### Claude Code

```bash
./scripts/run_claude_proxy.sh \
  --config-source examples/quickstart-provider-neutral.yaml \
  --workspace "$PWD" \
  --model preset-anthropic-compatible
```

Attach to an existing proxy:

```bash
./scripts/run_claude_proxy.sh \
  --proxy-base http://127.0.0.1:8080 \
  --config-source examples/quickstart-provider-neutral.yaml \
  --workspace "$PWD" \
  --model preset-anthropic-compatible
```

For throwaway harness work only, `--dangerous-harness` allows the wrapper to pass Claude's permission-skip flag. Leave it off for normal use.

### Gemini CLI

```bash
./scripts/run_gemini_proxy.sh \
  --config-source examples/quickstart-provider-neutral.yaml \
  --workspace "$PWD" \
  --model preset-openai-compatible
```

Attach to an existing proxy:

```bash
./scripts/run_gemini_proxy.sh \
  --proxy-base http://127.0.0.1:8080 \
  --config-source examples/quickstart-provider-neutral.yaml \
  --workspace "$PWD" \
  --model preset-openai-compatible
```

For throwaway harness work only, `--dangerous-harness` allows the wrapper to pass Gemini's no-sandbox and `yolo` flags. Leave it off for normal use.

## Client Base URL vs Server Route

The wrapper configures the client base URL, and the client appends its own protocol path on top.

That distinction matters because the values you set in client env vars are not the same string as the server route that eventually receives the request.

For Codex specifically, the wrapper currently fixes `wire_api="responses"`. That means Codex is wired to the Responses surface here, not to Chat Completions.

| Client | Wrapper-configured base URL | What the client appends | Server route that receives the request |
| --- | --- | --- | --- |
| Codex CLI | `OPENAI_BASE_URL=<proxy>/openai/v1` | `/responses` | `/openai/v1/responses` |
| Claude Code | `ANTHROPIC_BASE_URL=<proxy>/anthropic` | `/v1/messages` | `/anthropic/v1/messages` |
| Gemini CLI | `GOOGLE_GEMINI_BASE_URL=<proxy>/google` | `/v1beta/models/...` | `/google/v1beta/models/...` |

That is why the homepage no longer presents one flat endpoint table for manual client setup. For Codex, Claude, and Gemini, the wrapper-level base URL and the server-side route live at different layers.

## Reasoning And Continuity Boundaries

Reasoning effort such as `xhigh` is still a request-side or client-side setting. Keep that out of the alias name.

Responses reasoning/compaction continuity is intentionally bounded for cross-provider routes: default/max_compat may drop an opaque carrier only when visible summary text or visible transcript history remains; strict/balanced fail closed; opaque-only reasoning and opaque-only compaction fail closed; same-provider/native passthrough preserves provider-owned state.

## Manual Wiring Without Wrappers

Wrappers are still recommended, but the underlying client contracts are straightforward if you prefer to wire things by hand.

The release CLI wrapper matrix currently gates the wrapper surface in two deterministic parts: a structure gate that expands the tracked basic matrix for Codex CLI, Claude Code, and Gemini CLI, plus a hermetic scripted interactive Codex wrapper gate. That gate executes `scripts/run_codex_proxy.sh` with a fake Codex binary and fake local proxy for two stdin turns. This is not a full live multi-client/provider matrix; real live client evidence remains final GA/operator validation when those CLIs and provider credentials are available.

In `proxy_key` mode, set each client SDK key below to `$LLM_UNIVERSAL_PROXY_KEY`; the proxy reads the real upstream provider key from `provider_key_env`. In `client_provider_key` mode, set these SDK keys to the real provider key for the selected upstream.

### Codex

Set:

- `OPENAI_API_KEY=$LLM_UNIVERSAL_PROXY_KEY`
- `OPENAI_BASE_URL=<proxy>/openai/v1`

Codex then calls the OpenAI-style surface, typically `POST /openai/v1/responses`.

### Claude Code

Set:

- `ANTHROPIC_API_KEY=$LLM_UNIVERSAL_PROXY_KEY`
- `ANTHROPIC_BASE_URL=<proxy>/anthropic`

Claude then appends `/v1/messages`, which lands on `POST /anthropic/v1/messages`.

### Gemini CLI

Set:

- `GEMINI_API_KEY=$LLM_UNIVERSAL_PROXY_KEY`
- `GOOGLE_GEMINI_BASE_URL=<proxy>/google`

Gemini then appends `/v1beta/models/...`, which lands on `POST /google/v1beta/models/...`.

## Picking Model Names

Clients can use either:

- a stable alias from `model_aliases`, such as `preset-openai-compatible` or `preset-anthropic-compatible`
- an explicit upstream-qualified name such as `PRESET-OPENAI-COMPATIBLE:provider-model-id`

Aliases are the better default for day-to-day client use because they decouple the client from provider-specific model IDs.

## A Good First Setup

If you are new to the project, use this order:

1. start from [examples/quickstart-provider-neutral.yaml](../examples/quickstart-provider-neutral.yaml)
2. export the four `PRESET_*` variables
3. attach Codex or Gemini with `--model preset-openai-compatible`, or Claude Code with `--model preset-anthropic-compatible`
4. confirm the wrapper-managed session works
5. replace the preset endpoints with a concrete provider config only after the provider-neutral path is healthy

For the YAML side, see [Configuration Guide](./configuration.md).

For runtime updates and admin views, see [Admin and Dynamic Config](./admin-dynamic-config.md).
