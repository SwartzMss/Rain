use actix_web::{HttpResponse, delete, get, http::header::CACHE_CONTROL, post, web};
use serde::Deserialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState,
    auth::extractor::{OptionalUser, RequireBusinessUser},
    db::{finish_bundle_deletion_with_inactive_lease, renew_inactive_issue_lease},
    error::AppError,
    models::issues::{
        IssueBundlesResponse, IssueInactivityExpiry, IssueSummary, UploadStage, UploadStatus,
        UploadStatusWrapper,
    },
};

const ISSUE_CODE_MAX_LEN: usize = 64;
const ISSUE_NAME_MAX_LEN: usize = 128;
const INACTIVE_CLEANUP_LEASE_SECONDS: u64 = 10 * 60;
const MANUAL_CLEANUP_LEASE_SECONDS: u64 = 10 * 60;

pub(crate) async fn touch_issue_activity(
    pool: &sqlx::SqlitePool,
    code: &str,
) -> Result<bool, AppError> {
    let updated = sqlx::query("UPDATE issues SET last_activity_at = CURRENT_TIMESTAMP WHERE code = ? AND status = 'ACTIVE' AND datetime(last_activity_at) < datetime('now', '-1 hour')")
        .bind(code)
        .execute(pool)
        .await
        .map_err(AppError::Database)?
        .rows_affected();
    Ok(updated == 1)
}

pub(crate) async fn touch_issue_activity_best_effort(
    pool: &sqlx::SqlitePool,
    code: &str,
    operation: &'static str,
) -> bool {
    match touch_issue_activity(pool, code).await {
        Ok(updated) => updated,
        Err(error) => {
            tracing::warn!(issue_code = code, operation, %error, "failed to refresh issue activity after successful operation");
            false
        }
    }
}

pub fn normalize_issue_code(value: &str) -> Result<String, AppError> {
    let code = value.trim().to_uppercase();
    if code.is_empty() || code.len() > ISSUE_CODE_MAX_LEN {
        return Err(AppError::BadRequest(
            "issue_code must be 1-64 characters".into(),
        ));
    }
    if !code
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(AppError::BadRequest(
            "issue_code may only contain letters, numbers, '.', '_' and '-'".into(),
        ));
    }
    Ok(code)
}

// scoped under /api in routes::register, so keep relative paths here
#[get("/issues")]
pub async fn list_issues(
    user: OptionalUser,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let rows = sqlx::query_as::<_, IssueSummary>(
        r#"
        SELECT
            issues.code,
            issues.name,
            (SELECT COUNT(*) FROM bundles b WHERE b.issue_code = issues.code AND b.deleted_at IS NULL) AS bundle_count,
            CASE WHEN issues.owner_user_id = ? THEN 1 ELSE 0 END AS can_write,
            issue_owner.username AS owner_username
        FROM issues
        LEFT JOIN users issue_owner ON issue_owner.id = issues.owner_user_id
        WHERE issues.status = 'ACTIVE'
        ORDER BY issues.code DESC
        "#,
    )
    .bind(user.0.as_ref().map(|user| user.id.as_str()).unwrap_or(""))
    .fetch_all(&state.db.pool)
    .await
    .map_err(AppError::Database)?;
    let authenticated = user.0.is_some();
    let rows = rows
        .into_iter()
        .map(|mut row| {
            if !authenticated {
                row.owner_username = None;
            }
            row
        })
        .collect::<Vec<_>>();

    Ok(HttpResponse::Ok().json(rows))
}

#[derive(Debug, Deserialize)]
pub struct CreateIssueRequest {
    pub code: String,
    pub name: Option<String>,
}

#[post("/issues")]
pub async fn create_issue(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    payload: web::Json<CreateIssueRequest>,
) -> Result<HttpResponse, AppError> {
    let code = normalize_issue_code(&payload.code)?;
    let name = payload
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&code)
        .to_owned();

    if name.chars().count() > ISSUE_NAME_MAX_LEN {
        return Err(AppError::BadRequest(
            "issue name must not exceed 128 characters".into(),
        ));
    }

    let result = sqlx::query(
        r#"
        INSERT INTO issues (code, name, owner_user_id)
        VALUES (?, ?, ?)
        ON CONFLICT(code) DO NOTHING
        "#,
    )
    .bind(&code)
    .bind(&name)
    .bind(&user.0.id)
    .execute(&state.db.pool)
    .await
    .map_err(AppError::Database)?;

    if result.rows_affected() == 0 {
        return Err(AppError::Conflict(format!("issue {code} already exists")));
    }

    Ok(HttpResponse::Created().json(IssueSummary {
        code,
        name,
        bundle_count: 0,
        can_write: true,
        owner_username: Some(user.0.username.clone()),
    }))
}

#[get("/issues/{issue_id}")]
pub async fn get_issue_bundles(
    user: OptionalUser,
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let issue_code = normalize_issue_code(&path.into_inner())?;
    let issue = sqlx::query_as::<_, IssueRow>(
        "SELECT issues.code, issues.name, issues.owner_user_id, issues.last_activity_at, issue_owner.username AS owner_username FROM issues LEFT JOIN users issue_owner ON issue_owner.id = issues.owner_user_id WHERE issues.code = ? AND issues.status = 'ACTIVE' LIMIT 1",
    )
    .bind(&issue_code)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("issue {issue_code}")))?;

    let rows = sqlx::query_as::<_, BundleRow>(
        "SELECT hash, name, status, process_stage, failure_stage, failure_code, failure_reason, retryable, size_bytes FROM bundles WHERE issue_code = ? AND deleted_at IS NULL ORDER BY created_at DESC",
    )
    .bind(&issue.code)
    .fetch_all(&state.db.pool)
    .await
    .map_err(AppError::Database)?;

    let can_write = user
        .0
        .as_ref()
        .is_some_and(|user| issue.owner_user_id.as_deref() == Some(user.id.as_str()));

    let inactive_days = state
        .issue_inactive_days
        .load(std::sync::atomic::Ordering::Acquire);
    let inactivity_expiry = if can_write && (7..=30).contains(&inactive_days) {
        let previous_expires_at = sqlx::query_scalar::<_, String>(
            "SELECT strftime('%Y-%m-%dT%H:%M:%SZ', datetime(?, '+' || ? || ' days'))",
        )
        .bind(&issue.last_activity_at)
        .bind(inactive_days as i64)
        .fetch_one(&state.db.pool)
        .await
        .map_err(AppError::Database)?;
        let activity_refreshed =
            touch_issue_activity_best_effort(&state.db.pool, &issue_code, "issue detail read")
                .await;
        sqlx::query_as::<_, (String, i64)>(
            "SELECT strftime('%Y-%m-%dT%H:%M:%SZ', datetime(last_activity_at, '+' || ? || ' days')), CASE WHEN ? AND datetime(?) <= datetime('now', '+72 hours') THEN 1 ELSE 0 END FROM issues WHERE code = ? AND status = 'ACTIVE'",
        )
        .bind(inactive_days as i64)
        .bind(activity_refreshed)
        .bind(previous_expires_at)
        .bind(&issue_code)
        .fetch_optional(&state.db.pool)
        .await
        .map_err(AppError::Database)?
        .map(|(expires_at, renewed_from_expiring)| IssueInactivityExpiry {
            inactive_days,
            expires_at,
            renewed_from_expiring: renewed_from_expiring != 0,
        })
    } else {
        touch_issue_activity_best_effort(&state.db.pool, &issue_code, "issue detail read").await;
        None
    };

    let response = IssueBundlesResponse {
        name: issue.name,
        owner_username: user.0.as_ref().and_then(|_| issue.owner_username.clone()),
        can_write,
        inactivity_expiry,
        log_bundles: rows
            .into_iter()
            .map(|bundle| {
                let upload_status = UploadStatus::from_db_value(&bundle.status);
                crate::models::issues::UploadSummary {
                    hash: bundle.hash,
                    name: bundle.name,
                    status: UploadStatusWrapper { upload_status },
                    stage: match upload_status {
                        UploadStatus::Ready => UploadStage::Ready,
                        UploadStatus::Failed => UploadStage::Failed,
                        _ => UploadStage::from_db_value(&bundle.process_stage),
                    },
                    failure_reason: bundle.failure_reason,
                    failure_stage: bundle.failure_stage,
                    failure_code: bundle.failure_code,
                    retryable: bundle.retryable,
                    size_bytes: bundle.size_bytes.map(|size| size.max(0) as u64),
                }
            })
            .collect(),
    };

    Ok(HttpResponse::Ok()
        .insert_header((CACHE_CONTROL, "no-store, private"))
        .json(response))
}

pub async fn cleanup_inactive_issues(state: &web::Data<AppState>) -> Result<usize, AppError> {
    cleanup_inactive_issues_with_lease(state, INACTIVE_CLEANUP_LEASE_SECONDS).await
}

pub async fn resume_manual_issue_deletions(pool: &sqlx::SqlitePool) -> Result<u64, AppError> {
    let issues: Vec<String> = sqlx::query_scalar("SELECT code FROM issues WHERE status='DELETING' AND deletion_reason='MANUAL' AND (deletion_retry_at IS NULL OR datetime(deletion_retry_at) <= datetime('now')) AND (deletion_lease_until IS NULL OR datetime(deletion_lease_until) <= datetime('now')) ORDER BY COALESCE(deletion_retry_at, ''), code LIMIT 20")
        .fetch_all(pool).await.map_err(AppError::Database)?;
    let mut resumed = 0u64;
    for code in issues {
        let token = Uuid::new_v4().to_string();
        if claim_manual_recovery(pool, &code, &token, MANUAL_CLEANUP_LEASE_SECONDS).await? {
            match finish_manual_issue_deletion(pool, &code, &token, MANUAL_CLEANUP_LEASE_SECONDS)
                .await
            {
                Ok(()) => resumed += 1,
                Err(error) => {
                    schedule_manual_retry(pool, &code, &token).await;
                    tracing::error!(issue_code = code, %error, "manual issue deletion recovery failed");
                }
            }
        }
    }
    Ok(resumed)
}

async fn claim_manual_recovery(
    pool: &sqlx::SqlitePool,
    code: &str,
    token: &str,
    seconds: u64,
) -> Result<bool, AppError> {
    let changed = sqlx::query("UPDATE issues SET deletion_lease_token=?, deletion_lease_until=datetime('now', '+' || ? || ' seconds'), deletion_retry_at=NULL WHERE code=? AND status='DELETING' AND deletion_reason='MANUAL' AND (deletion_retry_at IS NULL OR datetime(deletion_retry_at) <= datetime('now')) AND (deletion_lease_until IS NULL OR datetime(deletion_lease_until) <= datetime('now'))")
        .bind(token).bind(seconds as i64).bind(code).execute(pool).await.map_err(AppError::Database)?.rows_affected();
    Ok(changed == 1)
}

async fn normalize_legacy_manual_deletion(
    pool: &sqlx::SqlitePool,
    code: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE issues SET deletion_reason='MANUAL', deletion_lease_token=NULL, deletion_lease_until=NULL, deletion_retry_at=NULL WHERE code=? AND status='DELETING' AND deletion_reason IS NULL",
    )
    .bind(code)
    .execute(pool)
    .await
    .map_err(AppError::Database)?;
    Ok(())
}

async fn schedule_manual_retry(pool: &sqlx::SqlitePool, code: &str, token: &str) {
    let _ = sqlx::query("UPDATE issues SET deletion_lease_token=NULL, deletion_lease_until=NULL, deletion_attempts=deletion_attempts+1, deletion_retry_at=datetime('now', '+' || MIN((deletion_attempts + 1) * 60, 3600) || ' seconds') WHERE code=? AND status='DELETING' AND deletion_reason='MANUAL' AND deletion_lease_token=?")
        .bind(code).bind(token).execute(pool).await;
}

async fn cleanup_inactive_issues_with_lease(
    state: &web::Data<AppState>,
    lease_seconds: u64,
) -> Result<usize, AppError> {
    if lease_seconds == 0 {
        return Err(AppError::Config(
            "inactive cleanup lease must be positive".into(),
        ));
    }
    let days = state
        .issue_inactive_days
        .load(std::sync::atomic::Ordering::Acquire);
    let deleting: Vec<(String, Option<String>, String, i64)> = sqlx::query_as(
        "SELECT code, owner_user_id, last_activity_at, inactive_claim_days FROM issues WHERE status='DELETING' AND deletion_reason='INACTIVE' AND inactive_claim_days IS NOT NULL AND (deletion_retry_at IS NULL OR datetime(deletion_retry_at) <= datetime('now')) AND (deletion_lease_until IS NULL OR datetime(deletion_lease_until) <= datetime('now')) ORDER BY COALESCE(deletion_retry_at, ''), code LIMIT 20",
    )
    .fetch_all(&state.db.pool)
    .await
    .map_err(AppError::Database)?;
    let mut cleaned = 0usize;
    for (code, owner, last_activity_at, claimed_days) in deleting {
        let lease_token = Uuid::new_v4().to_string();
        if !claim_inactive_recovery(&state.db.pool, &code, &lease_token, lease_seconds).await? {
            continue;
        }
        match finish_auto_issue_deletion(
            &state.db.pool,
            &code,
            owner.as_deref(),
            &last_activity_at,
            claimed_days as usize,
            &lease_token,
            lease_seconds,
        )
        .await
        {
            Ok(true) => cleaned += 1,
            Ok(false) => {}
            Err(error) => {
                schedule_inactive_retry(&state.db.pool, &code, &lease_token).await;
                tracing::error!(issue_code = code, %error, "deleting issue recovery failed")
            }
        }
    }
    if days == 0 {
        return Ok(cleaned);
    }
    let modifier = format!("-{days} days");
    let candidates: Vec<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT code, owner_user_id, last_activity_at FROM issues WHERE status='ACTIVE' AND datetime(last_activity_at) < datetime('now', ?) AND NOT EXISTS (SELECT 1 FROM bundles WHERE bundles.issue_code=issues.code AND bundles.status IN ('PENDING','PROCESSING')) ORDER BY last_activity_at LIMIT 20",
    )
    .bind(&modifier)
    .fetch_all(&state.db.pool)
    .await
    .map_err(AppError::Database)?;
    for (code, owner, last_activity_at) in candidates {
        let lease_token = Uuid::new_v4().to_string();
        if !claim_inactive_issue(
            &state.db.pool,
            &code,
            &modifier,
            days,
            &lease_token,
            lease_seconds,
        )
        .await?
        {
            continue;
        }
        match finish_auto_issue_deletion(
            &state.db.pool,
            &code,
            owner.as_deref(),
            &last_activity_at,
            days,
            &lease_token,
            lease_seconds,
        )
        .await
        {
            Ok(true) => cleaned += 1,
            Ok(false) => {}
            Err(error) => {
                schedule_inactive_retry(&state.db.pool, &code, &lease_token).await;
                tracing::error!(issue_code = code, %error, "inactive issue cleanup failed")
            }
        }
    }
    Ok(cleaned)
}

async fn claim_inactive_issue(
    pool: &sqlx::SqlitePool,
    code: &str,
    modifier: &str,
    days: usize,
    lease_token: &str,
    lease_seconds: u64,
) -> Result<bool, AppError> {
    let lease_modifier = format!("+{lease_seconds} seconds");
    let claimed = sqlx::query("UPDATE issues SET status='DELETING', deletion_reason='INACTIVE', inactive_claim_days=?, deletion_lease_token=?, deletion_lease_until=datetime('now',?), deletion_retry_at=NULL, deletion_attempts=0 WHERE code=? AND status='ACTIVE' AND datetime(last_activity_at) < datetime('now', ?) AND NOT EXISTS (SELECT 1 FROM bundles WHERE bundles.issue_code=issues.code AND bundles.status IN ('PENDING','PROCESSING'))")
        .bind(days as i64)
        .bind(lease_token)
        .bind(lease_modifier)
        .bind(code)
        .bind(modifier)
        .execute(pool)
        .await
        .map_err(AppError::Database)?
        .rows_affected();
    Ok(claimed == 1)
}

async fn claim_inactive_recovery(
    pool: &sqlx::SqlitePool,
    code: &str,
    lease_token: &str,
    lease_seconds: u64,
) -> Result<bool, AppError> {
    let lease_modifier = format!("+{lease_seconds} seconds");
    let claimed = sqlx::query("UPDATE issues SET deletion_lease_token=?, deletion_lease_until=datetime('now',?) WHERE code=? AND status='DELETING' AND deletion_reason='INACTIVE' AND inactive_claim_days IS NOT NULL AND (deletion_retry_at IS NULL OR datetime(deletion_retry_at) <= datetime('now')) AND (deletion_lease_until IS NULL OR datetime(deletion_lease_until) <= datetime('now'))")
        .bind(lease_token)
        .bind(lease_modifier)
        .bind(code)
        .execute(pool)
        .await
        .map_err(AppError::Database)?
        .rows_affected();
    Ok(claimed == 1)
}

async fn schedule_inactive_retry(pool: &sqlx::SqlitePool, code: &str, lease_token: &str) {
    if let Err(error) = sqlx::query("UPDATE issues SET deletion_lease_token=NULL, deletion_lease_until=NULL, deletion_attempts=deletion_attempts+1, deletion_retry_at=datetime('now', '+' || MIN((deletion_attempts + 1) * 60, 3600) || ' seconds') WHERE code=? AND status='DELETING' AND deletion_reason='INACTIVE' AND deletion_lease_token=?")
        .bind(code)
        .bind(lease_token)
        .execute(pool)
        .await
    {
        tracing::warn!(issue_code = code, %error, "failed to schedule inactive issue cleanup retry");
    }
}

async fn finish_auto_issue_deletion(
    pool: &sqlx::SqlitePool,
    code: &str,
    owner: Option<&str>,
    last_activity_at: &str,
    days: usize,
    lease_token: &str,
    lease_seconds: u64,
) -> Result<bool, AppError> {
    let bundles: Vec<BundleIdRow> =
        sqlx::query_as("SELECT id, issue_code, status FROM bundles WHERE issue_code = ?")
            .bind(code)
            .fetch_all(pool)
            .await
            .map_err(AppError::Database)?;
    reject_processing_bundles(&bundles)?;
    for bundle in &bundles {
        require_inactive_lease(pool, code, lease_token, lease_seconds).await?;
        if bundle.status != "DELETED" {
            if bundle.status != "DELETING" {
                sqlx::query(
                    "UPDATE bundles SET status='DELETING', deleted_at=CURRENT_TIMESTAMP WHERE id=?",
                )
                .bind(&bundle.id)
                .execute(pool)
                .await
                .map_err(AppError::Database)?;
            }
            finish_bundle_deletion_with_inactive_lease(
                pool,
                &bundle.id,
                code,
                lease_token,
                lease_seconds,
            )
            .await?;
        }
        require_inactive_lease(pool, code, lease_token, lease_seconds).await?;
    }
    let mut tx = pool.begin().await.map_err(AppError::Database)?;
    sqlx::query("DELETE FROM saved_searches WHERE scope_type='ISSUE' AND scope_key=?")
        .bind(code)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;
    let deleted = sqlx::query("DELETE FROM issues WHERE code=? AND status='DELETING' AND deletion_reason='INACTIVE' AND deletion_lease_token=?")
        .bind(code)
        .bind(lease_token)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?
        .rows_affected();
    if deleted != 1 {
        tx.rollback().await.map_err(AppError::Database)?;
        return Ok(false);
    }
    sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,action,old_value,new_value) VALUES(?,'SYSTEM','ISSUE_AUTO_EXPIRED',?,?)")
        .bind(Uuid::new_v4().to_string())
        .bind(format!("issue={code};owner={};last_activity_at={last_activity_at}", owner.unwrap_or("")))
        .bind(format!("inactive_days={days};bundles={}", bundles.len()))
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;
    tx.commit().await.map_err(AppError::Database)?;
    Ok(true)
}

async fn require_inactive_lease(
    pool: &sqlx::SqlitePool,
    code: &str,
    lease_token: &str,
    lease_seconds: u64,
) -> Result<(), AppError> {
    if renew_inactive_issue_lease(pool, code, lease_token, lease_seconds).await? {
        Ok(())
    } else {
        Err(AppError::Conflict(format!(
            "inactive cleanup lease for issue {code} was lost"
        )))
    }
}

pub async fn require_issue_owner(
    pool: &sqlx::SqlitePool,
    code: &str,
    user_id: &str,
) -> Result<String, AppError> {
    let code = normalize_issue_code(code)?;
    let owner = sqlx::query_as::<_, OwnerRow>(
        "SELECT owner_user_id FROM issues WHERE code = ? AND status = 'ACTIVE' LIMIT 1",
    )
    .bind(&code)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)?;
    let Some(owner) = owner else {
        return Err(AppError::NotFound(format!("issue {code}")));
    };
    if owner.owner_user_id.as_deref() != Some(user_id) {
        return Err(AppError::api(
            actix_web::http::StatusCode::FORBIDDEN,
            "ISSUE_WRITE_FORBIDDEN",
            "无权修改此 Issue",
        ));
    }
    Ok(code)
}

pub async fn require_issue_owner_for_delete(
    pool: &sqlx::SqlitePool,
    code: &str,
    user_id: &str,
) -> Result<String, AppError> {
    let code = normalize_issue_code(code)?;
    let row = sqlx::query_as::<_, DeleteOwnerRow>(
        "SELECT owner_user_id, status FROM issues WHERE code = ? LIMIT 1",
    )
    .bind(&code)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("issue {code}")))?;
    if !matches!(row.status.as_str(), "ACTIVE" | "DELETING") {
        return Err(AppError::NotFound(format!("issue {code}")));
    }
    if row.owner_user_id.as_deref() != Some(user_id) {
        return Err(AppError::api(
            actix_web::http::StatusCode::FORBIDDEN,
            "ISSUE_WRITE_FORBIDDEN",
            "无权修改此 Issue",
        ));
    }
    Ok(code)
}

#[derive(FromRow)]
struct OwnerRow {
    owner_user_id: Option<String>,
}

#[derive(FromRow)]
struct DeleteOwnerRow {
    owner_user_id: Option<String>,
    status: String,
}

#[derive(FromRow, Deserialize)]
struct IssueRow {
    code: String,
    name: String,
    owner_user_id: Option<String>,
    last_activity_at: String,
    owner_username: Option<String>,
}

#[derive(FromRow)]
struct BundleRow {
    hash: String,
    name: String,
    status: String,
    process_stage: String,
    failure_reason: Option<String>,
    failure_stage: Option<String>,
    failure_code: Option<String>,
    retryable: Option<bool>,
    size_bytes: Option<i64>,
}

#[derive(FromRow)]
struct BundleIdRow {
    id: String,
    #[allow(dead_code)]
    issue_code: String,
    status: String,
}

#[delete("/issues/{issue_id}/bundles/{bundle_hash}")]
pub async fn delete_issue_bundle(
    user: RequireBusinessUser,
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let (issue_code, bundle_hash) = path.into_inner();
    let issue_code = require_issue_owner(&state.db.pool, &issue_code, &user.0.id).await?;
    let bundle: BundleIdRow = sqlx::query_as(
        r#"
        SELECT id, issue_code, status
        FROM bundles
        WHERE issue_code = ? AND hash = ?
        LIMIT 1
        "#,
    )
    .bind(&issue_code)
    .bind(&bundle_hash)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("bundle {bundle_hash}")))?;
    reject_processing_bundle(&bundle)?;
    sqlx::query(
        "UPDATE bundles SET status = 'DELETING', deleted_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(&bundle.id)
    .execute(&state.db.pool)
    .await
    .map_err(AppError::Database)?;

    // Finish request-scoped writes before the heavyweight cleanup can compete for
    // SQLite's single writer lock.
    touch_issue_activity_best_effort(&state.db.pool, &issue_code, "bundle deletion").await;

    let pool = state.db.pool.clone();
    let bundle_id = bundle.id.clone();
    tokio::spawn(async move {
        if let Err(error) = crate::db::finish_bundle_deletion(&pool, &bundle_id).await {
            tracing::error!(bundle_id, %error, "background bundle deletion failed; it will be retried at startup");
        }
    });

    Ok(HttpResponse::Accepted().finish())
}

#[delete("/issues/{issue_id}")]
pub async fn delete_issue(
    user: RequireBusinessUser,
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let issue_code =
        require_issue_owner_for_delete(&state.db.pool, &path.into_inner(), &user.0.id).await?;
    let claimed =
        sqlx::query("UPDATE issues SET status = 'DELETING', deletion_reason = 'MANUAL', inactive_claim_days = NULL, deletion_lease_token = NULL, deletion_lease_until = NULL, deletion_retry_at = NULL, deletion_attempts = 0 WHERE code = ? AND status = 'ACTIVE'")
            .bind(&issue_code)
            .execute(&state.db.pool)
            .await
            .map_err(AppError::Database)?
            .rows_affected();
    let newly_claimed = claimed == 1;
    if claimed == 0 {
        let issue_state: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT status, deletion_reason FROM issues WHERE code = ?")
                .bind(&issue_code)
                .fetch_optional(&state.db.pool)
                .await
                .map_err(AppError::Database)?;
        match issue_state
            .as_ref()
            .map(|(status, reason)| (status.as_str(), reason.as_deref()))
        {
            None => return Ok(HttpResponse::NoContent().finish()),
            Some(("DELETING", Some("INACTIVE"))) => {
                return Err(AppError::Conflict(format!(
                    "issue {issue_code} is being deleted by inactive cleanup"
                )));
            }
            Some(("DELETING", None)) => {
                normalize_legacy_manual_deletion(&state.db.pool, &issue_code).await?;
            }
            Some(("DELETING", Some(_))) => {} // Resume a previous synchronous deletion attempt.
            Some((status, _)) => {
                return Err(AppError::Conflict(format!(
                    "issue {issue_code} cannot be deleted from {status}"
                )));
            }
        }
    }
    let bundles: Vec<BundleIdRow> = sqlx::query_as(
        r#"
        SELECT id, issue_code, status
        FROM bundles
        WHERE issue_code = ?
        "#,
    )
    .bind(&issue_code)
    .fetch_all(&state.db.pool)
    .await
    .map_err(AppError::Database)?;

    if let Err(error) = reject_processing_bundles(&bundles) {
        if newly_claimed {
            sqlx::query(
                "UPDATE issues SET status = 'ACTIVE', deletion_reason = NULL, inactive_claim_days = NULL, deletion_lease_token = NULL, deletion_lease_until = NULL, deletion_retry_at = NULL, deletion_attempts = 0 WHERE code = ? AND status = 'DELETING'",
            )
            .bind(&issue_code)
            .execute(&state.db.pool)
            .await
            .map_err(AppError::Database)?;
        }
        return Err(error);
    }

    let pool = state.db.pool.clone();
    let cleanup_issue_code = issue_code.clone();
    let lease_token = Uuid::new_v4().to_string();
    let claimed = claim_manual_recovery(
        &pool,
        &cleanup_issue_code,
        &lease_token,
        MANUAL_CLEANUP_LEASE_SECONDS,
    )
    .await?;
    if !claimed {
        return Ok(HttpResponse::Accepted().finish());
    }
    tokio::spawn(async move {
        if let Err(error) = finish_manual_issue_deletion(
            &pool,
            &cleanup_issue_code,
            &lease_token,
            MANUAL_CLEANUP_LEASE_SECONDS,
        )
        .await
        {
            schedule_manual_retry(&pool, &cleanup_issue_code, &lease_token).await;
            tracing::error!(issue_code = cleanup_issue_code, %error, "background issue deletion failed; it can be retried");
        }
    });

    Ok(HttpResponse::Accepted().finish())
}

async fn finish_manual_issue_deletion(
    pool: &sqlx::SqlitePool,
    issue_code: &str,
    lease_token: &str,
    lease_seconds: u64,
) -> Result<(), AppError> {
    let bundles: Vec<BundleIdRow> =
        sqlx::query_as("SELECT id, issue_code, status FROM bundles WHERE issue_code = ?")
            .bind(issue_code)
            .fetch_all(pool)
            .await
            .map_err(AppError::Database)?;
    reject_processing_bundles(&bundles)?;

    for bundle in &bundles {
        if !crate::db::renew_inactive_issue_lease(pool, issue_code, lease_token, lease_seconds)
            .await?
        {
            return Err(AppError::Conflict(
                "manual issue deletion lease was lost".into(),
            ));
        }
        if bundle.status == "DELETED" {
            continue;
        }
        if bundle.status != "DELETING" {
            sqlx::query(
                "UPDATE bundles SET status = 'DELETING', deleted_at = CURRENT_TIMESTAMP WHERE id = ?",
            )
            .bind(&bundle.id)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
        }
        crate::db::finish_bundle_deletion_with_inactive_lease(
            pool,
            &bundle.id,
            issue_code,
            lease_token,
            lease_seconds,
        )
        .await?;
    }

    let mut tx = pool.begin().await.map_err(AppError::Database)?;
    sqlx::query("DELETE FROM saved_searches WHERE scope_type='ISSUE' AND scope_key=?")
        .bind(issue_code)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;
    let deleted = sqlx::query("DELETE FROM issues WHERE code = ? AND status = 'DELETING' AND deletion_reason = 'MANUAL' AND deletion_lease_token = ?")
        .bind(issue_code)
        .bind(lease_token)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?
        .rows_affected();
    if deleted != 1 {
        tx.rollback().await.map_err(AppError::Database)?;
        return Err(AppError::Conflict(
            "manual issue deletion lease was lost".into(),
        ));
    }
    tx.commit().await.map_err(AppError::Database)?;
    Ok(())
}

fn reject_processing_bundle(bundle: &BundleIdRow) -> Result<(), AppError> {
    if is_processing_bundle_status(&bundle.status) {
        return Err(AppError::Conflict(
            "processing bundle cannot be deleted".into(),
        ));
    }
    Ok(())
}

fn reject_processing_bundles(bundles: &[BundleIdRow]) -> Result<(), AppError> {
    if bundles
        .iter()
        .any(|bundle| is_processing_bundle_status(&bundle.status))
    {
        return Err(AppError::Conflict(
            "issue with processing bundles cannot be deleted".into(),
        ));
    }
    Ok(())
}

fn is_processing_bundle_status(status: &str) -> bool {
    matches!(
        status.to_ascii_uppercase().as_str(),
        "PENDING" | "PROCESSING"
    )
}

#[cfg(test)]
mod tests {
    use actix_web::web;
    use sqlx::sqlite::SqlitePoolOptions;

    use crate::{
        AppState,
        config::AppLimits,
        db,
        repositories::users::{self, CreateUserOutcome},
    };

    use super::{
        claim_inactive_issue, claim_inactive_recovery, cleanup_inactive_issues,
        cleanup_inactive_issues_with_lease, normalize_legacy_manual_deletion,
        require_inactive_lease, require_issue_owner, require_issue_owner_for_delete,
        resume_manual_issue_deletions, touch_issue_activity,
    };

    #[tokio::test]
    async fn manual_recovery_rejects_processing_bundle_and_backs_off() {
        let pool = db::init_pool("sqlite:file:manual-processing-recovery?mode=memory&cache=shared")
            .unwrap();
        db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name,status,deletion_reason) VALUES('MANUAL_BUSY','Busy','DELETING','MANUAL')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status,process_stage) VALUES('manual-busy-bundle','MANUAL_BUSY','hash','busy','PROCESSING','INDEXING')")
            .execute(&pool).await.unwrap();

        assert_eq!(resume_manual_issue_deletions(&pool).await.unwrap(), 0);
        let state: (String, String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT status, deletion_reason, deletion_lease_token, deletion_retry_at FROM issues WHERE code='MANUAL_BUSY'",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(state.0, "DELETING");
        assert_eq!(state.1, "MANUAL");
        assert!(state.2.is_none());
        assert!(state.3.is_some());
        let bundle_status: String =
            sqlx::query_scalar("SELECT status FROM bundles WHERE id='manual-busy-bundle'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(bundle_status, "PROCESSING");
    }

    #[tokio::test]
    async fn legacy_manual_deletion_is_normalized_before_recovery() {
        let pool =
            db::init_pool("sqlite:file:legacy-manual-recovery?mode=memory&cache=shared").unwrap();
        db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name,status,deletion_reason) VALUES('LEGACY','Legacy','DELETING',NULL)")
            .execute(&pool).await.unwrap();

        normalize_legacy_manual_deletion(&pool, "LEGACY")
            .await
            .unwrap();
        assert_eq!(resume_manual_issue_deletions(&pool).await.unwrap(), 1);
        let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issues WHERE code='LEGACY'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(exists, 0);
    }

    #[tokio::test]
    async fn issue_owner_checks_reject_null_and_foreign_users_and_allow_delete_retry() {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        db::prepare_schema(&pool, false).await.unwrap();
        let owner = match users::create_user(&pool, "owner", "hash").await.unwrap() {
            CreateUserOutcome::Created(user) => user,
            CreateUserOutcome::DuplicateUsername => unreachable!(),
        };
        let other = match users::create_user(&pool, "other", "hash").await.unwrap() {
            CreateUserOutcome::Created(user) => user,
            CreateUserOutcome::DuplicateUsername => unreachable!(),
        };
        sqlx::query("INSERT INTO issues (code, name, owner_user_id) VALUES ('OWNED', 'Owned', ?), ('EMPTY', 'Empty', NULL)")
            .bind(&owner.id).execute(&pool).await.unwrap();

        assert!(require_issue_owner(&pool, "OWNED", &owner.id).await.is_ok());
        assert!(
            require_issue_owner(&pool, "OWNED", &other.id)
                .await
                .is_err()
        );
        assert!(
            require_issue_owner(&pool, "EMPTY", &owner.id)
                .await
                .is_err()
        );
        sqlx::query("UPDATE issues SET status = 'DELETING' WHERE code = 'OWNED'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            require_issue_owner_for_delete(&pool, "OWNED", &owner.id)
                .await
                .is_ok()
        );
        assert!(
            require_issue_owner_for_delete(&pool, "OWNED", &other.id)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn activity_is_throttled_and_inactive_cleanup_skips_fresh_and_processing_issues() {
        let pool = db::init_pool("sqlite::memory:").unwrap();
        db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name,last_activity_at) VALUES ('OLD','Old',datetime('now','-3 days')),('FRESH','Fresh',CURRENT_TIMESTAMP),('BUSY','Busy',datetime('now','-3 days'))")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status,process_stage) VALUES('busy-bundle','BUSY','busy-hash','busy','PROCESSING','INDEXING')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status,process_stage) VALUES('old-bundle','OLD','old-hash','old','READY','READY')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO files(bundle_id,name,path,is_dir,status) VALUES('old-bundle','old.log','old.log',0,'READY')")
            .execute(&pool).await.unwrap();
        let old_file_id: i64 =
            sqlx::query_scalar("SELECT id FROM files WHERE bundle_id='old-bundle'")
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query("INSERT INTO log_segments(bundle_id,file_id,content) VALUES('old-bundle',?,'old content')")
            .bind(old_file_id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO log_line_offsets(file_id,line_number,byte_offset) VALUES(?,1,0)")
            .bind(old_file_id)
            .execute(&pool)
            .await
            .unwrap();
        let owner = match users::create_user(&pool, "cleanup-owner", "hash")
            .await
            .unwrap()
        {
            CreateUserOutcome::Created(user) => user,
            CreateUserOutcome::DuplicateUsername => unreachable!(),
        };
        sqlx::query("UPDATE issues SET owner_user_id=? WHERE code='OLD'")
            .bind(&owner.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO saved_searches(id,user_id,name,search_type,query_text,scope_type,scope_key) VALUES('saved-old',?,'old','DETAIL','x','ISSUE','OLD')")
            .bind(&owner.id).execute(&pool).await.unwrap();
        let before: String =
            sqlx::query_scalar("SELECT last_activity_at FROM issues WHERE code='FRESH'")
                .fetch_one(&pool)
                .await
                .unwrap();
        touch_issue_activity(&pool, "FRESH").await.unwrap();
        let after: String =
            sqlx::query_scalar("SELECT last_activity_at FROM issues WHERE code='FRESH'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(before, after);
        sqlx::query(
            "UPDATE issues SET last_activity_at=datetime('now','-2 hours') WHERE code='FRESH'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let stale: String =
            sqlx::query_scalar("SELECT last_activity_at FROM issues WHERE code='FRESH'")
                .fetch_one(&pool)
                .await
                .unwrap();
        touch_issue_activity(&pool, "FRESH").await.unwrap();
        let refreshed: String =
            sqlx::query_scalar("SELECT last_activity_at FROM issues WHERE code='FRESH'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_ne!(refreshed, stale);

        let state = web::Data::new(AppState::new(
            pool.clone(),
            "data".into(),
            AppLimits::default(),
        ));
        assert_eq!(cleanup_inactive_issues(&state).await.unwrap(), 0);
        state
            .issue_inactive_days
            .store(1, std::sync::atomic::Ordering::Release);
        assert_eq!(cleanup_inactive_issues(&state).await.unwrap(), 1);
        let remaining: Vec<String> = sqlx::query_scalar("SELECT code FROM issues ORDER BY code")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, vec!["BUSY", "FRESH"]);
        let audit: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM admin_audit_logs WHERE action='ISSUE_AUTO_EXPIRED'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audit, 1);
        let saved: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM saved_searches WHERE scope_key='OLD'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(saved, 0);
        let content_rows: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM bundles WHERE id='old-bundle'),(SELECT COUNT(*) FROM files WHERE bundle_id='old-bundle'),(SELECT COUNT(*) FROM log_segments WHERE bundle_id='old-bundle'),(SELECT COUNT(*) FROM log_line_offsets WHERE file_id=?)")
            .bind(old_file_id).fetch_one(&pool).await.unwrap();
        assert_eq!(content_rows, (0, 0, 0, 0));

        sqlx::query("INSERT INTO issues(code,name,status,last_activity_at,deletion_reason,inactive_claim_days) VALUES ('RECOVER','Recover','DELETING',datetime('now','-3 days'),'INACTIVE',7),('MANUAL','Manual','DELETING',datetime('now','-3 days'),'MANUAL',NULL)")
            .execute(&pool).await.unwrap();
        state
            .issue_inactive_days
            .store(0, std::sync::atomic::Ordering::Release);
        assert_eq!(cleanup_inactive_issues(&state).await.unwrap(), 1);
        let deleting: Vec<String> =
            sqlx::query_scalar("SELECT code FROM issues WHERE status='DELETING' ORDER BY code")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(deleting, vec!["MANUAL"]);
        let recovery_audit: String = sqlx::query_scalar("SELECT new_value FROM admin_audit_logs WHERE action='ISSUE_AUTO_EXPIRED' AND old_value LIKE 'issue=RECOVER;%' LIMIT 1")
            .fetch_one(&pool).await.unwrap();
        assert!(recovery_audit.contains("inactive_days=7"));

        state
            .issue_inactive_days
            .store(1, std::sync::atomic::Ordering::Release);
        sqlx::query("INSERT INTO issues(code,name,last_activity_at) VALUES ('FAIL','Fail',datetime('now','-5 days')),('GOOD','Good',datetime('now','-4 days'))")
            .execute(&pool).await.unwrap();
        sqlx::query("CREATE TRIGGER reject_fail_issue_delete BEFORE DELETE ON issues WHEN OLD.code='FAIL' BEGIN SELECT RAISE(FAIL, 'forced cleanup failure'); END")
            .execute(&pool).await.unwrap();
        assert_eq!(cleanup_inactive_issues(&state).await.unwrap(), 1);
        let failed_status: String =
            sqlx::query_scalar("SELECT status FROM issues WHERE code='FAIL'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(failed_status, "DELETING");
        let good_exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issues WHERE code='GOOD'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(good_exists, 0);
        let audits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM admin_audit_logs WHERE action='ISSUE_AUTO_EXPIRED'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audits, 3);
    }

    #[tokio::test]
    async fn inactive_issue_claim_is_single_winner() {
        let pool = db::init_pool("sqlite:file:issue-claim?mode=memory&cache=shared").unwrap();
        db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name,last_activity_at) VALUES('RACE','Race',datetime('now','-3 days'))")
            .execute(&pool).await.unwrap();
        let (first, second) = tokio::join!(
            claim_inactive_issue(&pool, "RACE", "-1 days", 1, "first", 600),
            claim_inactive_issue(&pool, "RACE", "-1 days", 1, "second", 600)
        );
        assert_eq!(
            [first.unwrap(), second.unwrap()]
                .into_iter()
                .filter(|won| *won)
                .count(),
            1
        );
        let state: (String, Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT status,deletion_reason,inactive_claim_days FROM issues WHERE code='RACE'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, ("DELETING".into(), Some("INACTIVE".into()), Some(1)));
    }

    #[tokio::test]
    async fn inactive_lease_renewal_prevents_takeover_after_original_expiry() {
        let pool = db::init_pool("sqlite:file:lease-renewal?mode=memory&cache=shared").unwrap();
        db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name,last_activity_at) VALUES('LEASE','Lease',datetime('now','-3 days'))")
            .execute(&pool).await.unwrap();
        assert!(
            claim_inactive_issue(&pool, "LEASE", "-1 days", 1, "worker", 3)
                .await
                .unwrap()
        );
        let active_token: String =
            sqlx::query_scalar("SELECT deletion_lease_token FROM issues WHERE code='LEASE'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let original_lease_until: String =
            sqlx::query_scalar("SELECT deletion_lease_until FROM issues WHERE code='LEASE'")
                .fetch_one(&pool)
                .await
                .unwrap();
        require_inactive_lease(&pool, "LEASE", &active_token, 10)
            .await
            .unwrap();
        let mut original_lease_expired = false;
        for _ in 0..50 {
            original_lease_expired = sqlx::query_scalar("SELECT datetime('now') >= datetime(?)")
                .bind(&original_lease_until)
                .fetch_one(&pool)
                .await
                .unwrap();
            if original_lease_expired {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(
            original_lease_expired,
            "original lease did not expire during the test"
        );
        assert!(
            !claim_inactive_recovery(&pool, "LEASE", "takeover", 3)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn concurrent_inactive_recovery_deletes_and_audits_once() {
        let pool = db::init_pool("sqlite:file:issue-recovery?mode=memory&cache=shared").unwrap();
        db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name,status,last_activity_at,deletion_reason,inactive_claim_days) VALUES('RECOVERY_RACE','Race','DELETING',datetime('now','-5 days'),'INACTIVE',4)")
            .execute(&pool).await.unwrap();
        let state = web::Data::new(AppState::new(
            pool.clone(),
            "data".into(),
            AppLimits::default(),
        ));
        let (first, second) = tokio::join!(
            cleanup_inactive_issues(&state),
            cleanup_inactive_issues(&state)
        );
        assert_eq!(first.unwrap() + second.unwrap(), 1);
        let audit: (i64, String) = sqlx::query_as("SELECT COUNT(*),MAX(new_value) FROM admin_audit_logs WHERE action='ISSUE_AUTO_EXPIRED' AND old_value LIKE 'issue=RECOVERY_RACE;%' ")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(audit.0, 1);
        assert!(audit.1.contains("inactive_days=4"));
    }

    #[tokio::test]
    async fn failed_recovery_batch_backs_off_and_does_not_starve_later_issues() {
        let pool = db::init_pool("sqlite:file:issue-fairness?mode=memory&cache=shared").unwrap();
        db::prepare_schema(&pool, true).await.unwrap();
        for index in 0..20 {
            let code = format!("A{index:02}");
            sqlx::query("INSERT INTO issues(code,name,status,last_activity_at,deletion_reason,inactive_claim_days) VALUES(?,?,'DELETING',datetime('now','-5 days'),'INACTIVE',1)")
                .bind(&code)
                .bind(&code)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO issues(code,name,status,last_activity_at,deletion_reason,inactive_claim_days) VALUES('Z_SUCCESS','Success','DELETING',datetime('now','-5 days'),'INACTIVE',1)")
            .execute(&pool).await.unwrap();
        sqlx::query("CREATE TRIGGER fail_first_twenty BEFORE DELETE ON issues WHEN OLD.code LIKE 'A%' BEGIN SELECT RAISE(FAIL, 'permanent cleanup failure'); END")
            .execute(&pool).await.unwrap();
        let state = web::Data::new(AppState::new(
            pool.clone(),
            "data".into(),
            AppLimits::default(),
        ));

        assert_eq!(
            cleanup_inactive_issues_with_lease(&state, 60)
                .await
                .unwrap(),
            0
        );
        let backed_off: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issues WHERE code LIKE 'A%' AND deletion_attempts=1 AND datetime(deletion_retry_at)>datetime('now')")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(backed_off, 20);
        assert_eq!(
            cleanup_inactive_issues_with_lease(&state, 60)
                .await
                .unwrap(),
            1
        );
        let success_exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM issues WHERE code='Z_SUCCESS'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(success_exists, 0);
    }
}
