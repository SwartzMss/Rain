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
