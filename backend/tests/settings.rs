use backend::{
    config::{AiProviderEnv, AppLimits, AuthConfig},
    db,
    settings::{ApplyMode, SettingKey, SettingsService, SettingsValues},
};
use sqlx::sqlite::SqlitePoolOptions;

fn pool() -> sqlx::SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_lazy("sqlite::memory:")
        .expect("in-memory pool")
}

#[test]
fn metadata_covers_every_supported_setting_and_declares_apply_mode() {
    let fields = backend::settings::metadata::all();
    assert!(fields.len() >= 32);
    assert!(fields.iter().any(|field| {
        field.key == SettingKey::IssueMaxContentSize && field.apply_mode == ApplyMode::Hot
    }));
    assert!(fields.iter().any(|field| {
        field.key == SettingKey::UploadConcurrentProcessingTasks
            && field.apply_mode == ApplyMode::RestartRequired
    }));
    assert!(fields.iter().all(|field| !field.env_name.is_empty()));
}

#[test]
fn validation_checks_the_full_candidate_and_cross_field_limits() {
    let mut values = SettingsValues::from_config(&AppLimits::default(), &AuthConfig::default());
    values.api_default_search_results = 101;
    values.api_max_search_results = 100;
    let error = values.validate().expect_err("invalid search relationship");
    assert!(
        error
            .iter()
            .any(|item| item.field == SettingKey::ApiDefaultSearchResults)
    );

    values.api_default_search_results = 50;
    values.temp_results_max_result_size = values.temp_results_max_total_size + 1;
    let error = values
        .validate()
        .expect_err("invalid temp-result relationship");
    assert!(
        error
            .iter()
            .any(|item| item.field == SettingKey::TempResultsMaxResultSize)
    );
}

#[tokio::test]
async fn bootstrap_seeds_legacy_environment_once_and_database_wins_afterward() {
    let pool = pool();
    db::prepare_schema(&pool, true).await.expect("schema");
    let mut limits = AppLimits::default();
    limits.issue_max_content_size = 1234;
    let mut auth = AuthConfig::default();
    auth.allow_registration = false;
    let service = SettingsService::new(pool.clone());

    let first = service
        .initialize(&limits, &auth, &AiProviderEnv::default(), 0, None)
        .await
        .expect("initialization");
    assert_eq!(first.configured.issue_max_content_size, 1234);
    assert!(!first.configured.allow_registration);

    let mut changed = AppLimits::default();
    changed.issue_max_content_size = 9999;
    let mut changed_auth = AuthConfig::default();
    changed_auth.allow_registration = true;
    let second = service
        .initialize(&changed, &changed_auth, &AiProviderEnv::default(), 0, None)
        .await
        .expect("second initialization");
    assert_eq!(second.configured.issue_max_content_size, 1234);
    assert!(!second.configured.allow_registration);
}

#[tokio::test]
async fn save_requires_revision_and_applies_hot_values_atomically() {
    let pool = pool();
    db::prepare_schema(&pool, true).await.expect("schema");
    let service = SettingsService::new(pool.clone());
    let initial = service
        .initialize(
            &AppLimits::default(),
            &AuthConfig::default(),
            &AiProviderEnv::default(),
            0,
            None,
        )
        .await
        .expect("initialization");

    let mut changes = serde_json::Map::new();
    changes.insert("api_default_search_results".into(), serde_json::json!(25));
    changes.insert("search_tantivy_max_writers".into(), serde_json::json!(2));
    let saved = service
        .save(initial.revision, &changes, None)
        .await
        .expect("save");
    assert_eq!(saved.snapshot.configured.api_default_search_results, 25);
    assert_eq!(saved.snapshot.effective.api_default_search_results, 25);
    assert_eq!(saved.snapshot.configured.search_tantivy_max_writers, 2);
    assert_eq!(saved.snapshot.effective.search_tantivy_max_writers, 1);
    assert!(
        saved
            .pending_restart_fields
            .contains(&"search_tantivy_max_writers".to_owned())
    );

    let reloaded = service
        .initialize(
            &AppLimits::default(),
            &AuthConfig::default(),
            &AiProviderEnv::default(),
            0,
            None,
        )
        .await
        .expect("reload");
    assert_eq!(reloaded.configured.search_tantivy_max_writers, 2);
    assert_eq!(reloaded.effective.search_tantivy_max_writers, 1);

    let stale = service.save(initial.revision, &changes, None).await;
    assert!(matches!(
        stale,
        Err(backend::error::AppError::PublicApi {
            code: "SETTINGS_REVISION_CONFLICT",
            ..
        })
    ));
}
