#![cfg(feature = "tantivy-search")]

use backend::search::tantivy::{BundleIndexWriter, CandidateSearch, IndexedChunk};

#[test]
fn tantivy_candidate_hits_require_exact_contiguous_text_after_ngram_filtering() {
    let path = std::env::temp_dir().join(format!(
        "rain-tantivy-parity-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let mut writer = BundleIndexWriter::create(&path, 16 * 1024 * 1024).unwrap();
    for (chunk_index, content) in [(0, "abcd"), (1, "abc ... bcd"), (2, "ABCD tail")] {
        writer
            .add_chunk(&IndexedChunk {
                file_id: 11,
                chunk_index,
                line_start: Some(chunk_index),
                line_end: Some(chunk_index),
                event_time_start_ms: None,
                event_time_end_ms: None,
                timeline: Some("all".into()),
                content: content.into(),
                path: "/fixture.log".into(),
            })
            .unwrap();
    }
    let index = writer.commit().unwrap();
    let hits = CandidateSearch::new(index).search("abcd", 10).unwrap();
    assert_eq!(
        hits.into_iter()
            .map(|hit| hit.chunk_index)
            .collect::<Vec<_>>(),
        vec![0, 2]
    );
    std::fs::remove_dir_all(path).unwrap();
}
