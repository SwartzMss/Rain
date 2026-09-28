# Issue #226：上传任务 Bundle handoff 设计

## 背景

上传请求成功后，前端 upload queue 会把任务置为 `ACCEPTED`，并用本地 optimistic row 表示“已接收，等待处理”。服务端 Bundle 首次出现在 Issue 的 Bundle snapshot 后，本地 row 应交由服务端 Bundle row 接管。

当前实现只根据服务端当前 snapshot 是否包含相同 `bundle_hash` 来隐藏 `ACCEPTED` task。当 Bundle 被删除、Issue 被删除，或服务端 cleanup/recovery 使 Bundle 从 snapshot 消失时，原来的 `ACCEPTED` task 又会满足显示条件，产生幽灵行。

## 目标

- 在本地 upload task 与服务端 Bundle 之间记录一次性、单向的 ownership handoff。
- 服务端第一次观察到对应 Bundle 后，永久退出该 task 的 optimistic row。
- Bundle 后续从 snapshot 消失时，不重新显示“已接收，等待处理”或“处理中，暂不可删除”。
- 覆盖手动删除、Issue 删除、服务端 cleanup/recovery、轮询和同一页面内的刷新路径。
- 不改变服务端 Bundle 的生命周期、上传失败处理或重试语义。

## 非目标

- 不修复 ZIP 解压、Tantivy publication 或后台物理 cleanup。
- 不修改 SQLite writer admission、temp-result preview 或 Bundle 删除 API。
- 不把服务端 Bundle 状态镜像到本地 queue；服务端 snapshot 仍是已 handoff Bundle 的唯一展示来源。

## 方案比较

### 方案 A：显式 `HANDED_OFF` 状态（采用）

给 upload queue 增加 `HANDED_OFF` 状态，并提供幂等的 `markHandedOff(taskId)`。当当前 Bundle snapshot 首次包含 task response 的 hash 时，UI 调用该方法；之后所有 optimistic row 过滤都以这个永久状态为准。

优点是 handoff 事实和 queue task 生命周期位于同一状态机中，容易测试，也能在 Bundle 消失后保留可追踪的本地事实。代价是需要扩展状态联合类型、过滤逻辑和相关测试。

### 方案 B：首次观察到 Bundle 后直接移除 queue task

在观察到 Bundle 后从 queue 删除 task。实现更小，但会丢失本地 task 的最终状态，且 queue 的删除语义会与失败、重试、Issue 切换行为耦合；未来需要诊断上传时也没有 handoff 记录。

### 方案 C：只在页面层维护已确认 hash 集合

由 `HomeView` 保存已经观察到的 hash，过滤本地 rows。该集合与 queue 生命周期分离，容易在页面重挂载、Issue 切换或多个消费者之间出现状态不同步，因此不采用。

## 设计

### 1. Queue 状态和转换

`UploadQueueTaskStatus` 增加：

```ts
'HANDED_OFF'
```

只允许以下转换触发 handoff：

```text
ACCEPTED --首次观察到同 hash Bundle--> HANDED_OFF
```

`markHandedOff(taskId)` 必须满足：

- task 存在；
- 当前状态是 `ACCEPTED`；
- task 有 response；
- 只更新状态，不清空 response、文件名或其他诊断信息；
- 重复调用不产生额外状态变化，也不影响其他状态。

其他状态（包括 `QUEUED`、`UPLOADING`、`RETRY_WAIT`、`FAILED`、`UNCONFIRMED`）不能被错误标记为 handoff。

### 2. 服务端 snapshot 到 queue 的 handoff

`useUploadTask` 暴露一个按 Bundle hash 确认已接管任务的方法。`HomeView` 在收到 `bundles.bundles` 后，收集当前 snapshot 的 hash，并将同 Issue、状态为 `ACCEPTED`、response hash 命中的 task 标记为 `HANDED_OFF`。

这个确认发生在 React effect 中而不是 render 中，避免渲染期间修改外部 queue。现有 `useUploadTask` 在 `ACCEPTED` 后触发 Bundle/Issue 刷新；刷新成功后 `HomeView` 的 snapshot effect 完成 handoff。

如果刷新暂时失败或 snapshot 暂时不包含该 Bundle，不做 handoff，保留原有 `ACCEPTED` optimistic row，直到真正观察到对应 Bundle。若 Bundle 随后消失，已是 `HANDED_OFF` 的 task 仍不会复活。

### 3. 行过滤和服务端展示

本地 task 的显示规则调整为：

- `HANDED_OFF` 永远不生成 optimistic row；
- 未 handoff 的 `ACCEPTED` task 仍沿用现有逻辑：当当前 snapshot 已含 hash 时暂时隐藏，等待 effect 完成状态转换；
- 其他本地状态继续按现有规则展示；
- 服务端 Bundle row 继续完全由 `bundles.bundles` snapshot 构造。

`HomeView` 的上传面板和 `createOptimisticUploadRows` 使用同一 handoff 过滤语义，避免文件表格与上传面板出现不一致。Bundle 删除后，服务端 row 由 snapshot 移除，`HANDED_OFF` task 不会重新生成本地 row，最终列表为空。

### 4. 错误和边界处理

- 上传请求未确认、失败或等待重试时，不允许 handoff；原有重试入口保持不变。
- hash 只做精确字符串匹配，沿用现有 `bundle_hash` 语义。
- 同一个 task 被多个渲染周期或多个 snapshot 重复确认时，操作必须幂等。
- Issue 切换不改变 task 的原始 `issueCode`；只处理当前 Issue 对应的 task，避免旧 Issue 的 snapshot 误确认新 Issue 的任务。
- 页面内清空 Bundle snapshot 不会反向改变 queue 状态。

## 测试策略

在现有前端行为测试中增加：

1. Queue 接受后可从 `ACCEPTED` 转为 `HANDED_OFF`，并验证非法状态不会转换、重复转换幂等。
2. `HANDED_OFF` task 即使 `existingBundleHashes` 为空，也不生成 optimistic row。
3. 先观察到 Bundle、再从 snapshot 删除时，不重新生成本地 row，也不出现“已接收，等待处理”或“处理中，暂不可删除”。
4. FAILED Bundle 被删除后列表保持为空。
5. 正常 `READY` Bundle 仍由服务端 row 展示，handoff 不影响文件显示。
6. 轮询/重复刷新不会重复触发 handoff，也不会让旧 Issue task 污染当前 Issue。

验证命令：

```bash
cd frontend
npm test
npm run build
cd ../backend
cargo build
```

## 验收标准

- [ ] local upload task 与 server Bundle 存在一次性 ownership handoff。
- [ ] Bundle 第一次出现在 snapshot 后，local `ACCEPTED` row 永久退出。
- [ ] Bundle 删除后不复活 `已接收，等待处理`。
- [ ] FAILED Bundle 删除后列表保持为空。
- [ ] 修复不依赖 delete button 清理 local task。
- [ ] 正常上传、失败、删除、Issue 删除、刷新和轮询路径行为一致。
- [ ] 有前端行为测试覆盖 ghost row 回归。
