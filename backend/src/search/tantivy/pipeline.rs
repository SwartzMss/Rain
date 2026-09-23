//! A bounded producer/consumer bridge for the experimental Bundle writer.
//!
//! The reader owns only the current batch and the bounded channel capacity.
//! Tantivy work runs on Tokio's blocking pool so a large file cannot occupy an
//! async executor worker while segments are flushed.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use tokio::{sync::mpsc, task::JoinHandle};

use crate::error::AppError;

use super::super::resource::SearchResourcePermit;
use super::writer::{BundleIndexWriter, CommittedBundleIndex, IndexedChunk};

#[derive(Debug, Clone, Copy)]
pub struct PipelineConfig {
    pub queue_capacity: usize,
    pub writer_heap_size_bytes: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 2,
            writer_heap_size_bytes: 64 * 1024 * 1024,
        }
    }
}

pub struct BoundedBundlePipeline {
    sender: Mutex<Option<mpsc::Sender<Vec<IndexedChunk>>>>,
    task: tokio::sync::Mutex<Option<JoinHandle<Result<CommittedBundleIndex, AppError>>>>,
    aborted: Arc<AtomicBool>,
}

impl BoundedBundlePipeline {
    /// Start a bounded pipeline without tying it to the global search budget.
    /// Publication builds should use [`Self::start_with_permit`] so the permit
    /// lifetime covers the blocking writer task itself.
    pub fn start(path: PathBuf, config: PipelineConfig) -> Result<Self, AppError> {
        Self::start_inner(path, config, None)
    }

    pub fn start_with_permit(
        path: PathBuf,
        config: PipelineConfig,
        resource_permit: SearchResourcePermit,
    ) -> Result<Self, AppError> {
        Self::start_inner(path, config, Some(resource_permit))
    }

    fn start_inner(
        path: PathBuf,
        config: PipelineConfig,
        resource_permit: Option<SearchResourcePermit>,
    ) -> Result<Self, AppError> {
        if config.queue_capacity == 0 || config.writer_heap_size_bytes == 0 {
            return Err(AppError::Config(
                "Tantivy pipeline limits must be positive".into(),
            ));
        }
        let (sender, mut receiver) = mpsc::channel(config.queue_capacity);
        let aborted = Arc::new(AtomicBool::new(false));
        let worker_aborted = aborted.clone();
        let task = tokio::task::spawn_blocking(move || {
            let _resource_permit = resource_permit;
            let mut writer = BundleIndexWriter::create(path, config.writer_heap_size_bytes)?;
            while let Some(batch) = receiver.blocking_recv() {
                for chunk in &batch {
                    writer.add_chunk(chunk)?;
                }
            }
            if worker_aborted.load(Ordering::Acquire) {
                return Err(AppError::Conflict("Tantivy pipeline was aborted".into()));
            }
            writer.commit()
        });
        Ok(Self {
            sender: Mutex::new(Some(sender)),
            task: tokio::sync::Mutex::new(Some(task)),
            aborted,
        })
    }

    pub async fn submit(&self, batch: Vec<IndexedChunk>) -> Result<(), AppError> {
        let sender = self
            .sender
            .lock()
            .map_err(|_| AppError::Config("Tantivy pipeline sender lock poisoned".into()))?
            .as_ref()
            .cloned();
        let Some(sender) = sender else {
            return Err(AppError::Conflict(
                "Tantivy pipeline is already closed".into(),
            ));
        };
        sender.send(batch).await.map_err(|_| {
            AppError::Config("Tantivy writer stopped before the batch was accepted".into())
        })
    }

    pub async fn finish(&self) -> Result<CommittedBundleIndex, AppError> {
        self.sender
            .lock()
            .map_err(|_| AppError::Config("Tantivy pipeline sender lock poisoned".into()))?
            .take();
        self.task
            .lock()
            .await
            .take()
            .ok_or_else(|| AppError::Conflict("Tantivy pipeline is already closed".into()))?
            .await
            .map_err(|error| AppError::Config(format!("Tantivy writer task failed: {error}")))?
    }

    pub async fn abort(&self) -> Result<(), AppError> {
        self.aborted.store(true, Ordering::Release);
        self.sender
            .lock()
            .map_err(|_| AppError::Config("Tantivy pipeline sender lock poisoned".into()))?
            .take();
        let Some(task) = self.task.lock().await.take() else {
            return Ok(());
        };
        let _ = task.await;
        Ok(())
    }
}

impl Drop for BoundedBundlePipeline {
    fn drop(&mut self) {
        self.aborted.store(true, Ordering::Release);
        if let Ok(mut sender) = self.sender.lock() {
            sender.take();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{resource::SearchResourceBudget, tantivy::CandidateSearch};

    fn path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "rain-tantivy-pipeline-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[tokio::test]
    async fn bounded_pipeline_commits_batches_in_order() {
        let path = path();
        let pipeline = BoundedBundlePipeline::start_with_permit(
            path.clone(),
            PipelineConfig {
                queue_capacity: 2,
                writer_heap_size_bytes: 16 * 1024 * 1024,
            },
            SearchResourceBudget::new(1, 16 * 1024 * 1024)
                .unwrap()
                .acquire()
                .await
                .unwrap(),
        )
        .unwrap();
        for index in 0..5 {
            pipeline
                .submit(vec![IndexedChunk {
                    file_id: 1,
                    chunk_index: index,
                    line_start: Some(index),
                    line_end: Some(index),
                    event_time_start_ms: None,
                    event_time_end_ms: None,
                    timeline: Some("all".into()),
                    content: format!("marker chunk {index}"),
                    path: "/app.log".into(),
                }])
                .await
                .unwrap();
        }
        let committed = pipeline.finish().await.unwrap();
        assert_eq!(committed.document_count, 5);
        let hits = CandidateSearch::new(committed)
            .search("marker", 10)
            .unwrap();
        assert_eq!(hits.len(), 5);
        assert_eq!(hits[0].chunk_index, 0);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn invalid_limits_are_rejected_before_starting_a_writer() {
        let result = BoundedBundlePipeline::start_with_permit(
            path(),
            PipelineConfig {
                queue_capacity: 0,
                writer_heap_size_bytes: 1,
            },
            SearchResourceBudget::new(1, 1)
                .unwrap()
                .acquire()
                .await
                .unwrap(),
        );
        assert!(matches!(result, Err(AppError::Config(_))));
    }
}
