use actix_web::{HttpResponse, get, http::StatusCode, web};
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::{AppState, ReadinessSnapshot};

const READINESS_CACHE_TTL: Duration = Duration::from_secs(5);

fn build_version() -> &'static str {
    option_env!("RAIN_RELEASE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

#[get("/healthz")]
pub async fn health() -> HttpResponse {
    HttpResponse::Ok().json(json!({
        "status": "ok",
        "service": "rain-backend",
        "version": build_version()
    }))
}

#[get("/readyz")]
pub async fn readiness(state: web::Data<AppState>) -> HttpResponse {
    readiness_response(&state).await
}

async fn readiness_response(state: &AppState) -> HttpResponse {
    let snapshot = {
        // Keep the refresh lock through both probes so concurrent misses share one result.
        let mut cache = state.readiness_cache.0.lock().await;
        if let Some(snapshot) = cache.filter(|s| s.checked_at.elapsed() < READINESS_CACHE_TTL) {
            snapshot
        } else {
            let database_ok = check_database(&state.db.pool).await;
            let storage_ok = check_storage(&state.storage.data_root).await;
            let snapshot = ReadinessSnapshot {
                checked_at: Instant::now(),
                database_ok,
                storage_ok,
            };
            *cache = Some(snapshot);
            snapshot
        }
    };
    let database_ok = snapshot.database_ok;
    let storage_ok = snapshot.storage_ok;
    let recovery_ok = state.recovery.invariant_recovery_ready();
    let ready = database_ok && storage_ok && recovery_ok;
    HttpResponse::build(if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    })
    .json(json!({
        "status": if ready { "ready" } else { "not_ready" },
        "database": database_ok,
        "storage": storage_ok,
        "recovery": recovery_ok,
        "service": "rain-backend",
        "version": build_version()
    }))
}

async fn check_storage(root: &std::path::Path) -> bool {
    check_writable_dir(&root.join("blobs")).await
        && check_writable_dir(&root.join(".tmp")).await
        && check_writable_dir(&root.join("temp-results")).await
}

async fn check_writable_dir(directory: &std::path::Path) -> bool {
    if tokio::fs::create_dir_all(directory).await.is_err() {
        return false;
    }
    let probe = directory.join(format!(".ready-{}", Uuid::new_v4().simple()));
    match tokio::fs::File::create(&probe).await {
        Ok(mut file) => {
            file.write_all(b"ready").await.is_ok()
                && file.sync_all().await.is_ok()
                && tokio::fs::remove_file(probe).await.is_ok()
        }
        Err(_) => false,
    }
}

async fn check_database(pool: &sqlx::SqlitePool) -> bool {
    // The probe must roll back, so it cannot use the committing write::run helper.
    // This guard is released before readiness starts any storage I/O.
    let _writer = crate::db::write::acquire(pool).await;
    let Ok(mut transaction) = pool.begin().await else {
        return false;
    };
    let write = sqlx::query("INSERT INTO rain_ready_probe (id, value) VALUES (?, 1)")
        .bind(Uuid::new_v4().simple().to_string())
        .execute(&mut *transaction)
        .await;
    // Await rollback even on write failure before releasing writer admission.
    let rollback = transaction.rollback().await;
    write.is_ok() && rollback.is_ok()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use actix_web::{body::to_bytes, http::StatusCode, web};
    use sqlx::sqlite::SqlitePoolOptions;

    use super::readiness_response;
    use crate::{AppState, RecoveryRuntime, config::AppLimits, db};

    #[actix_web::test]
    async fn concurrent_readiness_refreshes_once_and_rolls_back() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let acquisitions = Arc::new(AtomicUsize::new(0));
        let counter = acquisitions.clone();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .before_acquire(move |_, _| {
                counter.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(true) })
            })
            .connect("sqlite::memory:")
            .await
            .unwrap();
        db::prepare_schema(&pool, false).await.unwrap();
        let root = std::env::temp_dir().join(format!("rain-ready-{}", uuid::Uuid::new_v4()));
        let state = AppState::new(pool.clone(), root.clone(), AppLimits::default());
        acquisitions.store(0, Ordering::SeqCst);

        for round in 0..2 {
            if round == 1 {
                // Expire the snapshot without relying on wall-clock sleeps.
                state
                    .readiness_cache
                    .0
                    .lock()
                    .await
                    .as_mut()
                    .unwrap()
                    .checked_at = std::time::Instant::now() - super::READINESS_CACHE_TTL;
            }
            let responses =
                futures_util::future::join_all((0..32).map(|_| readiness_response(&state))).await;
            assert!(responses.iter().all(|r| r.status() == StatusCode::OK));
            assert_eq!(acquisitions.load(Ordering::SeqCst), round + 1);
        }
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rain_ready_probe")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 0, "readiness writes must never commit");
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[actix_web::test]
    async fn readiness_honors_write_gate_and_recovers_from_cancelled_refresh() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        db::prepare_schema(&pool, false).await.unwrap();
        let root = std::env::temp_dir().join(format!("rain-ready-{}", uuid::Uuid::new_v4()));
        let state = AppState::new(pool.clone(), root.clone(), AppLimits::default());
        let gate = crate::db::write::acquire(&pool).await;
        {
            let mut pending = Box::pin(readiness_response(&state));
            assert!(futures_util::poll!(&mut pending).is_pending());
            // The write gate is acquired before a database connection is held.
            let connection =
                tokio::time::timeout(std::time::Duration::from_secs(5), pool.acquire())
                    .await
                    .expect("probe must wait before pool.begin")
                    .unwrap();
            drop(connection);
        }
        drop(gate);
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            readiness_response(&state),
        )
        .await
        .expect("cancelled refresh must release the cache lock");
        assert_eq!(response.status(), StatusCode::OK);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[actix_web::test]
    async fn database_failure_is_cached_until_expiry() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let root = std::env::temp_dir().join(format!("rain-ready-{}", uuid::Uuid::new_v4()));
        let state = AppState::new(pool.clone(), root.clone(), AppLimits::default());
        assert_eq!(
            readiness_response(&state).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        db::prepare_schema(&pool, false).await.unwrap();
        assert_eq!(
            readiness_response(&state).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        state
            .readiness_cache
            .0
            .lock()
            .await
            .as_mut()
            .unwrap()
            .checked_at = std::time::Instant::now() - super::READINESS_CACHE_TTL;
        assert_eq!(readiness_response(&state).await.status(), StatusCode::OK);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[actix_web::test]
    async fn readiness_cache_is_isolated_between_app_states() {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        db::prepare_schema(&pool, false).await.unwrap();
        let root = std::env::temp_dir().join(format!("rain-ready-{}", uuid::Uuid::new_v4()));
        let healthy = AppState::new(pool.clone(), root.clone(), AppLimits::default());
        assert_eq!(readiness_response(&healthy).await.status(), StatusCode::OK);
        let missing_schema = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let bad_database = AppState::new(missing_schema, root.clone(), AppLimits::default());
        assert_eq!(
            readiness_response(&bad_database).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        let bad_root = root.join("not-a-directory");
        tokio::fs::write(&bad_root, b"file").await.unwrap();
        let unhealthy = AppState::new(pool, bad_root, AppLimits::default());
        assert_eq!(
            readiness_response(&unhealthy).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[actix_web::test]
    async fn readiness_waits_for_invariant_recovery_and_reports_its_state() {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        db::prepare_schema(&pool, false).await.unwrap();
        let data_root = std::env::temp_dir().join(format!("rain-ready-{}", uuid::Uuid::new_v4()));
        let mut app_state = AppState::new(pool, PathBuf::from(&data_root), AppLimits::default());
        app_state.recovery = std::sync::Arc::new(RecoveryRuntime::default());
        let state = web::Data::new(app_state);

        let response = readiness_response(state.as_ref()).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body()).await.unwrap()).unwrap();
        assert_eq!(body["recovery"], false);

        state.recovery.mark_stale_skill_runs_ready();
        state.recovery.mark_stale_processing_bundles_ready();
        let response = readiness_response(state.as_ref()).await;
        assert_eq!(response.status(), StatusCode::OK);

        let _ = tokio::fs::remove_dir_all(data_root).await;
    }
}
