# 上传完成阶段常数时间优化设计

## 目标

上传处理完成后，不再按 Bundle 文件数量解析和重写 `files.meta`。Bundle 目录整体移动成功后，仅更新 Bundle 的终态，使完成阶段从 O(文件数) 降为 O(1)。

## 现状与根因

处理期间，文件记录的 `meta.storage_path` 保存 staging 目录下的绝对路径。Bundle 目录从 staging 整体 rename 到正式数据目录后，finalizer 查询 Bundle 下全部文件，逐条解析 JSON、替换绝对路径并执行 `UPDATE files`，最后才更新 Bundle 状态。

`files.path` 已保存以 Bundle hash 开头的稳定逻辑路径。staging 与正式 Bundle 的内部目录结构不变，因此最终磁盘位置可由正式 `data_root + files.path` 唯一解析，无需在 metadata 中重复保存一个需要改写的绝对路径。

## 写入模型

新创建的文件 metadata 不再包含 `storage_path`。metadata 继续保存展示和分类所需字段，例如：

```json
{
  "kind": "extracted_file",
  "preview_kind": "text"
}
```

上传处理期间需要访问文件时，处理函数继续使用当前调用链中的 staging `PathBuf`，不通过数据库重新解析路径。

## 路径解析与兼容性

正式文件的默认磁盘路径为：

```text
data_root.join(files.path.trim_start_matches('/'))
```

已有数据库记录可能仍包含绝对 `meta.storage_path`。读取和删除继续优先接受该字段，以兼容历史数据；字段不存在时使用 `files.path` fallback。候选路径仍必须经过现有 canonicalization 和 data-root 边界检查。

单文件或目录删除需要显式接收 `data_root`，为没有 `storage_path` 的新记录计算磁盘路径，避免只删除数据库记录而遗留文件。

## Finalizer

目录移动仍在 finalizer 之前完成。目录移动成功后，finalizer 不再接收 staging/final 路径，不查询 `files`，只在一个数据库操作中执行：

```sql
UPDATE bundles
SET status = 'READY',
    process_stage = 'READY',
    failure_reason = NULL
WHERE id = ?;
```

现有重试语义保留。状态更新失败时重试常数时间操作，不再重复扫描全部文件。

## 错误与安全

- Bundle 目录 rename 失败时不进入 READY finalizer。
- Bundle 状态更新失败时沿用现有重试和失败清理流程。
- 文件读取和删除解析出的路径必须位于 `data_root` 内。
- 旧绝对路径记录保持可读、可删除，不要求数据迁移。

## 验证

- 单元测试验证 finalizer 不读取或更新 `files`，即使其中包含无效 JSON 或大量记录也能只更新 Bundle。
- ingest 测试验证新上传、解压目录、解压文件和嵌套解压目录 metadata 均不写 `storage_path`。
- repository 测试验证新记录通过 `files.path` 解析，旧记录仍优先使用兼容字段。
- 删除集成测试验证没有 `storage_path` 的文件和目录会从磁盘及数据库同时删除。
- 上传 smoke 测试继续验证 Bundle 最终进入 `READY`，文件预览、搜索和删除行为不回归。
- 最终运行后端完整测试；本次不改变前端接口，前端构建作为集成验证继续执行。
