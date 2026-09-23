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
    ingest::{
        ArchiveBudget, IssueQuota, PreflightFileOptions, ProcessFileOptions,
        preflight_uploaded_file, process_uploaded_file,
    },
    search::publication::SearchBackendKind,
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
    /// Captured immediately after multipart receipt, before DB reservation finalization.
    pub received_at: Instant,
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
    pub search_backend: SearchBackendKind,
    pub search_resource_budget: crate::search::resource::SearchResourceBudget,
}

pub fn spawn_upload_job(job: UploadJob) {
    // Start before scheduling so enqueue-to-READY includes executor delay.
    let queued_at = Instant::now();
    tokio::spawn(async move {
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
            processing_queue_ms = queued_at.elapsed().as_millis() as u64,
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
    let generation = claim_search_publication(job).await?;
    let search_build = match start_search_build(job, generation).await {
        Ok(build) => build,
        Err(error) => {
            if let Some(generation) = generation {
                crate::search::publication::mark_publication_failed(
                    &job.pool,
                    &job.bundle_id,
                    SearchBackendKind::Tantivy,
                    generation,
                    "BUILD_FAILED",
                )
                .await;
            }
            return Err(error);
        }
    };
    let result =
        process_upload_files_and_publish(job, generation, clone_search_build(&search_build)).await;
    if let Err(error) = &result
        && let Some(generation) = generation
    {
        abort_search_build(&search_build).await;
        crate::search::publication::mark_publication_failed(
            &job.pool,
            &job.bundle_id,
            SearchBackendKind::Tantivy,
            generation,
            "BUILD_FAILED",
        )
        .await;
        tracing::warn!(bundle_id = %job.bundle_id, generation, %error, "Tantivy publication failed");
    }
    result
}

async fn claim_search_publication(job: &UploadJob) -> Result<Option<i64>, AppError> {
    if job.search_backend == SearchBackendKind::SqliteFts {
        return Ok(None);
    }
    #[cfg(feature = "tantivy-search")]
    {
        return crate::search::publication::claim_publication(
            &job.pool,
            &job.bundle_id,
            SearchBackendKind::Tantivy,
        )
        .await
        .map(Some);
    }
    #[cfg(not(feature = "tantivy-search"))]
    {
        Err(AppError::Config(
            "Tantivy backend requires the tantivy-search feature".into(),
        ))
    }
}

async fn process_upload_files_and_publish(
    job: &UploadJob,
    generation: Option<i64>,
    search_build: Option<SearchBuild>,
) -> Result<(), AppError> {
    let archive_budget = ArchiveBudget::new(job.archive_config.clone())
        .with_temp_budget(job.receive_reservation.temp_budget());
    let issue_quota = IssueQuota::new(
        job.pool.clone(),
        &job.issue_code,
        &job.bundle_id,
        job.issue_max_content_size,
    );

    crate::upload::lifecycle::set_bundle_stage(&job.pool, &job.bundle_id, "VALIDATING").await?;
    let preflight_started = Instant::now();
    for uploaded in &job.files {
        preflight_uploaded_file(PreflightFileOptions {
            pool: &job.pool,
            bundle_id: &job.bundle_id,
            bundle_hash: &job.bundle_hash,
            data_root: &job.staging_root,
            storage_name: &uploaded.storage_name,
            original_name: &uploaded.original_name,
            content_type: uploaded.content_type.as_deref(),
            source_path: &uploaded.temp_path,
            size_bytes: uploaded.size_bytes,
            archive_budget: archive_budget.clone(),
            issue_quota: issue_quota.clone(),
        })
        .await?;
    }
    info!(
        metric = "upload_preflight",
        request_id = job.request_id.as_deref().unwrap_or("unavailable"),
        bundle_id = %job.bundle_id,
        file_count = job.files.len(),
        received_bytes = job
            .files
            .iter()
            .fold(0_u64, |total, file| total.saturating_add(file.size_bytes)),
        preflight_elapsed_ms = preflight_started.elapsed().as_millis() as u64,
        active_writers = job.search_resource_budget.active_writers(),
        queued_writers = job.search_resource_budget.queued_writers(),
        "uploaded bundle preflight completed"
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
            search_index: search_build_to_index(&search_build),
            preflighted: true,
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

    if let Some(generation) = generation {
        finish_search_build(&search_build, job, generation).await?;
    }
    finalize_bundle_ready_with_retry(&job.pool, &job.bundle_id).await?;
    info!(
        metric = "upload_to_ready",
        bundle_id = %job.bundle_id,
        elapsed_us = crate::ingest::metrics::micros(job.received_at.elapsed()),
        "received upload became READY"
    );
    let _ = fs::remove_dir_all(job.staging_root.join(&job.bundle_hash)).await;
    Ok(())
}

#[cfg(feature = "tantivy-search")]
type SearchBuild = Arc<crate::search::tantivy::publication::BundleBuildSession>;

#[cfg(not(feature = "tantivy-search"))]
type SearchBuild = ();

async fn start_search_build(
    _job: &UploadJob,
    generation: Option<i64>,
) -> Result<Option<SearchBuild>, AppError> {
    #[cfg(feature = "tantivy-search")]
    if let Some(generation) = generation {
        return crate::search::tantivy::publication::BundleBuildSession::start(
            &_job.pool,
            &_job.data_root,
            &_job.temp_dir,
            &_job.bundle_id,
            generation,
            _job.search_resource_budget.clone(),
        )
        .await
        .map(Some);
    }
    #[cfg(not(feature = "tantivy-search"))]
    let _ = generation;
    Ok(None)
}

fn search_build_to_index(
    search_build: &Option<SearchBuild>,
) -> Option<std::sync::Arc<dyn crate::search::IngestIndex>> {
    #[cfg(feature = "tantivy-search")]
    {
        search_build
            .as_ref()
            .map(|build| build.clone() as std::sync::Arc<dyn crate::search::IngestIndex>)
    }
    #[cfg(not(feature = "tantivy-search"))]
    {
        let _ = search_build;
        None
    }
}

fn clone_search_build(search_build: &Option<SearchBuild>) -> Option<SearchBuild> {
    #[cfg(feature = "tantivy-search")]
    {
        search_build.clone()
    }
    #[cfg(not(feature = "tantivy-search"))]
    {
        *search_build
    }
}

async fn abort_search_build(_search_build: &Option<SearchBuild>) {
    #[cfg(feature = "tantivy-search")]
    if let Some(build) = _search_build {
        build.abort().await;
    }
}

async fn finish_search_build(
    search_build: &Option<SearchBuild>,
    _job: &UploadJob,
    _generation: i64,
) -> Result<(), AppError> {
    #[cfg(feature = "tantivy-search")]
    if let Some(build) = search_build {
        return build.finish().await;
    }
    #[cfg(not(feature = "tantivy-search"))]
    {
        let _ = search_build;
    }
    Err(AppError::Config(
        "Tantivy backend requires the tantivy-search feature".into(),
    ))
}
