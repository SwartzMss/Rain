use actix_web::{
    HttpRequest, HttpResponse,
    cookie::{Cookie, SameSite, time::Duration as CookieDuration},
    get,
    http::StatusCode,
    post, web,
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState,
    auth::{
        extractor::OptionalUser,
        session::{generate_session_token, hash_session_token},
    },
    error::AppError,
};

use crate::routes::issues::normalize_issue_code;
const GUEST_COOKIE: &str = "rain_workspace_guest";
const ACTIVITY_WRITE_INTERVAL: Duration = Duration::minutes(4);
// Give the throttled activity writer one full interval to publish the latest user input.
const SESSION_TTL: Duration = Duration::seconds(3 * 60 * 60 + 4 * 60);
const MAX_RESULT_REFS_PER_REQUEST: usize = 200;

#[derive(Serialize)]
struct SessionResponse {
    session_id: String,
    issue_code: String,
    server_now: String,
    last_activity_at: String,
    expires_at: String,
}

#[derive(Deserialize)]
struct ResultRefsRequest {
    #[serde(default)]
    add: Vec<String>,
}

#[derive(Serialize, FromRow)]
pub(crate) struct WorkspaceSessionRecord {
    pub(crate) id: String,
    pub(crate) issue_code: String,
    pub(crate) subject_key: String,
    pub(crate) state: String,
    pub(crate) last_activity_at: String,
    pub(crate) expires_at: String,
}

pub(crate) fn request_subject(
    user: &OptionalUser,
    request: &HttpRequest,
) -> (String, Option<Cookie<'static>>) {
    if let Some(user) = user.0.as_ref() {
        return (format!("user:{}", user.id), None);
    }
    if let Some(cookie) = request
        .cookie(GUEST_COOKIE)
        .filter(|cookie| !cookie.value().is_empty())
    {
        return (
            format!("guest:{}", hash_session_token(cookie.value())),
            None,
        );
    }
    let token = generate_session_token();
    let mut cookie = Cookie::build(GUEST_COOKIE, token.clone())
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(CookieDuration::days(30));
    if request.connection_info().scheme() == "https" {
        cookie = cookie.secure(true);
    }
    (
        format!("guest:{}", hash_session_token(&token)),
        Some(cookie.finish()),
    )
}

pub(crate) async fn validate_session_for_result(
    state: &web::Data<AppState>,
    session_id: &str,
    subject_key: &str,
    issue_code: &str,
) -> Result<(), AppError> {
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM issue_workspace_sessions WHERE id = ? AND subject_key = ? AND issue_code = ? AND state = 'ACTIVE' AND datetime(expires_at) > datetime('now'))",
    )
    .bind(session_id)
    .bind(subject_key)
    .bind(issue_code)
    .fetch_one(&state.db.pool)
    .await
    .map_err(AppError::Database)?;
    if valid {
        Ok(())
    } else {
        Err(AppError::api(
            StatusCode::CONFLICT,
            "WORKSPACE_SESSION_EXPIRED",
            "Issue 工作会话已结束，请重新搜索",
        ))
    }
}

pub(crate) async fn associate_result_with_workspace(
    state: &web::Data<AppState>,
    session_id: &str,
    subject_key: &str,
    issue_code: &str,
    result_id: &str,
) -> Result<(), AppError> {
    let input = (
        session_id.to_owned(),
        subject_key.to_owned(),
        issue_code.to_owned(),
        result_id.to_owned(),
    );
    crate::db::write::run(
        &state.db.pool,
        "associate temporary result with Issue workspace",
        &input,
        |conn, (session_id, subject_key, issue_code, result_id)| Box::pin(async move {
            let valid_session: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM issue_workspace_sessions WHERE id = ? AND subject_key = ? AND issue_code = ? AND state = 'ACTIVE' AND datetime(expires_at) > datetime('now'))",
            )
            .bind(session_id)
            .bind(subject_key)
            .bind(issue_code)
            .fetch_one(&mut *conn)
            .await
            .map_err(AppError::Database)?;
            if !valid_session {
                return Err(AppError::api(StatusCode::CONFLICT, "WORKSPACE_SESSION_EXPIRED", "Issue 工作会话已结束，请重新搜索"));
            }
            let updated = sqlx::query(
                "UPDATE temp_results SET issue_code = ? WHERE id = ? AND status = 'ACTIVE' AND datetime(expires_at) > datetime('now')",
            )
            .bind(issue_code)
            .bind(result_id)
            .execute(&mut *conn)
            .await
            .map_err(AppError::Database)?
            .rows_affected();
            if updated != 1 {
                return Err(AppError::NotFound(format!("temporary result {result_id}")));
            }
            sqlx::query(
                "INSERT INTO temp_result_workspace_refs (session_id, result_id, created_at) VALUES (?, ?, CURRENT_TIMESTAMP) ON CONFLICT(session_id, result_id) DO NOTHING",
            )
            .bind(session_id)
            .bind(result_id)
            .execute(conn)
            .await
            .map_err(AppError::Database)?;
            Ok(())
        }),
    )
    .await
}

fn require_id(value: &str) -> Result<(), AppError> {
    if value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::NotFound(format!("workspace session {value}")))
    }
}

async fn load_owned_session(
    state: &web::Data<AppState>,
    id: &str,
    subject_key: &str,
) -> Result<WorkspaceSessionRecord, AppError> {
    require_id(id)?;
    let session = sqlx::query_as::<_, WorkspaceSessionRecord>(
        "SELECT id, issue_code, subject_key, state, last_activity_at, expires_at FROM issue_workspace_sessions WHERE id = ? AND subject_key = ? AND state = 'ACTIVE' AND datetime(expires_at) > datetime('now') LIMIT 1",
    )
    .bind(id)
    .bind(subject_key)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(AppError::Database)?;
    session.ok_or_else(|| {
        AppError::api(
            StatusCode::CONFLICT,
            "WORKSPACE_SESSION_EXPIRED",
            "Issue 工作会话已结束，请重新搜索",
        )
    })
}

#[post("/issues/{issue_code}/workspace-sessions")]
pub(crate) async fn create_session(
    user: OptionalUser,
    request: HttpRequest,
    issue_code: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let issue_code = normalize_issue_code(&issue_code)?;
    let issue_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM issues WHERE code = ? AND status = 'ACTIVE')",
    )
    .bind(&issue_code)
    .fetch_one(&state.db.pool)
    .await
    .map_err(AppError::Database)?;
    if !issue_exists {
        return Err(AppError::NotFound(format!("issue {issue_code}")));
    }
    let (subject_key, new_cookie) = request_subject(&user, &request);
    let id = Uuid::new_v4().simple().to_string();
    let now = Utc::now();
    let expires_at = now + SESSION_TTL;
    crate::db::write::run(
        &state.db.pool,
        "create Issue workspace session",
        &(id.as_str(), issue_code.as_str(), subject_key.as_str(), now.to_rfc3339(), expires_at.to_rfc3339()),
        |conn, (id, issue_code, subject_key, created_at, expires_at)| Box::pin(async move {
            let active_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issue_workspace_sessions WHERE subject_key = ? AND state = 'ACTIVE' AND datetime(expires_at) > datetime('now')")
                .bind(subject_key).fetch_one(&mut *conn).await.map_err(AppError::Database)?;
            if active_count >= 16 {
                return Err(AppError::api(StatusCode::TOO_MANY_REQUESTS, "WORKSPACE_SESSION_LIMIT", "工作会话数量已达到上限，请关闭其他 Issue 页面"));
            }
            sqlx::query("INSERT INTO issue_workspace_sessions (id, issue_code, subject_key, created_at, last_activity_at, expires_at) VALUES (?, ?, ?, ?, ?, ?)")
                .bind(id).bind(issue_code).bind(subject_key).bind(created_at).bind(created_at).bind(expires_at)
                .execute(conn).await.map_err(AppError::Database)?;
            Ok(())
        }),
    ).await?;

    let response = SessionResponse {
        session_id: id,
        issue_code,
        server_now: now.to_rfc3339(),
        last_activity_at: now.to_rfc3339(),
        expires_at: expires_at.to_rfc3339(),
    };
    let mut builder = HttpResponse::Created();
    if let Some(cookie) = new_cookie {
        builder.cookie(cookie);
    }
    Ok(builder.json(response))
}

#[get("/workspace-sessions/{id}")]
pub(crate) async fn get_session(
    user: OptionalUser,
    request: HttpRequest,
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let (subject_key, _) = request_subject(&user, &request);
    let session = load_owned_session(&state, &id, &subject_key).await?;
    Ok(HttpResponse::Ok().json(SessionResponse {
        session_id: session.id,
        issue_code: session.issue_code,
        server_now: Utc::now().to_rfc3339(),
        last_activity_at: session.last_activity_at,
        expires_at: session.expires_at,
    }))
}

#[post("/workspace-sessions/{id}/activity")]
pub(crate) async fn record_activity(
    user: OptionalUser,
    request: HttpRequest,
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let (subject_key, _) = request_subject(&user, &request);
    let session = load_owned_session(&state, &id, &subject_key).await?;
    let now = Utc::now();
    let next_expiry = now + SESSION_TTL;
    let write_after = now - ACTIVITY_WRITE_INTERVAL;
    let input = (
        session.id.clone(),
        subject_key.clone(),
        now.to_rfc3339(),
        next_expiry.to_rfc3339(),
        write_after.to_rfc3339(),
    );
    let updated = crate::db::write::run(
        &state.db.pool,
        "renew Issue workspace session",
        &input,
        |conn, (id, subject_key, last_activity_at, expires_at, write_after)| Box::pin(async move {
            sqlx::query("UPDATE issue_workspace_sessions SET last_activity_at = ?, expires_at = ? WHERE id = ? AND subject_key = ? AND state = 'ACTIVE' AND datetime(expires_at) > datetime('now') AND datetime(last_activity_at) <= datetime(?)")
                .bind(last_activity_at).bind(expires_at).bind(id).bind(subject_key).bind(write_after)
                .execute(conn).await.map(|result| result.rows_affected() > 0).map_err(AppError::Database)
        }),
    ).await?;
    let current = load_owned_session(&state, &session.id, &subject_key).await?;
    let _ = updated;
    Ok(HttpResponse::Ok().json(SessionResponse {
        session_id: current.id,
        issue_code: current.issue_code,
        server_now: now.to_rfc3339(),
        last_activity_at: current.last_activity_at,
        expires_at: current.expires_at,
    }))
}

#[post("/workspace-sessions/{id}/end")]
pub(crate) async fn end_session(
    user: OptionalUser,
    request: HttpRequest,
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let (subject_key, _) = request_subject(&user, &request);
    require_id(&id)?;
    crate::db::write::run(
        &state.db.pool,
        "end Issue workspace session",
        &(id.as_str(), subject_key.as_str()),
        |conn, (id, subject_key)| Box::pin(async move {
            sqlx::query("UPDATE issue_workspace_sessions SET state = 'ENDED', ended_at = CURRENT_TIMESTAMP WHERE id = ? AND subject_key = ? AND state = 'ACTIVE'")
                .bind(id).bind(subject_key).execute(conn).await.map(|_| ()).map_err(AppError::Database)
        }),
    ).await?;
    Ok(HttpResponse::NoContent().finish())
}

#[post("/workspace-sessions/{id}/result-refs")]
pub(crate) async fn update_result_refs(
    user: OptionalUser,
    request: HttpRequest,
    id: web::Path<String>,
    payload: web::Json<ResultRefsRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let (subject_key, _) = request_subject(&user, &request);
    let session = load_owned_session(&state, &id, &subject_key).await?;
    let payload = payload.into_inner();
    if payload.add.len() > MAX_RESULT_REFS_PER_REQUEST
        || payload
            .add
            .iter()
            .any(|id| id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(AppError::BadRequest(
            "invalid temporary result reference list".into(),
        ));
    }
    crate::db::write::run(
        &state.db.pool,
        "update Issue workspace result refs",
        &(session.id.as_str(), session.subject_key.as_str(), session.issue_code.as_str(), payload.add.clone()),
        |conn, (session_id, subject_key, issue_code, add_ids)| Box::pin(async move {
            let session_valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM issue_workspace_sessions WHERE id = ? AND subject_key = ? AND issue_code = ? AND state = 'ACTIVE' AND datetime(expires_at) > datetime('now'))")
                .bind(session_id).bind(subject_key).bind(issue_code).fetch_one(&mut *conn).await.map_err(AppError::Database)?;
            if !session_valid {
                return Err(AppError::api(StatusCode::CONFLICT, "WORKSPACE_SESSION_EXPIRED", "Issue 工作会话已结束，请重新搜索"));
            }
            for id in add_ids {
                sqlx::query("INSERT INTO temp_result_workspace_refs (session_id, result_id, created_at) SELECT ?, r.id, CURRENT_TIMESTAMP FROM temp_results r WHERE r.id = ? AND r.status = 'ACTIVE' AND r.issue_code = ? AND (datetime(r.expires_at) >= datetime('now') OR EXISTS (SELECT 1 FROM temp_result_workspace_refs old_ref JOIN issue_workspace_sessions old_session ON old_session.id = old_ref.session_id WHERE old_ref.result_id = r.id AND old_session.state = 'ACTIVE' AND datetime(old_session.expires_at) > datetime('now'))) ON CONFLICT(session_id, result_id) DO NOTHING")
                    .bind(session_id).bind(id).bind(issue_code).execute(&mut *conn).await.map_err(AppError::Database)?;
                let ref_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM temp_result_workspace_refs WHERE session_id = ? AND result_id = ?)")
                    .bind(session_id).bind(id).fetch_one(&mut *conn).await.map_err(AppError::Database)?;
                if !ref_exists {
                    return Err(AppError::NotFound(format!("temporary result {id}")));
                }
            }
            Ok(())
        }),
    ).await?;
    Ok(HttpResponse::NoContent().finish())
}

pub(crate) async fn purge_inactive_sessions(state: &web::Data<AppState>) -> Result<(), AppError> {
    crate::db::write::run(
        &state.db.pool,
        "purge inactive Issue workspace sessions",
        &(),
        |conn, _| Box::pin(async move {
            sqlx::query("DELETE FROM issue_workspace_sessions WHERE state = 'ENDED' OR datetime(expires_at) <= datetime('now', '-1 day')")
                .execute(conn).await.map(|_| ()).map_err(AppError::Database)
        }),
    ).await
}
