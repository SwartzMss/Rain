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
    let cookie = Cookie::parse(set_cookie).expect("parse cookie").into_owned();

    let stored_hash: String =
        sqlx::query_scalar("SELECT token_hash FROM user_sessions LIMIT 1")
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
