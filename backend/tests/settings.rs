use backend::{
    config::{AppLimits, AuthConfig},
    db,
    settings::{
        ApplyMode, ResourceMode, ResourceModes, SettingKey, SettingsService, SettingsValues,
    },
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
    assert_eq!(fields.len(), SettingKey::ALL.len());
    assert!(fields.iter().all(|field| !matches!(
        field.env_name,
        "RAIN_TEMP_RESULT_MAX_SCAN_BYTES" | "RAIN_TEMP_RESULT_MAX_SOURCES"
    )));
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
fn metadata_exposes_presentation_contract_and_covers_supported_keys() {
    let fields = backend::settings::metadata::all();
    let keys: std::collections::HashSet<_> = fields.iter().map(|field| field.key).collect();
    assert_eq!(fields.len(), SettingKey::ALL.len());
    assert_eq!(keys.len(), fields.len());
    assert!(fields.iter().all(|field| {
        field.recommended_min.unwrap_or(0) <= field.recommended_max.unwrap_or(u64::MAX)
    }));

    let issue_size = fields
        .iter()
        .find(|field| field.key == SettingKey::IssueMaxContentSize)
        .expect("issue size metadata");
    let serialized = serde_json::to_value(issue_size).expect("serializable metadata");
    assert_eq!(serialized["category"], "common");
    assert_eq!(serialized["visibility"], "default");
    assert_eq!(serialized["sensitive"], false);
    assert!(serialized.get("recommended_min").is_some());
    assert!(serialized.get("recommended_max").is_some());

    let sensitive_issue_size = backend::settings::metadata::FieldMetadata::new_sensitive(
        SettingKey::IssueMaxContentSize,
        "issue_max_content_size",
        "RAIN_ISSUE_MAX_CONTENT_SIZE",
        ApplyMode::Hot,
    );
    let serialized = serde_json::to_value(sensitive_issue_size).expect("sensitive metadata");
    assert_eq!(serialized["sensitive"], true);
}

#[test]
fn metadata_declares_auto_values_and_protected_settings() {
    let processing = backend::settings::metadata::all()
        .iter()
        .find(|field| field.key == SettingKey::UploadConcurrentProcessingTasks)
        .expect("processing metadata");
    assert!(processing.supports_auto);
    assert_eq!(processing.auto_value, Some(4));
    assert!(processing.protected);
    assert!(
        !backend::settings::metadata::admin()
            .iter()
            .any(|field| field.key == SettingKey::UploadConcurrentProcessingTasks)
    );

    let argon2 = backend::settings::metadata::all()
        .iter()
        .find(|field| field.key == SettingKey::Argon2Concurrency)
        .expect("argon2 metadata");
    assert!(argon2.protected);
    assert!(
        !backend::settings::metadata::admin()
            .iter()
            .any(|field| field.key == SettingKey::Argon2Concurrency)
    );

    for key in [
        SettingKey::SessionTtlSeconds,
        SettingKey::RegisterIpLimitPerHour,
    ] {
        let field = backend::settings::metadata::all()
            .iter()
            .find(|field| field.key == key)
            .expect("system-managed metadata");
        assert!(field.protected);
        assert!(
            !backend::settings::metadata::admin()
                .iter()
                .any(|candidate| candidate.key == key)
        );
    }
    assert!(
        !backend::settings::metadata::admin()
            .iter()
            .any(|field| { field.key == SettingKey::LoginUsernameFailureLimitPer5Minutes })
    );
    let default_search_results = backend::settings::metadata::all()
        .iter()
        .find(|field| field.key == SettingKey::ApiDefaultSearchResults)
        .expect("default search results metadata");
    assert!(default_search_results.protected);
    assert!(
        !backend::settings::metadata::admin()
            .iter()
            .any(|field| field.key == SettingKey::ApiDefaultSearchResults)
    );
    let search_page_limit = backend::settings::metadata::all()
        .iter()
        .find(|field| field.key == SettingKey::ApiMaxSearchResults)
        .expect("search page limit metadata");
    assert!(search_page_limit.protected);
    assert!(
        !backend::settings::metadata::admin()
            .iter()
            .any(|field| field.key == SettingKey::ApiMaxSearchResults)
    );
    let search_window = backend::settings::metadata::all()
        .iter()
        .find(|field| field.key == SettingKey::ApiMaxSearchWindow)
        .expect("search window metadata");
    assert!(search_window.protected);
    assert!(
        !backend::settings::metadata::admin()
            .iter()
            .any(|field| field.key == SettingKey::ApiMaxSearchWindow)
    );
}

#[test]
fn resource_mode_serializes_stably() {
    assert_eq!(serde_json::to_value(ResourceMode::Auto).unwrap(), "auto");
    assert_eq!(
        serde_json::to_value(ResourceMode::Manual).unwrap(),
        "manual"
    );
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
        .initialize(&limits, &auth, 0, None)
        .await
        .expect("initialization");
    assert_eq!(first.configured.issue_max_content_size, 1234);
    assert!(!first.configured.allow_registration);

    let mut changed = AppLimits::default();
    changed.issue_max_content_size = 9999;
    let mut changed_auth = AuthConfig::default();
    changed_auth.allow_registration = true;
    let second = service
        .initialize(&changed, &changed_auth, 0, None)
        .await
        .expect("second initialization");
    assert_eq!(second.configured.issue_max_content_size, 1234);
    assert!(!second.configured.allow_registration);
}

#[tokio::test]
async fn resource_modes_default_to_manual_and_round_trip() {
    let pool = pool();
    db::prepare_schema(&pool, true).await.expect("schema");
    let service = SettingsService::new(pool.clone());
    let initial = service
        .initialize(&AppLimits::default(), &AuthConfig::default(), 0, None)
        .await
        .expect("initialize");
    let initialized_value: String = sqlx::query_scalar(
        "SELECT new_value FROM admin_audit_logs WHERE action='SYSTEM_SETTINGS_INITIALIZED'",
    )
    .fetch_one(&pool)
    .await
    .expect("initialization audit summary");
    assert_eq!(initialized_value, "系统配置已初始化");

    assert_eq!(
        initial.resource_modes["upload_concurrent_processing_tasks"],
        ResourceMode::Manual
    );

    let mut modes = ResourceModes::new();
    modes.insert(
        "upload_concurrent_processing_tasks".into(),
        ResourceMode::Auto,
    );
    let saved = service
        .save_with_modes(initial.revision, &serde_json::Map::new(), &modes, None)
        .await
        .expect("save mode");

    assert_eq!(
        saved.snapshot.resource_modes["upload_concurrent_processing_tasks"],
        ResourceMode::Auto
    );
    assert_eq!(
        saved.snapshot.configured.upload_concurrent_processing_tasks,
        4
    );
    let details: String = sqlx::query_scalar(
        "SELECT details_json FROM admin_audit_logs WHERE action='SETTINGS_UPDATED' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("resource mode audit details");
    let details: serde_json::Value = serde_json::from_str(&details).unwrap();
    assert_eq!(details["revision_before"], initial.revision);
    assert_eq!(details["revision_after"], initial.revision + 1);
    assert_eq!(details["changes"].as_array().unwrap().len(), 1);
    assert_eq!(
        details["changes"][0]["resource_mode"]["old_value"],
        "manual"
    );
    assert_eq!(
        details["changes"][0]["resource_mode"]["new_value"],
        "auto"
    );

    let reloaded = SettingsService::new(pool)
        .initialize(&AppLimits::default(), &AuthConfig::default(), 0, None)
        .await
        .expect("reload");
    assert_eq!(
        reloaded.resource_modes["upload_concurrent_processing_tasks"],
        ResourceMode::Auto
    );
}

#[tokio::test]
async fn settings_audit_records_only_changed_fields_with_values_and_apply_modes() {
    let pool = pool();
    db::prepare_schema(&pool, true).await.expect("schema");
    let service = SettingsService::new(pool.clone());
    let initial = service
        .initialize(&AppLimits::default(), &AuthConfig::default(), 0, None)
        .await
        .expect("initialize");

    let mut changes = serde_json::Map::new();
    changes.insert("issue_inactive_days".into(), serde_json::json!(14));
    changes.insert(
        "temp_results_max_total_size".into(),
        serde_json::json!(2_u64 * 1024 * 1024 * 1024),
    );
    changes.insert(
        "login_ip_limit_per_minute".into(),
        serde_json::json!(initial.configured.login_ip_limit_per_minute),
    );
    service
        .save(initial.revision, &changes, None)
        .await
        .expect("save settings");

    let audit: (String, String, String) = sqlx::query_as(
        "SELECT old_value,new_value,details_json FROM admin_audit_logs WHERE action='SETTINGS_UPDATED' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("settings update audit");
    let old_value: serde_json::Value = serde_json::from_str(&audit.0).unwrap();
    let new_value: serde_json::Value = serde_json::from_str(&audit.1).unwrap();
    let details: serde_json::Value = serde_json::from_str(&audit.2).unwrap();

    assert_eq!(old_value.as_object().unwrap().len(), 2);
    assert_eq!(new_value.as_object().unwrap().len(), 2);
    assert!(
        !old_value
            .as_object()
            .unwrap()
            .contains_key("login_ip_limit_per_minute")
    );
    assert_eq!(details["changed_fields"].as_array().unwrap().len(), 2);
    assert_eq!(details["changes"].as_array().unwrap().len(), 2);
    assert_eq!(details["revision_before"], initial.revision);
    assert_eq!(details["revision_after"], initial.revision + 1);
    let changes = details["changes"].as_array().unwrap();
    let issue_days = changes
        .iter()
        .find(|change| change["field"] == "issue_inactive_days")
        .expect("issue days change");
    assert_eq!(
        issue_days["old_value"],
        initial.configured.issue_inactive_days
    );
    assert_eq!(issue_days["new_value"], 14);
    assert_eq!(issue_days["apply_mode"], "hot");
    let total_size = changes
        .iter()
        .find(|change| change["field"] == "temp_results_max_total_size")
        .expect("result size change");
    assert_eq!(
        total_size["old_value"],
        initial.configured.temp_results_max_total_size
    );
    assert_eq!(total_size["new_value"], 2_u64 * 1024 * 1024 * 1024);
    assert_eq!(total_size["apply_mode"], "hot");
}

#[tokio::test]
async fn unsupported_resource_modes_are_rejected_without_writes() {
    let pool = pool();
    db::prepare_schema(&pool, true).await.expect("schema");
    let service = SettingsService::new(pool.clone());
    let initial = service
        .initialize(&AppLimits::default(), &AuthConfig::default(), 0, None)
        .await
        .expect("initialize");

    let mut modes = ResourceModes::new();
    modes.insert("api_max_search_window".into(), ResourceMode::Auto);
    let error = service
        .save_with_modes(initial.revision, &serde_json::Map::new(), &modes, None)
        .await
        .expect_err("unsupported Auto field");

    assert!(matches!(
        error,
        backend::error::AppError::PublicApi {
            code: "SETTINGS_INVALID_REQUEST",
            ..
        }
    ));
    assert_eq!(service.snapshot().await.revision, initial.revision);
}

#[tokio::test]
async fn save_requires_revision_and_applies_hot_values_atomically() {
    let pool = pool();
    db::prepare_schema(&pool, true).await.expect("schema");
    let service = SettingsService::new(pool.clone());
    let initial = service
        .initialize(&AppLimits::default(), &AuthConfig::default(), 0, None)
        .await
        .expect("initialization");

    let mut protected_changes = serde_json::Map::new();
    protected_changes.insert("api_default_search_results".into(), serde_json::json!(25));
    let protected = service
        .save(initial.revision, &protected_changes, None)
        .await
        .expect_err("default search results are system-managed");
    assert!(matches!(
        protected,
        backend::error::AppError::PublicApi {
            code: "SETTINGS_PROTECTED_FIELD",
            ..
        }
    ));

    let mut protected_runtime_changes = serde_json::Map::new();
    protected_runtime_changes.insert("search_tantivy_max_writers".into(), serde_json::json!(2));
    let protected = service
        .save(initial.revision, &protected_runtime_changes, None)
        .await
        .expect_err("search writer settings are system-managed");
    assert!(matches!(
        protected,
        backend::error::AppError::PublicApi {
            code: "SETTINGS_PROTECTED_FIELD",
            ..
        }
    ));

    let mut changes = serde_json::Map::new();
    changes.insert("temp_results_max_records".into(), serde_json::json!(80));
    changes.insert(
        "temp_results_max_result_size".into(),
        serde_json::json!(128 * 1024 * 1024),
    );
    changes.insert(
        "temp_results_max_total_size".into(),
        serde_json::json!(2_u64 * 1024 * 1024 * 1024),
    );
    changes.insert(
        "temp_results_max_scan_duration_seconds".into(),
        serde_json::json!(60),
    );
    let saved = service
        .save(initial.revision, &changes, None)
        .await
        .expect("save");
    assert_eq!(saved.snapshot.configured.temp_results_max_records, 80);
    assert_eq!(saved.snapshot.effective.temp_results_max_records, 80);
    assert!(
        saved
            .hot_applied_fields
            .contains(&"temp_results_max_records".to_owned())
    );

    let reloaded = service
        .initialize(&AppLimits::default(), &AuthConfig::default(), 0, None)
        .await
        .expect("reload");
    assert_eq!(reloaded.configured.temp_results_max_records, 80);
    assert_eq!(reloaded.effective.temp_results_max_records, 80);
    for (key, value) in &changes {
        assert_eq!(
            &serde_json::to_value(&saved.snapshot.effective).unwrap()[key],
            value
        );
        assert_eq!(
            &serde_json::to_value(&reloaded.effective).unwrap()[key],
            value
        );
        assert!(saved.hot_applied_fields.contains(key));
    }

    let stale = service.save(initial.revision, &changes, None).await;
    assert!(matches!(
        stale,
        Err(backend::error::AppError::PublicApi {
            code: "SETTINGS_REVISION_CONFLICT",
            ..
        })
    ));
}
