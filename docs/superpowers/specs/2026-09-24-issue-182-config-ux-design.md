# Issue #182 Configuration UX Design

## Status

Approved scope: first-stage configuration UX only. This design does not implement adaptive runtime tuning.

## Goal

Make the administrator settings page easier to understand by grouping settings by audience, showing useful guidance, and reducing the number of low-level controls visible by default.

## Current context

The backend already has one metadata record per supported setting. Metadata currently exposes the setting key, database column, environment variable, value type, unit, default, validation bounds, description, and whether the change is hot-applied or restart-required. The settings endpoint also returns configured and effective snapshots.

The current frontend keeps a small set of legacy controls as dedicated sections and renders every remaining metadata field in one advanced accordion. This means resource controls, security controls, and internal tuning controls have the same visibility and presentation.

## Scope

### In scope

- Add audience and product category metadata to every supported setting.
- Add a recommended range to metadata where a recommendation is meaningful.
- Keep common product settings visible by default.
- Put resource and operational settings in a collapsed Advanced section.
- Hide expert/internal tuning settings behind an explicit control.
- Render guidance, units, apply mode, validation bounds, and recommended ranges consistently.
- Preserve the existing settings endpoint, optimistic revision checks, validation, audit behavior, and permission checks.
- Add backend metadata invariants and frontend behavior tests.

### Explicitly out of scope

- Auto / Manual setting modes.
- Automatic resource calculation or mutation of user configuration.
- A new effective-value calculation pipeline or runtime status dashboard.
- Worker counts, queue depth, index status, or resource telemetry.
- Remote configuration, multi-instance synchronization, or automatic service restart.
- Changes to archive-bomb protection, issue quotas, or any security-limit semantics.
- Database migrations.

## Design

### Backend metadata

Extend `FieldMetadata` with stable presentation metadata:

- `category`: `common`, `advanced`, or `expert`.
- `visibility`: `default`, `collapsed`, or `expert`.
- `recommended_min` and `recommended_max`, nullable when no meaningful recommendation exists.

The existing `min` and `max` remain hard validation bounds. Recommended bounds are advisory only and must never change validation behavior.

Every `SettingKey` must have exactly one metadata entry and a complete category/visibility assignment. The serialized settings response includes the new fields so the frontend does not duplicate the classification table.

Suggested classification:

| Group | Settings | Default presentation |
| --- | --- | --- |
| Common | registration, session policy, authentication thresholds, Issue size/inactivity/cleanup policy, default search result size | Visible |
| Advanced | archive working budget, upload concurrency/temp budget, preview/page/search limits, temporary-result retention and scan limits | Collapsed |
| Expert | Argon2 concurrency, indexing line-size budget, Tantivy writers/heap, line-read concurrency, temporary-result materialization concurrency | Hidden until explicitly enabled |

The final assignment is made in the metadata table and is covered by an invariant test; no frontend-side key list is authoritative.

### Frontend settings page

Keep the existing dedicated common sections for their tailored controls, but use backend metadata to render the remaining fields into three presentation groups:

1. Common settings remain on the main page and receive the same description/range treatment.
2. Advanced settings render inside a collapsed `<details>` section.
3. Expert settings are not rendered initially. A clearly labeled “显示专家配置” control reveals them in a separate section without changing their values.

Each metadata-driven field shows:

- human-readable description;
- unit, when present;
- current configured value;
- hard validation range, when present;
- recommended range, when present;
- “即时生效” or “重启生效”.

The page keeps one save action per metadata group. Existing legacy save actions remain behaviorally compatible while the shared advanced/expert save path continues to send one revision-checked changes object.

The UI must not imply that recommended values are enforced or that an expert setting is unsafe merely because it is hidden. Copy should explain that advanced/expert values affect resource usage and are normally left unchanged.

### Data flow and compatibility

1. Admin requests `GET /api/admin/settings`.
2. Backend returns the existing snapshot plus serialized presentation metadata.
3. Frontend groups fields by metadata and renders the appropriate sections.
4. Admin edits a group and sends the existing `PATCH /api/admin/settings` revision/check payload.
5. Backend validates the same hard bounds and applies the same hot/restart behavior as today.
6. The response refreshes the revision, configured values, and pending restart fields.

No new endpoint or database column is required. Existing clients that ignore unknown metadata fields continue to work.

## Error handling

- Invalid values continue to be rejected by existing backend validation.
- A stale revision continues to trigger the existing reload/error path.
- If metadata loading fails, the page keeps its existing error state and does not render editable controls as loaded.
- Recommended-range text is never used as a client-side substitute for backend validation.

## Testing

### Backend

- Assert every `SettingKey` has exactly one metadata record.
- Assert every metadata record has a valid category and visibility.
- Assert recommended ranges are ordered and, when present, lie within hard validation bounds.
- Assert the serialized metadata includes category, visibility, recommended bounds, and the existing apply-mode fields.

### Frontend

- Common fields render without opening an accordion.
- Advanced fields are present but collapsed initially.
- Expert fields are absent until the explicit reveal control is used.
- Descriptions, units, recommended ranges, and apply-mode labels render from metadata.
- Saving advanced/expert fields continues to use the revision and preserves existing error handling.

## Acceptance mapping

- Administrator default page shows high-frequency settings: common group and existing dedicated sections.
- Advanced resource parameters are collapsed by default: advanced group.
- Expert parameters are hidden behind a separate entry: expert reveal control.
- Settings include descriptions and recommended ranges: metadata-driven field presentation.
- Auto / Manual mode: explicitly deferred to a later issue/PR.
- Configured/effective split: existing API remains intact, but visual effective-value presentation is deferred with adaptive tuning.
- Security limits remain unchanged: no validation or runtime limit semantics are modified.
- Categories and UI behavior are covered by backend and frontend tests.
