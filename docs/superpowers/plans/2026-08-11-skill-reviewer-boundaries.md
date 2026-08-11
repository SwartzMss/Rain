# Skill Reviewer 中文输出与诊断边界 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 收紧 Skill Reviewer 的用户可见反馈，使其统一使用简体中文并遵守 Rain 的能力、证据和停止条件边界。

**Architecture:** 仅扩展 `SKILL_REVIEW_SYSTEM_PROMPT`，通过模型指令约束 `warnings` 与 `suggestions` 的语言和语义。现有 JSON 解析、评分权重、grade 派生和持久化流程保持不变；同文件内的聚焦单元测试锁定新增 Prompt 契约。

**Tech Stack:** Rust、Actix Web、Rust 内置测试框架、Cargo

---

### Task 1: 锁定 Reviewer 输出边界契约

**Files:**
- Modify: `backend/src/routes/skills.rs:398`
- Test: `backend/src/routes/skills.rs` 的 `tests` 模块

- [ ] **Step 1: 写入失败测试**

在 `reviewer_rubric_maps_chinese_sections_and_penalizes_generic_content` 之后增加：

```rust
#[test]
fn reviewer_feedback_uses_chinese_and_respects_diagnostic_boundaries() {
    for expected in [
        "All user-visible warnings and suggestions must be written in Simplified Chinese",
        "Suggestions must describe diagnostic intent and strategy",
        "not shell commands, grep, external parsers, scripts, SQL, network access, or unavailable tools",
        "never recommend treating unsupported inference as a conclusion",
        "identifying missing evidence",
        "marking hypotheses as unverified",
        "Stopping-condition suggestions must be objectively checkable",
        "available logs being exhausted without enough evidence",
    ] {
        assert!(SKILL_REVIEW_SYSTEM_PROMPT.contains(expected));
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test routes::skills::tests::reviewer_feedback_uses_chinese_and_respects_diagnostic_boundaries`

Expected: FAIL，首个缺失约束触发 `assertion failed: SKILL_REVIEW_SYSTEM_PROMPT.contains(expected)`。

- [ ] **Step 3: 最小化补充 System Prompt**

在现有 unsupported-capability 约束之后、untrusted Markdown 约束之前加入：

```rust
"All user-visible warnings and suggestions must be written in Simplified Chinese. ",
"Suggestions must describe diagnostic intent and strategy, not shell commands, grep, external parsers, scripts, SQL, network access, or unavailable tools. ",
"For incomplete logs, never recommend treating unsupported inference as a conclusion. Recommend identifying missing evidence, requesting additional context when applicable, or marking hypotheses as unverified. ",
"Stopping-condition suggestions must be objectively checkable, such as verified evidence being sufficient, a defined diagnostic question being answered, or available logs being exhausted without enough evidence. ",
```

- [ ] **Step 4: 运行聚焦测试确认通过**

Run: `cargo test routes::skills::tests::reviewer_feedback_uses_chinese_and_respects_diagnostic_boundaries`

Expected: PASS，1 个测试通过、0 个失败。

- [ ] **Step 5: 运行现有 Reviewer 测试确认协议未回归**

Run: `cargo test routes::skills::tests::reviewer_`

Expected: PASS；rubric 映射、原始 Skill body 传递和新增边界测试均通过。

- [ ] **Step 6: 提交实现**

```bash
git add backend/src/routes/skills.rs
git commit -m "fix: constrain skill reviewer feedback"
```

### Task 2: 完整验证与发布准备

**Files:**
- Verify: `backend/src/routes/skills.rs`
- Verify: `docs/superpowers/specs/2026-08-11-skill-reviewer-boundaries-design.md`
- Verify: `docs/superpowers/plans/2026-08-11-skill-reviewer-boundaries.md`

- [ ] **Step 1: 格式化并确认无额外改动**

Run: `cargo fmt --check`

Expected: PASS，无格式差异。

- [ ] **Step 2: 运行 backend 完整测试**

Run: `cargo test`

Expected: PASS，所有测试通过、0 个失败。

- [ ] **Step 3: 检查补丁质量**

Run: `git diff origin/main...HEAD --check`

Expected: PASS，无尾随空格或冲突标记。

Run: `git status --short`

Expected: 工作区干净。

- [ ] **Step 4: 推送分支并创建草稿 PR**

```bash
git push -u origin codex/issue-96-reviewer-boundaries
```

创建以 `main` 为 base 的草稿 PR，标题使用 `fix: constrain skill reviewer feedback`，正文概述 Prompt 约束与测试，并包含 `Fixes #96`。

### Task 3: Review follow-up — 在保存前强制反馈契约

**Files:**
- Modify: `backend/src/routes/skills.rs`
- Modify: `docs/superpowers/specs/2026-08-11-skill-reviewer-boundaries-design.md`
- Test: `backend/src/routes/skills.rs` 的 `tests` 模块

- [x] **Step 1: 写入合法 JSON 但反馈违规的失败测试**

覆盖纯英文 warning/suggestion、推荐未授权工具、日志不完整时将推断作为根因、循环停止条件，以及包含英文技术标识符的合法中文反馈。

- [x] **Step 2: 运行聚焦测试确认 RED**

Run: `cargo test routes::skills::tests::parse_review_`

Expected: 违规反馈仍被接受的用例失败；合法反馈用例通过。

- [x] **Step 3: 实现确定性反馈校验**

要求每条反馈至少包含一个汉字；按句子检查 suggestions 是否推荐禁用能力、弱化证据规则或使用循环停止条件。英文能力名按独立词或短语匹配，并允许同句中的删除、禁止、避免等否定语义。

- [x] **Step 4: 运行聚焦测试确认 GREEN**

Run: `cargo test routes::skills::tests::parse_review_`

Expected: 所有 parser 契约测试通过。

- [x] **Step 5: 运行全量验证并更新 PR**

Run: `cargo fmt --check && cargo test && git diff origin/main...HEAD --check`

Expected: 格式检查和完整测试通过，补丁无空白错误；提交并推送到现有 PR #99。

### Task 4: Review follow-up — 收紧中文主体和开放式语义边界

**Files:**
- Modify: `backend/src/routes/skills.rs`
- Modify: `docs/superpowers/specs/2026-08-11-skill-reviewer-boundaries-design.md`
- Test: `backend/src/routes/skills.rs` 的 `tests` 模块

- [x] **Step 1: 复现中文前缀、未枚举工具和跨句证据升级**

加入 review 中的原样样例，运行 `cargo test routes::skills::tests::parse_review_rejects_`，确认三个新测试均为 RED。

- [x] **Step 2: 用中文主体比例替代单汉字判断**

统计 Han 字符与非技术 ASCII 自然语言词；要求 Han 数量至少是英文词数量的两倍，同时忽略错误码、包名、路径和缩写。

- [x] **Step 3: 用结构规则补充能力边界**

识别通用外部能力语义，并校验“调用对象 + 诊断动作”；只允许日志、证据、时间、模块、关键词、文件和上下文等策略对象。

- [x] **Step 4: 聚合整条 suggestion 的证据状态**

跨句按顺序识别日志不完整、推断和根因结论；待验证或明确否定结论的表达继续放行。

- [x] **Step 5: 验证合法技术文本与工具无关建议**

运行 `cargo test routes::skills::tests::parse_review_`，确认非法样例拒绝且 Bluetooth、包名、错误码、合法策略对象和工具无关建议均通过。

- [x] **Step 6: 运行 CI 等价验证并更新 PR**

Run: `cargo fmt --check && cargo clippy --locked -- -D warnings && cargo test --locked`

Expected: 格式、Clippy 和完整测试通过；提交并推送到 PR #99。

### Task 5: Review follow-up — 校验简体字与否定作用域

**Files:**
- Modify: `backend/Cargo.toml`
- Modify: `backend/Cargo.lock`
- Modify: `backend/src/routes/skills.rs`
- Modify: `docs/superpowers/specs/2026-08-11-skill-reviewer-boundaries-design.md`
- Test: `backend/src/routes/skills.rs` 的 `tests` 模块

- [x] **Step 1: 复现繁体、调用同义词和否定越界**

加入 review 原样样例及相同根因变体，运行聚焦测试确认繁体反馈、`用/利用` 外部工具和跨转折/跨子句否定均为 RED。

- [x] **Step 2: 使用 OpenCC 数据校验简体中文**

保留中文主体比例规则，并要求反馈转换到 `ZhHans` 后文本不变；使用 `zhconv` 的 OpenCC feature，避免维护不完整的手写简繁字符表。

- [x] **Step 3: 将工具否定绑定到具体能力位置**

枚举同一子句中的全部能力提及和调用结构，逐个判断其前置否定、转折边界和后置状态，防止早先的安全否定豁免后续推荐。

- [x] **Step 4: 将证据限定绑定到结论子句**

只接受与根因/结论位于同一子句的明确“待验证/不作为结论”限定，不再使用全 suggestion 的泛化否定词。

- [x] **Step 5: 运行 CI 等价验证并更新 PR**

Run: `cargo fmt --check && cargo clippy --locked -- -D warnings && cargo test --locked && git diff --check`

Expected: 格式、Clippy、完整测试和补丁检查通过；提交并推送到 PR #99。

### Task 6: Review follow-up — 收敛确定性 validator

- [x] 删除 invocation/object、否定作用域、Evidence 推理链和循环停止条件自然语言解析。
- [x] 简体文本改用 ZhHans 一致性与按 Han 分隔的局部 ASCII prose 检查。
- [x] suggestion 只保留上下文无关的明确禁用字面量检查。
- [x] 未知工具、Evidence Policy 和停止条件由 System Prompt best-effort 约束。
- [x] repair 提示重申简体中文、孤立技术标识和禁用能力字面量契约。

### Task 7: Review follow-up — 对齐关闭语义并收窄 denylist

- [x] deterministic denylist 只保留具体且稳定的能力名，允许 `工具`、`命令`、`外部工具` 和 `第三方工具` 等普通关系词出现在合规建议中。
- [x] 增加泛化能力措辞的回归测试，并同步 validator 设计记录。
- [x] 保留 #96 的原始硬验收标准，将 PR #99 的 `Fixes #96` 改为 `Refs #96`，避免合并时错误关闭 issue。
