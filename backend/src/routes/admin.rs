use actix_web::{HttpRequest, HttpResponse, delete, get, http::StatusCode, patch, post, web};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sqlx::{QueryBuilder, Sqlite};
use std::time::Instant;
use uuid::Uuid;

use crate::{
    AppState,
    auth::{UserRole, UserStatus, extractor::RequireAdmin},
    error::AppError,
    models::admin::*,
    services::issue_cleanup_policy::IssueCleanupPolicy,
    settings::{self, SettingsValues},
};

fn limit(value: Option<i64>) -> Result<i64, AppError> {
    let value = value.unwrap_or(50);
    if !(1..=100).contains(&value) {
        Err(AppError::api(
            StatusCode::BAD_REQUEST,
            "BAD_REQUEST",
            "limit 必须为 1 到 100",
        ))
    } else {
        Ok(value)
    }
}
fn decode_cursor(value: Option<&str>) -> Result<Option<(String, String)>, AppError> {
    value
        .map(|v| {
            let raw = URL_SAFE_NO_PAD.decode(v).map_err(|_| {
                AppError::api(StatusCode::BAD_REQUEST, "BAD_REQUEST", "cursor 无效")
            })?;
            let raw = String::from_utf8(raw).map_err(|_| {
                AppError::api(StatusCode::BAD_REQUEST, "BAD_REQUEST", "cursor 无效")
            })?;
            let (a, b) = raw.split_once('|').ok_or_else(|| {
                AppError::api(StatusCode::BAD_REQUEST, "BAD_REQUEST", "cursor 无效")
            })?;
            Ok((a.into(), b.into()))
        })
        .transpose()
}
fn encode_cursor(created_at: &str, id: &str) -> String {
    URL_SAFE_NO_PAD.encode(format!("{created_at}|{id}"))
}
fn parse_status(value: &str) -> Result<UserStatus, AppError> {
    value.parse().map_err(|_| {
        AppError::api(
            StatusCode::BAD_REQUEST,
            "INVALID_USER_STATUS",
            "用户状态无效",
        )
    })
}

#[get("/admin/auth-rate-limits")]
pub async fn auth_rate_limits(
    _admin: RequireAdmin,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let now = Instant::now();
    let mut limits = state
        .auth_runtime
        .rate_limits
        .lock()
        .map_err(|_| AppError::Config("认证限流状态不可用".into()))?;
    let username_limit = state
        .auth_runtime
        .login_username_failure_limit_per_5_minutes
        .load(std::sync::atomic::Ordering::Acquire);
    let ip_limit = state
        .auth_runtime
        .login_ip_limit_per_minute
        .load(std::sync::atomic::Ordering::Acquire);
    let mut usernames = Vec::new();
    limits.login_username_failure.retain(|_, bucket| {
        bucket.prune(now);
        !bucket.events.is_empty()
    });
    for (key, bucket) in &mut limits.login_username_failure {
        let retry_from = if bucket.events.len() >= username_limit {
            bucket
                .events
                .get(bucket.events.len() - username_limit)
                .copied()
        } else {
            None
        };
        usernames.push(AuthRateLimitEntry {
            key: key.clone(),
            username: Some(
                key.strip_prefix("login:username:")
                    .unwrap_or(key)
                    .to_owned(),
            ),
            ip: None,
            current_count: bucket.events.len(),
            limit: username_limit,
            window_seconds: 300,
            last_event_at: bucket.event_times.back().map(ToString::to_string),
            retry_after_seconds: retry_from
                .map(|event| 300u64.saturating_sub(now.duration_since(event).as_secs()))
                .unwrap_or(0),
            limited: bucket.events.len() >= username_limit,
        });
    }
    let mut ips = Vec::new();
    limits.login_ip.retain(|_, bucket| {
        bucket.prune(now);
        !bucket.events.is_empty()
    });
    for (key, bucket) in &mut limits.login_ip {
        let retry_from = if bucket.events.len() >= ip_limit {
            bucket.events.get(bucket.events.len() - ip_limit).copied()
        } else {
            None
        };
        ips.push(AuthRateLimitEntry {
            key: key.clone(),
            username: None,
            ip: Some(key.strip_prefix("login:ip:").unwrap_or(key).to_owned()),
            current_count: bucket.events.len(),
            limit: ip_limit,
            window_seconds: 60,
            last_event_at: bucket.event_times.back().map(ToString::to_string),
            retry_after_seconds: retry_from
                .map(|event| 60u64.saturating_sub(now.duration_since(event).as_secs()))
                .unwrap_or(0),
            limited: bucket.events.len() >= ip_limit,
        });
    }
    Ok(HttpResponse::Ok()
        .json(serde_json::json!({"username_failures": usernames, "login_ips": ips})))
}

async fn clear_auth_bucket(
    state: &web::Data<AppState>,
    admin: &RequireAdmin,
    key: &str,
    username: bool,
    req: &HttpRequest,
) -> Result<HttpResponse, AppError> {
    let count = {
        let limits = state
            .auth_runtime
            .rate_limits
            .lock()
            .map_err(|_| AppError::Config("认证限流状态不可用".into()))?;
        let bucket = if username {
            limits.login_username_failure.get(key)
        } else {
            limits.login_ip.get(key)
        };
        bucket.map(|v| v.events.len()).unwrap_or(0)
    };
    let action = if username {
        "AUTH_RATE_LIMIT_USERNAME_CLEARED"
    } else {
        "AUTH_RATE_LIMIT_IP_CLEARED"
    };
    let input = (
        Uuid::new_v4().to_string(),
        admin.0.id.clone(),
        action.to_owned(),
        format!("{key}:count={count}"),
        req.peer_addr().map(|a| a.ip().to_string()),
        req.headers()
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
    );
    crate::db::write::run(&state.db.pool, "clear auth rate limit", &input, |conn, input| {
        Box::pin(async move {
            sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,action,old_value,client_ip,user_agent) VALUES(?,'USER',?,?,?, ?, ?)")
                .bind(&input.0)
                .bind(&input.1)
                .bind(&input.2)
                .bind(&input.3)
                .bind(input.4.as_deref())
                .bind(input.5.as_deref())
                .execute(conn)
                .await
                .map(|_| ())
                .map_err(AppError::Database)
        })
    })
    .await?;
    let mut limits = state
        .auth_runtime
        .rate_limits
        .lock()
        .map_err(|_| AppError::Config("认证限流状态不可用".into()))?;
    if username {
        limits.login_username_failure.remove(key);
    } else {
        limits.login_ip.remove(key);
    }
    Ok(HttpResponse::NoContent().finish())
}

#[delete("/admin/auth-rate-limits/usernames/{key}")]
pub async fn clear_username_rate_limit(
    admin: RequireAdmin,
    state: web::Data<AppState>,
    path: web::Path<String>,
    req: HttpRequest,
) -> Result<HttpResponse, AppError> {
    clear_auth_bucket(&state, &admin, &path, true, &req).await
}

#[delete("/admin/auth-rate-limits/ips/{key}")]
pub async fn clear_ip_rate_limit(
    admin: RequireAdmin,
    state: web::Data<AppState>,
    path: web::Path<String>,
    req: HttpRequest,
) -> Result<HttpResponse, AppError> {
    clear_auth_bucket(&state, &admin, &path, false, &req).await
}

#[delete("/admin/auth-rate-limits/usernames")]
pub async fn clear_username_rate_limits(
    admin: RequireAdmin,
    state: web::Data<AppState>,
    req: HttpRequest,
) -> Result<HttpResponse, AppError> {
    clear_all_auth_limits(&state, &admin, true, &req).await?;
    Ok(HttpResponse::NoContent().finish())
}

#[delete("/admin/auth-rate-limits/ips")]
pub async fn clear_ip_rate_limits(
    admin: RequireAdmin,
    state: web::Data<AppState>,
    req: HttpRequest,
) -> Result<HttpResponse, AppError> {
    clear_all_auth_limits(&state, &admin, false, &req).await?;
    Ok(HttpResponse::NoContent().finish())
}

async fn clear_all_auth_limits(
    state: &web::Data<AppState>,
    admin: &RequireAdmin,
    username: bool,
    req: &HttpRequest,
) -> Result<(), AppError> {
    let (key_count, event_count) = {
        let limits = state
            .auth_runtime
            .rate_limits
            .lock()
            .map_err(|_| AppError::Config("认证限流状态不可用".into()))?;
        let source = if username {
            &limits.login_username_failure
        } else {
            &limits.login_ip
        };
        (
            source.len(),
            source
                .values()
                .map(|bucket| bucket.events.len())
                .sum::<usize>(),
        )
    };
    let action = if username {
        "AUTH_RATE_LIMIT_USERNAMES_CLEARED"
    } else {
        "AUTH_RATE_LIMIT_IPS_CLEARED"
    };
    let input = (
        Uuid::new_v4().to_string(),
        admin.0.id.clone(),
        action.to_owned(),
        format!("keys={key_count};count={event_count}"),
        req.peer_addr().map(|a| a.ip().to_string()),
        req.headers()
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
    );
    crate::db::write::transaction(&state.db.pool, "clear all auth rate limits", move |conn| {
        let input = input.clone();
        Box::pin(async move {
            sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,action,old_value,client_ip,user_agent) VALUES(?,'USER',?,?,?, ?, ?)")
                .bind(&input.0)
                .bind(&input.1)
                .bind(&input.2)
                .bind(&input.3)
                .bind(input.4.as_deref())
                .bind(input.5.as_deref())
                .execute(conn)
                .await
                .map(|_| ())
                .map_err(AppError::Database)
        })
    })
    .await?;
    let mut limits = state
        .auth_runtime
        .rate_limits
        .lock()
        .map_err(|_| AppError::Config("认证限流状态不可用".into()))?;
    if username {
        limits.login_username_failure.clear();
    } else {
        limits.login_ip.clear();
    }
    Ok(())
}

#[get("/admin/settings")]
pub async fn get_settings(
    _admin: RequireAdmin,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let snapshot = state.settings.load().await?;
    Ok(HttpResponse::Ok()
        .insert_header(("Cache-Control", "no-store, private"))
        .json(settings_response_with_metadata(&state, &snapshot).await?))
}

#[patch("/admin/settings")]
pub async fn update_settings(
    req: HttpRequest,
    admin: RequireAdmin,
    state: web::Data<AppState>,
    body: web::Json<UpdateRegistrationSettings>,
) -> Result<HttpResponse, AppError> {
    let current = state.settings.load().await?;
    if body.changes.as_ref().is_some_and(|changes| {
        changes.keys().any(|key| {
            settings::metadata::all().iter().any(|field| {
                field.protected
                    && serde_json::to_value(field.key).ok().as_ref()
                        == Some(&serde_json::Value::String(key.clone()))
            })
        })
    }) {
        return Err(AppError::public(
            StatusCode::UNPROCESSABLE_ENTITY,
            "SETTINGS_PROTECTED_FIELD",
            "该配置项由系统安全策略管理，管理员不可修改",
        ));
    }
    if body.session_ttl_seconds.is_some() || body.register_ip_limit_per_hour.is_some() {
        return Err(AppError::public(
            StatusCode::UNPROCESSABLE_ENTITY,
            "SETTINGS_PROTECTED_FIELD",
            "该配置项由系统安全策略管理，管理员不可修改",
        ));
    }
    if let Some(value) = body.issue_inactive_days.as_ref()
        && !value
            .as_i64()
            .is_some_and(|days| days == 0 || (7..=30).contains(&days))
    {
        return Err(AppError::api(
            StatusCode::BAD_REQUEST,
            "INVALID_ISSUE_INACTIVE_DAYS",
            "Issue 非活跃天数必须为 0，或 7 到 30 的整数",
        ));
    }
    if body
        .login_ip_limit_per_minute
        .is_some_and(|value| !(1..=1000).contains(&value))
        || body
            .login_username_failure_limit_per_5_minutes
            .is_some_and(|value| !(1..=100).contains(&value))
    {
        return Err(AppError::api(
            StatusCode::BAD_REQUEST,
            "INVALID_RATE_LIMIT",
            "IP 限流阈值必须为 1 到 1000，用户名限流阈值必须为 1 到 100",
        ));
    }
    let mut changes = body.changes.clone().unwrap_or_default();
    if body.changes.is_some()
        && (body.allow_registration.is_some()
            || body.login_ip_limit_per_minute.is_some()
            || body.login_username_failure_limit_per_5_minutes.is_some()
            || body.issue_inactive_days.is_some()
            || body.cleanup_exempt_usernames.is_some())
    {
        return Err(AppError::api(
            StatusCode::BAD_REQUEST,
            "SETTINGS_INVALID_REQUEST",
            "changes 与旧版扁平字段不能同时使用",
        ));
    }
    if body.changes.is_none() {
        if let Some(value) = body.allow_registration {
            changes.insert("allow_registration".into(), serde_json::json!(value));
        }
        if let Some(value) = body.login_ip_limit_per_minute {
            changes.insert("login_ip_limit_per_minute".into(), serde_json::json!(value));
        }
        if let Some(value) = body.login_username_failure_limit_per_5_minutes {
            changes.insert(
                "login_username_failure_limit_per_5_minutes".into(),
                serde_json::json!(value),
            );
        }
        if let Some(value) = &body.issue_inactive_days {
            changes.insert("issue_inactive_days".into(), value.clone());
        }
        if let Some(value) = &body.cleanup_exempt_usernames {
            let value = value.clone().ok_or_else(|| {
                AppError::api(
                    StatusCode::BAD_REQUEST,
                    "SETTINGS_INVALID_REQUEST",
                    "白名单必须为用户名数组",
                )
            })?;
            let policy = IssueCleanupPolicy::from_usernames(&value).map_err(|username| {
                AppError::public(
                    StatusCode::BAD_REQUEST,
                    "SETTINGS_INVALID_REQUEST",
                    format!("自动清理白名单中的用户名无效：{username}"),
                )
            })?;
            changes.insert(
                "cleanup_exempt_usernames".into(),
                serde_json::from_str(policy.exempt_usernames_json())
                    .map_err(|error| AppError::Config(error.to_string()))?,
            );
        }
    }
    let expected_revision = match body.expected_revision.as_ref() {
        Some(value) => parse_revision(value)?,
        None if body.changes.is_some() || body.resource_modes.is_some() => {
            return Err(AppError::api(
                StatusCode::PRECONDITION_REQUIRED,
                "SETTINGS_REVISION_REQUIRED",
                "保存配置必须携带 revision，请刷新后重试",
            ));
        }
        // Keep the pre-v2 flat request usable for existing operators. New
        // clients use `changes` and must provide an explicit revision.
        None => current.revision,
    };
    let client_ip = req.peer_addr().map(|address| address.ip().to_string());
    let user_agent = req
        .headers()
        .get("user-agent")
        .and_then(|value| value.to_str().ok());
    let resource_modes = body.resource_modes.clone().unwrap_or_default();
    let result = state
        .settings
        .save_with_context_and_modes(
            expected_revision,
            &changes,
            &resource_modes,
            Some(&admin.0.id),
            client_ip.as_deref(),
            user_agent,
        )
        .await?;
    state
        .auth_runtime
        .set_registration_allowed(result.snapshot.effective.allow_registration);
    state.auth_runtime.login_ip_limit_per_minute.store(
        result.snapshot.effective.login_ip_limit_per_minute,
        std::sync::atomic::Ordering::Release,
    );
    state
        .auth_runtime
        .login_username_failure_limit_per_5_minutes
        .store(
            result
                .snapshot
                .effective
                .login_username_failure_limit_per_5_minutes,
            std::sync::atomic::Ordering::Release,
        );
    state.auth_runtime.session_ttl_seconds.store(
        result.snapshot.effective.session_ttl_seconds,
        std::sync::atomic::Ordering::Release,
    );
    state.auth_runtime.register_ip_limit_per_hour.store(
        result.snapshot.effective.register_ip_limit_per_hour,
        std::sync::atomic::Ordering::Release,
    );
    state.line_read_per_client.store(
        result
            .snapshot
            .effective
            .api_concurrent_line_reads_per_client,
        std::sync::atomic::Ordering::Release,
    );
    state.upload.tmp_max_bytes.store(
        result.snapshot.effective.upload_max_tmp_bytes,
        std::sync::atomic::Ordering::Release,
    );
    state.issue_inactive_days.store(
        result.snapshot.effective.issue_inactive_days,
        std::sync::atomic::Ordering::Release,
    );
    let cleanup_policy =
        IssueCleanupPolicy::from_usernames(&result.snapshot.effective.cleanup_exempt_usernames)
            .map_err(|username| {
                AppError::Config(format!("saved cleanup whitelist is invalid: {username}"))
            })?;
    state.set_cleanup_policy(cleanup_policy);
    Ok(HttpResponse::Ok()
        .insert_header(("Cache-Control", "no-store, private"))
        .json(
            settings_response_with_metadata(&state, &std::sync::Arc::new(result.snapshot)).await?,
        ))
}

fn parse_revision(value: &serde_json::Value) -> Result<i64, AppError> {
    let parsed = match value {
        serde_json::Value::String(value) => value.parse::<i64>().ok(),
        serde_json::Value::Number(value) => value.as_i64(),
        _ => None,
    };
    parsed.filter(|revision| *revision >= 0).ok_or_else(|| {
        AppError::api(
            StatusCode::BAD_REQUEST,
            "SETTINGS_INVALID_REQUEST",
            "expected_revision 必须是非负十进制整数",
        )
    })
}

async fn settings_response_with_metadata(
    state: &web::Data<AppState>,
    snapshot: &std::sync::Arc<crate::settings::SettingsSnapshot>,
) -> Result<serde_json::Value, AppError> {
    let (updated_at, updated_by_username): (String, Option<String>) = sqlx::query_as(
        "SELECT s.updated_at,u.username FROM system_settings s LEFT JOIN users u ON u.id=s.updated_by_user_id WHERE s.id=1",
    )
    .fetch_one(&state.db.pool)
    .await
    .map_err(AppError::Database)?;
    let mut response = settings_response(snapshot);
    if let Some(plan) = &state.runtime_plan {
        response["runtime"] = serde_json::json!({
            "policy_version": "v1",
            "resources": plan.resources,
            "upload_concurrent_processing_tasks": plan.upload_processing_tasks,
            "search_tantivy_max_writers": plan.tantivy_max_writers,
            "search_tantivy_writer_heap_size": plan.tantivy_writer_heap_size,
            "estimated_bytes": plan.estimated_bytes,
            "memory_estimate": plan.memory_estimate,
            "current_process_rss_bytes": crate::runtime_adaptive::current_process_rss_bytes(),
            "adaptive_memory_target_bytes": plan.adaptive_memory_target_bytes,
            "warnings": plan.warnings,
            "decisions": plan.decisions,
        });
    }
    let jobs = crate::job_runtime::JobType::ALL
        .into_iter()
        .map(|kind| {
            let metrics = state.jobs.snapshot().for_type(kind);
            (
                kind.as_str().to_owned(),
                serde_json::json!({
                    "active": metrics.active,
                    "queued": metrics.queued,
                    "completed_total": metrics.completed_total,
                    "failed_total": metrics.failed_total,
                    "cancelled_total": metrics.cancelled_total,
                    "timed_out_total": metrics.timed_out_total,
                    "duration_count": metrics.duration_count,
                    "duration_sum_ms": metrics.duration_sum_ms,
                    "last_duration_ms": metrics.last_duration_ms,
                    "last_success_at": metrics.last_success_at,
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    response["jobs"] = serde_json::Value::Object(jobs);
    let upload_processing_capacity = state.limits.upload.concurrent_processing_tasks;
    let upload_receive_capacity = state.limits.upload.concurrent_receive_tasks;
    let sqlite_index_capacity = 1_usize;
    response["upload_resources"] = serde_json::json!({
        "receive": {
            "active": upload_receive_capacity.saturating_sub(
                state.upload.receive_permits.available_permits()
            ),
            "capacity": upload_receive_capacity,
        },
        "process": {
            "active": upload_processing_capacity.saturating_sub(
                state.upload.processing_permits.available_permits()
            ),
            "capacity": upload_processing_capacity,
        },
        "sqlite_index": {
            "active": sqlite_index_capacity.saturating_sub(
                state.upload.sqlite_index_permits.available_permits()
            ),
            "capacity": sqlite_index_capacity,
        },
        "tantivy_index": {
            "active": state.search.tantivy_budget.active_writers(),
            "queued": state.search.tantivy_budget.queued_writers(),
            "capacity": state.search.tantivy_budget.writer_capacity(),
        },
    });
    response["updated_at"] = serde_json::Value::String(updated_at);
    response["updated_by_username"] = updated_by_username
        .map(serde_json::Value::String)
        .unwrap_or(serde_json::Value::Null);
    Ok(response)
}

fn settings_response(
    snapshot: &std::sync::Arc<crate::settings::SettingsSnapshot>,
) -> serde_json::Value {
    let configured = public_settings_map(&snapshot.configured);
    let effective = public_settings_map(&snapshot.effective);
    let restart_fields = [
        "argon2_concurrency",
        "upload_concurrent_processing_tasks",
        "upload_concurrent_receive_tasks",
        "indexing_max_indexed_line_size",
        "search_tantivy_max_writers",
        "search_tantivy_writer_heap_size",
        "api_concurrent_line_reads",
        "temp_results_concurrent_materializations",
    ];
    let pending_restart_fields: Vec<&str> = restart_fields
        .into_iter()
        .filter(|field| *field != "argon2_concurrency")
        .filter(|field| configured.get(*field) != effective.get(*field))
        .collect();
    let auto_values = settings::metadata::admin()
        .into_iter()
        .filter_map(|field| {
            field.auto_value.map(|value| {
                (
                    serde_json::to_value(field.key)
                        .expect("setting metadata keys serialize")
                        .as_str()
                        .expect("setting metadata keys are strings")
                        .to_owned(),
                    serde_json::json!(value),
                )
            })
        })
        .collect::<serde_json::Map<_, _>>();
    serde_json::json!({
        "schema_version": 2,
        "revision": snapshot.revision.to_string(),
        "allow_registration": snapshot.configured.allow_registration,
        "login_ip_limit_per_minute": snapshot.configured.login_ip_limit_per_minute,
        "login_username_failure_limit_per_5_minutes": snapshot.configured.login_username_failure_limit_per_5_minutes,
        "issue_inactive_days": snapshot.configured.issue_inactive_days,
        "cleanup_exempt_usernames": snapshot.configured.cleanup_exempt_usernames,
        "configured": configured,
        "effective": effective,
        "resource_modes": snapshot.resource_modes,
        "auto_values": auto_values,
        "security": {"argon2id_enabled": true},
        "restart_required": !pending_restart_fields.is_empty(),
        "pending_restart_fields": pending_restart_fields,
        "fields": settings::metadata::admin(),
    })
}

fn public_settings_map(values: &SettingsValues) -> serde_json::Value {
    let mut value = serde_json::to_value(values).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(object) = value.as_object_mut() {
        object.remove("argon2_concurrency");
    }
    value
}

#[get("/admin/users")]
pub async fn list_users(
    _admin: RequireAdmin,
    state: web::Data<AppState>,
    query: web::Query<AdminListQuery>,
) -> Result<HttpResponse, AppError> {
    let limit = limit(query.limit)?;
    let cursor = decode_cursor(query.cursor.as_deref())?;
    let mut sql = QueryBuilder::<Sqlite>::new(
        "SELECT u.id,u.username,u.status,u.created_at,u.updated_at,u.last_login_at,(SELECT COUNT(*) FROM user_sessions s WHERE s.user_id=u.id AND s.revoked_at IS NULL AND datetime(s.expires_at)>CURRENT_TIMESTAMP) active_session_count,(SELECT COUNT(*) FROM issues i WHERE i.owner_user_id=u.id AND i.status='ACTIVE') issue_count,COALESCE((SELECT SUM(b.content_size_bytes) FROM bundles b WHERE b.uploader_user_id=u.id AND b.status IN ('READY','PROCESSING') AND b.deleted_at IS NULL),0) storage_bytes FROM users u WHERE u.role='USER'",
    );
    if let Some(q) = query.query.as_deref() {
        sql.push(" AND u.username_normalized LIKE ")
            .push_bind(format!("%{}%", q.to_ascii_lowercase()));
    }
    if let Some(status) = query.status.as_deref() {
        sql.push(" AND u.status = ")
            .push_bind(parse_status(status)?.to_string());
    }
    if let Some((created, id)) = cursor {
        sql.push(" AND (u.created_at < ")
            .push_bind(created.clone())
            .push(" OR (u.created_at = ")
            .push_bind(created)
            .push(" AND u.id < ")
            .push_bind(id)
            .push("))");
    }
    sql.push(" ORDER BY u.created_at DESC,u.id DESC LIMIT ")
        .push_bind(limit + 1);
    let mut items = sql
        .build_query_as::<AdminUser>()
        .fetch_all(&state.db.pool)
        .await
        .map_err(AppError::Database)?;
    let next_cursor = if items.len() as i64 > limit {
        items.pop();
        items.last().map(|u| encode_cursor(&u.created_at, &u.id))
    } else {
        None
    };
    Ok(HttpResponse::Ok().json(AdminUserPage { items, next_cursor }))
}

async fn mutate_user_status(
    state: &AppState,
    actor: &RequireAdmin,
    target: &str,
    new_status: UserStatus,
    req: &HttpRequest,
) -> Result<(UserStatus, u64), AppError> {
    let input = (
        target.to_owned(),
        actor.0.id.clone(),
        Uuid::new_v4().to_string(),
        new_status,
        req.peer_addr().map(|a| a.ip().to_string()),
        req.headers()
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
    );
    return crate::db::write::transaction(&state.db.pool, "mutate user status", move |conn| {
        let input = input.clone();
        Box::pin(async move {
            let current: Option<(UserRole, UserStatus)> =
                sqlx::query_as("SELECT role,status FROM users WHERE id=?")
                    .bind(&input.0)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(AppError::Database)?;
            let (role, old_status) = current.ok_or_else(|| {
                AppError::api(StatusCode::NOT_FOUND, "ADMIN_USER_NOT_FOUND", "用户不存在")
            })?;
            if role == UserRole::Admin {
                return Err(AppError::api(
                    StatusCode::CONFLICT,
                    "IMMUTABLE_ADMIN_ACCOUNT",
                    "管理员账户不可修改",
                ));
            }
            if old_status == input.3 {
                return Ok((old_status, 0));
            }
            sqlx::query("UPDATE users SET status=?,updated_at=CURRENT_TIMESTAMP WHERE id=?")
                .bind(input.3.to_string())
                .bind(&input.0)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
            let revoked = if input.3 == UserStatus::Disabled {
                sqlx::query("UPDATE user_sessions SET revoked_at=CURRENT_TIMESTAMP WHERE user_id=? AND revoked_at IS NULL")
                    .bind(&input.0)
                    .execute(&mut *conn)
                    .await
                    .map_err(AppError::Database)?
                    .rows_affected()
            } else {
                0
            };
            sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,target_user_id,action,old_value,new_value,client_ip,user_agent) VALUES(?,'USER',?,?,?,?,?,?,?)")
                .bind(&input.2)
                .bind(&input.1)
                .bind(&input.0)
                .bind("USER_STATUS_CHANGED")
                .bind(old_status.to_string())
                .bind(input.3.to_string())
                .bind(input.4.as_deref())
                .bind(input.5.as_deref())
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
            Ok((input.3, revoked))
        })
    })
    .await;
}

#[patch("/admin/users/{user_id}/status")]
pub async fn change_status(
    admin: RequireAdmin,
    state: web::Data<AppState>,
    path: web::Path<String>,
    body: web::Json<ChangeStatus>,
    req: HttpRequest,
) -> Result<HttpResponse, AppError> {
    let (status, _) =
        mutate_user_status(&state, &admin, &path, parse_status(&body.status)?, &req).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({"id":path.into_inner(),"status":status})))
}

#[post("/admin/users/{user_id}/revoke-sessions")]
pub async fn revoke_sessions(
    admin: RequireAdmin,
    state: web::Data<AppState>,
    path: web::Path<String>,
    req: HttpRequest,
) -> Result<HttpResponse, AppError> {
    let target = path.into_inner();
    let role: Option<UserRole> = sqlx::query_scalar("SELECT role FROM users WHERE id=?")
        .bind(&target)
        .fetch_one(&state.db.pool)
        .await
        .map_err(AppError::Database)?;
    let role = role.ok_or_else(|| {
        AppError::api(StatusCode::NOT_FOUND, "ADMIN_USER_NOT_FOUND", "用户不存在")
    })?;
    if role == UserRole::Admin {
        return Err(AppError::api(
            StatusCode::CONFLICT,
            "IMMUTABLE_ADMIN_ACCOUNT",
            "管理员账户不可修改",
        ));
    }
    let input = (
        target,
        admin.0.id.clone(),
        Uuid::new_v4().to_string(),
        req.peer_addr().map(|a| a.ip().to_string()),
        req.headers()
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
    );
    let revoked = crate::db::write::transaction(
        &state.db.pool,
        "revoke admin user sessions",
        move |conn| {
            let input = input.clone();
            Box::pin(async move {
                let revoked = sqlx::query("UPDATE user_sessions SET revoked_at=CURRENT_TIMESTAMP WHERE user_id=? AND revoked_at IS NULL")
                    .bind(&input.0)
                    .execute(&mut *conn)
                    .await
                    .map_err(AppError::Database)?
                    .rows_affected();
                sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,target_user_id,action,new_value,client_ip,user_agent) VALUES(?,'USER',?,?,'USER_SESSIONS_REVOKED',?,?,?)")
                    .bind(&input.2)
                    .bind(&input.1)
                    .bind(&input.0)
                    .bind(revoked.to_string())
                    .bind(input.3.as_deref())
                    .bind(input.4.as_deref())
                    .execute(&mut *conn)
                    .await
                    .map_err(AppError::Database)?;
                Ok(revoked)
            })
        },
    )
    .await?;
    Ok(HttpResponse::Ok().json(RevokedSessions {
        revoked_sessions: revoked,
    }))
}

#[get("/admin/audit-logs")]
pub async fn list_audit(
    _admin: RequireAdmin,
    state: web::Data<AppState>,
    query: web::Query<AuditListQuery>,
) -> Result<HttpResponse, AppError> {
    let limit = limit(query.limit)?;
    let cursor = decode_cursor(query.cursor.as_deref())?;
    let mut sql = QueryBuilder::<Sqlite>::new(
        "SELECT l.id,l.actor_type,l.actor_user_id,l.target_user_id,u.username AS target_username,l.action,l.old_value,l.new_value,l.details_json,l.client_ip,l.user_agent,l.created_at FROM admin_audit_logs l LEFT JOIN users u ON u.id=l.target_user_id WHERE 1=1",
    );
    if let Some(v) = query.action.as_deref() {
        sql.push(" AND l.action=").push_bind(v);
    }
    if let Some(v) = query.target_user_id.as_deref() {
        sql.push(" AND l.target_user_id=").push_bind(v);
    }
    if let Some((created, id)) = cursor {
        sql.push(" AND (l.created_at<")
            .push_bind(created.clone())
            .push(" OR (l.created_at=")
            .push_bind(created)
            .push(" AND l.id<")
            .push_bind(id)
            .push("))");
    }
    sql.push(" ORDER BY l.created_at DESC,l.id DESC LIMIT ")
        .push_bind(limit + 1);
    let mut items = sql
        .build_query_as::<AuditLog>()
        .fetch_all(&state.db.pool)
        .await
        .map_err(AppError::Database)?;
    for item in &mut items {
        if matches!(
            item.action.as_str(),
            "SETTINGS_UPDATED" | "SYSTEM_SETTINGS_INITIALIZED"
        ) {
            item.old_value = redact_settings_audit_json(item.old_value.take());
            item.new_value = redact_settings_audit_json(item.new_value.take());
            item.details_json = redact_settings_audit_json(item.details_json.take());
        }
    }
    let next_cursor = if items.len() as i64 > limit {
        items.pop();
        items.last().map(|v| encode_cursor(&v.created_at, &v.id))
    } else {
        None
    };
    Ok(HttpResponse::Ok().json(AuditLogPage { items, next_cursor }))
}

fn redact_settings_audit_json(raw: Option<String>) -> Option<String> {
    let mut value = serde_json::from_str::<serde_json::Value>(raw.as_deref()?).ok()?;
    redact_audit_json_value(&mut value);
    Some(value.to_string())
}

fn redact_audit_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            let sensitive_change = object
                .get("field")
                .and_then(serde_json::Value::as_str)
                .is_some_and(crate::settings::is_sensitive_audit_field);
            if sensitive_change {
                object.remove("old_value");
                object.remove("new_value");
                object.insert("redacted".into(), serde_json::Value::Bool(true));
            }
            let keys: Vec<_> = object.keys().cloned().collect();
            for key in keys {
                if crate::settings::is_sensitive_audit_field(&key) {
                    object.insert(key, serde_json::json!({ "redacted": true }));
                } else if let Some(child) = object.get_mut(&key) {
                    redact_audit_json_value(child);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_audit_json_value(item);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod audit_redaction_tests {
    use super::redact_settings_audit_json;

    #[test]
    fn response_redaction_removes_sensitive_snapshot_and_change_values() {
        let old_value = redact_settings_audit_json(Some(
            r#"{"issue_inactive_days":7,"provider_api_key":"old-secret"}"#.into(),
        ))
        .expect("JSON snapshot");
        let old_value: serde_json::Value = serde_json::from_str(&old_value).unwrap();
        assert_eq!(old_value["issue_inactive_days"], 7);
        assert_eq!(old_value["provider_api_key"]["redacted"], true);
        assert!(!old_value.to_string().contains("old-secret"));

        let details = redact_settings_audit_json(Some(
            r#"{"changes":[{"field":"provider_api_key","old_value":"old-secret","new_value":"new-secret","apply_mode":"hot"}]}"#.into(),
        ))
        .expect("JSON details");
        assert!(!details.contains("old-secret"));
        assert!(!details.contains("new-secret"));
        let details: serde_json::Value = serde_json::from_str(&details).unwrap();
        assert_eq!(details["changes"][0]["redacted"], true);
        assert!(details["changes"][0].get("old_value").is_none());
        assert!(details["changes"][0].get("new_value").is_none());
    }
}
