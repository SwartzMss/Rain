# 数据库迁移框架设计

## 目标

为 Rain 建立 SQLx migration 机制，把当前完整 SQLite schema 固化为首个 `0001_initial` baseline，并让已经存在的 legacy 数据库在不丢数据的前提下安全纳入 migration 管理。

完成后，启动流程只通过 migration 推进 schema；未知或不兼容的数据库在 recovery、后台任务和 HTTP 服务启动前明确失败。

## 当前问题

当前 `backend/src/db.rs` 中的 `create_schema()` 同时承担了三种职责：

1. 新数据库建表、建索引、创建 FTS5 表和触发器。
2. 通过 `CREATE TABLE IF NOT EXISTS` 维持已有数据库。
3. 通过 `ensure_*_optional_columns()`、索引创建和事件时间回填隐式升级旧数据库。

这种方式没有数据库版本、checksum 或 dirty 状态，无法区分“完整 legacy schema”和“部分升级 schema”，也不能清楚地审计某个 binary 对数据库做过哪些变更。

## 方案

### 1. SQLx migration 作为唯一正式 schema 链

启用 SQLx 的 `migrate` feature，并在 `backend/migrations/` 中放置：

```text
0001_initial.sql
```

`0001_initial.sql` 是当前 main 的完整 schema 快照，包含：

- 所有业务表、外键、CHECK、唯一约束和默认值；
- 当前全部普通索引和部分唯一索引；
- `log_segments_fts` FTS5 external-content 虚表；
- FTS insert/delete/update 触发器；
- readiness probe 表；
- `skill_runs.analysis_*` 和 `log_segments.event_time_*` 等已经由旧 ensure 逻辑补齐的字段；
- 当前事件时间索引。

baseline migration 不依赖运行时 `PRAGMA table_info`，也不执行仅针对历史数据的隐式修复。

Rust 侧使用编译期 `sqlx::migrate!()` 得到静态 Migrator。未来 schema 变化只增加编号递增的 migration，不能再向 `create_schema()` 或新的 `ensure_*()` 添加长期升级逻辑。

### 2. Legacy baseline adoption

启动时先检查 migration metadata 和用户 schema，分三种情况处理：

#### 空数据库

如果没有业务 schema 对象，则直接运行 SQLx Migrator。SQLx 创建 `_sqlx_migrations` 并执行 `0001_initial`。

#### 已有 legacy 数据库但没有 migration metadata

先执行严格的 baseline compatibility validation，再运行 baseline adoption。验证至少覆盖：

- 必需表及其必需字段；
- 字段类型、NOT NULL、默认值和关键 CHECK/外键关系；
- 必需普通索引、唯一索引和 partial index；
- FTS5 表的 external-content、content rowid 和 trigram 配置；
- 三个 FTS 同步触发器；
- 当前 baseline 要求的 readiness 和状态机字段。

验证允许未知的额外对象，以兼容未来或人工增加的非破坏性对象，但任何必需对象缺失或关键定义不匹配都返回包含对象名称和原因的 migration error。

验证通过后，在同一个受控启动阶段执行 baseline adoption，使 SQLx 以 `0001` 的真实 checksum 记录 `_sqlx_migrations`。adoption 不删除表、不重建数据、不覆盖业务记录。之后由同一个 Migrator 继续处理未来 migration。

#### 已有 migration metadata

不再执行 legacy ensure。直接使用 SQLx Migrator；SQLx 负责检查已应用 migration 的 checksum、dirty 状态和版本顺序。dirty、checksum mismatch 或缺少 migration 文件时 fail fast。

### 3. 启动和 reset

保留 `prepare_schema(pool, reset)` 作为应用层入口，但内部职责改为：

```text
init pool
  -> optional reset
  -> detect empty / legacy / managed database
  -> validate or adopt legacy baseline
  -> run SQLx migrations
  -> return
load settings
run recovery
start HTTP server
```

`reset=true` 时删除业务 schema 和 `_sqlx_migrations` metadata，然后通过同一 Migrator 从 `0001` 重建；不再调用独立的 `create_schema()` 建库路径。reset 只用于显式开发/测试配置，生产配置仍不会自动 destructive reset。

事件时间历史回填如果仍需要处理已有数据，应成为有明确版本边界的 migration-adjacent stage，并且必须可恢复、可观测；它不能再由每次 `prepare_schema()` 无版本地扫描执行。

### 4. 测试和文档

测试环境继续通过 `prepare_schema()` 初始化，但验证结果改为真实 migration 行为。新增覆盖：

- 空数据库执行 `0001_initial` 并验证完整对象集合；
- legacy schema 带业务数据时 adoption 成功且数据保持；
- 缺字段、错误索引或错误 FTS 定义的 legacy schema 被拒绝；
- 二次启动不重复执行 baseline；
- checksum mismatch 和 dirty migration 被拒绝；
- 后续测试 migration 能在已有 baseline 上按顺序执行；
- migration 失败时 recovery 不会运行，启动不会继续到 HTTP 服务。

更新 `doc/DB.md` 和 README，说明：

- binary 启动会自动执行未应用 migration；
- 当前已有数据库会先进行 baseline compatibility validation；
- 不兼容数据库会 fail fast，不会自动删除或猜测修复；
- `RESET_DB=true` 是开发/测试用途，不是生产升级方案。

## 错误处理和安全边界

- migration 和 baseline adoption 失败必须返回启动错误并终止进程；
- 不允许在 validator 失败后调用 `CREATE TABLE IF NOT EXISTS` 补洞；
- 不允许删除或重建 legacy 业务表来“修复”结构；
- 迁移写入使用 SQLite migration lock/事务，避免同一数据库被多个 Rain 进程同时升级；
- 日志记录数据库路径、检测分支、migration version 和失败对象，但不记录业务数据内容、密码或 API key；
- 仍只支持当前项目的 SQLite 单实例部署边界，不引入 PostgreSQL 或在线 destructive migration。

## 受影响的代码边界

- `backend/Cargo.toml`：启用 SQLx migration feature。
- `backend/migrations/0001_initial.sql`：当前完整 baseline。
- `backend/src/db.rs`：保留连接、reset、启动入口和受控 legacy validator；移除长期 ensure schema 路径。
- `backend/src/main.rs`：继续在 recovery 前调用 migration-aware `prepare_schema`，必要时调整错误日志。
- `backend/src/db/migrations.rs`（如实现需要拆分）：封装 metadata 检查、空库检测、legacy validation 和 adoption。
- `backend/src/db.rs` 测试或 `backend/tests/`：migration、adoption、失败和重启测试。
- `doc/DB.md`、`README.md`：升级和失败处理说明。

## 非目标

- 不切换数据库引擎。
- 不兼容任意历史开发快照或人工删改后的未知 schema。
- 不把业务表语义重构、Blob 状态机或 file-tree 删除引入本 PR。
- 不引入完整 online migration 或跨实例协调系统。

## 验收标准映射

| Issue #56 要求 | 设计落点 |
| --- | --- |
| SQLx migration framework | SQLx `migrate` feature + compile-time Migrator |
| 空库 baseline | `0001_initial.sql` |
| legacy 安全纳入 | 先 validator，再 baseline adoption |
| 未知 schema fail fast | validator 错误、SQLx checksum/dirty 错误 |
| 后续 migration 顺序执行 | `_sqlx_migrations` + numbered files |
| recovery 前完成 | `prepare_schema` 在 main recovery 前完成 |
| reset 共用迁移链 | reset metadata 后调用同一 Migrator |
| 测试统一 migration | 所有 schema setup 通过 migration-aware 入口 |
| 移除长期 ensure | ensure 逻辑迁入 baseline 或受控一次性 adoption |
| 文档 | README 与 `doc/DB.md` |
