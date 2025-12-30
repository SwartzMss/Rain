use actix_web::{HttpResponse, get, web};
use serde::Deserialize;
use sqlx::FromRow;

use crate::{
    AppState,
    error::AppError,
    models::issues::{IssueBundlesResponse, IssueSummary, UploadStatus, UploadStatusWrapper},
};

// scoped under /api in routes::register, so keep relative paths here
#[get("/issues")]
pub async fn list_issues(state: web::Data<AppState>) -> Result<HttpResponse, AppError> {
    let rows = sqlx::query_as::<_, IssueSummary>(
        r#"
        SELECT
            code,
            name,
            (SELECT COUNT(*) FROM bundles b WHERE b.issue_code = issues.code) AS bundle_count
        FROM issues
        ORDER BY code DESC
        LIMIT 200
        "#,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;

    Ok(HttpResponse::Ok().json(rows))
}

#[get("/issues/{issue_id}")]
pub async fn get_issue_bundles(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let issue_code = path.into_inner();
    let issue =
        sqlx::query_as::<_, IssueRow>("SELECT code, name FROM issues WHERE code = $1 LIMIT 1")
            .bind(&issue_code)
            .fetch_optional(&state.pool)
            .await
            .map_err(AppError::Database)?
            .ok_or_else(|| AppError::NotFound(format!("issue {issue_code}")))?;

    let rows = sqlx::query_as::<_, BundleRow>(
        "SELECT hash, name, status::text AS status FROM bundles WHERE issue_code = $1 ORDER BY created_at DESC",
    )
    .bind(&issue.code)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;

    let response = IssueBundlesResponse {
        name: issue.name,
        log_bundles: rows
            .into_iter()
            .map(|bundle| crate::models::issues::UploadSummary {
                hash: bundle.hash,
                name: bundle.name,
                status: UploadStatusWrapper {
                    upload_status: UploadStatus::from_db_value(&bundle.status),
                },
            })
            .collect(),
    };

    Ok(HttpResponse::Ok().json(response))
}

pub async fn ensure_issue(pool: &sqlx::PgPool, code: &str) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO issues (code, name)
        VALUES ($1, $1)
        ON CONFLICT (code) DO NOTHING
        "#,
    )
    .bind(code)
    .execute(pool)
    .await
    .map_err(AppError::Database)?;
    Ok(())
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
}
