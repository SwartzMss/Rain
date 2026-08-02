use super::lifecycle::checked_page_end;
use super::*;

pub(crate) async fn result_storage_size(
    log_path: &Path,
    meta_path: &Path,
    index_path: &Path,
) -> Result<u64, std::io::Error> {
    let mut total = 0_u64;
    for path in [log_path, meta_path, index_path] {
        let size = tokio::fs::metadata(path).await?.len();
        total = total
            .checked_add(size)
            .ok_or_else(|| std::io::Error::other("temporary result size overflow"))?;
    }
    Ok(total)
}

pub(crate) fn temp_result_too_large() -> AppError {
    AppError::public(
        StatusCode::PAYLOAD_TOO_LARGE,
        "TEMP_RESULT_TOO_LARGE",
        "临时结果超过大小限制",
    )
}

pub(crate) async fn read_indexed_lines(
    result_path: &Path,
    meta_path: &Path,
    index_path: &Path,
    start: i64,
    limit: i64,
    line_count: i64,
) -> Result<Vec<TempLine>, AppError> {
    if start >= line_count {
        return Ok(Vec::new());
    }
    let index_content = tokio::fs::read_to_string(index_path)
        .await
        .map_err(AppError::Io)?;
    if index_content.is_empty() {
        return Err(invalid_sidecar(
            "temporary result index is empty for a nonempty result",
        ));
    }
    let checkpoints = index_content
        .lines()
        .map(decode_sidecar::<SparseCheckpoint>)
        .collect::<Result<Vec<_>, _>>()?;
    let checkpoint = select_checkpoint(&checkpoints, start).ok_or_else(|| {
        AppError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "temporary result index has no checkpoint for requested line",
        ))
    })?;
    let mut log_reader = BufReader::new(File::open(result_path).await.map_err(AppError::Io)?);
    let mut meta_reader = BufReader::new(File::open(meta_path).await.map_err(AppError::Io)?);
    log_reader
        .seek(SeekFrom::Start(checkpoint.log_offset))
        .await
        .map_err(AppError::Io)?;
    meta_reader
        .seek(SeekFrom::Start(checkpoint.meta_offset))
        .await
        .map_err(AppError::Io)?;

    let mut current = checkpoint.result_line;
    let mut content = String::new();
    let mut metadata_line = String::new();
    let mut lines = Vec::new();
    let expected_end = checked_page_end(start, limit)?.min(line_count);
    while lines.len() < limit as usize {
        content.clear();
        metadata_line.clear();
        let content_bytes = log_reader
            .read_line(&mut content)
            .await
            .map_err(AppError::Io)?;
        let metadata_bytes = meta_reader
            .read_line(&mut metadata_line)
            .await
            .map_err(AppError::Io)?;
        if content_bytes == 0 && metadata_bytes == 0 {
            if current < expected_end {
                return Err(invalid_sidecar(
                    "temporary result ended before expected line count",
                ));
            }
            break;
        }
        if content_bytes == 0 || metadata_bytes == 0 {
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "temporary result content and metadata are out of sync",
            )));
        }
        let metadata = decode_sidecar::<MatchMetadata>(metadata_line.trim_end())?;
        if current >= start {
            lines.push(TempLine {
                bundle_hash: metadata.bundle_hash,
                file_id: metadata.file_id,
                path: Some(metadata.path),
                line_number: metadata.line_number,
                content: content.trim_end_matches(['\r', '\n']).to_string(),
            });
        }
        current += 1;
    }
    Ok(lines)
}

pub(crate) fn decode_sidecar<T: serde::de::DeserializeOwned>(line: &str) -> Result<T, AppError> {
    serde_json::from_str(line)
        .map_err(|error| AppError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error)))
}

pub(crate) fn invalid_sidecar(message: &str) -> AppError {
    AppError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    ))
}

pub(crate) async fn remove_result_files(log_path: &Path) -> Result<(), AppError> {
    for path in [
        log_path.to_path_buf(),
        log_path.with_extension("meta"),
        log_path.with_extension("idx"),
    ] {
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(AppError::Io(error)),
        }
    }
    Ok(())
}

pub(crate) fn staging_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".part");
    PathBuf::from(value)
}

pub(crate) async fn remove_preview_artifacts(log_path: &Path) -> Result<(), AppError> {
    let paths = [
        log_path.to_path_buf(),
        log_path.with_extension("meta"),
        log_path.with_extension("idx"),
    ];
    for path in paths {
        for candidate in [path.clone(), staging_path(&path)] {
            match tokio::fs::remove_file(candidate).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(AppError::Io(error)),
            }
        }
    }
    Ok(())
}

pub(crate) async fn cleanup_orphan_temp_files(
    state: &web::Data<AppState>,
    storage_paths: &HashSet<String>,
) -> Result<(), AppError> {
    let root = data_root(state).join("temp-results");
    let mut directory = match tokio::fs::read_dir(&root).await {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(AppError::Io(error)),
    };
    let cutoff = SystemTime::now()
        .checked_sub(ORPHAN_GRACE_PERIOD)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut orphan_stems = HashSet::new();
    while let Some(entry) = directory.next_entry().await.map_err(AppError::Io)? {
        let path = entry.path();
        let metadata = match entry.metadata().await {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(AppError::Io(error)),
        };
        if metadata
            .modified()
            .ok()
            .is_some_and(|modified| modified > cutoff)
        {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if artifact_staging_id(file_name).is_some_and(|id| is_staging_lease_active(state, &id)) {
            continue;
        }
        if file_name.starts_with(".ready-") || file_name.ends_with(".part") {
            remove_stale_file(&path).await;
            continue;
        }
        if matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("log" | "meta" | "idx")
        ) && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
        {
            orphan_stems.insert(stem.to_string());
        }
    }
    for stem in orphan_stems {
        let log_path = root.join(format!("{stem}.log"));
        if storage_paths.contains(&log_path.to_string_lossy().to_string()) {
            continue;
        }
        for path in [
            log_path,
            root.join(format!("{stem}.meta")),
            root.join(format!("{stem}.idx")),
        ] {
            remove_stale_file(&path).await;
        }
    }
    Ok(())
}

pub(crate) fn artifact_staging_id(file_name: &str) -> Option<String> {
    [".log.part", ".meta.part", ".idx.part"]
        .iter()
        .find_map(|suffix| file_name.strip_suffix(suffix).map(ToOwned::to_owned))
}

pub(crate) fn is_staging_lease_active(state: &web::Data<AppState>, id: &str) -> bool {
    state
        .temp_result_staging
        .lock()
        .map(|staging| staging.contains(id))
        .unwrap_or(false)
}

pub(crate) async fn remove_stale_file(path: &Path) {
    match tokio::fs::remove_file(path).await {
        Ok(()) => {
            tracing::debug!(path = %path.display(), "removed stale temporary result artifact")
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "stale temporary result artifact could not be removed")
        }
    }
}

pub(crate) fn checked_temp_path(
    state: &web::Data<AppState>,
    stored_path: &str,
) -> Result<PathBuf, AppError> {
    let root = data_root(state).join("temp-results");
    let path = Path::new(stored_path);
    if !path.starts_with(&root) {
        return Err(AppError::BadRequest(
            "temporary result path is invalid".into(),
        ));
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::staging_path;

    #[test]
    fn staging_artifact_uses_part_suffix() {
        let path = std::path::Path::new("temp-results/result.log");
        assert_eq!(
            staging_path(path),
            std::path::PathBuf::from("temp-results/result.log.part")
        );
    }
}
