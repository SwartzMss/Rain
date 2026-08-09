use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

use tokio::{fs, sync::Semaphore};
use tracing::{debug, error, info};

use crate::{
    blob_store::BlobStore,
    config::{ArchiveConfig, IndexingConfig},
    error::AppError,
    ingest::{ArchiveBudget, IssueQuota, ProcessFileOptions, process_uploaded_file},
};

use super::{
    finalizer::{finalize_bundle_failed, finalize_bundle_ready_with_retry},
    multipart::{ReceiveReservation, UploadedFile},
};

struct PendingTempCleanup {
    path: PathBuf,
    reservation: ReceiveReservation,
}

#[derive(Clone, Default)]
pub struct TempCleanupQueue(Arc<Mutex<VecDeque<PendingTempCleanup>>>);

impl TempCleanupQueue {
    pub fn enqueue(&self, path: PathBuf, reservation: ReceiveReservation) {
        if let Ok(mut pending) = self.0.lock() {
            pending.push_back(PendingTempCleanup { path, reservation });
        } else {
            std::mem::forget(reservation);
        }
    }
}

pub fn spawn_temp_cleanup_worker(queue: TempCleanupQueue) -> tokio::task::JoinHandle<()> {
    crate::spawn_periodic_job(
        "temporary-upload-cleanup",
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(30),
        move || {
            let queue = queue.clone();
            async move {
                let pending = queue
                    .0
                    .lock()
                    .map(|mut items| items.drain(..).collect::<Vec<_>>())
                    .unwrap_or_default();
                for item in pending {
                    match fs::remove_dir_all(&item.path).await {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => {
                            tracing::warn!(path = %item.path.display(), error = %error, "temporary upload cleanup retry failed");
                            queue.enqueue(item.path, item.reservation);
                        }
                    }
                }
                Ok(())
            }
        },
    )
}

pub struct UploadJob {
    pub pool: sqlx::SqlitePool,
    pub data_root: PathBuf,
    pub blob_store: Arc<dyn BlobStore>,
    pub temp_dir: PathBuf,
    pub staging_root: PathBuf,
    pub processing_permits: Arc<Semaphore>,
    pub archive_config: ArchiveConfig,
    pub indexing_config: IndexingConfig,
    pub request_id: Option<String>,
    pub issue_code: String,
    pub issue_max_content_size: u64,
    pub bundle_id: String,
    pub bundle_hash: String,
    pub files: Vec<UploadedFile>,
    pub receive_reservation: ReceiveReservation,
    pub temp_cleanup_queue: TempCleanupQueue,
}

pub fn spawn_upload_job(job: UploadJob) {
    tokio::spawn(async move {
        let queued_at = Instant::now();
        let file_count = job.files.len();
        let received_bytes = job
            .files
            .iter()
            .fold(0_u64, |total, file| total.saturating_add(file.size_bytes));
        let _permit = match job.processing_permits.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(error) => {
                error!(
                    request_id = job.request_id.as_deref().unwrap_or("unavailable"),
                    bundle_id = %job.bundle_id,
                    bundle_hash = %job.bundle_hash,
                    file_count,
                    received_bytes,
                    queue_elapsed_ms = queued_at.elapsed().as_millis() as u64,
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
                if let Err(cleanup_error) = fs::remove_dir_all(&job.temp_dir).await {
                    error!(
                        request_id = job.request_id.as_deref().unwrap_or("unavailable"),
                        bundle_id = %job.bundle_id,
                        path = %job.temp_dir.display(),
                        error = %cleanup_error,
                        "failed to remove temporary upload directory; retaining budget reservation"
                    );
                    job.temp_cleanup_queue
                        .enqueue(job.temp_dir.clone(), job.receive_reservation);
                }
                return;
            }
        };

        let processing_started = Instant::now();
        info!(
            request_id = job.request_id.as_deref().unwrap_or("unavailable"),
            bundle_id = %job.bundle_id,
            bundle_hash = %job.bundle_hash,
            file_count,
            received_bytes,
            queue_elapsed_ms = queued_at.elapsed().as_millis() as u64,
            "upload processing started"
        );
        let process_result = process_upload_job(&job).await;

        match process_result {
            Ok(()) => info!(
                request_id = job.request_id.as_deref().unwrap_or("unavailable"),
                bundle_id = %job.bundle_id,
                bundle_hash = %job.bundle_hash,
                file_count,
                received_bytes,
                elapsed_ms = processing_started.elapsed().as_millis() as u64,
                "upload processing completed"
            ),
            Err(error) => {
                error!(
                    request_id = job.request_id.as_deref().unwrap_or("unavailable"),
                    bundle_id = %job.bundle_id,
                    bundle_hash = %job.bundle_hash,
                    file_count,
                    received_bytes,
                    elapsed_ms = processing_started.elapsed().as_millis() as u64,
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
        }

        if let Err(cleanup_error) = fs::remove_dir_all(&job.temp_dir).await {
            error!(
                request_id = job.request_id.as_deref().unwrap_or("unavailable"),
                bundle_id = %job.bundle_id,
                path = %job.temp_dir.display(),
                error = %cleanup_error,
                "failed to remove temporary upload directory; retaining budget reservation"
            );
            job.temp_cleanup_queue
                .enqueue(job.temp_dir.clone(), job.receive_reservation);
        }
    });
}

async fn process_upload_job(job: &UploadJob) -> Result<(), AppError> {
    let archive_budget = ArchiveBudget::new(job.archive_config.clone())
        .with_temp_budget(job.receive_reservation.temp_budget());
    let issue_quota = IssueQuota::new(
        job.pool.clone(),
        &job.issue_code,
        &job.bundle_id,
        job.issue_max_content_size,
    );
    for (file_index, uploaded) in job.files.iter().enumerate() {
        let file_started = Instant::now();
        debug!(
            request_id = job.request_id.as_deref().unwrap_or("unavailable"),
            bundle_id = %job.bundle_id,
            file_index,
            size_bytes = uploaded.size_bytes,
            "uploaded file processing started"
        );
        process_uploaded_file(ProcessFileOptions {
            pool: &job.pool,
            bundle_id: &job.bundle_id,
            bundle_hash: &job.bundle_hash,
            data_root: &job.staging_root,
            blob_store: job.blob_store.clone(),
            storage_name: &uploaded.storage_name,
            original_name: &uploaded.original_name,
            display_name: &uploaded.display_name,
            content_type: uploaded.content_type.as_deref(),
            source_path: &uploaded.temp_path,
            size_bytes: uploaded.size_bytes,
            archive_budget: archive_budget.clone(),
            issue_quota: issue_quota.clone(),
            indexing: &job.indexing_config,
        })
        .await?;
        debug!(
            request_id = job.request_id.as_deref().unwrap_or("unavailable"),
            bundle_id = %job.bundle_id,
            file_index,
            size_bytes = uploaded.size_bytes,
            elapsed_ms = file_started.elapsed().as_millis() as u64,
            "uploaded file processing completed"
        );
    }

    finalize_bundle_ready_with_retry(&job.pool, &job.bundle_id).await?;
    let _ = fs::remove_dir_all(job.staging_root.join(&job.bundle_hash)).await;
    Ok(())
}
