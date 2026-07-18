use actix_multipart::Multipart;
use actix_web::{HttpResponse, get, http::StatusCode, post, web};
use serde::Serialize;
use tokio::fs;
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    models::issues::{UploadStage, UploadStatus},
    upload::{
        job::{UploadJob, spawn_upload_job},
        lifecycle::create_processing_bundle,
        multipart::collect_multipart_upload,
    },
};

use super::issues::{normalize_issue_code, require_issue_exists};

// scoped under /api in routes::register, so use relative path
#[post("/issues/{issue_code}/uploads")]
pub async fn upload_logs(
    state: web::Data<AppState>,
    path: web::Path<String>,
    payload: Multipart,
) -> Result<HttpResponse, AppError> {
    let issue_code = normalize_issue_code(&path.into_inner())?;
    require_issue_exists(&state.pool, &issue_code).await?;

    let upload_id = Uuid::new_v4().simple().to_string();
    let temp_dir = state.data_root.join(".tmp").join(&upload_id);
    fs::create_dir_all(&temp_dir).await.map_err(AppError::Io)?;

    let upload = match collect_multipart_upload(payload, &temp_dir).await {
        Ok(upload) => upload,
        Err(error) => {
            let _ = fs::remove_dir_all(&temp_dir).await;
            return Err(error);
        }
    };

    let bundle_hash = Uuid::new_v4().simple().to_string();
    let bundle_name = if upload.files.len() == 1 {
        upload.files[0].display_name.clone()
    } else {
        format!(
            "{} 等 {} 个文件",
            upload.files[0].display_name,
            upload.files.len()
        )
    };

    let bundle_id = Uuid::new_v4().simple().to_string();

    if let Err(error) = create_processing_bundle(
        &state.pool,
        &bundle_id,
        &issue_code,
        &bundle_hash,
        &bundle_name,
        upload.total_bytes,
    )
    .await
    {
        let _ = fs::remove_dir_all(&temp_dir).await;
        return Err(error);
    }

    let file_count = upload.files.len() as u64;
    let staging_root = temp_dir.join("staging");
    spawn_upload_job(UploadJob {
        pool: state.pool.clone(),
        data_root: state.data_root.clone(),
        blob_store: state.blob_store.clone(),
        temp_dir,
        staging_root,
        processing_permits: state.processing_permits.clone(),
        archive_config: crate::config::ArchiveConfig::for_content_limit(
            state.limits.issue_max_content_size,
        ),
        indexing_config: state.limits.indexing.clone(),
        issue_code: issue_code.clone(),
        issue_max_content_size: state.limits.issue_max_content_size,
        bundle_id: bundle_id.clone(),
        bundle_hash: bundle_hash.clone(),
        files: upload.files,
    });

    Ok(
        HttpResponse::build(StatusCode::ACCEPTED).json(UploadResponse {
            task_id: bundle_hash.clone(),
            issue_code,
            bundle_hash,
            status: UploadStatus::Processing,
            stage: UploadStage::Receiving,
            file_count,
            total_bytes: upload.total_bytes,
        }),
    )
}

#[derive(Serialize)]
struct UploadResponse {
    task_id: String,
    issue_code: String,
    bundle_hash: String,
    status: UploadStatus,
    stage: UploadStage,
    file_count: u64,
    total_bytes: u64,
}

#[get("/uploads/{task_id}")]
pub async fn get_upload_task(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let task_id = path.into_inner();
    let row = sqlx::query_as::<_, UploadTaskRow>(
        r#"
        SELECT issue_code, hash, status, process_stage, failure_stage, failure_code,
               failure_reason, retryable, size_bytes
        FROM bundles
        WHERE hash = ? AND deleted_at IS NULL
        LIMIT 1
        "#,
    )
    .bind(&task_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("upload task {task_id}")))?;

    let status = UploadStatus::from_db_value(&row.status);
    let progress_percent = match status {
        UploadStatus::Ready | UploadStatus::Failed => 100,
        UploadStatus::Processing => 0,
        UploadStatus::Pending => 0,
    };

    Ok(HttpResponse::Ok().json(UploadTaskResponse {
        task_id: row.hash.clone(),
        issue_code: row.issue_code,
        bundle_hash: row.hash,
        status,
        stage: match status {
            UploadStatus::Ready => UploadStage::Ready,
            UploadStatus::Failed => UploadStage::Failed,
            _ => UploadStage::from_db_value(&row.process_stage),
        },
        failure_reason: row.failure_reason,
        failure_stage: row.failure_stage,
        failure_code: row.failure_code,
        retryable: row.retryable,
        progress_percent,
        total_bytes: row.size_bytes.unwrap_or(0).max(0) as u64,
    }))
}

#[derive(sqlx::FromRow)]
struct UploadTaskRow {
    issue_code: String,
    hash: String,
    status: String,
    process_stage: String,
    failure_reason: Option<String>,
    failure_stage: Option<String>,
    failure_code: Option<String>,
    retryable: Option<bool>,
    size_bytes: Option<i64>,
}

#[derive(Serialize)]
struct UploadTaskResponse {
    task_id: String,
    issue_code: String,
    bundle_hash: String,
    status: UploadStatus,
    stage: UploadStage,
    failure_reason: Option<String>,
    failure_stage: Option<String>,
    failure_code: Option<String>,
    retryable: Option<bool>,
    progress_percent: u8,
    total_bytes: u64,
}
