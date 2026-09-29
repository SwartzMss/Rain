# Issue #259 源文件树定位设计

## 目标

修复搜索结果“在原文件中打开”时，目标文件已经在右侧打开但左侧文件树偶发无法显示、选中或滚动定位的问题。修复必须覆盖 root 深分页、多级目录和归档 flatten，同时不破坏现有分页 cursor 语义与右侧原始行号定位。

## 根因

`revealSourceNode()` 可以通过文件 ID 加载目标节点并回溯真实 `parent_id` 链，但单独加载的顶层节点不会自动挂回 synthetic `bundle:root.childrenIds`。文件树渲染从 root 的 `childrenIds` 开始，因此目标节点虽然存在于 `treeNodes` 并且 `selectedNodeId` 已设置，仍可能没有对应 DOM 节点，最终滚动 effect 无法定位。

## 方案

在前端 reveal 流程中显式 materialize 可渲染路径：

1. 加载目标节点和真实祖先节点。
2. 每次加载节点后，确保父节点的 `childrenIds` 包含该节点。
3. 当回溯到真实顶层节点时，显式把它加入 synthetic root 的 `childrenIds`。
4. 只展开可见祖先；归档 `_extracted` 节点继续使用现有 flatten 逻辑，不把内部容器显示出来。
5. 不把 root 或目录的 `hasMoreChildren` / `childrenCursor` 错误标记为已完成，也不为了 reveal 读取所有同级节点。
6. 保持现有 `selectedNodeId`、viewer tab 和行号定位流程不变；树更新后由现有 DOM effect 负责滚动。

## 错误处理

如果目标节点或祖先接口请求失败，沿用现有错误消息与右侧打开失败行为；不伪造不完整的树路径。source identity 缺失时继续显示“来源文件信息不可用”。

## 测试范围

- root 首屏目标：节点保持可渲染并选中。
- root 第二页目标：无需手动加载更多即可挂回 root 并渲染。
- 多级目录：完整祖先链挂载并展开。
- 归档 flatten：不显示 `_extracted` 容器，内部文件仍可 reveal。
- 原始行号：source reveal 后仍使用正确的文件行号页。
- 现有文件树分页、归档加载失败重试和全部前端测试继续通过。
