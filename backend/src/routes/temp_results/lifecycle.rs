use super::repository::TransitionResult;
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
        Ok(TransitionResult::Applied(())) => {}
        Ok(TransitionResult::NotFound | TransitionResult::StateMismatch) => return,
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
    repository::find_by_id(state, id).await
}

pub(crate) async fn load_and_renew(
    state: &web::Data<AppState>,
    id: &str,
) -> Result<TempResultRecord, AppError> {
    let expires_at = (Utc::now() + Duration::days(RETENTION_DAYS)).to_rfc3339();
    match repository::renew_active(state, id, &expires_at).await? {
        TransitionResult::Applied(()) => {}
        TransitionResult::NotFound | TransitionResult::StateMismatch => {
            return Err(AppError::NotFound(format!("temporary result {id}")));
        }
    }
    repository::find_by_id(state, id).await
}

pub(crate) async fn cleanup_expired(state: &web::Data<AppState>) -> Result<(), AppError> {
    for record in repository::list_deleting(state).await? {
        finish_deleting_temp_result(state, &record).await;
    }
    for record in repository::list_expired_active(state).await? {
        let TransitionResult::Applied(storage_path) =
            repository::claim_expired_active(state, &record.id, &record.expires_at).await?
        else {
            continue;
        };
        let mut claimed = record;
        claimed.storage_path = storage_path;
        finish_deleting_temp_result(state, &claimed).await;
    }
    for record in repository::list_stale_staging(state).await? {
        if is_staging_lease_active(state, &record.id) {
            continue;
        }
        let TransitionResult::Applied(storage_path) =
            repository::claim_stale_staging(state, &record.id, &record.created_at).await?
        else {
            continue;
        };
        let mut claimed = record;
        claimed.storage_path = storage_path;
        finish_deleting_temp_result(state, &claimed).await;
    }
    let storage_paths = repository::list_storage_paths(state).await?;
    cleanup_orphan_temp_files(state, &storage_paths).await?;
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
    match repository::delete_deleting_record(state, &record.id).await {
        Ok(TransitionResult::Applied(())) => {}
        Ok(TransitionResult::NotFound | TransitionResult::StateMismatch) => {
            tracing::debug!(result_id = %record.id, "temporary result deletion was already completed")
        }
        Err(error) => {
            tracing::warn!(result_id = %record.id, %error, "temporary result database record could not be removed; keeping DELETING record");
        }
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

#[cfg(test)]
mod tests {
    use super::preview_page_size;
    use crate::routes::temp_results::common::checked_page_end;
    use crate::{AppState, config::AppLimits, db, error::AppError};
    use actix_web::web;
    use chrono::{Duration, Utc};
    use sqlx::sqlite::SqlitePoolOptions;
    use std::path::PathBuf;

    #[test]
    fn lifecycle_pagination_helpers_reject_overflow() {
        assert_eq!(checked_page_end(10, 5).unwrap(), 15);
        assert!(checked_page_end(i64::MAX, 1).is_err());
        assert_eq!(preview_page_size(Some(20_000)), 10_000);
    }

    #[tokio::test]
    async fn expired_load_and_renew_returns_not_found() {
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect("sqlite::memory:?cache=shared")
            .await
            .unwrap();
        db::prepare_schema(&pool, false).await.unwrap();
        let state = web::Data::new(AppState::new(
            pool.clone(),
            PathBuf::from("data"),
            AppLimits::default(),
        ));
        let expired = (Utc::now() - Duration::minutes(1)).to_rfc3339();
        sqlx::query(
            "INSERT INTO temp_results (id, status, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at) VALUES ('expired-load', 'ACTIVE', 'expired.log', 'x', 'x', 'data/temp-results/expired.log', 0, 0, ?, ?)",
        )
        .bind(&expired)
        .bind(&expired)
        .execute(&pool)
        .await
        .unwrap();

        let result = super::load_and_renew(&state, "expired-load").await;
        assert!(matches!(result, Err(AppError::NotFound(_))));
    }
}
