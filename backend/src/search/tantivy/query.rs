use std::collections::HashSet;

use tantivy::{
    TantivyDocument,
    collector::TopDocs,
    query::{BooleanQuery, Occur, Query, TermQuery},
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
    pub content: String,
    pub path: String,
}

pub struct CandidateSearch {
    index: CommittedBundleIndex,
}

impl CandidateSearch {
    pub fn new(index: CommittedBundleIndex) -> Self {
        Self { index }
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, AppError> {
        let query = query.trim();
        if limit == 0 {
            return Ok(Vec::new());
        }
        if query.chars().count() < NGRAM_MIN {
            return Err(AppError::BadRequest(
                "Tantivy prototype requires at least 2 characters".into(),
            ));
        }
        let needle = query.to_lowercase();
        let terms = unique_ngrams(&query.to_lowercase(), NGRAM_MIN, NGRAM_MAX);
        let must: Vec<(Occur, Box<dyn Query>)> = terms
            .into_iter()
            .map(|gram| {
                let term = Term::from_field_text(self.index.fields.content, &gram);
                (
                    Occur::Must,
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic)) as Box<dyn Query>,
                )
            })
            .collect();
        let candidate_query = BooleanQuery::from(must);
        let searcher = self
            .index
            .index
            .reader()
            .map_err(|error| AppError::Config(format!("open Tantivy searcher: {error}")))?
            .searcher();
        let candidate_limit = searcher.num_docs().min(usize::MAX as u64) as usize;
        if candidate_limit == 0 {
            return Ok(Vec::new());
        }
        let docs = searcher
            .search(
                &candidate_query,
                &TopDocs::with_limit(candidate_limit).order_by_score(),
            )
            .map_err(|error| AppError::Config(format!("query Tantivy index: {error}")))?;
        let mut hits = Vec::new();
        for (_score, address) in docs {
            let document: TantivyDocument = searcher
                .doc(address)
                .map_err(|error| AppError::Config(format!("read Tantivy document: {error}")))?;
            let Some(content) = document
                .get_first(self.index.fields.content)
                .and_then(|value| value.as_str())
            else {
                continue;
            };
            if !content.to_lowercase().contains(&needle) {
                continue;
            }
            let file_id = document
                .get_first(self.index.fields.file_id)
                .and_then(|value| value.as_u64())
                .unwrap_or_default() as i64;
            let chunk_index = document
                .get_first(self.index.fields.chunk_index)
                .and_then(|value| value.as_u64())
                .unwrap_or_default() as i64;
            let line_start = document
                .get_first(self.index.fields.line_start)
                .and_then(|value| value.as_i64());
            let line_end = document
                .get_first(self.index.fields.line_end)
                .and_then(|value| value.as_i64());
            let path = document
                .get_first(self.index.fields.path)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned();
            hits.push(SearchHit {
                file_id,
                chunk_index,
                line_start,
                line_end,
                content: content.to_owned(),
                path,
            });
            if hits.len() >= limit {
                break;
            }
        }
        Ok(hits)
    }
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
