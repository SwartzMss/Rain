use actix_web::{HttpResponse, get, http::StatusCode, web};
use serde_json::json;
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
    let database_ok = sqlx::query("SELECT 1").execute(&state.pool).await.is_ok();
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
    if tokio::fs::create_dir_all(root.join("blobs")).await.is_err() {
        return false;
    }
    let probe = root.join(format!(".ready-{}", Uuid::new_v4().simple()));
    match tokio::fs::File::create(&probe).await {
        Ok(_) => tokio::fs::remove_file(probe).await.is_ok(),
        Err(_) => false,
    }
}
