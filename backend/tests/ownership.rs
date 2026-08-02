use actix_web::{App, body::to_bytes, cookie::Cookie, test, web};
use chrono::{Duration, Utc};
use serde_json::Value;
use sqlx::sqlite::SqlitePoolOptions;
use uuid::Uuid;

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
    sqlx::query("INSERT INTO bundles (id, issue_code, hash, name, status, process_stage) VALUES ('bundle-private', 'PRIVATE', 'hash-private', 'private.log', 'READY', 'READY')")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO files (bundle_id, name, path, is_dir, size_bytes, line_count, status) VALUES ('bundle-private', 'private.log', 'private.log', 0, 4, 1, 'READY')")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO blobs (content_hash, size_bytes, storage_backend, storage_key, state) VALUES ('hash-content', 4, 'local', 'blobs/ha/hash-content', 'READY')")
        .execute(&pool).await.unwrap();
    sqlx::query("UPDATE files SET blob_id = 1 WHERE bundle_id = 'bundle-private' AND id = 1")
        .execute(&pool)
        .await
        .unwrap();
    let data_root =
        std::env::temp_dir().join(format!("rain-ownership-{}", Uuid::new_v4().simple()));
    tokio::fs::create_dir_all(data_root.join("blobs/ha"))
        .await
        .unwrap();
    tokio::fs::write(data_root.join("blobs/ha/hash-content"), b"log\n")
        .await
        .unwrap();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppState::new(
                pool,
                data_root.clone(),
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
    let read_file = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/files/v1/hash-private/files/1")
            .cookie(foreign_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(read_file.status(), actix_web::http::StatusCode::OK);
    let download = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/files/v1/hash-private/files/1/download")
            .cookie(foreign_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(download.status(), actix_web::http::StatusCode::OK);
    let search = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/log/v2/hash-private/search?q=log")
            .cookie(foreign_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(search.status(), actix_web::http::StatusCode::OK);
    let issue_list = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/issues")
            .cookie(foreign_cookie.clone())
            .to_request(),
    )
    .await;
    let issue_list: Value = test::read_body_json(issue_list).await;
    assert_eq!(issue_list[0]["owner_username"], "owner-http");
    let guest_list = test::call_service(
        &app,
        test::TestRequest::get().uri("/api/issues").to_request(),
    )
    .await;
    let guest_list: Value = test::read_body_json(guest_list).await;
    assert!(guest_list[0]["owner_username"].is_null());
    let issue_detail = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/PRIVATE")
            .cookie(foreign_cookie.clone())
            .to_request(),
    )
    .await;
    let issue_detail: Value = test::read_body_json(issue_detail).await;
    assert_eq!(issue_detail["owner_username"], "owner-http");
    let guest_detail = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/PRIVATE")
            .to_request(),
    )
    .await;
    let guest_detail: Value = test::read_body_json(guest_detail).await;
    assert!(guest_detail["owner_username"].is_null());
    let _ = tokio::fs::remove_dir_all(data_root).await;

    let delete = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri("/api/issues/PRIVATE")
            .cookie(foreign_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(delete.status(), actix_web::http::StatusCode::FORBIDDEN);
    let delete_bundle = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri("/api/issues/PRIVATE/bundles/hash-private")
            .cookie(foreign_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(
        delete_bundle.status(),
        actix_web::http::StatusCode::FORBIDDEN
    );
    let delete_file = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri("/api/files/v1/hash-private/files/1")
            .cookie(foreign_cookie)
            .to_request(),
    )
    .await;
    assert_eq!(delete_file.status(), actix_web::http::StatusCode::FORBIDDEN);
    let _ = owner_cookie;
}
