use std::collections::HashSet;

use actix_web::{HttpResponse, http::StatusCode, post, web};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Sqlite};

use crate::{AppState, error::AppError};

const MAX_KEEP_ALIVE_RESULTS: usize = 100;

#[derive(Deserialize)]
struct KeepAliveRequest {
    result_ids: Vec<String>,
}

#[derive(Serialize)]
struct KeepAliveResponse {
    unavailable_ids: Vec<String>,
}

#[post("/temp-results/keep-alive")]
pub(crate) async fn keep_alive_temp_results(
    payload: web::Json<KeepAliveRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    if payload.result_ids.len() > MAX_KEEP_ALIVE_RESULTS
        || payload
            .result_ids
            .iter()
            .any(|id| id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(AppError::public(
            StatusCode::BAD_REQUEST,
            "TEMP_RESULT_KEEP_ALIVE_INVALID",
            "每次最多续期 100 个临时结果，结果 ID 必须为 32 位十六进制字符串",
        ));
    }
    let mut seen = HashSet::new();
    let ids: Vec<String> = payload
        .into_inner()
        .result_ids
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect();
    let renewed = renew_active_results(&state, &ids).await?;
    Ok(HttpResponse::Ok().json(KeepAliveResponse {
        unavailable_ids: ids.into_iter().filter(|id| !renewed.contains(id)).collect(),
    }))
}

async fn renew_active_results(
    state: &web::Data<AppState>,
    ids: &[String],
) -> Result<HashSet<String>, AppError> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    crate::db::write::run(&state.db.pool, "renew temporary results", ids, |conn, ids| {
        Box::pin(async move {
            // Take the clock after writer admission, including on a retry: an
            // expired result must never be revived after waiting for the writer.
            let now = Utc::now();
            let expires_at = (now + Duration::minutes(30)).to_rfc3339();
            let mut query = QueryBuilder::<Sqlite>::new(
                "UPDATE temp_results SET expires_at = CASE WHEN julianday(expires_at) < julianday(",
            );
            query.push_bind(&expires_at).push(") THEN ").push_bind(&expires_at)
                .push(" ELSE expires_at END WHERE status = 'ACTIVE' AND julianday(expires_at) > julianday(")
                .push_bind(now.to_rfc3339()).push(") AND id IN (");
            let mut separated = query.separated(", ");
            for id in ids {
                separated.push_bind(id);
            }
            separated.push_unseparated(") RETURNING id");
            query.build_query_scalar::<String>().fetch_all(conn).await
                .map(|ids| ids.into_iter().collect()).map_err(AppError::Database)
        })
    }).await
}

#[cfg(test)]
mod tests {
    use actix_web::{App, http::StatusCode, test, web};
    use chrono::{Duration, Utc};
    use serde_json::{Value, json};
    use sqlx::sqlite::SqlitePoolOptions;

    use super::super::repository::{
        TransitionResult, claim_expired_active, find_active_unexpired_by_id,
    };
    use crate::{AppState, config::AppLimits, db};

    const ACTIVE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const MISSING: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    async fn state() -> web::Data<AppState> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        db::prepare_schema(&pool, false).await.unwrap();
        web::Data::new(AppState::new(pool, "data".into(), AppLimits::default()))
    }

    async fn insert(state: &web::Data<AppState>, id: &str, status: &str, expires: &str) {
        sqlx::query("INSERT INTO temp_results (id, status, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at) VALUES (?, ?, 'result.log', 'x', 'x', 'data/temp-results/result.log', 0, 0, ?, ?)")
            .bind(id).bind(status).bind(Utc::now().to_rfc3339()).bind(expires)
            .execute(&state.db.pool).await.unwrap();
    }

    async fn post(state: &web::Data<AppState>, body: Value) -> (StatusCode, Vec<u8>) {
        let app = test::init_service(
            App::new()
                .app_data(state.clone())
                .configure(crate::routes::register),
        )
        .await;
        let request = test::TestRequest::post()
            .uri("/api/temp-results/keep-alive")
            .set_json(body)
            .to_request();
        let response = test::call_service(&app, request).await;
        let status = response.status();
        (status, test::read_body(response).await.to_vec())
    }

    #[actix_web::test]
    async fn anonymous_keep_alive_extends_active_results_without_shortening_long_retention() {
        let state = state().await;
        insert(
            &state,
            ACTIVE,
            "ACTIVE",
            &(Utc::now() + Duration::minutes(1)).to_rfc3339(),
        )
        .await;
        let full = "cccccccccccccccccccccccccccccccc";
        let full_expiry = (Utc::now() + Duration::days(7)).to_rfc3339();
        insert(&state, full, "ACTIVE", &full_expiry).await;
        let before = Utc::now() + Duration::minutes(30);
        let (status, body) = post(&state, json!({"result_ids": [ACTIVE, full, ACTIVE]})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"unavailable_ids": []})
        );
        let record = find_active_unexpired_by_id(&state, ACTIVE).await.unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(&record.expires_at).unwrap() >= before);
        assert_eq!(
            find_active_unexpired_by_id(&state, full)
                .await
                .unwrap()
                .expires_at,
            full_expiry
        );
    }

    #[actix_web::test]
    async fn keep_alive_rejects_invalid_and_oversized_batches_before_any_write() {
        let state = state().await;
        let expiry = (Utc::now() + Duration::minutes(1)).to_rfc3339();
        insert(&state, ACTIVE, "ACTIVE", &expiry).await;
        for body in [
            json!({"result_ids": [ACTIVE, "bad"]}),
            json!({"result_ids": ["g".repeat(32)]}),
            json!({"result_ids": vec![ACTIVE; 101]}),
            json!({}),
        ] {
            assert_eq!(post(&state, body).await.0, StatusCode::BAD_REQUEST);
        }
        assert_eq!(
            find_active_unexpired_by_id(&state, ACTIVE)
                .await
                .unwrap()
                .expires_at,
            expiry
        );
        assert_eq!(
            post(&state, json!({"result_ids": vec![ACTIVE; 100]}))
                .await
                .0,
            StatusCode::OK
        );
    }

    #[actix_web::test]
    async fn keep_alive_reports_expired_nonactive_and_missing_ids_once_and_accepts_empty() {
        let state = state().await;
        let expired = (Utc::now() - Duration::minutes(1)).to_rfc3339();
        let future = (Utc::now() + Duration::minutes(1)).to_rfc3339();
        let staging = "dddddddddddddddddddddddddddddddd";
        let deleting = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        insert(&state, ACTIVE, "ACTIVE", &expired).await;
        insert(&state, staging, "STAGING", &future).await;
        insert(&state, deleting, "DELETING", &future).await;
        let (status, body) = post(
            &state,
            json!({"result_ids": [ACTIVE, staging, deleting, MISSING, MISSING]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"unavailable_ids": [ACTIVE, staging, deleting, MISSING]})
        );
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT status, expires_at FROM temp_results ORDER BY id")
                .fetch_all(&state.db.pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![
                ("ACTIVE".into(), expired),
                ("STAGING".into(), future.clone()),
                ("DELETING".into(), future)
            ]
        );
        let (status, body) = post(&state, json!({"result_ids": []})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"unavailable_ids": []})
        );
    }

    #[actix_web::test]
    async fn renewed_result_survives_original_expiry_and_stale_cleanup_claim() {
        let state = state().await;
        let original_expiry = (Utc::now() + Duration::seconds(1)).to_rfc3339();
        insert(&state, ACTIVE, "ACTIVE", &original_expiry).await;
        assert_eq!(
            post(&state, json!({"result_ids": [ACTIVE]})).await.0,
            StatusCode::OK
        );
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        assert!(Utc::now() > chrono::DateTime::parse_from_rfc3339(&original_expiry).unwrap());
        assert_eq!(
            claim_expired_active(&state, ACTIVE, &original_expiry)
                .await
                .unwrap(),
            TransitionResult::StateMismatch
        );
        let app = test::init_service(
            App::new()
                .app_data(state.clone())
                .configure(crate::routes::register),
        )
        .await;
        let response = test::call_service(
            &app,
            test::TestRequest::get()
                .uri(&format!("/api/temp-results/{ACTIVE}"))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
}
