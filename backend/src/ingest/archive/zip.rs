use std::{collections::HashSet, path::Path};

use tokio::task;

use crate::error::AppError;

use super::{
    ArchiveBudget, io_error_at, join_error,
    path_policy::{
        archive_parent_depth, format_binary_size, normalize_extracted_path, sanitize_archive_path,
        validate_extracted_path, validate_zip_ratio,
    },
};

pub(crate) async fn extract_zip_archive(
    src: &Path,
    dest: &Path,
    archive_budget: ArchiveBudget,
) -> Result<(), AppError> {
    let src_path = src.to_path_buf();
    let dest_path = dest.to_path_buf();
    task::spawn_blocking(move || -> Result<(), AppError> {
        let file = std::fs::File::open(&src_path)
            .map_err(|error| io_error_at("open zip archive", &src_path, error))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|err| AppError::BadRequest(err.to_string()))?;

        if archive.len() > archive_budget.config.max_entries {
            return Err(AppError::BadRequest(format!(
                "zip has too many entries; max {}",
                archive_budget.config.max_entries
            )));
        }

        let mut seen_paths = HashSet::new();
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|err| AppError::BadRequest(err.to_string()))?;
            let entry_path = sanitize_archive_path(Path::new(entry.name()));
            if entry_path.as_os_str().is_empty() {
                continue;
            }

            let depth = if entry.is_dir() {
                entry_path.components().count()
            } else {
                archive_parent_depth(&entry_path)
            };
            if depth > archive_budget.config.max_path_depth {
                return Err(AppError::BadRequest(format!(
                    "zip entry is too deep: {}",
                    entry.name()
                )));
            }

            let uncompressed_size = entry.size();
            if !entry.is_dir() && uncompressed_size > archive_budget.config.max_entry_size {
                return Err(AppError::BadRequest(format!(
                    "archive entry exceeds configured limit; max entry size {}: {}",
                    format_binary_size(archive_budget.config.max_entry_size),
                    entry.name(),
                )));
            }

            archive_budget.reserve_entry()?;
            archive_budget.reserve_bytes(uncompressed_size)?;

            validate_zip_ratio(
                entry.name(),
                uncompressed_size,
                entry.compressed_size(),
                archive_budget.config.max_compression_ratio,
            )?;

            let out_path = dest_path.join(entry_path);
            validate_extracted_path(
                &out_path,
                entry.name(),
                archive_budget.config.max_output_path_chars,
            )?;
            let normalized_out = normalize_extracted_path(&out_path);
            if !seen_paths.insert(normalized_out) {
                return Err(AppError::BadRequest(format!(
                    "zip contains duplicate normalized path: {}",
                    entry.name()
                )));
            }

            if entry.is_dir() {
                std::fs::create_dir_all(&out_path)
                    .map_err(|error| io_error_at("create extracted directory", &out_path, error))?;
            } else {
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|error| io_error_at("create extraction parent", parent, error))?;
                }
                let mut outfile = std::fs::File::create(&out_path)
                    .map_err(|error| io_error_at("create extracted file", &out_path, error))?;
                let copied = std::io::copy(&mut entry, &mut outfile)
                    .map_err(|error| io_error_at("write extracted file", &out_path, error))?;
                if copied != uncompressed_size {
                    return Err(AppError::BadRequest(format!(
                        "zip entry size mismatch: {}",
                        entry.name()
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
