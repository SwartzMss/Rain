use std::path::PathBuf;

use actix_web::{App, cookie::Cookie, http::StatusCode, test, web};
use backend::{
    AppState,
    auth::session::{SESSION_COOKIE_NAME, generate_session_token, hash_session_token},
    config::AppLimits,
    db,
    repositories::sessions,
    routes,
};
use chrono::{Duration, Utc};

#[actix_web::test]
async fn review_rejects_a_historical_free_form_skill_before_model_work() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO users(id,username,username_normalized,password_hash) VALUES('u','user','user','hash')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_skills(id,owner_user_id,name,skill_markdown,content_hash) VALUES('skill','u','Legacy Skill','# free-form prompt','hash')")
        .execute(&pool)
        .await
        .unwrap();
    let token = generate_session_token();
    sessions::create_session(
        &pool,
        "u",
        &hash_session_token(&token),
        Utc::now() + Duration::hours(1),
        None,
        None,
    )
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

    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/me/skills/skill/review")
            .cookie(Cookie::new(SESSION_COOKIE_NAME, token))
            .to_request(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = test::read_body_json(response).await;
    assert_eq!(body["code"], "SKILL_FORMAT_INVALID");
    assert_eq!(body["message"], "缺少合法的 Front Matter");
}
