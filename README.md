# Rain

Rain 是一个面向开发者的日志查看 Web 应用，提供上传、解析、搜索本地日志/压缩包的能力，帮助快速定位问题。当前目标是完成文件上传和多视图浏览的 MVP，后续会迭代实时推送与权限控制。

## 功能概览

- **多格式上传**：支持 `.txt`、`.log`、`.zip`、`.tar.gz` 等文件类型，单次上传允许多个文件。
- **Files View**：展示上传历史及压缩包内部结构，按层级浏览，支持惰性展开嵌套压缩包。
- **Logs View**：聚合所有纯文本日志，提供关键词搜索和文本预览。
- **压缩包处理**：支持 `.zip`、`.tar.gz` 解压，自动识别编码格式，记录解析状态。
- **安全限制**：对单文件大小、展开总字节数、递归深度设定硬限制，避免 Zip Bomb。
- **扩展计划**：WebSocket 实时推送、日志过滤器、用户认证在 roadmap 中排队。

## 架构与实现摘要

| 层级 | 说明 |
| --- | --- |
| 前端 | React + Vite + TailwindCSS，单页应用，提供 Files / Logs 两个主视图；通过 REST API 交互。 |
| 后端 | Rust + Actix-Web，负责上传、压缩解析、文本索引、API；内置 Actix WebSocket 支持，未来可用于实时流。 |
| 存储 | 文件内容保存在 `data/uploads/<uuid>` 目录；解析出的元数据、索引信息放在 SQLite（后续可替换为 Postgres）。 |
| 搜索 | 服务端使用 SQLite FTS5 构建倒排索引，支持简单关键词匹配；返回命中片段供前端展示。 |

### 数据流

1. 前端通过 `/api/uploads` 上传文件；后端对每个文件生成 UUID 并落盘。
2. 后端同步解析文本文件，异步处理压缩包首层目录，记录结构信息。
3. 前端请求 `/api/files` 获取 Files View 树状数据；点击目录时再调用 `/api/files/{id}/children` 触发惰性展开。
4. Logs View 请求 `/api/logs/search?q=<keyword>`，由后端返回匹配的文件段落。

## 前后端 API 设计

围绕“上传 → 解析 → 浏览 → 搜索 → 埋点”这条主流程，前端会调用以下 REST 接口与后端交互，示例中的 `bundleId` 使用 `lp1yp7` 这一演示值（每次上传都会生成不同的 ID）。

### 上传与案件上下文

- `POST /api/uploads`：多文件上传，返回每个文件的 `uploadId` 与初始解析状态。
- `GET /api/uploads/{uploadId}`：轮询单次上传的任务进度、错误详情。
- `GET /api/issues/{issueId}` / `GET /api/analysissuite/{owner}/{caseId}`：查询某个案件下的 log bundle 列表。示例响应：

```json
{
  "name": "CN013",
  "log_bundles": [
    { "hash": "qqmzk6", "name": "0608.zip", "status": { "upload_status": "READY" } },
    { "hash": "lp1yp7", "name": "0704.zip", "status": { "upload_status": "READY" } }
  ]
}
```

### 文件浏览（Files View）

- `GET /api/files/v1/{bundleId}/files/{fileId}/metadata?include_rain_metadata=true`：返回文件/目录的基础信息、扩展元数据。
- `GET /api/files/v1/{bundleId}/files/{fileId}`：实际文件内容或子节点数据（如 `GET .../lp1yp7/files/490`）。
- `GET /api/files/v1/{bundleId}/search?path=...`：按路径快速查找 `fileId`。
- `POST /api/v2/file_browser_stats/{bundleId}/FileBrowser_fileselect`：记录用户展开/选择行为，便于之后分析。

### 日志视图（Logs View）

- `GET /api/log/v2/{bundleId}/_info`：提供可选时间线与默认值，样例：

```json
{
  "timelines": [
    { "name": "all", "label": "All files", "status": "uploaded" },
    { "name": "runtime", "label": "Runtime", "status": "uploaded" },
    { "name": "pm_4_startup", "label": "PM 4 Startup", "status": "uploaded" }
  ],
  "default": "runtime"
}
```

- `GET /api/log/v2/{bundleId}/search?q=...&timeline=...`：关键词搜索命中片段。
- `GET /api/log/v2/{bundleId}/streams/{timeline}?from=...&size=...`：流式按时间线加载日志内容。

### 埋点与统计

- `POST https://rain-umami.rain-dev.dyn.nesc.net/api/send`：前端页面埋点上报，其中 `cache`、`sessionId`、`visitId` 来自服务端响应：

```json
{
  "cache": ".eyJ3ZWJzaXRlSWQiOiI2OTRjMjcyOS02YjAwLTQyNTQtYmEzNC1kNGE5ZmIzYzYwMzMiLCJzZXNzaW9uSWQiOiIzOTEwNTFjZC0xNjgxLTUyNTUtOTVkMy0zM2QyMzU4MTk3NDAiLCJ2aXNpdElkIjoiODkzMjFlMGQtNDJiNi01ODMwLTg4ZjAtODg4ZTkyODU0M2M1IiwiaWF0IjoxNzY1NTA5MzI4fQ.Bob5N1gFJ6kIzaEi_T0_-0UKCFYrwnaNdhe4pVfbT8w",
  "sessionId": "391051cd--5255-95d3-33d235819740",
  "visitId": "-42b6-5830-88f0-888e928543c5"
}
```

- `POST /api/file_browser_stats/...` 等埋点接口用于记录 Files View 的交互；后续也可扩展日志筛选、搜索等事件。

## 数据库设计

采用 PostgreSQL 存储结构化信息，文件内容则保留在磁盘/对象存储中，仅在表中记录路径和校验信息，核心表如下：

| 表 | 关键字段 | 说明 |
| --- | --- | --- |
| `projects` | `id (uuid)`, `code`, `name`, `description`, `owner`, `archived`, `created_at` | 项目/问题单目录（如 `CN013`），所有 bundle 隶属于某个项目。 |
| `bundles` | `id (uuid)`, `project_id`, `name`, `status`, `upload_user`, `size_bytes`, `file_count`, `error_msg`, `raw_path`, `created_at`, `finished_at` | 一次上传与解析的整体信息，`id` 类似 `lp1yp7`，与解析任务状态挂钩。 |
| `files` | `id (bigserial)`, `bundle_id`, `parent_id`, `name`, `path`, `size_bytes`, `mime_type`, `is_dir`, `timeline`, `compression_level`, `status`, `checksum`, `storage_path`, `created_at` | 解析出的文件树节点，`parent_id` 形成层级，`storage_path` 指向解压后的文件。 |
| `file_metadata` | `file_id`, `meta jsonb` | 存放需要额外返回的雨量元数据（编码、解析策略等），供 `?include_rain_metadata=true` 使用。 |
| `timelines` | `id`, `bundle_id`, `name`, `label`, `status`, `owner`, `order_index`, `is_default` | `GET /api/log/v2/{bundle}/_info` 的数据来源。 |
| `log_segments` | `id`, `bundle_id`, `file_id`, `timeline`, `offset`, `length`, `content`, `tsv tsvector` | 日志全文索引，每行一段文本；`tsvector` 建 GIN 索引以支持搜索。 |
| `events_file_browser`（可选） | `id`, `bundle_id`, `file_id`, `event_type`, `payload jsonb`, `session_id`, `occurred_at` | 保存 Files View 埋点数据，便于行为分析。 |

> 如需细分上传请求，可额外保留 `uploads` 表（记录原始上传信息）并与 `bundles` 关联。权限/多租户场景可引入 `orgs`、`permissions` 等扩展表。

解析线程负责在解压完毕后批量写入 `files`、`timelines`、`log_segments`，并更新 `bundles.status`；大文件仍以文件系统路径的形式暴露，由 API 在请求时读取返回。

## 设计路线图（Design Roadmap）

结合现阶段需求，按优先级划分的建设顺序如下：

1. **项目与权限框架**：实现 `projects` / `bundles` 基础 CRUD、鉴权模型，确保上传与浏览都需绑定项目。
2. **上传与解析管线**：后端完成多文件上传接口、磁盘落盘、异步解压/解析线程、状态轮询/WebSocket 推送。
3. **PostgreSQL 迁移**：根据“数据库设计”章节编写迁移脚本与 DAO 层，接入 `bundles`、`files`、`timelines`、`log_segments` 等表。
4. **Files View API**：实现文件树增量加载、`metadata`/`content` 接口、路径搜索、文件导出，并接入 FileBrowser 埋点。
5. **Logs View 流程**：构建时间线生成器、日志全文检索接口、流式拉取 API，支持按 timeline/关键词过滤。
6. **监控与埋点**：完善 Umami 上报、FileBrowser 行为统计、解析任务 metrics（Prometheus/Grafana）。
7. **高级特性**：目录导出再压缩、权限细粒度控制（角色/组织）、对象存储支持、实时推送/协作等迭代需求。

## 技术栈

- **前端**：React 18、Vite、TailwindCSS、TypeScript。
- **后端**：Rust 1.75+、Actix-Web、Tokio、`zip`、`flate2`、`tar`、`walkdir`、`rusqlite`（FTS5）。
- **工具**：pnpm 或 npm、Cargo、SQLite 3.42+。

## 安装与运行

### 前置依赖

- Node.js 20+（推荐配合 pnpm）
- Rust 1.75+ 与 Cargo
- SQLite（用于本地测试，可随 binary 自动创建）

### 1. 克隆仓库

```bash
git clone https://github.com/your-username/rain.git
cd rain
```

### 2. 启动前端

```bash
cd frontend
npm install
npm run dev  # 默认 http://localhost:5173
```

环境变量：

- `VITE_API_BASE_URL`：后端 API 地址，默认 `http://localhost:8080`.

### 3. 启动后端

```bash
cd backend
cargo run
```

默认监听 `http://localhost:8080`，上传文件保存在 `data/uploads` 目录，可通过 `RAIN_DATA_ROOT` 修改。

### 4. 访问应用

浏览器打开 `http://localhost:5173`，前端会将请求代理到后端 API。

> Docker 镜像暂不提供，待 MVP 稳定后再补充。

## 运行时策略

- **惰性展开**：默认只解析压缩包第一层；当用户展开某个目录时，后端才继续解压，并在解析完成前返回“loading”状态。
- **大小限制**：单文件 50 MB，单次上传总量 200 MB，展开后的累计大小上限 500 MB；超限时直接拒绝。
- **递归深度**：最多 5 层嵌套，再深将提示用户手动拆包。
- **索引更新**：文本文件解析完即写入 FTS5 索引；压缩包中文件在惰性展开后再索引。

## Roadmap

1. WebSocket 实时推送最新日志。
2. 日志过滤器：时间、级别、关键词组合过滤。
3. 用户认证与权限管理，多租户隔离。
4. 对象存储适配（S3/MinIO），支持集群部署。
5. Docker Compose / K8s 部署模版。

## 贡献指南

1. Fork 仓库并创建 `feature/<topic>` 分支。
2. 提交前运行：
   ```bash
   cd frontend && npm test && npm run lint
   cd backend && cargo fmt && cargo clippy && cargo test
   ```
3. 提交 PR 时附带变更描述、截图/日志。
4. 遵循 MIT 许可证。

## License

MIT License（详见 `LICENSE` 文件）。

## Open Issues

1. **对象存储**：当前仅使用本地磁盘；若要云端部署，需要评估 S3/MinIO 接入和多节点一致性策略。
2. **Zip Bomb 防护实现细节**：需要明确如何检测压缩比、如何提前中断解压。
3. **搜索增强**：是否支持正则、模糊匹配和命中高亮仍待定义。
4. **日志编码**：目前假设 UTF-8；多语言日志的编码识别策略需要补充。
