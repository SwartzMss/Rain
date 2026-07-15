use std::{
    fs::File as StdFile,
    io::{Read, Write},
    path::Path,
};

use flate2::read::GzDecoder;
use tokio::task;

use crate::error::AppError;

use super::{
    ArchiveBudget, io_error, io_error_at, join_error,
    path_policy::{
        format_binary_size, gzip_output_name, validate_archive_ratio, validate_extracted_path,
    },
};

pub(crate) async fn extract_gzip_file(
    name: &str,
    src: &Path,
    dest: &Path,
    archive_budget: ArchiveBudget,
) -> Result<(), AppError> {
    let src_path = src.to_path_buf();
    let dest_path = dest.to_path_buf();
    let source_name = name.to_string();
    let output_name = gzip_output_name(name);
    task::spawn_blocking(move || -> Result<(), AppError> {
        archive_budget.reserve_entry()?;
        let remaining = archive_budget.remaining_bytes()?;
        let entry_limit = archive_budget.config.max_entry_size;
        let copy_limit = entry_limit.min(remaining);
        let compressed_size = std::fs::metadata(&src_path).map_err(io_error)?.len().max(1);
        let file = StdFile::open(&src_path)
            .map_err(|error| io_error_at("open gzip archive", &src_path, error))?;
        let mut decoder = GzDecoder::new(file);
        std::fs::create_dir_all(&dest_path)
            .map_err(|error| io_error_at("create gzip extraction directory", &dest_path, error))?;
        let out_path = dest_path.join(output_name);
        validate_extracted_path(
            &out_path,
            &source_name,
            archive_budget.config.max_output_path_chars,
        )?;
        if out_path.exists() {
            return Err(AppError::BadRequest(format!(
                "gzip output path already exists: {}",
                out_path.display()
            )));
        }
        let mut outfile = StdFile::create(&out_path)
            .map_err(|error| io_error_at("create gzip output", &out_path, error))?;
        let copied = copy_with_limit(&mut decoder, &mut outfile, copy_limit).map_err(|error| {
            if matches!(error, AppError::BadRequest(_)) {
                if remaining <= entry_limit {
                    AppError::BadRequest(format!(
                        "archive bundle exceeds configured extracted size; max bundle size {}",
                        format_binary_size(archive_budget.config.max_extracted_size)
                    ))
                } else {
                    AppError::BadRequest(format!(
                        "archive entry exceeds configured limit; max entry size {}",
                        format_binary_size(entry_limit)
                    ))
                }
            } else {
                error
            }
        })?;
        archive_budget.reserve_bytes(copied)?;
        validate_archive_ratio(
            "gzip file",
            copied,
            compressed_size,
            archive_budget.config.max_compression_ratio,
        )?;
        Ok(())
    })
    .await
    .map_err(join_error)??;

    Ok(())
}

fn copy_with_limit<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    limit: u64,
) -> Result<u64, AppError> {
    let mut buffer = [0u8; 16 * 1024];
    let mut total = 0u64;
    loop {
        let read = reader.read(&mut buffer).map_err(io_error)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| AppError::BadRequest("gzip extracted size overflow".into()))?;
        if total > limit {
            return Err(AppError::BadRequest(format!(
                "gzip exceeds limit of {limit} bytes"
            )));
        }
        writer.write_all(&buffer[..read]).map_err(io_error)?;
    }
    Ok(total)
}
