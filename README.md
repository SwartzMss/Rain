# Rain

Rain 是一个本地日志包浏览与检索工具。当前版本用于把文本日志或 `.zip`、`.tar.gz`、`.tgz`、`.gz` 压缩包上传到一个 Issue 下，浏览递归解压后的文件树，分页查看文本内容，并按关键词搜索日志。

默认使用 SQLite，本地启动不需要安装 PostgreSQL 或其他数据库服务。

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
SERVER_PORT=8080
RESET_DB=false
RAIN_SESSION_TTL_SECONDS=604800
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
# 检查 SQLite 与数据目录是否已就绪
curl -i http://localhost:8080/readyz
```

### 开发前端

```bash
cd frontend
npm install
npm run dev
```

开发时也可以继续使用 Vite dev server：`http://localhost:5173`。

Vite 会把浏览器的同源 `/api` 请求代理到默认的 `http://localhost:8080`。如果后端
开发端口不同，可在 `frontend/.env` 中设置仅供开发服务器使用的代理目标：

```dotenv
RAIN_DEV_API_PROXY_TARGET=http://localhost:8080
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

### 可配置限制

Issue 容量、后台处理并发、索引单行上限、预览单行上限和 API 限制使用同一个 `.env` 文件；不需要额外的 TOML 配置。程序优先读取可执行文件同目录的 `.env`，找不到时读取当前工作目录的 `.env`，而系统环境变量始终优先。未设置的项目使用下表默认值。压缩条目数、递归深度、路径和压缩比等安全防护采用程序内安全值，不需要部署者逐项配置。

字节大小可写成纯字节数或二进制单位 `KiB`、`MiB`、`GiB`、`TiB`，单位不区分大小写，例如 `64 KiB`、`4 GiB`。所有大小和数量必须大于零。启动时还会验证 API 默认页大小不大于对应最大值；错误配置会阻止启动并指出变量名称。

| 环境变量 | 默认值 | 用途 |
| --- | ---: | --- |
| `RAIN_ISSUE_MAX_CONTENT_SIZE` | `4 GiB` | 每个 Issue 最终可浏览文件总量；压缩包按解压后内容计算 |
| `RAIN_UPLOAD_CONCURRENT_PROCESSING_TASKS` | `4` | 并发后台处理任务 |
| `RAIN_UPLOAD_CONCURRENT_RECEIVE_TASKS` | `4` | 并发 Multipart 接收任务 |
| `RAIN_UPLOAD_MAX_TMP_BYTES` | `16 GiB` | 所有上传任务 `.tmp` 工作区的全局字节预算，包含原始接收文件和解压后的 staging 文件 |
| `RAIN_INDEXING_MAX_INDEXED_LINE_SIZE` | `256 KiB` | 单行进入搜索索引的最大前缀大小 |
| `RAIN_API_FILE_PREVIEW_SIZE` | `64 KiB` | 文件文本预览大小 |
| `RAIN_API_MAX_PREVIEW_LINE_SIZE` | `8 MiB` | 文件分页接口单行返回的最大前缀大小 |
| `RAIN_API_DEFAULT_LINE_PAGE_SIZE` | `5000` | 默认行分页大小 |
| `RAIN_API_MAX_LINE_PAGE_SIZE` | `10000` | 最大行分页大小 |
| `RAIN_API_DEFAULT_SEARCH_RESULTS` | `50` | 默认搜索结果数 |
| `RAIN_API_MAX_SEARCH_RESULTS` | `100` | 最大搜索结果数 |
| `RAIN_TEMP_RESULT_MAX_SIZE` | `64 MiB` | 单个临时搜索结果的 `.log/.meta/.idx` 总大小上限 |
| `RAIN_TEMP_RESULT_MAX_TOTAL_SIZE` | `1 GiB` | 临时结果目录的数据库登记总容量上限 |
| `RAIN_TEMP_RESULT_MAX_RECORDS` | `1000` | 临时结果最多保留的记录数 |
| `RAIN_TEMP_RESULT_CONCURRENT_MATERIALIZATIONS` | `2` | 并发物化临时结果的任务数 |
| `RAIN_SESSION_TTL_SECONDS` | `604800` | 登录 Session 有效期（秒），默认 7 天 |
| `RAIN_ALLOW_REGISTRATION` | `true` | 是否开放新用户注册；关闭后已有用户仍可登录 |
| `RAIN_AUTH_ARGON2_CONCURRENCY` | `5` | Argon2 哈希与校验并发上限 |
| `RAIN_AUTH_LOGIN_IP_LIMIT_PER_MINUTE` | `20` | 同一 IP 每分钟登录尝试上限 |
| `RAIN_AUTH_LOGIN_USERNAME_FAILURE_LIMIT_PER_5_MINUTES` | `10` | 同一用户名每 5 分钟失败登录上限 |
| `RAIN_ISSUE_INACTIVE_DAYS` | `0` | Issue 非活跃自动过期天数；0 关闭，启用范围 7–30 |
| `RAIN_AUTH_REGISTER_IP_LIMIT_PER_HOUR` | `10` | 同一 IP 每小时注册尝试上限 |
| `RAIN_AI_BASE_URL` | — | OpenAI-compatible API 的 `/v1` Base URL；作为数据库配置的兜底 |
| `RAIN_AI_API_KEY` | — | 环境变量 Provider 的 API Key |
| `RAIN_AI_MODEL` | — | 环境变量 Provider 使用的模型 |
| `RAIN_AI_TIMEOUT_SECONDS` | `120` | 单次模型请求超时，范围 1–300 秒 |
| `RAIN_AI_STRUCTURED_OUTPUT` | `json_object` | Skill final result 和 repair 请求的结构化输出模式；可选 `json_object` 或 `json_schema`。管理员 Provider Test 始终使用 `response_format: null`，只验证连通性，不验证 strict schema 能力 |
| `RAIN_AI_MASTER_KEY` | — | 32 字节随机主密钥的 Base64，用于加密管理员保存到数据库的 API Key |

默认配置会使用：

- SQLite 数据库：`./data/rain.db`
- 上传目录：`./data/uploads`
- 后端端口：`8080`

启动后访问 `http://localhost:8080`。首次运行后会在工作目录附近生成 `data/` 和 `log/`，这是 SQLite、上传文件和运行日志的正常运行时数据。

## 使用流程

1. 打开 `http://localhost:8080`。
2. 通过右上角“注册”创建账户；注册成功后使用用户名和密码登录。
3. 新建或选择一个 Issue，例如 `CN013`。
4. 在选中的 Issue 下拖拽或点击上传 `.log`、`.txt`、`.zip` 文件。
5. 点击 Issue 的“查看”打开文件浏览页。
6. 在左侧文件树选择文件，右侧会显示文本预览。
7. 在搜索框输入关键词，可搜索当前 Issue 下已索引的文本日志。

### 私有 Skills 与 Issue 诊断

登录用户可以在“账户 → 我的 Skills”创建自己的 `SKILL.md`、启停 Skill，并请求一次质量评估。所有 Skill 都必须符合 [`SKILL.md` v1](doc/SKILL_SCHEMA.md)；服务端在保存、详情读取、质量评估和运行时确定性校验 front matter、五个必填章节和 64 KiB 上限，格式合法性不交给 AI 判断。当前内测阶段不保留旧格式兼容层，格式发生不兼容变化时应重建开发数据。Skills 始终按用户隔离，系统不提供内置 Skills；每个用户最多 50 个，列表只返回摘要，完整 Markdown 按所选 Skill 单独读取。质量评估只保留当前 Skill 版本和当前评分规则的一份结果，重新评估会覆盖，正文修改会清除它。评分等级由服务端按总分确定，评估限制为每用户同时 1 个、每小时 5 次、全局同时 2 个模型任务，整体 90 秒超时。

管理员可以在“系统设置 → AI Provider”保存 OpenAI-compatible 服务配置并测试连接。空测试请求只测试当前生效配置；测试未保存配置必须完整提供 Base URL、API Key、模型和超时，修改 Base URL 时也必须重新输入 API Key，避免把已有密钥发送到新地址。完整且可解密的数据库配置优先于环境变量。数据库中的 API Key 使用 `RAIN_AI_MASTER_KEY` 进行 AES-256-GCM 加密；接口、审计日志和错误信息都不会返回明文或密文。未配置主密钥时仍可使用完整的环境变量 Provider，但不能把新的 API Key 保存进数据库。

任何能查看当前 ACTIVE Issue 的登录用户都可选择自己的已启用 Skill 运行诊断，不要求是 Issue 创建者。Runner 只有 `get_issue_manifest`、`list_files`、`search_logs`、`read_file_lines` 四个只读工具，且服务端固定绑定当前 Issue；文件列表提供游标续页和路径前缀过滤。Skill 的 front matter 不会作为诊断指令注入，Skill 内容也不能扩大 Issue scope 或工具权限。固定限制为 8 次迭代、24 次工具调用、每次最多 20 个搜索结果、30 个证据区间、单次工具输出 32 KiB、累计证据 128 KiB、总时长 120 秒，同一用户同时只能运行一个任务。Issue Manifest、日志和文件名始终按不可信检索上下文或证据处理。

诊断运行可选定“不限制时间”、事故时间前后窗口或直接时间范围；时间范围使用日志中的 wall-clock 文本，最多 24 小时，并保存为该 Run 的不可变快照。API 接受空格或 `T` 分隔的本地日期时间，可带小数秒，也接受 `datetime-local` 的分钟精度；不会自动转换为 UTC。`search_logs` 默认只检索与主窗口相交且具有事件时间索引的日志 chunk，模型最多请求 15 分钟的边界上下文扩展。`time_scope` 不传或为 `null` 时保持旧的全量搜索行为；只有 `HH:mm:ss` 而没有日期的日志不会臆测日期，带范围搜索会通过 coverage 信息报告被排除的未索引匹配。

页面刷新后可在同一浏览器会话中续接当前任务。成功、失败或取消后的结果仅按 run ID 临时保留 24 小时，随后连同步骤和 Skill 快照清理；Rain 不提供诊断历史列表。受支持的摘要、每项观察和推断都必须引用经过服务端核验的证据 ID；无证据摘要会被服务端替换为固定的“证据不足”文本。证据携带 Bundle hash、文件和行号，可直接跳回唯一来源。文件发现结果明确标记目录，误读目录会返回非致命工具错误。

## 用户认证

Rain 支持用户名和密码注册、登录、查询当前身份、修改密码、当前设备退出和全部设备退出。用户名长度为
3～32，只允许字母、数字、`.`、`_`、`-`，且不区分大小写；密码长度为
8～128 个字符。密码使用 Argon2id 保存，登录 Session 使用 HttpOnly Cookie，
数据库只保存 Session Token 的 SHA-256 哈希。

注册成功后不会自动登录。当前版本没有邮箱、手机号或自助找回密码功能；忘记密码
时只能由部署管理员直接维护数据库。当前版本面向可信内网 HTTP 部署，必须通过 Rain
后端提供的页面同源访问 API，不支持独立部署在其他来源的浏览器前端。

游客可以查看和搜索，但不能下载文件或临时搜索结果；创建 Issue、上传、删除 Issue、删除
Bundle、删除文件节点以及删除临时搜索结果需要登录。详细搜索会生成可过期清理的临时
结果文件，但仍属于游客可用的搜索流程。临时结果物化按 IP 每分钟最多 10 次；单个结果默认最多 64 MiB，
目录默认最多 1 GiB 或 1000 条记录，并发物化默认最多 2 个任务。结果默认保留 7 天，
访问结果会刷新过期时间。周期清理会原子认领过期记录为 `DELETING`，删除文件和数据库记录；
服务重启后会继续处理遗留的 `DELETING` 记录，并清理陈旧的 `.part`、`.ready-*` 和无数据库记录的孤儿结果文件。
物化中的结果先登记为 `STAGING` 并受活动 lease 保护，完成后才转为 `ACTIVE`。

登录用户可以将文件名搜索或详细搜索保存为个人条件，选择全局或当前 Issue 范围，
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
- Issue 列表会加载全部 ACTIVE Issue，不再因超过 200 条而无法从界面发现。
- 多文件上传。
- `.log`、`.txt` 等文本文件索引。
- `.zip`、`.tar.gz`、`.tgz`、`.gz` 后台递归解压并写入文件树，内层日志同样会建立索引和支持分页查看。
- `.exe`、Office、图片等二进制文件保留在文件树中，显示类型与大小并支持登录后显式下载，但不会文字预览或建立搜索索引。
- 每个 Issue 默认最多包含 4 GiB 最终可浏览文件；普通文件按实际大小计算，压缩包只计算解压后的最终文件，失败或删除 Bundle 会释放容量。
- 压缩包仍有固定的条目数量、嵌套深度、路径、压缩比和路径穿越防护，这些安全细节不需要通过 `.env` 调整。
- 文件树浏览。
- 文本文件分页读取，后端按行偏移索引快速跳转。
- 单行默认超过 8 MiB 时索引和分页展示会截断该行，并标记 `[line truncated]`；该限制可配置。
- Issue 范围和 bundle 范围采用 SQLite FTS5 trigram 子字符串搜索，支持标识符、错误码和连续中文的部分匹配；少于 3 个字符的关键词直接拒绝。结果返回最多 400 字符的命中附近摘要，默认 50 条、最多 100 条。
- 登录后的原始文件下载。
- 用户私有 Skill 管理、当前版本质量评估，以及 Issue 范围的受限 AI 诊断。
- 删除 Issue、Bundle、单个文件节点。
- 可选过期清理：设置 `RAIN_RETENTION_DAYS` 后启动时清理过期上传。

## 当前限制

- 暂不支持 `.rar`、`.7z` 解压。
- 上传传输有前端进度；后台任务通过 `RECEIVING/EXTRACTING/INDEXING/PUBLISHING` 阶段提供处理状态，暂未提供阶段内百分比。
- 上传接收阶段按单次请求限制文件总数和字节数，并受并发接收数与 `.tmp` 工作区全局字节预算限制；预算覆盖原始接收文件、递归解压后的 staging 文件和解压过程中的中间输出。接收字节上限为 Issue 最终内容上限的 2 倍，最终可浏览内容仍受 `RAIN_ISSUE_MAX_CONTENT_SIZE` 限制。Multipart 中的每个文件字段都会计入文件数量，即使字段内容为空。
- 后台处理在 `.tmp/{task_id}/staging` 中完成解压和索引；真实文件同步写入内容寻址 BlobStore，完成或失败后 staging 工作区会被清理。
- 临时搜索结果受单结果大小、全局总容量、记录数、并发物化数和按 IP 的请求频率共同限制；达到上限时不会继续创建结果文件。
- 搜索关键词少于 3 个字符会被拒绝，以避免公开接口执行无界的全文扫描。
- SQLite 使用 WAL 和 30 秒 busy timeout；日志索引每 5000 行批量提交一次，后台解压/索引任务默认最多 4 个并发，可通过 `RAIN_UPLOAD_CONCURRENT_PROCESSING_TASKS` 调整。
- `.zip`、`.tar.gz`、`.tgz`、`.gz` 会在同一 staging bundle 内递归处理并共享安全限额；暂不支持后台任务超时/取消。
- 搜索使用 SQLite FTS5 trigram external-content 索引；日志 chunk 正文仅存于 `log_segments.content`，FTS 不保存正文副本。
- 服务状态分为进程存活检查 `/healthz` 和依赖就绪检查 `/readyz`；页面顶部显示的是后者，检查 SQLite 和数据目录是否可用。
- 真实文件使用 SHA-256 内容寻址 Blob 存储，保存到数据根目录下的 `blobs/<hash前两位>/<完整hash>`；多个 Bundle 中的相同内容只保存一份。
- 文件字节访问统一经过 `BlobStore` 接口；当前使用 `LocalCasBlobStore`，上层业务不依赖本地物理路径。
- Bundle 使用逻辑删除；无引用 Blob 由后台 GC 基于数据库实际引用扫描，并在 24 小时宽限期后回收。
- `timeline` 目前仍固定为 `all`，仅作为旧的展示标签；日志 chunk 另外保存从普通日期时间、`[日期时间]` 或 `[E][日期时间][...]` 行首格式提取的 wall-clock 起止比较键，供带 `time_scope` 的 Skill Run 搜索过滤。数据库仍使用兼容性的 `*_ms` 列名，但这些整数值不是 Unix epoch、UTC 或绝对时间，只能与同一 wall-clock 编码比较。
- 当前 AI 分析只支持 OpenAI-compatible Chat Completions，不提供通用聊天、Shell、网络工具、用户脚本、MCP 或 Issue 写操作。

自动测试：

```bash
cd backend
cargo test routes::uploads::tests
```

Windows 手动验证时，可在启用 Defender 或目录索引的环境上传包含大量小文件的压缩包，并观察发生短暂锁定时任务最终进入 `READY`；持续占用 staging 目录超过重试窗口时，任务应进入 `FAILED`，且文件树接口不应返回半成品。

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
- `GET /api/files/v1/{bundleId}/files/{fileId}/download`（需要登录；访客不可下载）
- `DELETE /api/files/v1/{bundleId}/files/{fileId}`

文件节点包含 `preview_kind`（`directory`、`text`、`binary` 或 `archive`），前端据此决定展开目录、显示文字查看器或显示二进制文件信息页。

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
# 管理员初始化与权限

首次使用空数据库启动前必须设置管理员密码：

```env
RAIN_BOOTSTRAP_ADMIN_USERNAME=admin
RAIN_BOOTSTRAP_ADMIN_PASSWORD=<至少 8 个字符的强密码>
```

启动会在 Schema 准备完成后原子创建唯一的 `ACTIVE + ADMIN` 运营账户和审计记录。后续启动只验证数据库中恰好存在一个有效管理员，`.env` 不会覆盖密码或创建第二个管理员；管理员不能被提升、降级、停用、转让或强制注销。普通用户和游客可读取共享数据，只有管理员能新建 Issue、上传或删除共享数据；管理员可在 `/admin` 管理普通用户状态、Session 和审计日志。本版本按全新安装部署，不兼容旧数据库 Schema。
