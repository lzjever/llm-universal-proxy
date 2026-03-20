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
- **兼容 Codex CLI**：可作为 Responses 兼容入口，前接 Anthropic 兼容上游

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

  OPENAI:
    base_url: https://api.openai.com
    format: openai-responses
    credential_env: OPENAI_API_KEY

model_aliases:
  GLM-5: GLM-OFFICIAL:GLM-5
  gpt-4o: OPENAI:gpt-4o
```

说明：
- 最佳实践是让上游 `base_url` 不带协议版本号。代理会在内部按协议补上 `/v1` 或 `/v1beta`。
- Anthropic 兼容上游通常要求 `x-api-key` 和 `anthropic-version`。代理会优先透传客户端鉴权头；若客户端没有提供，可回退到该上游配置的 `credential_env`，并会为 Anthropic 上游默认补上 `anthropic-version: 2023-06-01`。
- 服务商特定静态头应配置在 `upstreams` 中对应上游的 `headers` 字段里。
- `credential_env` 表示“去哪个环境变量读取该上游的 fallback credential”，密钥本身不写进 YAML。

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
- 如果上游还需要额外静态协议头，可以在 `UPSTREAMS` 的对应上游里配置 `headers`。

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
