use super::repository::TransitionResult;
use super::storage::{
    checked_temp_path, cleanup_orphan_temp_files, is_read_lease_active, is_staging_lease_active,
    remove_result_files,
};
use super::*;
use super::{repository, storage};

const TEMP_RESULT_MAX_IN_FLIGHT_PER_CLIENT: usize = 1;
const TEMP_RESULT_MAX_IN_FLIGHT_CLIENTS: usize = 1024;

pub(crate) struct MaterializationLease {
    key: String,
    registry: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
}

impl Drop for MaterializationLease {
    fn drop(&mut self) {
        if let Ok(mut clients) = self.registry.lock()
            && let Some(count) = clients.get_mut(&self.key)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                clients.remove(&self.key);
            }
        }
    }
}

pub(crate) fn acquire_materialization_lease(
    state: &web::Data<AppState>,
    request: &HttpRequest,
) -> Result<MaterializationLease, AppError> {
    let key = super::request_client_key(request);
    let mut clients = state.temp_results.materializations.lock().map_err(|_| {
        AppError::api(
            StatusCode::SERVICE_UNAVAILABLE,
            "TEMP_RESULT_UNAVAILABLE",
            "临时结果服务暂时不可用",
        )
    })?;
    if !clients.contains_key(&key) && clients.len() >= TEMP_RESULT_MAX_IN_FLIGHT_CLIENTS {
        return Err(AppError::api(
            StatusCode::TOO_MANY_REQUESTS,
            "TEMP_RESULT_BUSY",
            "临时结果客户端数量超过限制，请稍后重试",
        ));
    }
    let count = clients.entry(key.clone()).or_insert(0);
    if *count >= TEMP_RESULT_MAX_IN_FLIGHT_PER_CLIENT {
        return Err(AppError::api(
            StatusCode::TOO_MANY_REQUESTS,
            "TEMP_RESULT_BUSY",
            "单个客户端的临时结果任务过多，请稍后重试",
        ));
    }
    *count = count.saturating_add(1);
    Ok(MaterializationLease {
        key,
        registry: state.temp_results.materializations.clone(),
    })
}

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

pub(crate) async fn load_active_unexpired_record(
    state: &web::Data<AppState>,
    id: &str,
) -> Result<TempResultRecord, AppError> {
    repository::find_active_unexpired_by_id(state, id).await
}

pub(crate) async fn acquire_active_result(
    state: &web::Data<AppState>,
    id: &str,
) -> Result<(TempResultRecord, TempResultReadLease), AppError> {
    validate_temp_result_id(id)?;
    repository::find_active_unexpired_by_id(state, id).await?;
    let lease = register_read_lease(state, id)?;
    let record = repository::find_active_unexpired_by_id(state, id).await?;
    Ok((record, lease))
}

pub(crate) fn validate_temp_result_id(id: &str) -> Result<(), AppError> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::NotFound(format!("temporary result {id}")));
    }
    Ok(())
}

pub(crate) async fn cleanup_expired(state: &web::Data<AppState>) -> Result<(), AppError> {
    for record in repository::list_deleting(state).await? {
        if is_read_lease_active(state, &record.id) {
            continue;
        }
        finish_deleting_temp_result(state, &record).await;
    }
    for record in repository::list_expired_active(state).await? {
        if is_read_lease_active(state, &record.id) {
            continue;
        }
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
    if is_read_lease_active(state, &record.id) {
        return;
    }
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

pub(crate) fn preview_page_size(requested: Option<i64>, default: i64, maximum: i64) -> i64 {
    requested.unwrap_or(default).clamp(1, maximum)
}

pub(crate) fn check_temp_result_rate_limit(
    state: &web::Data<AppState>,
    request: &HttpRequest,
) -> Result<(), AppError> {
    let key = super::request_client_key(request);
    let mut limits = state.temp_results.ip_limits.lock().map_err(|_| {
        AppError::api(
            StatusCode::SERVICE_UNAVAILABLE,
            "RATE_LIMIT_UNAVAILABLE",
            "服务暂时不可用",
        )
    })?;
    enforce_temp_result_rate_limit(&mut limits, &key, std::time::Instant::now())
}

fn enforce_temp_result_rate_limit(
    limits: &mut std::collections::HashMap<String, AuthRateLimitBucket>,
    key: &str,
    now: std::time::Instant,
) -> Result<(), AppError> {
    limits.retain(|_, bucket| {
        bucket.prune(now);
        !bucket.is_empty()
    });
    if !limits.contains_key(key) && limits.len() >= TEMP_RESULT_IP_MAX_BUCKETS {
        return Err(AppError::api(
            StatusCode::TOO_MANY_REQUESTS,
            "TEMP_RESULT_RATE_LIMITED",
            "临时结果请求过于频繁，请稍后重试",
        ));
    }
    let bucket = limits
        .entry(key.to_string())
        .or_insert_with(|| AuthRateLimitBucket::new(TEMP_RESULT_RATE_WINDOW));
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
    use super::{
        TEMP_RESULT_IP_MAX_BUCKETS, TEMP_RESULT_RATE_WINDOW, enforce_temp_result_rate_limit,
        preview_page_size,
    };
    use crate::routes::temp_results::common::checked_page_end;
    use crate::{AppState, AuthRateLimitBucket, config::AppLimits, db, error::AppError};
    use actix_web::web;
    use chrono::{Duration, Utc};
    use sqlx::sqlite::SqlitePoolOptions;
    use std::{collections::HashMap, path::PathBuf, time::Instant};

    #[test]
    fn lifecycle_pagination_helpers_reject_overflow() {
        assert_eq!(checked_page_end(10, 5).unwrap(), 15);
        assert!(checked_page_end(i64::MAX, 1).is_err());
        assert_eq!(preview_page_size(None, 5_000, 10_000), 5_000);
        assert_eq!(preview_page_size(Some(20_000), 5_000, 10_000), 10_000);
        assert_eq!(preview_page_size(Some(0), 5_000, 1_000), 1);
    }

    #[test]
    fn rate_limit_prunes_stale_ip_buckets_and_caps_new_ips() {
        let now = Instant::now();
        let mut limits = HashMap::new();
        for index in 0..TEMP_RESULT_IP_MAX_BUCKETS {
            let mut bucket = AuthRateLimitBucket::new(TEMP_RESULT_RATE_WINDOW);
            bucket
                .events
                .push_back(now - TEMP_RESULT_RATE_WINDOW - std::time::Duration::from_secs(1));
            bucket.event_times.push_back(Utc::now());
            limits.insert(format!("stale-{index}"), bucket);
        }

        enforce_temp_result_rate_limit(&mut limits, "new-ip", now).unwrap();
        assert_eq!(limits.len(), 1);
        assert!(limits.contains_key("new-ip"));

        limits.clear();
        for index in 0..TEMP_RESULT_IP_MAX_BUCKETS {
            let mut bucket = AuthRateLimitBucket::new(TEMP_RESULT_RATE_WINDOW);
            bucket.push(now);
            limits.insert(format!("active-{index}"), bucket);
        }
        let error = enforce_temp_result_rate_limit(&mut limits, "another-new-ip", now)
            .expect_err("new IPs must be bounded");
        assert!(matches!(
            error,
            AppError::Api {
                code: "TEMP_RESULT_RATE_LIMITED",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn expired_active_lookup_returns_not_found() {
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

        let result = super::load_active_unexpired_record(&state, "expired-load").await;
        assert!(matches!(result, Err(AppError::NotFound(_))));
    }

    #[tokio::test]
    async fn active_unexpired_lookup_rejects_non_active_records() {
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
        let expires_at = (Utc::now() + Duration::hours(1)).to_rfc3339();
        for (id, status) in [("staging-read", "STAGING"), ("deleting-read", "DELETING")] {
            sqlx::query(
                "INSERT INTO temp_results (id, status, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at) VALUES (?, ?, 'result.log', 'x', 'x', 'data/temp-results/result.log', 0, 0, ?, ?)",
            )
            .bind(id)
            .bind(status)
            .bind(&expires_at)
            .bind(&expires_at)
            .execute(&pool)
            .await
            .unwrap();
            assert!(
                super::load_active_unexpired_record(&state, id)
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn invalid_result_ids_do_not_enter_the_read_lease_registry() {
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

        assert!(
            super::acquire_active_result(&state, "random-invalid-id")
                .await
                .is_err()
        );
        assert!(state.temp_results.reads.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn loading_a_result_does_not_renew_its_expiry() {
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
        let expires_at = (Utc::now() + Duration::hours(1)).to_rfc3339();
        sqlx::query(
            "INSERT INTO temp_results (id, status, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at) VALUES ('fixed-expiry', 'ACTIVE', 'result.log', 'x', 'x', 'data/temp-results/result.log', 0, 0, ?, ?)",
        )
        .bind(&expires_at)
        .bind(&expires_at)
        .execute(&pool)
        .await
        .unwrap();

        let record = super::load_record(&state, "fixed-expiry").await.unwrap();
        assert_eq!(record.expires_at, expires_at);
        let stored: String =
            sqlx::query_scalar("SELECT expires_at FROM temp_results WHERE id = 'fixed-expiry'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, expires_at);
    }
}
