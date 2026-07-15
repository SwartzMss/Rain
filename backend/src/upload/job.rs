use std::{path::PathBuf, sync::Arc};

use tokio::{fs, sync::Semaphore};
use tracing::error;

use crate::{
    config::{ArchiveConfig, IndexingConfig},
    error::AppError,
    ingest::{ArchiveBudget, EventBudget, ProcessFileOptions, process_uploaded_file},
};

use super::{
    finalizer::{
        finalize_bundle_failed, finalize_bundle_ready_with_retry, move_bundle_directory_with_retry,
    },
    multipart::UploadedFile,
};

pub struct UploadJob {
    pub pool: sqlx::SqlitePool,
    pub data_root: PathBuf,
    pub temp_dir: PathBuf,
    pub staging_root: PathBuf,
    pub processing_permits: Arc<Semaphore>,
    pub archive_config: ArchiveConfig,
    pub indexing_config: IndexingConfig,
    pub bundle_id: String,
    pub bundle_hash: String,
    pub files: Vec<UploadedFile>,
}

pub fn spawn_upload_job(job: UploadJob) {
    tokio::spawn(async move {
        let _permit = match job.processing_permits.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(error) => {
                error!(
                    bundle_id = %job.bundle_id,
                    bundle_hash = %job.bundle_hash,
                    error = %error,
                    "failed to acquire upload processing permit"
                );
                finalize_bundle_failed(
                    &job.pool,
                    &job.bundle_id,
                    &job.data_root,
                    &job.staging_root,
                    &job.bundle_hash,
                    &AppError::Conflict("上传处理任务已停止".into()),
                )
                .await;
                let _ = fs::remove_dir_all(&job.temp_dir).await;
                return;
            }
        };

        let process_result = process_upload_job(&job).await;

        if let Err(error) = process_result {
            error!(
                bundle_id = %job.bundle_id,
                bundle_hash = %job.bundle_hash,
                error = %error,
                "failed to process uploaded log bundle"
            );
            finalize_bundle_failed(
                &job.pool,
                &job.bundle_id,
                &job.data_root,
                &job.staging_root,
                &job.bundle_hash,
                &error,
            )
            .await;
        }

        let _ = fs::remove_dir_all(&job.temp_dir).await;
    });
}

async fn process_upload_job(job: &UploadJob) -> Result<(), AppError> {
    let archive_budget = ArchiveBudget::new(job.archive_config.clone());
    let event_budget = EventBudget::new(job.indexing_config.max_events_per_bundle);
    for uploaded in &job.files {
        process_uploaded_file(ProcessFileOptions {
            pool: &job.pool,
            bundle_id: &job.bundle_id,
            bundle_hash: &job.bundle_hash,
            data_root: &job.staging_root,
            storage_name: &uploaded.storage_name,
            original_name: &uploaded.original_name,
            display_name: &uploaded.display_name,
            content_type: uploaded.content_type.as_deref(),
            source_path: &uploaded.temp_path,
            size_bytes: uploaded.size_bytes,
            archive_budget: archive_budget.clone(),
            event_budget: event_budget.clone(),
            indexing: &job.indexing_config,
        })
        .await?;
    }

    let staging_bundle_dir = job.staging_root.join(&job.bundle_hash);
    let final_bundle_dir = job.data_root.join(&job.bundle_hash);
    if fs::metadata(&final_bundle_dir).await.is_ok() {
        return Err(AppError::BadRequest(format!(
            "bundle directory already exists: {}",
            final_bundle_dir.display()
        )));
    }
    if let Some(parent) = final_bundle_dir.parent() {
        fs::create_dir_all(parent).await.map_err(AppError::Io)?;
    }
    move_bundle_directory_with_retry(&staging_bundle_dir, &final_bundle_dir).await?;
    finalize_bundle_ready_with_retry(
        &job.pool,
        &job.bundle_id,
        &staging_bundle_dir,
        &final_bundle_dir,
    )
    .await?;
    Ok(())
}
