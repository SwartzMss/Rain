use std::path::PathBuf;

use actix_web::{
    App,
    cookie::Cookie,
    http::{StatusCode, header},
    test, web,
};
use backend::{AppState, config::AppLimits, db, routes};
use serde_json::{Value, json};

#[actix_web::test]
async fn registration_login_me_and_logout_follow_the_public_contract() {
    let pool = db::init_pool("sqlite::memory:").expect("pool");
    db::prepare_schema(&pool, true).await.expect("schema");
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppState::new(
                pool.clone(),
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
            .cookie(bob)
            .to_request(),
    ] {
        let response = test::call_service(&app, request).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    let alice_list = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/me/saved-searches")
            .cookie(alice)
            .to_request(),
    )
    .await;
    let body: Value = test::read_body_json(alice_list).await;
    assert_eq!(body.as_array().expect("array").len(), 1);
    assert_eq!(body[0]["query_text"], "\"ERROR\"");
    assert!(body[0].get("temp_result_id").is_none());
}

#[actix_web::test]
async fn changing_password_revokes_other_sessions_and_keeps_current_session() {
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
    let register = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/register")
            .set_json(json!({"username": "password-user", "password": "password123"}))
            .to_request(),
    )
    .await;
    assert_eq!(register.status(), StatusCode::CREATED);
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

    let current_me = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/me")
            .cookie(current)
            .to_request(),
    )
    .await;
    let current_body: Value = test::read_body_json(current_me).await;
    assert_eq!(current_body["authenticated"], true);
    let other_me = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/auth/me")
            .cookie(other)
            .to_request(),
    )
    .await;
    let other_body: Value = test::read_body_json(other_me).await;
    assert_eq!(other_body["authenticated"], false);
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
    state.auth.allow_registration = false;
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
