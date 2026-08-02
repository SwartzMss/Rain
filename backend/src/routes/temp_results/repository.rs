use super::*;

pub(crate) async fn delete_temp_result_record(
    state: &web::Data<AppState>,
    id: &str,
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM temp_results WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(AppError::Database)?;
    Ok(())
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

pub(crate) async fn claim_staging_for_delete(
    state: &web::Data<AppState>,
    id: &str,
) -> Result<bool, AppError> {
    let updated = sqlx::query(
        "UPDATE temp_results SET status = 'DELETING' WHERE id = ? AND status = 'STAGING'",
    )
    .bind(id)
    .execute(&state.pool)
    .await
    .map_err(AppError::Database)?;
    Ok(updated.rows_affected() == 1)
}

pub(crate) async fn delete_deleting_record(
    state: &web::Data<AppState>,
    id: &str,
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM temp_results WHERE id = ? AND status = 'DELETING'")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(AppError::Database)?;
    Ok(())
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
