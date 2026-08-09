use actix_web::{App, cookie::Cookie, http::StatusCode, test as actix_test, web};
use backend::{
    AppState,
    ai_provider::{
        client::parse_chat_response,
        config::{ProviderSource, resolve_effective_config},
        crypto::SecretCipher,
    },
    auth::session::{SESSION_COOKIE_NAME, generate_session_token, hash_session_token},
    config::{AiProviderEnv, AppLimits},
    db,
    repositories::{bootstrap_admin, sessions},
    routes,
};
use chrono::{Duration, Utc};
use sqlx::sqlite::SqlitePoolOptions;
use std::path::PathBuf;

#[tokio::test]
async fn schema_creates_skill_runner_storage_and_active_run_guard() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    db::prepare_schema(&pool, false).await.unwrap();

    for name in [
        "user_skills",
        "skill_reviews",
        "ai_provider_settings",
        "skill_runs",
        "skill_run_steps",
    ] {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?")
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 1, "missing table {name}");
    }

    let index_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_skill_runs_one_active_per_user'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(index_count, 1);
}

#[test]
fn encrypted_provider_secret_round_trips_and_rejects_the_wrong_key() {
    let cipher = SecretCipher::new([3; 32]);
    let envelope = cipher.encrypt("provider-secret").unwrap();

    assert_eq!(cipher.decrypt(&envelope).unwrap(), "provider-secret");
    assert!(SecretCipher::new([4; 32]).decrypt(&envelope).is_err());
    assert!(!envelope.contains("provider-secret"));
}

#[tokio::test]
async fn effective_provider_prefers_database_then_falls_back_to_environment() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    let env = AiProviderEnv::from_values(
        Some("https://env.example/v1"),
        Some("env-secret"),
        Some("env-model"),
        90,
        Some([5; 32]),
    )
    .unwrap();

    let resolved = resolve_effective_config(&pool, &env)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.source, ProviderSource::Environment);
    assert_eq!(resolved.model, "env-model");
    assert_eq!(resolved.api_key(), "env-secret");

    let encrypted = SecretCipher::new([5; 32])
        .encrypt("database-secret")
        .unwrap();
    sqlx::query("INSERT INTO ai_provider_settings(id,base_url,encrypted_api_key,model,request_timeout_seconds) VALUES(1,?,?,?,?)")
        .bind("https://database.example/v1")
        .bind(encrypted)
        .bind("database-model")
        .bind(60_i64)
        .execute(&pool)
        .await
        .unwrap();

    let resolved = resolve_effective_config(&pool, &env)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.source, ProviderSource::Database);
    assert_eq!(resolved.model, "database-model");
    assert_eq!(resolved.api_key(), "database-secret");
}

#[tokio::test]
async fn effective_provider_is_unavailable_when_neither_source_is_complete() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    let env = AiProviderEnv::from_values(None, None, None, 120, None).unwrap();

    assert!(
        resolve_effective_config(&pool, &env)
            .await
            .unwrap()
            .is_none()
    );
}

#[actix_web::test]
async fn provider_settings_reject_guests() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    let app = actix_test::init_service(
        App::new()
            .app_data(web::Data::new(AppState::new(
                pool,
                PathBuf::from("data"),
                AppLimits::default(),
            )))
            .configure(routes::register),
    )
    .await;

    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::get()
            .uri("/api/admin/ai-provider")
            .to_request(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[actix_web::test]
async fn administrator_can_save_masked_provider_without_exposing_the_secret() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    db::prepare_schema(&pool, false).await.unwrap();
    bootstrap_admin::bootstrap_admin(&pool, "admin", "strong-password")
        .await
        .unwrap();
    let admin_id: String = sqlx::query_scalar("SELECT id FROM users WHERE role='ADMIN'")
        .fetch_one(&pool)
        .await
        .unwrap();
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
    .unwrap();
    let env = AiProviderEnv::from_values(None, None, None, 120, Some([5; 32])).unwrap();
    let state = AppState::new_with_ai(
        pool.clone(),
        PathBuf::from("data"),
        AppLimits::default(),
        env,
    );
    let app = actix_test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(routes::register),
    )
    .await;
    let cookie = Cookie::new(SESSION_COOKIE_NAME, token);

    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::put()
            .uri("/api/admin/ai-provider")
            .cookie(cookie.clone())
            .set_json(serde_json::json!({
                "base_url": "https://model.example/v1/",
                "api_key": "super-secret-key",
                "model": "analysis-model",
                "request_timeout_seconds": 75
            }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = actix_test::read_body_json(response).await;
    assert_eq!(body["source"], "DATABASE");
    assert_eq!(body["api_key_mask"], "••••••••");
    assert!(!body.to_string().contains("super-secret-key"));

    let encrypted: String =
        sqlx::query_scalar("SELECT encrypted_api_key FROM ai_provider_settings WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!encrypted.contains("super-secret-key"));
    assert_eq!(
        SecretCipher::new([5; 32]).decrypt(&encrypted).unwrap(),
        "super-secret-key"
    );

    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::put()
            .uri("/api/admin/ai-provider")
            .cookie(cookie.clone())
            .set_json(serde_json::json!({
                "base_url": "https://model.example/v1",
                "model": "replacement-model",
                "request_timeout_seconds": 80
            }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let preserved: String =
        sqlx::query_scalar("SELECT encrypted_api_key FROM ai_provider_settings WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(encrypted, preserved);

    for (master_key, expected_code) in [
        (None, "AI_MASTER_KEY_REQUIRED"),
        (Some([6; 32]), "AI_MASTER_KEY_INVALID"),
    ] {
        let env = AiProviderEnv::from_values(None, None, None, 120, master_key).unwrap();
        let app = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(AppState::new_with_ai(
                    pool.clone(),
                    PathBuf::from("data"),
                    AppLimits::default(),
                    env,
                )))
                .configure(routes::register),
        )
        .await;
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::put()
                .uri("/api/admin/ai-provider")
                .cookie(cookie.clone())
                .set_json(serde_json::json!({
                    "base_url": "https://model.example/v1",
                    "model": "replacement-model",
                    "request_timeout_seconds": 80
                }))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body: serde_json::Value = actix_test::read_body_json(response).await;
        assert_eq!(body["code"], expected_code);
        let after_failure: String =
            sqlx::query_scalar("SELECT encrypted_api_key FROM ai_provider_settings WHERE id=1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(after_failure, preserved);
    }

    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/api/admin/ai-provider/test")
            .cookie(cookie.clone())
            .set_json(serde_json::json!({
                "base_url": "https://attacker.example/v1",
                "model": "replacement-model",
                "request_timeout_seconds": 80
            }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = actix_test::read_body_json(response).await;
    assert_eq!(body["code"], "AI_PROVIDER_TEST_REQUIRES_COMPLETE_CONFIG");

    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::put()
            .uri("/api/admin/ai-provider")
            .cookie(cookie.clone())
            .set_json(serde_json::json!({
                "base_url": "https://other.example/v1",
                "model": "replacement-model",
                "request_timeout_seconds": 80
            }))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = actix_test::read_body_json(response).await;
    assert_eq!(body["code"], "AI_API_KEY_REQUIRED_FOR_BASE_URL_CHANGE");

    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/api/admin/ai-provider/test")
            .cookie(cookie)
            .set_json(serde_json::json!({"modle":"typo"}))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = actix_test::read_body_json(response).await;
    assert_eq!(body["code"], "INVALID_AI_PROVIDER_TEST");

    let audit_values: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT new_value FROM admin_audit_logs WHERE action='AI_PROVIDER_UPDATED'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(audit_values.len(), 2);
    assert!(
        audit_values
            .iter()
            .flatten()
            .all(|value| !value.contains("super-secret-key"))
    );

    let response = actix_test::call_service(
        &app,
        actix_test::TestRequest::post()
            .uri("/api/admin/ai-provider/test")
            .cookie(Cookie::new(SESSION_COOKIE_NAME, generate_session_token()))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn chat_completion_parser_extracts_tool_calls_and_sanitizes_invalid_responses() {
    let response = parse_chat_response(
        br#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"search_logs","arguments":"{\"query\":\"QSEE\"}"}}]}}]}"#,
    )
    .unwrap();
    assert_eq!(response.message.role, "assistant");
    assert_eq!(response.message.tool_calls.len(), 1);
    assert_eq!(response.message.tool_calls[0].function.name, "search_logs");

    let secret = "response-containing-super-secret";
    let error = parse_chat_response(secret.as_bytes()).unwrap_err();
    assert_eq!(error.code(), "AI_PROVIDER_INVALID_RESPONSE");
    assert!(!format!("{error:?}").contains(secret));
    assert!(!error.to_string().contains(secret));
}
