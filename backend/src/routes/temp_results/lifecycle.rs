use super::storage::{
    checked_temp_path, cleanup_orphan_temp_files, is_staging_lease_active, remove_result_files,
};
use super::*;
use super::{repository, storage};

pub(crate) async fn abort_staging_result(
    state: &web::Data<AppState>,
    id: &str,
    output_path: &Path,
) {
    match repository::claim_staging_for_delete(state, id).await {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            tracing::warn!(result_id = %id, %error, "failed to claim staging temporary result for cleanup");
            return;
        }
    }
    if let Err(error) = storage::remove_preview_artifacts(output_path).await {
        tracing::warn!(result_id = %id, %error, "staging temporary result files could not be removed; keeping DELETING record");
        return;
    }
    if let Err(error) = repository::delete_deleting_record(state, id).await {
        tracing::warn!(result_id = %id, %error, "staging temporary result record could not be removed; keeping DELETING record");
    }
}

pub(crate) async fn load_record(
    state: &web::Data<AppState>,
    id: &str,
) -> Result<TempResultRecord, AppError> {
    sqlx::query_as::<_, TempResultRecord>(
        r#"
        SELECT id, name, expression, source_label, storage_path, line_count,
               size_bytes, created_at, expires_at
        FROM temp_results WHERE id = ? LIMIT 1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("temporary result {id}")))
}

pub(crate) async fn load_and_renew(
    state: &web::Data<AppState>,
    id: &str,
) -> Result<TempResultRecord, AppError> {
    let expires_at = (Utc::now() + Duration::days(RETENTION_DAYS)).to_rfc3339();
    let updated =
        sqlx::query("UPDATE temp_results SET expires_at = ? WHERE id = ? AND status = 'ACTIVE'")
            .bind(&expires_at)
            .bind(id)
            .execute(&state.pool)
            .await
            .map_err(AppError::Database)?;
    if updated.rows_affected() != 1 {
        return Err(AppError::NotFound(format!("temporary result {id}")));
    }
    load_record(state, id).await
}

pub(crate) async fn cleanup_expired(state: &web::Data<AppState>) -> Result<(), AppError> {
    let deleting_records = sqlx::query_as::<_, TempResultRecord>(
        r#"
        SELECT id, name, expression, source_label, storage_path, line_count,
               size_bytes, created_at, expires_at
        FROM temp_results WHERE status = 'DELETING' ORDER BY created_at, id
        "#,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;
    for record in deleting_records {
        finish_deleting_temp_result(state, &record).await;
    }

    let expired_records = sqlx::query_as::<_, TempResultRecord>(
        r#"
        SELECT id, name, expression, source_label, storage_path, line_count,
               size_bytes, created_at, expires_at
        FROM temp_results
        WHERE status = 'ACTIVE' AND datetime(expires_at) < datetime('now')
        ORDER BY expires_at, id
        "#,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;
    for record in expired_records {
        let storage_path: Option<String> = sqlx::query_scalar(
            "UPDATE temp_results SET status = 'DELETING' WHERE id = ? AND status = 'ACTIVE' AND expires_at = ? AND datetime(expires_at) < datetime('now') RETURNING storage_path",
        )
        .bind(&record.id)
        .bind(&record.expires_at)
        .fetch_optional(&state.pool)
        .await
        .map_err(AppError::Database)?;
        let Some(storage_path) = storage_path else {
            continue;
        };
        let mut claimed = record;
        claimed.storage_path = storage_path;
        finish_deleting_temp_result(state, &claimed).await;
    }

    let staging_records = sqlx::query_as::<_, TempResultRecord>(
        r#"
        SELECT id, name, expression, source_label, storage_path, line_count,
               size_bytes, created_at, expires_at
        FROM temp_results
        WHERE status = 'STAGING' AND datetime(created_at) < datetime('now', '-600 seconds')
        ORDER BY created_at, id
        "#,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;
    for record in staging_records {
        if is_staging_lease_active(state, &record.id) {
            continue;
        }
        let claimed: Option<String> = sqlx::query_scalar(
            "UPDATE temp_results SET status = 'DELETING' WHERE id = ? AND status = 'STAGING' AND created_at = ? RETURNING storage_path",
        )
        .bind(&record.id)
        .bind(&record.created_at)
        .fetch_optional(&state.pool)
        .await
        .map_err(AppError::Database)?;
        let Some(storage_path) = claimed else {
            continue;
        };
        let mut claimed_record = record;
        claimed_record.storage_path = storage_path;
        finish_deleting_temp_result(state, &claimed_record).await;
    }

    cleanup_orphan_temp_files(state).await?;
    Ok(())
}

pub(crate) async fn finish_deleting_temp_result(
    state: &web::Data<AppState>,
    record: &TempResultRecord,
) {
    let path = match checked_temp_path(state, &record.storage_path) {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(result_id = %record.id, %error, "temporary result path is invalid; keeping DELETING record");
            return;
        }
    };
    if let Err(error) = remove_result_files(&path).await {
        tracing::warn!(result_id = %record.id, %error, "temporary result files could not be removed; keeping DELETING record");
        return;
    }
    if let Err(error) = sqlx::query("DELETE FROM temp_results WHERE id = ? AND status = 'DELETING'")
        .bind(&record.id)
        .execute(&state.pool)
        .await
    {
        tracing::warn!(result_id = %record.id, %error, "temporary result database record could not be removed; keeping DELETING record");
    }
}

pub(crate) fn preview_page_size(requested: Option<i64>) -> i64 {
    requested.unwrap_or(5_000).clamp(1, 10_000)
}

pub(crate) fn check_temp_result_rate_limit(
    state: &web::Data<AppState>,
    request: &HttpRequest,
) -> Result<(), AppError> {
    let key = request
        .peer_addr()
        .map(|address| address.ip().to_string())
        .unwrap_or_else(|| "unknown".into());
    let mut limits = state.auth_rate_limits.lock().map_err(|_| {
        AppError::api(
            StatusCode::SERVICE_UNAVAILABLE,
            "RATE_LIMIT_UNAVAILABLE",
            "服务暂时不可用",
        )
    })?;
    let bucket = limits
        .temp_result_ip
        .entry(key)
        .or_insert_with(|| AuthRateLimitBucket::new(TEMP_RESULT_RATE_WINDOW));
    let now = std::time::Instant::now();
    bucket.prune(now);
    if bucket.len() >= TEMP_RESULT_RATE_LIMIT {
        return Err(AppError::api(
            StatusCode::TOO_MANY_REQUESTS,
            "TEMP_RESULT_RATE_LIMITED",
            "临时结果请求过于频繁，请稍后重试",
        ));
    }
    bucket.push(now);
    Ok(())
}

pub(crate) fn checked_page_end(start: i64, limit: i64) -> Result<i64, AppError> {
    start
        .checked_add(limit)
        .ok_or_else(|| AppError::BadRequest("分页参数超出支持范围".into()))
}

pub(crate) fn to_response(record: TempResultRecord) -> TempResult {
    TempResult {
        id: record.id,
        name: record.name,
        expression: record.expression,
        source_label: record.source_label,
        line_count: record.line_count,
        size_bytes: record.size_bytes,
        created_at: record.created_at,
        expires_at: record.expires_at,
    }
}
