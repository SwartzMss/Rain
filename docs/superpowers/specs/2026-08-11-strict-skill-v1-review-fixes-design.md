# Strict SKILL.md v1 Review Fixes Design

## Context

Rain is still an internal pre-release build. Existing free-form Skill records and historical review rows do not need a compatibility or migration path; development databases may be rebuilt when the schema contract changes.

PR #94 introduces deterministic `SKILL.md` v1 parsing and separates structure validation from AI quality review. Review found five correctness issues: the review request duplicated standard section content, stored reviews were read without checking their rubric version, non-whitespace content between Front Matter and the first H1 was visible to the Runner but absent from the reviewer input, serializing every parsed section as a JSON object allowed a near-limit Skill containing thousands of short custom H1 sections to amplify into a much larger model request, and the summary list fetched and parsed every complete Skill only to derive the constant v1 schema version. The PR also contained legacy-Skill UI and API behavior that is unnecessary under the pre-release constraint.

## Goals

- Make a valid `SKILL.md` v1 document the only supported stored Skill representation.
- Send every Skill section body to the AI reviewer exactly once.
- Ensure only reviews produced by the current rubric are returned.
- Parse complete Markdown on admission and content-consuming read paths; keep summary reads independent of document size and section count.
- Remove legacy migration messaging and nullable schema behavior.
- Ensure the Runner cannot consume Skill instructions that the reviewer did not receive.
- Bound reviewer input growth independently of the number of custom sections.

## Non-goals

- Migrating, displaying, editing, reviewing, or running old free-form Skills.
- Automatically converting old headings or Markdown into v1.
- Preserving development database records created before this contract.
- Changing the six scoring dimensions, weights, Runner permissions, or evidence policy.

## API and Storage Invariant

Every Skill accepted through create or update must pass `parse_skill_markdown`. `schema_version` in summary and detail responses is a required integer rather than an optional integer.

The summary list trusts that storage invariant. Its SQL must not select `skill_markdown`, and each summary returns the compile-time `SKILL_SCHEMA_VERSION`. This avoids transferring and parsing up to 50 complete 64-KiB documents when the caller only needs summary metadata.

Detail retrieval already returns the complete Markdown, so it continues to parse the record and derives `schema_version` from that parse. Review and Run also continue parsing before model work. If an invalid record is inserted by bypassing supported admission paths, list may return its metadata as a v1 summary, while detail, Review, and Run reject it with `SKILL_FORMAT_INVALID`. This is a trust-boundary consequence, not a legacy compatibility mode.

Review persistence will define one `CURRENT_SKILL_REVIEW_RUBRIC` constant. Saving a review writes that value, and list/detail joins only return reviews whose `rubric_version` equals it and whose Skill version and content hash still match. Changing the rubric constant in the future invalidates old scores without a data migration.

## Reviewer Input

After successful v1 parsing, the review request will send `body_markdown` exactly once after a fixed untrusted-content prefix. It will not serialize a JSON object per section. The system prompt will map the six exact Chinese H1 headings to the five section-specific scoring dimensions and use the complete Markdown for `clarity`.

Front Matter remains excluded. The deterministic parser still owns schema validation, required-section recognition, alias rejection, duplicate detection, and the body-preamble invariant before any model request is built. JSON was only a transport representation and did not create a stronger prompt-injection boundary; the system message continues to label the raw Markdown as untrusted content that must be assessed rather than followed.

The system prompt will explicitly state that H1-looking lines inside fenced code blocks are content or examples, not Skill section boundaries. This keeps the reviewer's section interpretation aligned with the deterministic parser.

The model request length must be `body_markdown.len()` plus one fixed prefix. A near-limit regression test will construct a valid Skill with thousands of short custom H1 sections and assert exact body preservation, one occurrence of a unique marker, no per-section JSON fields, and only constant request overhead. Rain will not add an arbitrary section-count limit because the raw Markdown representation already makes section count irrelevant to transport size.

## Body Preamble Invariant

After the Front Matter closing delimiter, `SKILL.md` v1 permits only whitespace before the first H1. Any other content in that range—including prose, comments, lists, or fenced blocks—is rejected by `parse_skill_markdown` and therefore surfaces through API admission paths as `SKILL_FORMAT_INVALID`.

Blank lines remain valid with both LF and CRLF input. The parser continues preserving the post-Front-Matter Markdown in `body_markdown`; because the newly constrained prefix is semantically empty, the Runner and reviewer now consume the same instruction-bearing content without reconstructing or normalizing Markdown.

## Frontend and Documentation

The frontend type will require `schema_version: number`. The Skill editor and detail view will always render the returned version directly. The run selector will filter only by `enabled`; it will not count, hide, or explain legacy Skills because no such state is supported.

Legacy migration labels, behavior tests, and the historical-Skill section in `doc/SKILL_SCHEMA.md` will be removed. The schema documentation will state that pre-release format changes require rebuilding development data.

## Testing

Implementation follows red-green-refactor cycles:

1. Add reviewer request tests proving the raw post-Front-Matter Markdown is sent exactly once and no section JSON is produced.
2. Add repository/API tests proving mismatched rubric reviews are omitted while the current rubric is returned.
3. Update response and frontend tests to require a non-null schema version and remove migration behavior.
4. Add parser and API regression tests proving a whitespace-only body prefix is accepted and a non-whitespace preamble is rejected.
5. Add a near-64-KiB, thousands-of-short-H1 regression test proving reviewer input grows only by a fixed prefix.
6. Add a repository regression proving list returns the v1 constant without parsing an invalid direct-insert record, while detail still rejects that record.
7. Add a prompt-contract assertion for fenced-code H1 semantics.
8. Run targeted backend and frontend tests after each change, then the complete Rust and frontend verification suites, formatting, lint, build, and `git diff --check`.

## GitHub Scope

The five identified PR #94 correctness issues and the fenced-code prompt-alignment suggestion are addressed locally. This work does not reply to or resolve GitHub threads unless the user separately authorizes those GitHub write actions.
