use backend::{
    ai_provider::{
        config::{ProviderSource, resolve_effective_config},
        crypto::SecretCipher,
    },
    config::AiProviderEnv,
    db,
};
use sqlx::sqlite::SqlitePoolOptions;

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
