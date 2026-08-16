use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TempResultStatus {
    Staging,
    Active,
    Deleting,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TransitionResult<T> {
    Applied(T),
    NotFound,
    StateMismatch,
}

async fn classify_transition_failure(
    state: &web::Data<AppState>,
    id: &str,
) -> Result<TransitionResult<()>, AppError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM temp_results WHERE id = ?)")
        .bind(id)
        .fetch_one(&state.db.pool)
        .await
        .map_err(AppError::Database)?;
    Ok(if exists {
        TransitionResult::StateMismatch
    } else {
        TransitionResult::NotFound
    })
}

impl TempResultStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Staging => "STAGING",
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
        }
    }
}

pub(crate) async fn list_storage_paths(
    state: &web::Data<AppState>,
) -> Result<HashSet<String>, AppError> {
    sqlx::query_scalar("SELECT storage_path FROM temp_results")
        .fetch_all(&state.db.pool)
        .await
        .map(|paths| paths.into_iter().collect())
        .map_err(AppError::Database)
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
        VALUES (?, ?, ?, ?, ?, ?, 0, 0, ?, ?)
        "#,
    )
    .bind(id)
    .bind(TempResultStatus::Staging.as_str())
    .bind(name)
    .bind(expression)
    .bind(source_label)
    .bind(output_path.to_string_lossy().to_string())
    .bind(created_at.to_rfc3339())
    .bind(expires_at.to_rfc3339())
    .execute(&state.db.pool)
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
) -> Result<TransitionResult<()>, AppError> {
    let _capacity_guard = state.temp_results.capacity_lock.lock().await;
    ensure_temp_result_capacity(state, size_bytes, Some(id)).await?;
    let expires_at = (Utc::now() + Duration::days(RETENTION_DAYS)).to_rfc3339();
    let updated = sqlx::query(
        r#"
        UPDATE temp_results
        SET status = ?, expression = ?, source_label = ?, storage_path = ?,
            line_count = ?, size_bytes = ?, expires_at = ?
        WHERE id = ? AND status = ?
        "#,
    )
    .bind(TempResultStatus::Active.as_str())
    .bind(expression)
    .bind(source_label)
    .bind(output_path.to_string_lossy().to_string())
    .bind(line_count)
    .bind(size_bytes)
    .bind(expires_at)
    .bind(id)
    .bind(TempResultStatus::Staging.as_str())
    .execute(&state.db.pool)
    .await
    .map_err(AppError::Database)?;
    if updated.rows_affected() != 1 {
        return classify_transition_failure(state, id).await;
    }
    Ok(TransitionResult::Applied(()))
}

pub(crate) async fn claim_staging_for_delete(
    state: &web::Data<AppState>,
    id: &str,
) -> Result<TransitionResult<()>, AppError> {
    let updated = sqlx::query("UPDATE temp_results SET status = ? WHERE id = ? AND status = ?")
        .bind(TempResultStatus::Deleting.as_str())
        .bind(id)
        .bind(TempResultStatus::Staging.as_str())
        .execute(&state.db.pool)
        .await
        .map_err(AppError::Database)?;
    if updated.rows_affected() == 1 {
        return Ok(TransitionResult::Applied(()));
    }
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM temp_results WHERE id = ?)")
        .bind(id)
        .fetch_one(&state.db.pool)
        .await
        .map_err(AppError::Database)?;
    Ok(if exists {
        TransitionResult::StateMismatch
    } else {
        TransitionResult::NotFound
    })
}

pub(crate) async fn claim_active_for_delete(
    state: &web::Data<AppState>,
    id: &str,
) -> Result<TransitionResult<()>, AppError> {
    let updated = sqlx::query("UPDATE temp_results SET status = ? WHERE id = ? AND status = ?")
        .bind(TempResultStatus::Deleting.as_str())
        .bind(id)
        .bind(TempResultStatus::Active.as_str())
        .execute(&state.db.pool)
        .await
        .map_err(AppError::Database)?;
    if updated.rows_affected() == 1 {
        Ok(TransitionResult::Applied(()))
    } else {
        classify_transition_failure(state, id).await
    }
}

pub(crate) async fn delete_deleting_record(
    state: &web::Data<AppState>,
    id: &str,
) -> Result<TransitionResult<()>, AppError> {
    let updated = sqlx::query("DELETE FROM temp_results WHERE id = ? AND status = ?")
        .bind(id)
        .bind(TempResultStatus::Deleting.as_str())
        .execute(&state.db.pool)
        .await
        .map_err(AppError::Database)?;
    if updated.rows_affected() == 1 {
        return Ok(TransitionResult::Applied(()));
    }
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM temp_results WHERE id = ?)")
        .bind(id)
        .fetch_one(&state.db.pool)
        .await
        .map_err(AppError::Database)?;
    Ok(if exists {
        TransitionResult::StateMismatch
    } else {
        TransitionResult::NotFound
    })
}

pub(crate) async fn ensure_temp_result_budget(state: &web::Data<AppState>) -> Result<(), AppError> {
    let (count, total): (i64, i64) =
        sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM temp_results")
            .fetch_one(&state.db.pool)
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
        .fetch_one(&state.db.pool)
        .await
        .map_err(AppError::Database)?
    } else {
        sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM temp_results")
            .fetch_one(&state.db.pool)
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

pub(crate) async fn find_by_id(
    state: &web::Data<AppState>,
    id: &str,
) -> Result<TempResultRecord, AppError> {
    sqlx::query_as::<_, TempResultRecord>(
        "SELECT id, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at FROM temp_results WHERE id = ? LIMIT 1",
    )
    .bind(id).fetch_optional(&state.db.pool).await.map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("temporary result {id}")))
}

pub(crate) async fn find_active_unexpired_by_id(
    state: &web::Data<AppState>,
    id: &str,
) -> Result<TempResultRecord, AppError> {
    sqlx::query_as::<_, TempResultRecord>(
        "SELECT id, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at FROM temp_results WHERE id = ? AND status = 'ACTIVE' AND datetime(expires_at) >= datetime('now') LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("temporary result {id}")))
}

pub(crate) async fn list_deleting(
    state: &web::Data<AppState>,
) -> Result<Vec<TempResultRecord>, AppError> {
    sqlx::query_as::<_, TempResultRecord>("SELECT id, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at FROM temp_results WHERE status = ? ORDER BY created_at, id")
        .bind(TempResultStatus::Deleting.as_str()).fetch_all(&state.db.pool).await.map_err(AppError::Database)
}
pub(crate) async fn list_expired_active(
    state: &web::Data<AppState>,
) -> Result<Vec<TempResultRecord>, AppError> {
    sqlx::query_as::<_, TempResultRecord>("SELECT id, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at FROM temp_results WHERE status = ? AND datetime(expires_at) < datetime('now') ORDER BY expires_at, id")
        .bind(TempResultStatus::Active.as_str()).fetch_all(&state.db.pool).await.map_err(AppError::Database)
}
pub(crate) async fn claim_expired_active(
    state: &web::Data<AppState>,
    id: &str,
    expires_at: &str,
) -> Result<TransitionResult<String>, AppError> {
    let value: Option<String> = sqlx::query_scalar("UPDATE temp_results SET status = ? WHERE id = ? AND status = ? AND expires_at = ? AND datetime(expires_at) < datetime('now') RETURNING storage_path")
        .bind(TempResultStatus::Deleting.as_str()).bind(id).bind(TempResultStatus::Active.as_str()).bind(expires_at).fetch_optional(&state.db.pool).await.map_err(AppError::Database)
        ?;
    match value {
        Some(path) => Ok(TransitionResult::Applied(path)),
        None => match classify_transition_failure(state, id).await? {
            TransitionResult::NotFound => Ok(TransitionResult::NotFound),
            TransitionResult::StateMismatch => Ok(TransitionResult::StateMismatch),
            TransitionResult::Applied(()) => unreachable!(),
        },
    }
}
pub(crate) async fn list_stale_staging(
    state: &web::Data<AppState>,
) -> Result<Vec<TempResultRecord>, AppError> {
    sqlx::query_as::<_, TempResultRecord>("SELECT id, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at FROM temp_results WHERE status = ? AND datetime(created_at) < datetime('now', '-600 seconds') ORDER BY created_at, id")
        .bind(TempResultStatus::Staging.as_str()).fetch_all(&state.db.pool).await.map_err(AppError::Database)
}
pub(crate) async fn claim_stale_staging(
    state: &web::Data<AppState>,
    id: &str,
    created_at: &str,
) -> Result<TransitionResult<String>, AppError> {
    let value: Option<String> = sqlx::query_scalar("UPDATE temp_results SET status = ? WHERE id = ? AND status = ? AND created_at = ? RETURNING storage_path")
        .bind(TempResultStatus::Deleting.as_str()).bind(id).bind(TempResultStatus::Staging.as_str()).bind(created_at).fetch_optional(&state.db.pool).await.map_err(AppError::Database)
        ?;
    match value {
        Some(path) => Ok(TransitionResult::Applied(path)),
        None => match classify_transition_failure(state, id).await? {
            TransitionResult::NotFound => Ok(TransitionResult::NotFound),
            TransitionResult::StateMismatch => Ok(TransitionResult::StateMismatch),
            TransitionResult::Applied(()) => unreachable!(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use actix_web::web;
    use chrono::Utc;
    use sqlx::sqlite::SqlitePoolOptions;

    use crate::{AppState, config::AppLimits, db};

    use super::{
        TempResultStatus, TransitionResult, claim_active_for_delete, claim_staging_for_delete,
        delete_deleting_record, insert_staging_temp_result, publish_temp_result,
    };

    #[test]
    fn status_mapping_preserves_database_values() {
        assert_eq!(TempResultStatus::Staging.as_str(), "STAGING");
        assert_eq!(TempResultStatus::Active.as_str(), "ACTIVE");
        assert_eq!(TempResultStatus::Deleting.as_str(), "DELETING");
    }

    #[tokio::test]
    async fn staging_claim_and_final_delete_are_single_use() {
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect("sqlite:file:temp_result_claim_race?mode=memory&cache=shared")
            .await
            .unwrap();
        db::prepare_schema(&pool, false).await.unwrap();
        let state = web::Data::new(AppState::new(
            pool.clone(),
            PathBuf::from("data"),
            AppLimits::default(),
        ));
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO temp_results (id, status, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at) VALUES ('staging', 'STAGING', 'staging.log', 'x', 'x', 'data/temp-results/staging.log', 0, 0, ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            claim_staging_for_delete(&state, "staging").await.unwrap(),
            TransitionResult::Applied(())
        );
        assert_eq!(
            claim_staging_for_delete(&state, "staging").await.unwrap(),
            TransitionResult::StateMismatch
        );

        assert_eq!(
            claim_active_for_delete(&state, "staging").await.unwrap(),
            TransitionResult::StateMismatch
        );

        sqlx::query("UPDATE temp_results SET status = 'ACTIVE' WHERE id = 'staging'")
            .execute(&pool)
            .await
            .unwrap();

        let (first, second) = tokio::join!(
            claim_active_for_delete(&state, "staging"),
            claim_active_for_delete(&state, "staging"),
        );
        let results = [first.unwrap(), second.unwrap()];
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, TransitionResult::Applied(())))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, TransitionResult::StateMismatch))
                .count(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM temp_results WHERE id = 'staging'",
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            "DELETING"
        );

        assert_eq!(
            delete_deleting_record(&state, "staging").await.unwrap(),
            TransitionResult::Applied(())
        );
        assert_eq!(
            delete_deleting_record(&state, "staging").await.unwrap(),
            TransitionResult::NotFound
        );
        assert_eq!(
            claim_active_for_delete(&state, "missing").await.unwrap(),
            TransitionResult::NotFound
        );
    }

    #[tokio::test]
    async fn publish_is_applied_once_and_classifies_missing_records() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        db::prepare_schema(&pool, false).await.unwrap();
        let state = web::Data::new(AppState::new(
            pool,
            PathBuf::from("data"),
            AppLimits::default(),
        ));
        let path = PathBuf::from("data/temp-results/publish.log");
        assert_eq!(
            publish_temp_result(&state, "missing01", "x", "x", &path, 0, 0)
                .await
                .unwrap(),
            TransitionResult::NotFound
        );
        insert_staging_temp_result(&state, "publish01", "x", "x", &path)
            .await
            .unwrap();
        assert_eq!(
            publish_temp_result(&state, "publish01", "x", "x", &path, 0, 0)
                .await
                .unwrap(),
            TransitionResult::Applied(())
        );
        assert_eq!(
            publish_temp_result(&state, "publish01", "x", "x", &path, 0, 0)
                .await
                .unwrap(),
            TransitionResult::StateMismatch
        );
    }
}
