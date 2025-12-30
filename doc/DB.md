# 数据库设计概览

- Schema：由环境变量 `DATABASE_SCHEMA` 控制（默认 `Rain`），`search_path` 设置为 `<schema>, public`。
- 扩展：`pgcrypto`（生成 UUID）、`pg_trgm`（trigram 索引）。
- 枚举类型：`upload_status` = `READY | PROCESSING | FAILED | PENDING`。

## 表：issues
- `code` TEXT PK：Issue 编号（上传归属键）。
- `name` TEXT：显示名称（默认与 `code` 相同）。
- `description` TEXT：描述。
- `created_at` TIMESTAMPTZ：创建时间，默认 `now()`。

## 表：bundles
- `id` UUID PK（`gen_random_uuid()`）。
- `issue_code` TEXT：关联 `issues.code`，级联删除。
- `hash` TEXT UNIQUE：bundle 的公开 ID。
- `name` TEXT：bundle 显示名（当前为 `bundle-{hash}`）。
- `status` upload_status：默认 `PENDING`。
- `size_bytes` BIGINT：总字节数。
- `created_at` TIMESTAMPTZ：默认 `now()`。
- 索引：`idx_bundles_issue (issue_code, created_at DESC)`。

## 表：files
- `id` BIGSERIAL PK。
- `bundle_id` UUID：关联 `bundles.id`，级联删除。
- `parent_id` BIGINT：自关联父节点，级联删除。
- `name` TEXT：文件/目录名。
- `path` TEXT：bundle 内的绝对路径（如 `/hash/file.log`）。
- `is_dir` BOOLEAN：是否目录。
- `size_bytes` BIGINT：文件大小（目录为 NULL）。
- `mime_type` TEXT：MIME。
- `status` TEXT：状态标签（预留）。
- `meta` JSONB：存储路径、原始文件名等元数据。
- `created_at` TIMESTAMPTZ：默认 `now()`。
- 约束：`UNIQUE (bundle_id, path)`。
- 索引：`idx_files_parent (parent_id)`、`idx_files_bundle (bundle_id)`、`idx_files_path_trgm` (GIN + `gin_trgm_ops`)。

## 表：log_segments
- `id` BIGSERIAL PK。
- `bundle_id` UUID：关联 `bundles.id`，级联删除。
- `file_id` BIGINT：关联 `files.id`，删除时置 NULL。
- `timeline` TEXT：时间轴标签（默认 `all`）。
- `content` TEXT：行内容（去空行/截断后）。
- `line_offset` BIGINT：行号。
- `created_at` TIMESTAMPTZ：默认 `now()`。
- `tsv` tsvector：`to_tsvector('simple', content)` 生成，存储列。
- 索引：`idx_logs_bundle_timeline (bundle_id, timeline)`、`idx_logs_tsv` (GIN)。

## 关系与典型上传
- Issue → 多个 Bundle：同一个 issue 可多次上传，每次形成一条 Bundle（容器）。
- Bundle → Files：无论上传单文件还是 ZIP，都落在一个 Bundle 下。单文件只会有一个顶层 file 节点；ZIP 会解压成目录树（根节点 `is_dir=true`，其下是压缩包内文件/目录）。
- Bundle/Files → Log Segments：文本类文件（扩展名 log/txt 或 content-type `text/*`）会按行写入 `log_segments` 供搜索；非文本文件仅保留 `files` 记录。
- 路径形态：`files.path` 形如 `/{bundle_hash}/{file_name}`，ZIP 解压后的目录与文件在同一路径前缀下展开。

## 上传流程（Mermaid）
```mermaid
flowchart TD
    A[上传请求: issue_code + 文件] --> B{创建/复用 Issue?}
    B -->|无| C[写 issues 记录]
    B -->|有| D[复用 issues.code]
    C --> E[创建 Bundle 记录<br/>status=PENDING]
    D --> E
    E --> F[文件落盘<br/>路径 /{hash}/...]
    F --> G{是否 ZIP?}
    G -->|是| H[解压为目录树]
    G -->|否| I[单一文件节点]
    H --> J[写 files 树<br/>目录+子文件]
    I --> J[写 files 单节点]
    J --> K{文本类?}
    K -->|是| L[逐行写 log_segments<br/>timeline=all,line_offset]
    K -->|否| M[仅 files 记录]
    L --> N[Bundle 状态更新 READY/FAILED...]
    M --> N
```
