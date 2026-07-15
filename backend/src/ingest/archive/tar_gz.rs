use std::{collections::HashSet, fs::File as StdFile, path::Path};

use flate2::read::GzDecoder;
use tokio::task;

use crate::error::AppError;

use super::{
    ArchiveBudget, io_error, io_error_at, join_error,
    path_policy::{
        archive_parent_depth, format_binary_size, normalize_extracted_path, sanitize_archive_path,
        validate_archive_ratio, validate_extracted_path,
    },
};

pub(crate) async fn extract_tar_gz_archive(
    src: &Path,
    dest: &Path,
    archive_budget: ArchiveBudget,
) -> Result<(), AppError> {
    let src_path = src.to_path_buf();
    let dest_path = dest.to_path_buf();
    task::spawn_blocking(move || -> Result<(), AppError> {
        let compressed_size = std::fs::metadata(&src_path).map_err(io_error)?.len().max(1);
        let file = StdFile::open(&src_path)
            .map_err(|error| io_error_at("open tar.gz archive", &src_path, error))?;
        let decoder = GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        let mut total_uncompressed = 0u64;
        let mut entries_count = 0usize;
        let mut seen_paths = HashSet::new();

        for entry_result in archive.entries().map_err(io_error)? {
            entries_count += 1;
            if entries_count > archive_budget.config.max_entries {
                return Err(AppError::BadRequest(format!(
                    "tar.gz has too many entries; max {}",
                    archive_budget.config.max_entries
                )));
            }

            let mut entry = entry_result.map_err(io_error)?;
            let raw_path = entry.path().map_err(io_error)?.into_owned();
            let entry_path = sanitize_archive_path(&raw_path);
            if entry_path.as_os_str().is_empty() {
                continue;
            }

            let depth = if entry.header().entry_type().is_dir() {
                entry_path.components().count()
            } else {
                archive_parent_depth(&entry_path)
            };
            if depth > archive_budget.config.max_path_depth {
                return Err(AppError::BadRequest(format!(
                    "tar.gz entry is too deep: {}",
                    raw_path.display()
                )));
            }

            let entry_size = entry.header().size().map_err(io_error)?;
            if entry_size > archive_budget.config.max_entry_size {
                return Err(AppError::BadRequest(format!(
                    "archive entry exceeds configured limit; max entry size {}: {}",
                    format_binary_size(archive_budget.config.max_entry_size),
                    raw_path.display(),
                )));
            }

            total_uncompressed = total_uncompressed
                .checked_add(entry_size)
                .ok_or_else(|| AppError::BadRequest("tar.gz extracted size overflow".into()))?;
            archive_budget.reserve_entry()?;
            archive_budget.reserve_bytes(entry_size)?;
            validate_archive_ratio(
                &raw_path.display().to_string(),
                total_uncompressed,
                compressed_size,
                archive_budget.config.max_compression_ratio,
            )?;

            let out_path = dest_path.join(entry_path);
            validate_extracted_path(
                &out_path,
                &raw_path.display().to_string(),
                archive_budget.config.max_output_path_chars,
            )?;
            let normalized_out = normalize_extracted_path(&out_path);
            if !seen_paths.insert(normalized_out) {
                return Err(AppError::BadRequest(format!(
                    "tar.gz contains duplicate normalized path: {}",
                    raw_path.display()
                )));
            }
            if entry.header().entry_type().is_dir() {
                std::fs::create_dir_all(&out_path)
                    .map_err(|error| io_error_at("create extracted directory", &out_path, error))?;
            } else if entry.header().entry_type().is_file() {
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|error| io_error_at("create extraction parent", parent, error))?;
                }
                let mut outfile = StdFile::create(&out_path)
                    .map_err(|error| io_error_at("create extracted file", &out_path, error))?;
                let copied = std::io::copy(&mut entry, &mut outfile)
                    .map_err(|error| io_error_at("write extracted file", &out_path, error))?;
                if copied != entry_size {
                    return Err(AppError::BadRequest(format!(
                        "tar.gz entry size mismatch: {}",
                        raw_path.display()
                    )));
                }
            }
        }

        Ok(())
    })
    .await
    .map_err(join_error)??;

    Ok(())
}
