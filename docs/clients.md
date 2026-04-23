# Client Setup Guide

This guide explains how to connect Codex CLI, Claude Code, and Gemini CLI to `llmup`.

Use the wrapper scripts first. They are the least fragile path because they isolate local client state, inject the correct base URL, and add client-specific metadata where needed.

The quickstart config used throughout this guide is [examples/quickstart-openai-minimax.yaml](../examples/quickstart-openai-minimax.yaml). Its stable aliases are:

- `gpt-5-4` for OpenAI `gpt-5.4`
- `gpt-5-4-mini` as a local alias that routes to MiniMax `MiniMax-M2.7-highspeed`

If you want the MiniMax lane, swap `--model gpt-5-4` for `--model gpt-5-4-mini`.

## Recommended Path: Start With the Wrapper Scripts

Use the wrapper scripts in `scripts/`:

- `scripts/run_codex_proxy.sh`
- `scripts/run_claude_proxy.sh`
- `scripts/run_gemini_proxy.sh`

Each wrapper supports two modes:

- connect to an already running proxy with `--proxy-base`
- let the wrapper start and stop the proxy for you by omitting `--proxy-base`

If you already have a proxy process running, pass `--proxy-base`. If you omit it, the wrapper starts the proxy, waits for `/health`, launches the client, and stops the proxy when the session exits.

## Basic Client Commands

### Codex CLI

Connect Codex to an already running proxy:

```bash
./scripts/run_codex_proxy.sh \
  --proxy-base http://127.0.0.1:8080 \
  --config-source examples/quickstart-openai-minimax.yaml \
  --workspace "$PWD" \
  --model gpt-5-4
```

Managed mode, where the wrapper starts the proxy for you:

```bash
./scripts/run_codex_proxy.sh \
  --config-source examples/quickstart-openai-minimax.yaml \
  --workspace "$PWD" \
  --model gpt-5-4
```

Codex benefits the most from the wrapper because it fetches live `llmup.surface` metadata from the proxy model catalog and writes the temporary catalog payload from that runtime truth, instead of relying on legacy hard-coded Codex assumptions or the unknown-model fallback path.

### Claude Code

```bash
./scripts/run_claude_proxy.sh \
  --proxy-base http://127.0.0.1:8080 \
  --config-source examples/quickstart-openai-minimax.yaml \
  --workspace "$PWD" \
  --model gpt-5-4
```

Managed mode:

```bash
./scripts/run_claude_proxy.sh \
  --config-source examples/quickstart-openai-minimax.yaml \
  --workspace "$PWD" \
  --model gpt-5-4
```

### Gemini CLI

```bash
./scripts/run_gemini_proxy.sh \
  --proxy-base http://127.0.0.1:8080 \
  --config-source examples/quickstart-openai-minimax.yaml \
  --workspace "$PWD" \
  --model gpt-5-4
```

Managed mode:

```bash
./scripts/run_gemini_proxy.sh \
  --config-source examples/quickstart-openai-minimax.yaml \
  --workspace "$PWD" \
  --model gpt-5-4
```

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

## Manual Wiring Without Wrappers

Wrappers are still recommended, but the underlying client contracts are straightforward if you prefer to wire things by hand.

### Codex

Set:

- `OPENAI_API_KEY=dummy`
- `OPENAI_BASE_URL=<proxy>/openai/v1`

Codex then calls the OpenAI-style surface, typically `POST /openai/v1/responses`.

### Claude Code

Set:

- `ANTHROPIC_API_KEY=dummy`
- `ANTHROPIC_BASE_URL=<proxy>/anthropic`

Claude then appends `/v1/messages`, which lands on `POST /anthropic/v1/messages`.

### Gemini CLI

Set:

- `GEMINI_API_KEY=dummy`
- `GOOGLE_GEMINI_BASE_URL=<proxy>/google`

Gemini then appends `/v1beta/models/...`, which lands on `POST /google/v1beta/models/...`.

Dummy keys are usually enough when the real upstream credential lives on the proxy side and the upstream uses `auth_policy: force_server`.

## Picking Model Names

Clients can use either:

- a stable alias from `model_aliases`, such as `gpt-5-4` or `gpt-5-4-mini`
- an explicit upstream-qualified name such as `OPENAI:gpt-5.4`

Aliases are the better default for day-to-day client use because they decouple the client from provider-specific model IDs.

Reasoning effort such as `xhigh` is still a request-side or client-side setting. Keep that out of the alias name.

## A Good First Setup

If you are new to the project, use this order:

1. start from [examples/quickstart-openai-minimax.yaml](../examples/quickstart-openai-minimax.yaml)
2. start the proxy
3. attach one client with `--model gpt-5-4`
4. confirm the session works
5. switch to `gpt-5-4-mini` only after the first lane is already healthy

For the YAML side, see [Configuration Guide](./configuration.md).

For runtime updates and admin views, see [Admin and Dynamic Config](./admin-dynamic-config.md).
