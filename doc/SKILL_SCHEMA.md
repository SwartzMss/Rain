# Rain `SKILL.md` v1

Rain 的用户 Skill 是一个结构化诊断 Playbook。格式是否合法由服务端确定性校验，内容质量由 AI Reviewer 在格式合法之后单独评价。

## 最小模板

```markdown
---
schema_version: 1
---

# 目标

描述该 Skill 需要诊断的问题和最终目标。

# 分析范围

描述重点关注的模块、日志类型和边界。

# 检索策略

描述如何逐步定位相关日志、缩小范围和读取上下文。

# 证据规则

描述哪些信息可以作为结论证据，以及哪些信息只能用于定位。

# 日志不完整处理

描述日志缺失、截断或证据不足时应该如何处理。

# 停止条件

描述什么时候认为证据已经充分，以及什么时候应停止并报告证据不足。
```

## 结构规则

- 内容必须是有效 UTF-8，且不超过 64 KiB。
- 文件必须以 YAML front matter 开始，并声明 `schema_version: 1`。v1 不接受其他版本。
- 以下六个一级标题必须使用固定中文名称、各出现一次且正文非空；顺序可以自由调整。
- fenced code block 中形似标题的文本不参与章节识别。
- 可以添加 `输出重点`、`领域知识`、`已知模式`、`已知错误码`、`示例` 或其他自定义章节。
- 英文标题或中文别名不能替代标准章节，例如 `# Goal`、`# 任务目标` 和 `# 目的` 都不能替代 `# 目标`。

| 标准章节 | 内部 key | AI 评分维度 |
| --- | --- | --- |
| `目标` | `goal` | `task_scope` |
| `分析范围` | `scope` | `task_scope` |
| `检索策略` | `retrieval_strategy` | `retrieval_strategy` |
| `证据规则` | `evidence_rules` | `evidence_constraints` |
| `日志不完整处理` | `incomplete_logs` | `incomplete_logs` |
| `停止条件` | `stop_conditions` | `stopping_conditions` |

Reviewer 的第六个维度 `clarity` 根据全部章节整体评价。章节存在只代表结构完整；内容空泛仍会得到低分。

## 校验、评分与运行

创建或修改 Skill 时，服务端先解析并校验结构。格式错误返回 HTTP 400、稳定错误码 `SKILL_FORMAT_INVALID` 和具体中文原因；非法内容不会保存。

质量评估只接受合法 v1 Skill，继续返回固定的六个英文维度 key。格式非法时不会请求 AI Provider。

Runner 只注入 front matter 之后的 Playbook 正文，`schema_version` 不会成为诊断指令。Skill 描述 What/Strategy，不能授予 shell、网络、文件写入、SQL、跨 Issue 或额外工具权限；当前 Issue 绑定、只读工具和证据规则始终由平台强制执行。

## 历史 Skill

开发阶段已有的自由格式 Skill 仍可列出、查看和删除，详情中的 `schema_version` 为 `null`。下一次编辑、质量评估或运行前必须按 v1 模板完成迁移；Rain 不为旧标题维护永久 alias。
