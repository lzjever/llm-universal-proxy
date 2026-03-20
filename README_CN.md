# LLM Universal Proxy

[English](./README.md)

一个单二进制 HTTP 代理，为大语言模型 API 提供统一接口。它接受多种 LLM API 格式的请求，可以把模型路由到多个命名上游，并在需要时自动处理格式转换。

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
- **审计 Hooks**：可选异步 `exchange` / `usage` HTTP hooks，用于请求响应审计与用量统计
- **凭证策略**：支持 fallback credential、直接配置 credential，以及强制使用服务端凭证
- **兼容 Codex CLI**：可作为 Responses 兼容入口，前接 Anthropic 兼容上游
- **模型统一层**：可把不同供应商的真实模型，映射成稳定的本地模型名，例如 `opus`、`sonnet`、`haiku`

## 这个代理为什么有用

- **给不同供应商建立统一模型命名空间**：你可以把不同来源的模型统一映射成稳定的本地名字，例如 `opus`、`sonnet`、`haiku`，或者团队内部自己的 coding model 名称。这样很多依赖固定模型名的工具会更容易接入。
- **适合 Claude Code 风格的使用方式**：如果你希望上层工具始终使用一组固定模型名，但底层真实模型来自不同厂商，这个代理可以把这层差异收掉。
- **适合新版 Codex CLI**：新版 Codex CLI 只支持 OpenAI Responses API，不再支持 Completions。通过这个代理，Codex 仍然可以使用 Anthropic Messages、OpenAI Chat Completions，或者其他非 Responses 兼容接口。这对接入 GLM、MiniMax、Kimi 这类 coding 能力很强的模型特别有用。
- **跨协议统一入口**：你可以把 Anthropic 兼容、OpenAI 兼容、Gemini 风格的上游统一放到一个接口后面，而不是让每个客户端分别适配多套协议。
- **自带可观测性和数据导出能力**：`usage` hook 可以导出用量统计；`exchange` hook 可以导出完整的 client-facing query/response pair。这样就可以把线上数据持久化，用于分析、评估、审计，或者后续模型训练流程。

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

## 配置

代理通过 YAML 文件配置，并通过 `--config` 指定：

```yaml
listen: 0.0.0.0:8080
upstream_timeout_secs: 120

upstreams:
  GLM-OFFICIAL:
    base_url: https://open.bigmodel.cn/api/anthropic
    format: anthropic
    credential_env: GLM_APIKEY
    auth_policy: client_or_fallback

  OPENAI:
    base_url: https://api.openai.com
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
```

说明：
- 最佳实践是让上游 `base_url` 不带协议版本号。代理会在内部按协议补上 `/v1` 或 `/v1beta`，但也兼容已经带版本根路径的兼容地址，例如 `.../api/paas/v4`。
- Anthropic 兼容上游通常要求 `x-api-key` 和 `anthropic-version`。代理会优先透传客户端鉴权头；若客户端没有提供，可回退到该上游配置的 `credential_env`，并会为 Anthropic 上游默认补上 `anthropic-version: 2023-06-01`。
- 服务商特定静态头应配置在 `upstreams` 中对应上游的 `headers` 字段里。
- `credential_env` 表示“去哪个环境变量读取该上游的 fallback credential”，密钥本身不写进 YAML。
- `credential_actual` 可用于直接在 YAML 中写 fallback credential；它与 `credential_env` 互斥。
- `auth_policy` 支持 `client_or_fallback` 和 `force_server`。
- hooks 是异步 best-effort 模式。通常只开 `usage` 就够；`exchange` 会在请求结束后上报完整的 client-facing request/response pair。

### 完整 YAML 参考

```yaml
listen: 0.0.0.0:8080
upstream_timeout_secs: 120

upstreams:
  UPSTREAM_NAME:
    base_url: https://example.com
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
```

### 顶层字段

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `listen` | string | 否 | `0.0.0.0:8080` | 代理监听地址，格式为 `host:port` |
| `upstream_timeout_secs` | integer | 否 | `120` | 请求上游时的 HTTP 超时 |
| `upstreams` | map | 是 | 无 | 命名上游配置 |
| `model_aliases` | map | 否 | 空 | 把本地模型名映射到 `upstream:model` |
| `hooks` | object | 否 | 关闭 | 可选的异步审计与用量导出 hooks |

### `upstreams`

`upstreams` 是一个以“上游名字”为 key 的 YAML 对象：

```yaml
upstreams:
  GLM-OFFICIAL:
    base_url: https://open.bigmodel.cn/api/anthropic
    format: anthropic
    credential_env: GLM_APIKEY
```

每个 upstream 支持这些字段：

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `base_url` | string | 是 | 无 | 上游基础 URL |
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
| `max_pending_bytes` | integer | 否 | `104857600` | 所有待发送 hook payload 的内存预算上限 |
| `timeout_secs` | integer | 否 | `30` | hook HTTP 请求超时 |
| `failure_threshold` | integer | 否 | `3` | 连续失败多少次后进入 cooldown |
| `cooldown_secs` | integer | 否 | `300` | 熔断后等待多久再尝试恢复 |
| `usage` | object | 否 | 关闭 | 用量导出 hook |
| `exchange` | object | 否 | 关闭 | 完整 request/response 导出 hook |

Hook 行为：
- hooks 是异步、best-effort 的。
- `usage` 通常就足够做计费或观测。
- `exchange` 会在请求完成后导出完整的 client-facing request/response pair，包括完成后的流式结果。
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
    base_url: https://open.bigmodel.cn/api/anthropic
    format: anthropic
    credential_env: GLM_APIKEY

  OPENAI:
    base_url: https://api.openai.com
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
    base_url: https://open.bigmodel.cn/api/anthropic
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
  -c 'model_providers.glm-proxy.base_url="http://127.0.0.1:8099/v1"' \
  -c 'model_providers.glm-proxy.env_key="GLM_APIKEY"' \
  -c 'model_providers.glm-proxy.wire_api="responses"' \
  'Reply with exactly: codex-ok'
```

说明：
- 这里用了临时 `HOME` 和 `--ephemeral`，不会污染你全局的 Codex CLI 配置。
- 客户端访问的是代理的 `/v1/responses`；代理会先把本地模型名 `GLM-5` 解析成 `GLM-OFFICIAL:GLM-5`，再转换成 Anthropic Messages 发给上游。
- 如果上游还需要额外静态协议头，可以在对应 upstream 条目里配置 `headers`。

### 真实上游 Smoke 矩阵

仓库里带了一个真实 smoke 脚本，可通过代理联调 Anthropic 兼容和 OpenAI 兼容上游：

```bash
GLM_APIKEY="你的真实 Key" python3 scripts/real_endpoint_matrix.py
```

覆盖的客户端入口包括：
- `/v1/chat/completions`
- `/v1/responses`
- `/v1/messages`

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
| `POST /v1/chat/completions` | 主端点，接受所有 4 种格式 |
| `POST /v1/responses` | OpenAI Responses API 端点 |
| `POST /v1/messages` | Anthropic Messages API 端点 |
| `GET /health` | 健康检查（返回 `{"status":"ok"}`） |

### 示例请求

#### OpenAI Chat Completions 格式

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

#### Anthropic Claude 格式

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

#### Google Gemini 格式

```bash
curl "http://localhost:8080/v1/chat/completions?key=YOUR_API_KEY" \
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

# 生成详细测试报告
make test-report

# 代码检查
cargo clippy --all-targets --all-features -- -D warnings

# 格式化代码
cargo fmt --all -- --check
```

## 许可证

MIT License
