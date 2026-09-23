use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use sqlx::SqlitePool;
use tokio::{
    fs,
    sync::{Mutex, OnceCell},
};

use crate::{
    error::AppError,
    search::publication::{
        SearchBackendKind, artifact_relative_path, mark_publication_ready,
        refresh_publication_heartbeat,
    },
    search::{IndexBatch, IndexChunk, IngestIndex, resource::SearchResourceBudget},
};

use super::{
    pipeline::{BoundedBundlePipeline, PipelineConfig},
    writer::{IndexedChunk, open_committed},
};

const SPARSE_METADATA_COMMIT_CHUNKS: usize = 512;
const SPARSE_METADATA_COMMIT_BATCHES: usize = 512;

struct HeartbeatGuard(Option<tokio::task::JoinHandle<()>>);

impl HeartbeatGuard {
    fn new(task: tokio::task::JoinHandle<()>) -> Self {
        Self(Some(task))
    }

    fn take(&mut self) -> tokio::task::JoinHandle<()> {
        self.0.take().expect("publication heartbeat task missing")
    }
}

impl Drop for HeartbeatGuard {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

fn spawn_publication_heartbeat(
    pool: &SqlitePool,
    bundle_id: &str,
    generation: i64,
) -> tokio::task::JoinHandle<()> {
    let heartbeat_pool = pool.clone();
    let heartbeat_bundle_id = bundle_id.to_owned();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            match refresh_publication_heartbeat(&heartbeat_pool, &heartbeat_bundle_id, generation)
                .await
            {
                Ok(()) => {}
                Err(AppError::Conflict(_)) => break,
                Err(error) => {
                    tracing::warn!(
                        bundle_id = %heartbeat_bundle_id,
                        generation,
                        %error,
                        "failed to refresh Tantivy publication heartbeat"
                    );
                }
            }
        }
    })
}

/// Ingest-time Tantivy builder. Cleaned chunks are sent directly to the
/// bounded writer while SQLite receives only sparse navigation metadata.
pub struct BundleBuildSession {
    pool: SqlitePool,
    bundle_id: String,
    generation: i64,
    staging: PathBuf,
    final_path: PathBuf,
    pipeline: OnceCell<Arc<BoundedBundlePipeline>>,
    operation_lock: Mutex<()>,
    closing: AtomicBool,
    pending_metadata: Mutex<Vec<IndexBatch>>,
    expected_documents: AtomicU64,
    published: std::sync::atomic::AtomicBool,
    metric_emitted: AtomicBool,
    budget: SearchResourceBudget,
    admission_wait_micros: AtomicU64,
    writer_started: StdMutex<Option<Instant>>,
    writer_active_at_admission: AtomicU64,
    build_started: Instant,
    heartbeat_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
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
        let mut heartbeat =
            HeartbeatGuard::new(spawn_publication_heartbeat(pool, bundle_id, generation));
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
        let session = Arc::new(Self {
            pool: pool.clone(),
            bundle_id: bundle_id.to_owned(),
            generation,
            staging,
            final_path,
            pipeline: OnceCell::new(),
            operation_lock: Mutex::new(()),
            closing: AtomicBool::new(false),
            pending_metadata: Mutex::new(Vec::new()),
            expected_documents: AtomicU64::new(0),
            published: std::sync::atomic::AtomicBool::new(false),
            metric_emitted: AtomicBool::new(false),
            budget,
            admission_wait_micros: AtomicU64::new(0),
            writer_started: StdMutex::new(None),
            writer_active_at_admission: AtomicU64::new(0),
            build_started: Instant::now(),
            heartbeat_task: Mutex::new(None),
        });
        *session.heartbeat_task.lock().await = Some(heartbeat.take());
        Ok(session)
    }

    pub async fn finish(&self) -> Result<(), AppError> {
        let result = self.finish_inner().await;
        self.stop_heartbeat().await;
        self.emit_metric(if result.is_ok() { "success" } else { "error" });
        result
    }

    async fn stop_heartbeat(&self) {
        let heartbeat = self.heartbeat_task.lock().await.take();
        if let Some(heartbeat) = heartbeat {
            heartbeat.abort();
            let _ = heartbeat.await;
        }
    }

    async fn finish_inner(&self) -> Result<(), AppError> {
        let _operation = self.operation_lock.lock().await;
        if self.closing.load(Ordering::Acquire) {
            return Err(AppError::Conflict(
                "Tantivy build session is already closed".into(),
            ));
        }
        let pipeline = self.ensure_pipeline().await?;
        self.closing.store(true, Ordering::Release);
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

    async fn ensure_pipeline(&self) -> Result<Arc<BoundedBundlePipeline>, AppError> {
        if self.closing.load(Ordering::Acquire) && self.pipeline.get().is_none() {
            return Err(AppError::Conflict(
                "Tantivy build session is already closed".into(),
            ));
        }
        let budget = self.budget.clone();
        let staging = self.staging.clone();
        let closing = &self.closing;
        let admission_wait = &self.admission_wait_micros;
        let writer_started = &self.writer_started;
        let writer_active_at_admission = &self.writer_active_at_admission;
        let pipeline = self
            .pipeline
            .get_or_try_init(|| async move {
                let permit = budget.acquire().await?;
                if closing.load(Ordering::Acquire) {
                    drop(permit);
                    return Err(AppError::Conflict(
                        "Tantivy build session is already closed".into(),
                    ));
                }
                let waited = permit.queue_wait();
                let pipeline = BoundedBundlePipeline::start_with_permit(
                    staging,
                    PipelineConfig {
                        writer_heap_size_bytes: budget.writer_heap_size_bytes(),
                        ..PipelineConfig::default()
                    },
                    permit,
                )?;
                if let Ok(mut started) = writer_started.lock() {
                    *started = Some(Instant::now());
                }
                admission_wait.store(
                    waited.as_micros().min(u128::from(u64::MAX)) as u64,
                    Ordering::Release,
                );
                // The current count is captured at admission because the
                // completion metric is emitted after the permit is released.
                writer_active_at_admission.store(budget.active_writers() as u64, Ordering::Release);
                Ok(Arc::new(pipeline))
            })
            .await?;
        Ok(pipeline.clone())
    }

    fn emit_metric(&self, outcome: &'static str) {
        if self
            .metric_emitted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let writer_active_ms = self
                .writer_started
                .lock()
                .ok()
                .and_then(|started| started.map(|started| started.elapsed().as_millis() as u64))
                .unwrap_or(0);
            tracing::info!(
                metric = "tantivy_index_build",
                bundle_id = %self.bundle_id,
                outcome,
                admission_wait_ms = self.admission_wait_micros.load(Ordering::Acquire) / 1_000,
                writer_active_ms,
                indexing_elapsed_ms = writer_active_ms,
                build_elapsed_ms = self.build_started.elapsed().as_millis() as u64,
                active_writers = self.budget.active_writers(),
                writer_active_writers = self
                    .writer_active_at_admission
                    .load(Ordering::Acquire),
                queued_writers = self.budget.queued_writers(),
                writer_heap_size_bytes = self.budget.writer_heap_size_bytes(),
                "Tantivy Bundle index build completed"
            );
        }
    }

    pub async fn abort(&self) {
        self.closing.store(true, Ordering::Release);
        self.stop_heartbeat().await;
        let _operation = self.operation_lock.lock().await;
        if self.published.load(Ordering::Acquire) {
            return;
        }
        if let Some(pipeline) = self.pipeline.get() {
            let _ = pipeline.abort().await;
        }
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

impl Drop for BundleBuildSession {
    fn drop(&mut self) {
        if let Ok(mut heartbeat) = self.heartbeat_task.try_lock()
            && let Some(heartbeat) = heartbeat.take()
        {
            heartbeat.abort();
        }
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
        let _operation = self.operation_lock.lock().await;
        if self.closing.load(Ordering::Acquire) {
            return Err(AppError::Conflict(
                "Tantivy build session is already closed".into(),
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
        if !indexed.is_empty() {
            let pipeline = self.ensure_pipeline().await?;
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
            (chunk_count >= SPARSE_METADATA_COMMIT_CHUNKS
                || pending.len() >= SPARSE_METADATA_COMMIT_BATCHES)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db,
        search::publication::{SearchBackendKind, claim_publication},
    };

    fn fixture_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rain-tantivy-publication-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ))
    }

    async fn fixture(name: &str) -> (SqlitePool, PathBuf, i64) {
        let root = fixture_root(name);
        std::fs::create_dir_all(&root).unwrap();
        let pool = db::init_pool(&format!("sqlite://{}", root.join("rain.db").display())).unwrap();
        db::prepare_schema(&pool, false).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name) VALUES('PUBTEST','Publication test')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-pubtest','PUBTEST','hash','fixture','PROCESSING')")
            .execute(&pool)
            .await
            .unwrap();
        let generation = claim_publication(&pool, "bundle-pubtest", SearchBackendKind::Tantivy)
            .await
            .unwrap();
        (pool, root, generation)
    }

    #[tokio::test]
    async fn writer_admission_starts_at_first_searchable_batch() {
        let (pool, root, generation) = fixture("lazy").await;
        let budget = SearchResourceBudget::new(1, 16 * 1024 * 1024).unwrap();
        let session = BundleBuildSession::start(
            &pool,
            &root,
            &root.join(".tmp"),
            "bundle-pubtest",
            generation,
            budget.clone(),
        )
        .await
        .unwrap();
        assert_eq!(budget.active_writers(), 0);
        session
            .commit_ingest_batch(IndexBatch {
                bundle_id: "bundle-pubtest".into(),
                file_id: 1,
                path: "/app.log".into(),
                chunks: vec![IndexChunk {
                    chunk_index: 0,
                    line_start: Some(0),
                    line_end: Some(0),
                    event_time_start_ms: None,
                    event_time_end_ms: None,
                    content: "searchable".into(),
                }],
                offsets: vec![(0, 0)],
                final_line_count: Some(1),
            })
            .await
            .unwrap();
        assert_eq!(budget.active_writers(), 1);
        session.abort().await;
        drop(session);
        assert_eq!(budget.active_writers(), 0);
        pool.close().await;
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn empty_bundle_finishes_with_a_valid_empty_index_without_writer_admission() {
        let (pool, root, generation) = fixture("empty").await;
        let budget = SearchResourceBudget::new(1, 16 * 1024 * 1024).unwrap();
        let session = BundleBuildSession::start(
            &pool,
            &root,
            &root.join(".tmp"),
            "bundle-pubtest",
            generation,
            budget.clone(),
        )
        .await
        .unwrap();
        assert_eq!(budget.active_writers(), 0);
        session.finish().await.unwrap();
        assert_eq!(budget.active_writers(), 0);
        let index = open_committed(
            root.join(artifact_relative_path("bundle-pubtest", generation).unwrap()),
        )
        .unwrap();
        assert_eq!(index.document_count, 0);
        pool.close().await;
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn empty_ingest_batch_does_not_admit_a_writer() {
        let (pool, root, generation) = fixture("empty-batch").await;
        let budget = SearchResourceBudget::new(1, 16 * 1024 * 1024).unwrap();
        let session = BundleBuildSession::start(
            &pool,
            &root,
            &root.join(".tmp"),
            "bundle-pubtest",
            generation,
            budget.clone(),
        )
        .await
        .unwrap();
        session
            .commit_ingest_batch(IndexBatch {
                bundle_id: "bundle-pubtest".into(),
                file_id: 1,
                path: "/empty.log".into(),
                chunks: Vec::new(),
                offsets: Vec::new(),
                final_line_count: Some(0),
            })
            .await
            .unwrap();
        assert_eq!(budget.active_writers(), 0);
        session.abort().await;
        assert_eq!(budget.queued_writers(), 0);
        assert_eq!(budget.active_writers(), 0);
        pool.close().await;
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn multiple_lazy_sessions_can_preflight_before_writer_admission() {
        let (pool, root, generation) = fixture("parallel-preflight").await;
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-pubtest-2','PUBTEST','hash-2','fixture-2','PROCESSING')")
            .execute(&pool)
            .await
            .unwrap();
        let second_generation =
            claim_publication(&pool, "bundle-pubtest-2", SearchBackendKind::Tantivy)
                .await
                .unwrap();
        let budget = SearchResourceBudget::new(1, 16 * 1024 * 1024).unwrap();
        let first = BundleBuildSession::start(
            &pool,
            &root,
            &root.join(".tmp"),
            "bundle-pubtest",
            generation,
            budget.clone(),
        )
        .await
        .unwrap();
        let second = BundleBuildSession::start(
            &pool,
            &root,
            &root.join(".tmp"),
            "bundle-pubtest-2",
            second_generation,
            budget.clone(),
        )
        .await
        .unwrap();
        assert_eq!(budget.active_writers(), 0);
        assert_eq!(budget.queued_writers(), 0);
        first.abort().await;
        second.abort().await;
        pool.close().await;
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn cancelling_a_queued_writer_does_not_leave_admission_state() {
        let (pool, root, generation) = fixture("queued-cancel").await;
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-pubtest-2','PUBTEST','hash-2','fixture-2','PROCESSING')")
            .execute(&pool)
            .await
            .unwrap();
        let second_generation =
            claim_publication(&pool, "bundle-pubtest-2", SearchBackendKind::Tantivy)
                .await
                .unwrap();
        let budget = SearchResourceBudget::new(1, 16 * 1024 * 1024).unwrap();
        let first = BundleBuildSession::start(
            &pool,
            &root,
            &root.join(".tmp"),
            "bundle-pubtest",
            generation,
            budget.clone(),
        )
        .await
        .unwrap();
        first
            .commit_ingest_batch(IndexBatch {
                bundle_id: "bundle-pubtest".into(),
                file_id: 1,
                path: "/first.log".into(),
                chunks: vec![IndexChunk {
                    chunk_index: 0,
                    line_start: Some(0),
                    line_end: Some(0),
                    event_time_start_ms: None,
                    event_time_end_ms: None,
                    content: "first".into(),
                }],
                offsets: vec![(0, 0)],
                final_line_count: Some(1),
            })
            .await
            .unwrap();

        let second = BundleBuildSession::start(
            &pool,
            &root,
            &root.join(".tmp"),
            "bundle-pubtest-2",
            second_generation,
            budget.clone(),
        )
        .await
        .unwrap();
        let second_for_task = second.clone();
        let commit = tokio::spawn(async move {
            second_for_task
                .commit_ingest_batch(IndexBatch {
                    bundle_id: "bundle-pubtest-2".into(),
                    file_id: 2,
                    path: "/second.log".into(),
                    chunks: vec![IndexChunk {
                        chunk_index: 0,
                        line_start: Some(0),
                        line_end: Some(0),
                        event_time_start_ms: None,
                        event_time_end_ms: None,
                        content: "second".into(),
                    }],
                    offsets: vec![(0, 0)],
                    final_line_count: Some(1),
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while budget.queued_writers() == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        commit.abort();
        let _ = commit.await;
        assert_eq!(budget.queued_writers(), 0);
        second.abort().await;
        first.abort().await;
        assert_eq!(budget.active_writers(), 0);
        pool.close().await;
        std::fs::remove_dir_all(root).unwrap();
    }
}
