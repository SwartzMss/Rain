use std::path::PathBuf;

use actix_web::{App, cookie::Cookie, http::StatusCode, test, web};
use backend::{
    AppState,
    auth::session::{SESSION_COOKIE_NAME, generate_session_token, hash_session_token},
    config::AppLimits,
    db,
    models::skill_runs::NewSkillRun,
    repositories::{sessions, skill_runs},
    routes,
};
use chrono::{Duration, Utc};

const VALID_SKILL_V1: &str = r#"---
schema_version: 1
---
# 目标
定位故障。
# 分析范围
分析当前 Issue 日志。
# 检索策略
搜索信号并读取上下文。
# 证据规则
仅使用原始日志行作为证据。
# 日志不完整处理
报告缺失信息。
# 停止条件
证据充分或现有日志不足时停止。
"#;

#[tokio::test]
async fn run_state_is_atomic_concurrent_and_temporary() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO users(id,username,username_normalized,password_hash) VALUES('u','user','user','hash'),('v','viewer','viewer','hash')")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('ISSUE','Issue')")
        .execute(&pool)
        .await
        .unwrap();
    let new_run = NewSkillRun {
        user_id: "u".into(),
        issue_code: "ISSUE".into(),
        skill_id: "skill".into(),
        skill_version: 1,
        skill_name: "Skill".into(),
        skill_snapshot_markdown: "# Skill".into(),
    };
    let first = skill_runs::create(&pool, &new_run).await.unwrap();
    assert!(skill_runs::create(&pool, &new_run).await.is_err());
    assert_eq!(
        skill_runs::find_active_owned(&pool, "u")
            .await
            .unwrap()
            .unwrap()
            .id,
        first.id
    );
    assert!(skill_runs::mark_running(&pool, &first.id).await.unwrap());
    assert!(skill_runs::cancel(&pool, &first.id, "u").await.unwrap());
    assert!(
        skill_runs::find_active_owned(&pool, "u")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !skill_runs::complete(&pool, &first.id, "{\"summary\":\"late\"}")
            .await
            .unwrap()
    );

    sqlx::query("UPDATE skill_runs SET completed_at=datetime('now','-25 hours') WHERE id=?")
        .bind(&first.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        skill_runs::cleanup_expired(&pool, 24 * 60 * 60)
            .await
            .unwrap(),
        1
    );
}

#[actix_web::test]
async fn run_creation_requires_a_valid_skill_and_configured_provider_but_not_issue_ownership() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO users(id,username,username_normalized,password_hash) VALUES('owner','owner','owner','hash'),('viewer','viewer','viewer','hash')")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO issues(code,name,owner_user_id) VALUES('ISSUE','Issue','owner')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_skills(id,owner_user_id,name,skill_markdown,content_hash) VALUES('skill','viewer','Skill','# Analyze','hash')")
        .execute(&pool).await.unwrap();
    let token = generate_session_token();
    sessions::create_session(
        &pool,
        "viewer",
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
            .uri("/api/issues/ISSUE/skill-runs")
            .cookie(Cookie::new(SESSION_COOKIE_NAME, token.clone()))
            .set_json(serde_json::json!({"skill_id":"skill"}))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = test::read_body_json(response).await;
    assert_eq!(body["code"], "SKILL_FORMAT_INVALID");

    sqlx::query("UPDATE user_skills SET skill_markdown=? WHERE id='skill'")
        .bind(VALID_SKILL_V1)
        .execute(&pool)
        .await
        .unwrap();

    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/issues/ISSUE/skill-runs")
            .cookie(Cookie::new(SESSION_COOKIE_NAME, token))
            .set_json(serde_json::json!({"skill_id":"skill"}))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = test::read_body_json(response).await;
    assert_eq!(body["code"], "AI_PROVIDER_NOT_CONFIGURED");
}
