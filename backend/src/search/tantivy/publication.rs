use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use sqlx::SqlitePool;
use tokio::{fs, sync::Mutex};

use crate::{
    error::AppError,
    search::publication::{SearchBackendKind, artifact_relative_path, mark_publication_ready},
    search::{
        IndexBatch, IndexChunk, IngestIndex,
        resource::{SearchResourceBudget, SearchResourcePermit},
    },
};

use super::{
    pipeline::{BoundedBundlePipeline, PipelineConfig},
    writer::{IndexedChunk, open_committed},
};

const SPARSE_METADATA_COMMIT_CHUNKS: usize = 512;

/// Ingest-time Tantivy builder. Cleaned chunks are sent directly to the
/// bounded writer while SQLite receives only sparse navigation metadata.
pub struct BundleBuildSession {
    pool: SqlitePool,
    bundle_id: String,
    generation: i64,
    staging: PathBuf,
    final_path: PathBuf,
    pipeline: Mutex<Option<BoundedBundlePipeline>>,
    pending_metadata: Mutex<Vec<IndexBatch>>,
    expected_documents: AtomicU64,
    published: std::sync::atomic::AtomicBool,
    metric_emitted: AtomicBool,
    budget: SearchResourceBudget,
    admission_wait: Duration,
    build_started: Instant,
    _resource_permit: SearchResourcePermit,
}

impl BundleBuildSession {
    pub async fn start(
        pool: &SqlitePool,
        data_root: &Path,
        temp_dir: &Path,
        bundle_id: &str,
        generation: i64,
        budget: SearchResourceBudget,
    ) -> Result<Arc<Self>, AppError> {
        let relative = artifact_relative_path(bundle_id, generation)?;
        let staging = temp_dir
            .join("search")
            .join(bundle_id)
            .join(generation.to_string());
        let final_path = data_root.join(&relative);
        if fs::try_exists(&staging).await.map_err(AppError::Io)? {
            fs::remove_dir_all(&staging).await.map_err(AppError::Io)?;
        }
        if fs::try_exists(&final_path).await.map_err(AppError::Io)? {
            fs::remove_dir_all(&final_path)
                .await
                .map_err(AppError::Io)?;
        }
        let resource_permit = budget.acquire().await?;
        let admission_wait = resource_permit.queue_wait();
        let pipeline = BoundedBundlePipeline::start(
            staging.clone(),
            PipelineConfig {
                writer_heap_size_bytes: budget.writer_heap_size_bytes(),
                ..PipelineConfig::default()
            },
        )?;
        Ok(Arc::new(Self {
            pool: pool.clone(),
            bundle_id: bundle_id.to_owned(),
            generation,
            staging,
            final_path,
            pipeline: Mutex::new(Some(pipeline)),
            pending_metadata: Mutex::new(Vec::new()),
            expected_documents: AtomicU64::new(0),
            published: std::sync::atomic::AtomicBool::new(false),
            metric_emitted: AtomicBool::new(false),
            budget,
            admission_wait,
            build_started: Instant::now(),
            _resource_permit: resource_permit,
        }))
    }

    pub async fn finish(&self) -> Result<(), AppError> {
        let result = self.finish_inner().await;
        self.emit_metric(if result.is_ok() { "success" } else { "error" });
        result
    }

    async fn finish_inner(&self) -> Result<(), AppError> {
        let pipeline =
            self.pipeline.lock().await.take().ok_or_else(|| {
                AppError::Conflict("Tantivy build session is already closed".into())
            })?;
        let committed = pipeline.finish().await?;
        let pending = self
            .pending_metadata
            .lock()
            .await
            .drain(..)
            .collect::<Vec<_>>();
        if !pending.is_empty() {
            crate::ingest::persist_sparse_index_batches(&self.pool, &pending).await?;
        }
        let expected = self.expected_documents.load(Ordering::Acquire);
        if committed.document_count != expected {
            return Err(AppError::Config(format!(
                "Tantivy document count mismatch: expected {expected}, got {}",
                committed.document_count
            )));
        }
        drop(committed);
        if let Some(parent) = self.final_path.parent() {
            fs::create_dir_all(parent).await.map_err(AppError::Io)?;
        }
        fs::rename(&self.staging, &self.final_path)
            .await
            .map_err(AppError::Io)?;
        let verify_path = self.final_path.clone();
        let verified = match tokio::task::spawn_blocking(move || open_committed(verify_path)).await
        {
            Ok(result) => match result {
                Ok(index) => index,
                Err(error) => {
                    remove_published_artifact(&self.final_path).await;
                    return Err(error);
                }
            },
            Err(error) => {
                remove_published_artifact(&self.final_path).await;
                return Err(AppError::Config(format!(
                    "Tantivy verification task failed: {error}"
                )));
            }
        };
        if verified.document_count != expected {
            let actual = verified.document_count;
            drop(verified);
            remove_published_artifact(&self.final_path).await;
            return Err(AppError::Config(format!(
                "Tantivy verification count mismatch: expected {expected}, got {actual}"
            )));
        }
        drop(verified);
        if let Err(error) = mark_publication_ready(
            &self.pool,
            &self.bundle_id,
            SearchBackendKind::Tantivy,
            self.generation,
        )
        .await
        {
            remove_published_artifact(&self.final_path).await;
            return Err(error);
        }
        self.published.store(true, Ordering::Release);
        Ok(())
    }

    fn emit_metric(&self, outcome: &'static str) {
        if self
            .metric_emitted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            tracing::info!(
                metric = "tantivy_index_build",
                bundle_id = %self.bundle_id,
                outcome,
                admission_wait_ms = self.admission_wait.as_millis() as u64,
                build_elapsed_ms = self.build_started.elapsed().as_millis() as u64,
                active_writers = self.budget.active_writers(),
                queued_writers = self.budget.queued_writers(),
                writer_heap_size_bytes = self.budget.writer_heap_size_bytes(),
                "Tantivy Bundle index build completed"
            );
        }
    }

    pub async fn abort(&self) {
        if self.published.load(Ordering::Acquire) {
            return;
        }
        self.pipeline.lock().await.take();
        for path in [&self.staging, &self.final_path] {
            match fs::remove_dir_all(path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "failed to remove aborted Tantivy artifact")
                }
            }
        }
        self.emit_metric("cancelled");
    }
}

#[async_trait]
impl IngestIndex for BundleBuildSession {
    async fn commit_ingest_batch(&self, batch: IndexBatch) -> Result<(), AppError> {
        if batch.bundle_id != self.bundle_id {
            return Err(AppError::Config(
                "Tantivy batch belongs to a different Bundle".into(),
            ));
        }
        let indexed = batch
            .chunks
            .iter()
            .map(|chunk| IndexedChunk {
                file_id: batch.file_id,
                chunk_index: chunk.chunk_index,
                line_start: chunk.line_start,
                line_end: chunk.line_end,
                event_time_start_ms: chunk.event_time_start_ms,
                event_time_end_ms: chunk.event_time_end_ms,
                timeline: Some("all".into()),
                content: chunk.content.clone(),
                path: batch.path.clone(),
            })
            .collect::<Vec<_>>();
        let count = indexed.len() as u64;
        {
            let pipeline = self.pipeline.lock().await;
            let Some(pipeline) = pipeline.as_ref() else {
                return Err(AppError::Conflict(
                    "Tantivy build session is already closed".into(),
                ));
            };
            pipeline.submit(indexed).await?;
        }
        let metadata = IndexBatch {
            bundle_id: batch.bundle_id,
            file_id: batch.file_id,
            path: batch.path,
            chunks: batch
                .chunks
                .into_iter()
                .map(|chunk| IndexChunk {
                    chunk_index: chunk.chunk_index,
                    line_start: chunk.line_start,
                    line_end: chunk.line_end,
                    event_time_start_ms: chunk.event_time_start_ms,
                    event_time_end_ms: chunk.event_time_end_ms,
                    content: String::new(),
                })
                .collect(),
            offsets: batch.offsets,
            final_line_count: batch.final_line_count,
        };
        let flush = {
            let mut pending = self.pending_metadata.lock().await;
            pending.push(metadata);
            let chunk_count = pending
                .iter()
                .map(|batch| batch.chunks.len())
                .sum::<usize>();
            (chunk_count >= SPARSE_METADATA_COMMIT_CHUNKS)
                .then(|| pending.drain(..).collect::<Vec<_>>())
        };
        if let Some(flush) = flush {
            crate::ingest::persist_sparse_index_batches(&self.pool, &flush).await?;
        }
        self.expected_documents.fetch_add(count, Ordering::AcqRel);
        Ok(())
    }
}

async fn remove_published_artifact(path: &Path) {
    match fs::remove_dir_all(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "failed to remove unpublished Tantivy artifact")
        }
    }
}
