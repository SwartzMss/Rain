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
    sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,action,old_value,client_ip,user_agent) VALUES(?,'USER',?,?,?, ?, ?)")
        .bind(Uuid::new_v4().to_string()).bind(&admin.0.id).bind(action).bind(format!("{key}:count={count}"))
        .bind(req.peer_addr().map(|a| a.ip().to_string())).bind(req.headers().get("user-agent").and_then(|v| v.to_str().ok())).execute(&state.db.pool).await.map_err(AppError::Database)?;
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
    let mut tx = state.db.pool.begin().await.map_err(AppError::Database)?;
    sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,action,old_value,client_ip,user_agent) VALUES(?,'USER',?,?,?, ?, ?)")
        .bind(Uuid::new_v4().to_string()).bind(&admin.0.id).bind(action).bind(format!("keys={key_count};count={event_count}"))
        .bind(req.peer_addr().map(|a| a.ip().to_string())).bind(req.headers().get("user-agent").and_then(|v| v.to_str().ok())).execute(&mut *tx).await.map_err(AppError::Database)?;
    tx.commit().await.map_err(AppError::Database)?;
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
    crate::db::load_or_initialize_registration_setting(
        &state.db.pool,
        state.auth_runtime.registration_allowed(),
    )
    .await?;
    let settings = sqlx::query_as::<_, RegistrationSettings>(
        "SELECT s.allow_registration, s.updated_at, u.username AS updated_by_username, s.login_ip_limit_per_minute, s.login_username_failure_limit_per_5_minutes, s.issue_inactive_days FROM system_settings s LEFT JOIN users u ON u.id=s.updated_by_user_id WHERE s.id=1",
    ).fetch_one(&state.db.pool).await.map_err(AppError::Database)?;
    Ok(HttpResponse::Ok().json(serde_json::json!({
        "allow_registration": settings.allow_registration != 0,
        "updated_at": settings.updated_at,
        "updated_by_username": settings.updated_by_username,
        "login_ip_limit_per_minute": settings.login_ip_limit_per_minute,
        "login_username_failure_limit_per_5_minutes": settings.login_username_failure_limit_per_5_minutes,
        "issue_inactive_days": settings.issue_inactive_days,
    })))
}

#[patch("/admin/settings")]
pub async fn update_settings(
    req: HttpRequest,
    admin: RequireAdmin,
    state: web::Data<AppState>,
    body: web::Json<UpdateRegistrationSettings>,
) -> Result<HttpResponse, AppError> {
    let _settings_guard = state.auth_runtime.registration_settings_lock.lock().await;
    let old: (i64, i64, i64, i64) = sqlx::query_as("SELECT allow_registration, login_ip_limit_per_minute, login_username_failure_limit_per_5_minutes, issue_inactive_days FROM system_settings WHERE id=1")
        .fetch_one(&state.db.pool).await.map_err(AppError::Database)?;
    sqlx::query("INSERT OR IGNORE INTO system_settings(id, allow_registration) VALUES(1, ?)")
        .bind(state.auth_runtime.registration_allowed() as i64)
        .execute(&state.db.pool)
        .await
        .map_err(AppError::Database)?;
    let ip_limit = body.login_ip_limit_per_minute.unwrap_or_else(|| {
        state
            .auth_runtime
            .login_ip_limit_per_minute
            .load(std::sync::atomic::Ordering::Acquire)
    });
    let username_limit = body
        .login_username_failure_limit_per_5_minutes
        .unwrap_or_else(|| {
            state
                .auth_runtime
                .login_username_failure_limit_per_5_minutes
                .load(std::sync::atomic::Ordering::Acquire)
        });
    if !(1..=1000).contains(&ip_limit) || !(1..=100).contains(&username_limit) {
        return Err(AppError::api(
            StatusCode::BAD_REQUEST,
            "INVALID_RATE_LIMIT",
            "IP 限流阈值必须为 1 到 1000，用户名限流阈值必须为 1 到 100",
        ));
    }
    let issue_inactive_days = match body.issue_inactive_days.as_ref() {
        None => old.3 as usize,
        Some(value) => value
            .as_i64()
            .filter(|days| (0..=30).contains(days))
            .map(|days| days as usize)
            .ok_or_else(|| {
                AppError::api(
                    StatusCode::BAD_REQUEST,
                    "INVALID_ISSUE_INACTIVE_DAYS",
                    "Issue 非活跃天数必须为 0 到 30 的整数",
                )
            })?,
    };
    let mut settings_tx = state.db.pool.begin().await.map_err(AppError::Database)?;
    let allow_registration = body.allow_registration.unwrap_or(old.0 != 0);
    sqlx::query("UPDATE system_settings SET allow_registration=?, login_ip_limit_per_minute=?, login_username_failure_limit_per_5_minutes=?, issue_inactive_days=?, updated_by_user_id=?, updated_at=CURRENT_TIMESTAMP WHERE id=1")
        .bind(allow_registration as i64).bind(ip_limit as i64).bind(username_limit as i64).bind(issue_inactive_days as i64).bind(&admin.0.id).execute(&mut *settings_tx).await.map_err(AppError::Database)?;
    let mut auth_changes = Vec::new();
    if old.0 != allow_registration as i64 {
        auth_changes.push(format!(
            "registration:{}->{}",
            old.0 != 0,
            allow_registration
        ));
    }
    if old.1 != ip_limit as i64 {
        auth_changes.push(format!("ip_limit:{}->{ip_limit}", old.1));
    }
    if old.2 != username_limit as i64 {
        auth_changes.push(format!("username_limit:{}->{username_limit}", old.2));
    }
    let issue_changed = old.3 != issue_inactive_days as i64;
    let client_ip = req.peer_addr().map(|address| address.ip().to_string());
    let user_agent = req
        .headers()
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if !auth_changes.is_empty() {
        sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,action,old_value,new_value,client_ip,user_agent) VALUES(?,'USER',?,'AUTH_SETTINGS_UPDATED',?,?,?,?)")
            .bind(Uuid::new_v4().to_string())
            .bind(&admin.0.id)
            .bind(format!("registration={};ip_limit={};username_limit={}", old.0 != 0, old.1, old.2))
            .bind(auth_changes.join(";"))
            .bind(client_ip.as_deref())
            .bind(user_agent.as_deref())
            .execute(&mut *settings_tx)
            .await
            .map_err(AppError::Database)?;
    }
    if issue_changed {
        sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,action,old_value,new_value,client_ip,user_agent) VALUES(?,'USER',?,'ISSUE_INACTIVE_SETTINGS_UPDATED',?,?,?,?)")
            .bind(Uuid::new_v4().to_string())
            .bind(&admin.0.id)
            .bind(format!("issue_inactive_days={}", old.3))
            .bind(format!("issue_inactive_days={issue_inactive_days}"))
            .bind(client_ip.as_deref())
            .bind(user_agent.as_deref())
            .execute(&mut *settings_tx)
            .await
            .map_err(AppError::Database)?;
    }
    settings_tx.commit().await.map_err(AppError::Database)?;
    state
        .auth_runtime
        .set_registration_allowed(allow_registration);
    state
        .auth_runtime
        .login_ip_limit_per_minute
        .store(ip_limit, std::sync::atomic::Ordering::Release);
    state
        .auth_runtime
        .login_username_failure_limit_per_5_minutes
        .store(username_limit, std::sync::atomic::Ordering::Release);
    state
        .issue_inactive_days
        .store(issue_inactive_days, std::sync::atomic::Ordering::Release);
    let settings = sqlx::query_as::<_, RegistrationSettings>(
        "SELECT s.allow_registration, s.updated_at, u.username AS updated_by_username, s.login_ip_limit_per_minute, s.login_username_failure_limit_per_5_minutes, s.issue_inactive_days FROM system_settings s LEFT JOIN users u ON u.id=s.updated_by_user_id WHERE s.id=1",
    ).fetch_one(&state.db.pool).await.map_err(AppError::Database)?;
    Ok(HttpResponse::Ok().json(serde_json::json!({
        "allow_registration": settings.allow_registration != 0,
        "updated_at": settings.updated_at,
        "updated_by_username": settings.updated_by_username,
        "login_ip_limit_per_minute": settings.login_ip_limit_per_minute,
        "login_username_failure_limit_per_5_minutes": settings.login_username_failure_limit_per_5_minutes,
        "issue_inactive_days": settings.issue_inactive_days,
    })))
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
    let mut conn = state.db.pool.acquire().await.map_err(AppError::Database)?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *conn)
        .await
        .map_err(AppError::Database)?;
    let result=async {
        let current: Option<(UserRole,UserStatus)>=sqlx::query_as("SELECT role,status FROM users WHERE id=?").bind(target).fetch_optional(&mut *conn).await.map_err(AppError::Database)?;
        let (role,old_status)=current.ok_or_else(|| AppError::api(StatusCode::NOT_FOUND,"ADMIN_USER_NOT_FOUND","用户不存在"))?;
        if role == UserRole::Admin { return Err(AppError::api(StatusCode::CONFLICT,"IMMUTABLE_ADMIN_ACCOUNT","管理员账户不可修改")); }
        if old_status == new_status { return Ok((old_status, 0)); }
        sqlx::query("UPDATE users SET status=?,updated_at=CURRENT_TIMESTAMP WHERE id=?").bind(new_status.to_string()).bind(target).execute(&mut *conn).await.map_err(AppError::Database)?;
        let revoked=if old_status!=new_status && new_status==UserStatus::Disabled { sqlx::query("UPDATE user_sessions SET revoked_at=CURRENT_TIMESTAMP WHERE user_id=? AND revoked_at IS NULL").bind(target).execute(&mut *conn).await.map_err(AppError::Database)?.rows_affected() } else { 0 };
        sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,target_user_id,action,old_value,new_value,client_ip,user_agent) VALUES(?,'USER',?,?,?,?,?,?,?)")
            .bind(Uuid::new_v4().to_string()).bind(&actor.0.id).bind(target).bind("USER_STATUS_CHANGED").bind(old_status.to_string()).bind(new_status.to_string()).bind(req.peer_addr().map(|a|a.ip().to_string())).bind(req.headers().get("user-agent").and_then(|v|v.to_str().ok())).execute(&mut *conn).await.map_err(AppError::Database)?;
        Ok((new_status,revoked))
    }.await;
    match result {
        Ok(v) => {
            sqlx::query("COMMIT")
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
            Ok(v)
        }
        Err(e) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            Err(e)
        }
    }
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
    let mut tx = state.db.pool.begin().await.map_err(AppError::Database)?;
    let revoked=sqlx::query("UPDATE user_sessions SET revoked_at=CURRENT_TIMESTAMP WHERE user_id=? AND revoked_at IS NULL").bind(&target).execute(&mut *tx).await.map_err(AppError::Database)?.rows_affected();
    sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,target_user_id,action,new_value,client_ip,user_agent) VALUES(?,'USER',?,?,'USER_SESSIONS_REVOKED',?,?,?)").bind(Uuid::new_v4().to_string()).bind(&admin.0.id).bind(&target).bind(revoked.to_string()).bind(req.peer_addr().map(|a|a.ip().to_string())).bind(req.headers().get("user-agent").and_then(|v|v.to_str().ok())).execute(&mut *tx).await.map_err(AppError::Database)?;
    tx.commit().await.map_err(AppError::Database)?;
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
        "SELECT l.id,l.actor_type,l.actor_user_id,l.target_user_id,u.username AS target_username,l.action,l.old_value,l.new_value,l.client_ip,l.user_agent,l.created_at FROM admin_audit_logs l LEFT JOIN users u ON u.id=l.target_user_id WHERE 1=1",
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
    let next_cursor = if items.len() as i64 > limit {
        items.pop();
        items.last().map(|v| encode_cursor(&v.created_at, &v.id))
    } else {
        None
    };
    Ok(HttpResponse::Ok().json(AuditLogPage { items, next_cursor }))
}
