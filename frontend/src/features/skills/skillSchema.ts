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

描述该 Skill 需要诊断的问题和最终目标。

# 分析范围

描述重点关注的模块、日志类型和边界。

# 关键流程

描述正常业务流程、关键步骤和前置依赖。

# 关键日志

描述关键日志模式，以及它们分别代表的业务事件或状态。

# 关系与影响

描述事件之间的依赖、因果、影响和故障传播关系。
`;
