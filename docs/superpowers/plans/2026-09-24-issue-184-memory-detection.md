# Issue 184 Memory Detection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make adaptive runtime memory detection reliable in cgroup v1/v2, WSL/ordinary Linux, and Windows while exposing the source and fallback reason without changing adaptive tuning.

**Architecture:** Keep `ResourceSnapshot`, `RuntimePlan`, and `resolve` in `backend/src/runtime_adaptive.rs`. Add pure Linux cgroup/meminfo parsers driven by virtual file contents, then let the production probe provide the real `/proc` and cgroup files; use `GlobalMemoryStatusEx` only on Windows. Extend the existing serialized resource snapshot and render the new source/reason in the administrator resource card.

**Tech Stack:** Rust 2024, serde, `windows-sys` 0.59, Linux `/proc` and cgroup files, React/TypeScript, Vitest/Testing Library.

---

## File map

- Modify `backend/Cargo.toml`: enable the Windows system-information API feature.
- Modify `backend/src/runtime_adaptive.rs`: resource source model, fallback reason, pure meminfo/cgroup parsers, Linux probe order, Windows probe, and unit tests.
- Modify `frontend/src/api/types.ts`: add `proc_meminfo` and `memory_fallback_reason` to the runtime contract.
- Modify `frontend/src/features/admin/AdminPage.tsx`: render source and fallback reason as separate memory metadata.
- Modify `frontend/tests/admin-settings-ux.behavior.test.tsx`: lock the new source/reason display behavior.

## Task 1: Lock down Linux memory parsing with failing backend tests

**Files:**
- Modify: `backend/src/runtime_adaptive.rs` test module

- [ ] **Step 1: Add a `/proc/meminfo` parser test**

Add a unit test named `parses_memtotal_as_bytes` that calls the planned pure helper `parse_linux_meminfo` with:

```text
MemTotal:       24511652 kB
MemFree:         4289624 kB
```

and asserts `Some(24511652 * 1024)`. Add a second assertion that missing `MemTotal` returns `None`.

- [ ] **Step 2: Add invalid and overflow tests**

Add `rejects_invalid_meminfo_values` covering a non-numeric value, a non-`kB` unit, zero, and a value whose KiB-to-byte multiplication overflows. Each case must return `None`.

- [ ] **Step 3: Run the focused tests and verify RED**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml runtime_adaptive::tests::parses_memtotal_as_bytes
cargo test --manifest-path backend/Cargo.toml runtime_adaptive::tests::rejects_invalid_meminfo_values
```

Expected: both commands fail because `parse_linux_meminfo` does not yet exist.

## Task 2: Implement pure `/proc/meminfo` parsing

**Files:**
- Modify: `backend/src/runtime_adaptive.rs`

- [ ] **Step 1: Implement the smallest parser that passes the tests**

Under `cfg(target_os = "linux")`, add:

```rust
fn parse_linux_meminfo(contents: &str) -> Option<u64>
```

It must locate the `MemTotal:` line, parse the numeric field, require the `kB` unit, reject zero, and use `checked_mul(1024)`. Change `read_linux_meminfo` to read `/proc/meminfo` and delegate to this parser.

- [ ] **Step 2: Run the focused tests and verify GREEN**

Run the same focused cargo command from Task 1. Expected: all new parser tests pass.

- [ ] **Step 3: Commit the parser change**

```bash
git add backend/src/runtime_adaptive.rs
git commit -m "test: cover proc meminfo parsing"
```

## Task 3: Lock down cgroup v1/v2 path and limit resolution

**Files:**
- Modify: `backend/src/runtime_adaptive.rs` test module

- [ ] **Step 1: Add cgroup v2 current/ancestor tests**

Add tests that pass synthetic `/proc/self/cgroup`, `/proc/self/mountinfo`, and a virtual file map to a pure resolver:

```text
0::/user.slice/user-1000.slice/session.scope
```

with a cgroup2 mount at `/sys/fs/cgroup`, a current limit of `max`, and an ancestor `memory.max` of `64 GiB`; assert the result is `Some(64 * 1024 * 1024 * 1024)`. Add a second current limit of `2 GiB` and ancestor limit of `4 GiB`; assert the minimum `2 GiB` is selected.

- [ ] **Step 2: Add cgroup v1 and unlimited-value tests**

Add a synthetic v1 membership such as `5:memory:/docker/abc`, a cgroup mount whose controller list contains `memory`, and `memory.limit_in_bytes` at the current directory. Assert the finite value is returned. Add cases for `max`, zero, malformed input, and a v1 unlimited sentinel at or above `1 << 60`; assert they are ignored.

- [ ] **Step 3: Run the focused tests and verify RED**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml runtime_adaptive::tests::cgroup
```

Expected: compilation/test failure because the pure cgroup resolver and its types do not yet exist.

## Task 4: Implement cgroup mount discovery and limit resolution

**Files:**
- Modify: `backend/src/runtime_adaptive.rs`

- [ ] **Step 1: Add pure cgroup parsing types and helpers**

Under Linux-only code, define a private cgroup version/mount representation and helpers with these responsibilities:

```rust
fn parse_cgroup_memberships(contents: &str) -> Vec<CgroupMembership>
fn parse_cgroup_mounts(contents: &str) -> Vec<CgroupMount>
fn cgroup_memory_limit<F>(proc_cgroup: &str, mountinfo: &str, read_file: F) -> Option<u64>
where
    F: Fn(&std::path::Path) -> Option<String>
```

`parse_cgroup_mounts` must split mountinfo at ` - `, decode the mount root and mount point fields, identify `cgroup2`, and identify v1 `cgroup` mounts whose super-options include `memory`. Membership selection must match unified v2 (`0::...`) or v1 memory-controller entries.

- [ ] **Step 2: Resolve current and ancestor directories**

For each matching mount, ensure the membership path is under the mount root, join it to the mount point, walk toward the mount point, and read `memory.max` for v2 or `memory.limit_in_bytes` for v1. Parse each value, ignore unlimited/invalid values, and return the smallest finite value across all visited levels and matching mounts.

- [ ] **Step 3: Add production wrapper and run tests**

Implement `read_linux_memory_limit` with `std::fs::read_to_string` closures for `/proc/self/cgroup`, `/proc/self/mountinfo`, and the discovered cgroup files. Run:

```bash
cargo test --manifest-path backend/Cargo.toml runtime_adaptive::tests::cgroup
```

Expected: all cgroup tests pass and no test reads the host cgroup filesystem.

- [ ] **Step 4: Commit the cgroup resolver**

```bash
git add backend/src/runtime_adaptive.rs
git commit -m "fix: resolve memory limits from process cgroups"
```

## Task 5: Integrate source metadata, fallback reason, and Windows API

**Files:**
- Modify: `backend/Cargo.toml`
- Modify: `backend/src/runtime_adaptive.rs`

- [ ] **Step 1: Add failing model/probe tests**

Add tests asserting:

- `ResourceSource::ProcMeminfo` serializes as `"proc_meminfo"`;
- a conservative snapshot contains `memory_fallback_reason == Some("memory_detection_unavailable")`;
- a successful Linux host-memory probe reports `ProcMeminfo` and a null reason;
- a probe with no memory sources retains 512 MiB, `Fallback`, the reason code, and `memory_probe_fallback`.

Use pure injected source helpers for deterministic tests; do not assert the host machine’s actual memory size.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml runtime_adaptive::tests::resource_source
cargo test --manifest-path backend/Cargo.toml runtime_adaptive::tests::memory_probe
```

Expected: both commands fail because the new enum variant, field, and source-selection helpers do not yet exist.

- [ ] **Step 3: Extend the resource model and probe order**

Add `ResourceSource::ProcMeminfo` and `memory_fallback_reason: Option<String>` to `ResourceSnapshot`. Keep existing warning codes. Update `conservative` and `probe` so Linux chooses cgroup first, then `/proc/meminfo`, and otherwise sets the fallback reason. Keep `resolve` unchanged except for compiling updated test fixtures.

- [ ] **Step 4: Add the Windows implementation and dependency feature**

Extend the target-specific `windows-sys` features with `Win32_System_SystemInformation`. Under `cfg(windows)`, implement a helper that zero-initializes `MEMORYSTATUSEX`, sets `dwLength`, calls `GlobalMemoryStatusEx`, and returns `ullTotalPhys` when nonzero with `ResourceSource::Os`. Call it after Linux-specific sources and before fallback. Non-Linux/non-Windows builds must compile to the fallback path without macOS APIs.

- [ ] **Step 5: Run backend tests and clippy**

Run:

```bash
cargo test --manifest-path backend/Cargo.toml runtime_adaptive
cargo clippy --manifest-path backend/Cargo.toml --all-targets --all-features -- -D warnings
```

Expected: runtime-adaptive tests pass and clippy exits successfully.

- [ ] **Step 6: Commit backend integration**

```bash
git add backend/Cargo.toml backend/Cargo.lock backend/src/runtime_adaptive.rs
git commit -m "fix: improve cross-platform memory detection"
```

## Task 6: Update the admin API type and resource card

**Files:**
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/features/admin/AdminPage.tsx`

- [ ] **Step 1: Add the frontend contract fields**

Extend the runtime resource union with `'proc_meminfo'` and add:

```ts
memory_fallback_reason: string | null;
```

- [ ] **Step 2: Add source/reason presentation helpers**

Map `proc_meminfo` to `/proc/meminfo`, `os` to `操作系统 API`, and preserve the existing cgroup/fallback labels. Map `memory_detection_unavailable` to `内存探测不可用`, with unknown reason codes rendered as their original string.

- [ ] **Step 3: Render memory metadata separately**

Keep the current CPU card. In the memory card, render the formatted byte value, a `来源：...` line, and a conditional `原因：...` line only when the source is fallback and the reason is non-null. Keep the existing warning block and adaptive values unchanged.

- [ ] **Step 4: Run TypeScript checks**

Run:

```bash
npm run lint
```

Expected: exit 0.

## Task 7: Add frontend regression coverage

**Files:**
- Modify: `frontend/tests/admin-settings-ux.behavior.test.tsx`

- [ ] **Step 1: Extend the fallback fixture and assertion**

Set `memory_fallback_reason: 'memory_detection_unavailable'` in the existing fallback runtime fixture and assert `原因：内存探测不可用` is rendered.

- [ ] **Step 2: Add the `/proc/meminfo` source case**

Add a focused admin-settings render case with `memory_source: 'proc_meminfo'`, a non-null memory value, and a null fallback reason. Assert `/proc/meminfo` is rendered and `原因：` is absent.

- [ ] **Step 3: Run the focused frontend tests and verify GREEN**

Run:

```bash
npm test -- --run tests/admin-settings-ux.behavior.test.tsx
```

Expected: all admin settings behavior tests pass.

- [ ] **Step 4: Commit frontend integration**

```bash
git add frontend/src/api/types.ts frontend/src/features/admin/AdminPage.tsx frontend/tests/admin-settings-ux.behavior.test.tsx
git commit -m "feat: show runtime memory detection source"
```

## Task 8: Full verification and PR handoff

**Files:**
- No further source changes unless a verification failure requires a targeted fix.

- [ ] **Step 1: Build the frontend bundle used by RustEmbed**

```bash
npm run build
```

Expected: TypeScript and Vite build succeed.

- [ ] **Step 2: Run the complete backend suite**

```bash
cargo test --manifest-path backend/Cargo.toml
```

Expected: all non-ignored tests pass; existing ignored benchmarks remain ignored.

- [ ] **Step 3: Run the complete frontend suite**

```bash
npm test
```

Expected: all Vitest and standalone frontend behavior tests pass.

- [ ] **Step 4: Review the final diff and status**

```bash
git diff --check origin/main...HEAD
git status --short
git log --oneline --decorate -8
```

Confirm only the design/plan documents and the scoped backend/frontend changes are present, with no edits to the user’s original worktree.

- [ ] **Step 5: Create the pull request**

Push the isolated branch and create a PR titled:

```text
fix: improve memory detection for WSL and non-container environments
```

PR body must mention #184 and summarize:

- cgroup v1/v2 current/ancestor detection;
- `/proc/meminfo` and Windows API fallback order;
- source/reason visibility in admin UI;
- no adaptive tuning algorithm changes;
- full backend/frontend verification results.
