# LLM Universal Proxy

[English README](./README.md) · [文档索引](./docs/README.md)

`llmup` 是一个单二进制 LLM HTTP 代理。你可以把它放在客户端和真实模型服务之间，让不同协议的客户端都通过一个稳定入口访问上游模型；当客户端协议和上游协议不一致时，代理会自动完成必要的转换。

它最适合这些场景：

- 让 Codex CLI 使用非 OpenAI 原生的上游模型
- 让 Claude Code、Gemini CLI 通过一个本地代理接不同厂商
- 给客户端暴露稳定的本地 alias，而不是直接暴露厂商模型 ID

> [!IMPORTANT]
> `llmup` 适合连接 OpenAI / Anthropic / Gemini 风格 API，或兼容这些协议的服务。它不是把第三方工具接入厂商第一方 App 订阅权益的桥。

![LLMUP dashboard](./docs/images/dashboard.png)

可选的本地 dashboard 可以帮助你查看路由、流式响应、取消、上游状态和 hook 工作情况。

## Quick Start

这条首页路径直接展示两个 upstream：

- 官方 OpenAI API 上的 `gpt-5.4`
- MiniMax OpenAI 兼容入口上的 `MiniMax-M2.7-highspeed`

直接从 [examples/quickstart-openai-minimax.yaml](./examples/quickstart-openai-minimax.yaml) 开始。文件内容如下：

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

这两个 alias 的含义是：

- `gpt-5-4` 是本地稳定 alias，对应 OpenAI `gpt-5.4`
- `gpt-5-4-mini` 也是本地 alias；在这个示例里它路由到 MiniMax `MiniMax-M2.7-highspeed`

构建并启动代理：

```bash
git clone https://github.com/lzjever/llm-universal-proxy.git
cd llm-universal-proxy
cargo build --locked --release

export OPENAI_API_KEY="your-openai-key"
export MINIMAX_API_KEY="your-minimax-key"

./target/release/llm-universal-proxy --config examples/quickstart-openai-minimax.yaml
```

健康检查：

```bash
curl -fsS http://127.0.0.1:8080/health && echo
```

通过同一个本地 OpenAI 风格入口分别试两个 alias：

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

像 `xhigh` 这样的 reasoning effort 是客户端/请求侧设置，不是模型名的一部分。模型 alias 保持稳定，把 reasoning 放在请求或客户端配置里即可。

## Compatibility Contract

`llmup` 提供稳定的本地协议入口，但不承诺不同厂商能力可以无限等价。

- 同协议路径尽量保持 native passthrough
- 跨协议翻译路径以 portable core 为主，遇到不可移植能力会 warning 或 reject
- native extension 和厂商托管的 lifecycle state 默认只留在同厂商路径，除非有明确 documented shim
- quickstart 里的 `surface_defaults` 是保守的 text-only 默认值；只有确认模型 surface 支持时，才打开 search、image 或 parallel-tool 标志
- 多模态 `surface.modalities.input` 只 gate 媒体类型，不承诺所有 source transport；HTTP(S) 图片/PDF URL 和 `gs://`、`s3://`、`file://` 这类 provider/local URI 是不同边界
- Gemini `inlineData` 翻译到 OpenAI Chat/Responses 时可以保留；但所有 Gemini `fileData.fileUri` source 当前都会 fail closed，直到有明确的 fetch/upload adapter
- typed media 的元数据必须自洽；例如 `mime_type` 和 `file_data` data URI 里声明的 MIME 冲突时，代理会在请求上游前拒绝

## Codex / Claude Code / Gemini 基本接法

日常使用更推荐仓库自带的 wrapper，而不是直接手配客户端参数。它们会帮你处理本地环境隔离、base URL 注入，以及部分客户端需要的模型元数据。

沿用上面的 quickstart 配置，先用 `--model gpt-5-4`；如果要切到 MiniMax 这条 lane，就换成 `--model gpt-5-4-mini`。

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

wrapper 设置的 base URL 和代理实际收到的 endpoint 有关系，但不是同一个字符串。

对 Codex 来说，wrapper 当前固定 `wire_api="responses"`，所以它走的是 Responses 路由：

| 客户端 | wrapper 注入的 base URL | 客户端追加的路径 | 代理实际命中的 endpoint |
| --- | --- | --- | --- |
| Codex CLI | `OPENAI_BASE_URL=<proxy>/openai/v1` | `/responses` | `/openai/v1/responses` |
| Claude Code | `ANTHROPIC_BASE_URL=<proxy>/anthropic` | `/v1/messages` | `/anthropic/v1/messages` |
| Gemini CLI | `GOOGLE_GEMINI_BASE_URL=<proxy>/google` | `/v1beta/models/...` | `/google/v1beta/models/...` |

Codex 对 wrapper 的依赖尤其明显，因为 wrapper 会为代理 alias 注入临时模型元数据。细节请看 [docs/clients.md](./docs/clients.md)。

## 最常用静态配置

静态 YAML 的主线很简单：

| 字段 | 作用 |
| --- | --- |
| `listen` | 代理监听地址 |
| `upstream_timeout_secs` | 上游请求超时 |
| `upstreams` | 上游 API 根路径、协议格式与鉴权策略 |
| `model_aliases` | 本地稳定名字到 `UPSTREAM:MODEL` 的映射 |
| `surface_defaults` / `surface` | 可选的客户端可见能力元数据，供 wrapper 和模型目录使用 |
| `proxy` | 可选的默认上游代理 |
| `hooks` | 可选的 usage / exchange 导出 hook |
| `debug_trace` | 可选的本地调试 trace |

实用规则：

- `api_root` 应写厂商 API 根路径，并包含版本段，例如 `.../v1` 或 `.../v1beta`
- `format` 用来固定上游协议：`openai-responses`、`openai-completion`、`anthropic`、`google`
- `gpt-5-4`、`gpt-5-4-mini` 这样的 alias 是本地名字，不要求和真实 upstream model ID 一样
- 只有在你需要补充 `limits` 或 `surface` 元数据时，才需要改成 `target: UPSTREAM:MODEL` 的结构化 alias 写法

完整 YAML 参考和更多示例请看 [docs/configuration.md](./docs/configuration.md)。

## 动态配置概要

默认推荐静态 YAML。只有在你需要运行中改配置时，再用 admin 接口读取运行时状态或替换 namespace 配置。

当前 admin 入口：

- `GET /admin/state`
- `GET /admin/namespaces/:namespace/state`
- `POST /admin/namespaces/:namespace/config`

这部分细节在 [docs/admin-dynamic-config.md](./docs/admin-dynamic-config.md)。

## 继续阅读

- [docs/configuration.md](./docs/configuration.md)：静态配置、alias 设计、YAML 参考
- [docs/clients.md](./docs/clients.md)：Codex / Claude Code / Gemini wrapper 与 base URL 细节
- [docs/admin-dynamic-config.md](./docs/admin-dynamic-config.md)：admin API、运行时配置、CAS 更新
- [docs/protocol-compatibility-matrix.md](./docs/protocol-compatibility-matrix.md)：兼容边界与可移植性摘要
- [docs/max-compat-design.md](./docs/max-compat-design.md)：translated path 的更深入兼容性说明
- [docs/DESIGN.md](./docs/DESIGN.md)：当前架构图
- [docs/README.md](./docs/README.md)：文档索引
