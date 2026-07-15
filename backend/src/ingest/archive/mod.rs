use std::{io, path::Path};

use crate::error::AppError;

mod budget;
mod gzip;
pub(crate) mod path_policy;
mod tar_gz;
mod zip;

pub use budget::ArchiveBudget;
#[cfg(test)]
pub(crate) use gzip::extract_gzip_file;
pub(crate) use path_policy::validate_extracted_path;
#[cfg(test)]
pub(crate) use path_policy::{archive_parent_depth, gzip_output_name, sanitize_archive_path};

pub(crate) async fn extract_archive(
    name: &str,
    src: &Path,
    dest: &Path,
    archive_budget: ArchiveBudget,
) -> Result<(), AppError> {
    if is_zip_file(name) {
        zip::extract_zip_archive(src, dest, archive_budget).await
    } else if is_tar_gz_file(name) {
        tar_gz::extract_tar_gz_archive(src, dest, archive_budget).await
    } else if is_gzip_file(name) {
        gzip::extract_gzip_file(name, src, dest, archive_budget).await
    } else {
        Err(AppError::BadRequest(format!(
            "unsupported archive type: {name}"
        )))
    }
}

fn is_zip_file(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
}

fn is_tar_gz_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".tar.gz") || lower.ends_with(".tgz")
}

fn is_gzip_file(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".gz") && !is_tar_gz_file(name)
}

pub(super) fn io_error(err: std::io::Error) -> AppError {
    AppError::Io(err)
}

pub(super) fn io_error_at(operation: &str, path: &Path, error: std::io::Error) -> AppError {
    AppError::Io(std::io::Error::new(
        error.kind(),
        format!("{operation} {}: {error}", path.display()),
    ))
}

pub(super) fn join_error(err: tokio::task::JoinError) -> AppError {
    io_error(io::Error::other(err.to_string()))
}
