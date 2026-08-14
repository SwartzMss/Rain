use std::path::PathBuf;

use actix_web::{App, cookie::Cookie, http::StatusCode, test, web};
use backend::{
    AppState,
    auth::session::{SESSION_COOKIE_NAME, generate_session_token, hash_session_token},
    config::{AiProviderEnv, AppLimits},
    db,
    models::skill_runs::NewSkillRun,
    repositories::{sessions, skill_runs},
    routes,
    services::skill_time_scope::{TimeScopeInput, parse_time_scope},
};
use chrono::{Duration, Utc};

const VALID_SKILL_V1: &str = r#"---
schema_version: 1
---
# 目标
定位故障。
# 分析范围
分析当前 Issue 日志。
# 关键流程
搜索信号并读取上下文。
# 关键日志
描述关键事件日志。
# 关系与影响
描述故障对后续流程的影响。
"#;

#[tokio::test]
async fn scoped_skill_run_persists_wall_clock_analysis_window() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO users(id,username,username_normalized,password_hash) VALUES('u','user','user','hash')")
        .execute(&pool)
        .await
        .unwrap();
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
    let scope = parse_time_scope(Some(TimeScopeInput {
        start: Some("2026-08-14 09:27:15".into()),
        end: Some("2026-08-14T09:37:15".into()),
        ..Default::default()
    }))
    .unwrap()
    .unwrap();

    let run = skill_runs::create_with_scope(&pool, &new_run, Some(&scope))
        .await
        .unwrap();

    assert_eq!(
        run.analysis_start_time.as_deref(),
        Some("2026-08-14 09:27:15")
    );
    assert_eq!(
        run.analysis_end_time.as_deref(),
        Some("2026-08-14 09:37:15")
    );
    assert_eq!(run.analysis_start_ms, Some(scope.start_ms));
    assert_eq!(run.analysis_end_ms, Some(scope.end_ms));
    let serialized = serde_json::to_value(&run).unwrap();
    assert!(serialized.get("analysis_start_ms").is_none());
    assert!(serialized.get("analysis_end_ms").is_none());
}

#[tokio::test]
async fn unscoped_skill_run_has_no_analysis_window() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO users(id,username,username_normalized,password_hash) VALUES('u','user','user','hash')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('ISSUE','Issue')")
        .execute(&pool)
        .await
        .unwrap();

    let run = skill_runs::create(
        &pool,
        &NewSkillRun {
            user_id: "u".into(),
            issue_code: "ISSUE".into(),
            skill_id: "skill".into(),
            skill_version: 1,
            skill_name: "Skill".into(),
            skill_snapshot_markdown: "# Skill".into(),
        },
    )
    .await
    .unwrap();

    assert!(run.analysis_start_time.is_none());
    assert!(run.analysis_end_time.is_none());
    assert!(run.analysis_start_ms.is_none());
    assert!(run.analysis_end_ms.is_none());
}

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

#[tokio::test]
async fn recover_active_is_idempotent_and_releases_the_active_slot() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO users(id,username,username_normalized,password_hash) VALUES('u','user','user','hash')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('ISSUE','Issue')")
        .execute(&pool)
        .await
        .unwrap();
    let run = skill_runs::create(
        &pool,
        &NewSkillRun {
            user_id: "u".into(),
            issue_code: "ISSUE".into(),
            skill_id: "skill".into(),
            skill_version: 1,
            skill_name: "Skill".into(),
            skill_snapshot_markdown: "# Skill".into(),
        },
    )
    .await
    .unwrap();

    assert_eq!(skill_runs::recover_active(&pool).await.unwrap(), 1);
    assert_eq!(skill_runs::recover_active(&pool).await.unwrap(), 0);
    assert!(
        skill_runs::find_active_owned(&pool, "u")
            .await
            .unwrap()
            .is_none()
    );
    let recovered: (String, String, String) =
        sqlx::query_as("SELECT status, error_code, error_message FROM skill_runs WHERE id=?")
            .bind(run.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(recovered.0, "FAILED");
    assert_eq!(recovered.1, "SERVICE_RESTARTED");
    assert_eq!(recovered.2, "服务重启导致任务中断");
}

#[tokio::test]
async fn recover_active_before_does_not_touch_runs_created_after_cutoff() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO users(id,username,username_normalized,password_hash) VALUES('u','user','user','hash')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('ISSUE','Issue')")
        .execute(&pool)
        .await
        .unwrap();
    let old_run = skill_runs::create(
        &pool,
        &NewSkillRun {
            user_id: "u".into(),
            issue_code: "ISSUE".into(),
            skill_id: "old-skill".into(),
            skill_version: 1,
            skill_name: "Old Skill".into(),
            skill_snapshot_markdown: "# Old Skill".into(),
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE skill_runs SET created_at='2021-01-01 00:00:00' WHERE id=?")
        .bind(&old_run.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(skill_runs::recover_active(&pool).await.unwrap() > 0);

    let new_run = skill_runs::create(
        &pool,
        &NewSkillRun {
            user_id: "u".into(),
            issue_code: "ISSUE".into(),
            skill_id: "new-skill".into(),
            skill_version: 1,
            skill_name: "New Skill".into(),
            skill_snapshot_markdown: "# New Skill".into(),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        skill_runs::recover_active_before(&pool, "2021-01-01 00:00:00")
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        skill_runs::find_active_owned(&pool, "u")
            .await
            .unwrap()
            .unwrap()
            .id,
        new_run.id
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

#[actix_web::test]
async fn skill_run_api_exposes_canonical_analysis_window_in_all_run_views() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO users(id,username,username_normalized,password_hash) VALUES('u','user','user','hash')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO issues(code,name) VALUES('ISSUE','Issue')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_skills(id,owner_user_id,name,skill_markdown,content_hash) VALUES('skill','u','Skill',?,'hash')")
        .bind(VALID_SKILL_V1)
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

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let _provider_server = tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let mut request = [0_u8; 4096];
        let _ = socket.read(&mut request).await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let _ = socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
    });
    let base_url = format!("http://{address}/v1");
    let ai_provider = AiProviderEnv::from_values(
        Some(&base_url),
        Some("test-key"),
        Some("test-model"),
        10,
        None,
    )
    .unwrap();
    let state = web::Data::new(AppState::new_with_ai(
        pool.clone(),
        PathBuf::from("data"),
        AppLimits::default(),
        ai_provider,
    ));
    let app = test::init_service(App::new().app_data(state).configure(routes::register)).await;

    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/issues/ISSUE/skill-runs")
            .cookie(Cookie::new(SESSION_COOKIE_NAME, token.clone()))
            .set_json(serde_json::json!({
                "skill_id": "skill",
                "time_scope": {
                    "start": "2026-08-14 09:27:15",
                    "end": "2026-08-14T09:37:15"
                }
            }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let created: serde_json::Value = test::read_body_json(response).await;
    let run_id = created["id"].as_str().unwrap().to_owned();
    for body in [&created] {
        assert_eq!(body["analysis_start_time"], "2026-08-14 09:27:15");
        assert_eq!(body["analysis_end_time"], "2026-08-14 09:37:15");
    }

    let response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/skill-runs/{run_id}"))
            .cookie(Cookie::new(SESSION_COOKIE_NAME, token.clone()))
            .to_request(),
    )
    .await;
    let fetched: serde_json::Value = test::read_body_json(response).await;
    assert_eq!(fetched["analysis_start_time"], "2026-08-14 09:27:15");
    assert_eq!(fetched["analysis_end_time"], "2026-08-14 09:37:15");

    let response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/me/skill-runs/active")
            .cookie(Cookie::new(SESSION_COOKIE_NAME, token.clone()))
            .to_request(),
    )
    .await;
    let active: serde_json::Value = test::read_body_json(response).await;
    assert_eq!(active["analysis_start_time"], "2026-08-14 09:27:15");
    assert_eq!(active["analysis_end_time"], "2026-08-14 09:37:15");

    let response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/skill-runs/{run_id}/events"))
            .cookie(Cookie::new(SESSION_COOKIE_NAME, token))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let sse = String::from_utf8(test::read_body(response).await.to_vec()).unwrap();
    assert!(sse.contains("event: snapshot"));
    assert!(sse.contains("\"analysis_start_time\":\"2026-08-14 09:27:15\""));
    assert!(sse.contains("\"analysis_end_time\":\"2026-08-14 09:37:15\""));
}

#[actix_web::test]
async fn invalid_skill_run_time_scopes_are_rejected_before_downstream_work() {
    let pool = db::init_pool("sqlite::memory:").unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    sqlx::query("INSERT INTO users(id,username,username_normalized,password_hash) VALUES('u','user','user','hash')")
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
                pool.clone(),
                PathBuf::from("data"),
                AppLimits::default(),
            )))
            .configure(routes::register),
    )
    .await;

    let invalid_scopes = [
        serde_json::json!({
            "start": "not-a-timestamp",
            "end": "2026-08-14 09:37:15"
        }),
        serde_json::json!({
            "start": "2026-08-14 09:37:15",
            "end": "2026-08-14 09:27:15"
        }),
        serde_json::json!({
            "start": "2026-08-14 09:27:15",
            "end": "2026-08-14 09:27:15"
        }),
        serde_json::json!({
            "start": "2026-08-14 09:00:00",
            "end": "2026-08-15 09:00:01"
        }),
        serde_json::json!({}),
        serde_json::json!({"start": "2026-08-14 09:27:15"}),
        serde_json::json!({"end": "2026-08-14 09:37:15"}),
        serde_json::json!({
            "start": 1723602435,
            "end": "2026-08-14 09:37:15"
        }),
        serde_json::json!({
            "start": "2026-08-14 09:27:15",
            "end": ["2026-08-14 09:37:15"]
        }),
        serde_json::json!([]),
        serde_json::json!("not-an-object"),
    ];
    for time_scope in invalid_scopes {
        let response = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/api/issues/MISSING/skill-runs")
                .cookie(Cookie::new(SESSION_COOKIE_NAME, token.clone()))
                .set_json(serde_json::json!({
                    "skill_id": "missing-skill",
                    "time_scope": time_scope
                }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = test::read_body(response).await;
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("\"code\":\"INVALID_TIME_SCOPE\""),
            "unexpected invalid time scope response: {body}"
        );
    }

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skill_runs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
