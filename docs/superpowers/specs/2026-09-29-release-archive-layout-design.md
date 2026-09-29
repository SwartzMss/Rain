# Release archive layout

## Context

The release workflow currently places the three runtime files directly at the
root of each Linux tarball and Windows zip archive. This makes extracting a
release into a shared directory easy to collide with other versions.

## Design

Keep the build scripts and their `release/` output unchanged. In the release
workflow's asset preparation step, stage each platform's three files under a
versioned, platform-specific top-level directory before creating the archive:

- `Rain-${TAG_NAME}-linux-x64/` containing `rain`, `.env`, and `VERSION`
- `Rain-${TAG_NAME}-windows-x64/` containing `Rain.exe`, `.env`, and `VERSION`

The downloadable archive filenames remain unchanged:

- `rain-linux-x64-${TAG_NAME}.tar.gz`
- `Rain-windows-x64-${TAG_NAME}.zip`

This keeps existing download names stable while making extraction self-
contained and collision-resistant.

## Verification

The workflow should stage files explicitly and create both archives from the
staging directory. Local verification will inspect each archive's listing and
confirm the expected top-level directory and all three files are present.
