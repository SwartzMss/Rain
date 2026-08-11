# Strict SKILL.md v1 Review Fixes Design

## Context

Rain is still an internal pre-release build. Existing free-form Skill records and historical review rows do not need a compatibility or migration path; development databases may be rebuilt when the schema contract changes.

PR #94 introduces deterministic `SKILL.md` v1 parsing and separates structure validation from AI quality review. Review found two correctness issues: the review request duplicates standard section content, and stored reviews are read without checking their rubric version. The PR also contains legacy-Skill UI and API behavior that is unnecessary under the pre-release constraint.

## Goals

- Make a valid `SKILL.md` v1 document the only supported stored Skill representation.
- Send every Skill section body to the AI reviewer exactly once.
- Ensure only reviews produced by the current rubric are returned.
- Use one complete v1 parse as the authoritative meaning of `schema_version` everywhere.
- Remove legacy migration messaging and nullable schema behavior.

## Non-goals

- Migrating, displaying, editing, reviewing, or running old free-form Skills.
- Automatically converting old headings or Markdown into v1.
- Preserving development database records created before this contract.
- Changing the six scoring dimensions, weights, Runner permissions, or evidence policy.

## API and Storage Invariant

Every Skill accepted through create or update must pass `parse_skill_markdown`. Read paths will enforce the same invariant. `schema_version` in summary and detail responses becomes a required integer rather than an optional integer.

If an invalid Skill somehow exists in the database, list or detail retrieval fails with the existing stable `SKILL_FORMAT_INVALID` contract. Rain will not reinterpret the record as a legacy Skill or expose a migration state.

Review persistence will define one `CURRENT_SKILL_REVIEW_RUBRIC` constant. Saving a review writes that value, and list/detail joins only return reviews whose `rubric_version` equals it and whose Skill version and content hash still match. Changing the rubric constant in the future invalidates old scores without a data migration.

## Reviewer Input

The parser already returns an ordered `sections` collection containing each heading, body, and optional standard-section key. The review request will serialize that collection once, with enough metadata to map standard sections to the five content dimensions and to let `clarity` evaluate all standard and custom sections.

The request will no longer include both `standard_sections` and the complete `body_markdown`. Front matter remains excluded. A near-limit regression test will assert that a large section marker appears once and that the serialized user input remains close to the original document size rather than approximately doubling it.

## Frontend and Documentation

The frontend type will require `schema_version: number`. The Skill editor and detail view will always render the returned version directly. The run selector will filter only by `enabled`; it will not count, hide, or explain legacy Skills because no such state is supported.

Legacy migration labels, behavior tests, and the historical-Skill section in `doc/SKILL_SCHEMA.md` will be removed. The schema documentation will state that pre-release format changes require rebuilding development data.

## Testing

Implementation follows red-green-refactor cycles:

1. Add reviewer request tests proving sections are serialized once and near-limit input does not duplicate content.
2. Add repository/API tests proving mismatched rubric reviews are omitted while the current rubric is returned.
3. Update response and frontend tests to require a non-null schema version and remove migration behavior.
4. Run targeted backend and frontend tests after each change, then the complete Rust and frontend verification suites, formatting, lint, build, and `git diff --check`.

## GitHub Scope

Both currently unresolved PR #94 review threads are addressed locally. This work does not reply to or resolve GitHub threads unless the user separately authorizes those GitHub write actions.
