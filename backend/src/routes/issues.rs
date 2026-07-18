use actix_web::{HttpResponse, delete, get, post, web};
use serde::Deserialize;
use sqlx::FromRow;

use crate::{
    AppState,
    db::finish_bundle_deletion,
    error::AppError,
    models::issues::{
        IssueBundlesResponse, IssueSummary, UploadStage, UploadStatus, UploadStatusWrapper,
    },
};

const ISSUE_CODE_MAX_LEN: usize = 64;
const ISSUE_NAME_MAX_LEN: usize = 128;

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
pub async fn list_issues(state: web::Data<AppState>) -> Result<HttpResponse, AppError> {
    let rows = sqlx::query_as::<_, IssueSummary>(
        r#"
        SELECT
            code,
            name,
            (SELECT COUNT(*) FROM bundles b WHERE b.issue_code = issues.code AND b.deleted_at IS NULL) AS bundle_count
        FROM issues
        WHERE status = 'ACTIVE'
        ORDER BY code DESC
        LIMIT 200
        "#,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;

    Ok(HttpResponse::Ok().json(rows))
}

#[derive(Debug, Deserialize)]
pub struct CreateIssueRequest {
    pub code: String,
    pub name: Option<String>,
}

#[post("/issues")]
pub async fn create_issue(
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
        INSERT INTO issues (code, name)
        VALUES (?, ?)
        ON CONFLICT(code) DO NOTHING
        "#,
    )
    .bind(&code)
    .bind(&name)
    .execute(&state.pool)
    .await
    .map_err(AppError::Database)?;

    if result.rows_affected() == 0 {
        return Err(AppError::Conflict(format!("issue {code} already exists")));
    }

    Ok(HttpResponse::Created().json(IssueSummary {
        code,
        name,
        bundle_count: 0,
    }))
}

#[get("/issues/{issue_id}")]
pub async fn get_issue_bundles(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let issue_code = normalize_issue_code(&path.into_inner())?;
    let issue = sqlx::query_as::<_, IssueRow>(
        "SELECT code, name FROM issues WHERE code = ? AND status = 'ACTIVE' LIMIT 1",
    )
    .bind(&issue_code)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("issue {issue_code}")))?;

    let rows = sqlx::query_as::<_, BundleRow>(
        "SELECT hash, name, status, process_stage, failure_stage, failure_code, failure_reason, retryable, size_bytes FROM bundles WHERE issue_code = ? AND deleted_at IS NULL ORDER BY created_at DESC",
    )
    .bind(&issue.code)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;

    let response = IssueBundlesResponse {
        name: issue.name,
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

    Ok(HttpResponse::Ok().json(response))
}

pub async fn require_issue_exists(pool: &sqlx::SqlitePool, code: &str) -> Result<String, AppError> {
    let code = normalize_issue_code(code)?;
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM issues
            WHERE code = ? AND status = 'ACTIVE'
        )
        "#,
    )
    .bind(&code)
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;

    if !exists {
        return Err(AppError::NotFound(format!("issue {code}")));
    }

    Ok(code)
}

#[derive(FromRow, Deserialize)]
struct IssueRow {
    code: String,
    name: String,
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
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let (issue_code, bundle_hash) = path.into_inner();
    let issue_code = normalize_issue_code(&issue_code)?;
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
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("bundle {bundle_hash}")))?;
    reject_processing_bundle(&bundle)?;
    sqlx::query(
        "UPDATE bundles SET status = 'DELETING', deleted_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(&bundle.id)
    .execute(&state.pool)
    .await
    .map_err(AppError::Database)?;
    let pool = state.pool.clone();
    let bundle_id = bundle.id.clone();
    tokio::spawn(async move {
        if let Err(error) = crate::db::finish_bundle_deletion(&pool, &bundle_id).await {
            tracing::error!(bundle_id, %error, "background bundle deletion failed; it will be retried at startup");
        }
    });

    Ok(HttpResponse::NoContent().finish())
}

#[delete("/issues/{issue_id}")]
pub async fn delete_issue(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let issue_code = normalize_issue_code(&path.into_inner())?;
    let claimed =
        sqlx::query("UPDATE issues SET status = 'DELETING' WHERE code = ? AND status = 'ACTIVE'")
            .bind(&issue_code)
            .execute(&state.pool)
            .await
            .map_err(AppError::Database)?
            .rows_affected();
    let newly_claimed = claimed == 1;
    if claimed == 0 {
        let status: Option<String> = sqlx::query_scalar("SELECT status FROM issues WHERE code = ?")
            .bind(&issue_code)
            .fetch_optional(&state.pool)
            .await
            .map_err(AppError::Database)?;
        match status.as_deref() {
            None => return Ok(HttpResponse::NoContent().finish()),
            Some("DELETING") => {} // Resume a previous synchronous deletion attempt.
            Some(status) => {
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
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;

    if bundles.is_empty() {
        sqlx::query("DELETE FROM issues WHERE code = ?")
            .bind(&issue_code)
            .execute(&state.pool)
            .await
            .map_err(AppError::Database)?;
        return Ok(HttpResponse::NoContent().finish());
    }

    if let Err(error) = reject_processing_bundles(&bundles) {
        if newly_claimed {
            sqlx::query(
                "UPDATE issues SET status = 'ACTIVE' WHERE code = ? AND status = 'DELETING'",
            )
            .bind(&issue_code)
            .execute(&state.pool)
            .await
            .map_err(AppError::Database)?;
        }
        return Err(error);
    }

    for bundle in &bundles {
        if bundle.status == "DELETED" {
            continue;
        }
        if bundle.status != "DELETING" {
            sqlx::query(
                "UPDATE bundles SET status = 'DELETING', deleted_at = CURRENT_TIMESTAMP WHERE id = ?",
            )
            .bind(&bundle.id)
            .execute(&state.pool)
            .await
            .map_err(AppError::Database)?;
        }
        finish_bundle_deletion(&state.pool, &bundle.id).await?;
    }

    sqlx::query("DELETE FROM issues WHERE code = ?")
        .bind(&issue_code)
        .execute(&state.pool)
        .await
        .map_err(AppError::Database)?;

    Ok(HttpResponse::NoContent().finish())
}

fn reject_processing_bundle(bundle: &BundleIdRow) -> Result<(), AppError> {
    if is_active_bundle_status(&bundle.status) {
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

fn is_active_bundle_status(status: &str) -> bool {
    matches!(
        status.to_ascii_uppercase().as_str(),
        "PENDING" | "PROCESSING" | "DELETING"
    )
}
