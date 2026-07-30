use actix_web::{HttpRequest, HttpResponse, get, http::StatusCode, patch, post, web};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sqlx::{QueryBuilder, Sqlite};
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
fn parse_role(value: &str) -> Result<UserRole, AppError> {
    value
        .parse()
        .map_err(|_| AppError::api(StatusCode::BAD_REQUEST, "INVALID_USER_ROLE", "用户角色无效"))
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

#[get("/admin/users")]
pub async fn list_users(
    _admin: RequireAdmin,
    state: web::Data<AppState>,
    query: web::Query<AdminListQuery>,
) -> Result<HttpResponse, AppError> {
    let limit = limit(query.limit)?;
    let cursor = decode_cursor(query.cursor.as_deref())?;
    let mut sql = QueryBuilder::<Sqlite>::new(
        "SELECT u.id,u.username,u.role,u.status,u.created_at,u.updated_at,u.last_login_at,(SELECT COUNT(*) FROM user_sessions s WHERE s.user_id=u.id AND s.revoked_at IS NULL AND datetime(s.expires_at)>CURRENT_TIMESTAMP) active_session_count FROM users u WHERE 1=1",
    );
    if let Some(q) = query.query.as_deref() {
        sql.push(" AND u.username_normalized LIKE ")
            .push_bind(format!("%{}%", q.to_ascii_lowercase()));
    }
    if let Some(role) = query.role.as_deref() {
        sql.push(" AND u.role = ")
            .push_bind(parse_role(role)?.to_string());
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
        .fetch_all(&state.pool)
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

async fn mutate_user(
    state: &AppState,
    actor: &RequireAdmin,
    target: &str,
    role: Option<UserRole>,
    status: Option<UserStatus>,
    req: &HttpRequest,
) -> Result<(UserRole, UserStatus, u64), AppError> {
    let mut conn = state.pool.acquire().await.map_err(AppError::Database)?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *conn)
        .await
        .map_err(AppError::Database)?;
    let result=async {
        let current: Option<(UserRole,UserStatus)>=sqlx::query_as("SELECT role,status FROM users WHERE id=?").bind(target).fetch_optional(&mut *conn).await.map_err(AppError::Database)?;
        let (old_role,old_status)=current.ok_or_else(|| AppError::api(StatusCode::NOT_FOUND,"ADMIN_USER_NOT_FOUND","用户不存在"))?;
        if role == Some(old_role) || status == Some(old_status) { return Ok((old_role, old_status, 0)); }
        if role.is_some() && actor.0.id==target { return Err(AppError::api(StatusCode::CONFLICT,"CANNOT_CHANGE_OWN_ROLE","不能修改自己的角色")); }
        if status==Some(UserStatus::Disabled) && actor.0.id==target { return Err(AppError::api(StatusCode::CONFLICT,"CANNOT_DISABLE_SELF","不能停用自己")); }
        let new_role=role.unwrap_or(old_role); let new_status=status.unwrap_or(old_status);
        if old_role==UserRole::Admin && old_status==UserStatus::Active && (new_role!=UserRole::Admin || new_status!=UserStatus::Active) {
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role='ADMIN' AND status='ACTIVE'").fetch_one(&mut *conn).await.map_err(AppError::Database)?;
            if count<=1 { return Err(AppError::api(StatusCode::CONFLICT,"LAST_ACTIVE_ADMIN","必须保留至少一个有效管理员")); }
        }
        sqlx::query("UPDATE users SET role=?,status=?,updated_at=CURRENT_TIMESTAMP WHERE id=?").bind(new_role.to_string()).bind(new_status.to_string()).bind(target).execute(&mut *conn).await.map_err(AppError::Database)?;
        let revoked=if old_status!=new_status && new_status==UserStatus::Disabled { sqlx::query("UPDATE user_sessions SET revoked_at=CURRENT_TIMESTAMP WHERE user_id=? AND revoked_at IS NULL").bind(target).execute(&mut *conn).await.map_err(AppError::Database)?.rows_affected() } else { 0 };
        let (action,old,new)=if role.is_some() { ("USER_ROLE_CHANGED",old_role.to_string(),new_role.to_string()) } else { ("USER_STATUS_CHANGED",old_status.to_string(),new_status.to_string()) };
        sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,target_user_id,action,old_value,new_value,client_ip,user_agent) VALUES(?,'USER',?,?,?,?,?,?,?)")
            .bind(Uuid::new_v4().to_string()).bind(&actor.0.id).bind(target).bind(action).bind(old).bind(new).bind(req.peer_addr().map(|a|a.ip().to_string())).bind(req.headers().get("user-agent").and_then(|v|v.to_str().ok())).execute(&mut *conn).await.map_err(AppError::Database)?;
        Ok((new_role,new_status,revoked))
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

#[patch("/admin/users/{user_id}/role")]
pub async fn change_role(
    admin: RequireAdmin,
    state: web::Data<AppState>,
    path: web::Path<String>,
    body: web::Json<ChangeRole>,
    req: HttpRequest,
) -> Result<HttpResponse, AppError> {
    let (role, status, _) = mutate_user(
        &state,
        &admin,
        &path,
        Some(parse_role(&body.role)?),
        None,
        &req,
    )
    .await?;
    Ok(HttpResponse::Ok()
        .json(serde_json::json!({"id":path.into_inner(),"role":role,"status":status})))
}
#[patch("/admin/users/{user_id}/status")]
pub async fn change_status(
    admin: RequireAdmin,
    state: web::Data<AppState>,
    path: web::Path<String>,
    body: web::Json<ChangeStatus>,
    req: HttpRequest,
) -> Result<HttpResponse, AppError> {
    let (role, status, _) = mutate_user(
        &state,
        &admin,
        &path,
        None,
        Some(parse_status(&body.status)?),
        &req,
    )
    .await?;
    Ok(HttpResponse::Ok()
        .json(serde_json::json!({"id":path.into_inner(),"role":role,"status":status})))
}

#[post("/admin/users/{user_id}/revoke-sessions")]
pub async fn revoke_sessions(
    admin: RequireAdmin,
    state: web::Data<AppState>,
    path: web::Path<String>,
    req: HttpRequest,
) -> Result<HttpResponse, AppError> {
    let target = path.into_inner();
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id=?")
        .bind(&target)
        .fetch_one(&state.pool)
        .await
        .map_err(AppError::Database)?;
    if exists == 0 {
        return Err(AppError::api(
            StatusCode::NOT_FOUND,
            "ADMIN_USER_NOT_FOUND",
            "用户不存在",
        ));
    }
    let mut tx = state.pool.begin().await.map_err(AppError::Database)?;
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
        "SELECT id,actor_type,actor_user_id,target_user_id,action,old_value,new_value,client_ip,user_agent,created_at FROM admin_audit_logs WHERE 1=1",
    );
    if let Some(v) = query.action.as_deref() {
        sql.push(" AND action=").push_bind(v);
    }
    if let Some(v) = query.target_user_id.as_deref() {
        sql.push(" AND target_user_id=").push_bind(v);
    }
    if let Some((created, id)) = cursor {
        sql.push(" AND (created_at<")
            .push_bind(created.clone())
            .push(" OR (created_at=")
            .push_bind(created)
            .push(" AND id<")
            .push_bind(id)
            .push("))");
    }
    sql.push(" ORDER BY created_at DESC,id DESC LIMIT ")
        .push_bind(limit + 1);
    let mut items = sql
        .build_query_as::<AuditLog>()
        .fetch_all(&state.pool)
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
