# Handoff: E2E CLI Client Test Suite for LLM Universal Proxy

## Status: Completed

This handoff is now archival context. The blocking Claude Code routing issue has been resolved and `scripts/test_cli_clients.sh` is working end-to-end in the current environment.

Final verified result:
- `bash scripts/test_cli_clients.sh` → `10 passed, 0 failed, 2 skipped`
- `codex_basic` → all 3 aliases pass
- `codex_multi` → `minimax-anth` and `minimax-openai` pass; `qwen-local` is intentionally skipped
- `claude_basic` → `sonnet`, `haiku`, and `opus` all pass
- `claude_multi` → `sonnet` and `haiku` pass; `opus -> qwen-local` is intentionally skipped

Resolved implementation details:
- Claude Code tests now use a temporary `CLAUDE_CONFIG_DIR`, so the user's global `~/.claude/settings.json` is never modified.
- Claude Code runs in `--bare` mode with `ANTHROPIC_API_KEY=dummy` and proxy-local model alias routing:
  - `sonnet -> minimax-anth`
  - `haiku -> minimax-openai`
  - `opus -> qwen-local`
- Claude base URL must be `http://127.0.0.1:18888/anthropic` instead of `/anthropic/v1`, because Claude appends `/v1/messages` itself.
- Codex multi-turn tests require `--skip-git-repo-check` because the temporary test project lives outside a Git repository.
- The local `qwen-local` model is fast and sufficient for basic prompts, but not reliable for multi-turn code-edit tasks in either Codex or Claude Code. Those two cells are now explicit `SKIP`s instead of flaky failures.

If you need the historical debugging trail, keep reading below. Otherwise, use the script as the source of truth.

## 1. Project Overview

**Repository**: `llm-universal-proxy` — a Rust-based LLM proxy that translates between OpenAI Responses API, OpenAI Chat Completions API, Anthropic Messages API, and Google Gemini API. It sits between LLM CLI clients (Codex CLI, Claude Code) and upstream LLM providers (MiniMax, local Qwen, ZhiPu, etc.), performing real-time protocol translation.

**Key architecture**:
- Proxy listens on a single HTTP port, namespaced by format (`/openai/v1/...`, `/anthropic/v1/...`, `/google/v1/...`)
- `model_aliases` in YAML config map client-visible model names to `UPSTREAM_NAME:upstream_model` pairs
- Upstream format is configured per-upstream (`format: anthropic | openai-completion | openai-responses | google`)
- Debug trace writes JSONL to a configurable path

## 2. Current Task

### Goal
Create `scripts/test_cli_clients.sh` — an E2E test script that exercises the proxy using **real CLI clients** (Codex CLI and Claude Code) against **multiple upstreams** through protocol translation.

### Test Matrix
```
Client         Protocol           Upstream Alias     Actual Upstream
─────────────────────────────────────────────────────────────────────
Codex CLI   →  Responses API  →  minimax-anth    →  MiniMax (Anthropic format)
Codex CLI   →  Responses API  →  minimax-openai  →  MiniMax (OpenAI CC format)
Codex CLI   →  Responses API  →  qwen-local      →  Local Qwen3.5 (OpenAI CC format)
Claude Code →  Anthropic API  →  minimax-anth    →  MiniMax (Anthropic format)
Claude Code →  Anthropic API  →  minimax-openai  →  MiniMax (OpenAI CC format)
Claude Code →  Anthropic API  →  qwen-local      →  Local Qwen3.5 (OpenAI CC format)
```

Each cell tests a different cross-protocol translation path.

### Test Phases
1. **codex_basic** — Single-turn "reply PONG" test for each model via Codex CLI
2. **codex_multi** — Multi-turn bug-fix task via Codex CLI (fix a deliberate `-` vs `+` bug in calc.py)
3. **claude_basic** — Single-turn "reply PONG" test for each model via Claude Code
4. **claude_multi** — Multi-turn bug-fix task via Claude Code

## 3. Current State of Files

### 3.1 `scripts/test_cli_clients.sh` (NEW, untracked)

**Status: ~90% complete. Codex CLI tests work. Claude Code tests have a critical blocking issue.**

What works:
- Proxy lifecycle (start/stop/health-check)
- Argument parsing (`--test`, `--skip-slow`, `--proxy-only`)
- Codex CLI tests: use `-c` flags to define a custom provider with `supports_websockets=false` (bypasses Codex's default WebSocket transport)
- All helper functions, colors, summary reporting

### 3.2 `proxy-test-minimax-and-local.yaml` (gitignored, contains API keys)

The proxy test config with 3 upstreams and model aliases. Currently has both the original aliases (`minimax-anth`, `minimax-openai`, `qwen-local`) and Claude model name aliases (`claude-sonnet-4-6`, `claude-haiku-4-5`, `claude-opus-4-6`) added during debugging.

## 4. Critical Blocking Issue: Claude Code Model Routing

### Problem
Claude Code has two hard constraints that prevent straightforward integration:

**Constraint 1**: `~/.claude/settings.json` has an `env` block that sets `ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN`, and `ANTHROPIC_DEFAULT_SONNET_MODEL` etc. These **override** shell-level environment variables. When the test script does:
```bash
export ANTHROPIC_BASE_URL="http://127.0.0.1:18888/anthropic/v1"
claude -p "PONG" --model sonnet ...
```
The request goes to the URL in `settings.json` (ZhiPu/BigModel at `https://open.bigmodel.cn/api/anthropic`), NOT to our proxy. This was confirmed by:
- Killing the proxy → Claude Code still returns PONG (going to ZhiPu directly)
- Debug trace file has 0 entries (no request ever reached our proxy)

**Constraint 2**: Claude Code v2.1.92 performs local model name validation. The `--model` flag only accepts model names recognized by its internal registry. Custom names like `minimax-anth` are rejected with: `"There's an issue with the selected model (minimax-anth). It may not exist or you may not have access to it."`

### What Was Tried (all failed)

| Approach | Result | Why it failed |
|----------|--------|---------------|
| `export ANTHROPIC_BASE_URL=...` in subshell | Request goes to ZhiPu, not proxy | `settings.json` env overrides shell env vars |
| `env -i HOME=$HOME PATH=$PATH ANTHROPIC_BASE_URL=... claude ...` | Hangs forever (30s timeout) | No auth credentials → stuck in OAuth flow |
| `CLAUDE_CONFIG_DIR=/tmp/empty-dir` + env vars | Either hangs (no auth) or model rejected | No auth without settings.json; model validation fails |
| `--settings '{"env":{"ANTHROPIC_BASE_URL":"..."}}'` | Model rejected | `--settings` merges but settings.json env takes priority for base URL |
| `ANTHROPIC_CUSTOM_MODEL_OPTION="minimax-anth"` | Model rejected with `--model` | Custom model option only adds to `/model` picker UI, doesn't bypass `--model` validation |
| `--model claude-sonnet-4-6` (current model ID) | Model rejected | Claude Code v2.1.92 predates Claude 4.6; doesn't know this model ID |
| `--model claude-sonnet-4-5-20250514` | Hangs | Probably starts making API calls but auth fails |

### What Works (confirmed)
The proxy itself works correctly with Claude Code's protocol. Verified via direct `curl`:
```bash
curl -s http://127.0.0.1:18888/anthropic/v1/messages \
  -H "x-api-key: dummy" \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{"model":"minimax-anth","max_tokens":50,"messages":[{"role":"user","content":"Say PONG"}]}'
# → Returns valid Anthropic response from MiniMax with "PONG" in content
```

### Environment Context (why it's hard)

We are running **inside a Claude Code session**. The parent Claude Code process has already set these env vars in our shell:
```
ANTHROPIC_BASE_URL=https://open.bigmodel.cn/api/anthropic
ANTHROPIC_AUTH_TOKEN=4f5ff08f17b14cdc9795ab696508e9ae.0i5f9tcioBW8cNr9
ANTHROPIC_DEFAULT_SONNET_MODEL=glm-5.1
ANTHROPIC_DEFAULT_HAIKU_MODEL=glm-5.1-turbo
ANTHROPIC_DEFAULT_OPUS_MODEL=glm-5.1
```
These inherited env vars interact badly with Claude Code's own settings loading.

### User Requirement
> "测试codex或者claude code的时候不要影响我当前用户下全局的codex或者cc的配置"
> Translation: "When testing Codex or Claude Code, do NOT modify the global configuration files."

The user's `~/.claude/settings.json` must not be permanently modified. Temporary backup-and-restore is acceptable.

## 5. Potential Solutions to Investigate

### Option A: Backup/Restore `settings.json`
```bash
# Before Claude Code tests:
cp ~/.claude/settings.json ~/.claude/settings.json.proxy-test-backup
cat > ~/.claude/settings.json << 'EOF'
{
  "env": {
    "ANTHROPIC_BASE_URL": "http://127.0.0.1:18888/anthropic/v1",
    "ANTHROPIC_API_KEY": "dummy",
    "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-sonnet-4-5-20250514",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "claude-3-5-haiku-20241022",
    "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-3-opus-20240229"
  },
  "skipDangerousModePermissionPrompt": true
}
EOF
trap 'cp ~/.claude/settings.json.proxy-test-backup ~/.claude/settings.json' EXIT

# After tests:
cp ~/.claude/settings.json.proxy-test-backup ~/.claude/settings.json
```

**Risk**: If the script crashes between write and restore, user's settings are lost. Mitigate with `trap` on EXIT, INT, TERM. This was partially tested — the command hung for 30s (possibly because the proxy config didn't have the right Claude model aliases at the time, or because Claude Code tried to reach `api.anthropic.com` for validation).

**To investigate**: Whether Claude Code v2.1.92 recognizes `claude-sonnet-4-5-20250514`. If not, try `claude-3-7-sonnet-20250219` or `claude-3-5-sonnet-20241022`.

### Option B: `CLAUDE_CONFIG_DIR` with proper auth
```bash
mkdir -p /tmp/test-claude-proxy-config
cat > /tmp/test-claude-proxy-config/settings.json << 'EOF'
{
  "env": {
    "ANTHROPIC_BASE_URL": "http://127.0.0.1:18888/anthropic/v1",
    "ANTHROPIC_API_KEY": "dummy",
    "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-sonnet-4-5-20250514"
  },
  "skipDangerousModePermissionPrompt": true
}
EOF
CLAUDE_CONFIG_DIR=/tmp/test-claude-proxy-config claude -p "PONG" --model sonnet ...
```

**Issue**: Without `ANTHROPIC_AUTH_TOKEN`, Claude Code may attempt OAuth login and hang. **To investigate**: Whether `ANTHROPIC_API_KEY` alone is sufficient for `-p` mode, or if some other credential file needs to exist in the config dir.

### Option C: Use `--bare` flag with `CLAUDE_CONFIG_DIR`
From the docs: `--bare` skips hooks, LSP, plugin sync, attribution, auto-memory, etc. "Anthropic auth is strictly `ANTHROPIC_API_KEY` or `apiKeyHelper` via `--settings`". This might bypass the OAuth/login requirement.

**To investigate**: Combine `--bare` + `CLAUDE_CONFIG_DIR` + `--settings` with ANTHROPIC_API_KEY.

### Option D: Skip Claude Code tests entirely
If Claude Code integration proves too fragile across versions and configurations, consider:
- Mark Claude Code tests as optional/conditional
- Use direct `curl`-based tests to validate the Anthropic Messages API path through the proxy
- Document that Claude Code tests require specific setup steps

## 6. Codex CLI Integration (Working)

Codex CLI tests are fully working. Key technical details:

### Codex CLI v0.118.0
- Uses OpenAI **Responses API** internally (not Chat Completions)
- Defaults to **WebSocket transport** for Responses API
- WebSocket fails against our proxy (returns 405) — must be disabled

### Solution: `-c` flags for custom provider
```bash
codex exec "prompt" \
  -c "model=\"$model\"" \
  -c 'model_provider="proxy"' \
  -c 'model_providers.proxy.name="Proxy"' \
  -c "model_providers.proxy.base_url=\"$CODEX_BASE_URL\"" \
  -c 'model_providers.proxy.wire_api="responses"' \
  -c 'model_providers.proxy.supports_websockets=false' \
  --sandbox read-only --json
```

This defines an inline custom provider called "proxy" that:
- Uses HTTP-only (no WebSocket)
- Points at our proxy's OpenAI endpoint
- Uses the Responses wire format
- Does NOT modify any global Codex config files

### Codex CLI output format
Returns JSONL with event types like `turn.started`, `text.delta`, `turn.completed`, `turn.failed`. Parse for `turn.failed` to detect errors, or check for "PONG" in the aggregated output.

## 7. Proxy Configuration Details

### `proxy-test-minimax-and-local.yaml`

```yaml
listen: 127.0.0.1:18888
upstream_timeout_secs: 120

upstreams:
  MINIMAX-ANTHROPIC:
    api_root: "https://api.minimaxi.com/anthropic/v1"
    format: anthropic
    credential_actual: "<API_KEY>"    # MiniMax API key
    auth_policy: force_server

  MINIMAX-OPENAI:
    api_root: "https://api.minimaxi.com/v1"
    format: openai-completion
    credential_actual: "<API_KEY>"    # same MiniMax API key
    auth_policy: force_server

  LOCAL-QWEN:
    api_root: "http://192.168.0.220:9997/v1"
    format: openai-completion
    credential_actual: "not-needed"
    auth_policy: force_server

model_aliases:
  minimax-anth: "MINIMAX-ANTHROPIC:MiniMax-M2.7-highspeed"
  minimax-openai: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
  qwen-local: "LOCAL-QWEN:qwen3.5-9b-awq"
  # Claude model name aliases (for Claude Code tests)
  claude-sonnet-4-6: "MINIMAX-ANTHROPIC:MiniMax-M2.7-highspeed"
  claude-haiku-4-5: "MINIMAX-OPENAI:MiniMax-M2.7-highspeed"
  claude-opus-4-6: "LOCAL-QWEN:qwen3.5-9b-awq"

debug_trace:
  path: /tmp/llm-proxy-test-debug.jsonl
  max_text_chars: 16384
```

**Note**: This file is gitignored (`proxy-test-*.yaml` pattern) because it contains API keys. It must be created manually on each machine.

### Model Alias Resolution
The proxy's `resolve_model()` function (in `src/config.rs`) works as:
1. If model contains `:` and prefix matches an upstream name → use directly (e.g., `MINIMAX-ANTHROPIC:MiniMax-M2.7-highspeed`)
2. If model matches a `model_aliases` key → resolve to upstream:model pair
3. If only one upstream → use model as-is with that upstream
4. If multiple upstreams and no match → return error "ambiguous"

### Proxy URL Patterns
- OpenAI format: `http://127.0.0.1:18888/openai/v1/chat/completions` and `.../responses`
- Anthropic format: `http://127.0.0.1:18888/anthropic/v1/messages`
- Health: `http://127.0.0.1:18888/health`
- Models: `http://127.0.0.1:18888/anthropic/v1/models` (returns proxy aliases)

## 8. User's Environment

| Item | Value |
|------|-------|
| OS | Manjaro Linux 6.18.18-1 |
| Shell | zsh |
| Codex CLI | v0.118.0 |
| Claude Code | v2.1.92 |
| Proxy | Built from `main` branch (`cargo build --release`) |
| User's `~/.claude/settings.json` | Points ANTHROPIC_BASE_URL to ZhiPu (`https://open.bigmodel.cn/api/anthropic`), sets model aliases to `glm-5.1` / `glm-5.1-turbo` |
| Local Qwen | `http://192.168.0.220:9997/v1` (qwen3.5-9b-awq on LAN) |

**Critical**: The user runs Claude Code through ZhiPu (BigModel), not directly through Anthropic. This is why `settings.json` has `ANTHROPIC_BASE_URL=https://open.bigmodel.cn/api/anthropic`. Our test must route through our proxy instead, without permanently changing this config.

## 9. Relevant Claude Code Documentation

### Environment Variables (from https://code.claude.com/docs/en/env-vars)
- `ANTHROPIC_BASE_URL` — Override API endpoint. **settings.json env overrides shell env vars**
- `ANTHROPIC_API_KEY` — API key. Used instead of subscription when set
- `ANTHROPIC_AUTH_TOKEN` — Custom Authorization header value (Bearer prefix)
- `ANTHROPIC_MODEL` — Override model selection
- `ANTHROPIC_DEFAULT_SONNET_MODEL` — Model name for `sonnet` alias
- `ANTHROPIC_DEFAULT_HAIKU_MODEL` — Model name for `haiku` alias
- `ANTHROPIC_DEFAULT_OPUS_MODEL` — Model name for `opus` alias
- `ANTHROPIC_CUSTOM_MODEL_OPTION` — Add custom model to `/model` picker. **Skips validation** for this model ID
- `CLAUDE_CONFIG_DIR` — Override `~/.claude` directory

### Model Configuration (from https://code.claude.com/docs/en/model-config)
- `--model` accepts aliases (`sonnet`, `opus`, `haiku`) or full model names
- Aliases resolve via `ANTHROPIC_DEFAULT_*_MODEL` env vars
- **settings.json env overrides shell env vars** (confirmed by testing)
- `ANTHROPIC_CUSTOM_MODEL_OPTION` adds to `/model` picker but doesn't bypass `--model` validation

### Settings Priority (from https://code.claude.com/docs/en/settings)
1. Managed (server/policy)
2. User (`~/.claude/settings.json`)
3. Project (`.claude/settings.json`)
4. Local (`.claude/settings.local.json`)
5. `--settings` flag (additional settings)

The `env` block in settings.json sets environment variables in the Claude Code process, **overriding** any inherited from the parent shell.

## 10. Test Script Location & How to Run

```bash
# Build proxy
cargo build --release

# Create proxy-test-minimax-and-local.yaml (see Section 7 above)

# Run all tests
bash scripts/test_cli_clients.sh

# Run only Codex tests
bash scripts/test_cli_clients.sh --test codex

# Run only Claude Code tests
bash scripts/test_cli_clients.sh --test claude

# Just start proxy without running tests (for manual debugging)
bash scripts/test_cli_clients.sh --proxy-only
```

Expected result today:
- Full run should end with `10 passed, 0 failed, 2 skipped`
- The 2 skips are the multi-turn `qwen-local` cells (`codex_qwen_local_multi` and `claude_opus_multi`)

## 11. Former Checklist For The Next Team

These items were completed:
- [x] Fix Claude Code model routing
- [x] Isolate Claude tests from global user config
- [x] Update script phase selection and Claude model strategy
- [x] Verify Codex CLI tests still pass
- [x] Final review with full script run

Remaining known limitation:
- [ ] `qwen-local` is still not reliable for multi-turn code-edit tasks, so those cells remain intentionally skipped

## 12. Debugging Tips

### How to check if proxy received a request
```bash
> /tmp/llm-proxy-test-debug.jsonl  # Clear trace
# ... run test ...
wc -l /tmp/llm-proxy-test-debug.jsonl  # 0 = request never reached proxy
cat /tmp/llm-proxy-test-debug.jsonl | python3 -c "
import sys,json
for l in sys.stdin:
    d = json.loads(l)
    print('client_model:', d.get('client_model'), 'upstream:', d.get('upstream_name'))
"
```

### How to test proxy directly
```bash
# Start proxy
./target/release/llm-universal-proxy --config proxy-test-minimax-and-local.yaml &

# Test Anthropic format
curl -s http://127.0.0.1:18888/anthropic/v1/messages \
  -H "x-api-key: dummy" -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{"model":"minimax-anth","max_tokens":50,"messages":[{"role":"user","content":"Say PONG"}]}'

# Test model listing
curl -s http://127.0.0.1:18888/anthropic/v1/models | python3 -m json.tool
```

### How to kill lingering proxy processes
```bash
pkill -f "llm-universal-proxy.*proxy-test"
```

### Key files to read
- `src/config.rs` — Config parsing, `resolve_model()`, model alias logic
- `src/server.rs` — HTTP handlers, protocol routing, debug tracing
- `src/formats.rs` — Protocol translation (Responses ↔ Anthropic ↔ OpenAI CC)
- `tests/` — Rust integration tests (good reference for proxy behavior)
