use std::path::Path;

use futures_util::TryStreamExt;
use sqlx::SqlitePool;
use tokio::fs;

use crate::{
    error::AppError,
    search::publication::{SearchBackendKind, artifact_relative_path, mark_publication_ready},
};

use super::{
    pipeline::{BoundedBundlePipeline, PipelineConfig},
    writer::{IndexedChunk, open_committed},
};

const PUBLISH_BATCH_SIZE: usize = 64;

#[derive(Debug, sqlx::FromRow)]
struct SegmentRow {
    file_id: i64,
    path: String,
    timeline: Option<String>,
    content: String,
    line_offset: Option<i64>,
    line_end: Option<i64>,
    chunk_index: Option<i64>,
    event_time_start_ms: Option<i64>,
    event_time_end_ms: Option<i64>,
}

/// Build a per-Bundle Tantivy index from the already-normalized SQLite chunks.
/// The SQLite transaction is read in small batches while Tantivy applies its
/// own bounded producer/consumer queue. The final rename is the filesystem
/// publication point; the metadata row is marked READY only after reopening.
pub async fn publish_bundle(
    pool: &SqlitePool,
    data_root: &Path,
    temp_dir: &Path,
    bundle_id: &str,
    generation: i64,
) -> Result<(), AppError> {
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

    let pipeline = BoundedBundlePipeline::start(staging.clone(), PipelineConfig::default())?;
    let mut stream = sqlx::query_as::<_, SegmentRow>(
        "SELECT ls.file_id, f.path, ls.timeline, ls.content, ls.line_offset, ls.line_end, ls.chunk_index, ls.event_time_start_ms, ls.event_time_end_ms FROM log_segments ls JOIN visible_files f ON f.id = ls.file_id WHERE ls.bundle_id = ? ORDER BY ls.id",
    )
    .bind(bundle_id)
    .fetch(pool);
    let mut batch = Vec::with_capacity(PUBLISH_BATCH_SIZE);
    let mut expected = 0_u64;
    while let Some(row) = stream.try_next().await.map_err(AppError::Database)? {
        expected = expected.saturating_add(1);
        batch.push(IndexedChunk {
            file_id: row.file_id,
            chunk_index: row.chunk_index.unwrap_or_default(),
            line_start: row.line_offset,
            line_end: row.line_end,
            event_time_start_ms: row.event_time_start_ms,
            event_time_end_ms: row.event_time_end_ms,
            timeline: row.timeline,
            content: row.content,
            path: row.path,
        });
        if batch.len() == PUBLISH_BATCH_SIZE {
            pipeline.submit(std::mem::take(&mut batch)).await?;
        }
    }
    if !batch.is_empty() {
        pipeline.submit(batch).await?;
    }
    drop(stream);
    let committed = pipeline.finish().await?;
    if committed.document_count != expected {
        return Err(AppError::Config(format!(
            "Tantivy document count mismatch: expected {expected}, got {}",
            committed.document_count
        )));
    }
    drop(committed);

    if let Some(parent) = final_path.parent() {
        fs::create_dir_all(parent).await.map_err(AppError::Io)?;
    }
    fs::rename(&staging, &final_path)
        .await
        .map_err(AppError::Io)?;
    let verify_path = final_path.clone();
    let verified = match tokio::task::spawn_blocking(move || open_committed(verify_path)).await {
        Ok(result) => match result {
            Ok(index) => index,
            Err(error) => {
                remove_published_artifact(&final_path).await;
                return Err(error);
            }
        },
        Err(error) => {
            remove_published_artifact(&final_path).await;
            return Err(AppError::Config(format!(
                "Tantivy verification task failed: {error}"
            )));
        }
    };
    if verified.document_count != expected {
        let actual = verified.document_count;
        drop(verified);
        remove_published_artifact(&final_path).await;
        return Err(AppError::Config(format!(
            "Tantivy verification count mismatch: expected {expected}, got {actual}"
        )));
    }
    drop(verified);
    if let Err(error) =
        mark_publication_ready(pool, bundle_id, SearchBackendKind::Tantivy, generation).await
    {
        remove_published_artifact(&final_path).await;
        return Err(error);
    }
    Ok(())
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
