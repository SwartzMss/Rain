use actix_web::{HttpResponse, get, http::StatusCode, web};
use serde_json::json;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::AppState;

const READINESS_CACHE_TTL: Duration = Duration::from_secs(5);
static READINESS_CACHE: OnceLock<Mutex<Option<ReadinessSnapshot>>> = OnceLock::new();

#[derive(Clone, Copy)]
struct ReadinessSnapshot {
    checked_at: Instant,
    database_ok: bool,
    storage_ok: bool,
}

#[get("/healthz")]
pub async fn health() -> HttpResponse {
    HttpResponse::Ok().json(json!({
        "status": "ok",
        "service": "rain-backend",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

#[get("/readyz")]
pub async fn readiness(state: web::Data<AppState>) -> HttpResponse {
    readiness_response(&state).await
}

async fn readiness_response(state: &AppState) -> HttpResponse {
    let snapshot = if let Some(snapshot) = cached_readiness() {
        snapshot
    } else {
        let snapshot = ReadinessSnapshot {
            checked_at: Instant::now(),
            database_ok: check_database(&state.db.pool).await,
            storage_ok: check_storage(&state.storage.data_root).await,
        };
        if let Ok(mut cache) = READINESS_CACHE.get_or_init(|| Mutex::new(None)).lock() {
            *cache = Some(snapshot);
        }
        snapshot
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
        "version": env!("CARGO_PKG_VERSION")
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

fn cached_readiness() -> Option<ReadinessSnapshot> {
    let cache = READINESS_CACHE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()?;
    let snapshot = (*cache)?;
    (snapshot.checked_at.elapsed() < READINESS_CACHE_TTL).then_some(snapshot)
}

async fn check_database(pool: &sqlx::SqlitePool) -> bool {
    let Ok(mut transaction) = pool.begin().await else {
        return false;
    };
    let result = async {
        sqlx::query("INSERT INTO rain_ready_probe (id, value) VALUES (?, 1)")
            .bind(Uuid::new_v4().simple().to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.rollback().await
    }
    .await;
    result.is_ok()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use actix_web::{body::to_bytes, http::StatusCode, web};
    use sqlx::sqlite::SqlitePoolOptions;

    use super::{READINESS_CACHE, readiness_response};
    use crate::{AppState, RecoveryRuntime, config::AppLimits, db};

    #[actix_web::test]
    async fn readiness_waits_for_invariant_recovery_and_reports_its_state() {
        if let Ok(mut cache) = READINESS_CACHE
            .get_or_init(|| std::sync::Mutex::new(None))
            .lock()
        {
            *cache = None;
        }
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
use std::{
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
