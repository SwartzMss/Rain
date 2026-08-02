use std::path::PathBuf;

use actix_web::{
    App,
    cookie::Cookie,
    http::{StatusCode, header},
    middleware::from_fn,
    test, web,
};
use backend::{AppState, config::AppLimits, db, routes};
use serde_json::{Value, json};

#[actix_web::test]
async fn session_dependent_responses_are_not_cacheable() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
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

    let guest_me = test::call_service(
        &app,
        test::TestRequest::get().uri("/api/auth/me").to_request(),
    )
    .await;
    assert_eq!(
        guest_me.headers().get(header::CACHE_CONTROL),
        Some(&header::HeaderValue::from_static("no-store, private"))
    );

    let register = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username": "CacheUser", "password": "password123"}))
            .to_request(),
    )
    .await;
    assert_eq!(register.status(), StatusCode::CREATED);
    let login = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/login")
            .set_json(json!({"username": "CacheUser", "password": "password123"}))
            .to_request(),
    )
    .await;
    let cookie = login
        .response()
        .cookies()
        .next()
        .expect("session cookie")
        .into_owned();

    let authenticated_me = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/me")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(
        authenticated_me.headers().get(header::CACHE_CONTROL),
        Some(&header::HeaderValue::from_static("no-store, private"))
    );

    let change_error = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/change-password")
            .cookie(cookie.clone())
            .set_json(json!({"current_password": "short", "new_password": "new-password123"}))
            .to_request(),
    )
    .await;
    assert_eq!(change_error.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        change_error.headers().get(header::CACHE_CONTROL),
        Some(&header::HeaderValue::from_static("no-store, private"))
    );

    let saved_searches = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/me/saved-searches")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(saved_searches.status(), StatusCode::OK);
    assert_eq!(
        saved_searches.headers().get(header::CACHE_CONTROL),
        Some(&header::HeaderValue::from_static("no-store, private"))
    );

    let health =
        test::call_service(&app, test::TestRequest::get().uri("/healthz").to_request()).await;
    assert_eq!(health.status(), StatusCode::OK);
    assert!(health.headers().get(header::CACHE_CONTROL).is_none());
}

#[actix_web::test]
async fn registration_login_me_and_logout_follow_the_public_contract() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
    let state = web::Data::new(AppState::new(
        pool.clone(),
        PathBuf::from("data"),
        AppLimits::default(),
    ));
    let app = test::init_service(
        App::new()
            .app_data(state.clone())
            .configure(routes::register),
    )
    .await;

    let guest_me = test::call_service(
        &app,
        test::TestRequest::get().uri("/api/auth/me").to_request(),
    )
    .await;
    assert_eq!(guest_me.status(), StatusCode::OK);
    let guest: Value = test::read_body_json(guest_me).await;
    assert_eq!(guest, json!({"authenticated": false, "user": null}));

    let register = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username": "Swartz", "password": "password123"}))
            .to_request(),
    )
    .await;
    assert_eq!(register.status(), StatusCode::CREATED);
    assert!(register.response().cookies().next().is_none());

    let duplicate = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username": "swartz", "password": "password123"}))
            .to_request(),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    let duplicate_body: Value = test::read_body_json(duplicate).await;
    assert_eq!(duplicate_body["code"], "USERNAME_ALREADY_EXISTS");

    for credentials in [
        json!({"username": "missing", "password": "password123"}),
        json!({"username": "Swartz", "password": "wrong-password"}),
    ] {
        let invalid = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/api/auth/login")
                .set_json(credentials)
                .to_request(),
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
        let body: Value = test::read_body_json(invalid).await;
        assert_eq!(
            body,
            json!({"code": "INVALID_CREDENTIALS", "message": "用户名或密码错误"})
        );
    }

    let username_failures_before_success = state
        .auth_runtime
        .rate_limits
        .lock()
        .expect("rate limits")
        .login_username_failure
        .get("login:username:swartz")
        .map_or(0, backend::AuthRateLimitBucket::len);
    let login = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/login")
            .set_json(json!({"username": "SWARTZ", "password": "password123"}))
            .to_request(),
    )
    .await;
    assert_eq!(login.status(), StatusCode::OK);
    let set_cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .expect("set-cookie")
        .to_str()
        .expect("cookie text")
        .to_owned();
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Lax"));
    assert!(!set_cookie.contains("Secure"));
    assert_eq!(
        state
            .auth_runtime
            .rate_limits
            .lock()
            .expect("rate limits")
            .login_username_failure
            .get("login:username:swartz")
            .map_or(0, backend::AuthRateLimitBucket::len),
        username_failures_before_success
    );
    let cookie = Cookie::parse(set_cookie)
        .expect("parse cookie")
        .into_owned();

    let stored_hash: String = sqlx::query_scalar("SELECT token_hash FROM user_sessions LIMIT 1")
        .fetch_one(&pool)
        .await
        .expect("stored token hash");
    assert_ne!(stored_hash, cookie.value());

    let authenticated_me = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/me")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(authenticated_me.status(), StatusCode::OK);
    let authenticated: Value = test::read_body_json(authenticated_me).await;
    assert_eq!(authenticated["authenticated"], true);
    assert_eq!(authenticated["user"]["username"], "Swartz");

    let logout = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/logout")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert!(
        logout
            .headers()
            .get(header::SET_COOKIE)
            .expect("cleared cookie")
            .to_str()
            .expect("cookie text")
            .contains("Max-Age=0")
    );

    let after_logout = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/me")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    let after_logout_body: Value = test::read_body_json(after_logout).await;
    assert_eq!(
        after_logout_body,
        json!({"authenticated": false, "user": null})
    );
}

#[actix_web::test]
async fn unsafe_cross_origin_requests_are_rejected() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
    let app = test::init_service(
        App::new()
            .wrap(from_fn(backend::auth::same_origin::enforce_same_origin))
            .app_data(web::Data::new(AppState::new(
                pool,
                PathBuf::from("data"),
                AppLimits::default(),
            )))
            .configure(routes::register),
    )
    .await;

    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .insert_header((header::HOST, "rain.internal:8080"))
            .insert_header((header::ORIGIN, "http://other.internal:8080"))
            .set_json(json!({"username": "swartz", "password": "password123"}))
            .to_request(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body: Value = test::read_body_json(response).await;
    assert_eq!(body["code"], "CROSS_ORIGIN_REQUEST_REJECTED");
}

#[actix_web::test]
async fn same_origin_browser_requests_forwarded_by_the_dev_proxy_are_allowed() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
    let app = test::init_service(
        App::new()
            .wrap(from_fn(backend::auth::same_origin::enforce_same_origin))
            .app_data(web::Data::new(AppState::new(
                pool,
                PathBuf::from("data"),
                AppLimits::default(),
            )))
            .configure(routes::register),
    )
    .await;

    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .insert_header((header::HOST, "localhost:8080"))
            .insert_header((header::ORIGIN, "http://localhost:5173"))
            .insert_header(("Sec-Fetch-Site", "same-origin"))
            .set_json(json!({"username": "swartz", "password": "password123"}))
            .to_request(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::CREATED);
}

#[actix_web::test]
async fn guest_write_routes_are_rejected() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
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

    let create_issue = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/issues")
            .set_json(json!({"code": "AUTH401"}))
            .to_request(),
    )
    .await;
    assert_eq!(create_issue.status(), StatusCode::UNAUTHORIZED);
    let body: Value = test::read_body_json(create_issue).await;
    assert_eq!(body["code"], "AUTHENTICATION_REQUIRED");

    let delete_issue = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri("/api/issues/AUTH401")
            .to_request(),
    )
    .await;
    assert_eq!(delete_issue.status(), StatusCode::UNAUTHORIZED);
}

#[actix_web::test]
async fn saved_searches_are_private_and_owned_mutations_cannot_be_bypassed() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
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
    let mut cookies = Vec::new();
    for username in ["alice", "bob-user"] {
        let register = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/api/auth/register")
                .set_json(json!({"username": username, "password": "password123"}))
                .to_request(),
        )
        .await;
        assert_eq!(register.status(), StatusCode::CREATED);
        let login = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/api/auth/login")
                .set_json(json!({"username": username, "password": "password123"}))
                .to_request(),
        )
        .await;
        cookies.push(
            Cookie::parse(
                login
                    .headers()
                    .get(header::SET_COOKIE)
                    .unwrap()
                    .to_str()
                    .unwrap(),
            )
            .unwrap()
            .into_owned(),
        );
    }
    let alice = cookies.remove(0);
    let bob = cookies.remove(0);
    let create = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/me/saved-searches")
            .cookie(alice.clone())
            .set_json(json!({
                "name": "Errors",
                "search_type": "DETAIL",
                "query_text": "\"ERROR\"",
                "scope_type": "GLOBAL",
                "scope_key": null,
                "options": {"version": 1},
                "is_pinned": true
            }))
            .to_request(),
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let created: Value = test::read_body_json(create).await;
    let id = created["id"].as_str().expect("id");

    let bob_list = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/me/saved-searches")
            .cookie(bob.clone())
            .to_request(),
    )
    .await;
    let body: Value = test::read_body_json(bob_list).await;
    assert_eq!(body, json!([]));

    for request in [
        test::TestRequest::delete()
            .uri(&format!("/api/me/saved-searches/{id}"))
            .cookie(bob.clone())
            .to_request(),
        test::TestRequest::post()
            .uri(&format!("/api/me/saved-searches/{id}/use"))
            .cookie(bob.clone())
            .to_request(),
    ] {
        let response = test::call_service(&app, request).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    let bob_update = test::call_service(
        &app,
        test::TestRequest::patch()
            .uri(&format!("/api/me/saved-searches/{id}"))
            .cookie(bob)
            .set_json(json!({
                "name": "Stolen",
                "search_type": "FILENAME",
                "query_text": "secret",
                "scope_type": "GLOBAL",
                "scope_key": null,
                "options": {},
                "is_pinned": false,
                "sort_order": 0
            }))
            .to_request(),
    )
    .await;
    assert_eq!(bob_update.status(), StatusCode::NOT_FOUND);

    let alice_update = test::call_service(
        &app,
        test::TestRequest::patch()
            .uri(&format!("/api/me/saved-searches/{id}"))
            .cookie(alice.clone())
            .set_json(json!({
                "name": "Warnings",
                "search_type": "FILENAME",
                "query_text": "warn",
                "scope_type": "ISSUE",
                "scope_key": "cn013",
                "options": {"version": 1},
                "is_pinned": false,
                "sort_order": 7
            }))
            .to_request(),
    )
    .await;
    assert_eq!(alice_update.status(), StatusCode::OK);
    let updated: Value = test::read_body_json(alice_update).await;
    assert_eq!(updated["name"], "Warnings");
    assert_eq!(updated["search_type"], "FILENAME");
    assert_eq!(updated["query_text"], "warn");
    assert_eq!(updated["scope_type"], "GLOBAL");
    assert_eq!(updated["scope_key"], Value::Null);
    assert_eq!(updated["is_pinned"], false);
    assert_eq!(updated["sort_order"], 0);

    let alice_list = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/me/saved-searches")
            .cookie(alice.clone())
            .to_request(),
    )
    .await;
    let body: Value = test::read_body_json(alice_list).await;
    assert_eq!(body.as_array().expect("array").len(), 1);
    assert_eq!(body[0]["query_text"], "warn");
    assert!(body[0].get("temp_result_id").is_none());

    let scoped = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/me/saved-searches")
            .cookie(alice.clone())
            .set_json(json!({
                "name": "Issue errors",
                "search_type": "FILENAME",
                "query_text": "error",
                "scope_type": "ISSUE",
                "scope_key": " cn013 ",
                "options": {}
            }))
            .to_request(),
    )
    .await;
    assert_eq!(scoped.status(), StatusCode::CREATED);
    let scoped_body: Value = test::read_body_json(scoped).await;
    assert_eq!(scoped_body["scope_type"], "GLOBAL");
    assert_eq!(scoped_body["scope_key"], Value::Null);

    let normalized_list = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/me/saved-searches?issue_code=cn013")
            .cookie(alice.clone())
            .to_request(),
    )
    .await;
    let normalized_body: Value = test::read_body_json(normalized_list).await;
    assert_eq!(normalized_body.as_array().expect("array").len(), 2);

    let nested = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/me/saved-searches")
            .cookie(alice.clone())
            .set_json(json!({
                "name": "Nested detail",
                "search_type": "DETAIL",
                "query_text": "(ERROR OR WARN) AND \"tracking point\"",
                "scope_type": "GLOBAL",
                "scope_key": null,
                "options": {"version": 1}
            }))
            .to_request(),
    )
    .await;
    assert_eq!(nested.status(), StatusCode::CREATED);

    let invalid_expression = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/me/saved-searches")
            .cookie(alice)
            .set_json(json!({
                "name": "Broken detail",
                "search_type": "DETAIL",
                "query_text": "ERROR AND OR WARN",
                "scope_type": "GLOBAL",
                "scope_key": null,
                "options": {"version": 1}
            }))
            .to_request(),
    )
    .await;
    assert_eq!(invalid_expression.status(), StatusCode::BAD_REQUEST);
    let invalid_body: Value = test::read_body_json(invalid_expression).await;
    assert_eq!(invalid_body["code"], "SAVED_SEARCH_EXPRESSION_INVALID");
}

#[actix_web::test]
async fn changing_password_revokes_all_old_sessions_and_issues_a_new_cookie() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
    let state = web::Data::new(AppState::new(
        pool.clone(),
        PathBuf::from("data"),
        AppLimits::default(),
    ));
    let app = test::init_service(
        App::new()
            .app_data(state.clone())
            .configure(routes::register),
    )
    .await;
    let register = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username": "password-user", "password": "password123"}))
            .to_request(),
    )
    .await;
    assert_eq!(register.status(), StatusCode::CREATED);
    let user_id: String =
        sqlx::query_scalar("SELECT id FROM users WHERE username_normalized = 'password-user'")
            .fetch_one(&pool)
            .await
            .expect("user id");
    let failure_key = format!("change-password:user:{user_id}");
    let first_login = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/login")
            .set_json(json!({"username": "password-user", "password": "password123"}))
            .to_request(),
    )
    .await;
    let current = Cookie::parse(
        first_login
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap(),
    )
    .unwrap()
    .into_owned();
    let other = {
        let login = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/api/auth/login")
                .set_json(json!({"username": "password-user", "password": "password123"}))
                .to_request(),
        )
        .await;
        Cookie::parse(
            login
                .headers()
                .get(header::SET_COOKIE)
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap()
        .into_owned()
    };
    let wrong_current = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/change-password")
            .cookie(current.clone())
            .set_json(json!({
                "current_password": "wrong-password",
                "new_password": "new-password123"
            }))
            .to_request(),
    )
    .await;
    assert_eq!(wrong_current.status(), StatusCode::UNAUTHORIZED);
    let wrong_body: Value = test::read_body_json(wrong_current).await;
    assert_eq!(wrong_body["code"], "CURRENT_PASSWORD_INVALID");
    assert_eq!(
        state
            .auth_runtime
            .rate_limits
            .lock()
            .expect("rate limits")
            .change_password_user_attempt
            .get(&failure_key)
            .map_or(0, backend::AuthRateLimitBucket::len),
        1
    );

    let changed = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/change-password")
            .cookie(current.clone())
            .set_json(json!({"current_password": "password123", "new_password": "new-password123"}))
            .to_request(),
    )
    .await;
    assert_eq!(changed.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        state
            .auth_runtime
            .rate_limits
            .lock()
            .expect("rate limits")
            .change_password_user_attempt
            .get(&failure_key)
            .map_or(0, backend::AuthRateLimitBucket::len),
        2
    );
    let replacement = Cookie::parse(
        changed
            .headers()
            .get(header::SET_COOKIE)
            .expect("replacement cookie")
            .to_str()
            .expect("cookie text"),
    )
    .expect("cookie")
    .into_owned();
    assert_ne!(replacement.value(), current.value());

    for old_cookie in [current, other] {
        let old_me = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/api/auth/me")
                .cookie(old_cookie)
                .to_request(),
        )
        .await;
        let old_body: Value = test::read_body_json(old_me).await;
        assert_eq!(old_body["authenticated"], false);
    }
    let replacement_me = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/me")
            .cookie(replacement)
            .to_request(),
    )
    .await;
    let replacement_body: Value = test::read_body_json(replacement_me).await;
    assert_eq!(replacement_body["authenticated"], true);
}

#[actix_web::test]
async fn password_change_in_flight_and_attempt_limit_reject_before_password_verification() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
    let state = web::Data::new(AppState::new(
        pool.clone(),
        PathBuf::from("data"),
        AppLimits::default(),
    ));
    let app = test::init_service(
        App::new()
            .app_data(state.clone())
            .configure(routes::register),
    )
    .await;
    let register = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username": "limited-change", "password": "password123"}))
            .to_request(),
    )
    .await;
    assert_eq!(register.status(), StatusCode::CREATED);
    let user_id: String =
        sqlx::query_scalar("SELECT id FROM users WHERE username_normalized = 'limited-change'")
            .fetch_one(&pool)
            .await
            .expect("user id");
    let login = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/login")
            .set_json(json!({"username": "limited-change", "password": "password123"}))
            .to_request(),
    )
    .await;
    let cookie = login
        .response()
        .cookies()
        .next()
        .expect("session cookie")
        .into_owned();
    state
        .auth_runtime
        .rate_limits
        .lock()
        .expect("rate limits")
        .change_password_in_flight
        .insert(user_id.clone());
    let concurrent = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/change-password")
            .cookie(cookie.clone())
            .set_json(json!({
                "current_password": "password123",
                "new_password": "new-password123"
            }))
            .to_request(),
    )
    .await;
    assert_eq!(concurrent.status(), StatusCode::TOO_MANY_REQUESTS);
    state
        .auth_runtime
        .rate_limits
        .lock()
        .expect("rate limits")
        .change_password_in_flight
        .remove(&user_id);

    let mut bucket = backend::AuthRateLimitBucket::new(std::time::Duration::from_secs(15 * 60));
    for _ in 0..5 {
        bucket.push(std::time::Instant::now());
    }
    state
        .auth_runtime
        .rate_limits
        .lock()
        .expect("rate limits")
        .change_password_user_attempt
        .insert(format!("change-password:user:{user_id}"), bucket);

    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/change-password")
            .cookie(cookie)
            .set_json(json!({
                "current_password": "password123",
                "new_password": "new-password123"
            }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body: Value = test::read_body_json(response).await;
    assert_eq!(body["code"], "TOO_MANY_REQUESTS");
}

#[actix_web::test]
async fn password_change_preserves_argon2_capacity_errors() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
    let state = web::Data::new(AppState::new(
        pool,
        PathBuf::from("data"),
        AppLimits::default(),
    ));
    let app = test::init_service(
        App::new()
            .app_data(state.clone())
            .configure(routes::register),
    )
    .await;

    let register = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username": "capacity-user", "password": "password123"}))
            .to_request(),
    )
    .await;
    assert_eq!(register.status(), StatusCode::CREATED);
    let login = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/login")
            .set_json(json!({"username": "capacity-user", "password": "password123"}))
            .to_request(),
    )
    .await;
    let cookie = Cookie::parse(
        login
            .headers()
            .get(header::SET_COOKIE)
            .expect("session cookie")
            .to_str()
            .expect("cookie text"),
    )
    .expect("cookie")
    .into_owned();

    let _permit = state
        .auth_runtime
        .hash_permits
        .clone()
        .acquire_many_owned(state.auth_runtime.config.argon2_concurrency as u32)
        .await
        .expect("argon2 permits");
    let invalid_current = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/change-password")
            .cookie(cookie.clone())
            .set_json(json!({
                "current_password": "x".repeat(10_000),
                "new_password": "new-password123"
            }))
            .to_request(),
    )
    .await;
    assert_eq!(invalid_current.status(), StatusCode::UNAUTHORIZED);
    let invalid_body: Value = test::read_body_json(invalid_current).await;
    assert_eq!(invalid_body["code"], "CURRENT_PASSWORD_INVALID");

    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/change-password")
            .cookie(cookie)
            .set_json(json!({
                "current_password": "password123",
                "new_password": "new-password123"
            }))
            .to_request(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body: Value = test::read_body_json(response).await;
    assert_eq!(body["code"], "TOO_MANY_REQUESTS");
}

#[actix_web::test]
async fn registration_switch_does_not_block_existing_user_login() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
    let password_hash = backend::auth::password::hash_password("password123").expect("hash");
    backend::repositories::users::create_user(&pool, "existing-user", &password_hash)
        .await
        .expect("user");
    let mut state = AppState::new(pool, PathBuf::from("data"), AppLimits::default());
    state.auth_runtime.config.allow_registration = false;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(routes::register),
    )
    .await;
    let register = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username": "new-user", "password": "password123"}))
            .to_request(),
    )
    .await;
    assert_eq!(register.status(), StatusCode::FORBIDDEN);
    let body: Value = test::read_body_json(register).await;
    assert_eq!(body["code"], "REGISTRATION_DISABLED");

    let login = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/login")
            .set_json(json!({"username": "existing-user", "password": "password123"}))
            .to_request(),
    )
    .await;
    assert_eq!(login.status(), StatusCode::OK);
}

#[actix_web::test]
async fn invalid_session_response_does_not_clear_a_newer_browser_session() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
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
    let response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/me")
            .cookie(Cookie::new("rain_session", "unknown-token"))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .next()
            .is_none(),
        "an old invalid-session response must not mutate the browser's current cookie"
    );
}

#[actix_web::test]
async fn saturated_argon2_capacity_returns_rate_limit_instead_of_bad_credentials() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
    let state = AppState::new(pool, PathBuf::from("data"), AppLimits::default());
    let _permit = state
        .auth_runtime
        .hash_permits
        .clone()
        .acquire_many_owned(state.auth_runtime.config.argon2_concurrency as u32)
        .await
        .expect("argon2 permit");
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(routes::register),
    )
    .await;
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/login")
            .set_json(json!({"username": "missing-user", "password": "password123"}))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body: Value = test::read_body_json(response).await;
    assert_eq!(body["code"], "TOO_MANY_REQUESTS");
}
