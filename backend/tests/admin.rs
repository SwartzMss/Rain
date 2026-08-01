use actix_web::{App, cookie::Cookie, http::StatusCode, test, web};
use backend::{
    AppState,
    auth::{
        UserRole, UserStatus,
        session::{SESSION_COOKIE_NAME, generate_session_token, hash_session_token},
    },
    config::AppLimits,
    db,
    repositories::{bootstrap_admin, sessions},
    routes,
};
use chrono::{Duration, Utc};
use std::path::PathBuf;

#[tokio::test]
async fn bootstrap_creates_exactly_one_active_admin_and_audit_record() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");

    bootstrap_admin::bootstrap_admin(&pool, "admin", "strong-password")
        .await
        .expect("bootstrap");
    bootstrap_admin::bootstrap_admin(&pool, "ignored", "another-password")
        .await
        .expect("idempotent bootstrap");

    let users: Vec<(String, String)> = sqlx::query_as("SELECT role, status FROM users")
        .fetch_all(&pool)
        .await
        .expect("users");
    assert_eq!(
        users,
        vec![(UserRole::Admin.to_string(), UserStatus::Active.to_string())]
    );
    let actions: Vec<String> = sqlx::query_scalar("SELECT action FROM admin_audit_logs")
        .fetch_all(&pool)
        .await
        .expect("audit");
    assert_eq!(actions, vec!["ADMIN_BOOTSTRAPPED"]);
}

#[tokio::test]
async fn bootstrap_rejects_missing_credentials_and_username_conflicts() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
    let missing = bootstrap_admin::bootstrap_admin(&pool, "admin", "")
        .await
        .expect_err("missing password");
    assert!(missing.to_string().contains("BOOTSTRAP_ADMIN_REQUIRED"));

    sqlx::query("INSERT INTO users (id, username, username_normalized, password_hash) VALUES ('u', 'admin', 'admin', 'hash')")
        .execute(&pool)
        .await
        .expect("ordinary user");
    let conflict = bootstrap_admin::bootstrap_admin(&pool, "admin", "strong-password")
        .await
        .expect_err("conflict");
    assert!(conflict.to_string().contains("BOOTSTRAP_ADMIN_CONFLICT"));
}

#[tokio::test]
async fn startup_rejects_multiple_or_disabled_administrators() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
    bootstrap_admin::bootstrap_admin(&pool, "admin", "strong-password")
        .await
        .expect("bootstrap");
    sqlx::query("DROP INDEX idx_users_single_admin")
        .execute(&pool)
        .await
        .expect("simulate externally corrupted database");
    sqlx::query("INSERT INTO users (id, username, username_normalized, password_hash, role, status) VALUES ('second-admin', 'second-admin', 'second-admin', 'hash', 'ADMIN', 'ACTIVE')").execute(&pool).await.expect("second admin");
    let multiple = bootstrap_admin::bootstrap_admin(&pool, "admin", "strong-password")
        .await
        .expect_err("multiple administrators");
    assert!(multiple.to_string().contains("ADMIN_INVARIANT_VIOLATION"));
    sqlx::query("DELETE FROM users WHERE id = 'second-admin'")
        .execute(&pool)
        .await
        .expect("remove second");
    let mut corrupted_connection = pool.acquire().await.expect("corruption connection");
    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(&mut *corrupted_connection)
        .await
        .expect("simulate externally corrupted status");
    sqlx::query("UPDATE users SET status = 'DISABLED' WHERE role = 'ADMIN'")
        .execute(&mut *corrupted_connection)
        .await
        .expect("disable admin");
    drop(corrupted_connection);
    let disabled = bootstrap_admin::bootstrap_admin(&pool, "admin", "strong-password")
        .await
        .expect_err("disabled administrator");
    assert!(disabled.to_string().contains("ADMIN_INVARIANT_VIOLATION"));
}

#[tokio::test]
async fn schema_rejects_unknown_role_and_status() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
    let role = sqlx::query("INSERT INTO users (id, username, username_normalized, password_hash, role) VALUES ('r', 'role', 'role', 'hash', 'ROOT')")
        .execute(&pool)
        .await;
    assert!(role.is_err());
    let status = sqlx::query("INSERT INTO users (id, username, username_normalized, password_hash, status) VALUES ('s', 'status', 'status', 'hash', 'LOCKED')")
        .execute(&pool)
        .await;
    assert!(status.is_err());

    sqlx::query("INSERT INTO users (id, username, username_normalized, password_hash, role, status) VALUES ('a', 'admin', 'admin', 'hash', 'ADMIN', 'ACTIVE')")
        .execute(&pool)
        .await
        .expect("first administrator");
    let second_admin = sqlx::query("INSERT INTO users (id, username, username_normalized, password_hash, role, status) VALUES ('a2', 'admin2', 'admin2', 'hash', 'ADMIN', 'ACTIVE')")
        .execute(&pool)
        .await;
    assert!(second_admin.is_err());
    let disabled_admin = sqlx::query("UPDATE users SET status = 'DISABLED' WHERE id = 'a'")
        .execute(&pool)
        .await;
    assert!(disabled_admin.is_err());
}

#[actix_web::test]
async fn admin_api_lists_users_and_protects_self_from_disable() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
    bootstrap_admin::bootstrap_admin(&pool, "admin", "strong-password")
        .await
        .expect("bootstrap");
    let admin_id: String = sqlx::query_scalar("SELECT id FROM users WHERE role='ADMIN'")
        .fetch_one(&pool)
        .await
        .expect("admin");
    let token = generate_session_token();
    sessions::create_session(
        &pool,
        &admin_id,
        &hash_session_token(&token),
        Utc::now() + Duration::hours(1),
        None,
        None,
    )
    .await
    .expect("session");
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
    let cookie = Cookie::new(SESSION_COOKIE_NAME, token);
    let list = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/admin/users")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let body: serde_json::Value = test::read_body_json(list).await;
    assert_eq!(body["items"].as_array().expect("items").len(), 0);
    let disable = test::call_service(
        &app,
        test::TestRequest::patch()
            .uri(&format!("/api/admin/users/{admin_id}/status"))
            .cookie(cookie.clone())
            .set_json(serde_json::json!({"status":"DISABLED"}))
            .to_request(),
    )
    .await;
    assert_eq!(disable.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = test::read_body_json(disable).await;
    assert_eq!(body["code"], "IMMUTABLE_ADMIN_ACCOUNT");

    let revoke = test::call_service(
        &app,
        test::TestRequest::post()
            .uri(&format!("/api/admin/users/{admin_id}/revoke-sessions"))
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(revoke.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = test::read_body_json(revoke).await;
    assert_eq!(body["code"], "IMMUTABLE_ADMIN_ACCOUNT");

    let role = test::call_service(
        &app,
        test::TestRequest::patch()
            .uri(&format!("/api/admin/users/{admin_id}/role"))
            .cookie(cookie.clone())
            .set_json(serde_json::json!({"role":"USER"}))
            .to_request(),
    )
    .await;
    assert_eq!(role.status(), StatusCode::NOT_FOUND);

    let business_write = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/issues")
            .cookie(cookie)
            .set_json(serde_json::json!({"code": "ADMIN_WRITE"}))
            .to_request(),
    )
    .await;
    assert_eq!(business_write.status(), StatusCode::FORBIDDEN);
    let body: serde_json::Value = test::read_body_json(business_write).await;
    assert_eq!(body["code"], "BUSINESS_USER_REQUIRED");
}
