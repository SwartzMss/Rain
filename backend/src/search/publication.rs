//! Durable publication identifiers shared by future backend coordinators.
//! This module does not mark Bundles READY; callers must validate an artifact
//! before updating `bundle_search_indexes` in the same controlled transaction.

use std::path::PathBuf;

use crate::error::AppError;

pub const SQLITE_FTS_SCHEMA_VERSION: i64 = 1;
pub const TANTIVY_SCHEMA_VERSION: i64 = 1;
pub const TANTIVY_TOKENIZER_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchBackendKind {
    SqliteFts,
    Tantivy,
}

impl SearchBackendKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SqliteFts => "sqlite_fts",
            Self::Tantivy => "tantivy",
        }
    }
}

/// Build an artifact key from the immutable internal Bundle id and generation.
/// Issue codes and user filenames are intentionally excluded from this path.
pub fn artifact_relative_path(bundle_id: &str, generation: i64) -> Result<PathBuf, AppError> {
    if generation < 0
        || bundle_id.is_empty()
        || !bundle_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(AppError::Config("invalid search artifact identity".into()));
    }
    Ok(PathBuf::from("search")
        .join(bundle_id)
        .join(generation.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{SearchBackendKind, artifact_relative_path};

    #[test]
    fn artifact_paths_use_only_internal_ids_and_generations() {
        assert_eq!(
            artifact_relative_path("bundle-01", 3).unwrap(),
            std::path::PathBuf::from("search/bundle-01/3")
        );
        assert!(artifact_relative_path("../escape", 1).is_err());
        assert!(artifact_relative_path("bundle", -1).is_err());
        assert_eq!(SearchBackendKind::Tantivy.as_str(), "tantivy");
    }
}
