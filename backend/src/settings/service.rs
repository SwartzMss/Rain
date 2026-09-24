use std::sync::Arc;

use actix_web::http::StatusCode;
use sqlx::{Row, SqlitePool};
use tokio::sync::{Mutex, RwLock};

use crate::{
    config::{AppLimits, AuthConfig},
    db,
    error::AppError,
};

use super::{ResourceMode, ResourceModes, SaveResult, SettingsSnapshot, SettingsValues, metadata};

const RESOURCE_MODE_KEYS: [&str; 6] = [
    "upload_concurrent_processing_tasks",
    "upload_concurrent_receive_tasks",
    "search_tantivy_max_writers",
    "search_tantivy_writer_heap_size",
    "api_concurrent_line_reads",
    "temp_results_concurrent_materializations",
];

fn resource_mode_defaults() -> ResourceModes {
    RESOURCE_MODE_KEYS
        .into_iter()
        .map(|key| (key.to_owned(), ResourceMode::Manual))
        .collect()
}

fn resource_metadata(key: &str) -> Option<&'static metadata::FieldMetadata> {
    metadata::all().iter().find(|field| {
        serde_json::to_value(field.key)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .as_deref()
            == Some(key)
    })
}

fn normalize_resource_modes(modes: ResourceModes) -> Result<ResourceModes, AppError> {
    if let Some(key) = modes
        .keys()
        .find(|key| !RESOURCE_MODE_KEYS.contains(&key.as_str()))
    {
        return Err(AppError::Config(format!(
            "invalid resource setting mode key: {key}"
        )));
    }
    let mut normalized = resource_mode_defaults();
    normalized.extend(modes);
    Ok(normalized)
}

fn validate_resource_mode_patch(modes: &ResourceModes) -> Result<(), AppError> {
    if let Some(key) = modes
        .keys()
        .find(|key| !RESOURCE_MODE_KEYS.contains(&key.as_str()))
    {
        return Err(AppError::public(
            StatusCode::BAD_REQUEST,
            "SETTINGS_INVALID_REQUEST",
            format!("不支持为该配置项设置资源模式：{key}"),
        ));
    }
    Ok(())
}

fn validate_protected_changes(
    changes: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), AppError> {
    if changes.contains_key("argon2_concurrency") {
        return Err(AppError::public(
            StatusCode::UNPROCESSABLE_ENTITY,
            "SETTINGS_PROTECTED_FIELD",
            "该配置项由系统安全策略管理，管理员不可修改",
        ));
    }
    Ok(())
}

#[derive(Clone)]
pub struct SettingsService {
    pool: SqlitePool,
    snapshot: Arc<RwLock<Arc<SettingsSnapshot>>>,
    save_lock: Arc<Mutex<()>>,
}

fn is_restart_required(field: &str) -> bool {
    matches!(
        field,
        "argon2_concurrency"
            | "upload_concurrent_processing_tasks"
            | "upload_concurrent_receive_tasks"
            | "indexing_max_indexed_line_size"
            | "search_tantivy_max_writers"
            | "search_tantivy_writer_heap_size"
            | "api_concurrent_line_reads"
            | "temp_results_concurrent_materializations"
    )
}

fn pending_restart_fields(configured: &SettingsValues, effective: &SettingsValues) -> Vec<String> {
    let configured = serde_json::to_value(configured)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let effective = serde_json::to_value(effective)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    configured
        .keys()
        .filter(|key| is_restart_required(key) && configured.get(*key) != effective.get(*key))
        .cloned()
        .collect()
}

impl SettingsService {
    pub fn new(pool: SqlitePool) -> Self {
        Self::new_with_config(pool, &AppLimits::default(), &AuthConfig::default())
    }

    pub fn new_with_config(pool: SqlitePool, limits: &AppLimits, auth: &AuthConfig) -> Self {
        let defaults = SettingsValues::from_config(limits, auth);
        let snapshot = SettingsSnapshot {
            revision: 0,
            configured: defaults.clone(),
            effective: defaults,
            resource_modes: resource_mode_defaults(),
        };
        Self {
            pool,
            snapshot: Arc::new(RwLock::new(Arc::new(snapshot))),
            save_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn snapshot(&self) -> Arc<SettingsSnapshot> {
        self.snapshot.read().await.clone()
    }

    /// Publish values that were actually used to construct process runtimes.
    /// This does not touch the database or configuration revision.
    pub async fn set_effective(&self, effective: SettingsValues) -> Result<(), AppError> {
        effective
            .validate()
            .map_err(|errors| AppError::Config(format!("effective settings: {errors:?}")))?;
        let current = self.snapshot().await;
        let snapshot = Arc::new(SettingsSnapshot {
            revision: current.revision,
            configured: current.configured.clone(),
            effective,
            resource_modes: current.resource_modes.clone(),
        });
        *self.snapshot.write().await = snapshot;
        Ok(())
    }

    /// Reload the already-initialized row without mutating the database. This
    /// is used by management reads created by tests or by an embedded caller;
    /// normal application startup calls `initialize` once before serving HTTP.
    pub async fn load(&self) -> Result<Arc<SettingsSnapshot>, AppError> {
        let _guard = self.save_lock.lock().await;
        let previous = self.snapshot().await;
        let configured = self.load_values(&previous.configured).await?;
        let resource_modes = self.load_resource_modes().await?;
        let revision = self.load_revision().await?;
        let effective = if previous.revision == 0 {
            configured.clone()
        } else {
            merge_effective(&previous.effective, &configured)?
        };
        let snapshot = Arc::new(SettingsSnapshot {
            revision,
            effective,
            configured,
            resource_modes,
        });
        *self.snapshot.write().await = snapshot.clone();
        Ok(snapshot)
    }

    pub async fn save(
        &self,
        expected_revision: i64,
        changes: &serde_json::Map<String, serde_json::Value>,
        actor_user_id: Option<&str>,
    ) -> Result<SaveResult, AppError> {
        self.save_with_context(expected_revision, changes, actor_user_id, None, None)
            .await
    }

    pub async fn save_with_context(
        &self,
        expected_revision: i64,
        changes: &serde_json::Map<String, serde_json::Value>,
        actor_user_id: Option<&str>,
        client_ip: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<SaveResult, AppError> {
        self.save_with_context_and_modes(
            expected_revision,
            changes,
            &ResourceModes::new(),
            actor_user_id,
            client_ip,
            user_agent,
        )
        .await
    }

    pub async fn save_with_modes(
        &self,
        expected_revision: i64,
        changes: &serde_json::Map<String, serde_json::Value>,
        resource_modes: &ResourceModes,
        actor_user_id: Option<&str>,
    ) -> Result<SaveResult, AppError> {
        self.save_with_context_and_modes(
            expected_revision,
            changes,
            resource_modes,
            actor_user_id,
            None,
            None,
        )
        .await
    }

    pub async fn save_with_context_and_modes(
        &self,
        expected_revision: i64,
        changes: &serde_json::Map<String, serde_json::Value>,
        resource_modes: &ResourceModes,
        actor_user_id: Option<&str>,
        client_ip: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<SaveResult, AppError> {
        let _guard = self.save_lock.lock().await;
        let current = self.snapshot().await;
        if current.revision != expected_revision {
            return Err(AppError::public(
                StatusCode::CONFLICT,
                "SETTINGS_REVISION_CONFLICT",
                "配置已被其他管理员更新，请刷新后重试",
            ));
        }
        if changes.is_empty() && resource_modes.is_empty() {
            return Err(AppError::api(
                StatusCode::BAD_REQUEST,
                "SETTINGS_INVALID_REQUEST",
                "至少需要修改一个配置项",
            ));
        }
        validate_resource_mode_patch(resource_modes)?;
        validate_protected_changes(changes)?;
        if expected_revision == i64::MAX {
            return Err(AppError::public(
                StatusCode::CONFLICT,
                "SETTINGS_REVISION_EXHAUSTED",
                "配置版本已达到上限，请先备份数据库并执行维护",
            ));
        }
        let mut candidate_json = serde_json::to_value(&current.configured)
            .map_err(|error| AppError::Config(format!("serialize settings: {error}")))?
            .as_object()
            .cloned()
            .ok_or_else(|| AppError::Config("settings are not an object".into()))?;
        for (key, value) in changes {
            candidate_json.insert(key.clone(), value.clone());
        }
        for (key, mode) in resource_modes {
            if matches!(mode, ResourceMode::Auto) {
                let auto_value = resource_metadata(key)
                    .and_then(|field| field.auto_value)
                    .ok_or_else(|| {
                        AppError::public(
                            StatusCode::BAD_REQUEST,
                            "SETTINGS_INVALID_REQUEST",
                            format!("配置项不支持 Auto 模式：{key}"),
                        )
                    })?;
                candidate_json.insert(key.clone(), serde_json::json!(auto_value));
            }
        }
        let candidate: SettingsValues = serde_json::from_value(serde_json::Value::Object(
            candidate_json.clone(),
        ))
        .map_err(|error| {
            AppError::public(
                StatusCode::UNPROCESSABLE_ENTITY,
                "SETTINGS_VALIDATION_FAILED",
                format!("配置项无效：{error}"),
            )
        })?;
        candidate.validate().map_err(|errors| {
            AppError::public(
                StatusCode::UNPROCESSABLE_ENTITY,
                "SETTINGS_VALIDATION_FAILED",
                format!("配置项无效：{errors:?}"),
            )
        })?;
        let current_json = serde_json::to_value(&current.configured)
            .map_err(|error| AppError::Config(format!("serialize settings: {error}")))?;
        let current_map = current_json.as_object().expect("settings object");
        let mut changed_fields = Vec::new();
        for key in changes.keys() {
            if candidate_json.get(key) != current_map.get(key) {
                changed_fields.push(key.clone());
            }
        }
        let mut next_resource_modes = current.resource_modes.clone();
        let mut mode_changed_fields = Vec::new();
        for (key, mode) in resource_modes {
            if next_resource_modes.get(key) != Some(mode) {
                next_resource_modes.insert(key.clone(), *mode);
                mode_changed_fields.push(key.clone());
            }
        }
        for field in mode_changed_fields {
            if !changed_fields.contains(&field) {
                changed_fields.push(field);
            }
        }
        if changed_fields.is_empty() {
            return Ok(SaveResult {
                snapshot: (*current).clone(),
                changed_fields,
                hot_applied_fields: Vec::new(),
                pending_restart_fields: pending_restart_fields(
                    &current.configured,
                    &current.effective,
                ),
            });
        }
        let mut effective_json = serde_json::to_value(&current.effective)
            .map_err(|error| AppError::Config(format!("serialize effective settings: {error}")))?
            .as_object()
            .cloned()
            .ok_or_else(|| AppError::Config("effective settings are not an object".into()))?;
        let mut hot_applied_fields = Vec::new();
        for field in &changed_fields {
            if !is_restart_required(field) {
                effective_json.insert(field.clone(), candidate_json[field].clone());
                hot_applied_fields.push(field.clone());
            }
        }
        let effective: SettingsValues =
            serde_json::from_value(serde_json::Value::Object(effective_json))
                .map_err(|error| AppError::Config(format!("effective settings: {error}")))?;
        effective
            .validate()
            .map_err(|errors| AppError::Config(format!("effective settings: {errors:?}")))?;
        let operation_id = uuid::Uuid::new_v4().to_string();
        let old_json = serde_json::to_string(&current.configured)
            .map_err(|error| AppError::Config(error.to_string()))?;
        let new_json = serde_json::to_string(&candidate)
            .map_err(|error| AppError::Config(error.to_string()))?;
        let actor = actor_user_id.map(str::to_owned);
        let input = (
            expected_revision,
            actor,
            operation_id.clone(),
            old_json,
            new_json,
            client_ip.map(str::to_owned),
            user_agent.map(str::to_owned),
        );
        let pool = self.pool.clone();
        let update_values = candidate.clone();
        let old_values = current.configured.clone();
        let mut changed_for_audit = changes.clone();
        for (key, mode) in resource_modes {
            if current.resource_modes.get(key) != Some(mode) {
                changed_for_audit.insert(
                    key.clone(),
                    serde_json::to_value(mode)
                        .map_err(|error| AppError::Config(error.to_string()))?,
                );
            }
        }
        let resource_modes_json = serde_json::to_string(&next_resource_modes)
            .map_err(|error| AppError::Config(error.to_string()))?;
        db::write::run(&pool, "save system settings", &input, move |conn, input| {
            let values = update_values.clone();
            let old_values = old_values.clone();
            let changes = changed_for_audit.clone();
            let resource_modes_json = resource_modes_json.clone();
            Box::pin(async move {
                let updated = sqlx::query(
                    "UPDATE system_settings SET allow_registration=?,session_ttl_seconds=?,register_ip_limit_per_hour=?,login_ip_limit_per_minute=?,login_username_failure_limit_per_5_minutes=?,argon2_concurrency=?,issue_inactive_days=?,issue_max_content_size=?,archive_max_working_size=?,upload_concurrent_processing_tasks=?,upload_concurrent_receive_tasks=?,upload_max_tmp_bytes=?,indexing_max_indexed_line_size=?,search_tantivy_max_writers=?,search_tantivy_writer_heap_size=?,api_file_preview_size=?,api_max_preview_line_size=?,api_default_line_page_size=?,api_max_line_page_size=?,api_max_line_page_bytes=?,api_concurrent_line_reads=?,api_concurrent_line_reads_per_client=?,api_default_search_results=?,api_max_search_results=?,api_max_search_window=?,temp_results_max_result_size=?,temp_results_max_total_size=?,temp_results_max_records=?,temp_results_concurrent_materializations=?,temp_results_max_sources=?,temp_results_max_scan_bytes=?,temp_results_max_scan_duration_seconds=?,cleanup_exempt_usernames_json=?,resource_modes_json=?,cleanup_exempt_users_initialized=1,updated_by_user_id=?,updated_at=CURRENT_TIMESTAMP,revision=revision+1 WHERE id=1 AND revision=?",
                )
                .bind(values.allow_registration as i64).bind(values.session_ttl_seconds as i64).bind(values.register_ip_limit_per_hour as i64).bind(values.login_ip_limit_per_minute as i64).bind(values.login_username_failure_limit_per_5_minutes as i64).bind(values.argon2_concurrency as i64).bind(values.issue_inactive_days as i64).bind(values.issue_max_content_size as i64).bind(values.archive_max_working_size as i64).bind(values.upload_concurrent_processing_tasks as i64).bind(values.upload_concurrent_receive_tasks as i64).bind(values.upload_max_tmp_bytes as i64).bind(values.indexing_max_indexed_line_size as i64).bind(values.search_tantivy_max_writers as i64).bind(values.search_tantivy_writer_heap_size as i64).bind(values.api_file_preview_size as i64).bind(values.api_max_preview_line_size as i64).bind(values.api_default_line_page_size).bind(values.api_max_line_page_size).bind(values.api_max_line_page_bytes as i64).bind(values.api_concurrent_line_reads as i64).bind(values.api_concurrent_line_reads_per_client as i64).bind(values.api_default_search_results).bind(values.api_max_search_results).bind(values.api_max_search_window).bind(values.temp_results_max_result_size as i64).bind(values.temp_results_max_total_size as i64).bind(values.temp_results_max_records).bind(values.temp_results_concurrent_materializations as i64).bind(values.temp_results_max_sources as i64).bind(values.temp_results_max_scan_bytes as i64).bind(values.temp_results_max_scan_duration_seconds as i64).bind(serde_json::to_string(&values.cleanup_exempt_usernames).map_err(|error| AppError::Config(error.to_string()))?).bind(resource_modes_json).bind(input.1.as_deref()).bind(input.0)
                    .execute(&mut *conn).await.map_err(AppError::Database)?
                    .rows_affected();
                if updated != 1 {
                    return Err(AppError::public(StatusCode::CONFLICT, "SETTINGS_REVISION_CONFLICT", "配置已被其他管理员更新，请刷新后重试"));
                }
                let details = serde_json::json!({
                    "revision_before": input.0,
                    "revision_after": input.0 + 1,
                    "changed_fields": changes.keys().collect::<Vec<_>>(),
                    "hot_applied_fields": changed_fields_for_audit(&changes),
                }).to_string();
                sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,action,old_value,new_value,operation_id,details_json,client_ip,user_agent) VALUES(?,'USER',?,'SETTINGS_UPDATED',?,?,?,?,?,?)")
                    .bind(uuid::Uuid::new_v4().to_string()).bind(input.1.as_deref()).bind(&input.3).bind(&input.4).bind(&input.2).bind(&details).bind(input.5.as_deref()).bind(input.6.as_deref()).execute(&mut *conn).await.map_err(AppError::Database)?;
                let changed = |field: &str| changes.contains_key(field);
                if changed("allow_registration") || changed("login_ip_limit_per_minute") || changed("login_username_failure_limit_per_5_minutes") {
                    sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,action,old_value,new_value,client_ip,user_agent,operation_id,details_json) VALUES(?,'USER',?,'AUTH_SETTINGS_UPDATED',?,?,?,?,?,?)")
                        .bind(uuid::Uuid::new_v4().to_string()).bind(input.1.as_deref())
                        .bind(format!("registration={};ip_limit={};username_limit={}", old_values.allow_registration, old_values.login_ip_limit_per_minute, old_values.login_username_failure_limit_per_5_minutes))
                        .bind(format!("registration={};ip_limit={};username_limit={}", values.allow_registration, values.login_ip_limit_per_minute, values.login_username_failure_limit_per_5_minutes))
                        .bind(input.5.as_deref()).bind(input.6.as_deref()).bind(&input.2).bind(&details)
                        .execute(&mut *conn).await.map_err(AppError::Database)?;
                }
                if changed("issue_inactive_days") {
                    sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,action,old_value,new_value,client_ip,user_agent,operation_id,details_json) VALUES(?,'USER',?,'ISSUE_INACTIVE_SETTINGS_UPDATED',?,?,?,?,?,?)")
                        .bind(uuid::Uuid::new_v4().to_string()).bind(input.1.as_deref())
                        .bind(format!("issue_inactive_days={}", old_values.issue_inactive_days))
                        .bind(format!("issue_inactive_days={}", values.issue_inactive_days))
                        .bind(input.5.as_deref()).bind(input.6.as_deref()).bind(&input.2).bind(&details)
                        .execute(&mut *conn).await.map_err(AppError::Database)?;
                }
                if changed("cleanup_exempt_usernames") {
                    let old_cleanup = serde_json::to_string(&old_values.cleanup_exempt_usernames).map_err(|error| AppError::Config(error.to_string()))?;
                    let new_cleanup = serde_json::to_string(&values.cleanup_exempt_usernames).map_err(|error| AppError::Config(error.to_string()))?;
                    sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,action,old_value,new_value,client_ip,user_agent,operation_id,details_json) VALUES(?,'USER',?,'ISSUE_CLEANUP_EXEMPT_USERS_UPDATED',?,?,?,?,?,?)")
                        .bind(uuid::Uuid::new_v4().to_string()).bind(input.1.as_deref()).bind(old_cleanup).bind(new_cleanup).bind(input.5.as_deref()).bind(input.6.as_deref()).bind(&input.2).bind(&details)
                        .execute(&mut *conn).await.map_err(AppError::Database)?;
                }
                Ok(())
            })
        }).await?;
        let snapshot = Arc::new(SettingsSnapshot {
            revision: expected_revision + 1,
            configured: candidate,
            effective,
            resource_modes: next_resource_modes,
        });
        let result = SaveResult {
            snapshot: (*snapshot).clone(),
            changed_fields,
            hot_applied_fields,
            pending_restart_fields: pending_restart_fields(
                &snapshot.configured,
                &snapshot.effective,
            ),
        };
        *self.snapshot.write().await = snapshot;
        Ok(result)
    }

    /// Complete the one-time import of business settings. Existing non-null
    /// columns are intentionally preserved; `COALESCE` is what makes a fresh
    /// migration and an upgraded database follow the same path.
    pub async fn initialize(
        &self,
        limits: &AppLimits,
        auth: &AuthConfig,
        issue_inactive_days: usize,
        cleanup_json: Option<&str>,
    ) -> Result<Arc<SettingsSnapshot>, AppError> {
        let _guard = self.save_lock.lock().await;
        let mut values = SettingsValues::from_config(limits, auth);
        values.issue_inactive_days = issue_inactive_days;
        values
            .validate()
            .map_err(|errors| AppError::Config(format!("invalid settings: {errors:?}")))?;
        let cleanup = cleanup_json.unwrap_or("[]").to_owned();
        let closure_values = values.clone();
        let pool = self.pool.clone();
        db::write::run(&pool, "initialize system settings", &cleanup, move |conn, cleanup| {
            let values = closure_values.clone();
            Box::pin(async move {
                sqlx::query(
                    "INSERT OR IGNORE INTO system_settings(id,allow_registration) VALUES(1,?)",
                )
                .bind(values.allow_registration as i64)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                let needs_initialization: i64 = sqlx::query_scalar(
                    "SELECT CASE WHEN session_ttl_seconds IS NULL OR cleanup_exempt_users_initialized=0 THEN 1 ELSE 0 END FROM system_settings WHERE id=1",
                )
                .fetch_one(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                sqlx::query(
                    "UPDATE system_settings SET
                    session_ttl_seconds=COALESCE(session_ttl_seconds,?),
                    register_ip_limit_per_hour=COALESCE(register_ip_limit_per_hour,?),
                    argon2_concurrency=COALESCE(argon2_concurrency,?),
                    issue_inactive_days=COALESCE(issue_inactive_days,?),
                    issue_max_content_size=COALESCE(issue_max_content_size,?),
                    archive_max_working_size=COALESCE(archive_max_working_size,?),
                    upload_concurrent_processing_tasks=COALESCE(upload_concurrent_processing_tasks,?),
                    upload_concurrent_receive_tasks=COALESCE(upload_concurrent_receive_tasks,?),
                    upload_max_tmp_bytes=COALESCE(upload_max_tmp_bytes,?),
                    indexing_max_indexed_line_size=COALESCE(indexing_max_indexed_line_size,?),
                    search_tantivy_max_writers=COALESCE(search_tantivy_max_writers,?),
                    search_tantivy_writer_heap_size=COALESCE(search_tantivy_writer_heap_size,?),
                    api_file_preview_size=COALESCE(api_file_preview_size,?),
                    api_max_preview_line_size=COALESCE(api_max_preview_line_size,?),
                    api_default_line_page_size=COALESCE(api_default_line_page_size,?),
                    api_max_line_page_size=COALESCE(api_max_line_page_size,?),
                    api_max_line_page_bytes=COALESCE(api_max_line_page_bytes,?),
                    api_concurrent_line_reads=COALESCE(api_concurrent_line_reads,?),
                    api_concurrent_line_reads_per_client=COALESCE(api_concurrent_line_reads_per_client,?),
                    api_default_search_results=COALESCE(api_default_search_results,?),
                    api_max_search_results=COALESCE(api_max_search_results,?),
                    api_max_search_window=COALESCE(api_max_search_window,?),
                    temp_results_max_result_size=COALESCE(temp_results_max_result_size,?),
                    temp_results_max_total_size=COALESCE(temp_results_max_total_size,?),
                    temp_results_max_records=COALESCE(temp_results_max_records,?),
                    temp_results_concurrent_materializations=COALESCE(temp_results_concurrent_materializations,?),
                    temp_results_max_sources=COALESCE(temp_results_max_sources,?),
                    temp_results_max_scan_bytes=COALESCE(temp_results_max_scan_bytes,?),
                    temp_results_max_scan_duration_seconds=COALESCE(temp_results_max_scan_duration_seconds,?),
                    cleanup_exempt_usernames_json=CASE WHEN cleanup_exempt_users_initialized=0 THEN ? ELSE cleanup_exempt_usernames_json END,
                    cleanup_exempt_users_initialized=CASE WHEN cleanup_exempt_users_initialized=0 THEN 1 ELSE cleanup_exempt_users_initialized END
                    WHERE id=1 AND (session_ttl_seconds IS NULL OR register_ip_limit_per_hour IS NULL OR argon2_concurrency IS NULL OR issue_inactive_days IS NULL OR issue_max_content_size IS NULL OR archive_max_working_size IS NULL OR upload_concurrent_processing_tasks IS NULL OR upload_concurrent_receive_tasks IS NULL OR upload_max_tmp_bytes IS NULL OR indexing_max_indexed_line_size IS NULL OR search_tantivy_max_writers IS NULL OR search_tantivy_writer_heap_size IS NULL OR api_file_preview_size IS NULL OR api_max_preview_line_size IS NULL OR api_default_line_page_size IS NULL OR api_max_line_page_size IS NULL OR api_max_line_page_bytes IS NULL OR api_concurrent_line_reads IS NULL OR api_concurrent_line_reads_per_client IS NULL OR api_default_search_results IS NULL OR api_max_search_results IS NULL OR api_max_search_window IS NULL OR temp_results_max_result_size IS NULL OR temp_results_max_total_size IS NULL OR temp_results_max_records IS NULL OR temp_results_concurrent_materializations IS NULL OR temp_results_max_sources IS NULL OR temp_results_max_scan_bytes IS NULL OR temp_results_max_scan_duration_seconds IS NULL OR cleanup_exempt_users_initialized=0)",
                )
                .bind(values.session_ttl_seconds as i64)
                .bind(values.register_ip_limit_per_hour as i64)
                .bind(values.argon2_concurrency as i64)
                .bind(values.issue_inactive_days as i64)
                .bind(values.issue_max_content_size as i64)
                .bind(values.archive_max_working_size as i64)
                .bind(values.upload_concurrent_processing_tasks as i64)
                .bind(values.upload_concurrent_receive_tasks as i64)
                .bind(values.upload_max_tmp_bytes as i64)
                .bind(values.indexing_max_indexed_line_size as i64)
                .bind(values.search_tantivy_max_writers as i64)
                .bind(values.search_tantivy_writer_heap_size as i64)
                .bind(values.api_file_preview_size as i64)
                .bind(values.api_max_preview_line_size as i64)
                .bind(values.api_default_line_page_size)
                .bind(values.api_max_line_page_size)
                .bind(values.api_max_line_page_bytes as i64)
                .bind(values.api_concurrent_line_reads as i64)
                .bind(values.api_concurrent_line_reads_per_client as i64)
                .bind(values.api_default_search_results)
                .bind(values.api_max_search_results)
                .bind(values.api_max_search_window)
                .bind(values.temp_results_max_result_size as i64)
                .bind(values.temp_results_max_total_size as i64)
                .bind(values.temp_results_max_records)
                .bind(values.temp_results_concurrent_materializations as i64)
                .bind(values.temp_results_max_sources as i64)
                .bind(values.temp_results_max_scan_bytes as i64)
                .bind(values.temp_results_max_scan_duration_seconds as i64)
                .bind(cleanup.as_str())
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                if needs_initialization != 0 {
                    sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,action,new_value,operation_id,details_json) VALUES(?,'SYSTEM','SYSTEM_SETTINGS_INITIALIZED',?,?,?)")
                        .bind(uuid::Uuid::new_v4().to_string())
                        .bind(serde_json::to_string(&values).map_err(|error| AppError::Config(error.to_string()))?)
                        .bind(uuid::Uuid::new_v4().to_string())
                        .bind(serde_json::json!({"source": "legacy_env_or_defaults", "revision": 0}).to_string())
                        .execute(&mut *conn)
                        .await
                        .map_err(AppError::Database)?;
                }
                Ok(())
            })
        })
        .await?;
        let configured = self.load_values(&values).await?;
        let resource_modes = self.load_resource_modes().await?;
        let revision = self.load_revision().await?;
        let previous = self.snapshot().await;
        let effective = if previous.revision == 0 {
            configured.clone()
        } else {
            merge_effective(&previous.effective, &configured)?
        };
        let snapshot = Arc::new(SettingsSnapshot {
            revision,
            effective,
            configured,
            resource_modes,
        });
        *self.snapshot.write().await = snapshot.clone();
        Ok(snapshot)
    }

    async fn load_revision(&self) -> Result<i64, AppError> {
        sqlx::query_scalar("SELECT COALESCE(revision,0) FROM system_settings WHERE id=1")
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::Database)
    }

    async fn load_resource_modes(&self) -> Result<ResourceModes, AppError> {
        let raw: String = sqlx::query_scalar(
            "SELECT COALESCE(resource_modes_json,'{}') FROM system_settings WHERE id=1",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;
        let modes: ResourceModes = serde_json::from_str(&raw)
            .map_err(|error| AppError::Config(format!("invalid resource modes: {error}")))?;
        normalize_resource_modes(modes)
    }

    async fn load_values(&self, fallback: &SettingsValues) -> Result<SettingsValues, AppError> {
        let row = sqlx::query("SELECT * FROM system_settings WHERE id=1")
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::Database)?;
        let optional_i64 = |name: &str| -> Result<Option<i64>, AppError> {
            row.try_get(name).map_err(AppError::Database)
        };
        let u64_value = |name: &str, default: u64| -> Result<u64, AppError> {
            optional_i64(name)?
                .map(|value| value.try_into())
                .transpose()
                .map_err(|_| AppError::Config(format!("invalid setting {name}")))
                .map(|value| value.unwrap_or(default))
        };
        let i64_value = |name: &str, default: i64| -> Result<i64, AppError> {
            Ok(optional_i64(name)?.unwrap_or(default))
        };
        let mut values = fallback.clone();
        values.allow_registration = row
            .try_get::<i64, _>("allow_registration")
            .map_err(AppError::Database)?
            != 0;
        values.session_ttl_seconds = u64_value("session_ttl_seconds", values.session_ttl_seconds)?;
        values.register_ip_limit_per_hour = u64_value(
            "register_ip_limit_per_hour",
            values.register_ip_limit_per_hour as u64,
        )? as usize;
        values.login_ip_limit_per_minute = row
            .try_get::<i64, _>("login_ip_limit_per_minute")
            .map_err(AppError::Database)? as usize;
        values.login_username_failure_limit_per_5_minutes =
            row.try_get::<i64, _>("login_username_failure_limit_per_5_minutes")
                .map_err(AppError::Database)? as usize;
        values.argon2_concurrency =
            u64_value("argon2_concurrency", values.argon2_concurrency as u64)? as usize;
        values.issue_inactive_days = row
            .try_get::<i64, _>("issue_inactive_days")
            .map_err(AppError::Database)?
            .max(0) as usize;
        let cleanup: String = row
            .try_get("cleanup_exempt_usernames_json")
            .map_err(AppError::Database)?;
        values.cleanup_exempt_usernames = serde_json::from_str(&cleanup)
            .map_err(|error| AppError::Config(format!("invalid cleanup whitelist: {error}")))?;
        values.issue_max_content_size =
            u64_value("issue_max_content_size", values.issue_max_content_size)?;
        values.archive_max_working_size =
            u64_value("archive_max_working_size", values.archive_max_working_size)?;
        values.upload_concurrent_processing_tasks = u64_value(
            "upload_concurrent_processing_tasks",
            values.upload_concurrent_processing_tasks as u64,
        )? as usize;
        values.upload_concurrent_receive_tasks = u64_value(
            "upload_concurrent_receive_tasks",
            values.upload_concurrent_receive_tasks as u64,
        )? as usize;
        values.upload_max_tmp_bytes =
            u64_value("upload_max_tmp_bytes", values.upload_max_tmp_bytes)?;
        values.indexing_max_indexed_line_size = u64_value(
            "indexing_max_indexed_line_size",
            values.indexing_max_indexed_line_size,
        )?;
        values.search_tantivy_max_writers = u64_value(
            "search_tantivy_max_writers",
            values.search_tantivy_max_writers as u64,
        )? as usize;
        values.search_tantivy_writer_heap_size = u64_value(
            "search_tantivy_writer_heap_size",
            values.search_tantivy_writer_heap_size,
        )?;
        values.api_file_preview_size =
            u64_value("api_file_preview_size", values.api_file_preview_size)?;
        values.api_max_preview_line_size = u64_value(
            "api_max_preview_line_size",
            values.api_max_preview_line_size,
        )?;
        values.api_default_line_page_size = i64_value(
            "api_default_line_page_size",
            values.api_default_line_page_size,
        )?;
        values.api_max_line_page_size =
            i64_value("api_max_line_page_size", values.api_max_line_page_size)?;
        values.api_max_line_page_bytes =
            u64_value("api_max_line_page_bytes", values.api_max_line_page_bytes)?;
        values.api_concurrent_line_reads = u64_value(
            "api_concurrent_line_reads",
            values.api_concurrent_line_reads as u64,
        )? as usize;
        values.api_concurrent_line_reads_per_client = u64_value(
            "api_concurrent_line_reads_per_client",
            values.api_concurrent_line_reads_per_client as u64,
        )? as usize;
        values.api_default_search_results = i64_value(
            "api_default_search_results",
            values.api_default_search_results,
        )?;
        values.api_max_search_results =
            i64_value("api_max_search_results", values.api_max_search_results)?;
        values.api_max_search_window =
            i64_value("api_max_search_window", values.api_max_search_window)?;
        values.temp_results_max_result_size = u64_value(
            "temp_results_max_result_size",
            values.temp_results_max_result_size,
        )?;
        values.temp_results_max_total_size = u64_value(
            "temp_results_max_total_size",
            values.temp_results_max_total_size,
        )?;
        values.temp_results_max_records =
            i64_value("temp_results_max_records", values.temp_results_max_records)?;
        values.temp_results_concurrent_materializations = u64_value(
            "temp_results_concurrent_materializations",
            values.temp_results_concurrent_materializations as u64,
        )? as usize;
        values.temp_results_max_sources = u64_value(
            "temp_results_max_sources",
            values.temp_results_max_sources as u64,
        )? as usize;
        values.temp_results_max_scan_bytes = u64_value(
            "temp_results_max_scan_bytes",
            values.temp_results_max_scan_bytes,
        )?;
        values.temp_results_max_scan_duration_seconds = u64_value(
            "temp_results_max_scan_duration_seconds",
            values.temp_results_max_scan_duration_seconds,
        )?;
        values
            .validate()
            .map_err(|errors| AppError::Config(format!("invalid database settings: {errors:?}")))?;
        Ok(values)
    }
}

fn merge_effective(
    previous: &SettingsValues,
    configured: &SettingsValues,
) -> Result<SettingsValues, AppError> {
    let mut effective_json = serde_json::to_value(previous)
        .map_err(|error| AppError::Config(error.to_string()))?
        .as_object()
        .cloned()
        .ok_or_else(|| AppError::Config("effective settings are not an object".into()))?;
    let configured_json = serde_json::to_value(configured)
        .map_err(|error| AppError::Config(error.to_string()))?
        .as_object()
        .cloned()
        .ok_or_else(|| AppError::Config("configured settings are not an object".into()))?;
    for (field, value) in configured_json {
        if !is_restart_required(&field) {
            effective_json.insert(field, value);
        }
    }
    serde_json::from_value(serde_json::Value::Object(effective_json))
        .map_err(|error| AppError::Config(format!("effective settings: {error}")))
}

fn changed_fields_for_audit(changes: &serde_json::Map<String, serde_json::Value>) -> Vec<&str> {
    changes
        .keys()
        .filter_map(|field| (!is_restart_required(field)).then_some(field.as_str()))
        .collect()
}
