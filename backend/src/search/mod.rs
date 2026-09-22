//! Search-plane contracts. Callers depend on these owned values, not SQL or FTS5.
use async_trait::async_trait;

use crate::error::AppError;

pub mod publication;
pub mod sqlite;
#[cfg(feature = "tantivy-search")]
pub mod tantivy;

#[derive(Debug, Clone)]
pub enum ContentSearchScope {
    Bundle {
        bundle_id: String,
        timeline: Option<String>,
        file_id: Option<i64>,
    },
    Issue {
        issue_code: String,
    },
}

#[derive(Debug, Clone)]
pub struct ContentSearchRequest {
    pub scope: ContentSearchScope,
    pub query: String,
    pub path_like: Option<String>,
    pub from: i64,
    pub size: i64,
}

#[derive(Debug, Clone)]
pub struct ContentSearchRow {
    pub file_id: i64,
    pub path: String,
    pub bundle_hash: Option<String>,
    pub timeline: Option<String>,
    pub offset: Option<i64>,
    pub line_end: Option<i64>,
    pub chunk_index: Option<i64>,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ContentSearchResult {
    pub total: i64,
    pub rows: Vec<ContentSearchRow>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct FilenameSearchRequest {
    pub issue_code: String,
    pub query: String,
    pub from: i64,
    pub size: i64,
}

#[derive(Debug, Clone)]
pub struct FilenameSearchRow {
    pub file_id: i64,
    pub name: String,
    pub path: String,
    pub bundle_hash: String,
}

#[derive(Debug, Clone)]
pub struct FilenameSearchResult {
    pub total: i64,
    pub rows: Vec<FilenameSearchRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSearchMode {
    Fts,
    ShortLiteral,
}

#[derive(Debug, Clone, Copy)]
pub struct SearchTimeWindow {
    pub start_key: i64,
    pub end_key: i64,
}

#[derive(Debug, Clone)]
pub struct SkillSearchRequest {
    pub issue_code: String,
    pub query: String,
    pub mode: SkillSearchMode,
    pub path_prefix: Option<String>,
    pub bundle_hash: Option<String>,
    pub file_id: Option<i64>,
    pub time_window: Option<SearchTimeWindow>,
    pub fetch_limit: i64,
}

#[derive(Debug, Clone)]
pub struct SkillSearchRow {
    pub file_id: i64,
    pub bundle_hash: String,
    pub path: String,
    pub start_line: i64,
    pub end_line: i64,
    pub snippet: String,
}

#[derive(Debug, Clone)]
pub struct SkillSearchResult {
    pub rows: Vec<SkillSearchRow>,
    pub has_unindexed_matches: bool,
}

#[derive(Debug, Clone)]
pub struct IndexBatch {
    pub bundle_id: String,
    pub file_id: i64,
    pub chunks: Vec<IndexChunk>,
    pub offsets: Vec<(i64, i64)>,
    pub final_line_count: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct IndexChunk {
    pub chunk_index: i64,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
    pub event_time_start_ms: Option<i64>,
    pub event_time_end_ms: Option<i64>,
    pub content: String,
}

#[async_trait]
pub trait SearchIndex: Send + Sync {
    async fn search_content(
        &self,
        request: ContentSearchRequest,
    ) -> Result<ContentSearchResult, AppError>;
    async fn search_filenames(
        &self,
        request: FilenameSearchRequest,
    ) -> Result<FilenameSearchResult, AppError>;
    async fn search_skill(
        &self,
        request: SkillSearchRequest,
    ) -> Result<SkillSearchResult, AppError>;
    async fn commit_batch(&self, batch: IndexBatch) -> Result<(), AppError>;
}

pub async fn search_tantivy_bundle(
    path: std::path::PathBuf,
    request: ContentSearchRequest,
) -> Result<ContentSearchResult, AppError> {
    #[cfg(feature = "tantivy-search")]
    {
        tantivy::search_bundle(path, request).await
    }
    #[cfg(not(feature = "tantivy-search"))]
    {
        let _ = (path, request);
        Err(AppError::Config(
            "Tantivy backend requires the tantivy-search feature".into(),
        ))
    }
}
