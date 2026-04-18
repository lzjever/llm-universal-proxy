# LLM Universal Proxy

[English](./README.md)

一个单二进制 HTTP 代理，为大语言模型 API 提供统一接口。它接受多种 LLM API 格式的请求，可以把模型路由到多个命名上游，并在需要时自动处理格式转换。

**让 Codex CLI、Claude Code、Gemini CLI 直接用上 GLM、Kimi、MiniMax 这类原生并不兼容的模型。**

这个代理最有价值的地方，就是把“客户端支持的协议”和“你真正想用的模型协议”解耦开。比如新版 Codex CLI 只支持 OpenAI Responses API，但通过 `llm-universal-proxy`，它仍然可以接入 Anthropic 兼容或 OpenAI Completions 兼容的 coding 模型，例如 GLM、Kimi、MiniMax。

![Proxec dashboard](./docs/images/dashboard.png)

运行时 dashboard 可以直接看到路由、流式请求、取消统计、上游流量和 hook 状态。

![通过 proxec 使用 GLM-5-Turbo 的 Codex CLI](./docs/images/codex-glm5-turbo.png)

这是一张真实的 Codex CLI 使用截图：前端是 Codex，底层实际模型是通过代理路由到 `GLM-5-Turbo`。

## 功能特性

- **多格式支持**：接受 4 种不同的 LLM API 格式请求：
  - Google Gemini
  - Anthropic Claude
  - OpenAI Chat Completions
  - OpenAI Responses API
- **自动发现**：自动检测上游服务支持的格式
- **智能路由**：当客户端格式与上游能力匹配时直接透传（无转换开销）
- **格式转换**：在需要时无缝转换格式
- **流式支持**：同时支持流式和非流式响应
- **并发请求**：异步处理，高性能
- **命名上游**：一个代理实例可同时连接多个上游
- **本地模型别名**：可为任意上游模型暴露一个本地唯一模型名
- **审计 Hooks**：可选异步 `exchange` / `usage` HTTP hooks，用于用量统计和有界的 client-facing 审计导出
- **本地 Debug Trace**：可选本地 JSONL 调试轨迹，用于按 turn 排查协议与响应问题
- **凭证策略**：支持 fallback credential、直接配置 credential，以及强制使用服务端凭证
- **兼容 Codex CLI**：可作为 Responses 兼容入口，前接 Anthropic 兼容上游
- **模型统一层**：可把不同供应商的真实模型，映射成稳定的本地模型名，例如 `opus`、`sonnet`、`haiku`

重要路由边界：
- OpenAI Responses lifecycle 路由只有在代理能从当前请求上下文唯一确定原生 Responses 上游时才会透传。代理不会自行发明 response 到 upstream 的会话映射。

## 这个代理为什么有用

- **给不同供应商建立统一模型命名空间**：你可以把不同来源的模型统一映射成稳定的本地名字，例如 `opus`、`sonnet`、`haiku`，或者团队内部自己的 coding model 名称。这样很多依赖固定模型名的工具会更容易接入。
- **适合 Claude Code 风格的使用方式**：如果你希望上层工具始终使用一组固定模型名，但底层真实模型来自不同厂商，这个代理可以把这层差异收掉。
- **适合新版 Codex CLI**：新版 Codex CLI 只支持 OpenAI Responses API，不再支持 Completions。通过这个代理，Codex 仍然可以使用 Anthropic Messages、OpenAI Chat Completions，或者其他非 Responses 兼容接口。这对接入 GLM、MiniMax、Kimi 这类 coding 能力很强的模型特别有用。
- **跨协议统一入口**：你可以把 Anthropic 兼容、OpenAI 兼容、Gemini 风格的上游统一放到一个接口后面，而不是让每个客户端分别适配多套协议。
- **自带可观测性和数据导出能力**：`usage` hook 可以导出用量统计；`exchange` hook 可以导出 best-effort、受预算约束的 client-facing 捕获；`debug_trace` 则提供本地、按 turn 的轻量调试线索。这样既能做分析、评估和审计，也不会把“完整原始流式归档”强行塞进实时链路。

## 安装

### 下载二进制文件

从 [Releases](https://github.com/lzjever/llm-universal-proxy/releases) 页面下载最新版本。

### 从源码构建

```bash
# 克隆仓库
git clone https://github.com/lzjever/llm-universal-proxy.git
cd llm-universal-proxy

# 构建 release 版本
cargo build --release

# 二进制文件位于 ./target/release/llm-universal-proxy
```

### 使用 Make

```bash
make build        # 构建 release 版本
make test         # 运行所有测试
make run-release  # 构建并以 release 模式运行
```

## 本地 Binary Smoke 脚本

仓库内还提供了一个本地 binary smoke 脚本：[scripts/test_binary_smoke.sh](/home/percy/works/mbos-v1/llm-universal-proxy/scripts/test_binary_smoke.sh)。它会先使用 release 二进制启动代理，再在脚本内部拉起 mock upstream，验证少量高价值的启动与路由路径。它的目标是确认“编译出来的二进制确实能启动并跑通核心入口”，不是替代 Rust 集成测试。

常用方式：

```bash
make test-binary-smoke
```

## 真实客户端矩阵

正式的自动化真实客户端矩阵以 `scripts/real_cli_matrix.py` 为主入口。它会先基于 `proxy-test-minimax-and-local.yaml` 派生一份临时运行时配置，再通过代理驱动真实的 `codex`、`claude`、`gemini` CLI。

常用方式：

```bash
cargo build --release
python3 scripts/real_cli_matrix.py
```

兼容 shim 入口：

```bash
bash scripts/test_cli_clients.sh --list-matrix
```

这个 runner 会负责：
- 使用 runner 管理的 home/config 目录和每次运行单独注入的环境变量隔离用户全局配置，避免改写你平时使用的 Codex、Claude Code、Gemini CLI 配置；其中 Gemini 会在 reports root 下复用一份由 runner 管理的 home/cache，而不是回退到你平时的用户目录。
- 在 `test-reports/cli-matrix/<timestamp>/` 下输出带时间戳的矩阵报告目录，并在运行结束时打印最终路径。
- 使用 `--list-matrix` 列出 cases，使用 `--case <case-id>` 精确挑选指定行（可重复传入），使用 `--skip-slow` 跳过长时程任务，使用 `--proxy-only` 只启动代理并等待。
- 可通过 `python3 scripts/real_cli_matrix.py --help` 查看当前 checkout 支持的完整参数集合。

兼容层说明：
- `scripts/test_cli_clients.sh` 是给旧流程和包装脚本保留的兼容 shim。它会直接转发到 `scripts/real_cli_matrix.py`，所以两种入口支持同一组参数。
- 常见入口示例：

```bash
python3 scripts/real_cli_matrix.py --list-matrix
python3 scripts/real_cli_matrix.py --case <case-id>
bash scripts/test_cli_clients.sh --skip-slow
bash scripts/test_cli_clients.sh --proxy-only
```

说明：
- `proxy-test-minimax-and-local.yaml` 是这套矩阵的源配置。runner 会从它派生临时运行时配置，而不是原地修改该文件。
- `.env.test` 只是可选的本地输入文件，不应提交到仓库。若文件存在，runner 只会把它加载到代理子进程中；这些变量不会变成持久 shell 环境，也不会被当成用户全局 CLI 配置源。需要时可用 `--env-file` 指向其他 dotenv 文件。
- `qwen-local` 属于可选覆盖项。只有同时配置了 `LOCAL_QWEN_BASE_URL` 和 `LOCAL_QWEN_MODEL` 时，这条 lane 才会启用；否则会被跳过。即使启用，默认矩阵也只把它用于 smoke 覆盖，并会排除长时程代码编辑类 fixture。
- 这套矩阵用于验证“真实 CLI 端到端行为”。如果你只想验证更低层的协议/HTTP 路径，而不启动真实 CLI，请使用下面介绍的 `scripts/real_endpoint_matrix.py`。

## 配置

代理通过 YAML 文件配置，并通过 `--config` 指定：

```yaml
listen: 0.0.0.0:8080
upstream_timeout_secs: 120

upstreams:
  GLM-OFFICIAL:
    api_root: https://open.bigmodel.cn/api/anthropic/v1
    format: anthropic
    credential_env: GLM_APIKEY
    auth_policy: client_or_fallback

  OPENAI:
    api_root: https://api.openai.com/v1
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

debug_trace:
  path: /tmp/llm-proxy-debug.jsonl
  max_text_chars: 16384
```

说明：
- `api_root` 应直接填写官方上游 API root，并且必须包含版本号，例如 `https://api.openai.com/v1`、`https://api.anthropic.com/v1`、`https://generativelanguage.googleapis.com/v1beta`。
- 代理对外只提供按协议分 namespace 的正式 API：`/openai/v1/...`、`/anthropic/v1/...`、`/google/v1beta/...`。旧的混合 `/v1/...` 路由刻意不再提供。
- Anthropic 兼容上游通常要求 `x-api-key` 和 `anthropic-version`。代理会优先透传客户端鉴权头；若客户端没有提供，可回退到该上游配置的 `credential_env`，并会为 Anthropic 上游默认补上 `anthropic-version: 2023-06-01`。
- 服务商特定静态头应配置在 `upstreams` 中对应上游的 `headers` 字段里。
- `credential_env` 表示“去哪个环境变量读取该上游的 fallback credential”，密钥本身不写进 YAML。
- `credential_actual` 可用于直接在 YAML 中写 fallback credential；它与 `credential_env` 互斥。
- `auth_policy` 支持 `client_or_fallback` 和 `force_server`。
- hooks 是异步 best-effort 模式。通常只开 `usage` 就够；`exchange` 会在请求结束后上报 client-facing 请求和最终响应形状，但流式 capture 是有界的，必要时会截断。
- `debug_trace` 会把按 turn 的请求尾部增量和归一化响应摘要写入本地 JSONL，适合交互式排障，不适合长期原始流量归档。
- 对流式响应，`debug_trace` 记录的是客户端可见结果的聚合摘要，例如文本、reasoning、tool call、terminal event、finish reason 和归一化错误，而不是原始 SSE 逐行镜像。

## Admin 控制面

admin 路由与数据面是显式分离的：

- admin 路由统一位于 `/admin/...`
- admin 路由**不会**继承代理对数据面的全局 CORS 策略
- `/openai/v1/...` 这类数据面路由仍保留面向浏览器客户端的宽松 CORS

当前 admin 访问策略：

- 如果设置了环境变量 `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN`，所有 admin 请求都必须带 `Authorization: Bearer <token>`
- 如果没有设置 `LLM_UNIVERSAL_PROXY_ADMIN_TOKEN`，则只允许 loopback 客户端访问 admin（`127.0.0.1` / `::1`）

当前 admin 接口：

- `GET /admin/state`
- `GET /admin/namespaces/:namespace/state`
- `POST /admin/namespaces/:namespace/config`

读写模型边界：

- `POST /admin/namespaces/:namespace/config` 继续沿用现有 runtime config 写入形状
- admin 读接口使用单独的 redacted view model，不会直接序列化内部 `Config`
- admin state 响应绝不会明文返回上游 `fallback_credential_actual` 或 hook `authorization`
- 对应位置会改为布尔标记，例如 `fallback_credential_configured` 和 `authorization_configured`

示例：

```json
{
  "namespace": "demo",
  "revision": "rev-1",
  "config": {
    "upstreams": [
      {
        "name": "default",
        "fallback_credential_env": "OPENAI_API_KEY",
        "fallback_credential_configured": true
      }
    ],
    "hooks": {
      "exchange": {
        "url": "https://example.com/hooks/exchange",
        "authorization_configured": true
      }
    }
  }
}
```

### 完整 YAML 参考

```yaml
listen: 0.0.0.0:8080
upstream_timeout_secs: 120

upstreams:
  UPSTREAM_NAME:
    api_root: https://example.com/v1
    format: anthropic
    credential_env: EXAMPLE_API_KEY
    # credential_actual: sk-xxx
    auth_policy: client_or_fallback
    headers:
      x-example-header: example-value

model_aliases:
  local-model-name: UPSTREAM_NAME:real-upstream-model

hooks:
  max_pending_bytes: 104857600
  timeout_secs: 30
  failure_threshold: 3
  cooldown_secs: 300
  usage:
    url: https://example.com/hooks/usage
    authorization: Bearer usage-hook-token
  exchange:
    url: https://example.com/hooks/exchange
    authorization: Bearer exchange-hook-token

debug_trace:
  path: /tmp/llm-proxy-debug.jsonl
  max_text_chars: 16384
```

### 顶层字段

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `listen` | string | 否 | `0.0.0.0:8080` | 代理监听地址，格式为 `host:port` |
| `upstream_timeout_secs` | integer | 否 | `120` | 请求上游时的 HTTP 超时 |
| `upstreams` | map | 是 | 无 | 命名上游配置 |
| `model_aliases` | map | 否 | 空 | 把本地模型名映射到 `upstream:model` |
| `hooks` | object | 否 | 关闭 | 可选的异步审计与用量导出 hooks |
| `debug_trace` | object | 否 | 关闭 | 可选的本地 JSONL 调试轨迹，记录按 turn 的请求/响应摘要 |

### `debug_trace`

当你需要在本机排查客户端、代理或协议转换问题，但又不想打开完整 exchange capture 时，可以使用 `debug_trace`。

```yaml
debug_trace:
  path: /tmp/llm-proxy-debug.jsonl
  max_text_chars: 16384
```

设计边界：
- request entry 只记录当前 turn 新增的输入尾部，不会每次都把完整历史对话重写一遍。
- response entry 记录的是归一化摘要。
  - 非流式：最终响应体的摘要。
  - 流式：聚合后的文本、reasoning 文本、tool-call 增量、terminal event、finish reason，以及归一化错误信息。
- 它记录的是客户端可见语义结果，不是原始 SSE 逐行日志。
- writer 对实时请求路径保持非阻塞；如果本地写入跟不上，会写出显式 overflow 记录，而不是阻塞 teardown。

### `upstreams`

`upstreams` 是一个以“上游名字”为 key 的 YAML 对象：

```yaml
upstreams:
  GLM-OFFICIAL:
    api_root: https://open.bigmodel.cn/api/anthropic/v1
    format: anthropic
    credential_env: GLM_APIKEY
```

每个 upstream 支持这些字段：

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `api_root` | string | 是 | 无 | 包含版本号的官方上游 API root |
| `format` | enum | 否 | 自动探测 | 固定指定上游协议格式 |
| `credential_env` | string | 否 | 无 | 指向 fallback credential 的环境变量名 |
| `credential_actual` | string | 否 | 无 | 直接写在 YAML 里的 fallback credential |
| `auth_policy` | enum | 否 | `client_or_fallback` | 控制是否接受客户端传入的认证信息 |
| `headers` | map<string,string> | 否 | 空 | 注入到该上游每个请求的静态头 |

规则：
- `credential_env` 和 `credential_actual` 互斥。
- 如果使用 `auth_policy: force_server`，则该 upstream 必须配置 `credential_env` 或 `credential_actual`。
- `headers` 是按 upstream 单独配置，不是全局配置。

#### `format` 枚举

允许的值：

| 值 | 含义 |
|----|------|
| `openai-completion` | OpenAI Chat Completions 风格上游 |
| `openai-responses` | OpenAI Responses 风格上游 |
| `anthropic` | Anthropic Messages 风格上游 |
| `google` | Google Gemini GenerateContent / streamGenerateContent 风格上游 |
| `responses` | `openai-responses` 的别名 |

如果省略，代理会主动探测该上游支持哪些格式。

#### `auth_policy` 枚举

| 值 | 含义 |
|----|------|
| `client_or_fallback` | 优先使用客户端传入的认证；如果客户端没传，再使用上游 fallback credential |
| `force_server` | 忽略客户端传入的认证，只使用上游 fallback credential |

### `model_aliases`

`model_aliases` 用于把稳定的本地模型名映射到一个具体上游模型：

```yaml
model_aliases:
  sonnet: ANTHROPIC:claude-sonnet-4
  coder-fast: GLM-OFFICIAL:GLM-4.5-Air
```

规则：
- key：暴露给客户端的本地模型名
- value：`UPSTREAM_NAME:REAL_MODEL_NAME`
- 本地模型名应保持唯一
- 如果配置了多个上游，而客户端请求了一个未映射的裸模型名，代理会返回 `400`

### `hooks`

`hooks` 用于配置可选的异步 HTTP 审计和统计导出。

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `max_pending_bytes` | integer | 否 | `104857600` | 待发送 hook payload 的预算上限；同时也是流式 `exchange` capture 的有界预算 |
| `timeout_secs` | integer | 否 | `30` | hook HTTP 请求超时 |
| `failure_threshold` | integer | 否 | `3` | 连续失败多少次后进入 cooldown |
| `cooldown_secs` | integer | 否 | `300` | 熔断后等待多久再尝试恢复 |
| `usage` | object | 否 | 关闭 | 用量导出 hook |
| `exchange` | object | 否 | 关闭 | 有界的 request/response 导出 hook |

Hook 行为：
- hooks 是异步、best-effort 的。
- `usage` 通常就足够做计费或观测。
- `exchange` 会在请求完成后导出 client-facing 请求和 client-facing 响应快照。对非流式请求，通常就是最终响应体；对流式请求，则是处理后的客户端可见结果的有界 capture，而不是原始 SSE 逐行镜像。
- 如果流式 `exchange` capture 超出预算，或者后台 capture 路径发生 overflow，hook 仍会 best-effort 送出，但 `response.body` 会变成显式的截断/不可用标记，例如 `capture_truncated` / `capture_unavailable`，而不是继续回放完整 body。
- 当待发送 hook payload 总大小超过 `max_pending_bytes` 时，新 hook payload 会被丢弃，直到压力下降。
- `usage` 和 `exchange` 各自有独立熔断器；连续失败到达 `failure_threshold` 后，会暂停 `cooldown_secs`。

#### `hooks.usage` 与 `hooks.exchange`

这两个 endpoint 都支持：

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `url` | string | 是 | 无 | 接收 hook payload 的 HTTP/HTTPS 地址 |
| `authorization` | string | 否 | 无 | 可选的 `Authorization` 请求头值 |

## 使用方法

### 多上游示例

```bash
cat > proxy.yaml <<'YAML'
listen: 0.0.0.0:8080
upstream_timeout_secs: 120

upstreams:
  GLM-OFFICIAL:
    api_root: https://open.bigmodel.cn/api/anthropic/v1
    format: anthropic
    credential_env: GLM_APIKEY

  OPENAI:
    api_root: https://api.openai.com/v1
    format: openai-responses
    credential_env: OPENAI_API_KEY

model_aliases:
  GLM-5: GLM-OFFICIAL:GLM-5
  gpt-4o: OPENAI:gpt-4o
YAML

export GLM_APIKEY="你的 GLM Key"
export OPENAI_API_KEY="你的 OpenAI Key"

./llm-universal-proxy --config proxy.yaml
```

客户端随后可以用两种方式选模型：
- 显式上游选择：`GLM-OFFICIAL:GLM-5`
- 本地别名：`GLM-5`

如果配置了多个上游，而模型既不是显式 `上游名:模型名`，也不是已配置的本地 alias，代理会返回 `400`。

### 稳定的本地模型命名

一个很实用的模式是：对外暴露一层与供应商无关的本地模型名，把真实的厂商模型 ID 隐藏在后面：

```yaml
model_aliases:
  opus: ANTHROPIC:claude-opus-4-1
  sonnet: ANTHROPIC:claude-sonnet-4
  haiku: ANTHROPIC:claude-haiku-4
  coder-fast: GLM-OFFICIAL:GLM-4.5-Air
  coder-strong: KIMI:kimi-k2
```

这样客户端只需要请求 `opus`、`sonnet`、`haiku`、`coder-fast`、`coder-strong`，不需要关心底层到底接的是哪家模型。

### 通过 Codex CLI 使用 Anthropic 兼容上游

这是一个真实可用的场景：客户端是 Codex CLI，只会发 OpenAI Responses API；真实上游却是 Anthropic Messages 兼容接口。

1. 先启动代理，指向 Anthropic 兼容上游：

```bash
cat > codex-proxy.yaml <<'YAML'
listen: 127.0.0.1:8099

upstreams:
  GLM-OFFICIAL:
    api_root: https://open.bigmodel.cn/api/anthropic/v1
    format: anthropic
    credential_env: GLM_APIKEY
    auth_policy: client_or_fallback

model_aliases:
  GLM-5: GLM-OFFICIAL:GLM-5
YAML

./target/release/llm-universal-proxy --config codex-proxy.yaml
```

2. 再让 Codex CLI 指向本地代理，并使用隔离配置：

```bash
HOME="$(mktemp -d)" GLM_APIKEY="你的真实 Key" codex exec --ephemeral \
  -c 'model="GLM-5"' \
  -c 'model_provider="glm-proxy"' \
  -c 'model_providers.glm-proxy.name="GLM Proxy"' \
  -c 'model_providers.glm-proxy.base_url="http://127.0.0.1:8099/openai/v1"' \
  -c 'model_providers.glm-proxy.env_key="GLM_APIKEY"' \
  -c 'model_providers.glm-proxy.wire_api="responses"' \
  'Reply with exactly: codex-ok'
```

说明：
- 这里用了临时 `HOME` 和 `--ephemeral`，不会污染你全局的 Codex CLI 配置。
- 客户端访问的是代理的 `/openai/v1/responses`；代理会先把本地模型名 `GLM-5` 解析成 `GLM-OFFICIAL:GLM-5`，再转换成 Anthropic Messages 发给上游。
- 如果上游还需要额外静态协议头，可以在对应 upstream 条目里配置 `headers`。

### 隔离的 CLI Smoke 测试

下面这些模式可以让你在不碰用户级配置的前提下，用真实 CLI 通过代理做联调。所有示例都使用临时 `HOME` 和占位符密钥。

先启动代理：

```yaml
listen: 127.0.0.1:18129
upstream_timeout_secs: 120

upstreams:
  GLM-ANTHROPIC:
    api_root: https://open.bigmodel.cn/api/anthropic/v1
    format: anthropic
    credential_env: GLM_APIKEY
    auth_policy: force_server

  GLM-OPENAI:
    api_root: https://open.bigmodel.cn/api/coding/paas/v4
    format: openai-completion
    credential_env: GLM_APIKEY
    auth_policy: force_server

model_aliases:
  claude-local: GLM-ANTHROPIC:GLM-5
  codex-local: GLM-OPENAI:glm-4.7
  gemini-local: GLM-OPENAI:glm-4.7
```

启动命令：

```bash
GLM_APIKEY="your-real-key" ./target/release/llm-universal-proxy --config proxec-test.yaml
```

Codex CLI 通过 `/openai/v1`：

```bash
HOME="$(mktemp -d)" GLM_APIKEY=dummy codex exec --ephemeral \
  -C /path/to/llm-universal-proxy \
  -c 'model="codex-local"' \
  -c 'model_provider="proxec"' \
  -c 'model_providers.proxec.name="proxec"' \
  -c 'model_providers.proxec.base_url="http://127.0.0.1:18129/openai/v1"' \
  -c 'model_providers.proxec.env_key="GLM_APIKEY"' \
  -c 'model_providers.proxec.wire_api="responses"' \
  'Reply with exactly: codex-ok'
```

Claude Code 通过 `/anthropic/v1`：

```bash
HOME="$(mktemp -d)" \
ANTHROPIC_API_KEY=dummy \
ANTHROPIC_BASE_URL='http://127.0.0.1:18129/anthropic' \
claude --print --output-format text --no-session-persistence \
  --model claude-local \
  'Reply with exactly: claude-ok'
```

Gemini CLI 通过 `/google/v1beta`：

```bash
HOME="$(mktemp -d)" \
GEMINI_API_KEY=dummy \
GOOGLE_GEMINI_BASE_URL='http://127.0.0.1:18129/google' \
HTTP_PROXY= HTTPS_PROXY= http_proxy= https_proxy= \
NO_PROXY='127.0.0.1,localhost' no_proxy='127.0.0.1,localhost' \
gemini --prompt 'Reply with exactly: gemini-ok' \
  --model gemini-local \
  --sandbox=false \
  --output-format text
```

说明：
- 因为这里配置了 `auth_policy: force_server`，所以真正使用的是代理侧配置的上游凭证；客户端传入的 dummy key 只是为了满足各个 CLI 自身的校验。
- 当代理跑在 `127.0.0.1` 时，Gemini CLI 建议显式清空代理环境变量；某些 Node 代理栈否则会尝试把本地流量也走全局 HTTP 代理。
- 把 `/path/to/llm-universal-proxy` 替换成你的实际仓库路径；如果你已经在仓库目录里执行，也可以直接去掉 `-C`。

### 真实上游 Smoke 矩阵

仓库里还带了一个更低层的协议/HTTP smoke 脚本，可通过代理联调 Anthropic 兼容和 OpenAI 兼容上游，但不会启动真实 CLI：

```bash
GLM_APIKEY="你的真实 Key" python3 scripts/real_endpoint_matrix.py
```

覆盖的客户端入口包括：
- `/openai/v1/chat/completions`
- `/openai/v1/responses`
- `/anthropic/v1/messages`

同时验证：
- 非流式路径
- 流式路径
- Anthropic 兼容上游
- OpenAI 兼容上游

### Docker

```bash
# 构建镜像
docker build -t llm-universal-proxy .

# 运行容器
docker run -p 8080:8080 \
  -v "$PWD/proxy.yaml:/app/proxy.yaml:ro" \
  llm-universal-proxy \
  --config /app/proxy.yaml
```

### API 端点

| 端点 | 描述 |
|------|------|
| `POST /openai/v1/chat/completions` | OpenAI Chat Completions 视图 |
| `POST /openai/v1/responses` | OpenAI Responses 视图 |
| `GET /openai/v1/responses/{response_id}` | OpenAI Responses 查询视图 |
| `DELETE /openai/v1/responses/{response_id}` | OpenAI Responses 删除视图 |
| `POST /openai/v1/responses/{response_id}/cancel` | OpenAI Responses 取消视图 |
| `POST /openai/v1/responses/compact` | OpenAI Responses compact 视图 |
| `GET /openai/v1/models` | OpenAI 兼容本地模型目录 |
| `GET /openai/v1/models/{id}` | OpenAI 兼容本地模型详情 |
| `POST /anthropic/v1/messages` | Anthropic Messages 视图 |
| `GET /anthropic/v1/models` | Anthropic 兼容本地模型目录 |
| `GET /anthropic/v1/models/{id}` | Anthropic 兼容本地模型详情 |
| `GET /google/v1beta/models` | Gemini 兼容本地模型目录 |
| `GET /google/v1beta/models/{id}` | Gemini 兼容本地模型详情 |
| `POST /google/v1beta/models/{model}:generateContent` | Gemini GenerateContent 视图 |
| `POST /google/v1beta/models/{model}:streamGenerateContent` | Gemini 流式视图 |
| `GET /health` | 健康检查（返回 `{"status":"ok"}`） |

### 示例请求

#### OpenAI Chat Completions 格式

```bash
curl http://localhost:8080/openai/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -d '{
    "model": "gpt-4",
    "messages": [{"role": "user", "content": "Hello!"}],
    "stream": false
  }'
```

#### Anthropic Claude 格式

```bash
curl http://localhost:8080/anthropic/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: YOUR_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "claude-3-opus-20240229",
    "messages": [{"role": "user", "content": "Hello!"}],
    "max_tokens": 1024
  }'
```

#### Google Gemini 格式

```bash
curl "http://localhost:8080/google/v1beta/models/gemini-local:generateContent" \
  -H "Content-Type: application/json" \
  -d '{
    "contents": [{"parts": [{"text": "Hello!"}]}]
  }'
```

## 工作原理

1. **格式检测**：分析请求路径和请求体来确定客户端的 API 格式
2. **能力发现**：探测上游服务以确定支持的格式
3. **智能路由**：
   - 如果客户端格式与上游匹配 → **透传**（零开销）
   - 如果格式不同 → **转换**（使用 OpenAI Chat Completions 作为中间格式）
4. **流式支持**：处理 SSE 流并逐块转换

## 架构

```
                    ┌──────────────────────┐
                    │   LLM Universal      │
   客户端请求       │       Proxy          │   上游请求
   (任意格式) ────▶│                      │──────────────────▶
                    │  ┌────────────────┐  │   (按需转换)
                    │  │   检测模块      │  │
                    │  └───────┬────────┘  │
                    │          │           │
                    │  ┌───────▼────────┐  │
                    │  │   转换模块      │  │
                    │  └───────┬────────┘  │
                    │          │           │
                    │  ┌───────▼────────┐  │
                    │  │   上游客户端    │  │
                    │  └────────────────┘──┼──────▶ OpenAI / Anthropic / Google
                    └──────────────────────┘
```

## 支持的格式转换

| 从 → 到 | OpenAI | Anthropic | Gemini |
|---------|--------|-----------|--------|
| OpenAI | ✅ 透传 | ✅ 转换 | ✅ 转换 |
| Anthropic | ✅ 转换 | ✅ 透传 | ✅ 转换 |
| Gemini | ✅ 转换 | ✅ 转换 | ✅ 透传 |

## 开发

```bash
# 运行测试
cargo test

# 构建 release 二进制并运行本地 binary smoke
make test-binary-smoke

# 生成详细测试报告
make test-report

# 代码检查
cargo clippy --all-targets --all-features -- -D warnings

# 格式化代码
cargo fmt --all -- --check
```

## 许可证

MIT License
