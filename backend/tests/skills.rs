use std::path::PathBuf;

use actix_web::{App, cookie::Cookie, http::StatusCode, test, web};
use backend::{
    AppState,
    auth::session::{SESSION_COOKIE_NAME, generate_session_token, hash_session_token},
    config::AppLimits,
    db,
    models::skills::{SkillPayload, SkillReview},
    repositories::{sessions, skills},
    routes,
};
use chrono::{Duration, Utc};

async fn user_cookie(pool: &sqlx::SqlitePool, id: &str, username: &str) -> Cookie<'static> {
    sqlx::query(
        "INSERT INTO users(id,username,username_normalized,password_hash) VALUES(?,?,?,'hash')",
    )
    .bind(id)
    .bind(username)
    .bind(username.to_ascii_lowercase())
    .execute(pool)
    .await
    .unwrap();
    let token = generate_session_token();
    sessions::create_session(
        pool,
        id,
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
async fn review_save_is_conditioned_on_the_current_skill_version_and_hash() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO users(id,username,username_normalized,password_hash) VALUES('u','user','user','hash')")
        .execute(&pool).await.unwrap();
    let payload = SkillPayload {
        name: "diagnose".into(),
        description: None,
        skill_markdown: "# v1".into(),
        enabled: true,
    };
    let created = skills::create(&pool, "u", &payload, "hash-v1")
        .await
        .unwrap();
    let snapshot = skills::find_owned(&pool, "u", &created.id)
        .await
        .unwrap()
        .unwrap();
    let review = SkillReview {
        overall_score: 80,
        grade: "GOOD".into(),
        dimensions: serde_json::json!({"scope": 80}),
        warnings: vec![],
        suggestions: vec![],
        evaluated_at: None,
    };
    assert!(
        skills::save_review(&pool, &snapshot, "model", &review)
            .await
            .unwrap()
    );

    let changed = SkillPayload {
        skill_markdown: "# v2".into(),
        ..payload
    };
    skills::update(&pool, "u", &created.id, &changed, "hash-v2")
        .await
        .unwrap();
    assert!(
        !skills::save_review(&pool, &snapshot, "model", &review)
            .await
            .unwrap()
    );
    assert!(
        skills::find_response(&pool, "u", &created.id)
            .await
            .unwrap()
            .unwrap()
            .review
            .is_none()
    );
}

#[actix_web::test]
async fn skills_are_private_versioned_and_validated() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    let owner = user_cookie(&pool, "owner", "owner").await;
    let other = user_cookie(&pool, "other", "other").await;
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
            .uri("/api/me/skills")
            .cookie(owner.clone())
            .set_json(serde_json::json!({
                "name": "QSEE analysis",
                "description": "private",
                "skill_markdown": "# Goal\nSearch QSEE evidence",
                "enabled": true
            }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: serde_json::Value = test::read_body_json(response).await;
    let id = created["id"].as_str().unwrap();
    assert_eq!(created["version"], 1);
    assert!(created["review"].is_null());

    let response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/me/skills/{id}"))
            .cookie(other.clone())
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = test::call_service(
        &app,
        test::TestRequest::put()
            .uri(&format!("/api/me/skills/{id}"))
            .cookie(owner.clone())
            .set_json(serde_json::json!({
                "name": "QSEE analysis",
                "description": "updated",
                "skill_markdown": "# Goal\nSearch QSEE evidence and error codes",
                "enabled": false
            }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let updated: serde_json::Value = test::read_body_json(response).await;
    assert_eq!(updated["version"], 2);
    assert_eq!(updated["enabled"], false);

    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/me/skills")
            .cookie(owner.clone())
            .set_json(serde_json::json!({
                "name": "qsee ANALYSIS",
                "skill_markdown": "content"
            }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri(&format!("/api/me/skills/{id}/review"))
            .cookie(owner.clone())
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let response = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri(&format!("/api/me/skills/{id}"))
            .cookie(owner)
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}
