# Issue #184：WSL 与非容器环境的内存探测设计

状态：已批准方案，待实现。

## 1. 目标与边界

本次修复属于 #184 adaptive runtime tuning 的资源探测基础能力，只改善内存资源探测可靠性和可解释性，不改变现有 adaptive tuning 公式、退让顺序、Manual/Auto 语义或运行时并发算法。

覆盖范围：

- Linux、Docker/Podman 等 cgroup v1/v2 环境；
- Windows 原生环境；
- Windows + WSL2，优先使用 Linux `/proc/meminfo`；
- 普通 Linux、无容器限制环境。

明确不支持 macOS 专用内存 API。本次在 macOS 上没有可用探测结果时保留保守 fallback。

## 2. 方案结论

保留现有 `backend/src/runtime_adaptive.rs` 的 `ResourceSnapshot`、`RuntimePlan` 和 `resolve` 结构，只扩展内存探测实现、来源模型和 UI 展示。

内存探测顺序固定为：

```text
当前进程所属 cgroup v2/v1 的有限内存限制
        ↓ 失败或无限制
Linux /proc/meminfo 的 MemTotal
        ↓ 不适用或读取/解析失败
Windows GlobalMemoryStatusEx
        ↓ 不适用或调用失败
512 MiB fallback + fallback reason
```

CPU 探测、资源计算和 adaptive plan 的公式不在本次修改范围内。

## 3. 后端资源模型与数据流

### 3.1 来源

`ResourceSource` 新增 `ProcMeminfo`，序列化为 `proc_meminfo`；既有 `Cgroup`、`Os`、`Fallback` 保留。

管理员 API 的资源快照继续由 `RuntimePlan.resources` 自动序列化，因此不新增 endpoint。`memory_source` 的含义变为：

- `cgroup`：使用当前进程 cgroup 层级及祖先中发现的有限限制；
- `proc_meminfo`：使用 `/proc/meminfo` 的 `MemTotal`；
- `os`：使用 Windows `GlobalMemoryStatusEx`；
- `fallback`：以上探测都不可用。

`ResourceSnapshot` 增加可选 `memory_fallback_reason` 字段。只有 `memory_source == fallback` 时填充稳定原因码 `memory_detection_unavailable`，其他来源为 `null`。路径、挂载点和底层错误文本不直接返回 UI，避免泄露主机细节。

原有 `memory_probe_fallback` warning 保留，确保现有调用方兼容；fallback reason 负责提供 UI 的具体原因展示。

数据流保持：

```text
ResourceSnapshot::probe()
        ↓
RuntimePlan::resolve()
        ↓
state.runtime_plan
        ↓
GET /api/admin/settings.runtime.resources
        ↓
管理员页面资源卡片
```

`resolve` 只消费 `memory_limit_bytes`，不根据来源改变任何自适应算法。

### 3.2 Linux cgroup 探测

不再假设 `/sys/fs/cgroup/memory.max` 或 `/sys/fs/cgroup/memory/memory.limit_in_bytes` 是固定根路径。

实现分为可测试的解析步骤和生产文件读取步骤：

1. 读取 `/proc/self/cgroup`，获得当前进程在 unified cgroup 或 memory controller 下的相对路径；
2. 读取 `/proc/self/mountinfo`，找到 cgroup v2 挂载点，或包含 `memory` controller 的 cgroup v1 挂载点；
3. 将挂载根、挂载点和当前相对路径组合成当前进程 cgroup 目录；
4. 从当前目录向挂载根逐级检查内存限制，并取所有有限有效值的最小值；
5. v2 读取 `memory.max`；v1 读取 `memory.limit_in_bytes`；
6. `max`、读取失败、解析失败、零值以及 v1 的无限制哨兵值均视为该层无有限限制，而不是失败整个探测；
7. 如果没有任何有限限制，继续 `/proc/meminfo`。

解析器使用字符串和虚拟文件读取器作为输入，测试不依赖宿主机 cgroup。生产环境仍只读系统文件，不修改 cgroup 状态。

### 3.3 Linux host memory

解析 `/proc/meminfo` 中的 `MemTotal:`，按 KiB 转换为字节并检查乘法溢出。缺失、格式错误、零值或无法读取时继续下一个来源。

该来源明确返回 `ResourceSource::ProcMeminfo`，让 WSL/普通 Linux 页面显示 `/proc/meminfo`，不再笼统显示“操作系统”。

### 3.4 Windows API

在 `cfg(windows)` 下使用 `windows-sys` 的 `GlobalMemoryStatusEx` 和 `MEMORYSTATUSEX::ullTotalPhys`。调用成功且值大于零时返回 `ResourceSource::Os`；结构初始化、API 失败或数值无效时继续 fallback。

本次不实现 macOS `sysctl` 或其他 macOS 专用 API；非 Linux、非 Windows 平台保留 fallback。

## 4. UI 与兼容性

前端 API 类型新增 `proc_meminfo` 来源和可空 `memory_fallback_reason`。

运行时资源卡片调整为：

- 内存数值单独显示；
- 下一行显示来源：`cgroup 限制`、`/proc/meminfo`、`操作系统 API` 或 `fallback`；
- 只有 fallback 且存在 reason 时显示“原因：内存探测不可用”；
- CPU 维持现有紧凑显示和文案；
- 内存目标、当前估算占用和 warning 继续原样展示。

后端来源码和 reason code 是稳定契约，中文文案由前端映射。未知来源码仍使用原始字符串兜底，避免旧后端/新前端组合崩溃。

## 5. 错误处理

- 任一 cgroup 文件缺失或权限失败不会阻止启动；继续查找其他层级或下一个来源；
- 单个 cgroup 层级格式错误不会覆盖其他祖先的有效限制；
- `/proc/meminfo` 不可读或格式错误不会产生 panic；
- Windows API 失败不会阻止启动；
- 所有来源失败后使用现有 512 MiB 保守值，设置 `memory_source=fallback`、`memory_fallback_reason=memory_detection_unavailable` 和 `memory_probe_fallback` warning；
- 不改变已有 CPU fallback、adaptive plan warning 和资源预算逻辑。

## 6. 测试设计

### 后端

在 `runtime_adaptive` 单元测试中覆盖：

- `/proc/meminfo` 正常解析、单位转换和格式错误；
- cgroup v2 当前层级限制；
- cgroup v2 祖先限制取最小值；
- cgroup v1 memory controller 路径和限制解析；
- `max`、v1 无限大哨兵、零值和坏值被忽略；
- cgroup 无有限限制时回退到 `proc_meminfo`；
- 所有来源不可用时返回 512 MiB、`fallback` 和 reason code；
- `ResourceSource` 与新增字段的 JSON 序列化；
- 现有 adaptive plan 测试继续证明探测来源不会改变 tuning 公式。

Windows API 分支通过 `cfg(windows)` 编译路径保持类型正确；Linux 单元测试不调用真实宿主机 cgroup。

### 前端

在管理员设置行为测试中覆盖：

- `proc_meminfo` 显示为 `/proc/meminfo`；
- fallback 显示来源和 fallback reason；
- 非 fallback 来源不显示 fallback reason；
- 现有资源 warning、Auto/Manual 和设置保存行为不回归。

完成前运行后端完整 `cargo test --manifest-path backend/Cargo.toml`、前端完整 `npm test` 和前端 `npm run build`。

## 7. 非目标

- 不改变 `resolve` 中 CPU/内存到并发数的公式；
- 不增加热调、周期采样或运行中重建 runtime；
- 不新增管理员配置项或数据库 migration；
- 不实现 macOS 内存 API；
- 不把 `MemAvailable` 当作固定内存上限；
- 不把宿主机 `/proc/meminfo` 误当成容器限制：只在没有有限 cgroup 限制时使用它。
