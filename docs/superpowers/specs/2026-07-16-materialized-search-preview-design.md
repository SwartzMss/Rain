# 详细搜索结果物化设计

## 目标

Issue 或单文件详细搜索只扫描原始日志一次。首次搜索返回第一页和精确总数，后续翻页从带稀疏索引的临时结果读取，不再重复扫描原始日志，同时保留跳转到原文件和原始行号所需的信息。

## 范围

- 优化 `POST /api/temp-results/preview` 和搜索结果标签页的分页路径。
- 首次请求保持同步：完整扫描结束后返回精确 `total`。
- 保留现有 `POST /api/temp-results` 分享、下载、续期与删除行为。
- 不在本次加入后台任务、搜索进度、取消搜索或内容倒排索引。

## 存储格式

每次 preview 创建一个 SQLite `temp_results` 记录，并在 `temp-results/` 中生成三个同 ID 文件：

- `<id>.log`：按匹配顺序保存完整原始日志行，供分页与下载使用。
- `<id>.meta`：每条匹配结果一条元数据记录，保存 `bundle_hash`、`file_id`、来源路径和原始行号。
- `<id>.idx`：每 1000 条匹配结果保存一个检查点，包含结果行号以及 `.log`、`.meta` 的 byte offset。

写入使用临时文件并在扫描成功后登记数据库。扫描、写入或数据库登记失败时删除本次产生的文件，避免留下不可访问的部分结果。

旧临时结果可能只有 `.log`。读取接口在 sidecar 不存在时沿用顺序读取逻辑，因此升级后仍可访问旧结果。

## API

`POST /api/temp-results/preview` 请求格式保持不变。响应增加 `result_id`：

```json
{
  "result_id": "4d1f...",
  "lines": [
    {
      "bundle_hash": "...",
      "file_id": "42",
      "path": "application.log",
      "line_number": 123,
      "content": "ERROR ..."
    }
  ],
  "total": 123456
}
```

首次扫描在写入物化文件的同时收集第一页，避免为了响应第一页再次读取结果文件。

`GET /api/temp-results/{id}/lines?start=<n>&limit=<n>` 对带 sidecar 的结果返回来源信息：

```json
{
  "start": 3000,
  "limit": 1000,
  "line_count": 123456,
  "next_start": 4000,
  "lines": [
    {
      "bundle_hash": "...",
      "file_id": "42",
      "path": "application.log",
      "line_number": 123,
      "content": "ERROR ..."
    }
  ]
}
```

读取时选择不大于 `start` 的最近检查点，seek 到两个 sidecar offset，最多顺序跳过 999 条后开始返回当前页。没有 sidecar 的旧结果继续返回结果文件内的行号和内容，来源字段为空。

## 前端数据流

首次详细搜索仍调用 preview。搜索标签页除表达式、第一页和总数外保存响应中的 `result_id`。

搜索标签页翻页统一调用 `/api/temp-results/{result_id}/lines`，不再根据原始 Issue、文件或临时结果重新提交表达式。接口返回的来源字段继续映射为现有搜索命中结构，因此原文件定位行为保持不变。

在搜索结果上继续过滤时，以当前 `result_id` 作为 `source_temp_id` 创建新的物化搜索结果，避免重新扫描更早的原始来源。

## 生命周期与错误处理

- preview 生成的结果沿用现有 7 天有效期和访问续期规则。
- 删除结果时同时删除 `.log`、`.meta` 和 `.idx`。
- 过期惰性清理同时删除三个文件。
- sidecar 损坏或与 `.log` 不一致时返回明确的服务端错误，不静默返回错位的来源信息。
- 原始文件在 preview 完成后被移动或删除，不影响已物化结果的分页。

## 验证

- 后端单元测试验证检查点生成、最近检查点选择和 sidecar 分页。
- 后端集成测试验证 preview 返回 `result_id` 和精确总数。
- 集成测试在 preview 后删除或改名原始文件，再读取第二页，以证明分页不再依赖原始文件。
- 多来源测试验证每条结果的 bundle、文件、路径和原始行号保持正确。
- 删除与过期测试验证三个文件均被清理。
- 兼容性测试验证只有 `.log` 的旧临时结果仍能分页。
- 前端测试或类型检查验证搜索标签页保存 `result_id`，翻页调用结果行接口。
- 最终运行后端完整测试以及前端 TypeScript/Vite 构建。
