# 开发与构建

[返回使用指南](../README.md)

## 源码启动

准备 Node.js 22（与 CI 一致）和 Rust stable，安装 rustfmt、clippy。以下命令从仓库根目录开始执行。

先复制 `backend/.env.example` 为 `backend/.env`，填写 `RAIN_BOOTSTRAP_ADMIN_PASSWORD`。首次安装要求见[部署与维护](operations.md)。

构建前端，然后启动后端：

```bash
cd frontend
npm ci
npm run build
cd ../backend
cargo run --locked
```

访问 <http://localhost:8078>。后端编译时嵌入 `frontend/dist`，所以第一次编译前必须先构建前端。

## 前端开发

后端运行时，在另一个终端从仓库根目录执行：

```bash
cd frontend
npm ci
npm run dev
```

访问 <http://localhost:5173>。Vite 将同源 `/api` 请求代理到 `http://localhost:8078`。后端端口不同时，在 `frontend/.env` 配置：

```dotenv
RAIN_DEV_API_PROXY_TARGET=http://localhost:8078
```

该变量只用于开发代理。发布环境由 Rain 提供页面和 API。

## 构建发布目录

在仓库根目录执行 Windows 脚本：

```powershell
$env:RAIN_RELEASE_VERSION = 'vX.Y.Z' # 替换为本次构建版本
.\build-windows.bat
```

Linux：

```bash
RAIN_RELEASE_VERSION=vX.Y.Z bash ./build-linux.sh
```

脚本会安装并构建前端、检查和测试后端、编译 release 程序，输出到 `release/`。Windows 程序名为 `Rain.exe`，Linux 为 `rain`；同时包含 `.env` 和 `VERSION`。脚本会覆盖发布目录内的这三个产物，运行实例应放在独立目录。运行前填写发布目录 `.env` 中的管理员密码。

正式发布工作流见 [release.yml](../.github/workflows/release.yml)，当前提供 Windows x64 和 Linux x64 预编译包。

手动编译也必须先完成前端构建，再在 `backend/` 执行 `cargo build --release --locked`。默认 features 已包含 Tantivy。

## 检查与测试

前端（在 `frontend/`）：

```bash
npm ci
npm run lint
npm run build
npm test
```

后端（在 `backend/`）：

```bash
cargo fmt --check
cargo check --locked
cargo clippy --locked -- -D warnings
cargo test --locked
cargo check --locked --no-default-features
cargo test --locked --no-default-features
```

SQLite runtime writer 审计（仓库根目录、Bash 环境）：

```bash
bash scripts/audit-runtime-sqlite-writers.sh
```

Windows Git 若将脚本检出为 CRLF，Git Bash 可能在 `set -euo pipefail` 处失败；检查 `git ls-files --eol scripts/audit-runtime-sqlite-writers.sh`，使用 LF checkout 或 Linux 环境执行。

完整 CI 定义见 [ci.yml](../.github/workflows/ci.yml)。大日志性能测试默认忽略，运行方法和统计边界见[性能基准](performance/large-log-baseline.md)。

Windows 手动验证可在启用 Defender 或目录索引时上传大量小文件的压缩包，确认短暂文件锁定后能进入 READY；持续占用超过重试窗口时应进入 FAILED，文件树不应暴露半成品。
