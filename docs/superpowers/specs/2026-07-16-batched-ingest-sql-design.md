# 索引阶段 SQL 批量写入设计

## 目标

降低压缩包目录扫描与文本索引阶段的 SQLite statement 和事务提交次数，同时保持现有文件树、配额、搜索结果、行号和失败清理语义。

本次包含：

- 同一目录直接子项的文件记录在一个短事务内写入。
- 同一索引提交窗口内的 log segment 批量写入。
- 对应的 FTS 行批量写入。
- line offset 分批写入。

当前版本没有 structured event 存储，本次不新增或恢复该功能。

## 目录扫描与文件记录

每个目录按以下阶段处理：

1. 读取并稳定排序全部直接子项。
2. 在事务外读取 metadata、识别 MIME/preview kind，并完成需要的 Issue 配额预留。
3. 开启目录级短事务。
4. 插入该目录的全部直接子项并取得各自行 ID。
5. 提交事务。
6. 根据已取得的 ID 分别索引文本、递归处理子目录或解压嵌套 archive。

事务中不得执行文件读取、内容分类、配额事务、文本索引、递归解压或子目录遍历，避免长时间持有 SQLite 写锁。

首层上传文件仍由上传列表逐个处理，不跨上传文件建立长事务。archive 解压生成的每个目录独立使用上述短事务。

文件 INSERT 可以继续逐条执行，但必须复用同一个目录事务；主要收益来自消除每条记录的独立 autocommit。接口同时改为接受通用 sqlx executor，使首层写入和目录事务共用插入逻辑。

## Segment 与 FTS 批量写入

日志仍每 200 个有效行形成一个 `LogChunk`，每 5000 个源行作为一个提交窗口。窗口内不立即逐 chunk 写数据库，而是将最多约 25 个 chunk 保存在内存中。

提交窗口时：

1. 使用 `sqlx::QueryBuilder` 批量插入 `log_segments`。
2. 通过 `RETURNING id, chunk_index` 取得 segment ID，并以 `chunk_index` 映射回内容。
3. 使用第二个 `QueryBuilder` 批量写入 `log_segments_fts`。
4. 提交当前事务并开启下一窗口。

SQLite 不保证 `RETURNING` 行顺序，因此不得按返回位置配对，必须使用同一文件内唯一的 `chunk_index` 映射。

一个 segment 有 7 个 bind 参数。批量函数按照保守的最大 bind 数分片，避免超过 SQLite 构建参数上限；默认每批最多 100 个 segment，远高于正常 25 个窗口但便于独立测试和复用。

同一窗口的 segment 和 FTS 必须位于同一事务。任一批失败时回滚该窗口，不允许留下没有 FTS 的 segment。

## Line Offset 批量写入

仍每 1000 行记录一个 byte offset。文件读取完成后先删除该文件已有 offset，再用 `QueryBuilder` 每批最多 500 条写入：

```sql
INSERT INTO log_line_offsets (file_id, line_number, byte_offset)
VALUES (?, ?, ?), ...
```

offset 批量写入、最终 `files.line_count` 更新和最后一个 segment 窗口处于同一文件事务中。

## 内存与事务边界

- 文本内容内存上限仍由 5000 行提交窗口约束。
- 单个 chunk 仍限制为 200 行，不改变搜索 segment 粒度。
- 每 5000 行提交一次，不把整个大日志放入单一事务。
- 每个目录一个短文件记录事务，不把整个压缩包放入单一事务。
- 批量大小均为常量，并根据 bind 数留出充足余量。

## 错误与恢复

- 目录记录事务失败时，该目录不产生部分子项记录。
- segment 或 FTS 批次失败时，当前 5000 行窗口整体回滚。
- 后续上传失败处理继续删除 Bundle 数据库和文件系统产物。
- 文件和目录处理顺序保持稳定，现有文件树 ID 不要求与旧实现完全一致。

## 验证

- 单元测试验证同一目录的直接子项共享一个事务，失败会回滚全部子项。
- 单元测试验证分类与配额步骤发生在目录事务之外。
- 单元测试验证 25 个以上 chunk 批量写入并按 `chunk_index` 正确建立 FTS 对应关系。
- 单元测试验证 segment 批次中故意制造 FTS 失败会回滚 segment。
- 单元测试验证超过 500 条 offset 会分批完整写入，行号和 byte offset 不丢失。
- 现有 smoke 测试验证上传、解压、文件树、搜索、预览和删除不回归。
- 最终执行后端完整测试、Rust 格式检查、前端测试和生产构建。
