# Issue #182 Auto/Manual and Effective Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Add persisted Auto/Manual policies and configured/effective presentation for selected resource settings while protecting Argon2 concurrency from administrator editing.

**Architecture:** Keep the existing strongly typed SettingsValues, revision checks, numeric columns, and runtime startup behavior. Add a small resource_modes_json policy map to system_settings; Auto mode writes a backend-owned fixed value into the existing numeric setting, while the current effective snapshot continues to represent what the running process uses. Keep Argon2 in the internal model for compatibility, but filter it from the admin response and reject attempts to patch it.

**Tech Stack:** Rust, Actix Web, SQLx/SQLite migrations, React, TypeScript, Vitest, existing admin settings API.

---

### Task 1: Extend the settings model and metadata contract

**Files:**
- Modify: backend/src/settings/model.rs
- Modify: backend/src/settings/metadata.rs
- Modify: backend/src/settings/mod.rs
- Test: backend/tests/settings.rs

- [x] Step 1: Write failing model and metadata tests

Add tests before production changes:

~~~rust
#[test]
fn metadata_declares_auto_values_and_protected_settings() {
    let processing = metadata::all()
        .iter()
        .find(|field| field.key == SettingKey::UploadConcurrentProcessingTasks)
        .expect("processing metadata");
    assert!(processing.supports_auto);
    assert_eq!(processing.auto_value, Some(4));

    let argon2 = metadata::all()
        .iter()
        .find(|field| field.key == SettingKey::Argon2Concurrency)
        .expect("argon2 metadata");
    assert!(argon2.protected);
    assert!(!metadata::admin().iter().any(|field| field.key == SettingKey::Argon2Concurrency));
}

#[test]
fn resource_mode_serializes_stably() {
    assert_eq!(serde_json::to_value(ResourceMode::Auto).unwrap(), "auto");
    assert_eq!(serde_json::to_value(ResourceMode::Manual).unwrap(), "manual");
}
~~~

The tests must initially fail because ResourceMode, the metadata fields, and metadata::admin do not exist.

- [x] Step 2: Run the focused tests and confirm the expected failure

Run from backend/:

~~~bash
cargo test --locked --test settings metadata_declares_auto_values_and_protected_settings
cargo test --locked --test settings resource_mode_serializes_stably
~~~

Expected: compilation failure for the missing symbols.

- [x] Step 3: Add the resource mode and metadata fields

In backend/src/settings/model.rs add:

~~~rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceMode {
    Auto,
    Manual,
}

pub type ResourceModes = std::collections::BTreeMap<String, ResourceMode>;
~~~

Add resource_modes: ResourceModes to SettingsSnapshot and re-export the new types from settings/mod.rs. Preserve SettingsValues and its Argon2 field unchanged.

In metadata.rs extend FieldMetadata with supports_auto, auto_value: Option<u64>, and protected. Set supports_auto=true and fixed values from the spec for the five allowlisted resource fields; set protected=true only for Argon2Concurrency. Include the properties in serialization and add:

~~~rust
pub fn admin() -> Vec<&'static FieldMetadata> {
    all().iter().filter(|field| !field.protected).collect()
}
~~~

Keep all() as the exhaustive internal table so SettingKey::ALL coverage remains complete. Do not remove Argon2 from runtime construction.

- [x] Step 4: Run the focused tests and formatting

~~~bash
cargo fmt --check
cargo test --locked --test settings metadata_declares_auto_values_and_protected_settings
cargo test --locked --test settings resource_mode_serializes_stably
~~~

Expected: formatting passes and both tests pass.

- [x] Step 5: Commit the model and metadata contract

~~~bash
git add backend/src/settings/model.rs backend/src/settings/metadata.rs backend/src/settings/mod.rs backend/tests/settings.rs
git commit -m "feat: add resource setting modes and protected metadata"
~~~

### Task 2: Persist and validate resource modes

**Files:**
- Create: backend/migrations/0011_resource_setting_modes.sql
- Modify: backend/src/settings/service.rs
- Modify: backend/src/settings/model.rs
- Test: backend/tests/settings.rs

- [x] Step 1: Write failing persistence tests

Add tests for default Manual behavior, Auto fixed-value behavior, and rejected policies:

~~~rust
#[tokio::test]
async fn resource_modes_default_to_manual_and_round_trip() {
    let pool = pool();
    db::prepare_schema(&pool, true).await.expect("schema");
    let service = SettingsService::new(pool.clone());
    let initial = service
        .initialize(&AppLimits::default(), &AuthConfig::default(), 0, None)
        .await
        .expect("initialize");

    assert_eq!(
        initial.resource_modes["upload_concurrent_processing_tasks"],
        ResourceMode::Manual
    );

    let mut modes = ResourceModes::new();
    modes.insert(
        "upload_concurrent_processing_tasks".into(),
        ResourceMode::Auto,
    );
    let saved = service
        .save_with_modes(initial.revision, &serde_json::Map::new(), &modes, None)
        .await
        .expect("save mode");

    assert_eq!(
        saved.snapshot.resource_modes["upload_concurrent_processing_tasks"],
        ResourceMode::Auto
    );
    assert_eq!(saved.snapshot.configured.upload_concurrent_processing_tasks, 4);
}

#[tokio::test]
async fn unsupported_resource_modes_are_rejected_without_writes() {
    let pool = pool();
    db::prepare_schema(&pool, true).await.expect("schema");
    let service = SettingsService::new(pool.clone());
    let initial = service
        .initialize(&AppLimits::default(), &AuthConfig::default(), 0, None)
        .await
        .expect("initialize");

    let mut modes = ResourceModes::new();
    modes.insert("api_max_search_window".into(), ResourceMode::Auto);
    let error = service
        .save_with_modes(initial.revision, &serde_json::Map::new(), &modes, None)
        .await
        .expect_err("unsupported Auto field");

    assert!(matches!(
        error,
        backend::error::AppError::PublicApi {
            code: "SETTINGS_INVALID_REQUEST",
            ..
        }
    ));
    assert_eq!(service.snapshot().await.revision, initial.revision);
}
~~~

The tests must fail before migration and service support exist.

- [x] Step 2: Add the SQL migration

Create backend/migrations/0011_resource_setting_modes.sql:

~~~sql
ALTER TABLE system_settings
ADD COLUMN resource_modes_json TEXT NOT NULL DEFAULT '{}';
~~~

The empty JSON object makes existing rows resolve every supported resource setting to Manual. Do not add a new table or change numeric setting columns.

- [x] Step 3: Implement mode loading, normalization, and validation

In settings/service.rs:

- Define one allowlist helper containing the five Auto-capable keys.
- Load resource_modes_json with serde_json::from_str::<ResourceModes>.
- Fill missing allowlisted keys with ResourceMode::Manual.
- Reject keys outside the allowlist and invalid mode values with a configuration error.
- Include the normalized map in snapshots created by new_with_config, load, and initialize.
- Add save_with_modes while keeping save as a compatibility wrapper that passes an empty mode patch.

The save path merges a partial mode patch with the current normalized map. For every requested Auto mode, find the metadata auto_value, insert that numeric value into the candidate settings object, and validate the complete candidate with SettingsValues::validate. For Manual mode, preserve the submitted numeric change or the current numeric value if no value was supplied. Treat mode changes as changes for revision and audit purposes even when the numeric value is unchanged.

Update the existing transaction to write resource_modes_json alongside numeric fields. Return the new snapshot while retaining current hot/restart effective merge logic.

- [x] Step 4: Run focused persistence tests

~~~bash
cargo fmt --check
cargo test --locked --test settings resource_modes
~~~

Expected: migration, default Manual behavior, Auto fixed-value behavior, and invalid mode rejection pass.

- [ ] Step 5: Commit mode persistence

~~~bash
git add backend/migrations/0011_resource_setting_modes.sql backend/src/settings/model.rs backend/src/settings/service.rs backend/tests/settings.rs
git commit -m "feat: persist resource setting modes"
~~~

### Task 3: Protect Argon2 and expose the new admin API contract

**Files:**
- Modify: backend/src/models/admin.rs
- Modify: backend/src/routes/admin.rs
- Modify: backend/src/settings/service.rs
- Test: backend/tests/admin.rs
- Test: backend/tests/settings.rs

- [x] Step 1: Write failing API and protection tests

Extend the existing admin settings integration test after the initial GET:

~~~rust
let fields = body["fields"].as_array().expect("public fields");
assert!(!fields.iter().any(|field| field["key"] == "argon2_concurrency"));
assert_eq!(body["security"]["argon2id_enabled"], true);
assert_eq!(
    body["resource_modes"]["upload_concurrent_processing_tasks"],
    "manual"
);
assert_eq!(body["auto_values"]["upload_concurrent_processing_tasks"], 4);
~~~

Add a PATCH with the current revision and changes containing argon2_concurrency. Assert HTTP 422, code SETTINGS_PROTECTED_FIELD, unchanged revision, and no audit update. Add a mode-only Auto PATCH and assert mode auto, configured value 4, and a pending restart entry for the restart-required field.

- [x] Step 2: Run the API test and observe the expected failure

~~~bash
cargo test --locked --test admin registration_settings_are_persistent_and_admin_only
~~~

Expected: failure because the response has no security, resource_modes, or auto_values and protected-field handling is absent.

- [x] Step 3: Extend the request type and route response

In backend/src/models/admin.rs add:

~~~rust
pub resource_modes: Option<std::collections::BTreeMap<String, ResourceMode>>,
~~~

In routes/admin.rs:

- pass resource_modes.as_ref() to the settings service save method;
- keep the old flat request path unchanged;
- serialize public configured/effective maps after removing argon2_concurrency;
- serialize only metadata::admin() into fields;
- add resource_modes, auto_values from Auto-capable metadata, and security.argon2id_enabled=true;
- continue returning revision, pending restart fields, and all legacy flat properties.

Reject a protected key before persistence with:

~~~rust
AppError::public(
    StatusCode::UNPROCESSABLE_ENTITY,
    "SETTINGS_PROTECTED_FIELD",
    "该配置项由系统安全策略管理，管理员不可修改",
)
~~~

Reject malformed mode combinations using SETTINGS_INVALID_REQUEST. Do not update runtime resources for restart-required fields and preserve existing hot authentication/cleanup updates.

- [x] Step 4: Run backend API and regression tests

~~~bash
cargo fmt --check
cargo test --locked --test admin registration_settings_are_persistent_and_admin_only
cargo test --locked --test settings
~~~

Expected: selected tests pass, including legacy flat PATCH compatibility and new protection/mode assertions.

- [ ] Step 5: Commit API and security behavior

~~~bash
git add backend/src/models/admin.rs backend/src/routes/admin.rs backend/src/settings/service.rs backend/tests/admin.rs backend/tests/settings.rs
git commit -m "feat: expose resource modes and protect argon2 settings"
~~~

### Task 4: Add frontend mode and effective-value helpers

**Files:**
- Modify: frontend/src/api/types.ts
- Modify: frontend/src/api/client.ts
- Modify: frontend/src/features/admin/settingsFields.ts
- Test: frontend/tests/admin-settings-ux.behavior.test.tsx

- [x] Step 1: Write failing helper tests

Add:

~~~ts
it('formats configured and effective values separately', () => {
  expect(effectiveSettingLabel(4, 2, true)).toBe('已配置 4；当前生效 2（待重启）');
  expect(effectiveSettingLabel(4, 4, false)).toBe('已配置 4；当前生效 4');
});

it('serializes resource mode patches without changing legacy values', () => {
  expect(serializeResourceModePatch('upload_concurrent_processing_tasks', 'auto'))
    .toEqual({ upload_concurrent_processing_tasks: 'auto' });
});
~~~

Run and confirm failure:

~~~bash
cd frontend
npx vitest run tests/admin-settings-ux.behavior.test.tsx
~~~

- [x] Step 2: Extend frontend API types

In frontend/src/api/types.ts add ResourceMode as auto/manual and AdminSecurityStatus with argon2id_enabled. Add supports_auto, auto_value, resource_modes, auto_values, and security to the existing API types. Update the v2 client PATCH helper to accept an optional resource_modes object and include it only for the new payload.

- [x] Step 3: Implement pure display and payload helpers

In settingsFields.ts add:

~~~ts
export function effectiveSettingLabel(
  configured: unknown,
  effective: unknown,
  pendingRestart: boolean,
): string {
  const suffix = pendingRestart ? '（待重启）' : '';
  return '已配置 ' + String(configured) + '；当前生效 ' + String(effective) + suffix;
}

export function serializeResourceModePatch(
  key: string,
  mode: ResourceMode,
): Record<string, ResourceMode> {
  return { [key]: mode };
}
~~~

Keep numeric conversion in serializeSettingValue. Do not calculate Auto values in TypeScript.

- [x] Step 4: Run focused frontend helper tests

~~~bash
cd frontend
npx vitest run tests/admin-settings-ux.behavior.test.tsx
npm run lint
~~~

Expected: helper and existing settings UX tests pass with no TypeScript errors.

- [ ] Step 5: Commit frontend contract helpers

~~~bash
git add frontend/src/api/types.ts frontend/src/api/client.ts frontend/src/features/admin/settingsFields.ts frontend/tests/admin-settings-ux.behavior.test.tsx
git commit -m "feat: add resource mode frontend contract"
~~~

### Task 5: Render and save Auto/Manual settings in the admin page

**Files:**
- Modify: frontend/src/features/admin/AdminPage.tsx
- Modify: frontend/src/features/admin/settingsFields.ts
- Test: frontend/tests/admin-settings-ux.behavior.test.tsx

- [x] Step 1: Add failing rendered behavior tests

Extend the settings fixture with resource_modes, auto_values, effective, and security. Assert:

~~~ts
expect(screen.getByLabelText('upload_concurrent_processing_tasks mode')).toHaveValue('auto');
expect(screen.getByText('已配置 4；当前生效 2（待重启）')).toBeInTheDocument();
expect(screen.getByLabelText('upload_concurrent_processing_tasks')).toBeDisabled();
expect(screen.getByText('Argon2id 已启用')).toBeInTheDocument();
~~~

Add a Manual assertion that selecting Manual enables numeric input and saving sends both numeric changes and resource_modes to updateAdminSettingsV2.

- [x] Step 2: Run rendered tests and confirm failure

~~~bash
cd frontend
npx vitest run tests/admin-settings-ux.behavior.test.tsx
~~~

Expected: failure because the metadata grid has no mode selector, effective-value label, or security status.

- [x] Step 3: Implement mode-aware metadata rendering

Update AdminSettingsPage state to retain resourceModes, autoValues, effectiveValues, and securityStatus from GET. Extend MetadataSettingsGrid with the mode map and effective values.

For field.supports_auto:

- render a labeled select with auto and manual options;
- initialize from resourceModes[field.key] or manual;
- in Auto, show autoValues[field.key], disable numeric input, and synchronize the draft numeric value to the Auto value;
- in Manual, enable numeric editing;
- render effectiveSettingLabel using the draft value, effective value, and pending restart membership.

When saving a metadata group, pass changed modes as the third v2 payload property. Preserve current group save/revision/error/pending-restart behavior. Protected fields are absent from fields.

Add a compact security panel near the settings header that renders Argon2id 已启用 only when security.argon2id_enabled is true.

- [x] Step 4: Run focused frontend and admin guard tests

~~~bash
cd frontend
npx vitest run tests/admin-settings-ux.behavior.test.tsx tests/admin-guard.behavior.test.tsx
~~~

Expected: all mode, effective-value, protection-status, and admin guard tests pass.

- [x] Step 5: Commit the settings UI

~~~bash
git add frontend/src/features/admin/AdminPage.tsx frontend/src/features/admin/settingsFields.ts frontend/tests/admin-settings-ux.behavior.test.tsx
git commit -m "feat: add auto manual settings controls"
~~~

### Task 6: Verify migration, compatibility, and full regression coverage

**Files:**
- Modify: docs/configuration.md
- Modify: docs/superpowers/plans/2026-09-24-issue-182-auto-manual.md
- Test: all existing backend and frontend suites

- [x] Step 1: Document the operator contract

Update docs/configuration.md to explain that resource modes default to Manual, Auto uses fixed backend-owned values, configured/effective may differ until restart, Argon2 concurrency is protected, and CPU/memory-aware adaptation and live runtime tuning are not enabled yet.

- [x] Step 2: Run complete backend verification

From backend/:

~~~bash
cargo fmt --check
cargo check --locked
cargo clippy --locked -- -D warnings
cargo test --locked --quiet
~~~

Expected: all tests pass; ignored benchmarks remain ignored.

- [x] Step 3: Run complete frontend verification

From frontend/:

~~~bash
npm run lint
npm run build
npm test
~~~

Expected: TypeScript, production build, Vitest, and Node behavior tests pass.

- [x] Step 4: Review final diff and plan coverage

Run:

~~~bash
git diff --check
git status --short
git diff --stat origin/main..HEAD
~~~

Confirm only the second-stage plan/spec, migration, settings service/API changes, frontend mode UI, tests, and configuration documentation are present. Confirm no live semaphore mutation, telemetry, automatic restart, or hard-limit changes slipped into the diff.

- [x] Step 5: Commit documentation and plan status

~~~bash
git add docs/configuration.md docs/superpowers/plans/2026-09-24-issue-182-auto-manual.md
git commit -m "docs: document issue 182 resource modes"
~~~

- [x] Step 6: Push and create the independent PR

~~~bash
git push -u origin fix/issue-182-auto-manual
gh pr create --base main --head fix/issue-182-auto-manual \
  --title "feat: add auto manual resource settings" \
  --body "Partially addresses #182. Adds persisted Auto/Manual resource policies, configured/effective value presentation, fixed backend Auto values, and protected Argon2 admin settings. Hardware-aware adaptation, live runtime reconfiguration, telemetry, and automatic restart remain deferred."
~~~

After opening the PR, wait for the remote Build and Test check and report its result before claiming the PR is ready for review.
