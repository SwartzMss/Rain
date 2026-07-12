# 数据库设计概览

当前默认使用 SQLite，数据库文件由 `DATABASE_URL` 控制，默认示例为 `sqlite://../data/rain.db`。后端启动时会自动创建数据库文件的父目录，并执行 `CREATE TABLE IF NOT EXISTS` 初始化表结构。

## 设计取舍

- SQLite 适合当前本地 MVP：部署简单、无需单独数据库服务、方便重新启动项目。
- 当前搜索使用 `LIKE`；后续日志量增加后，建议引入 SQLite FTS5。
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
- `status` TEXT：上传/解析状态，当前主要写入 `READY`。
- `size_bytes` INTEGER：本次上传总字节数。
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
- `content` TEXT：日志行内容（去空行/截断后）。
- `line_offset` INTEGER：原始行号，从 0 开始。
- `created_at` TEXT：创建时间，默认 `CURRENT_TIMESTAMP`。
- 索引：`idx_logs_bundle_timeline`、`idx_logs_content`。

## 关系与典型上传

- Issue -> 多个 Bundle：同一个 Issue 可多次上传，每次形成一个 Bundle。
- Bundle -> Files：单文件上传会形成一个顶层 file 节点；ZIP 上传会形成原始 ZIP 节点和一个 `{zip_name}_extracted` 解压目录。
- Files -> Log Segments：文本类文件（扩展名 log/txt 等或 content-type `text/*`）会按行写入 `log_segments` 供搜索；非文本文件仅保留 `files` 记录。

## 上传流程

```mermaid
flowchart TD
    A[上传请求: issue_code + 文件] --> B{创建/复用 Issue?}
    B -->|无| C[写 issues 记录]
    B -->|有| D[复用 issues.code]
    C --> E[创建 Bundle 记录]
    D --> E
    E --> F[文件落盘]
    F --> G{是否 ZIP?}
    G -->|是| H[同步解压为目录树]
    G -->|否| I[单一文件节点]
    H --> J[写 files 树]
    I --> J
    J --> K{文本类?}
    K -->|是| L[逐行写 log_segments]
    K -->|否| M[仅 files 记录]
```
