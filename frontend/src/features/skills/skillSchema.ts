export const SKILL_SCHEMA_VERSION = 1;
export const UNRECOGNIZED_SKILL_SCHEMA_LABEL = '未识别（需迁移到 v1）';

export const REQUIRED_SKILL_SECTIONS = [
  '目标',
  '分析范围',
  '检索策略',
  '证据规则',
  '日志不完整处理',
  '停止条件'
] as const;

export const DEFAULT_SKILL_MARKDOWN = `---
schema_version: ${SKILL_SCHEMA_VERSION}
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
`;
