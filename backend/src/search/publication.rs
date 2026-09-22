//! Durable publication identifiers shared by future backend coordinators.
//! This module does not mark Bundles READY; callers must validate an artifact
//! before updating `bundle_search_indexes` in the same controlled transaction.

use std::path::PathBuf;

use sqlx::SqlitePool;

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
    pub fn parse(value: Option<&str>) -> Result<Self, AppError> {
        match value
            .unwrap_or("sqlite_fts")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "sqlite_fts" | "sqlite" => Ok(Self::SqliteFts),
            "tantivy" => {
                #[cfg(feature = "tantivy-search")]
                {
                    Ok(Self::Tantivy)
                }
                #[cfg(not(feature = "tantivy-search"))]
                {
                    Err(AppError::Config(
                        "RAIN_SEARCH_BACKEND=tantivy requires the tantivy-search feature".into(),
                    ))
                }
            }
            _ => Err(AppError::Config(
                "RAIN_SEARCH_BACKEND must be sqlite_fts or tantivy".into(),
            )),
        }
    }

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

/// Claim the next generation for a non-SQLite publisher. The row remains in
/// BUILDING until the artifact has been committed and reopened successfully.
pub async fn claim_publication(
    pool: &SqlitePool,
    bundle_id: &str,
    backend: SearchBackendKind,
) -> Result<i64, AppError> {
    if backend == SearchBackendKind::SqliteFts {
        return Ok(0);
    }
    crate::db::write::run(
        pool,
        "claim search publication",
        &(bundle_id, backend.as_str()),
        |conn, &(bundle_id, backend)| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT OR IGNORE INTO bundle_search_indexes (bundle_id, backend, state) VALUES (?, 'sqlite_fts', 'BUILDING')",
                )
                .bind(bundle_id)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                let current: Option<(String, String, i64)> = sqlx::query_as(
                    "SELECT backend, state, generation FROM bundle_search_indexes WHERE bundle_id = ?",
                )
                .bind(bundle_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                let generation = current
                    .as_ref()
                    .map(|(_, _, generation)| generation.saturating_add(1))
                    .unwrap_or(1);
                let artifact_key = artifact_relative_path(bundle_id, generation)?
                    .to_string_lossy()
                    .into_owned();
                let result = sqlx::query(
                    "UPDATE bundle_search_indexes SET backend = ?, schema_version = ?, tokenizer_version = ?, generation = ?, artifact_key = ?, state = 'BUILDING', built_at = NULL, last_error_code = NULL, updated_at = CURRENT_TIMESTAMP WHERE bundle_id = ? AND EXISTS (SELECT 1 FROM bundles WHERE id = bundle_search_indexes.bundle_id AND status = 'PROCESSING') AND (state IN ('LEGACY', 'READY', 'FAILED', 'NEEDS_REBUILD') OR (state = 'BUILDING' AND backend = 'sqlite_fts'))",
                )
                .bind(backend)
                .bind(TANTIVY_SCHEMA_VERSION)
                .bind(TANTIVY_TOKENIZER_VERSION)
                .bind(generation)
                .bind(artifact_key)
                .bind(bundle_id)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                if result.rows_affected() != 1 {
                    return Err(AppError::Conflict(
                        "search index publication is already being built".into(),
                    ));
                }
                Ok(generation)
            })
        },
    )
    .await
}

pub async fn mark_publication_ready(
    pool: &SqlitePool,
    bundle_id: &str,
    backend: SearchBackendKind,
    generation: i64,
) -> Result<(), AppError> {
    crate::db::write::run(
        pool,
        "publish search index",
        &(bundle_id, backend.as_str(), generation),
        |conn, &(bundle_id, backend, generation)| {
            Box::pin(async move {
                let result = sqlx::query(
                    "UPDATE bundle_search_indexes SET state = 'READY', built_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE bundle_id = ? AND backend = ? AND generation = ? AND state = 'BUILDING' AND EXISTS (SELECT 1 FROM bundles WHERE id = bundle_search_indexes.bundle_id AND status = 'PROCESSING')",
                )
                .bind(bundle_id)
                .bind(backend)
                .bind(generation)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                if result.rows_affected() != 1 {
                    return Err(AppError::Conflict(
                        "search index publication generation changed while building".into(),
                    ));
                }
                Ok(())
            })
        },
    )
    .await
}

pub async fn mark_publication_failed(
    pool: &SqlitePool,
    bundle_id: &str,
    backend: SearchBackendKind,
    generation: i64,
    error_code: &str,
) {
    let result = crate::db::write::run(
        pool,
        "fail search publication",
        &(bundle_id, backend.as_str(), generation, error_code),
        |conn, &(bundle_id, backend, generation, error_code)| {
            Box::pin(async move {
                sqlx::query(
                    "UPDATE bundle_search_indexes SET state = 'FAILED', last_error_code = ?, updated_at = CURRENT_TIMESTAMP WHERE bundle_id = ? AND backend = ? AND generation = ? AND state = 'BUILDING'",
                )
                .bind(error_code)
                .bind(bundle_id)
                .bind(backend)
                .bind(generation)
                .execute(&mut *conn)
                .await
                .map(|_| ())
                .map_err(AppError::Database)
            })
        },
    )
    .await;
    if let Err(error) = result {
        tracing::warn!(bundle_id, %error, "failed to mark search publication as failed");
    }
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
        assert_eq!(
            SearchBackendKind::parse(Some("sqlite")).unwrap(),
            SearchBackendKind::SqliteFts
        );
    }
}
