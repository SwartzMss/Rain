# 数据库设计概览

当前默认使用 SQLite，数据库文件由 `DATABASE_URL` 控制，默认示例为 `sqlite://../data/rain.db`。后端启动时会自动创建数据库文件的父目录，并执行 `CREATE TABLE IF NOT EXISTS` 初始化表结构。

## 设计取舍

- SQLite 适合当前本地 MVP：部署简单、无需单独数据库服务、方便重新启动项目。
- SQLite 连接启用 WAL、`synchronous=NORMAL` 和 30 秒 `busy_timeout`，降低读写互相阻塞的概率。
- 当前搜索使用 SQLite FTS5，文本日志按 chunk 建完整索引，启动时会回填尚未进入 FTS 索引的历史 `log_segments`。
- 上传请求只负责接收并保存文件；解压、行偏移、FTS 和基础事件解析在 `.tmp/{task_id}/staging` 后台执行。
- 后台任务全部成功后才移动到正式 bundle 目录并标记 `READY`；失败时清理 staging/正式目录和半成品 file/index/event 记录，仅保留 bundle 的 `FAILED` 状态。
- 后台解析使用流式读取和事务批量写入，日志索引每 5000 行提交一次，避免大文件一次性读入内存、逐行零散提交和过长写事务。
- 当前 `meta` 以 JSON 字符串存储在 TEXT 列中；后续如要对象存储或多节点部署，关键存储路径应提升为明确列。
- 生产化前建议引入迁移工具，不要长期依赖启动时建表。

## 表：issues

- `code` TEXT PK：Issue 编号（上传归属键）。
- `name` TEXT：显示名称（默认与 `code` 相同）。
- `description` TEXT：描述。
- `created_at` TEXT：创建时间，默认 `CURRENT_TIMESTAMP`。

## 表：bundles

- `id` TEXT PK：内部 bundle ID，由后端生成 UUID 字符串。
- `issue_code` TEXT：关联 `issues.code`，级联删除。
- `hash` TEXT UNIQUE：bundle 的公开 ID，前端和 API 使用它定位 bundle。
- `name` TEXT：bundle 显示名（当前为 `bundle-{hash}`）。
- `status` TEXT：上传/解析状态，上传后为 `PROCESSING`，后台处理成功后为 `READY`，失败为 `FAILED`。
- `size_bytes` INTEGER：本次上传总字节数。
- `content_size_bytes` INTEGER：计入 Issue 配额的最终可浏览文件总字节数；压缩包和目录本身不重复计入。
- `created_at` TEXT：创建时间，默认 `CURRENT_TIMESTAMP`。
- 索引：`idx_bundles_issue (issue_code, created_at DESC)`。

## 表：files

- `id` INTEGER PK AUTOINCREMENT。
- `bundle_id` TEXT：关联 `bundles.id`，级联删除。
- `parent_id` INTEGER：自关联父节点，级联删除。
- `name` TEXT：文件/目录名。
- `path` TEXT：bundle 内路径，如 `/{bundle_hash}/{file_name}`。
- `is_dir` INTEGER：是否目录，按 bool 读写。
- `size_bytes` INTEGER：文件大小，目录为 NULL。
- `line_count` INTEGER：文本文件行数，用于分页展示。
- `mime_type` TEXT：MIME。
- `status` TEXT：状态标签（预留）。
- `meta` TEXT：JSON 字符串，存储 `storage_path`、原始文件名等元数据。
- `created_at` TEXT：创建时间，默认 `CURRENT_TIMESTAMP`。
- 约束：`UNIQUE (bundle_id, path)`。
- 索引：`idx_files_parent`、`idx_files_bundle`、`idx_files_path`。

## 表：log_segments

- `id` INTEGER PK AUTOINCREMENT。
- `bundle_id` TEXT：关联 `bundles.id`，级联删除。
- `file_id` INTEGER：关联 `files.id`，级联删除。
- `timeline` TEXT：时间轴标签，当前固定为 `all`。
- `content` TEXT：日志 chunk 内容，通常最多 200 行，已去空行和空字节。
- `line_offset` INTEGER：chunk 起始原始行号，从 0 开始。
- `line_end` INTEGER：chunk 结束原始行号，从 0 开始。
- `chunk_index` INTEGER：文件内 chunk 序号，从 0 开始。
- `created_at` TEXT：创建时间，默认 `CURRENT_TIMESTAMP`。
- 索引：`idx_logs_bundle_timeline`、`idx_logs_file_chunk`；全文检索走 `log_segments_fts`。

## 表：log_events

- `id` INTEGER PK AUTOINCREMENT。
- `bundle_id` TEXT：关联 `bundles.id`，级联删除。
- `file_id` INTEGER：关联 `files.id`，级联删除。
- `segment_id` INTEGER：关联 `log_segments.id`，级联删除。
- `line_number` INTEGER：事件所在原始行号，从 0 开始。
- `timestamp` TEXT：基础解析出的时间戳，可为空。
- `level` TEXT：基础解析出的日志级别，如 `INFO`、`WARN`、`ERROR`。
- `component` TEXT：基础解析出的组件名，可为空。
- `message` TEXT：去掉时间戳/级别/组件后的消息。
- `raw` TEXT：原始日志行。
- `parser_name` TEXT：解析器名称，当前为 `basic-log-line`。
- `parser_confidence` REAL：基础置信度，供后续 AI/规则层判断可靠性。
- 索引：`idx_events_bundle_level`、`idx_events_file_line`。

## 表：log_line_offsets

- `file_id` INTEGER：关联 `files.id`，级联删除。
- `line_number` INTEGER：采样行号，从 0 开始。
- `byte_offset` INTEGER：该行在原始文件中的字节偏移。
- 主键：`(file_id, line_number)`。
- 用途：分页读取时先跳到最近采样点，再顺序读取目标页，避免每次从文件开头数行。

## 表：log_segments_fts

- SQLite FTS5 虚表。
- `content`：全文检索内容。
- `segment_id` UNINDEXED：关联 `log_segments.id`。
- `bundle_id` UNINDEXED：用于 bundle 范围过滤。
- `file_id` UNINDEXED：用于文件删除时清理索引。
- `timeline` UNINDEXED：预留 timeline 过滤。

## 关系与典型上传

- Issue -> 多个 Bundle：同一个 Issue 可多次上传，每次形成一个 Bundle。
- Bundle -> Files：单文件上传会形成一个顶层 file 节点；每一层 `.zip`、`.tar.gz`、`.tgz`、`.gz` 都保留原始压缩包节点，并在其下挂载一个 `{archive_name}_extracted` 解压目录。
- Files -> Log Segments：文本类文件（扩展名 log/txt 等或 content-type `text/*`）会流式读取并按 chunk 写入 `log_segments` 供搜索；非文本文件仅保留 `files` 记录。
- Files -> Line Offsets：文本类文件会每 1000 行记录一次 byte offset，用于 `/lines` 分页读取。
- 单行读取上限为 1 MB，超过后会丢弃到下一个换行符，并在索引/分页内容中追加 `[line truncated]` 标记。
- Log Segments -> Log Events：基础解析器会从日志行中提取 timestamp/level/component/message，写入 `log_events`，为后续聚合和 AI 分析准备。

## 上传流程

```mermaid
flowchart TD
    A[上传请求: issue_code + 文件] --> B{创建/复用 Issue?}
    B -->|无| C[写 issues 记录]
    B -->|有| D[复用 issues.code]
    C --> E[创建 Bundle 记录]
    D --> E
    E --> F[文件流式落盘到临时目录]
    F --> G[返回 PROCESSING]
    G --> H[后台任务移动文件到 bundle 目录]
    H --> I{是否支持的压缩包?}
    I -->|是| J[解压为目录树]
    I -->|否| K[单一文件节点]
    J --> J1{解压结果仍有支持的压缩包?}
    J1 -->|是| J
    J1 -->|否| L[写 files 树]
    K --> L
    L --> M{文本类?}
    M -->|是| N[流式读取并写 line offsets]
    M -->|否| O[仅 files 记录]
    N --> P[按 chunk 写完整 log_segments/FTS]
    P --> Q[基础解析写 log_events]
    Q --> R[Bundle 标记 READY]
```

递归解压、文本扫描和索引全部在 `.tmp/{task_id}/staging/{bundle_hash}` 中完成。嵌套深度、条目总数和解压总字节数由同一 bundle 共享预算；任一层损坏或超过安全限制时，任务标记为 `FAILED`，并删除 staging 文件及该 bundle 的 `files`、行偏移、FTS 和事件半成品记录。
