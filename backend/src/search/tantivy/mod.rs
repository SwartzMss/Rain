//! Per-Bundle Tantivy backend used by the v0.1.x default build.
//!
//! Every hit is verified against the stored cleaned chunk before it is
//! returned, so n-gram false positives cannot change search semantics.

use std::{collections::HashSet, path::PathBuf};

use crate::{
    error::AppError,
    search::{ContentSearchRequest, ContentSearchResult, ContentSearchRow, ContentSearchScope},
};
use tantivy::{
    DocAddress, DocSet, TERMINATED, TantivyDocument,
    query::{AllQuery, EnableScoring, Query},
    schema::Value,
};

pub mod pipeline;
pub mod publication;
pub mod query;
pub mod schema;
pub mod tokenizer;
pub mod writer;

pub use query::{CandidateSearch, SearchHit};
use query::{SearchOptions, SearchPage};
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
        let page = CandidateSearch::new(committed).search_page(
            &query,
            SearchOptions {
                file_id,
                timeline: timeline.as_deref(),
                path_like: path_like.as_deref(),
                visible_file_ids: visible_file_ids.as_ref(),
                from,
                size,
            },
        )?;
        let SearchPage {
            hits,
            total,
            metrics,
        } = page;
        let rows: Vec<ContentSearchRow> = hits
            .into_iter()
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
        tracing::debug!(
            metric = "tantivy_search",
            candidate_docs = metrics.candidate_docs,
            stored_doc_reads = metrics.stored_doc_reads,
            exact_hits = metrics.exact_hits,
            retained_hits = metrics.max_retained_hits,
            returned_hits = rows.len(),
            "completed bounded Tantivy search"
        );
        Ok(ContentSearchResult {
            total,
            rows,
            truncated: false,
        })
    })
    .await
    .map_err(|error| AppError::Config(format!("Tantivy search task failed: {error}")))?
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
    let weight = AllQuery
        .weight(EnableScoring::disabled_from_searcher(&searcher))
        .map_err(|error| AppError::Config(format!("prepare Tantivy rebuild scan: {error}")))?;
    let mut writer = BundleIndexWriter::create(destination, heap_size_bytes)?;
    for (segment_ord, segment_reader) in searcher.segment_readers().iter().enumerate() {
        let mut scorer = weight
            .scorer(segment_reader, 1.0)
            .map_err(|error| AppError::Config(format!("scan Tantivy rebuild source: {error}")))?;
        let mut doc_id = scorer.doc();
        while doc_id != TERMINATED {
            let address = DocAddress {
                segment_ord: segment_ord as u32,
                doc_id,
            };
            let document: TantivyDocument = searcher.doc(address).map_err(|error| {
                AppError::Config(format!("read Tantivy rebuild document: {error}"))
            })?;
            let file_id = document
                .get_first(source.fields.file_id)
                .and_then(|value| value.as_u64())
                .unwrap_or_default() as i64;
            if visible_file_ids.contains(&file_id)
                && let Some(content) = document
                    .get_first(source.fields.content)
                    .and_then(|value| value.as_str())
            {
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
            doc_id = scorer.advance();
        }
    }
    Ok(writer.commit()?.document_count)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        BundleIndexWriter, CandidateSearch, IndexedChunk, SearchOptions, rebuild_visible_index,
        writer,
    };

    fn temp_index_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "rain-tantivy-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[test]
    fn rejects_unbounded_window_even_for_an_empty_index() {
        let path = temp_index_path();
        let writer = BundleIndexWriter::create(&path, 16 * 1024 * 1024).unwrap();
        let search = CandidateSearch::new(writer.commit().unwrap());
        for (from, size) in [(usize::MAX, 20), (99_999, 2), (0, 100_001)] {
            let result = search.search_page(
                "marker",
                SearchOptions {
                    from,
                    size,
                    ..SearchOptions::default()
                },
            );
            assert!(result.is_err(), "unbounded window {from} + {size} accepted");
        }
        drop(search);
        std::fs::remove_dir_all(path).unwrap();
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
    fn bounded_page_applies_visibility_and_preserves_total() {
        let path = temp_index_path();
        let mut writer = BundleIndexWriter::create(&path, 16 * 1024 * 1024).unwrap();
        for (file_id, chunk_index, line_start) in [(10, 0, 0), (11, 1, 1), (12, 2, 2)] {
            writer
                .add_chunk(&IndexedChunk {
                    file_id,
                    chunk_index,
                    line_start: Some(line_start),
                    line_end: Some(line_start),
                    event_time_start_ms: None,
                    event_time_end_ms: None,
                    timeline: Some("all".into()),
                    content: "marker".into(),
                    path: "/visible.log".into(),
                })
                .unwrap();
        }
        let committed = writer.commit().unwrap();
        let visible_file_ids = HashSet::from([11_i64, 12]);
        let page = CandidateSearch::new(committed)
            .search_page(
                "marker",
                SearchOptions {
                    visible_file_ids: Some(&visible_file_ids),
                    from: 1,
                    size: 1,
                    ..SearchOptions::default()
                },
            )
            .unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].file_id, 12);
        assert_eq!(page.metrics.max_retained_hits, 2);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn bounded_page_pushes_file_id_filter_into_candidate_scan() {
        let path = temp_index_path();
        let mut writer = BundleIndexWriter::create(&path, 16 * 1024 * 1024).unwrap();
        for (file_id, chunk_index) in [(7, 0), (7, 1), (8, 2), (9, 3)] {
            writer
                .add_chunk(&IndexedChunk {
                    file_id,
                    chunk_index,
                    line_start: Some(chunk_index),
                    line_end: Some(chunk_index),
                    event_time_start_ms: None,
                    event_time_end_ms: None,
                    timeline: Some("all".into()),
                    content: "marker".into(),
                    path: "/app.log".into(),
                })
                .unwrap();
        }
        let committed = writer.commit().unwrap();
        let page = CandidateSearch::new(committed)
            .search_page(
                "marker",
                SearchOptions {
                    file_id: Some(7),
                    from: 0,
                    size: 10,
                    ..SearchOptions::default()
                },
            )
            .unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.metrics.candidate_docs, 2);
        assert_eq!(page.hits.len(), 2);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn bounded_page_keeps_deep_pagination_ordered() {
        let path = temp_index_path();
        let mut writer = BundleIndexWriter::create(&path, 16 * 1024 * 1024).unwrap();
        for line_start in 0..20 {
            writer
                .add_chunk(&IndexedChunk {
                    file_id: 1,
                    chunk_index: line_start,
                    line_start: Some(line_start),
                    line_end: Some(line_start),
                    event_time_start_ms: None,
                    event_time_end_ms: None,
                    timeline: Some("all".into()),
                    content: "marker".into(),
                    path: "/app.log".into(),
                })
                .unwrap();
        }
        let committed = writer.commit().unwrap();
        let page = CandidateSearch::new(committed)
            .search_page(
                "marker",
                SearchOptions {
                    from: 10,
                    size: 3,
                    ..SearchOptions::default()
                },
            )
            .unwrap();
        assert_eq!(page.total, 20);
        assert_eq!(
            page.hits
                .iter()
                .map(|hit| hit.chunk_index)
                .collect::<Vec<_>>(),
            vec![10, 11, 12]
        );
        assert_eq!(page.metrics.max_retained_hits, 13);
        let count_only = CandidateSearch::new(writer::open_committed(&path).unwrap())
            .search_page(
                "marker",
                SearchOptions {
                    from: 10,
                    size: 0,
                    ..SearchOptions::default()
                },
            )
            .unwrap();
        assert_eq!(count_only.total, 20);
        assert!(count_only.hits.is_empty());
        assert_eq!(count_only.metrics.max_retained_hits, 0);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn visible_rebuild_copies_documents_without_materializing_addresses() {
        let source_path = temp_index_path();
        let destination_path = temp_index_path();
        let mut writer = BundleIndexWriter::create(&source_path, 16 * 1024 * 1024).unwrap();
        for file_id in [7, 8] {
            writer
                .add_chunk(&IndexedChunk {
                    file_id,
                    chunk_index: 0,
                    line_start: Some(file_id),
                    line_end: Some(file_id),
                    event_time_start_ms: None,
                    event_time_end_ms: None,
                    timeline: Some("all".into()),
                    content: "marker".into(),
                    path: "/app.log".into(),
                })
                .unwrap();
        }
        writer.commit().unwrap();
        let copied = rebuild_visible_index(
            &source_path,
            &destination_path,
            &HashSet::from([7_i64]),
            16 * 1024 * 1024,
        )
        .unwrap();
        assert_eq!(copied, 1);
        let committed = writer::open_committed(&destination_path).unwrap();
        let hits = CandidateSearch::new(committed)
            .search("marker", 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file_id, 7);
        std::fs::remove_dir_all(source_path).unwrap();
        std::fs::remove_dir_all(destination_path).unwrap();
    }
}
