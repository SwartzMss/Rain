# AI Provider 瞬态错误重试设计

## 背景与目标

Skill Run、结果修复、Skill Review 或 Provider Test 的模型请求目前遇到单次 HTTP 429/部分 5xx 或特定网络瞬态错误时会立即失败。对已经执行多轮工具调用的 Skill Run，这会丢弃前面已完成的检索与证据收集。

本改动在 AI Provider service 层增加统一、有限且可观测的重试机制。它不修改 Tool Calling 协议、证据规则、结果结构、迭代次数或 Tool Call 预算，也不改变对外 API 错误契约。

## 统一调用边界

新增 `ai_provider::retry` 模块，提供统一的 `complete_with_retry` helper。所有 Provider 调用入口改用该 helper：

- Skill Run 的常规模型请求；
- Tool Call 预算耗尽后的 `final_model_request`；
- 结果校验失败后的 `result_repair`；
- Skill Review 及其 repair 请求；
- Provider Test。

helper 继续调用现有 `ChatCompletionClient` trait，因此现有测试替身和协议边界保持不变。调用方传入现有 `ProviderRequestContext`，使重试日志保留 `stage`、`run_id` 和 `iteration` 等安全上下文。最终失败日志也由 helper 统一记录，调用方不再重复记录同一错误。

## 重试策略

每个 Provider 操作最多进行 3 次总尝试，即首次请求加 2 次重试。

以下错误可重试：

- HTTP 429、502、503、504；
- transport `connection_reset`；
- transport `connect_failed`。

以下错误默认不可重试，并在首次失败后立即返回：

- HTTP 400、401、403、404 及其他未列入白名单的状态码；
- Provider 单次请求 timeout；
- DNS、TLS 和一般 `request_failed` transport 错误；
- `invalid_response`；
- `response_too_large`。

不重试 timeout 是为了避免一次已经耗尽请求时限的操作再次占用同等时长；Skill Run 和 Review 的整体超时仍是最终保护边界。

## 退避与 Retry-After

默认退避固定为第一次重试等待 1 秒、第二次重试等待 2 秒，不增加 jitter，以保持行为与测试确定。

对于可重试 HTTP 响应，`OpenAiChatClient` 从 `Retry-After` 解析：

- 非负整数秒数；
- 合法 HTTP-date，并换算为从当前时间开始的等待时长。

合法的 `Retry-After` 优先于默认指数退避，并保留 Provider 指定的完整等待时长。缺失、无效或已经过期的 HTTP-date 回退到默认退避。响应正文不会进入错误或日志。

`ProviderError::HttpStatus` 增加内部 `retry_after` 元数据；该类型仍保持可复制，不影响公共 HTTP API。`httpdate` 作为直接依赖用于标准 HTTP-date 解析。

## 总超时与取消

重试 helper 的整个 future 仍位于已有边界内部：

- Skill Run 的 120 秒 `tokio::time::timeout`；
- Skill Review 的 `SKILL_REVIEW_TIMEOUT`；
- Skill Run 各 Provider 请求现有的 cancellation `select!`。

因此请求尝试和退避等待都计入总时限。helper 同时接收调用方 deadline；如果完整 backoff 无法放进剩余预算，则不提前重试，直接返回当前 Provider 错误。外层超时和 Skill Run 取消仍是最终保护边界。Provider Test 使用其配置的 request timeout 作为本次测试操作的总 deadline。

## 安全可观测性

每次准备重试时记录一条结构化 warn 日志：

- `stage`、`run_id`、`iteration`；
- `attempt`（刚失败的尝试，从 1 开始）；
- `max_attempts=3`；
- `error_category`；
- `http_status` 或 allow-listed transport `reason`；
- `backoff_ms`；
- 现有安全请求形状字段。

可重试错误在第 3 次仍失败时，最终失败日志增加 `attempt=3`、`max_attempts=3` 和 `retry_exhausted=true`。不可重试错误的最终日志不标记耗尽。

日志不得包含 API Key、Authorization、Provider URL 凭据、Prompt、Skill Markdown、Issue 日志正文、Tool 返回正文或 Provider response body。

## 测试策略

单元测试覆盖：

- 429、502、503、504 的 retryable 分类；
- 400、401、403、404、timeout、无效响应和超限响应不重试；
- `connect_failed`、`connection_reset` 重试，DNS/TLS/一般请求错误不重试；
- 1 秒/2 秒默认退避；
- `Retry-After` 秒数、HTTP-date、无效值及长等待值不被缩短；
- `Retry-After` 超过剩余总预算时不再重试；
- 首次失败后成功，以及连续失败后第 3 次耗尽；
- 重试和耗尽日志包含规定字段且不包含敏感数据。

Skill Runner 集成测试覆盖 `result_repair` 首次返回 429、随后返回有效结果时 Run 成功，证明末端修复请求实际经过统一 helper。既有测试继续保证公共错误码和结果结构不变。
