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
    let ready = database_ok && storage_ok;
    HttpResponse::build(if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    })
    .json(json!({
        "status": if ready { "ready" } else { "not_ready" },
        "database": database_ok,
        "storage": storage_ok,
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
use std::{
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
