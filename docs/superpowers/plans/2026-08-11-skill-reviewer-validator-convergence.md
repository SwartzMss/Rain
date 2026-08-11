# Skill Reviewer Validator Convergence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 Skill Reviewer 的确定性校验收敛到结构、简体文本、局部英文正文和明确禁用字面量，把未知工具与 Evidence Policy 等开放式语义明确留给 System Prompt best-effort 约束。

**Architecture:** `parse_review()` 不再解析 invocation/object、否定作用域、推理链或停止条件语义。文本契约按 Han 字符隔开的 ASCII 片段检测多词英文正文，继续用 `zhconv` 检查简体字；suggestion 只执行上下文无关的禁用字面量检查。现有一次 repair 保留，并重申确定性文本契约。

**Tech Stack:** Rust 2024、Actix Web、serde、zhconv/OpenCC、Cargo 内置测试框架

---

### Task 1: 用测试锁定收敛后的 parser 边界

**Files:**
- Modify: `backend/src/routes/skills.rs` 的 `tests` 模块

- [ ] **Step 1: 增加“中文不能稀释英文正文”的失败测试**

在 `parse_review_rejects_chinese_prefix_with_english_body` 后加入：

```rust
#[test]
fn parse_review_rejects_english_prose_after_chinese_context() {
    let review = review_with_findings(
        "[]",
        r#"["建议进一步明确蓝牙故障范围和证据规则，Clarify the Bluetooth failure scope."]"#,
    );

    assert!(parse_review(Some(&review)).is_err());
}
```

- [ ] **Step 2: 增加 parser 不分类开放式语义的失败测试**

```rust
#[test]
fn parse_review_does_not_classify_open_ended_semantics() {
    let review = review_with_findings(
        "[]",
        r#"["使用 awk 对日志进行搜索。","日志不完整时先保留待验证假设；补齐缺失日志并验证后再形成根因结论。"]"#,
    );

    assert!(parse_review(Some(&review)).is_ok());
}
```

该测试明确记录边界：`awk` 与 Evidence Policy 的语义由 Prompt 约束，不由 parser 猜测。当前实现会因第二条 suggestion 的 inference-to-conclusion 规则而失败。

- [ ] **Step 3: 增加具体禁用能力不解释上下文的失败测试**

```rust
#[test]
fn parse_review_rejects_forbidden_capability_literals_regardless_of_context() {
    for suggestion in [
        "使用 grep 搜索蓝牙日志。",
        "删除 grep 指令并改写检索策略。",
        "不要调用外部解析器。",
        "不要发起网络访问。",
    ] {
        let review = review_with_findings("[]", &serde_json::json!([suggestion]).to_string());
        assert!(parse_review(Some(&review)).is_err(), "{suggestion}");
    }
}
```

同时增加 `parse_review_accepts_generic_capability_language`，确保 `工具`、`命令`、`外部工具` 和 `第三方工具` 这类普通关系词不会误触发 repair。

- [ ] **Step 4: 用新的责任边界测试替换旧语义断言**

删除以下旧测试；它们要求 parser 对未知工具、否定作用域、Evidence Policy 或停止条件做开放式语义分类，与已确认设计冲突：

- `parse_review_rejects_unenumerated_external_tools`
- `parse_review_rejects_cross_sentence_unsupported_inference`
- `parse_review_does_not_apply_negation_to_later_violations`
- `parse_review_rejects_suggestions_that_cross_diagnostic_boundaries`

这些覆盖分别由 `parse_review_does_not_classify_open_ended_semantics`、`parse_review_rejects_forbidden_capability_literals_regardless_of_context`、`parse_review_accepts_generic_capability_language` 和 System Prompt 契约测试承担。

- [ ] **Step 5: 调整合法反馈 fixture**

从 `parse_review_allows_chinese_feedback_with_technical_terms_and_safe_boundaries` 的合法 suggestions 中删除含具体禁用能力名的否定式样例；普通的工具无关表述由独立回归测试覆盖。保留：

```rust
let review = review_with_findings(
    r#"["Skill 中存在未授权能力说明。"]"#,
    r#"["检查 Bluetooth 日志。","读取 com.android.bluetooth 和 BT_PARSER_TIMEOUT 的原始日志上下文。","使用时间和模块逐步缩小候选日志范围。","通过关键词搜索蓝牙失败信号。","日志截断时，将 HCI_TIMEOUT 根因假设标记为待验证。","日志不完整时，可以保留推断。将结果标记为待验证假设，不作为根因结论。","当原始日志证据足够或可用日志已耗尽时停止。"]"#,
);
```

- [ ] **Step 6: 运行聚焦测试确认 RED**

Run:

```bash
cd backend
cargo test routes::skills::tests::parse_review_
```

Expected: `parse_review_rejects_english_prose_after_chinese_context`、`parse_review_does_not_classify_open_ended_semantics` 和至少一个上下文无关字面量 case 失败；现有结构/评分测试不报编译错误。

### Task 2: 用稳定文本契约替换自然语言规则引擎

**Files:**
- Modify: `backend/src/routes/skills.rs:246-570`
- Test: `backend/src/routes/skills.rs` 的 `tests` 模块

- [ ] **Step 1: 将反馈校验改为局部 ASCII 片段规则**

用以下实现替换 `feedback_is_chinese_dominant`：

```rust
fn ascii_prose_word_count(value: &str) -> usize {
    value
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '/' | ':'))
        })
        .filter(|token| token.chars().any(|character| character.is_ascii_alphabetic()))
        .filter(|token| {
            let has_identifier_syntax = token.chars().any(|character| {
                character.is_ascii_digit() || matches!(character, '_' | '.' | '/' | ':')
            });
            let letters: Vec<_> = token
                .chars()
                .filter(|character| character.is_ascii_alphabetic())
                .collect();
            let is_acronym = letters.len() > 1
                && letters
                    .iter()
                    .all(|character| character.is_ascii_uppercase());

            !has_identifier_syntax && !is_acronym
        })
        .count()
}

fn feedback_matches_text_contract(value: &str) -> bool {
    value.chars().any(is_han)
        && zhconv(value, Variant::ZhHans) == value
        && !value
            .split(is_han)
            .any(|ascii_run| ascii_prose_word_count(ascii_run) >= 2)
}
```

在 `parse_review()` 中把 `feedback_is_chinese_dominant(item)` 改为 `feedback_matches_text_contract(item)`。

- [ ] **Step 2: 增加上下文无关的 suggestion 字面量校验**

保留 `contains_ascii_term` 的词边界行为，并增加：

```rust
fn suggestion_contains_forbidden_literal(suggestion: &str) -> bool {
    const ASCII_LITERALS: &[&str] = &[
        "grep",
        "shell",
        "parser",
        "script",
        "sql",
        "network access",
        "network request",
        "curl",
    ];
    const CHINESE_LITERALS: &[&str] = &["解析器", "脚本", "网络访问", "网络请求"];

    let suggestion = suggestion.to_lowercase();
    ASCII_LITERALS
        .iter()
        .any(|literal| contains_ascii_term(&suggestion, literal))
        || CHINESE_LITERALS
            .iter()
            .any(|literal| suggestion.contains(literal))
}
```

在 `parse_review()` 中调用 `suggestion_contains_forbidden_literal`。

- [ ] **Step 3: 删除开放式自然语言分析代码**

完整删除以下内容，不保留替代词表：

- `find_earliest`
- `capability_is_negated`
- `suggestion_crosses_diagnostic_boundary`
- `INVOCATIONS`、`DIAGNOSTIC_ACTIONS`、`STRATEGY_OBJECTS`
- `INCOMPLETE_LOGS`、`INFERENCES`、`CONCLUSIONS`、`UNVERIFIED_QUALIFIERS`
- `CIRCULAR_STOPS` 及其句子/子句扫描

- [ ] **Step 4: 运行聚焦测试确认 GREEN**

Run:

```bash
cd backend
cargo test routes::skills::tests::parse_review_
```

Expected: 所有 `parse_review_` 测试通过，输出无 warning。

- [ ] **Step 5: 提交 parser 收敛变更**

```bash
git add backend/src/routes/skills.rs
git commit -m "fix: narrow reviewer validation contract"
```

### Task 3: 让 repair 和文档准确描述责任边界

**Files:**
- Modify: `backend/src/routes/skills.rs:20-50, 214-225`
- Modify: `docs/superpowers/plans/2026-08-11-skill-reviewer-boundaries.md`
- Test: `backend/src/routes/skills.rs` 的 `tests` 模块

- [ ] **Step 1: 先写 repair 提示契约测试**

定义测试目标为常量 `SKILL_REVIEW_REPAIR_PROMPT`，并在 `reviewer_feedback_uses_chinese_and_respects_diagnostic_boundaries` 中增加：

```rust
for expected in [
    "Simplified Chinese",
    "Do not include forbidden capability names",
] {
    assert!(SKILL_REVIEW_REPAIR_PROMPT.contains(expected));
}
```

此时常量尚不存在，Expected: 编译失败，报告找不到 `SKILL_REVIEW_REPAIR_PROMPT`。

- [ ] **Step 2: 运行单测确认 RED**

Run:

```bash
cd backend
cargo test routes::skills::tests::reviewer_feedback_uses_chinese_and_respects_diagnostic_boundaries
```

Expected: FAIL to compile because `SKILL_REVIEW_REPAIR_PROMPT` is unresolved。

- [ ] **Step 3: 定义并使用 repair 提示常量**

在 System Prompt 常量后加入：

```rust
const SKILL_REVIEW_REPAIR_PROMPT: &str = concat!(
    "Return only valid JSON matching the requested review schema. ",
    "Write every warning and suggestion in Simplified Chinese, allowing only isolated technical identifiers. ",
    "Do not include forbidden capability names in suggestions; describe the diagnostic intent instead."
);
```

将 repair user message 的硬编码字符串替换为：

```rust
content: Some(SKILL_REVIEW_REPAIR_PROMPT.into()),
```

并将该常量加入测试模块的 `use super::{...}`。

- [ ] **Step 4: 更新旧实施计划的最终架构记录**

在 `docs/superpowers/plans/2026-08-11-skill-reviewer-boundaries.md` 末尾增加：

```markdown
### Task 6: Review follow-up — 收敛确定性 validator

- [x] 删除 invocation/object、否定作用域、Evidence 推理链和循环停止条件自然语言解析。
- [x] 简体文本改用 ZhHans 一致性与按 Han 分隔的局部 ASCII prose 检查。
- [x] suggestion 只保留上下文无关的具体禁用能力字面量检查，不拦截 `工具`、`命令` 等普通关系词。
- [x] 未知工具、Evidence Policy 和停止条件由 System Prompt best-effort 约束。
- [x] repair 提示重申简体中文、孤立技术标识和禁用能力字面量契约。
```

- [ ] **Step 5: 运行 Reviewer 测试确认 GREEN**

Run:

```bash
cd backend
cargo test routes::skills::tests::reviewer_
cargo test routes::skills::tests::parse_review_
```

Expected: 两组测试均通过，0 个失败、无 warning。

- [ ] **Step 6: 提交 repair 与文档变更**

```bash
git add backend/src/routes/skills.rs docs/superpowers/plans/2026-08-11-skill-reviewer-boundaries.md
git commit -m "docs: record reviewer validator boundary"
```

### Task 4: 完整验证并更新 PR

**Files:**
- Verify: `backend/Cargo.toml`
- Verify: `backend/Cargo.lock`
- Verify: `backend/src/routes/skills.rs`
- Verify: `docs/superpowers/specs/2026-08-11-skill-reviewer-boundaries-design.md`
- Verify: `docs/superpowers/plans/2026-08-11-skill-reviewer-validator-convergence.md`

- [ ] **Step 1: 运行 CI 等价验证**

Run:

```bash
cd backend
cargo fmt --check
cargo clippy --locked -- -D warnings
cargo test --locked
cd ..
git diff --check
```

Expected: format、Clippy、全部自动化测试和补丁检查通过；只有手工 benchmark 保持 ignored。

- [ ] **Step 2: 检查架构收敛结果**

Run:

```bash
rg -n "INVOCATIONS|DIAGNOSTIC_ACTIONS|STRATEGY_OBJECTS|UNVERIFIED_QUALIFIERS|capability_is_negated|suggestion_crosses_diagnostic_boundary" backend/src/routes/skills.rs
git status --short
```

Expected: `rg` 无匹配；工作区没有未提交改动。

- [ ] **Step 3: 推送并更新 PR #99**

```bash
git push origin codex/issue-96-reviewer-boundaries
```

PR body 必须明确：确定性 validator 只保证结构、简体转换、局部英文正文和列出的禁用字面量；未知工具、Evidence Policy 和停止条件由 System Prompt best-effort 约束；不得声称任意自然语言语义均被持久化前强校验。

- [ ] **Step 4: 等待 CI 完成**

Expected: 新 head 对应的 GitHub Actions CI conclusion 为 `success`。不合并 PR，不回复或 resolve review thread，除非用户另行明确授权。
