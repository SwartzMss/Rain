export const SKILL_SCHEMA_VERSION = 1;

export const REQUIRED_SKILL_SECTIONS = [
  '目标',
  '分析范围',
  '关键流程',
  '关键日志',
  '关系与影响'
] as const;

export const DEFAULT_SKILL_MARKDOWN = `---
schema_version: ${SKILL_SCHEMA_VERSION}
---

# 目标

诊断：
- 需要确定什么故障；
- 最终希望回答什么问题。

非目标：
- 本 Skill 不负责判断哪些相邻问题。

# 分析范围

关注：
- 相关组件；
- 业务对象；
- 日志域；
- 相关子流程。

排除：
- 明确不属于本 Skill 的业务范围。

# 关键流程

正常流程：

阶段 A → 阶段 B → 阶段 C

前置依赖：
- B 依赖 A 成功；
- C 依赖 B 产生有效结果。

关键状态：
- A_SUCCESS：……
- B_FAILED：……

# 关键日志

## 信号 A

特征：\`xxx\`

类型：定位信号

含义：用于识别某业务调用，本身不表示异常。

## 信号 B

特征：\`yyy\`

类型：失败信号

含义：表示……

# 领域判定规则

- 信号 A + 信号 B → 支持候选原因 X。
- 信号 C 成功 → 排除候选原因 Y。
- 只有信号 B → 只能确认阶段 B 失败，不能确定其上游原因。

# 关系与影响

A 是 B 的前置依赖。

A 失败 → B 无法完成 → C 出现失败症状。

因此 A 属于上游原因，B/C 可能是故障传播结果。
`;
