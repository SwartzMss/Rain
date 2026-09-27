use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use sevenz_rust2::{ArchiveReader, Password};
use tokio::task;

use crate::{error::AppError, file_classification::is_supported_archive_name};

use super::{
    ArchiveBudget, io_error_at, join_error,
    path_policy::{
        archive_parent_depth, format_binary_size, normalize_extracted_path, validate_archive_ratio,
        validate_extracted_path,
    },
};

const SEVEN_Z_SIGNATURE: &[u8; 6] = b"7z\xBC\xAF'\x1C";
const SEVEN_Z_START_HEADER_BYTES: usize = 32;
const MAX_SEVEN_Z_HEADER_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) async fn extract_seven_z_archive(
    src: &Path,
    dest: &Path,
    archive_budget: ArchiveBudget,
) -> Result<(), AppError> {
    let src_path = src.to_path_buf();
    let dest_path = dest.to_path_buf();
    task::spawn_blocking(move || {
        extract_seven_z_archive_blocking(&src_path, &dest_path, archive_budget)
    })
    .await
    .map_err(join_error)??;
    Ok(())
}

fn extract_seven_z_archive_blocking(
    src: &Path,
    dest: &Path,
    archive_budget: ArchiveBudget,
) -> Result<(), AppError> {
    let compressed_size = validate_start_header(src, archive_budget.config.max_working_size)?;
    let file = File::open(src).map_err(|error| io_error_at("open 7z archive", src, error))?;
    let mut reader = ArchiveReader::new(file, Password::empty()).map_err(map_seven_z_error)?;
    reader.set_thread_count(1);

    let archive = reader.archive();
    if archive.files.len() > archive_budget.config.max_entries {
        return Err(AppError::BadRequest(format!(
            "7z has too many entries; max {}",
            archive_budget.config.max_entries
        )));
    }
    let packed_size = archive
        .pack_sizes()
        .iter()
        .try_fold(0_u64, |total, size| total.checked_add(*size))
        .ok_or_else(|| AppError::BadRequest("7z packed size overflow".into()))?;
    let compressed_size = compressed_size.max(packed_size).max(1);

    let mut seen_paths = HashSet::new();
    let mut total_uncompressed = 0_u64;
    let mut extraction_error = None;
    let decode_result = reader.for_each_entries(|entry, input| {
        match extract_entry(
            entry,
            input,
            dest,
            &archive_budget,
            compressed_size,
            &mut total_uncompressed,
            &mut seen_paths,
        ) {
            Ok(()) => Ok(true),
            Err(error) => {
                extraction_error = Some(error);
                Err(std::io::Error::other("Rain rejected 7z entry").into())
            }
        }
    });
    if let Some(error) = extraction_error {
        return Err(error);
    }
    decode_result.map_err(map_seven_z_error)?;
    Ok(())
}

fn extract_entry(
    entry: &sevenz_rust2::ArchiveEntry,
    input: &mut dyn Read,
    dest: &Path,
    archive_budget: &ArchiveBudget,
    compressed_size: u64,
    total_uncompressed: &mut u64,
    seen_paths: &mut HashSet<String>,
) -> Result<(), AppError> {
    if entry.is_anti_item {
        return Err(AppError::BadRequest(format!(
            "7z anti-item is unsupported: {}",
            entry.name()
        )));
    }
    let entry_path = safe_entry_path(entry.name())?;
    let depth = if entry.is_directory() {
        entry_path.components().count()
    } else {
        archive_parent_depth(&entry_path)
    };
    if depth > archive_budget.config.max_path_depth {
        return Err(AppError::BadRequest(format!(
            "7z entry is too deep: {}",
            entry.name()
        )));
    }

    let entry_size = entry.size();
    if !entry.is_directory()
        && !is_supported_archive_name(entry.name())
        && entry_size > archive_budget.config.max_entry_size
    {
        return Err(AppError::BadRequest(format!(
            "archive entry exceeds configured limit; max entry size {}: {}",
            format_binary_size(archive_budget.config.max_entry_size),
            entry.name(),
        )));
    }
    archive_budget.reserve_entry()?;
    archive_budget.reserve_bytes(entry_size)?;
    archive_budget.reserve_temp_bytes(entry_size)?;
    *total_uncompressed = total_uncompressed
        .checked_add(entry_size)
        .ok_or_else(|| AppError::BadRequest("7z working size overflow".into()))?;
    if entry_size > 0 {
        validate_archive_ratio(
            entry.name(),
            *total_uncompressed,
            compressed_size,
            archive_budget.config.max_compression_ratio,
        )?;
    }

    let out_path = dest.join(entry_path);
    validate_extracted_path(
        &out_path,
        entry.name(),
        archive_budget.config.max_output_path_chars,
    )?;
    let normalized_out = normalize_extracted_path(&out_path);
    if !seen_paths.insert(normalized_out) {
        return Err(AppError::BadRequest(format!(
            "7z contains duplicate normalized path: {}",
            entry.name()
        )));
    }

    if entry.is_directory() {
        std::fs::create_dir_all(&out_path)
            .map_err(|error| io_error_at("create 7z extracted directory", &out_path, error))?;
        return Ok(());
    }

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| io_error_at("create 7z extraction parent", parent, error))?;
    }
    let mut outfile = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&out_path)
        .map_err(|error| io_error_at("create 7z extracted file", &out_path, error))?;
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer).map_err(|error| {
            AppError::BadRequest(format!("invalid 7z entry {}: {error}", entry.name()))
        })?;
        if read == 0 {
            break;
        }
        copied = copied
            .checked_add(read as u64)
            .ok_or_else(|| AppError::BadRequest("7z entry size overflow".into()))?;
        if copied > entry_size {
            return Err(AppError::BadRequest(format!(
                "7z entry exceeds declared size: {}",
                entry.name()
            )));
        }
        outfile
            .write_all(&buffer[..read])
            .map_err(|error| io_error_at("write 7z extracted file", &out_path, error))?;
    }
    outfile
        .flush()
        .map_err(|error| io_error_at("flush 7z extracted file", &out_path, error))?;
    if copied != entry_size {
        return Err(AppError::BadRequest(format!(
            "7z entry size mismatch: {}",
            entry.name()
        )));
    }
    Ok(())
}

fn safe_entry_path(name: &str) -> Result<PathBuf, AppError> {
    let normalized = name.replace('\\', "/");
    if normalized.starts_with('/')
        || normalized.starts_with("//")
        || (normalized.len() >= 2
            && normalized.as_bytes()[1] == b':'
            && normalized.as_bytes()[0].is_ascii_alphabetic())
    {
        return Err(AppError::BadRequest(format!(
            "7z entry path is absolute: {name}"
        )));
    }
    let path = Path::new(&normalized);
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(AppError::BadRequest(format!(
            "7z entry path escapes destination: {name}"
        )));
    }
    let sanitized = super::path_policy::sanitize_archive_path(path);
    if sanitized.as_os_str().is_empty() {
        return Err(AppError::BadRequest(format!(
            "7z entry path is empty: {name}"
        )));
    }
    Ok(sanitized)
}

fn validate_start_header(path: &Path, max_working_size: u64) -> Result<u64, AppError> {
    let file_size = std::fs::metadata(path)
        .map_err(|error| io_error_at("read 7z archive metadata", path, error))?
        .len();
    let mut file =
        File::open(path).map_err(|error| io_error_at("open 7z archive header", path, error))?;
    let mut header = [0_u8; SEVEN_Z_START_HEADER_BYTES];
    file.read_exact(&mut header)
        .map_err(|error| io_error_at("read 7z archive header", path, error))?;
    if &header[..6] != SEVEN_Z_SIGNATURE {
        return Err(AppError::BadRequest("invalid 7z signature".into()));
    }
    if header[6] != 0 {
        return Err(AppError::BadRequest(format!(
            "unsupported 7z version {}.{}",
            header[6], header[7]
        )));
    }
    let next_header_offset = u64::from_le_bytes(header[12..20].try_into().unwrap());
    let next_header_size = u64::from_le_bytes(header[20..28].try_into().unwrap());
    let header_end = 32_u64
        .checked_add(next_header_offset)
        .and_then(|value| value.checked_add(next_header_size))
        .ok_or_else(|| AppError::BadRequest("7z header offset overflow".into()))?;
    if header_end > file_size {
        return Err(AppError::BadRequest(
            "7z header exceeds archive size".into(),
        ));
    }
    let max_header_size = max_working_size.min(MAX_SEVEN_Z_HEADER_BYTES);
    if next_header_size > max_header_size {
        return Err(AppError::BadRequest(format!(
            "7z header exceeds configured limit; max {}",
            format_binary_size(max_header_size)
        )));
    }
    Ok(file_size)
}

fn map_seven_z_error(error: sevenz_rust2::Error) -> AppError {
    use sevenz_rust2::Error;
    match error {
        Error::PasswordRequired | Error::MaybeBadPassword(_) => {
            AppError::BadRequest("7z encrypted archives are unsupported".into())
        }
        Error::UnsupportedCompressionMethod(method) => {
            AppError::BadRequest(format!("unsupported 7z compression method: {method}"))
        }
        Error::ExternalUnsupported | Error::Unsupported(_) => {
            AppError::BadRequest("unsupported 7z compression method".into())
        }
        Error::MaxMemLimited { .. } => {
            AppError::BadRequest("7z decoder memory requirement exceeds the supported limit".into())
        }
        other => AppError::BadRequest(format!("invalid 7z archive: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter, SourceReader};

    use super::{extract_seven_z_archive_blocking, safe_entry_path};
    use crate::config::ArchiveConfig;
    use crate::ingest::archive::ArchiveBudget;

    #[test]
    fn rejects_traversal_and_absolute_entry_names() {
        for name in [
            "../outside.log",
            r"..\outside.log",
            "/outside.log",
            r"C:\outside.log",
        ] {
            assert!(safe_entry_path(name).is_err(), "{name} should be rejected");
        }
    }

    #[test]
    fn rejects_invalid_signature_before_decoder_startup() {
        let root =
            std::env::temp_dir().join(format!("rain-seven-z-invalid-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create test root");
        let archive_path = root.join("invalid.7z");
        let output = root.join("output");
        std::fs::write(&archive_path, [0_u8; 32]).expect("write invalid archive");
        let error = extract_seven_z_archive_blocking(
            &archive_path,
            &output,
            ArchiveBudget::new(ArchiveConfig::default()),
        )
        .expect_err("invalid signature must fail");
        assert!(error.to_string().contains("invalid 7z signature"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_solid_and_non_solid_7z_entries() {
        let root = std::env::temp_dir().join(format!("rain-seven-z-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create test root");
        let output = root.join("output");

        for (suffix, solid) in [("solid", true), ("non-solid", false)] {
            let archive_path = root.join(format!("{suffix}.7z"));
            let mut writer = ArchiveWriter::create(&archive_path).expect("create archive");
            if solid {
                writer
                    .push_archive_entries(
                        vec![
                            ArchiveEntry::new_file("one.log"),
                            ArchiveEntry::new_file("nested/two.log"),
                        ],
                        vec![
                            SourceReader::new(std::io::Cursor::new(b"one\n".to_vec())),
                            SourceReader::new(std::io::Cursor::new(b"two\n".to_vec())),
                        ],
                    )
                    .expect("write solid archive");
            } else {
                writer
                    .push_archive_entry(
                        ArchiveEntry::new_file("one.log"),
                        Some(std::io::Cursor::new(b"one\n".to_vec())),
                    )
                    .expect("write first non-solid entry");
                writer
                    .push_archive_entry(
                        ArchiveEntry::new_file("nested/two.log"),
                        Some(std::io::Cursor::new(b"two\n".to_vec())),
                    )
                    .expect("write non-solid archive");
            }
            writer.finish().expect("finish archive");

            let destination = output.join(suffix);
            std::fs::create_dir_all(&destination).expect("create output directory");
            extract_seven_z_archive_blocking(
                &archive_path,
                &destination,
                ArchiveBudget::new(ArchiveConfig::for_content_limit(1024)),
            )
            .expect("extract archive");
            assert_eq!(
                std::fs::read_to_string(destination.join("one.log")).unwrap(),
                "one\n"
            );
            assert_eq!(
                std::fs::read_to_string(destination.join("nested/two.log")).unwrap(),
                "two\n"
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
