# Pre-GA Conversation State Bridge 工作计划

- 状态：handoff-ready development plan
- 日期：2026-05-16
- 范围：为 `llmup` 增加可选、纯内存的会话状态桥，用于把使用状态型接口的客户端转换到无状态 provider 协议
- 非范围：LLM 响应缓存、语义缓存、跨进程持久化数据库、provider 私有 opaque state 反解、默认无配置保存用户数据、后台任务队列产品化、提示词管理产品

## 目标

让 Codex-like、OpenAI Responses-like 的状态型客户端，在显式开启的情况下，也能通过 `llmup` 使用 Anthropic Messages、OpenAI Chat Completions、Gemini GenerateContent 等无状态或手动 replay 型协议。

核心目标：

- 当客户端使用本地 `previous_response_id` 时，`llmup` 可以从自己维护的内存状态中展开可重放上下文。
- 展开后的上下文继续走现有协议转换器，目标 provider 看到的是完整 transcript，而不是 OpenAI Responses 的状态句柄。
- 状态桥只保存会话重放所需的输入/输出事件，不缓存或复用模型响应。
- 默认行为保持 fail closed；只有显式配置启用状态桥的路由才改变现有边界。
- 状态桥不进入 `StrictPassthrough`。它属于有状态兼容增强，必须在 trace 和 warnings 中可见。

一句话边界：这是 `ConversationStateBridge`，不是 cache。

## MVP 范围

为了保持实现简单快速，初版只做一个能力：

- `POST /openai/v1/responses` translated route 上的 `previous_response_id` continuation。

初版不做：

- 本地完整 Conversations API 模拟。
- `GET /responses/{id}` / `DELETE /responses/{id}` / cancel 等 Responses lifecycle 模拟。
- `background` 任务生命周期。
- hosted `prompt` 模板展开。
- `context_management` / compact 本地实现。
- 复杂内存配额、LRU、admin state browser、跨进程恢复。

也就是说，MVP 是一个短期内存 replay buffer：收到第一轮 translated Responses 请求后保存可重放 transcript；第二轮带本地 `resp_llmup_*` 时展开历史并继续调用目标 provider。`llmup` 重启、TTL 到期、ID 未命中时，直接 fail closed。

## 背景与现状

OpenAI Responses 和 Conversations 是状态型接口。官方文档描述了两种主要状态模式：

- `previous_response_id`：用上一个 response ID 串联多轮 response。
- `conversation`：使用 Conversations API 保存并检索 conversation items。

Chat Completions、Anthropic Messages、Gemini `generateContent` 的共同基线是显式 transcript replay。客户端或 SDK 通常需要在每次请求里带上完整历史。

当前 `llmup` 已经支持 OpenAI Responses 请求转换到 Chat/Anthropic/Gemini，但只支持当前请求体里可见的 `input` / `instructions` / tools 等内容。当前请求只带 `previous_response_id` 或 `conversation` 时，`llmup` 没有上下文可展开，因此现在正确地 fail closed。

## 当前 Codebase 判断

已具备：

- OpenAI Responses 原生 state/resource 路由透传，包括 `/responses/compact`、`/responses/{id}/input_items`、`/conversations/*`。
- 省略 `model` 的 stateful OpenAI Responses 请求可以在唯一、明确的 native Responses upstream 上路由。
- OpenAI Responses 带完整 `input` 时，可以通过 `responses_to_messages()` 转成 Chat-style messages，再进入现有 Anthropic/Gemini/OpenAI Chat 转换链。
- 请求边界检查会识别 `previous_response_id`、`conversation`、`background`、`prompt`、`context_management`、`store: true` 等 stateful controls，并在跨协议时拒绝。
- 文档和测试已经锁定“provider-owned state 不重建”的现有行为。
- server-side stateful detector 与 translator detector 有一个已知不一致：server-side `responses_stateful_request_controls()` 当前未包含 `context_management`，translator detector 已包含。即使 MVP 不实现 `context_management`，也应在合同更新时统一检测，避免模型缺省路由和翻译边界分叉。

不具备：

- 没有本地 conversation state store。
- 没有 response ID 到可重放 transcript 的映射。
- 没有 local conversation item list。
- 没有在 translated OpenAI Responses 响应里稳定生成 `llmup` 自己拥有的 `resp_*` / `conv_*` ID。
- 没有非流式或流式响应输出的状态捕获与提交点。
- 没有 TTL、基本 owner 隔离、重启/过期后 fail-closed 语义。

需要接受的 pre-GA 方向变化：

- `docs/CONSTITUTION.md` 目前把 persistent conversation state 和 provider-owned lifecycle state reconstruction 写在 out of scope。
- 本计划不引入持久化数据库，但会引入可选内存状态。需要更新宪章措辞：默认仍是无状态；可配置的内存 `ConversationStateBridge` 是协议转换兼容增强，不是默认产品行为。

## 设计原则

1. 默认关闭。未启用状态桥时，所有现有 fail-closed 行为保持不变。
2. 只重放 `llmup` 自己观察并保存过的状态。外部 OpenAI 返回的 `resp_*` / `conv_*` ID 不可凭空使用。
3. 只保存可重放会话事件，不保存可直接返回给用户的响应缓存条目。
4. 不服务缓存响应。每次客户端请求都必须调用目标 provider 生成新响应。
5. 不反解 provider-private state。`encrypted_content`、opaque reasoning、provider compact state、Gemini `thoughtSignature` 等不能跨协议重建。
6. 不在 strict passthrough 中启用。native OpenAI Responses upstream 继续透传 provider state；状态桥只服务 translated compatibility route。
7. 明确最小 owner 边界。namespace 和认证主体必须参与状态隔离，避免不同调用方互相读取状态。
8. 只实现简单 TTL 和全局最大内存占用。状态过期、进程重启、状态不存在时直接 fail closed。
9. `store: false` 默认不保存状态。
10. 不通过自然语言判断内容是否可压缩或可省略。任何裁剪、摘要、compaction 都必须是显式后续阶段。

状态类型必须分清：

- `ProviderNativeHandle`：OpenAI provider 的真实 `resp_*` / `conv_*`、Gemini `thoughtSignature`、Gemini `cachedContent`、Anthropic thinking signature 等。只在原生 passthrough 中保留。
- `LlmupOwnedTranscript`：`llmup` 自己保存的短期内存 transcript，可跨协议 replay。
- `OpaqueCarrier`：`encrypted_content`、opaque compaction、不可见 reasoning carrier 等。不能跨协议展开。

## 配置草案

初始配置只支持纯内存 store，并且刻意保持小配置面：

```yaml
conversation_state_bridge:
  mode: off          # off | memory
  ttl_seconds: 3600
  max_bytes: 268435456
```

推荐默认：

- `mode: off`
- `mode: memory` 后才捕获和展开 OpenAI Responses 状态。
- `ttl_seconds` 控制状态生命周期。
- `max_bytes` 是全局内存上限，不做 per-tenant/per-conversation 细分。
- `store: false` 优先于 bridge 保存，但这是固定语义，不做成配置项。

## 执行路径

新增一个显式执行能力，不改变 strict passthrough 定义：

```rust
enum ExecutionLane {
    StrictPassthrough,
    ProviderPromptCacheOptimized,
    ConversationStateBridgeTranslation,
    CompatibilityTranslation,
}
```

`ConversationStateBridgeTranslation` 的触发条件：

- client format 是 OpenAI Responses。
- target upstream format 不是 OpenAI Responses，或者 route 明确要求翻译。
- 请求包含本地 `resp_llmup_*` 形式的 `previous_response_id`。
- `conversation_state_bridge.mode = "memory"`。
- 状态 ID 属于当前 namespace / auth subject。

如果目标是 native OpenAI Responses，则继续原生透传，不走状态桥。

插入点要求：

- 状态展开必须发生在现有 request boundary assessment 之前。
- 展开成功后，要移除 `previous_response_id`，把历史和当前 `input` 合成显式 `input`，再进入现有 `assess_request_translation_with_surface()` 和 `translate_request_with_policy()`。
- 状态 store 挂在 `AppState`，不要塞进 `RuntimeState` 快照，避免每次认证上下文 clone 大状态。

## 内部状态模型

状态只保存在内存中，挂在 `AppState` 下，例如：

```rust
struct ConversationStateStore {
    responses: HashMap<String, BridgeResponse>,
    ttl: Duration,
    max_bytes: usize,
    current_bytes: usize,
}

struct StateOwner {
    namespace: String,
    auth_subject_hash: String,
}

struct BridgeResponse {
    id: String,
    owner: StateOwner,
    parent_response_id: Option<String>,
    request_items: Vec<BridgeItem>,
    output_items: Vec<BridgeItem>,
    status: BridgeResponseStatus,
    created_at_ms: i64,
    expires_at_ms: i64,
}
```

保存内容：

- 可重放 OpenAI Responses input items。
- assistant output items 中可转换的 message / function_call / custom_tool_call / reasoning summary。
- tool call 与 tool output 的 `call_id` 关联。
- 当前请求的 `tools`、tool choice、parallel tool policy、response format 等必要 controls。
- 当前 response 的状态、完成时间、截断/不完整原因。

不保存：

- provider credentials、downstream Authorization header、proxy key。
- 原始 response body 的“可直接返回副本”。
- provider-private opaque state。
- debug trace / hook payload中的未脱敏副本。
- `store: false` 请求对应的 response state。

## ID 策略

状态桥必须生成 `llmup` 自己拥有的 ID：

- response：`resp_llmup_<opaque>`

规则：

- translated route 上返回给 OpenAI Responses client 的 ID 必须是 `llmup` ID，不能冒充 provider 真实 `resp_*`。
- native OpenAI Responses passthrough 保留 provider ID，不导入本地状态。
- 如果客户端传入的 `previous_response_id` 不是本地已知 ID，fail closed。
- ID 不编码 owner、prompt、模型或 provider 信息。

## 请求展开规则

### `previous_response_id`

流程：

1. 查找本地 `BridgeResponse`。
2. 验证 namespace / auth subject / route policy。
3. 沿 parent chain 展开历史 request/output items。
4. 追加当前请求 `input`。
5. 使用当前请求的 `instructions`，不自动继承上一轮 `instructions`。
6. 构造完整 Responses input，再交给 `responses_to_messages()` 和现有目标协议转换。

重要语义：

- OpenAI 官方文档说明 `previous_response_id` 与 `instructions` 一起使用时，上一轮 instructions 不会自动带到下一轮。因此状态桥也不能盲目重放旧 instructions。
- 如果历史里只有 opaque reasoning / compaction carrier，而没有可见 summary 或 transcript，fail closed。

### `conversation`

MVP 不支持本地 `conversation` bridge。

行为：

- native OpenAI Responses passthrough 保持现状。
- translated route 上继续 fail closed。
- 后续如果需要支持，只做本地 `conv_llmup_*`，不导入外部 OpenAI `conv_*`。
- `previous_response_id` 和 `conversation` 不能同时使用；这个官方限制需要继续保留。

### `store`

规则：

- `store: false`：不保存 response state；返回的 response ID 不能用于后续 `previous_response_id` replay。
- `store: true` 或省略：在 bridge mode 且 route 允许时保存。
- 如果未来引入 route-level no-store/ZDR policy，它必须禁用 bridge 保存；初版只需要尊重请求级 `store: false`。

### `background`

初始版本不支持 `background: true` 的跨协议状态桥。

原因：

- background 是异步 lifecycle 语义，不只是上下文 replay。
- 纯内存 store 无法在进程重启后保留任务状态。
- 需要独立任务队列、polling state、cancel 行为和生命周期语义。

行为：bridge mode 下仍 fail closed，并在错误中说明当前不支持 background lifecycle emulation。

### `prompt`

初始版本不支持 OpenAI hosted prompt template 跨协议展开。

行为：

- 如果 `prompt` 出现在 translated route 上，fail closed。
- 后续可通过本地 prompt-template registry 显式支持，但不属于本计划初版。

### `context_management` / `/responses/compact`

初始版本不做自动 compaction。

行为：

- native OpenAI Responses passthrough 保持现状。
- translated route 上继续 fail closed，除非未来实现显式本地 compact adapter。
- request-side compaction item 只有在已有可见 summary/text 可重放时，才沿用现有 degrade 规则。

## 响应捕获规则

### 非流式

1. 上游成功返回后，先完成现有 response translation。
2. 如果客户端协议是 OpenAI Responses 且 bridge mode 启用：
   - 预生成一个候选本地 `resp_llmup_*`。
   - 尝试把请求 input items 和转换后的 output items 提交到 store。
   - commit 成功后，把客户端可见 response `id` 替换为候选本地 ID。
   - commit 因 `max_bytes` 失败时，当前响应仍可返回，但不承诺后续 continuation；trace/warning 记录 `state_bridge_memory_limit`。
3. 上游失败或转换失败不写入状态。

### 流式

1. 在 response.created 阶段预分配本地 response ID。
2. streaming sink 收集可重放 output items。
3. 只有收到 completed / incomplete terminal event 后提交状态。
4. 客户端断连、上游错误、stream parse fatal 时不提交 completed 状态；可选记录 aborted metadata，但不能用于 replay。
5. 流式事件中客户端可见 ID 必须与最终 store ID 一致。

## 转换覆盖范围

初始支持：

- OpenAI Responses client -> OpenAI Chat upstream。
- OpenAI Responses client -> Anthropic Messages upstream。
- OpenAI Responses client -> Gemini GenerateContent upstream。
- text message replay。
- assistant text replay。
- function call / function call output replay。
- custom tool call 在现有 compatibility mode 支持范围内 replay。
- visible reasoning summary replay。

初始不支持：

- 本地 Conversations API bridge。
- OpenAI hosted tools 的 provider-side state。
- web search / file search / computer use / code interpreter 的 provider-private state。
- `background: true`。
- hosted `prompt`。
- opaque-only reasoning encrypted content。
- opaque-only compaction。
- 外部 OpenAI provider ID 导入。
- 跨进程恢复。

## 与 Prompt Cache 支持的关系

状态桥与 prompt cache optimizer 是相邻但不同的能力：

- 状态桥负责把缺失的 conversation context 展开成完整 target prompt。
- prompt cache optimizer 可以在 target prompt 构造完成之后，再添加目标 provider 的 cache request controls。
- 状态桥本身不决定哪些内容应该被 provider cache。
- 状态展开后的稳定 prefix 可以作为 `prompt_cache_key` 或 Anthropic breakpoint 策略的输入，但必须通过前一份 prompt-cache plan 的策略和 trace 规则。

执行顺序：

1. Conversation state 展开。
2. Source -> target protocol translation。
3. Provider prompt-cache optimization。
4. Upstream request。

## 安全与隔离

必须实现：

- State owner 至少包含 namespace 和认证主体 hash。
- client-provider-key 模式下，认证主体可以由下游 provider key 的安全 hash 派生。
- proxy-key 模式下，初版至少按 namespace 隔离；如果后续引入用户身份 header/policy，再把它纳入 owner。
- store lookup 只有四种结果：命中、未找到、过期、owner mismatch。除命中外都 fail closed。
- debug trace 只记录状态 ID、展开条数和 fail reason，不记录 prompt 内容。

内存保护：

- 初版只实现 TTL 清理和一个全局 `max_bytes`，不实现 LRU、per-tenant 配额、per-conversation 配额、跨进程恢复或后台压缩。
- 过期清理可以是请求路径上的惰性清理，也可以是轻量周期任务；选择实现最简单的一种。
- 写入新状态前先清理过期项；如果仍超过 `max_bytes`，当前 response 不提交可 replay 状态，并在 trace/warning 中说明 `state_bridge_memory_limit`。
- 初版可以用一把简单 mutex/RwLock 串行化 store 写入，不引入版本协议。

隐私保护：

- 纯内存不等于无数据保留。文档必须明确：开启状态桥会在 `llmup` 进程内保存 prompt 和模型输出，直到 TTL 或进程退出。
- `store: false` 不保存。
- 不允许 hook/debug 输出状态内容。

## 开发阶段

### Phase 0：合同冻结与文档更新

交付：

- 更新 `CONSTITUTION.md`：默认 stateless；可选纯内存 `ConversationStateBridge` 是明确配置的兼容增强。
- 更新 state-continuity docs：区分 provider-owned state、llmup-owned bridge state、cache。
- 新增配置 schema 文档和默认关闭说明。

验收：

- 未配置状态桥时，现有 fail-closed 测试全部保持。
- 文档明确“不做 response cache”。

### Phase 1：内存 Store 骨架

交付：

- 在 `AppState` 加入 `ConversationStateStore`。
- 增加 `conversation_state_bridge` 配置解析和有效配置解析。
- 实现 ID minting、StateOwner、TTL、全局 `max_bytes`、基本 get/put/delete。
- 使用简单 mutex/RwLock 保护内存 HashMap。

验收：

- store 单元测试覆盖 create/get/expire/max_bytes/owner mismatch/restart-miss 语义。
- 默认配置不创建 store 或 store disabled。

### Phase 2：非流式 `previous_response_id` Replay

交付：

- translated OpenAI Responses 非流式响应生成 `resp_llmup_*`。
- 成功响应后保存 request/output items。
- 后续 `previous_response_id` 查本地状态并展开为完整 input。
- 展开后复用现有 `responses_to_messages()` 和目标协议转换。

验收：

- Responses -> Anthropic 第一轮返回本地 response ID。
- 第二轮带 `previous_response_id`，Anthropic upstream 捕获到第一轮 user、第一轮 assistant、新 user input。
- 未知/过期/owner mismatch response ID fail closed。
- `store: false` 后续 replay fail closed。

### Phase 3：工具调用 Replay

交付：

- 保存 assistant function_call / custom_tool_call output items。
- 保存 client 后续 function_call_output / custom_tool_call_output input items。
- 用现有 tool bridge 规则转成 Chat/Anthropic/Gemini 可接受的历史。
- 处理 pending tool call 状态。

验收：

- 第一轮模型返回 tool call，第二轮 client 提交 tool output + `previous_response_id`，目标 upstream 收到完整 assistant tool call + tool result 历史。
- call_id 缺失、重复、跨 parent chain mismatch fail closed。
- custom tool 在 strict/balanced/max_compat 下遵循现有 capability surface。

### Phase 4：流式响应捕获

交付：

- streaming response 预分配本地 response ID。
- streaming sink 收集 output deltas 并还原可重放 output items。
- terminal event 后提交状态。
- abort/error 不提交可 replay 状态。

验收：

- 流式第一轮完成后，第二轮 `previous_response_id` 可 replay。
- 客户端断连后 response ID 不可 replay 或明确标记 incomplete。
- stream 中所有可见 response ID 一致。

### Phase 5：轻量清理与观测

交付：

- 实现 TTL 惰性清理或轻量周期清理。
- 实现全局 `max_bytes` 检查。
- 在 debug trace 中记录 bridge enabled、state hit/miss/expired/owner_mismatch、replay item count。
- 确认 hook/debug 不包含状态内容。
- 统一 server-side 和 translator-side stateful detector，至少补齐 `context_management`。

验收：

- TTL 到期后 replay fail closed。
- 超过 `max_bytes` 时不提交 replay 状态，并记录 warning/trace。
- debug trace 只含 metadata，不含 prompt 内容。
- `store: false` 不保存。
- model-less `context_management` 在 native routing resolver 和 translation boundary 上行为一致。

### Phase 6：可选增强

仅在核心稳定后考虑：

- 本地 Conversations API bridge。
- 本地 compaction adapter。
- 本地 prompt template registry。
- 持久化 store 后端。
- 外部 OpenAI state import adapter。
- Gemini explicit cachedContent managed handle 与 ConversationStateBridge 的协同。
- 容量配额、LRU、admin state browser、跨进程恢复、分布式状态同步。

这些增强必须单独评审，不属于初版。

## 测试矩阵

必须覆盖：

| 区域 | 覆盖要求 |
| --- | --- |
| 默认行为 | bridge off 时，现有 stateful controls 跨协议 fail closed |
| 非流式 replay | Responses -> Chat/Anthropic/Gemini 的 `previous_response_id` 多轮上下文展开 |
| 工具调用 | function_call/custom_tool_call + tool output replay |
| 流式 | completed 后可 replay，abort/error 不可 replay |
| 隔离 | namespace/auth subject mismatch fail closed |
| TTL | expired state fail closed 且有 trace reason |
| max_bytes | 超过全局内存上限时不提交 state，且有 warning/trace |
| store:false | 不保存、不 replay、不泄露内容 |
| detector 统一 | `context_management` 在 resolver 和 translation boundary 上一致 fail/route |
| Native passthrough | OpenAI Responses native routes 不被本地 bridge 改写 |
| Prompt cache 顺序 | state 展开先于 provider prompt-cache optimizer |

## Handoff 任务顺序

推荐 PR 栈：

1. 文档与配置合同：新增配置、更新 state-continuity/constitution。
2. 内存 `ConversationStateStore` 和 ID/TTL/max_bytes 基础设施。
3. 非流式 translated Responses response capture。
4. `previous_response_id` replay 到 Chat/Anthropic/Gemini。
5. 工具调用 replay。
6. 流式响应捕获。
7. TTL 清理、trace metadata、hook/debug 泄露检查、`context_management` detector 统一。
8. 与 prompt-cache optimizer 的执行顺序和 trace 集成。
9. 后续再评估本地 Conversations API bridge。

主要代码区域：

- [src/config.rs](../../src/config.rs)
- [src/server/proxy.rs](../../src/server/proxy.rs)
- [src/server/responses_resources.rs](../../src/server/responses_resources.rs)
- [src/translate/internal/openai_responses.rs](../../src/translate/internal/openai_responses.rs)
- [src/translate/internal.rs](../../src/translate/internal.rs)
- [src/streaming/stream.rs](../../src/streaming/stream.rs)
- [src/streaming/openai_sink.rs](../../src/streaming/openai_sink.rs)
- [src/streaming/state.rs](../../src/streaming/state.rs)
- [src/telemetry.rs](../../src/telemetry.rs)
- [tests/integration_test.rs](../../tests/integration_test.rs)

## 明确非目标

- 不做 LLM 响应缓存。
- 不做语义缓存。
- 不把 provider 私有状态转换成通用状态。
- 不默认保存任何 prompt / response。
- 不支持外部 provider response ID 自动导入。
- 不在初版支持 background lifecycle。
- 不在初版支持 hosted prompt template。
- 不在初版支持自动 compaction。
- 不引入数据库或外部服务。

## 参考资料

官方参考：

- OpenAI Conversation state guide: <https://developers.openai.com/api/docs/guides/conversation-state>
- OpenAI Responses create reference: <https://developers.openai.com/api/reference/resources/responses/methods/create>
- OpenAI Conversations reference: <https://developers.openai.com/api/reference/resources/conversations>
- OpenAI Background mode guide: <https://developers.openai.com/api/docs/guides/background>

本地参考：

- [docs/protocol-baselines/capabilities/state-continuity.md](../protocol-baselines/capabilities/state-continuity.md)
- [docs/protocol-compatibility-matrix.md](../protocol-compatibility-matrix.md)
- [docs/engineering/pre-ga-strict-passthrough-prompt-cache-support-plan.md](./pre-ga-strict-passthrough-prompt-cache-support-plan.md)
