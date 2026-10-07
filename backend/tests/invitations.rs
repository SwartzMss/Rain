use std::path::PathBuf;

use actix_web::{App, cookie::Cookie, http::StatusCode, test, web};
use backend::{
    AppState,
    config::{AppLimits, AuthConfig},
    db,
    repositories::{bootstrap_admin, invitations},
    routes,
};
use serde_json::{Value, json};
use sqlx::SqlitePool;

async fn initialized_pool() -> SqlitePool {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
    bootstrap_admin::bootstrap_admin(&pool, "admin", "strong-password")
        .await
        .expect("bootstrap admin");
    pool
}

#[actix_web::test]
async fn registration_preflight_rejects_invalid_requests_before_argon2() {
    let pool = initialized_pool().await;
    let state = web::Data::new(AppState::new(
        pool.clone(),
        PathBuf::from("data"),
        AppLimits::default(),
    ));
    state
        .settings
        .initialize(&AppLimits::default(), &AuthConfig::default(), 0, None)
        .await
        .expect("initialize settings");

    let valid_code = "PREFLIGHT-VALID-CODE";
    let admin_id: String =
        sqlx::query_scalar("SELECT id FROM users WHERE username_normalized='admin'")
            .fetch_one(&pool)
            .await
            .expect("bootstrap admin id");
    sqlx::query("INSERT INTO invitations(id,batch_id,code_hash,created_by) VALUES(?,?,?,?)")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(invitations::code_hash(valid_code).expect("valid code hash"))
        .bind(admin_id)
        .execute(&pool)
        .await
        .expect("insert invitation");

    let app = test::init_service(
        App::new()
            .app_data(state.clone())
            .configure(routes::register),
    )
    .await;
    let _argon2_permits = state
        .auth_runtime
        .hash_permits
        .clone()
        .acquire_many_owned(state.auth_runtime.config.argon2_concurrency as u32)
        .await
        .expect("acquire all argon2 permits");

    let missing = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username":"preflight-missing","password":"password123"}))
            .to_request(),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
    let missing: Value = test::read_body_json(missing).await;
    assert_eq!(missing["code"], "INVITE_CODE_REQUIRED");

    let invalid = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username":"preflight-invalid","password":"password123","invite_code":"not-a-valid-code"}))
            .to_request(),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let invalid: Value = test::read_body_json(invalid).await;
    assert_eq!(invalid["code"], "INVITE_CODE_INVALID");

    let valid = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username":"preflight-valid","password":"password123","invite_code":valid_code}))
            .to_request(),
    )
    .await;
    assert_eq!(valid.status(), StatusCode::TOO_MANY_REQUESTS);
    let valid: Value = test::read_body_json(valid).await;
    assert_eq!(valid["code"], "TOO_MANY_REQUESTS");

    sqlx::query("UPDATE system_settings SET allow_registration=0 WHERE id=1")
        .execute(&pool)
        .await
        .expect("close registration");
    let closed = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username":"preflight-closed","password":"password123"}))
            .to_request(),
    )
    .await;
    assert_eq!(closed.status(), StatusCode::FORBIDDEN);
    let closed: Value = test::read_body_json(closed).await;
    assert_eq!(closed["code"], "REGISTRATION_DISABLED");
}

#[actix_web::test]
async fn invitation_registration_is_one_time_and_failures_do_not_consume_codes() {
    let pool = initialized_pool().await;
    let state = web::Data::new(AppState::new(
        pool.clone(),
        PathBuf::from("data"),
        AppLimits::default(),
    ));
    state
        .settings
        .initialize(&AppLimits::default(), &AuthConfig::default(), 0, None)
        .await
        .expect("initialize settings");
    let app = test::init_service(App::new().app_data(state).configure(routes::register)).await;
    let login = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/login")
            .set_json(json!({"username":"admin","password":"strong-password"}))
            .to_request(),
    )
    .await;
    assert_eq!(login.status(), StatusCode::OK);
    let admin_cookie = Cookie::parse(
        login
            .headers()
            .get(actix_web::http::header::SET_COOKIE)
            .expect("session cookie")
            .to_str()
            .expect("cookie header"),
    )
    .expect("parse cookie")
    .into_owned();

    let unauthenticated = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/admin/invitations")
            .to_request(),
    )
    .await;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let created = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/admin/invitations")
            .cookie(admin_cookie.clone())
            .set_json(json!({"count":2,"validity_days":7,"note":"test batch"}))
            .to_request(),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(
        created
            .headers()
            .get(actix_web::http::header::CACHE_CONTROL)
            .expect("cache policy"),
        "no-store"
    );
    let created: Value = test::read_body_json(created).await;
    let codes = created["invitations"].as_array().expect("codes");
    assert_eq!(codes.len(), 2);
    let code_one = codes[0]["code"].as_str().expect("first code").to_owned();
    let code_two = codes[1]["code"].as_str().expect("second code").to_owned();

    let stored_hash: String = sqlx::query_scalar("SELECT code_hash FROM invitations WHERE id=?")
        .bind(codes[0]["id"].as_str().expect("invitation id"))
        .fetch_one(&pool)
        .await
        .expect("stored digest");
    assert_eq!(Some(stored_hash), invitations::code_hash(&code_one));

    let weak_password = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(
                json!({"username":"weak-password-user","password":"short","invite_code":code_two}),
            )
            .to_request(),
    )
    .await;
    assert_eq!(weak_password.status(), StatusCode::BAD_REQUEST);
    let weak_body: Value = test::read_body_json(weak_password).await;
    assert_eq!(weak_body["code"], "PASSWORD_TOO_WEAK");

    let registered = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username":"alice","password":"password123","invite_code":code_one}))
            .to_request(),
    )
    .await;
    assert_eq!(registered.status(), StatusCode::CREATED);
    assert!(registered.response().cookies().next().is_none());

    let registered_after_retry = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username":"bob","password":"password123","invite_code":code_two}))
            .to_request(),
    )
    .await;
    assert_eq!(registered_after_retry.status(), StatusCode::CREATED);

    let replay = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username":"charlie","password":"password123","invite_code":code_one}))
            .to_request(),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
    let replay_body: Value = test::read_body_json(replay).await;
    assert_eq!(replay_body["code"], "INVITE_CODE_INVALID");

    let page = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/admin/invitations?status=USED")
            .cookie(admin_cookie)
            .to_request(),
    )
    .await;
    assert_eq!(page.status(), StatusCode::OK);
    let page: Value = test::read_body_json(page).await;
    assert_eq!(page["items"].as_array().expect("items").len(), 2);
    assert!(!page.to_string().contains(&code_one));
    assert!(!page.to_string().contains(&code_two));
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM admin_audit_logs WHERE action='INVITATION_REDEEMED'",
    )
    .fetch_one(&pool)
    .await
    .expect("redemption audits");
    assert_eq!(audit_count, 2);
}

#[actix_web::test]
async fn one_invitation_cannot_create_two_users_under_concurrent_registration() {
    let pool = initialized_pool().await;
    let state = web::Data::new(AppState::new(
        pool.clone(),
        PathBuf::from("data"),
        AppLimits::default(),
    ));
    state
        .settings
        .initialize(&AppLimits::default(), &AuthConfig::default(), 0, None)
        .await
        .expect("initialize settings");
    let app = test::init_service(App::new().app_data(state).configure(routes::register)).await;
    let login = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/login")
            .set_json(json!({"username":"admin","password":"strong-password"}))
            .to_request(),
    )
    .await;
    assert_eq!(login.status(), StatusCode::OK);
    let admin_cookie = Cookie::parse(
        login
            .headers()
            .get(actix_web::http::header::SET_COOKIE)
            .expect("session cookie")
            .to_str()
            .expect("cookie header"),
    )
    .expect("parse cookie")
    .into_owned();
    let created = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/admin/invitations")
            .cookie(admin_cookie)
            .set_json(json!({"count":1,"validity_days":null}))
            .to_request(),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: Value = test::read_body_json(created).await;
    assert!(created["invitations"][0]["expires_at"].is_null());
    let code = created["invitations"][0]["code"]
        .as_str()
        .expect("invitation code")
        .to_owned();

    let first_request = test::TestRequest::post()
        .uri("/api/auth/register")
        .set_json(
            json!({"username":"racer-one","password":"password123","invite_code":code.clone()}),
        )
        .to_request();
    let second_request = test::TestRequest::post()
        .uri("/api/auth/register")
        .set_json(json!({"username":"racer-two","password":"password123","invite_code":code}))
        .to_request();
    let (first, second) = tokio::join!(
        test::call_service(&app, first_request),
        test::call_service(&app, second_request),
    );
    let statuses = [first.status(), second.status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CREATED)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::BAD_REQUEST)
            .count(),
        1
    );
    let users: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM users WHERE username_normalized IN ('racer-one','racer-two')",
    )
    .fetch_one(&pool)
    .await
    .expect("racer accounts");
    assert_eq!(users, 1);
}

#[actix_web::test]
async fn required_codes_expire_and_revocation_is_idempotent() {
    let pool = initialized_pool().await;
    let state = web::Data::new(AppState::new(
        pool.clone(),
        PathBuf::from("data"),
        AppLimits::default(),
    ));
    state
        .settings
        .initialize(&AppLimits::default(), &AuthConfig::default(), 0, None)
        .await
        .expect("initialize settings");
    let app = test::init_service(App::new().app_data(state).configure(routes::register)).await;

    let registration_status = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/registration-status")
            .to_request(),
    )
    .await;
    assert!(
        registration_status
            .headers()
            .get(actix_web::http::header::CACHE_CONTROL)
            .expect("registration status cache policy")
            .to_str()
            .expect("cache policy header")
            .contains("no-store")
    );
    let registration_status: Value = test::read_body_json(registration_status).await;
    assert_eq!(registration_status["registration_mode"], "INVITE_ONLY");
    assert_eq!(registration_status["requires_invite_code"], true);

    let missing_code = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username":"no-code","password":"password123"}))
            .to_request(),
    )
    .await;
    assert_eq!(missing_code.status(), StatusCode::BAD_REQUEST);
    let missing_body: Value = test::read_body_json(missing_code).await;
    assert_eq!(missing_body["code"], "INVITE_CODE_REQUIRED");

    let login = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/login")
            .set_json(json!({"username":"admin","password":"strong-password"}))
            .to_request(),
    )
    .await;
    assert_eq!(login.status(), StatusCode::OK);
    let admin_cookie = Cookie::parse(
        login
            .headers()
            .get(actix_web::http::header::SET_COOKIE)
            .expect("session cookie")
            .to_str()
            .expect("cookie header"),
    )
    .expect("parse cookie")
    .into_owned();
    let created = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/admin/invitations")
            .cookie(admin_cookie.clone())
            .set_json(json!({"count":2,"validity_days":null}))
            .to_request(),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: Value = test::read_body_json(created).await;
    let invitations = created["invitations"].as_array().expect("invitations");
    let revoked_id = invitations[0]["id"].as_str().expect("revoked id");
    let revoked_code = invitations[0]["code"].as_str().expect("revoked code");
    let expired_id = invitations[1]["id"].as_str().expect("expired id");
    let expired_code = invitations[1]["code"].as_str().expect("expired code");

    for _ in 0..2 {
        let revoked = test::call_service(
            &app,
            test::TestRequest::post()
                .uri(&format!("/api/admin/invitations/{revoked_id}/revoke"))
                .cookie(admin_cookie.clone())
                .to_request(),
        )
        .await;
        assert_eq!(revoked.status(), StatusCode::NO_CONTENT);
    }
    sqlx::query("UPDATE invitations SET expires_at=strftime('%Y-%m-%dT%H:%M:%f+00:00','now','-1 second') WHERE id=?")
        .bind(expired_id)
        .execute(&pool)
        .await
        .expect("expire invitation");
    let expired_revoke = test::call_service(
        &app,
        test::TestRequest::post()
            .uri(&format!("/api/admin/invitations/{expired_id}/revoke"))
            .cookie(admin_cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(expired_revoke.status(), StatusCode::CONFLICT);

    for (username, code) in [
        ("revoked-user", revoked_code),
        ("expired-user", expired_code),
    ] {
        let registration = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/api/auth/register")
                .set_json(json!({"username":username,"password":"password123","invite_code":code}))
                .to_request(),
        )
        .await;
        assert_eq!(registration.status(), StatusCode::BAD_REQUEST);
        let body: Value = test::read_body_json(registration).await;
        assert_eq!(body["code"], "INVITE_CODE_INVALID");
    }

    sqlx::query("UPDATE system_settings SET registration_requires_invite=0 WHERE id=1")
        .execute(&pool)
        .await
        .expect("open registration mode");
    let open_mode_invitation = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/admin/invitations")
            .cookie(admin_cookie.clone())
            .set_json(json!({"count":1,"validity_days":null}))
            .to_request(),
    )
    .await;
    let open_mode_invitation: Value = test::read_body_json(open_mode_invitation).await;
    let open_mode_item = &open_mode_invitation["invitations"][0];
    let open_mode_id = open_mode_item["id"].as_str().expect("open-mode invite id");
    let open_mode_code = open_mode_item["code"]
        .as_str()
        .expect("open-mode invite code");
    let open_registration = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username":"open-mode-user","password":"password123","invite_code":open_mode_code}))
            .to_request(),
    )
    .await;
    assert_eq!(open_registration.status(), StatusCode::CREATED);
    let open_mode_used_at: Option<String> =
        sqlx::query_scalar("SELECT used_at FROM invitations WHERE id=?")
            .bind(open_mode_id)
            .fetch_one(&pool)
            .await
            .expect("open-mode invite state");
    assert!(open_mode_used_at.is_none());

    let closed_mode_invitation = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/admin/invitations")
            .cookie(admin_cookie)
            .set_json(json!({"count":1,"validity_days":null}))
            .to_request(),
    )
    .await;
    let closed_mode_invitation: Value = test::read_body_json(closed_mode_invitation).await;
    let closed_mode_item = &closed_mode_invitation["invitations"][0];
    let closed_mode_id = closed_mode_item["id"]
        .as_str()
        .expect("closed-mode invite id");
    let closed_mode_code = closed_mode_item["code"]
        .as_str()
        .expect("closed-mode invite code");
    sqlx::query("UPDATE system_settings SET allow_registration=0 WHERE id=1")
        .execute(&pool)
        .await
        .expect("close registration mode");
    let closed_registration = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username":"closed-mode-user","password":"password123","invite_code":closed_mode_code}))
            .to_request(),
    )
    .await;
    assert_eq!(closed_registration.status(), StatusCode::FORBIDDEN);
    let closed_mode_used_at: Option<String> =
        sqlx::query_scalar("SELECT used_at FROM invitations WHERE id=?")
            .bind(closed_mode_id)
            .fetch_one(&pool)
            .await
            .expect("closed-mode invite state");
    assert!(closed_mode_used_at.is_none());

    let revoked_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM admin_audit_logs WHERE action='INVITATION_REVOKED'",
    )
    .fetch_one(&pool)
    .await
    .expect("revocation audit count");
    assert_eq!(revoked_audits, 1);
}
