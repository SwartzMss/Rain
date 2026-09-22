//! A bounded producer/consumer bridge for the experimental Bundle writer.
//!
//! The reader owns only the current batch and the bounded channel capacity.
//! Tantivy work runs on Tokio's blocking pool so a large file cannot occupy an
//! async executor worker while segments are flushed.

use std::path::PathBuf;

use tokio::{sync::mpsc, task::JoinHandle};

use crate::error::AppError;

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
    sender: Option<mpsc::Sender<Vec<IndexedChunk>>>,
    task: Option<JoinHandle<Result<CommittedBundleIndex, AppError>>>,
}

impl BoundedBundlePipeline {
    pub fn start(path: PathBuf, config: PipelineConfig) -> Result<Self, AppError> {
        if config.queue_capacity == 0 || config.writer_heap_size_bytes == 0 {
            return Err(AppError::Config(
                "Tantivy pipeline limits must be positive".into(),
            ));
        }
        let (sender, mut receiver) = mpsc::channel(config.queue_capacity);
        let task = tokio::task::spawn_blocking(move || {
            let mut writer = BundleIndexWriter::create(path, config.writer_heap_size_bytes)?;
            while let Some(batch) = receiver.blocking_recv() {
                for chunk in &batch {
                    writer.add_chunk(chunk)?;
                }
            }
            writer.commit()
        });
        Ok(Self {
            sender: Some(sender),
            task: Some(task),
        })
    }

    pub async fn submit(&self, batch: Vec<IndexedChunk>) -> Result<(), AppError> {
        let Some(sender) = &self.sender else {
            return Err(AppError::Conflict(
                "Tantivy pipeline is already closed".into(),
            ));
        };
        sender.send(batch).await.map_err(|_| {
            AppError::Config("Tantivy writer stopped before the batch was accepted".into())
        })
    }

    pub async fn finish(mut self) -> Result<CommittedBundleIndex, AppError> {
        self.sender.take();
        self.task
            .take()
            .expect("Tantivy pipeline task must exist before finish")
            .await
            .map_err(|error| AppError::Config(format!("Tantivy writer task failed: {error}")))?
    }
}

impl Drop for BoundedBundlePipeline {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::tantivy::CandidateSearch;

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
        let pipeline = BoundedBundlePipeline::start(
            path.clone(),
            PipelineConfig {
                queue_capacity: 2,
                writer_heap_size_bytes: 16 * 1024 * 1024,
            },
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
        let result = BoundedBundlePipeline::start(
            path(),
            PipelineConfig {
                queue_capacity: 0,
                writer_heap_size_bytes: 1,
            },
        );
        assert!(matches!(result, Err(AppError::Config(_))));
    }
}
