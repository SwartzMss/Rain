# Issue #182 Configuration UX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the administrator settings page metadata-driven, with common settings visible, advanced settings collapsed, expert settings hidden behind an explicit reveal action, and clear advisory guidance for each field.

**Architecture:** Extend the existing backend `FieldMetadata` contract with presentation classification and advisory recommended bounds. Keep the current `/api/admin/settings` endpoint, revision checks, validation, persistence, and apply-mode behavior unchanged. Refactor the frontend settings page to group metadata fields rather than maintaining a frontend key classification table, while preserving its existing dedicated common controls and save paths.

**Tech Stack:** Rust, Actix Web, SQLx/SQLite, React, TypeScript, Vitest, Node behavior tests.

---

### Task 1: Extend and validate backend setting metadata

**Files:**
- Modify: `backend/src/settings/metadata.rs`
- Modify: `backend/src/settings/model.rs`
- Modify: `backend/src/settings/mod.rs`
- Test: `backend/tests/settings.rs` existing metadata tests

- [x] **Step 1: Add the presentation enums and metadata fields**

Add serializable enums for the three categories and visibility modes, then add them to `FieldMetadata` and its serializer:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingCategory {
    Common,
    Advanced,
    Expert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingVisibility {
    Default,
    Collapsed,
    Expert,
}
```

`FieldMetadata` must expose `category`, `visibility`, `recommended_min`, and `recommended_max` in the API response. Recommended bounds are advisory and separate from the existing hard `min`/`max` bounds.

Add an exhaustive `SettingKey::ALL` array in `backend/src/settings/model.rs` so coverage tests can compare the metadata table against the supported setting enum without maintaining a second test-only list.

- [x] **Step 2: Assign every existing `SettingKey` a category, visibility, and recommendation**

Update the single metadata table in `backend/src/settings/metadata.rs`. Use these rules:

```text
Common: registration, session/authentication policy, Issue size/inactivity/cleanup policy,
        and default search result size.
Advanced: archive working budget, upload concurrency/temp budget, preview/page/search limits,
          and temporary-result retention/scan limits.
Expert: Argon2 concurrency, indexing line-size budget, Tantivy writers/heap,
        line-read concurrency, and temporary-result materialization concurrency.
```

Use `Default` visibility for Common, `Collapsed` for Advanced, and `Expert` for Expert. Only add recommended bounds where the project has a defensible operational recommendation; leave them `None` when no recommendation should be implied.

- [x] **Step 3: Add a failing metadata invariant test**

Extend the metadata test module with assertions that:

```rust
assert_eq!(metadata::all().len(), SettingKey::ALL.len());
let keys: std::collections::HashSet<_> = metadata::all().iter().map(|field| field.key).collect();
assert_eq!(keys.len(), metadata::all().len());
assert!(metadata::all().iter().all(|field| {
    field.recommended_min.unwrap_or(0) <= field.recommended_max.unwrap_or(u64::MAX)
}));
```

Use the repository’s existing `SettingKey`/metadata coverage pattern rather than introducing a second source of truth. The test must also assert that the serialized response contains the new keys.

- [x] **Step 4: Run the focused backend tests and observe the expected failure before implementation is complete**

Run from the backend directory:

```bash
cargo test --test settings metadata
```

Expected: the new assertions initially fail because the metadata contract and assignments are not complete.

- [x] **Step 5: Implement the minimum metadata changes and make the focused tests pass**

Complete the enum serialization, all metadata assignments, and invariant checks. Do not alter validation logic or settings persistence.

Run from the backend directory:

```bash
cargo fmt --check
cargo test --test settings metadata
```

Expected: formatting passes and all focused metadata tests pass.

- [x] **Step 6: Commit the backend metadata contract**

```bash
git add backend/src/settings/metadata.rs backend/src/settings/model.rs backend/src/settings/mod.rs
git commit -m "feat: classify admin settings metadata"
```

### Task 2: Add frontend metadata types and grouped field helpers

**Files:**
- Modify: `frontend/src/api/types.ts`
- Create: `frontend/src/features/admin/settingsFields.ts`
- Test: `frontend/tests/admin-settings-ux.behavior.test.tsx`

- [x] **Step 1: Extend the API type for metadata presentation fields**

Extract the inline field type from `RegistrationSettings` and add the exact serialized values:

```ts
type SettingCategory = 'common' | 'advanced' | 'expert';
type SettingVisibility = 'default' | 'collapsed' | 'expert';

export interface RegistrationSettingField {
  key: string;
  db_column: string;
  env_name: string;
  value_type?: string;
  unit?: string | null;
  default_value?: unknown;
  default_rule?: string | null;
  min?: number | null;
  max?: number | null;
  description?: string;
  category: SettingCategory;
  visibility: SettingVisibility;
  recommended_min?: number | null;
  recommended_max?: number | null;
  apply_mode: 'hot' | 'restart_required';
  sensitive?: boolean;
}
```

Keep existing optional fields and response properties compatible.

- [x] **Step 2: Extract deterministic grouping and display helpers**

Create helpers that group fields using only backend metadata and format guidance without changing values:

```ts
export function groupSettingFields(fields: RegistrationSettingField[]) {
  return {
    common: fields.filter((field) => field.category === 'common'),
    advanced: fields.filter((field) => field.category === 'advanced'),
    expert: fields.filter((field) => field.category === 'expert'),
  };
}

export function recommendedRangeLabel(field: RegistrationSettingField): string | null {
  if (field.recommended_min == null && field.recommended_max == null) return null;
  return `推荐 ${field.recommended_min ?? '无下限'}–${field.recommended_max ?? '无上限'}`;
}
```

Keep these helpers pure so the frontend behavior tests can exercise them without rendering the whole admin shell.

- [x] **Step 3: Write failing frontend behavior tests**

Add tests that prove:

```ts
it('groups metadata fields by backend category', () => {
  expect(groupSettingFields(fields)).toEqual({
    common: [commonField],
    advanced: [advancedField],
    expert: [expertField],
  });
});

it('formats only advisory recommended ranges', () => {
  expect(recommendedRangeLabel(advancedField)).toBe('推荐 1–8');
  expect(recommendedRangeLabel(fieldWithoutRecommendation)).toBeNull();
});
```

- [x] **Step 4: Run the focused frontend test and verify it fails for the missing helpers**

Run from the frontend directory:

```bash
npm test -- --run tests/admin-settings-ux.behavior.test.tsx
```

Expected: FAIL because the grouping helpers do not yet exist.

- [x] **Step 5: Implement the helpers and make the focused test pass**

Run the same command and expect all focused tests to pass. Do not add a frontend classification map keyed by setting name.

- [x] **Step 6: Commit the frontend metadata helpers**

```bash
git add frontend/src/api/types.ts frontend/src/features/admin/settingsFields.ts frontend/tests/admin-settings-ux.behavior.test.tsx
git commit -m "feat: add grouped settings metadata helpers"
```

### Task 3: Refactor the administrator settings page presentation

**Files:**
- Modify: `frontend/src/features/admin/AdminPage.tsx`
- Modify: `frontend/src/features/admin/settingsFields.ts` if display helpers need a shared type
- Test: `frontend/tests/admin-settings-ux.behavior.test.tsx`

- [x] **Step 1: Write failing rendered behavior tests**

Add a settings-page test fixture containing one field per category and assert:

```ts
expect(screen.getByText('常用配置')).toBeVisible();
expect(screen.getByText('高级运行参数')).toBeVisible();
expect(screen.getByText('专家配置')).not.toBeVisible();
expect(screen.getByRole('button', { name: '显示专家配置' })).toBeVisible();
```

Also assert that clicking the reveal control renders the expert field, that advanced content is inside a closed `<details>`, and that the field displays its description, unit, hard range, recommendation, and apply mode.

- [x] **Step 2: Run the rendered test and verify the expected presentation failure**

Run from the frontend directory:

```bash
npm test -- --run tests/admin-settings-ux.behavior.test.tsx
```

Expected: FAIL because the current page has one undifferentiated advanced section and no expert reveal control.

- [x] **Step 3: Implement common, advanced, and expert rendering**

Keep the current dedicated common controls for registration, authentication limits, Issue expiry, and cleanup users. For metadata-driven fields:

- render Common metadata fields not covered by a dedicated control in a visible “常用配置” section with its own draft and revision-checked save action;
- render Advanced fields in a closed `<details>` section;
- keep Expert fields out of the DOM until `showExpertSettings` is true;
- render a separate expert section after the reveal action;
- show `description`, `unit`, hard validation bounds, recommendation, and `apply_mode` from metadata;
- keep one revision-checked save action for advanced fields and one for expert fields;
- preserve loading, stale revision, save failure, and pending restart states.

The reveal control changes visibility only; it must not mutate drafts or submit changes.

- [x] **Step 4: Run the focused rendered tests and the existing admin tests**

Run from the frontend directory:

```bash
npm test -- --run tests/admin-settings-ux.behavior.test.tsx tests/admin-guard.behavior.test.tsx
```

Expected: all settings UX and existing admin guard tests pass.

- [x] **Step 5: Commit the settings page UX**

```bash
git add frontend/src/features/admin/AdminPage.tsx frontend/src/features/admin/settingsFields.ts frontend/tests/admin-settings-ux.behavior.test.tsx
git commit -m "feat: organize admin settings by visibility"
```

### Task 4: Verify API compatibility and full regression coverage

**Files:**
- Modify: `backend/tests/settings.rs` if API metadata assertions need to be added
- Modify: `frontend/tests/admin-settings-ux.behavior.test.tsx` only if regression coverage is incomplete
- Modify: `docs/superpowers/specs/2026-09-24-issue-182-config-ux-design.md` only to record implementation status after verification

- [x] **Step 1: Add or update the backend response compatibility test**

Assert that `GET /api/admin/settings` includes the new metadata fields while existing configured values, revision handling, and pending restart fields remain unchanged.

- [x] **Step 2: Run the complete backend verification suite**

```bash
cargo fmt --check
cargo check --locked
cargo clippy --locked -- -D warnings
cargo test --locked
```

Expected: exit code 0; existing ignored benchmarks may remain ignored.

- [x] **Step 3: Run the complete frontend verification suite**

```bash
npm run lint
npm run build
npm test
```

Expected: TypeScript, production build, Vitest, and Node behavior tests all pass.

- [ ] **Step 4: Check the final diff and update the design status**

```bash
git diff --check
git status --short
```

Confirm that only Issue #182 implementation files and the committed design/plan are included. Update the spec status to say the first-stage configuration UX is implemented locally; keep the Auto/Manual and runtime-feedback deferrals explicit.

- [ ] **Step 5: Commit the final documentation status**

```bash
git add docs/superpowers/specs/2026-09-24-issue-182-config-ux-design.md backend/tests/settings.rs frontend/tests/admin-settings-ux.behavior.test.tsx
git commit -m "docs: mark issue 182 configuration UX complete"
```

- [ ] **Step 6: Push and open the independent pull request**

```bash
git push -u origin fix/issue-182-config-ux
gh pr create --base main --head fix/issue-182-config-ux \
  --title "feat: organize administrator settings by audience" \
  --body "Partially addresses #182. This PR implements the first-stage configuration UX: common settings stay visible, advanced settings are collapsed, expert settings are explicitly revealed, and metadata guidance is rendered consistently. Auto/Manual modes, adaptive runtime tuning, and effective-value status presentation remain deferred to follow-up work."
```

After creating the PR, wait for the remote CI checks and report their result before claiming the PR is ready to merge.
