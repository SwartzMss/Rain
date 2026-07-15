use std::path::{Path, PathBuf};

use crate::error::AppError;

pub(crate) fn normalize_extracted_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn validate_extracted_path(
    path: &Path,
    source_path: &str,
    max_output_path_chars: usize,
) -> Result<(), AppError> {
    let path_chars = path.to_string_lossy().encode_utf16().count();
    if path_chars > max_output_path_chars {
        return Err(AppError::BadRequest(format!(
            "archive output path is too long ({path_chars} > {max_output_path_chars}): {source_path}"
        )));
    }
    Ok(())
}

pub(crate) fn archive_parent_depth(path: &Path) -> usize {
    path.parent()
        .map(|parent| parent.components().count())
        .unwrap_or(0)
}

pub(crate) fn validate_zip_ratio(
    name: &str,
    uncompressed_size: u64,
    compressed_size: u64,
    max_compression_ratio: u64,
) -> Result<(), AppError> {
    validate_archive_ratio(
        name,
        uncompressed_size,
        compressed_size,
        max_compression_ratio,
    )
}

pub(crate) fn validate_archive_ratio(
    name: &str,
    uncompressed_size: u64,
    compressed_size: u64,
    max_compression_ratio: u64,
) -> Result<(), AppError> {
    if uncompressed_size == 0 {
        return Ok(());
    }

    if compressed_size == 0 {
        return Err(AppError::BadRequest(format!(
            "zip entry has invalid compressed size: {name}"
        )));
    }

    if uncompressed_size / compressed_size > max_compression_ratio {
        return Err(AppError::BadRequest(format!(
            "archive compression ratio is too high: {name}"
        )));
    }

    Ok(())
}

pub(crate) fn format_binary_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{} GiB", bytes / 1024 / 1024 / 1024)
    } else if bytes >= 1024 * 1024 {
        format!("{} MiB", bytes / 1024 / 1024)
    } else if bytes >= 1024 {
        format!("{} KiB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

pub(crate) fn sanitize_archive_path(path: &Path) -> PathBuf {
    let mut sanitized = PathBuf::new();
    for component in path.components() {
        if let std::path::Component::Normal(os_str) = component
            && let Some(segment) = os_str.to_str()
        {
            let mut safe = segment
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || "-_.".contains(ch) {
                        ch
                    } else {
                        '_'
                    }
                })
                .collect::<String>();
            while safe.ends_with('.') {
                safe.pop();
            }
            if safe.is_empty() {
                safe.push('_');
            }
            if is_windows_reserved_name(&safe) {
                safe.insert(0, '_');
            }
            sanitized.push(safe);
        }
    }
    sanitized
}

pub(crate) fn is_windows_reserved_name(segment: &str) -> bool {
    let stem = segment
        .split('.')
        .next()
        .unwrap_or(segment)
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem.strip_prefix("COM").is_some_and(|suffix| {
            matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
        })
        || stem.strip_prefix("LPT").is_some_and(|suffix| {
            matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
        })
}

pub(crate) fn gzip_output_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let stripped = if lower.ends_with(".gz") {
        name.get(..name.len().saturating_sub(3)).unwrap_or(name)
    } else {
        name
    };
    let sanitized = stripped
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let mut output = if sanitized.is_empty() {
        "decompressed".to_string()
    } else {
        sanitized
    };
    while output.ends_with('.') {
        output.pop();
    }
    if is_windows_reserved_name(&output) {
        output.insert(0, '_');
    }
    output
}
