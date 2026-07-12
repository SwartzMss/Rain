# Rain

Rain 是一个本地日志包浏览与检索工具。当前版本用于把 `.log`、`.txt` 或 `.zip` 上传到一个 Issue 下，浏览解压后的文件树，预览文本内容，并按关键词搜索日志行。

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
DATABASE_URL=sqlite://../data/rain.db
RAIN_DATA_ROOT=../data/uploads
RAIN_LOG_DIR=../log
SERVER_HOST=0.0.0.0
SERVER_PORT=8080
RESET_DB=false
```

### 2. 启动后端

```bash
cd backend
cargo run
```

后端默认地址：`http://localhost:8080`

健康检查：

```bash
curl http://localhost:8080/healthz
```

### 3. 启动前端

```bash
cd frontend
npm install
npm run dev
```

前端默认地址：`http://localhost:5173`

如果前端和后端不是同源部署，在 `frontend/.env` 中设置：

```dotenv
VITE_API_BASE_URL=http://localhost:8080
```

## 直接运行 EXE

当前不需要 nginx、systemd、证书或反向代理。后端可以直接编译成可执行文件运行：

```bash
cd backend
cargo build --release
```

Windows:

```powershell
.\target\release\backend.exe
```

Linux/macOS:

```bash
./target/release/backend
```

运行前可在同目录准备 `.env`，或直接依赖默认值。默认会使用：

- SQLite 数据库：`../data/rain.db`
- 上传目录：`./data/uploads`
- 后端端口：`8080`

前端当前仍按 Vite 应用运行：

```bash
cd frontend
npm run dev
```

## 使用流程

1. 打开 `http://localhost:5173`。
2. 在首页输入或选择一个 Issue ID，例如 `CN013`。
3. 拖拽或点击上传 `.log`、`.txt`、`.zip` 文件。
4. 双击 Issue 打开文件浏览页。
5. 在左侧文件树选择文件，右侧会显示文本预览。
6. 在搜索框输入关键词，可搜索当前 Issue 下已索引的文本日志。

## 当前支持

- Issue 列表、打开、删除。
- 多文件上传。
- `.log`、`.txt` 等文本文件索引。
- `.zip` 同步解压并写入文件树。
- 文件树浏览。
- 文本文件 64 KB 预览。
- Issue 范围和 bundle 范围关键词搜索。
- 删除 Issue、Bundle、单个文件节点。

## 当前限制

- 暂不支持 `.tar.gz`、`.rar`、`.7z` 解压。
- ZIP 上传时同步解压，尚未做后台任务和进度轮询。
- 尚未实现完整 Zip Bomb 防护。
- 搜索目前使用 SQLite `LIKE`，大数据量后应切换到 SQLite FTS5。
- timeline 目前固定为 `all`。
- AI 分析能力尚未接入。

## 数据位置

默认数据都在仓库根目录下的 `data/`，该目录已被 `.gitignore` 忽略：

- SQLite 数据库：`data/rain.db`
- 上传和解压文件：`data/uploads/`
- 后端运行日志：`log/backend.log`

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
tail -f log/backend.log
```

Windows PowerShell 可用：

```powershell
Get-Content log\backend.log -Wait
```

## API 摘要

### Issues / Bundles

- `GET /api/issues`
- `GET /api/issues/{issueCode}`
- `DELETE /api/issues/{issueCode}`
- `DELETE /api/issues/{issueCode}/bundles/{bundleHash}`

### Upload

- `POST /api/uploads`

Multipart 字段：

- `issue_code`
- `files`

### Files

- `GET /api/files/v1/{bundleId}/files/root`
- `GET /api/files/v1/{bundleId}/files/{fileId}`
- `GET /api/files/v1/{bundleId}/files/{fileId}/content`
- `DELETE /api/files/v1/{bundleId}/files/{fileId}`

### Search

- `GET /api/log/v2/{bundleId}/search?q=keyword`
- `GET /api/issues/{issueCode}/search?q=keyword`

## 后续方向

短期优先级：

1. 异步解析任务、进度状态、失败重试。
2. ZIP 安全限制：总大小、文件数、深度、超时、压缩比。
3. SQLite FTS5 全文检索。
4. 结构化日志事件提取。
5. 带日志引用的 AI 分析。

数据库细节见 [doc/DB.md](doc/DB.md)。
