//! Search-plane contracts. Callers depend on these owned values, not SQL or FTS5.
use async_trait::async_trait;

use crate::error::AppError;

/// Absolute safety ceiling for any search page retained by a backend.
///
/// API configuration may choose a lower value, but it cannot raise this
/// bound. Keeping the invariant here protects direct index callers as well as
/// HTTP routes from allocating an unbounded `from + size` heap.
pub const HARD_MAX_SEARCH_WINDOW: usize = 100_000;
pub const DEFAULT_MAX_SEARCH_WINDOW: i64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchWindow {
    pub from: usize,
    pub size: usize,
    pub limit: usize,
}

pub fn validate_search_window(
    from: i64,
    size: i64,
    max_window: i64,
) -> Result<SearchWindow, AppError> {
    if max_window <= 0 || usize::try_from(max_window).is_err() {
        return Err(AppError::Config(
            "search window limit must be a positive platform-sized integer".into(),
        ));
    }
    let max_window = max_window as usize;
    if max_window > HARD_MAX_SEARCH_WINDOW {
        return Err(AppError::Config(format!(
            "search window limit must not exceed {HARD_MAX_SEARCH_WINDOW}"
        )));
    }

    let from = from.max(0) as u128;
    let size = size.max(0) as u128;
    if from > max_window as u128 {
        return Err(AppError::public(
            actix_web::http::StatusCode::BAD_REQUEST,
            "SEARCH_OFFSET_TOO_LARGE",
            format!("search offset exceeds the server limit of {max_window}"),
        ));
    }
    if from == max_window as u128 && size > 0 {
        return Err(AppError::public(
            actix_web::http::StatusCode::BAD_REQUEST,
            "SEARCH_OFFSET_TOO_LARGE",
            format!("search offset must be smaller than the server limit of {max_window}"),
        ));
    }
    let limit = if size == 0 {
        0
    } else {
        from.checked_add(size).ok_or_else(|| {
            AppError::public(
                actix_web::http::StatusCode::BAD_REQUEST,
                "SEARCH_WINDOW_TOO_LARGE",
                format!("search window exceeds the server limit of {max_window}"),
            )
        })?
    };
    if limit > max_window as u128 {
        return Err(AppError::public(
            actix_web::http::StatusCode::BAD_REQUEST,
            "SEARCH_WINDOW_TOO_LARGE",
            format!("from + size must not exceed the server limit of {max_window}"),
        ));
    }
    Ok(SearchWindow {
        from: from as usize,
        size: size as usize,
        limit: limit as usize,
    })
}

pub fn validate_tantivy_search_window(from: usize, size: usize) -> Result<SearchWindow, AppError> {
    if from > HARD_MAX_SEARCH_WINDOW {
        return Err(AppError::public(
            actix_web::http::StatusCode::BAD_REQUEST,
            "SEARCH_OFFSET_TOO_LARGE",
            format!("search offset exceeds the server limit of {HARD_MAX_SEARCH_WINDOW}"),
        ));
    }
    let limit = if size == 0 {
        0
    } else {
        from.checked_add(size).ok_or_else(|| {
            AppError::public(
                actix_web::http::StatusCode::BAD_REQUEST,
                "SEARCH_WINDOW_TOO_LARGE",
                format!("search window exceeds the server limit of {HARD_MAX_SEARCH_WINDOW}"),
            )
        })?
    };
    if limit > HARD_MAX_SEARCH_WINDOW {
        return Err(AppError::public(
            actix_web::http::StatusCode::BAD_REQUEST,
            "SEARCH_WINDOW_TOO_LARGE",
            format!("from + size must not exceed the server limit of {HARD_MAX_SEARCH_WINDOW}"),
        ));
    }
    Ok(SearchWindow { from, size, limit })
}

pub(crate) mod generation_lease;
pub mod publication;
#[cfg(feature = "tantivy-search")]
pub mod rebuild;
pub mod resource;
pub mod sqlite;
#[cfg(feature = "tantivy-search")]
pub mod tantivy;
pub mod visibility;

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
    pub path: String,
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

/// Ingest-time sink used by a Bundle's selected search backend.
///
/// The sink owns the backend-specific write path. SQLite writes the legacy
/// segment content and FTS shadow rows; Tantivy streams the cleaned content to
/// its bounded writer and persists only sparse metadata in SQLite.
#[async_trait]
pub trait IngestIndex: Send + Sync {
    async fn commit_ingest_batch(&self, batch: IndexBatch) -> Result<(), AppError>;
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

#[cfg(test)]
mod tests {
    use super::{DEFAULT_MAX_SEARCH_WINDOW, validate_search_window};
    use crate::error::AppError;

    fn error_code(error: AppError) -> &'static str {
        match error {
            AppError::PublicApi { code, .. } => code,
            other => panic!("expected public search-window error, got {other:?}"),
        }
    }

    #[test]
    fn search_window_accepts_normal_and_boundary_pages() {
        assert_eq!(
            validate_search_window(100, 20, DEFAULT_MAX_SEARCH_WINDOW).unwrap(),
            super::SearchWindow {
                from: 100,
                size: 20,
                limit: 120,
            }
        );
        assert_eq!(
            validate_search_window(9_999, 1, DEFAULT_MAX_SEARCH_WINDOW)
                .unwrap()
                .limit,
            10_000
        );
    }

    #[test]
    fn search_window_rejects_offset_and_overflow() {
        assert_eq!(
            error_code(validate_search_window(10_000, 1, DEFAULT_MAX_SEARCH_WINDOW).unwrap_err()),
            "SEARCH_OFFSET_TOO_LARGE"
        );
        assert_eq!(
            error_code(validate_search_window(9_999, 2, DEFAULT_MAX_SEARCH_WINDOW).unwrap_err()),
            "SEARCH_WINDOW_TOO_LARGE"
        );
        assert_eq!(
            error_code(validate_search_window(i64::MAX, 1, DEFAULT_MAX_SEARCH_WINDOW).unwrap_err()),
            "SEARCH_OFFSET_TOO_LARGE"
        );
    }
}

#[cfg(feature = "tantivy-search")]
pub async fn search_tantivy_bundle_visible(
    path: std::path::PathBuf,
    request: ContentSearchRequest,
    visible_file_ids: std::collections::HashSet<i64>,
) -> Result<ContentSearchResult, AppError> {
    tantivy::search_bundle_visible(path, request, visible_file_ids).await
}

#[cfg(feature = "tantivy-search")]
pub(crate) async fn search_tantivy_bundle_visible_with_lease(
    path: std::path::PathBuf,
    request: ContentSearchRequest,
    visible_file_ids: std::collections::HashSet<i64>,
    lease: generation_lease::GenerationLease,
) -> Result<ContentSearchResult, AppError> {
    tantivy::search_bundle_visible_with_lease(path, request, visible_file_ids, lease).await
}

#[cfg(not(feature = "tantivy-search"))]
pub async fn search_tantivy_bundle_visible(
    path: std::path::PathBuf,
    request: ContentSearchRequest,
    visible_file_ids: std::collections::HashSet<i64>,
) -> Result<ContentSearchResult, AppError> {
    let _ = (path, request, visible_file_ids);
    Err(AppError::Config(
        "Tantivy backend requires the tantivy-search feature".into(),
    ))
}

#[cfg(not(feature = "tantivy-search"))]
pub(crate) async fn search_tantivy_bundle_visible_with_lease(
    path: std::path::PathBuf,
    request: ContentSearchRequest,
    visible_file_ids: std::collections::HashSet<i64>,
    lease: generation_lease::GenerationLease,
) -> Result<ContentSearchResult, AppError> {
    let _ = (path, request, visible_file_ids, lease);
    Err(AppError::Config(
        "Tantivy backend requires the tantivy-search feature".into(),
    ))
}
