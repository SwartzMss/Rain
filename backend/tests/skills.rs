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

fn valid_skill_markdown() -> String {
    r#"---
schema_version: 1
---

# 目标

定位蓝牙连接失败的直接原因。

# 分析范围

关注 Bluetooth Framework、HAL 与 HCI。

# 检索策略

先定位失败信号，再读取原始日志上下文。

# 证据规则

关键事实和根因必须由原始日志行支持。

# 日志不完整处理

证据不足时说明缺失信息，不猜测根因。

# 停止条件

证据足以支持根因或现有日志不足时停止。
"#
    .into()
}

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
                pool.clone(),
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
                "skill_markdown": valid_skill_markdown(),
                "enabled": true
            }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: serde_json::Value = test::read_body_json(response).await;
    let id = created["id"].as_str().unwrap();
    assert_eq!(created["version"], 1);
    assert_eq!(created["schema_version"], 1);
    assert!(created["review"].is_null());

    let response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/me/skills")
            .cookie(owner.clone())
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let listed: serde_json::Value = test::read_body_json(response).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert!(listed[0].get("skill_markdown").is_none());
    assert_eq!(listed[0]["schema_version"], 1);

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
                "skill_markdown": valid_skill_markdown().replace(
                    "定位蓝牙连接失败的直接原因。",
                    "定位 QSEE 失败的直接原因并关联错误码。"
                ),
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
                "skill_markdown": valid_skill_markdown()
            }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    for index in 1..50 {
        sqlx::query("INSERT INTO user_skills(id,owner_user_id,name,skill_markdown,content_hash) VALUES(?,?,?,?,?)")
            .bind(format!("limit-{index}"))
            .bind("owner")
            .bind(format!("Skill {index}"))
            .bind("# Skill")
            .bind(format!("hash-{index}"))
            .execute(&pool)
            .await
            .unwrap();
    }
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/me/skills")
            .cookie(owner.clone())
            .set_json(serde_json::json!({
                "name": "Skill over limit",
                "skill_markdown": valid_skill_markdown()
            }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = test::read_body_json(response).await;
    assert_eq!(body["code"], "SKILL_LIMIT_REACHED");

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

#[actix_web::test]
async fn create_and_update_reject_invalid_skill_format_before_persistence() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    let owner = user_cookie(&pool, "owner", "owner").await;
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

    let missing_schema = valid_skill_markdown().replacen("schema_version: 1\n", "", 1);
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/me/skills")
            .cookie(owner.clone())
            .set_json(serde_json::json!({
                "name": "invalid create",
                "skill_markdown": missing_schema
            }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = test::read_body_json(response).await;
    assert_eq!(body["code"], "SKILL_FORMAT_INVALID");
    assert_eq!(body["message"], "Front Matter 缺少 schema_version");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_skills")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);

    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/me/skills")
            .cookie(owner.clone())
            .set_json(serde_json::json!({
                "name": "valid",
                "skill_markdown": valid_skill_markdown()
            }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: serde_json::Value = test::read_body_json(response).await;
    let id = created["id"].as_str().unwrap();

    let duplicate_goal = format!("{}\n# 目标\n\n第二个目标。\n", valid_skill_markdown());
    let response = test::call_service(
        &app,
        test::TestRequest::put()
            .uri(&format!("/api/me/skills/{id}"))
            .cookie(owner.clone())
            .set_json(serde_json::json!({
                "name": "should not persist",
                "skill_markdown": duplicate_goal,
                "enabled": false
            }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = test::read_body_json(response).await;
    assert_eq!(body["code"], "SKILL_FORMAT_INVALID");
    assert_eq!(body["message"], "重复定义必填章节：目标");

    let response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/me/skills/{id}"))
            .cookie(owner)
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let stored: serde_json::Value = test::read_body_json(response).await;
    assert_eq!(stored["name"], "valid");
    assert_eq!(stored["version"], 1);
    assert_eq!(stored["enabled"], true);
}
