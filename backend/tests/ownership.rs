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

#[tokio::test]
async fn inactivity_expiry_is_an_owner_only_persisted_activity_snapshot() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, true).await.unwrap();
    let owner_cookie = user_with_session(&pool, "expiry-owner").await;
    let foreign_cookie = user_with_session(&pool, "expiry-foreign").await;
    let admin_cookie = user_with_session(&pool, "expiry-admin").await;
    sqlx::query("UPDATE users SET role='ADMIN' WHERE username='expiry-admin'")
        .execute(&pool)
        .await
        .unwrap();
    let owner_id: String = sqlx::query_scalar("SELECT id FROM users WHERE username='expiry-owner'")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO issues(code,name,owner_user_id,last_activity_at) VALUES('EXPIRY','Expiry',?,datetime('now','-30 minutes')),('UNOWNED','Unowned',NULL,datetime('now','-30 minutes'))")
        .bind(owner_id)
        .execute(&pool)
        .await
        .unwrap();
    let state = web::Data::new(AppState::new(
        pool.clone(),
        std::env::temp_dir().join(format!("rain-expiry-{}", Uuid::new_v4().simple())),
        AppLimits::default(),
    ));
    state
        .issue_inactive_days
        .store(7, std::sync::atomic::Ordering::Release);
    let app = test::init_service(
        App::new()
            .app_data(state.clone())
            .configure(routes::register),
    )
    .await;

    let before: String =
        sqlx::query_scalar("SELECT last_activity_at FROM issues WHERE code='EXPIRY'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let owner_response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/EXPIRY")
            .cookie(owner_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(owner_response.status(), actix_web::http::StatusCode::OK);
    assert_eq!(
        owner_response
            .headers()
            .get(actix_web::http::header::CACHE_CONTROL),
        Some(&actix_web::http::header::HeaderValue::from_static(
            "no-store, private"
        ))
    );
    let owner_body: Value = test::read_body_json(owner_response).await;
    assert_eq!(owner_body["inactivity_expiry"]["inactive_days"], 7);
    assert_eq!(
        owner_body["inactivity_expiry"]["renewed_from_expiring"],
        false
    );
    let expires_at = owner_body["inactivity_expiry"]["expires_at"]
        .as_str()
        .unwrap();
    chrono::DateTime::parse_from_rfc3339(expires_at).unwrap();
    assert!(owner_body.get("last_activity_at").is_none());
    assert!(owner_body.get("owner_user_id").is_none());
    let after: String =
        sqlx::query_scalar("SELECT last_activity_at FROM issues WHERE code='EXPIRY'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after, before,
        "the one-hour throttle must keep persisted activity"
    );
    let expected: String = sqlx::query_scalar(
        "SELECT strftime('%Y-%m-%dT%H:%M:%SZ', datetime(last_activity_at, '+7 days')) FROM issues WHERE code='EXPIRY'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(expires_at, expected);

    for cookie in [Some(foreign_cookie), Some(admin_cookie), None] {
        let mut request = test::TestRequest::get().uri("/api/issues/EXPIRY");
        if let Some(cookie) = cookie {
            request = request.cookie(cookie);
        }
        let response = test::call_service(&app, request.to_request()).await;
        let body: Value = test::read_body_json(response).await;
        assert!(body["inactivity_expiry"].is_null());
    }

    let unowned = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/UNOWNED")
            .cookie(owner_cookie.clone())
            .to_request(),
    )
    .await;
    let unowned: Value = test::read_body_json(unowned).await;
    assert!(unowned["inactivity_expiry"].is_null());

    sqlx::query(
        "UPDATE issues SET last_activity_at=datetime('now','-6 days','-12 hours') WHERE code='EXPIRY'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let renewed = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/EXPIRY")
            .cookie(owner_cookie.clone())
            .to_request(),
    )
    .await;
    let renewed: Value = test::read_body_json(renewed).await;
    assert_eq!(renewed["inactivity_expiry"]["renewed_from_expiring"], true);
    let renewed_expires_at = renewed["inactivity_expiry"]["expires_at"].as_str().unwrap();
    let renewed_beyond_warning_window: i64 =
        sqlx::query_scalar("SELECT datetime(?) > datetime('now', '+72 hours')")
            .bind(renewed_expires_at)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(renewed_beyond_warning_window, 1);

    state
        .issue_inactive_days
        .store(0, std::sync::atomic::Ordering::Release);
    let disabled = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/EXPIRY")
            .cookie(owner_cookie.clone())
            .to_request(),
    )
    .await;
    let disabled: Value = test::read_body_json(disabled).await;
    assert!(disabled["inactivity_expiry"].is_null());

    sqlx::query(
        "UPDATE issues SET last_activity_at=datetime('now','-2 hours') WHERE code='EXPIRY'",
    )
    .execute(&pool)
    .await
    .unwrap();
    state
        .issue_inactive_days
        .store(30, std::sync::atomic::Ordering::Release);
    let refreshed = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/EXPIRY")
            .cookie(owner_cookie)
            .to_request(),
    )
    .await;
    let refreshed: Value = test::read_body_json(refreshed).await;
    assert_eq!(refreshed["inactivity_expiry"]["inactive_days"], 30);
    let refreshed_activity: String =
        sqlx::query_scalar("SELECT last_activity_at FROM issues WHERE code='EXPIRY'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let refreshed_expiry: String = sqlx::query_scalar(
        "SELECT strftime('%Y-%m-%dT%H:%M:%SZ', datetime(last_activity_at, '+30 days')) FROM issues WHERE code='EXPIRY'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_ne!(refreshed_activity, before);
    assert_eq!(
        refreshed["inactivity_expiry"]["expires_at"],
        refreshed_expiry
    );
}

#[tokio::test]
async fn committed_bundle_deletion_is_not_reported_as_failed_when_activity_touch_fails() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, true).await.unwrap();
    let owner_cookie = user_with_session(&pool, "activity-owner").await;
    let owner_id: String =
        sqlx::query_scalar("SELECT id FROM users WHERE username='activity-owner'")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO issues(code,name,owner_user_id,last_activity_at) VALUES('ACTIVITY','Activity',?,datetime('now','-6 days','-12 hours'))")
        .bind(owner_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status,process_stage) VALUES('activity-bundle','ACTIVITY','activity-hash','activity.log','READY','READY')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_activity_touch BEFORE UPDATE OF last_activity_at ON issues BEGIN SELECT RAISE(FAIL, 'forced activity failure'); END")
        .execute(&pool)
        .await
        .unwrap();
    let data_root = std::env::temp_dir().join(format!("rain-activity-{}", Uuid::new_v4().simple()));
    tokio::fs::create_dir_all(&data_root).await.unwrap();
    let state = web::Data::new(AppState::new(
        pool.clone(),
        data_root.clone(),
        AppLimits::default(),
    ));
    state
        .issue_inactive_days
        .store(7, std::sync::atomic::Ordering::Release);
    let app = test::init_service(App::new().app_data(state).configure(routes::register)).await;

    let detail = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/issues/ACTIVITY")
            .cookie(owner_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(detail.status(), actix_web::http::StatusCode::OK);
    let detail: Value = test::read_body_json(detail).await;
    assert_eq!(detail["inactivity_expiry"]["inactive_days"], 7);
    assert_eq!(detail["inactivity_expiry"]["renewed_from_expiring"], false);
    let expires_within_warning_window: i64 =
        sqlx::query_scalar("SELECT datetime(?) <= datetime('now', '+72 hours')")
            .bind(detail["inactivity_expiry"]["expires_at"].as_str().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(expires_within_warning_window, 1);

    let response = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri("/api/issues/ACTIVITY/bundles/activity-hash")
            .cookie(owner_cookie)
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), actix_web::http::StatusCode::ACCEPTED);
    for _ in 0..100 {
        let status: String =
            sqlx::query_scalar("SELECT status FROM bundles WHERE id='activity-bundle'")
                .fetch_one(&pool)
                .await
                .unwrap();
        if status == "DELETED" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let status: String =
        sqlx::query_scalar("SELECT status FROM bundles WHERE id='activity-bundle'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "DELETED");
    let _ = tokio::fs::remove_dir_all(data_root).await;
}
