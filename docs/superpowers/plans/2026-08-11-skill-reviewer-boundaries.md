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
