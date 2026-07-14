# Rain

Rain 是一个本地日志包浏览与检索工具。当前版本用于把文本日志或 `.zip`、`.tar.gz`、`.tgz`、`.gz` 压缩包上传到一个 Issue 下，浏览递归解压后的文件树，分页查看文本内容，并按关键词搜索日志。

默认使用 SQLite，本地启动不需要安装 PostgreSQL 或其他数据库服务。

## 快速启动

### 依赖

- Node.js 20+
- Rust 1.75+

### 1. 配置后端

复制环境变量示例：

```bash
cd backend
cp .env.example .env
```

默认配置如下，通常可以直接使用：

```dotenv
DATABASE_URL=sqlite://./data/rain.db
RAIN_DATA_ROOT=./data/uploads
RAIN_LOG_DIR=./log
SERVER_HOST=0.0.0.0
SERVER_PORT=8080
RESET_DB=false
```

### 2. 构建前端

```bash
cd frontend
npm install
npm run build
```

构建产物会写入 `frontend/dist`。后端编译时会把这个目录嵌入到可执行文件中。

### 3. 启动后端

```bash
cd backend
cargo run
```

打开 `http://localhost:8080` 即可使用应用。

健康检查：

```bash
curl http://localhost:8080/healthz
```

### 开发前端

```bash
cd frontend
npm install
npm run dev
```

开发时也可以继续使用 Vite dev server：`http://localhost:5173`。

如果使用 Vite dev server，在 `frontend/.env` 中设置：

```dotenv
VITE_API_BASE_URL=http://localhost:8080
```

## 构建发布包

当前不需要 nginx、systemd、证书或反向代理。前端页面会在后端编译时嵌入到可执行文件中，发布时不需要复制 `frontend/dist`。

Windows:

```bat
build-windows.bat
```

产物：

```text
release\Rain.exe
release\.env
```

Linux/macOS:

```bash
chmod +x ./build-linux.sh
./build-linux.sh
```

产物：

```text
release/rain
release/.env
```

手动构建时仍然需要先构建前端，再编译后端：

```bash
cd frontend
npm install
npm run build
```

```bash
cd backend
cargo build --release
```

Windows:

```powershell
.\backend\target\release\backend.exe
```

Linux/macOS:

```bash
./backend/target/release/backend
```

发布包包含可执行程序和外置 `.env` 配置文件：Windows 为 ZIP，Linux 为 tar.gz。解压后应保持两个文件位于同一目录；修改 `.env` 后重启 Rain 即可改变端口、数据库和数据目录等设置，不需要重新编译。程序会优先读取可执行文件同目录的 `.env`，因此从其他工作目录启动也能找到配置；已设置的系统环境变量优先级高于 `.env`。

默认配置会使用：

- SQLite 数据库：`./data/rain.db`
- 上传目录：`./data/uploads`
- 后端端口：`8080`

启动后访问 `http://localhost:8080`。首次运行后会在工作目录附近生成 `data/` 和 `log/`，这是 SQLite、上传文件和运行日志的正常运行时数据。

## 使用流程

1. 打开 `http://localhost:8080`。
2. 新建或选择一个 Issue，例如 `CN013`。
3. 在选中的 Issue 下拖拽或点击上传 `.log`、`.txt`、`.zip` 文件。
4. 点击 Issue 的“查看”打开文件浏览页。
5. 在左侧文件树选择文件，右侧会显示文本预览。
6. 在搜索框输入关键词，可搜索当前 Issue 下已索引的文本日志。

## 当前支持

- Issue 列表、打开、删除。
- 多文件上传。
- `.log`、`.txt` 等文本文件索引。
- `.zip`、`.tar.gz`、`.tgz`、`.gz` 后台递归解压并写入文件树，内层日志同样会建立索引和支持分页查看。
- 上传安全限制：单文件 512 MB、单次 2 GB、最多 100 个文件。
- 压缩包防护按整个 bundle 累计：最多 16 层嵌套压缩、10000 个条目、500 MB 解压内容；单条目最多 100 MB、条目路径最多 16 层、完整输出路径最多 1024 个 UTF-16 字符、压缩比上限 100:1。
- 文件树浏览。
- 文本文件分页读取，后端按行偏移索引快速跳转。
- 单行超过 1 MB 时索引和分页展示会截断该行，并标记 `[line truncated]`。
- Issue 范围和 bundle 范围关键词搜索，文本文件会完整建立 SQLite FTS5 chunk 索引；搜索结果返回命中附近摘要，默认 50 条、最多 100 条。
- 原始文件下载。
- 删除 Issue、Bundle、单个文件节点。
- 可选过期清理：设置 `RAIN_RETENTION_DAYS` 后启动时清理过期上传。

## 当前限制

- 暂不支持 `.rar`、`.7z` 解压。
- 上传传输有前端进度；解压和索引在后台任务执行，当前有 `PROCESSING/READY/FAILED` 状态轮询，没有细粒度解析百分比。
- 后台处理先在 `.tmp/{task_id}/staging` 中完成；成功后移动到正式目录，失败会清理半成品文件和索引，只保留失败任务状态。
- SQLite 使用 WAL 和 30 秒 busy timeout；日志索引每 5000 行批量提交一次，后台解压/索引任务最多 2 个并发。
- `.zip`、`.tar.gz`、`.tgz`、`.gz` 会在同一 staging bundle 内递归处理并共享安全限额；暂不支持后台任务超时/取消。
- 搜索使用 SQLite FTS5，并按日志 chunk 建完整索引。
- timeline 目前固定为 `all`。
- 已有基础结构化日志事件提取，事件查询和 AI 分析能力尚未接入。

## 数据位置

默认数据都在仓库根目录下的 `data/`，该目录已被 `.gitignore` 忽略：

- SQLite 数据库：`data/rain.db`
- 上传和解压文件：`data/uploads/`
- 后端运行日志：`log/YYYY-MM-DD.backend.log`（按天轮转）

如果想清空本地数据，可以停止服务后删除 `data/`，或临时设置：

```dotenv
RESET_DB=true
```

注意：`RESET_DB=true` 会重建表，并清空配置的数据目录，仅适合本地调试。

## 常用命令

后端检查：

```bash
cd backend
cargo fmt --check
cargo check
cargo test
```

前端构建：

```bash
cd frontend
npm run build
```

构建后端 EXE：

```bash
cd backend
cargo build --release
```

查看后端日志：

```bash
tail -f log/$(date +%F).backend.log
```

Windows PowerShell 可用：

```powershell
Get-Content (Join-Path log "$((Get-Date).ToString('yyyy-MM-dd')).backend.log") -Wait
```

## API 摘要

### Issues / Bundles

- `GET /api/issues`
- `POST /api/issues`
- `GET /api/issues/{issueCode}`
- `DELETE /api/issues/{issueCode}`
- `DELETE /api/issues/{issueCode}/bundles/{bundleHash}`

### Upload

- `POST /api/issues/{issueCode}/uploads`：返回 `202 Accepted`，响应包含 `task_id`、`bundle_hash` 和初始 `PROCESSING` 状态。
- `GET /api/uploads/{taskId}`：查询后台解压/索引任务状态。

Multipart 字段：

- `files`

### Files

- `GET /api/files/v1/{bundleId}/files/root`
- `GET /api/files/v1/{bundleId}/files/{fileId}`
- `GET /api/files/v1/{bundleId}/files/{fileId}/content`
- `GET /api/files/v1/{bundleId}/files/{fileId}/lines?start=0&limit=200`
- `GET /api/files/v1/{bundleId}/files/{fileId}/download`
- `DELETE /api/files/v1/{bundleId}/files/{fileId}`

### Search

- `GET /api/log/v2/{bundleId}/search?q=keyword`
- `GET /api/issues/{issueCode}/search?q=keyword`

## 后续方向

短期优先级：

1. 解析任务细粒度进度、取消和失败重试。
2. 结构化事件查询 API，例如按 level、component、时间范围过滤。
3. 搜索任务取消、后台搜索和并发限制。
4. 更完整的日志 parser 规则和多行异常合并。
5. 带日志引用的 AI 分析。

数据库细节见 [doc/DB.md](doc/DB.md)。
