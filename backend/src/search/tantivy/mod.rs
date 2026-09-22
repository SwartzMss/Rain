//! Experimental per-Bundle Tantivy backend.
//!
//! This module is deliberately opt-in behind `tantivy-search`. It is a
//! candidate index: every hit is verified against the stored cleaned chunk
//! before it is returned, so n-gram false positives cannot change search
//! semantics.

use std::path::PathBuf;

use crate::{
    error::AppError,
    search::{ContentSearchRequest, ContentSearchResult, ContentSearchRow, ContentSearchScope},
};

pub mod pipeline;
pub mod publication;
pub mod query;
pub mod schema;
pub mod tokenizer;
pub mod writer;

pub use query::{CandidateSearch, SearchHit};
pub use schema::{BundleSchema, build_schema};
pub use writer::{BundleIndexWriter, IndexedChunk};

pub async fn search_bundle(
    path: PathBuf,
    request: ContentSearchRequest,
) -> Result<ContentSearchResult, AppError> {
    let ContentSearchScope::Bundle {
        timeline, file_id, ..
    } = &request.scope
    else {
        return Err(AppError::Config(
            "Tantivy Bundle search received a non-Bundle scope".into(),
        ));
    };
    let timeline = timeline.clone();
    let file_id = *file_id;
    let path_like = request.path_like.clone();
    let from = request.from.max(0) as usize;
    let size = request.size.max(0) as usize;
    let query = request.query;
    tokio::task::spawn_blocking(move || {
        let committed = writer::open_committed(path)?;
        let hits = CandidateSearch::new(committed).search(&query, usize::MAX)?;
        let mut rows: Vec<_> = hits
            .into_iter()
            .filter(|hit| file_id.is_none_or(|value| value == hit.file_id))
            .filter(|hit| {
                timeline
                    .as_deref()
                    .is_none_or(|value| hit.timeline.as_deref() == Some(value))
            })
            .filter(|hit| {
                path_like
                    .as_deref()
                    .is_none_or(|value| hit.path.contains(value))
            })
            .map(|hit| ContentSearchRow {
                file_id: hit.file_id,
                path: hit.path,
                bundle_hash: None,
                timeline: hit.timeline,
                offset: hit.line_start,
                line_end: hit.line_end,
                chunk_index: Some(hit.chunk_index),
                content: hit.content,
            })
            .collect();
        rows.sort_by_key(|row| {
            (
                row.offset.unwrap_or(i64::MIN),
                row.file_id,
                row.chunk_index.unwrap_or(i64::MIN),
            )
        });
        let total = rows.len() as i64;
        let rows = rows.into_iter().skip(from).take(size).collect();
        Ok(ContentSearchResult {
            total,
            rows,
            truncated: false,
        })
    })
    .await
    .map_err(|error| AppError::Config(format!("Tantivy search task failed: {error}")))?
}

#[cfg(test)]
mod tests {
    use super::{BundleIndexWriter, CandidateSearch, IndexedChunk};

    fn temp_index_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "rain-tantivy-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[test]
    fn ngram_candidates_are_verified_before_returning_hits() {
        let path = temp_index_path();
        let mut writer = BundleIndexWriter::create(&path, 16 * 1024 * 1024).unwrap();
        for (chunk_index, content) in [
            (0, "prefix ABCD suffix"),
            (1, "abc ... bcd"),
            (2, "a very different line"),
        ] {
            writer
                .add_chunk(&IndexedChunk {
                    file_id: 7,
                    chunk_index,
                    line_start: Some(chunk_index),
                    line_end: Some(chunk_index),
                    event_time_start_ms: None,
                    event_time_end_ms: None,
                    timeline: Some("all".into()),
                    content: content.to_owned(),
                    path: "/app.log".into(),
                })
                .unwrap();
        }
        let committed = writer.commit().unwrap();
        assert_eq!(committed.document_count, 3);
        let hits = CandidateSearch::new(committed).search("abcd", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].chunk_index, 0);
        assert_eq!(hits[0].content, "prefix ABCD suffix");
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn duplicate_ngrams_and_unicode_are_supported() {
        let path = temp_index_path();
        let mut writer = BundleIndexWriter::create(&path, 16 * 1024 * 1024).unwrap();
        writer
            .add_chunk(&IndexedChunk {
                file_id: 1,
                chunk_index: 0,
                line_start: None,
                line_end: None,
                event_time_start_ms: None,
                event_time_end_ms: None,
                timeline: Some("all".into()),
                content: "错误错误标记".into(),
                path: "/中文.log".into(),
            })
            .unwrap();
        let committed = writer.commit().unwrap();
        let hits = CandidateSearch::new(committed).search("错误", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "/中文.log");
        std::fs::remove_dir_all(path).unwrap();
    }
}
