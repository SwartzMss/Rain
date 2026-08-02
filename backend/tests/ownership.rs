use std::path::PathBuf;

use actix_web::{App, body::to_bytes, cookie::Cookie, test, web};
use chrono::{Duration, Utc};
use serde_json::Value;
use sqlx::sqlite::SqlitePoolOptions;

use backend::{
    AppState,
    auth::session::{SESSION_COOKIE_NAME, generate_session_token, hash_session_token},
    config::AppLimits,
    db,
    repositories::{
        sessions,
        users::{self, CreateUserOutcome},
    },
    routes,
};

async fn user_with_session(pool: &sqlx::SqlitePool, name: &str) -> Cookie<'static> {
    let user = match users::create_user(pool, name, "hash").await.unwrap() {
        CreateUserOutcome::Created(user) => user,
        CreateUserOutcome::DuplicateUsername => panic!("duplicate test user"),
    };
    let token = generate_session_token();
    sessions::create_session(
        pool,
        &user.id,
        &hash_session_token(&token),
        Utc::now() + Duration::hours(1),
        None,
        None,
    )
    .await
    .unwrap();
    Cookie::new(SESSION_COOKIE_NAME, token)
}

#[tokio::test]
async fn foreign_user_cannot_upload_or_delete_owned_issue() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    let owner_cookie = user_with_session(&pool, "owner-http").await;
    let foreign_cookie = user_with_session(&pool, "foreign-http").await;
    let owner_id: String = sqlx::query_scalar("SELECT id FROM users WHERE username = 'owner-http'")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO issues (code, name, owner_user_id) VALUES ('PRIVATE', 'Private', ?)")
        .bind(owner_id)
        .execute(&pool)
        .await
        .unwrap();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppState::new(
                pool,
                PathBuf::from("data"),
                AppLimits::default(),
            )))
            .configure(routes::register),
    )
    .await;

    let upload = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/issues/PRIVATE/uploads")
            .cookie(foreign_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(upload.status(), actix_web::http::StatusCode::FORBIDDEN);
    let upload_body: Value =
        serde_json::from_slice(&to_bytes(upload.into_body()).await.unwrap()).unwrap();
    assert_eq!(upload_body["code"], "ISSUE_WRITE_FORBIDDEN");

    let delete = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri("/api/issues/PRIVATE")
            .cookie(foreign_cookie)
            .to_request(),
    )
    .await;
    assert_eq!(delete.status(), actix_web::http::StatusCode::FORBIDDEN);
    let _ = owner_cookie;
}
