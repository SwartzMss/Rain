# Rain

Rain 是一个本地日志包浏览与检索工具。当前版本用于把文本日志或 `.zip`、`.tar.gz`、`.tgz`、`.gz`、`.7z` 压缩包上传到一个 Issue 下，浏览递归解压后的文件树，分页查看文本内容，并按关键词搜索日志。

当前版本为 `v0.1.0`。默认使用 Tantivy 作为日志搜索后端，SQLite 继续承担本地控制面；本地启动不需要安装 PostgreSQL 或其他数据库服务。v0.1.x 是全新初始版本，不兼容旧的 SQLite 搜索数据目录。

## 快速启动

### 依赖

- Node.js 20+
- Rust 1.85+（项目使用 Rust 2024 edition）

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
SERVER_PORT=8078
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

启动时会先执行 SQLx 数据库 migration，再进行恢复阶段和启动 HTTP 服务。空数据库会创建当前 baseline；已有无 migration metadata 的旧数据库会先验证完整 schema，缺失或被人工改坏的表、索引、约束或 FTS 定义会直接阻止启动。默认 Tantivy 模式还会拒绝包含旧 SQLite 搜索 Bundle 的数据目录，并提示使用新的数据库和数据目录。详细规则见 [`doc/DB.md`](doc/DB.md)。

打开 `http://localhost:8078` 即可使用应用。

健康检查：

```bash
curl http://localhost:8078/healthz
# 检查 SQLite 与数据目录是否已就绪
curl -i http://localhost:8078/readyz
```

### 开发前端

```bash
cd frontend
npm install
npm run dev
```

开发时也可以继续使用 Vite dev server：`http://localhost:5173`。

Vite 会把浏览器的同源 `/api` 请求代理到默认的 `http://localhost:8078`。如果后端
开发端口不同，可在 `frontend/.env` 中设置仅供开发服务器使用的代理目标：

```dotenv
RAIN_DEV_API_PROXY_TARGET=http://localhost:8078
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
release\VERSION
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
release/VERSION
```

手动构建时仍然需要先构建前端，再编译后端。后端默认 feature 已包含 Tantivy，不需要额外的 `--features` 参数：

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

发布包包含可执行程序、外置 `.env` 配置文件和记录版本的 `VERSION` 文件：Windows 为 ZIP，Linux 为 tar.gz。解压后应保持三个文件位于同一目录；修改 `.env` 后重启 Rain 即可改变端口、数据库和数据目录等设置，不需要重新编译。程序会优先读取可执行文件同目录的 `.env`，因此从其他工作目录启动也能找到配置；已设置的系统环境变量优先级高于 `.env`。

### 配置分层

Rain 不再把所有业务限制当作启动环境变量。程序优先读取可执行文件同目录的 `.env`，找不到时读取当前工作目录的 `.env`，系统环境变量优先级最高。

#### 启动配置

下面这些值决定数据库、文件目录和监听服务，放在 `.env` 中并在重启时生效：

```dotenv
DATABASE_URL=sqlite://./data/rain.db
RAIN_DATA_ROOT=./data/uploads
RAIN_LOG_DIR=./log
SERVER_HOST=0.0.0.0
SERVER_PORT=8078
RESET_DB=false
```

首次安装还可以用 `RAIN_BOOTSTRAP_ADMIN_USERNAME` 和 `RAIN_BOOTSTRAP_ADMIN_PASSWORD` 创建管理员；已有管理员不会被它们覆盖。

#### DB 热配置

Issue 配额、归档工作区、上传临时空间、文件预览和分页、搜索结果/窗口、临时结果容量与超时、注册与认证限流、Issue 非活跃天数以及自动清理白名单，都保存在 SQLite 的 `system_settings` 中。管理员在 `/admin/settings` 修改，页面会标出即时生效或需要重启的字段；已初始化数据库中的值优先于旧 ENV。

上传处理并发、Tantivy writer 数量和 writer heap 也保存于 `system_settings`。它们可以选择 `Manual` 或 `Auto`，但只有这 3 项会进入启动时的 runtime adaptive engine；运行时查询并发是内部保护参数，不开放配置。

文件内容搜索不再限制累计扫描字节数或源文件数量；旧的 `RAIN_TEMP_RESULT_MAX_SCAN_BYTES`、`RAIN_TEMP_RESULT_MAX_SOURCES` 环境变量及对应数据库配置不再生效。单次搜索仍受结果容量和扫描超时约束，并保留全局临时结果配额与并发保护。

#### 高级启动调优与兼容入口

`RAIN_SEARCH_BACKEND` 默认是 `tantivy`；`sqlite_fts` 只用于 legacy/诊断场景。`RAIN_INDEXING_MAX_INDEXED_LINE_SIZE` 仅保留为高级启动与旧版本迁移入口，普通部署无需设置。旧业务 ENV 只会在对应 `system_settings` 字段尚未初始化时导入一次，之后修改 ENV 不会覆盖数据库值。

默认配置会使用：

- SQLite 数据库：`./data/rain.db`
- 上传目录：`./data/uploads`
- 后端端口：`8078`

启动后访问 `http://localhost:8078`。首次运行后会在工作目录附近生成 `data/` 和 `log/`，这是 SQLite、上传文件和运行日志的正常运行时数据。

## 使用流程

1. 打开 `http://localhost:8078`。
2. 通过右上角“注册”创建账户；注册成功后使用用户名和密码登录。
3. 新建或选择一个 Issue，例如 `CN013`。
4. 在选中的 Issue 下拖拽或点击上传 `.log`、`.txt`、`.zip`、`.7z` 文件。
5. 点击 Issue 的“查看”打开文件浏览页。
6. 在左侧文件树选择文件，右侧会显示文本预览。
7. 在搜索框输入关键词，可搜索当前 Issue 下已索引的文本日志。

## 用户认证

Rain 支持用户名和密码注册、登录、查询当前身份、修改密码、当前设备退出和全部设备退出。用户名长度为
3～32，只允许字母、数字、`.`、`_`、`-`，且不区分大小写；密码长度为
8～128 个字符。密码使用 Argon2id 保存，登录 Session 使用 HttpOnly Cookie，
数据库只保存 Session Token 的 SHA-256 哈希。

注册成功后不会自动登录。当前版本没有邮箱、手机号或自助找回密码功能；忘记密码
时只能由部署管理员直接维护数据库。当前版本面向可信内网 HTTP 部署，必须通过 Rain
后端提供的页面同源访问 API，不支持独立部署在其他来源的浏览器前端。

游客可以查看和搜索，但不能下载文件或临时搜索结果；创建 Issue、上传、删除 Issue、删除
Bundle、删除文件节点以及删除临时搜索结果需要登录。上述业务写入操作只允许 Issue 所有者的
活跃普通用户执行；管理员账号是独立的运营管理角色，不会自动获得普通用户业务写入权限。
详细搜索会生成可过期清理的临时
结果文件，但仍属于游客可用的搜索流程。临时结果物化按 IP 每分钟最多 10 次；单个结果默认最多 64 MiB，
目录默认最多 1 GiB 或 1000 条记录，并发物化默认最多 2 个任务。Preview 结果默认保留 30 分钟，
完整结果默认保留 7 天；读取结果不会刷新过期时间。周期清理会原子认领过期记录为 `DELETING`，删除文件和数据库记录；
服务重启后会继续处理遗留的 `DELETING` 记录，并清理陈旧的 `.part`、`.ready-*` 和无数据库记录的孤儿结果文件。
物化中的结果先登记为 `STAGING` 并受活动 lease 保护，完成后才转为 `ACTIVE`。

活跃普通用户可以将文件名搜索或详细搜索保存为个人条件，选择全局或当前 Issue 范围，
之后从“我的搜索条件”重新使用或删除。条件只保存查询与稳定选项，不保存会过期的
临时结果 ID；所有查询、修改和删除均按当前用户隔离。游客点击“保存条件”时会先登录，
返回原页面后恢复条件并继续保存。

服务每小时删除过期或已撤销的 Session。可通过 `RAIN_ALLOW_REGISTRATION=false`
关闭注册入口对应的后端能力；此时注册 API 返回 `REGISTRATION_DISABLED`，已有账户
仍可正常登录。

登录接口按 IP 限制为每分钟 20 次尝试，并按用户名限制为每 5 分钟 10 次失败；成功
登录不累计用户名失败次数。注册接口按 IP 限制为每小时 10 次。认证同时限制 Argon2
并发，避免公开入口耗尽 CPU 或 Actix blocking pool。浏览器访问遵循同源策略，服务端
不发送跨域许可响应头。

## 当前支持

- Issue 列表、打开、删除。
- Issue 列表通过后端 cursor 分页读取 `ACTIVE` Issue，支持“我的 Issue”和“所有 Issue”范围以及编号/名称筛选，不会一次加载全部记录。
- 多文件上传。
- `.log`、`.txt` 等文本文件索引。
- `.zip`、`.tar.gz`、`.tgz`、`.gz`、`.7z` 后台递归解压并写入文件树，内层日志同样会建立索引和支持分页查看；`.7z` 首期不含加密正文、加密文件名、分卷和自解压 EXE。
- `.exe`、Office、图片等二进制文件保留在文件树中，显示类型与大小并支持登录后显式下载，但不会文字预览或建立搜索索引。
- 每个 Issue 默认最多包含 8 GiB 最终可浏览文件；普通文件按实际大小计算，压缩包只计算解压后的最终文件，失败或删除 Bundle 会释放容量。
- 压缩包仍有固定的条目数量、嵌套深度、路径、压缩比和路径穿越防护，这些安全细节不需要通过 `.env` 调整。
- 文件树浏览。
- 文本文件分页读取，后端按行偏移索引快速跳转。
- 单行默认超过 8 MiB 时索引和分页展示会截断该行，并标记 `[line truncated]`；该限制可配置。
- Issue 范围和 Bundle 范围采用 Tantivy trigram 子字符串搜索，支持标识符、错误码和连续中文的部分匹配；少于 3 个字符的关键词直接拒绝。结果返回最多 400 字符的命中附近摘要，默认 50 条、最多 100 条。
- 登录后的原始文件下载。
- 删除 Issue、Bundle、单个文件节点。
- Issue 自动清理按 owner 的非活跃状态执行；管理员可在“系统设置 → 自动清理白名单”中实时维护豁免用户。`RAIN_CLEANUP_EXEMPT_USERS` 仅在数据库尚未初始化白名单时作为一次性迁移来源，后续不会覆盖管理员配置。旧的 `RAIN_RETENTION_DAYS` 已废弃并会被忽略。

## 当前限制

- 暂不支持 `.rar` 解压；`.7z` 支持普通非加密归档、目录和 solid/non-solid 内容，但不支持加密、分卷（`.7z.001`）和自解压 EXE。
- 上传传输有前端进度；后台任务通过 `RECEIVING/EXTRACTING/INDEXING/PUBLISHING` 阶段提供处理状态，暂未提供阶段内百分比。
- 上传接收阶段按单次请求限制文件总数和字节数，并受并发接收数与 `.tmp` 工作区全局字节预算限制；预算覆盖原始接收文件、递归解压后的 staging 文件和解压过程中的中间输出。接收字节上限为 Issue 最终内容上限的 2 倍，最终可浏览内容仍受 `RAIN_ISSUE_MAX_CONTENT_SIZE` 限制。Multipart 中的每个文件字段都会计入文件数量，即使字段内容为空。
- 后台处理在 `.tmp/{task_id}/staging` 中完成解压和索引；真实文件同步写入内容寻址 BlobStore，完成或失败后 staging 工作区会被清理。
- 临时搜索结果受单结果大小、全局总容量、记录数、并发物化数和按 IP 的请求频率共同限制；Preview 结果固定保留 30 分钟，完整结果固定保留 7 天，读取不会滑动续期；达到上限时不会继续创建结果文件。
- 文件和临时结果行分页同时受近似字节预算、全局并发读取数和单客户端并发读取数限制，避免少数超大分页请求占满内存或 I/O；当单行的 JSON 编码结果仍超过分页预算时，服务端会返回带 `[response truncated]` 标记的有界前缀，并继续推进分页游标。
- 搜索关键词少于 3 个字符会被拒绝，以避免公开接口执行无界的全文扫描。
- SQLite 使用 WAL 和 30 秒 busy timeout；上传写库、Blob 维护和清理通过进程内共享写入队列按事务排队。Tantivy 索引在独立 Bundle writer 中构建，不占用 SQLite writer admission；后台解压/索引任务的有效并发由管理员的 `Manual/Auto` 设置和启动时 runtime adaptive engine 决定。
- Bundle 清理默认每批 100 行，每批提交后重新排队，避免一次清理长期占用写入队列。此队列不能协调其他进程；同一数据库应由一个 Rain 实例使用，并放在本地文件系统上。
- `.zip`、`.tar.gz`、`.tgz`、`.gz`、`.7z` 会在同一 staging bundle 内递归处理并共享安全限额；暂不支持后台任务超时/取消。
- 搜索使用 Tantivy trigram 索引；日志 chunk 正文由 Bundle Tantivy artifact 持有，SQLite 只保存行定位和生命周期元数据。
- 服务状态分为进程存活检查 `/healthz` 和依赖就绪检查 `/readyz`；页面顶部显示的是后者，检查 SQLite 和数据目录是否可用。`/readyz` 保留数据库写入后回滚的探测，结果缓存 5 秒，并发请求共享一次探测。
- 真实文件使用 SHA-256 内容寻址 Blob 存储，保存到数据根目录下的 `blobs/<hash前两位>/<完整hash>`；多个 Bundle 中的相同内容只保存一份。
- 文件字节访问统一经过 `BlobStore` 接口；当前使用 `LocalCasBlobStore`，上层业务不依赖本地物理路径。
- Bundle 使用逻辑删除；无引用 Blob 由后台 GC 基于数据库实际引用扫描，并在 24 小时宽限期后回收。

自动测试：

```bash
cd backend
cargo test routes::uploads::tests
```

Windows 手动验证时，可在启用 Defender 或目录索引的环境上传包含大量小文件的压缩包，并观察发生短暂锁定时任务最终进入 `READY`；持续占用 staging 目录超过重试窗口时，任务应进入 `FAILED`，且文件树接口不应返回半成品。

## 数据位置

默认数据都在仓库根目录下的 `data/`，该目录已被 `.gitignore` 忽略：

- SQLite 控制数据库：`data/rain.db`
- 上传、解压文件和 Tantivy 索引：`data/uploads/`
- 后端运行日志：`log/YYYY-MM-DD.backend.log`（按天轮转）

如果想清空本地数据，可以停止服务后删除 `data/`，或临时设置：

```dotenv
RESET_DB=true
```

v0.1.0 不会迁移旧版本的 SQLite 搜索数据。全新安装应使用新的 `DATABASE_URL` 和 `RAIN_DATA_ROOT`；如果启动时提示需要新的数据目录，请先备份旧数据，再创建新的目录并重新上传日志。显式设置 `RAIN_SEARCH_BACKEND=sqlite_fts` 只用于测试和旧数据诊断，不是 v0.1.0 的默认发布路径。

注意：`RESET_DB=true` 会删除当前应用 schema 和 migration metadata，再通过同一 migration chain 重建表，并清空配置的数据目录，仅适合本地调试或测试；不要在生产环境用它代替数据库升级。

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
- `GET /api/files/v1/{bundleId}/files/{fileId}/download`（需要登录；访客不可下载）
- `DELETE /api/files/v1/{bundleId}/files/{fileId}`

文件节点包含 `preview_kind`（`directory`、`text`、`binary` 或 `archive`），前端据此决定展开目录、显示文字查看器或显示二进制文件信息页。

### Search

- `GET /api/log/v2/{bundleId}/search?q=keyword`
- `GET /api/issues/{issueCode}/search?q=keyword`

搜索接口支持 `from`、`size` 分页，服务端限制 `from + size <= RAIN_API_MAX_SEARCH_WINDOW`；超出 offset 返回 `SEARCH_OFFSET_TOO_LARGE`，窗口溢出返回 `SEARCH_WINDOW_TOO_LARGE`（HTTP 400）。响应中的 `max_search_window` 是当前生效上限，Issue 内容搜索会在所有 Bundle 间做全局有界排序。

## 后续方向

短期优先级：

1. 在目标机器完成 1/2/4 并发和 1/5 GiB 大文件基线，确认吞吐、RSS 和查询 p95。
2. 解析任务细粒度进度、取消和失败重试。
3. 结构化事件查询 API，例如按 level、component、时间范围过滤。
4. 更完整的日志 parser 规则和多行异常合并。

数据库细节见 [doc/DB.md](doc/DB.md)。
# 管理员初始化与权限

首次使用空数据库启动前必须设置管理员密码：

```env
RAIN_BOOTSTRAP_ADMIN_USERNAME=admin
RAIN_BOOTSTRAP_ADMIN_PASSWORD=<至少 8 个字符的强密码>
```

启动会在 Schema migration 完成后原子创建唯一的 `ACTIVE + ADMIN` 运营账户和审计记录。后续启动只验证数据库中恰好存在一个有效管理员，`.env` 不会覆盖密码或创建第二个管理员；管理员不能被提升、降级、停用、转让或强制注销。

权限按角色划分如下：

| 角色 | 读取/搜索 | 下载或查看临时结果 | 创建 Issue、上传、删除自己的资源 | 用户、Session、审计、系统设置 |
| --- | --- | --- | --- | --- |
| 游客 | 可以 | 不可以 | 不可以 | 不可以 |
| 活跃普通用户（`USER`） | 可以 | 可以 | 可以，但仅限自己拥有的 Issue | 不可以 |
| 活跃管理员（`ADMIN`） | 可以 | 可以 | 不可以使用普通用户业务写入路由 | 可以，在 `/admin` 中管理 |

已有数据库必须通过当前 baseline compatibility validation；不兼容的旧 schema 会 fail fast，不会自动删除数据。
