use actix_web::{HttpResponse, get};
use serde_json::json;

#[get("/healthz")]
pub async fn health() -> HttpResponse {
    HttpResponse::Ok().json(json!({
        "status": "ok",
        "service": "rain-backend",
        "version": env!("CARGO_PKG_VERSION")
    }))
}
