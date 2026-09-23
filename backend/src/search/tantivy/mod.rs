//! Per-Bundle Tantivy backend used by the v0.1.x default build.
//!
//! Every hit is verified against the stored cleaned chunk before it is
//! returned, so n-gram false positives cannot change search semantics.

use std::{collections::HashSet, path::PathBuf};

use crate::{
    error::AppError,
    search::{ContentSearchRequest, ContentSearchResult, ContentSearchRow, ContentSearchScope},
};
use tantivy::{TantivyDocument, collector::TopDocs, query::AllQuery, schema::Value};

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
    search_bundle_inner(path, request, None).await
}

pub async fn search_bundle_visible(
    path: PathBuf,
    request: ContentSearchRequest,
    visible_file_ids: HashSet<i64>,
) -> Result<ContentSearchResult, AppError> {
    search_bundle_inner(path, request, Some(visible_file_ids)).await
}

async fn search_bundle_inner(
    path: PathBuf,
    request: ContentSearchRequest,
    visible_file_ids: Option<HashSet<i64>>,
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
        let (hits, visibility_total) = match visible_file_ids.as_ref() {
            Some(visible) => filter_visible_hits(hits, visible),
            None => {
                let total = hits.len() as i64;
                (hits, total)
            }
        };
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
        let total = if visible_file_ids.is_some() {
            rows.len() as i64
        } else {
            visibility_total
        };
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

pub(crate) fn filter_visible_hits(
    hits: Vec<SearchHit>,
    visible_file_ids: &HashSet<i64>,
) -> (Vec<SearchHit>, i64) {
    let filtered = hits
        .into_iter()
        .filter(|hit| visible_file_ids.contains(&hit.file_id))
        .collect::<Vec<_>>();
    let total = filtered.len() as i64;
    (filtered, total)
}

/// Rebuild an immutable generation by copying only documents belonging to the
/// current visible file set. This avoids rereading CAS content for deletion
/// compaction while preserving every stored search field.
pub fn rebuild_visible_index(
    source: impl AsRef<std::path::Path>,
    destination: impl AsRef<std::path::Path>,
    visible_file_ids: &HashSet<i64>,
    heap_size_bytes: usize,
) -> Result<u64, AppError> {
    let source = writer::open_committed(source)?;
    let reader = source
        .index
        .reader()
        .map_err(|error| AppError::Config(format!("open Tantivy rebuild reader: {error}")))?;
    let searcher = reader.searcher();
    let addresses = searcher
        .search(
            &AllQuery,
            &TopDocs::with_limit(searcher.num_docs() as usize).order_by_score(),
        )
        .map_err(|error| AppError::Config(format!("scan Tantivy rebuild source: {error}")))?;
    let mut writer = BundleIndexWriter::create(destination, heap_size_bytes)?;
    for (_, address) in addresses {
        let document: TantivyDocument = searcher
            .doc(address)
            .map_err(|error| AppError::Config(format!("read Tantivy rebuild document: {error}")))?;
        let file_id = document
            .get_first(source.fields.file_id)
            .and_then(|value| value.as_u64())
            .unwrap_or_default() as i64;
        if !visible_file_ids.contains(&file_id) {
            continue;
        }
        let Some(content) = document
            .get_first(source.fields.content)
            .and_then(|value| value.as_str())
        else {
            continue;
        };
        let chunk = IndexedChunk {
            file_id,
            chunk_index: document
                .get_first(source.fields.chunk_index)
                .and_then(|value| value.as_u64())
                .unwrap_or_default() as i64,
            line_start: document
                .get_first(source.fields.line_start)
                .and_then(|value| value.as_i64()),
            line_end: document
                .get_first(source.fields.line_end)
                .and_then(|value| value.as_i64()),
            event_time_start_ms: document
                .get_first(source.fields.event_time_start)
                .and_then(|value| value.as_i64()),
            event_time_end_ms: document
                .get_first(source.fields.event_time_end)
                .and_then(|value| value.as_i64()),
            timeline: document
                .get_first(source.fields.timeline)
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned),
            content: content.to_owned(),
            path: document
                .get_first(source.fields.path)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned(),
        };
        writer.add_chunk(&chunk)?;
    }
    Ok(writer.commit()?.document_count)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{BundleIndexWriter, CandidateSearch, IndexedChunk, filter_visible_hits};

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
        let hits = CandidateSearch::new(committed)
            .search("错误标", 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "/中文.log");
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn visibility_filter_fills_page_after_hidden_candidates() {
        let hits = vec![
            super::SearchHit {
                file_id: 10,
                chunk_index: 0,
                line_start: Some(0),
                line_end: Some(0),
                timeline: None,
                content: "marker".into(),
                path: "/deleted.log".into(),
            },
            super::SearchHit {
                file_id: 11,
                chunk_index: 0,
                line_start: Some(1),
                line_end: Some(1),
                timeline: None,
                content: "marker".into(),
                path: "/visible.log".into(),
            },
        ];
        let visible = HashSet::from([11_i64]);
        let (filtered, total) = filter_visible_hits(hits, &visible);
        assert_eq!(total, 1);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].file_id, 11);
    }
}
