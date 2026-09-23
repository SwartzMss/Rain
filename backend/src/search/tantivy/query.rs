use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashSet},
};

use tantivy::{
    DocAddress, DocSet, TERMINATED, TantivyDocument,
    query::{BooleanQuery, EnableScoring, Occur, Query, TermQuery},
    schema::{IndexRecordOption, Term, Value},
};

use crate::error::AppError;

use super::{
    tokenizer::{NGRAM_MAX, NGRAM_MIN},
    writer::CommittedBundleIndex,
};

const MAX_CANDIDATE_GRAMS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub file_id: i64,
    pub chunk_index: i64,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
    pub timeline: Option<String>,
    pub content: String,
    pub path: String,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SearchOptions<'a> {
    pub file_id: Option<i64>,
    pub timeline: Option<&'a str>,
    pub path_like: Option<&'a str>,
    pub visible_file_ids: Option<&'a HashSet<i64>>,
    pub from: usize,
    pub size: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SearchMetrics {
    pub candidate_docs: u64,
    pub stored_doc_reads: u64,
    pub exact_hits: u64,
    pub max_retained_hits: usize,
}

#[derive(Debug)]
pub(crate) struct SearchPage {
    pub hits: Vec<SearchHit>,
    pub total: i64,
    pub metrics: SearchMetrics,
}

#[derive(Debug)]
struct RankedHit {
    key: (i64, i64, i64),
    hit: SearchHit,
}

impl PartialEq for RankedHit {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for RankedHit {}

impl PartialOrd for RankedHit {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedHit {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

pub struct CandidateSearch {
    index: CommittedBundleIndex,
}

impl CandidateSearch {
    pub fn new(index: CommittedBundleIndex) -> Self {
        Self { index }
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, AppError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        Ok(self
            .search_page(
                query,
                SearchOptions {
                    size: limit,
                    ..SearchOptions::default()
                },
            )?
            .hits)
    }

    pub(crate) fn search_page(
        &self,
        query: &str,
        options: SearchOptions<'_>,
    ) -> Result<SearchPage, AppError> {
        let query = query.trim();
        if query.chars().count() < NGRAM_MIN {
            return Err(AppError::BadRequest(format!(
                "Tantivy search requires at least {NGRAM_MIN} characters"
            )));
        }

        if options.file_id.is_some_and(|file_id| file_id < 0)
            || options
                .visible_file_ids
                .is_some_and(|visible_file_ids| visible_file_ids.is_empty())
        {
            return Ok(SearchPage {
                hits: Vec::new(),
                total: 0,
                metrics: SearchMetrics::default(),
            });
        }

        let needle = query.to_lowercase();
        let terms = unique_ngrams(&needle, NGRAM_MIN, NGRAM_MAX);
        let mut must: Vec<(Occur, Box<dyn Query>)> = terms
            .into_iter()
            .map(|gram| {
                let term = Term::from_field_text(self.index.fields.content, &gram);
                (
                    Occur::Must,
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic)) as Box<dyn Query>,
                )
            })
            .collect();
        if let Some(file_id) = options.file_id {
            let term = Term::from_field_u64(self.index.fields.file_id, file_id as u64);
            must.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        }
        let candidate_query = BooleanQuery::from(must);
        let searcher = self
            .index
            .index
            .reader()
            .map_err(|error| AppError::Config(format!("open Tantivy searcher: {error}")))?
            .searcher();
        let weight = candidate_query
            .weight(EnableScoring::disabled_from_searcher(&searcher))
            .map_err(|error| AppError::Config(format!("prepare Tantivy query: {error}")))?;
        let window_limit = if options.size == 0 {
            0
        } else {
            options.from.saturating_add(options.size)
        };
        let mut retained = BinaryHeap::new();
        let mut metrics = SearchMetrics::default();
        let mut total = 0_u64;

        for (segment_ord, segment_reader) in searcher.segment_readers().iter().enumerate() {
            let mut scorer = weight
                .scorer(segment_reader, 1.0)
                .map_err(|error| AppError::Config(format!("score Tantivy query: {error}")))?;
            let mut doc_id = scorer.doc();
            while doc_id != TERMINATED {
                metrics.candidate_docs += 1;
                let address = DocAddress {
                    segment_ord: segment_ord as u32,
                    doc_id,
                };
                let document: TantivyDocument = searcher
                    .doc(address)
                    .map_err(|error| AppError::Config(format!("read Tantivy document: {error}")))?;
                metrics.stored_doc_reads += 1;

                let file_id = document
                    .get_first(self.index.fields.file_id)
                    .and_then(|value| value.as_u64())
                    .unwrap_or_default() as i64;
                let visible = options
                    .visible_file_ids
                    .is_none_or(|visible_file_ids| visible_file_ids.contains(&file_id));
                let file_matches = options.file_id.is_none_or(|expected| expected == file_id);
                let timeline = document
                    .get_first(self.index.fields.timeline)
                    .and_then(|value| value.as_str())
                    .map(ToOwned::to_owned);
                let timeline_matches = options
                    .timeline
                    .is_none_or(|expected| timeline.as_deref() == Some(expected));
                let path = document
                    .get_first(self.index.fields.path)
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_owned();
                let path_matches = options
                    .path_like
                    .is_none_or(|expected| path.contains(expected));

                if visible && file_matches && timeline_matches && path_matches {
                    let Some(content) = document
                        .get_first(self.index.fields.content)
                        .and_then(|value| value.as_str())
                    else {
                        doc_id = scorer.advance();
                        continue;
                    };
                    if content.to_lowercase().contains(&needle) {
                        let hit = SearchHit {
                            file_id,
                            chunk_index: document
                                .get_first(self.index.fields.chunk_index)
                                .and_then(|value| value.as_u64())
                                .unwrap_or_default()
                                as i64,
                            line_start: document
                                .get_first(self.index.fields.line_start)
                                .and_then(|value| value.as_i64()),
                            line_end: document
                                .get_first(self.index.fields.line_end)
                                .and_then(|value| value.as_i64()),
                            timeline,
                            content: content.to_owned(),
                            path,
                        };
                        total += 1;
                        if window_limit > 0 {
                            let ranked = RankedHit {
                                key: sort_key(&hit),
                                hit,
                            };
                            if retained.len() < window_limit {
                                retained.push(ranked);
                            } else if retained
                                .peek()
                                .is_some_and(|current| ranked.key < current.key)
                            {
                                retained.pop();
                                retained.push(ranked);
                            }
                            metrics.max_retained_hits =
                                metrics.max_retained_hits.max(retained.len());
                        }
                    }
                }
                doc_id = scorer.advance();
            }
        }

        metrics.exact_hits = total;
        let mut ordered_hits = retained
            .into_iter()
            .map(|ranked| ranked.hit)
            .collect::<Vec<_>>();
        ordered_hits.sort_by_key(sort_key);
        let hits: Vec<SearchHit> = ordered_hits
            .into_iter()
            .skip(options.from)
            .take(options.size)
            .collect();
        Ok(SearchPage {
            hits,
            total: total as i64,
            metrics,
        })
    }
}

fn sort_key(hit: &SearchHit) -> (i64, i64, i64) {
    (
        hit.line_start.unwrap_or(i64::MIN),
        hit.file_id,
        hit.chunk_index,
    )
}

pub(crate) fn unique_ngrams(value: &str, min: usize, max: usize) -> Vec<String> {
    let chars: Vec<char> = value.chars().collect();
    let mut grams = HashSet::new();
    let mut ordered = Vec::new();
    for width in min..=max.min(chars.len()) {
        for window in chars.windows(width) {
            let gram = window.iter().collect::<String>();
            if grams.insert(gram.clone()) {
                ordered.push(gram);
                if ordered.len() == MAX_CANDIDATE_GRAMS {
                    return ordered;
                }
            }
        }
    }
    ordered
}
