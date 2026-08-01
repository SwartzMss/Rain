use actix_web::{HttpResponse, get, http::StatusCode, web};
use serde_json::json;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::AppState;

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
    let database_ok = check_database(&state.pool).await;
    let storage_ok = check_storage(&state.data_root).await;
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
    let blobs = root.join("blobs");
    if tokio::fs::create_dir_all(&blobs).await.is_err() {
        return false;
    }
    let probe = blobs.join(format!(".ready-{}", Uuid::new_v4().simple()));
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
    let Ok(mut transaction) = pool.begin().await else {
        return false;
    };
    let result = async {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS rain_ready_probe (id TEXT PRIMARY KEY, value INTEGER NOT NULL)",
        )
            .execute(&mut *transaction)
            .await?;
        sqlx::query("INSERT INTO rain_ready_probe (id, value) VALUES (?, 1)")
            .bind(Uuid::new_v4().simple().to_string())
            .execute(&mut *transaction)
            .await?;
        transaction.rollback().await
    }
    .await;
    result.is_ok()
}
