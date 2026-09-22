//! Experimental per-Bundle Tantivy backend.
//!
//! This module is deliberately opt-in behind `tantivy-search`. It is a
//! candidate index: every hit is verified against the stored cleaned chunk
//! before it is returned, so n-gram false positives cannot change search
//! semantics. Publication and production routing are intentionally left to a
//! later phase.

pub mod pipeline;
pub mod query;
pub mod schema;
pub mod tokenizer;
pub mod writer;

pub use query::{CandidateSearch, SearchHit};
pub use schema::{BundleSchema, build_schema};
pub use writer::{BundleIndexWriter, IndexedChunk};

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
