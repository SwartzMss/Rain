use std::path::PathBuf;

use actix_web::{App, http::StatusCode, test, web};
use backend::{AppState, config::AppLimits, db, routes};
use serde_json::{Value, json};
use sqlx::sqlite::SqlitePoolOptions;
use uuid::Uuid;

async fn test_state() -> web::Data<AppState> {
    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect("sqlite::memory:")
        .await
        .expect("database");
    db::prepare_schema(&pool, false).await.expect("schema");
    web::Data::new(AppState::new(
        pool,
        PathBuf::from("data"),
        AppLimits::default(),
    ))
}

#[actix_web::test]
async fn guest_can_reserve_and_cancel_with_capability_only() {
    let state = test_state().await;
    let app = test::init_service(App::new().app_data(state).configure(routes::register)).await;
    let search_id = Uuid::new_v4().to_string();
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/search-requests")
            .set_json(json!({ "search_id": search_id }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response.headers().get("cache-control").unwrap(),
        "no-store, private"
    );
    let reservation: Value = test::read_body_json(response).await;
    let token = reservation["cancel_token"].as_str().expect("capability");

    let response = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri(&format!("/api/search-requests/{search_id}"))
            .insert_header(("X-Search-Cancel-Token", token))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = test::read_body_json(response).await;
    assert_eq!(body["status"], "cancelled");

    let response = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri(&format!("/api/search-requests/{search_id}"))
            .insert_header(("X-Search-Cancel-Token", "wrong-token"))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[actix_web::test]
async fn duplicate_ids_and_guest_capacity_do_not_overwrite_reservations() {
    let state = test_state().await;
    let app = test::init_service(App::new().app_data(state).configure(routes::register)).await;
    let first_id = Uuid::new_v4().to_string();
    let first = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/search-requests")
            .set_json(json!({ "search_id": first_id }))
            .to_request(),
    )
    .await;
    assert_eq!(first.status(), StatusCode::CREATED);

    let duplicate = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/search-requests")
            .set_json(json!({ "search_id": first_id }))
            .to_request(),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    let second = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/search-requests")
            .set_json(json!({ "search_id": Uuid::new_v4().to_string() }))
            .to_request(),
    )
    .await;
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[actix_web::test]
async fn cancelled_reservation_cannot_late_start_preview() {
    let state = test_state().await;
    let app = test::init_service(App::new().app_data(state).configure(routes::register)).await;
    let search_id = Uuid::new_v4().to_string();
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/search-requests")
            .set_json(json!({ "search_id": search_id }))
            .to_request(),
    )
    .await;
    let reservation: Value = test::read_body_json(response).await;
    let token = reservation["cancel_token"].as_str().expect("capability");
    let cancelled = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri(&format!("/api/search-requests/{search_id}"))
            .insert_header(("X-Search-Cancel-Token", token))
            .to_request(),
    )
    .await;
    assert_eq!(cancelled.status(), StatusCode::OK);

    let preview = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/temp-results/preview")
            .insert_header(("X-Search-Cancel-Token", token))
            .set_json(
                json!({ "search_id": search_id, "expression": "ERROR", "issue_code": "MISSING" }),
            )
            .to_request(),
    )
    .await;
    assert_eq!(preview.status(), StatusCode::CONFLICT);
}

#[actix_web::test]
async fn unknown_cancel_requests_have_an_independent_bounded_rate_limit() {
    let state = test_state().await;
    let app = test::init_service(App::new().app_data(state).configure(routes::register)).await;
    let search_id = Uuid::new_v4();
    for _ in 0..120 {
        let response = test::call_service(
            &app,
            test::TestRequest::delete()
                .uri(&format!("/api/search-requests/{search_id}"))
                .insert_header(("X-Search-Cancel-Token", "invalid"))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
    let response = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri(&format!("/api/search-requests/{search_id}"))
            .insert_header(("X-Search-Cancel-Token", "invalid"))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}
