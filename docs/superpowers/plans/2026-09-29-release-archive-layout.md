# Release Archive Layout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Package Linux and Windows release archives with versioned, platform-specific top-level directories while preserving archive filenames and runtime contents.

**Architecture:** Keep `build-linux.sh` and `build-windows.bat` unchanged so their `release/` output remains three flat runtime files. Update only `.github/workflows/release.yml` to stage those files under `Rain-${TAG_NAME}-linux-x64/` or `Rain-${TAG_NAME}-windows-x64/` before creating each archive.

**Tech Stack:** GitHub Actions YAML, Bash, `tar`, `zip`, `unzip`.

---

### Task 1: Stage versioned platform directories in the release workflow

**Files:**
- Modify: `.github/workflows/release.yml:72-82`
- Test: temporary fixture directories under `/tmp` only

- [ ] **Step 1: Update the asset preparation commands**

Replace the current direct archive commands with staging and archive commands equivalent to:

```bash
mkdir -p dist staging
windows_dir="Rain-${TAG_NAME}-windows-x64"
linux_dir="Rain-${TAG_NAME}-linux-x64"

mkdir -p "staging/${windows_dir}" "staging/${linux_dir}"
cp artifacts/Rain-windows-x64/Rain.exe "staging/${windows_dir}/Rain.exe"
cp artifacts/Rain-windows-x64/.env "staging/${windows_dir}/.env"
cp artifacts/Rain-windows-x64/VERSION "staging/${windows_dir}/VERSION"
cp artifacts/rain-linux-x64/rain "staging/${linux_dir}/rain"
cp artifacts/rain-linux-x64/.env "staging/${linux_dir}/.env"
cp artifacts/rain-linux-x64/VERSION "staging/${linux_dir}/VERSION"

chmod +x "staging/${linux_dir}/rain"
(cd staging && zip -r "../dist/Rain-windows-x64-${TAG_NAME}.zip" "${windows_dir}")
(cd staging && tar -czf "../dist/rain-linux-x64-${TAG_NAME}.tar.gz" "${linux_dir}")
```

- [ ] **Step 2: Check the workflow diff**

Run:

```bash
git diff --check
git diff -- .github/workflows/release.yml
```

Expected: no whitespace errors; only the `Prepare assets` shell block changes.

- [ ] **Step 3: Verify both archive listings with a temporary fixture**

Create dummy files in `/tmp/rain-release-layout-fixture`, run the same staging and archive commands with `TAG_NAME=v9.9.9`, then inspect:

```bash
tar -tzf /tmp/rain-release-layout-fixture/dist/rain-linux-x64-v9.9.9.tar.gz
unzip -Z1 /tmp/rain-release-layout-fixture/dist/Rain-windows-x64-v9.9.9.zip
```

Expected listings:

```text
Rain-v9.9.9-linux-x64/
Rain-v9.9.9-linux-x64/rain
Rain-v9.9.9-linux-x64/.env
Rain-v9.9.9-linux-x64/VERSION
Rain-v9.9.9-windows-x64/
Rain-v9.9.9-windows-x64/Rain.exe
Rain-v9.9.9-windows-x64/.env
Rain-v9.9.9-windows-x64/VERSION
```

- [ ] **Step 4: Commit the workflow change**

```bash
git add .github/workflows/release.yml
git commit -m "ci: nest release assets under versioned platform directories"
```

### Task 2: Submit and verify the pull request

**Files:**
- No additional files

- [ ] **Step 1: Push the branch and create the pull request**

```bash
git push -u origin chore/release-archive-layout
gh pr create --base main --head chore/release-archive-layout \
  --title "ci: nest release assets under versioned platform directories" \
  --body "Package Linux and Windows release archives with versioned platform-specific top-level directories while preserving archive filenames and runtime files."
```

- [ ] **Step 2: Wait for required CI checks**

```bash
gh pr checks chore/release-archive-layout
```

Expected: `Build and Test` passes on the branch based on the merged CI fix.
