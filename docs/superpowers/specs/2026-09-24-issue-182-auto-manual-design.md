# Issue #182 Auto/Manual and Effective Settings Design

## Status

Approved for implementation as the second stage of Issue #182. This stage adds a persisted Auto/Manual policy and clearer configured/effective presentation without hardware-aware adaptation or live runtime reconfiguration.

## Goal

Let administrators distinguish resource settings that use a fixed backend recommendation from explicit manual overrides, while showing the value currently configured and the value currently active in the process. Security-sensitive Argon2 concurrency must no longer be an administrator-editable setting.

## Scope

### In scope

- Persist Auto/Manual mode for selected resource-concurrency settings.
- Keep existing numeric setting values and revision-checked PATCH compatibility.
- Use fixed, backend-owned Auto values; do not inspect CPU, memory, storage, or index size in this stage.
- Return resource modes and Auto values alongside the existing configured/effective snapshots.
- Render Auto/Manual controls and configured/effective values in the administrator settings page.
- Preserve restart-required behavior: changing a restart-required resource setting updates configured state but does not rebuild the current runtime resource.
- Remove Argon2 concurrency from administrator-editable metadata and values in the admin API.
- Reject attempts to update protected Argon2 concurrency with a stable public API error.
- Expose only a safe Argon2 protection status, such as `argon2id_enabled`, to the admin page.
- Add database, API, backend service, and frontend behavior coverage.

### Explicitly out of scope

- CPU/memory/storage/index-size-aware recommendation calculation.
- Live semaphore, worker, writer, or queue reconfiguration.
- Worker counts, queue depth, index status, and resource telemetry.
- Automatic service restart.
- Changes to hard validation limits or security-limit semantics.
- Removing the legacy Argon2 database column immediately; it remains readable for runtime and migration compatibility.

## Data model

### Resource modes

Add a nullable-safe `resource_modes_json` column to `system_settings` through the next SQLx migration. The stored object is keyed only by the allowlisted resource settings and contains `auto` or `manual`. Missing keys resolve to `manual`, so existing databases preserve current behavior.

The initial Auto-capable allowlist is:

- `upload_concurrent_processing_tasks`;
- `upload_concurrent_receive_tasks`;
- `search_tantivy_max_writers`;
- `api_concurrent_line_reads`;
- `temp_results_concurrent_materializations`.

The existing numeric columns remain the canonical configured values. Switching to Auto writes the field's fixed backend-owned `auto_value` into that numeric setting and persists the mode atomically. Switching to Manual persists the submitted numeric value and mode together. This keeps the existing `configured` and `effective` snapshots backward-compatible while giving the UI an explicit mode map.

Metadata for Auto-capable fields adds:

- `supports_auto: true`;
- `auto_value`, validated against the existing hard bounds;
- the existing `apply_mode`, recommendation, and description fields.

All other admin-visible settings remain Manual-only.

### Protected Argon2 setting

`argon2_concurrency` remains in the internal `SettingsValues` model and existing database schema so already-initialized deployments continue to start safely. It is removed from the admin-visible metadata response and from the public `configured`/`effective` maps returned by the admin settings endpoint.

The settings service rejects `argon2_concurrency` in an admin PATCH with `SETTINGS_PROTECTED_FIELD`. Existing environment/bootstrap and runtime loading paths remain unchanged in this stage; the value is no longer exposed as an administrator tuning control.

The admin response includes a non-sensitive protection status:

```json
{
  "security": {
    "argon2id_enabled": true
  }
}
```

The metadata invariant tests continue to cover every internal `SettingKey`, while a separate admin-exposure assertion verifies that protected keys are absent from the public field list.

## API and persistence flow

1. `GET /api/admin/settings` loads the current snapshot and resource mode map.
2. The response keeps `configured`, `effective`, `revision`, and `pending_restart_fields`; it adds `resource_modes`, `auto_values`, and the safe security status.
3. A new client PATCH payload may include:

   ```json
   {
     "expected_revision": "7",
     "changes": {
       "upload_concurrent_processing_tasks": 6
     },
     "resource_modes": {
       "upload_concurrent_processing_tasks": "manual"
     }
   }
   ```

4. The backend validates the mode keys, the mode values, and the resulting complete candidate settings before writing the numeric values and JSON mode map in one transaction.
5. Mode changes are included in the revision comparison and audit details even when the numeric value is unchanged.
6. Auto mode uses the fixed metadata `auto_value`; it never mutates the live runtime resource in this stage.
7. The response returns the next revision, configured/effective snapshots, mode map, Auto values, and pending restart fields.

Existing flat PATCH requests remain supported for legacy controls. Requests that use the new `changes` payload must continue to provide an explicit revision.

## Frontend behavior

For Auto-capable fields:

- show an Auto/Manual selector;
- show `Auto value` when Auto is selected;
- disable manual numeric input in Auto mode;
- show the configured numeric value and the effective runtime value separately;
- show the existing restart-required warning when the values differ because the process has not restarted.

For Manual mode, the numeric input remains editable and the submitted value is sent with the selected mode. Existing common, advanced, and expert grouping remains unchanged. Argon2 is not rendered in those groups; the page displays the safe security status instead.

Recommended ranges remain advisory text. The frontend does not duplicate backend validation or calculate Auto values.

## Error handling and compatibility

- Unknown resource mode keys, invalid mode strings, Auto on unsupported settings, and invalid numeric values return the existing settings validation family of errors.
- Updating a protected setting returns `SETTINGS_PROTECTED_FIELD` without changing the revision or audit state.
- Stale revisions continue to return `SETTINGS_REVISION_CONFLICT`.
- Missing `resource_modes_json` or malformed legacy mode data resolves safely to Manual only when the data is absent; malformed persisted data fails startup/settings loading with a configuration error rather than silently changing a policy.
- Existing configured/effective consumers can ignore the additional response properties.
- No security hard bounds, upload quotas, archive limits, or authorization behavior change.

## Testing

### Backend

- Migration upgrades an existing settings row and defaults all modes to Manual.
- Mode parsing accepts only the allowlisted fields and `auto`/`manual` values.
- Auto mode writes the fixed value atomically and preserves effective restart semantics.
- Manual mode preserves existing numeric save behavior.
- Mode-only changes advance revision and produce audit details.
- Stale revisions, invalid values, unsupported Auto fields, and protected Argon2 changes are rejected without partial writes.
- Admin responses include `resource_modes`, `auto_values`, and safe security status while omitting Argon2 from public fields and value maps.
- Existing settings, admin API, and full regression tests remain green.

### Frontend

- Auto-capable fields render the mode selector and effective-value comparison.
- Auto disables manual input and submits the selected mode.
- Manual enables numeric editing and submits mode plus value.
- Restart-required differences remain visible after a save.
- Argon2 is absent from editable settings and the security status is shown.
- Existing admin guard and settings UX tests remain green.

## Acceptance mapping

- Auto / Manual distinction: persisted resource mode map and UI selector.
- Configured/effective split: existing snapshots plus explicit per-field presentation.
- Security protection: Argon2 no longer administrator-editable or publicly exposed as a numeric setting.
- Compatibility: old flat PATCH calls and numeric configured/effective maps remain supported.
- Runtime safety: no live resource mutation, automatic restart, or hard-limit change in this stage.
