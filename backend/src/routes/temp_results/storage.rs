use super::lifecycle::checked_page_end;
use super::lifecycle::remove_preview_artifacts;
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

pub(crate) async fn insert_staging_temp_result(
    state: &web::Data<AppState>,
    id: &str,
    expression: &str,
    source_label: &str,
    output_path: &Path,
) -> Result<(), AppError> {
    let created_at = Utc::now();
    let expires_at = created_at + Duration::days(RETENTION_DAYS);
    let name = format!("filtered-{}.log", &id[..8]);
    sqlx::query(
        r#"
        INSERT INTO temp_results
            (id, status, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at)
        VALUES (?, 'STAGING', ?, ?, ?, ?, 0, 0, ?, ?)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(expression)
    .bind(source_label)
    .bind(output_path.to_string_lossy().to_string())
    .bind(created_at.to_rfc3339())
    .bind(expires_at.to_rfc3339())
    .execute(&state.pool)
    .await
    .map_err(AppError::Database)?;
    Ok(())
}

pub(crate) async fn publish_temp_result(
    state: &web::Data<AppState>,
    id: &str,
    expression: &str,
    source_label: &str,
    output_path: &Path,
    line_count: i64,
    size_bytes: i64,
) -> Result<(), AppError> {
    let _capacity_guard = state.temp_result_capacity_lock.lock().await;
    ensure_temp_result_capacity(state, size_bytes, Some(id)).await?;
    let expires_at = (Utc::now() + Duration::days(RETENTION_DAYS)).to_rfc3339();
    let updated = sqlx::query(
        r#"
        UPDATE temp_results
        SET status = 'ACTIVE', expression = ?, source_label = ?, storage_path = ?,
            line_count = ?, size_bytes = ?, expires_at = ?
        WHERE id = ? AND status = 'STAGING'
        "#,
    )
    .bind(expression)
    .bind(source_label)
    .bind(output_path.to_string_lossy().to_string())
    .bind(line_count)
    .bind(size_bytes)
    .bind(expires_at)
    .bind(id)
    .execute(&state.pool)
    .await
    .map_err(AppError::Database)?;
    if updated.rows_affected() != 1 {
        return Err(AppError::NotFound(format!("temporary result {id}")));
    }
    Ok(())
}

pub(crate) async fn abort_staging_result(
    state: &web::Data<AppState>,
    id: &str,
    output_path: &Path,
) {
    if let Err(error) = sqlx::query(
        "UPDATE temp_results SET status = 'DELETING' WHERE id = ? AND status = 'STAGING'",
    )
    .bind(id)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(result_id = %id, %error, "failed to claim staging temporary result for cleanup");
        return;
    }
    if let Err(error) = remove_preview_artifacts(output_path).await {
        tracing::warn!(result_id = %id, %error, "staging temporary result files could not be removed; keeping DELETING record");
        return;
    }
    if let Err(error) = sqlx::query("DELETE FROM temp_results WHERE id = ? AND status = 'DELETING'")
        .bind(id)
        .execute(&state.pool)
        .await
    {
        tracing::warn!(result_id = %id, %error, "staging temporary result record could not be removed; keeping DELETING record");
    }
}

pub(crate) async fn ensure_temp_result_budget(state: &web::Data<AppState>) -> Result<(), AppError> {
    let (count, total): (i64, i64) =
        sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM temp_results")
            .fetch_one(&state.pool)
            .await
            .map_err(AppError::Database)?;
    if count >= state.limits.temp_results.max_records
        || total >= i64::try_from(state.limits.temp_results.max_total_size).unwrap_or(i64::MAX)
    {
        return Err(AppError::api(
            StatusCode::TOO_MANY_REQUESTS,
            "TEMP_RESULT_BUDGET_EXCEEDED",
            "临时结果存储配额已用尽，请稍后重试",
        ));
    }
    Ok(())
}

pub(crate) async fn ensure_temp_result_capacity(
    state: &web::Data<AppState>,
    size_bytes: i64,
    current_id: Option<&str>,
) -> Result<(), AppError> {
    let (count, total): (i64, i64) = if let Some(current_id) = current_id {
        sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM temp_results WHERE id != ?",
        )
        .bind(current_id)
        .fetch_one(&state.pool)
        .await
        .map_err(AppError::Database)?
    } else {
        sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM temp_results")
            .fetch_one(&state.pool)
            .await
            .map_err(AppError::Database)?
    };
    let next_total = total.checked_add(size_bytes).ok_or_else(|| {
        AppError::api(
            StatusCode::TOO_MANY_REQUESTS,
            "TEMP_RESULT_BUDGET_EXCEEDED",
            "临时结果存储配额已用尽，请稍后重试",
        )
    })?;
    if count >= state.limits.temp_results.max_records
        || next_total > i64::try_from(state.limits.temp_results.max_total_size).unwrap_or(i64::MAX)
    {
        return Err(AppError::api(
            StatusCode::TOO_MANY_REQUESTS,
            "TEMP_RESULT_BUDGET_EXCEEDED",
            "临时结果存储配额已用尽，请稍后重试",
        ));
    }
    Ok(())
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
