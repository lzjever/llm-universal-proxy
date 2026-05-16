# Pre-GA 移除 Native Gemini Format 工作计划

- 状态：handoff-ready development plan
- 日期：2026-05-16
- 范围：从 `llmup` 中移除 Google/Gemini native `generateContent` 客户端协议和 upstream wire format 支持
- 非范围：封禁 Gemini 品牌模型、实现 Gemini 专有缓存资源管理、兼容历史 `format: google` 配置、保留 `/google/v1beta/*` 路由、做大型抽象重构

## 结论

这个方向合理，建议在 pre-GA 直接执行。

关键判断：`llmup` 应该围绕少数稳定 wire protocol 做协议转换，而不是围绕每个 provider 品牌维护 native API 适配器。Google 官方已经提供 OpenAI-compatible Gemini API；因此如果用户要把 Gemini 作为模型供应方接入，可以通过 OpenAI Chat Completions 形状的 endpoint 配置为普通 OpenAI-compatible upstream。`llmup` 不再需要维护 Gemini `generateContent` 这套独立客户端协议和 provider 格式。

这不是“移除 Gemini 模型可用性”，而是“移除 Gemini native wire protocol 支持”。删除后，`llmup` 的主协议面收敛为：

- OpenAI Chat Completions
- OpenAI Responses
- Anthropic Messages

Gemini 只能作为 OpenAI-compatible upstream 使用；在 `llmup` 内部不再存在 `google` / `gemini` format。

## 三计划协同

本计划是另外两份计划的范围前置条件：

- [pre-ga-strict-passthrough-prompt-cache-support-plan.md](./pre-ga-strict-passthrough-prompt-cache-support-plan.md) 必须按 3 个 active protocol families 设计：OpenAI Chat、OpenAI Responses、Anthropic Messages。
- [pre-ga-conversation-state-bridge-plan.md](./pre-ga-conversation-state-bridge-plan.md) 的 MVP 只支持 Responses -> OpenAI Chat / Anthropic replay，不实现 Responses -> Gemini `generateContent`。
- 删除 native Gemini 的 PR 应优先合并，或至少作为其他两个分支的共同 rebase base。

并行开发时的文件所有权建议：

| Workstream | 主要所有权 | 避免踩线 |
| --- | --- | --- |
| Remove Native Gemini | `UpstreamFormat::Google`、`/google/*` routes、Gemini translators、Gemini streaming、Gemini tests/docs/examples/scripts | 不新增 prompt-cache/state bridge 逻辑 |
| Strict Passthrough + Provider Prompt Cache | execution lane、raw passthrough、OpenAI/Anthropic cache optimizer、usage observation | 不修改或新增 Gemini translator/cache 功能；等 Gemini 删除后收敛测试矩阵 |
| Conversation State Bridge | memory store、Responses `previous_response_id` replay、state capture、state trace | 不实现 Gemini replay；状态展开后再交给 prompt-cache optimizer |

合并顺序：

1. 先合并本计划的 Phase 0-4，移除 public Gemini surface 和核心转换/streaming 分支。
2. strict passthrough/cache 分支基于 3 协议矩阵补 golden tests 和 optimizer。
3. state bridge 分支基于 3 协议目标实现 replay。
4. 最后统一跑全量 `cargo test`、Python 文档合同测试、`rg` 清理项和 `git diff --check`。

如果必须完全并行开发，其他两个 workstream 必须把所有 Gemini 相关改动视为 remove-native-gemini workstream 的独占范围，不再添加新的 Gemini cache/state 测试或 helper。

## 为什么值得做

Native Gemini 是当前复杂度最高、收益最低的一条协议线：

- Gemini `GenerateContentRequest` 使用 `contents[]` / `parts[]` / `systemInstruction` / `generationConfig` / `safetySettings` / `cachedContent` 等独立 schema。
- Gemini 的 `cachedContent` 是 provider-side named resource，和 OpenAI `prompt_cache_key`、Anthropic `cache_control` 不是同一类语义。
- Gemini `thoughtSignature`、`fileData.fileUri`、`inlineData`、`functionResponse.parts`、native safety settings 都会在跨协议转换里制造大量 fail-closed 分支。
- Gemini SSE 与 OpenAI/Anthropic streaming 事件模型不同，当前代码里已经形成独立 source/sink 和状态字段。
- Google OpenAI-compatible endpoint 已经覆盖常见 Chat Completions 调用路径，继续维护 native Gemini 对 `llmup` 的低心智负担目标不划算。

删除 native Gemini 后，前面两个 pre-GA 计划也会更简单：

- Prompt-cache 计划不再需要处理 Gemini `cachedContent` 生命周期、`extra_body.google.cached_content` 透传和 Gemini cache handle 跨协议失败问题。
- Conversation state bridge 不再需要把 OpenAI Responses replay 到 Gemini `generateContent`。
- Strict passthrough 矩阵从 4x4 收敛到 3x3，测试和文档都更容易稳定。

## 保留什么

保留 Gemini 作为 OpenAI-compatible upstream 的能力：

```yaml
upstreams:
  - name: gemini-openai-compatible
    base_url: https://generativelanguage.googleapis.com/v1beta/openai
    format: openai-completion
    auth:
      type: bearer
      env: GEMINI_API_KEY
```

这条路径的原则：

- 它是 OpenAI-compatible wire protocol，不是 Gemini format。
- 同协议时可以走 OpenAI Chat strict passthrough，前提是不需要 body mutation。
- Gemini 模型名，例如 `gemini-3-flash-preview`，只是 model string，不让 `llmup` 进入 Gemini-native adapter。
- OpenAI-compatible provider extensions 默认不做特殊支持；如果以后确实需要，必须作为显式 provider extension plan 独立评估。

## 删除什么

必须删除的用户可见能力：

- `format: google`
- `format: gemini`
- `/google/v1beta/models`
- `/google/v1beta/models/{model}:generateContent`
- `/google/v1beta/models/{model}:streamGenerateContent`
- `/namespaces/{namespace}/google/v1beta/*`
- `GOOGLE_GEMINI_BASE_URL=<proxy>/google` 类客户端说明
- native Gemini protocol baseline 作为活跃支持文档
- Gemini wrapper/live-profile 作为一等客户端矩阵项

必须删除的内部能力：

- `UpstreamFormat::Google`
- Google/Gemini model action routing
- Gemini request translator：Gemini -> OpenAI、OpenAI/Anthropic/Responses -> Gemini
- Gemini response translator：Gemini -> OpenAI/Anthropic/Responses、OpenAI -> Gemini
- Gemini stream source/sink
- 仅服务 native Gemini stream conversion 的 Gemini-specific state fields
- Gemini native cache handle support，包括 `cachedContent` / `cached_content` / `extra_body.google.cached_content`
- Gemini-specific fail-closed boundary checks，除非只用于 retired docs 或 migration error message

## 不做兼容迁移

项目仍处于 pre-GA，因此不保留历史行为。

删除后：

- 旧配置里的 `format: google` / `format: gemini` 直接配置加载失败。
- 错误信息应给出简短迁移提示：使用 Google OpenAI-compatible endpoint 并配置 `format: openai-completion`。
- `/google/*` 路由返回 404 即可，不需要兼容转发。
- 不新增 hidden adapter 把 Gemini native 请求偷偷翻成 OpenAI Chat。
- 不新增 feature flag 暂时保留 Gemini native。

这能避免“名义删除，实际继续维护两套行为”的范围蔓延。

## 当前 Codebase 耦合面

本地代码扫描显示 Gemini 已经深度进入核心路径，不建议用小补丁屏蔽：

- [src/formats.rs](../../src/formats.rs)：`UpstreamFormat::Google`，`google` / `gemini` serde 和 parser alias。
- [src/server/mod.rs](../../src/server/mod.rs)：`/google/v1beta/models` 和 namespaced Google routes。
- [src/server/proxy.rs](../../src/server/proxy.rs)：`handle_google_model_action*` 和 `generateContent` / `streamGenerateContent` action 解析。
- [src/config.rs](../../src/config.rs)：Google upstream URL builder、stream URL builder 和相关测试。
- [src/upstream.rs](../../src/upstream.rs)：Google model path 特判。
- [src/translate/internal.rs](../../src/translate/internal.rs)：Gemini request/response 分支、边界检查、tool/media/cache portability 检查。
- [src/translate/internal/request_gemini.rs](../../src/translate/internal/request_gemini.rs)：Gemini 转换主体。
- [src/streaming/gemini_source.rs](../../src/streaming/gemini_source.rs)：Gemini stream source。
- [src/streaming/openai_sink.rs](../../src/streaming/openai_sink.rs)：OpenAI chunk -> Gemini SSE sink。
- [src/streaming/state.rs](../../src/streaming/state.rs)：Gemini stream state。
- [src/server/models.rs](../../src/server/models.rs)：Google model listing/detail handlers。
- [src/hooks.rs](../../src/hooks.rs)：Google/Gemini usage、SSE accumulator、hook summary。
- [src/debug_trace.rs](../../src/debug_trace.rs)：Google/Gemini stream/request summary 和相关测试。
- [src/detect.rs](../../src/detect.rs)、[src/discovery.rs](../../src/discovery.rs)、[src/server/headers.rs](../../src/server/headers.rs)：Gemini request detection、discovery capability、Google API key/header 处理。
- [tests/multimodal_gemini_boundary_test.rs](../../tests/multimodal_gemini_boundary_test.rs)：Gemini multimodal boundary tests。
- [tests/reasoning_test.rs](../../tests/reasoning_test.rs)、[tests/multimodal_integration_test.rs](../../tests/multimodal_integration_test.rs)：Gemini reasoning/multimodal translation coverage。
- [src/streaming/tests/gemini_source.rs](../../src/streaming/tests/gemini_source.rs) / [src/streaming/tests/gemini_sink.rs](../../src/streaming/tests/gemini_sink.rs)：Gemini stream tests。
- [scripts/run_gemini_proxy.sh](../../scripts/run_gemini_proxy.sh)：Gemini client helper。
- [scripts/real_cli_matrix.py](../../scripts/real_cli_matrix.py)、[scripts/real_endpoint_matrix.py](../../scripts/real_endpoint_matrix.py)：Gemini native route smoke/matrix。
- [examples/upstream-proxy.yaml](../../examples/upstream-proxy.yaml)：`format: google` 示例。
- [README.md](../../README.md)、[docs/clients.md](../clients.md)、[docs/protocol-compatibility-matrix.md](../protocol-compatibility-matrix.md)、[docs/protocol-baselines/google-gemini.md](../protocol-baselines/google-gemini.md)：用户文档和协议矩阵。

## 开发阶段

### Phase 0：锁定决策和文档边界

目标：

- 合并本计划。
- 在工程文档中明确：native Gemini support 被移除，Gemini 只通过 OpenAI-compatible upstream 使用。
- 更新 prompt-cache 和 conversation-state 两份计划，把 Gemini native rows 标记为 removed 或删除。这是硬冻结点，不能等到后续阶段。
- 更新 docs contract tests 的预期，避免旧合同继续要求 Gemini native active support。

验收：

- `docs/engineering/README.md` 链接本计划。
- 用户文档不再把 Gemini native 作为 GA 支持路径。
- 保留的 Gemini 提及只用于 OpenAI-compatible migration 或 retired baseline。
- Prompt-cache/state 两份计划不再包含 `provider_prompt_cache.gemini.*`、Gemini resource adapter、Responses -> Gemini replay、Gemini cache/state 测试任务。

### Phase 1：关闭 public surface 和配置入口

任务：

- 从 router 删除 `/google/v1beta/*` 和 namespaced `/google/v1beta/*`。
- 暂时保留内部 `UpstreamFormat::Google`，直到 translation/streaming/observability 引用都删除；不要在本阶段先删 enum 导致无法编译。
- 对用户配置删除 `google` / `gemini` parser alias 或在 validation 层拒绝它们，并返回迁移错误。
- 配置加载遇到 `format: google` / `format: gemini` 时返回明确错误。
- Google URL builder、auth/header、discovery match arms 在内部 enum 删除前可以先保留为 inert code；只有在所有调用点移除后再删除。不要为了 Phase 1 破坏 `cargo check`。

验收：

- `format: google` / `format: gemini` 不再能启动服务。
- `/google/v1beta/models` 和 `/google/v1beta/models/{id}:generateContent` 不再注册。
- 错误提示包含 `format: openai-completion` 和 Google OpenAI-compatible base URL 迁移方向。
- 本阶段仍能 `cargo check`；不要求 `UpstreamFormat::Google` 命中为零。

### Phase 2：删除 Gemini translation 分支

任务：

- 删除 `src/translate/internal/request_gemini.rs`。
- 删除 `gemini_to_openai()`、`openai_to_gemini()`、`openai_response_to_gemini()`、`gemini_response_to_openai()` 及所有只服务 Gemini 的 helper。
- 收敛 translation match：只保留 OpenAI Chat、OpenAI Responses、Anthropic 三种 source/target。
- 删除 Gemini-specific nonportable checks。
- 删除 Gemini `cachedContent` extension 映射。

验收：

- translator 编译时不存在 `UpstreamFormat::Google` match。
- 跨协议转换矩阵是 3x3。
- `src/translate/**` 中不存在 Gemini native request/response 转换逻辑。
- 本阶段仍能 `cargo check`；全局 `UpstreamFormat::Google` 清零留到最终清理阶段。

### Phase 3：删除 Gemini streaming 实现

任务：

- 删除 `src/streaming/gemini_source.rs`。
- 删除 OpenAI -> Gemini SSE sink。
- 删除 `StreamState` 中仅服务 Gemini 的字段。
- 删除 Gemini stream validation 和 error-shape helpers。
- 修正 streaming module exports。

验收：

- stream source/sink 只覆盖 OpenAI Chat、OpenAI Responses、Anthropic。
- Gemini stream tests 删除或移入 retired reference，不参与 CI。
- 同协议 OpenAI-compatible Gemini upstream 仍可使用 OpenAI Chat stream path。

### Phase 4：删除 observability、detect、models、scripts、docs

任务：

- 删除 Google model handler。
- 删除 hooks/debug trace 中的 Gemini accumulator、usage summary 和 stream/request summary。
- 删除 Gemini request detection、discovery default target、Google-specific auth/header helpers。
- 删除 `scripts/run_gemini_proxy.sh` 或改为已废弃说明，不再推荐。
- 更新 `scripts/real_cli_matrix.py` 和 `scripts/real_endpoint_matrix.py`，移除 native Gemini route/smoke 项。
- 更新 examples，移除 `format: google`。
- 更新 README、clients、configuration、container、GA readiness、protocol compatibility matrix。
- 将 `docs/protocol-baselines/google-gemini.md` 移到 retired 区域，或在文件头标记“historical reference only, not active support”。
- 删除 Google protocol snapshot 合同测试要求，避免误导为 active baseline。

验收：

- 用户入口文档不再承诺 native Gemini。
- 示例配置中不存在 `format: google`。
- `docs/protocol-baselines` 明确区分 active baselines 和 retired references。

### Phase 5：核验 prompt-cache 和 state 计划仍然收敛

任务：

- 核验 prompt-cache plan 仍是 3x3，不重新引入 Gemini cache fields、`extra_body.google.cached_content` mapping 或 Gemini resource adapter。
- 核验 conversation-state bridge plan 仍只支持 OpenAI Responses -> OpenAI Chat / Anthropic Messages。
- 核验 Google OpenAI-compatible upstream 只被描述为 OpenAI-shaped upstream，不是 native Gemini format。

验收：

- 两份 plan 不再把 Gemini native 作为未来开发任务。
- cache/state 文档不再要求实现 Gemini provider-side resource lifecycle。
- 仍然保留“provider brand 与 wire protocol 分离”的说明。

### Phase 6：测试收敛

任务：

- 删除或重写所有 Gemini native unit/integration tests。
- 更新 Python docs contract tests。
- 更新 CLI matrix tests，移除 Gemini wrapper/native route 项。
- 增加 migration negative tests：
  - `format: google` rejected。
  - `format: gemini` rejected。
  - `/google/v1beta/models` 404。
- 增加 OpenAI-compatible Gemini config smoke/mock test，证明 Gemini brand 可以通过 OpenAI Chat wire protocol 接入。

验收命令：

```bash
cargo check
cargo test --no-run
cargo test
python3 -m unittest \
  tests.test_protocol_docs_contract \
  tests.test_project_docs_contract \
  tests.test_ga_docs_contract \
  tests.test_cli_matrix_contracts \
  tests.test_real_cli_matrix \
  tests.test_real_endpoint_matrix
git diff --check
```

### Phase 7：最终清理

任务：

- 删除内部 `UpstreamFormat::Google` variant、serde alias、parser alias、Display 分支和所有 leftover match arms。
- `rg -n "UpstreamFormat::Google|format: google|format: gemini|GOOGLE_GEMINI_BASE_URL|generateContent|streamGenerateContent|cachedContent|thoughtSignature" src tests docs examples scripts README.md`
- 对命中项分类：
  - active support：必须为零。
  - migration note：允许少量，必须明确写“removed native Gemini format”。
  - retired baseline：允许，但不能被 docs index 当作 active support。
- 更新 crate description：从 “4 formats” 改为 “OpenAI Chat, OpenAI Responses, Anthropic Messages”。

验收：

- active code 不存在 Gemini native format。
- active docs 不存在 Gemini native setup。
- CI 全绿。

## Prompt Cache 影响

删除 native Gemini 后，prompt-cache 支持面更清楚：

- OpenAI-compatible Gemini upstream 可以保留 OpenAI-shaped request 字段，例如普通 Chat Completions 参数。
- 不默认支持 Gemini native `cachedContent`，因为它是 provider-side resource handle，会重新引入 Gemini resource lifecycle。
- 不默认支持 `extra_body.google.cached_content`，即使 Google OpenAI-compatible 文档允许 `extra_body.google` 传递部分 Gemini 字段。这个扩展会重新制造 provider-specific branch，和本次简化目标冲突。
- 如果后续明确有强经济收益，可以单独设计 “Google OpenAI-compatible provider extension” 小计划，但它必须保持显式配置、不可跨协议泛化、不可进入 strict passthrough。

这样做会损失 Gemini explicit cached-content 优化，但换来主转换矩阵和状态/cache 设计的显著简化。对于 pre-GA，建议优先收敛复杂度。

## State Bridge 影响

删除 native Gemini 后，状态桥目标只需要支持：

- OpenAI Responses stateful client -> OpenAI Chat upstream
- OpenAI Responses stateful client -> Anthropic Messages upstream

如果 upstream 是 Google OpenAI-compatible endpoint，它属于 OpenAI Chat upstream，不需要 Gemini-specific replay。

这能避免把本地 transcript store 和 Gemini `thoughtSignature`、`cachedContent`、native chat history 混在一起。

## 迁移说明草案

旧配置：

```yaml
upstreams:
  - name: google
    base_url: https://generativelanguage.googleapis.com/v1beta
    format: google
```

新配置：

```yaml
upstreams:
  - name: google-openai-compatible
    base_url: https://generativelanguage.googleapis.com/v1beta/openai
    format: openai-completion
```

客户端侧：

- 不再把 base URL 指向 `http://localhost:PORT/google`。
- 使用 OpenAI Chat Completions compatible base URL：`http://localhost:PORT/openai` 或 namespaced OpenAI route。
- model 继续使用 Gemini 模型名，前提是 Google OpenAI-compatible endpoint 支持该模型。

## 风险与取舍

接受的损失：

- Native Gemini client 不再能直接接入 `llmup`。
- Native Gemini `cachedContent`、`safetySettings`、`thoughtSignature`、`fileData`、`generateContent` 不再被代理或转换。
- 一些 Gemini-specific multimodal 转换能力被删除。

换来的收益：

- 协议矩阵从 4x4 降到 3x3。
- Strict passthrough 定义更清楚。
- Prompt-cache 计划避免 Gemini provider resource lifecycle。
- State bridge 不需要兼容 Gemini opaque state。
- 测试、文档、用户配置、错误模型都更容易理解。

这是 pre-GA 最适合做的取舍。

## 参考资料

- Google Gemini OpenAI compatibility: <https://ai.google.dev/gemini-api/docs/openai>
- Google Gemini `generateContent` API: <https://ai.google.dev/api/generate-content>
- Gemini CLI documentation: <https://google-gemini.github.io/gemini-cli/docs/>
