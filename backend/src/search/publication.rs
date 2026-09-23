//! Durable publication identifiers shared by future backend coordinators.
//! This module does not mark Bundles READY; callers must validate an artifact
//! before updating `bundle_search_indexes` in the same controlled transaction.

use std::path::PathBuf;

use sqlx::SqlitePool;

use crate::error::AppError;

pub const SQLITE_FTS_SCHEMA_VERSION: i64 = 1;
pub const TANTIVY_SCHEMA_VERSION: i64 = 1;
pub const TANTIVY_TOKENIZER_VERSION: i64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchBackendKind {
    SqliteFts,
    Tantivy,
}

impl SearchBackendKind {
    pub fn parse(value: Option<&str>) -> Result<Self, AppError> {
        let default_backend = if cfg!(feature = "tantivy-search") {
            "tantivy"
        } else {
            "sqlite_fts"
        };
        match value
            .unwrap_or(default_backend)
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

/// v0.1.x is a Tantivy-first release and does not migrate pre-release SQLite
/// search data. Refuse to start against a database that still has a
/// SQLite-backed Bundle so an upgrade cannot silently hide existing results.
pub async fn ensure_fresh_tantivy_data(pool: &SqlitePool) -> Result<(), AppError> {
    let legacy_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM bundle_search_indexes WHERE backend = 'sqlite_fts'",
    )
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;
    if legacy_count > 0 {
        return Err(AppError::Config(
            "v0.1 Tantivy requires a fresh data directory; existing SQLite search data must be removed before startup".into(),
        ));
    }
    Ok(())
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
                    "UPDATE bundle_search_indexes SET backend = ?, schema_version = ?, tokenizer_version = ?, generation = ?, artifact_key = ?, state = 'BUILDING', built_at = NULL, last_error_code = NULL, updated_at = CURRENT_TIMESTAMP WHERE bundle_id = ? AND pending_state='IDLE' AND pending_generation IS NULL AND EXISTS (SELECT 1 FROM bundles WHERE id = bundle_search_indexes.bundle_id AND status = 'PROCESSING') AND (state IN ('LEGACY', 'READY', 'FAILED', 'NEEDS_REBUILD') OR (state = 'BUILDING' AND backend = 'sqlite_fts'))",
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
                sqlx::query(
                    "INSERT OR IGNORE INTO bundle_search_artifacts(bundle_id,generation,state,active_readers,retired_at) VALUES(?,?, 'ACTIVE', 0, NULL)",
                )
                .bind(bundle_id)
                .bind(generation)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
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

#[cfg(feature = "tantivy-search")]
pub(crate) async fn mark_rebuild_failed(
    pool: &SqlitePool,
    claim: &crate::search::rebuild::RebuildClaim,
    error_code: &str,
) {
    let _ = crate::db::write::run(
        pool,
        "fail Tantivy deletion rebuild",
        &(claim.bundle_id.as_str(), claim.target_generation, error_code),
        |conn, (bundle_id, target_generation, error_code)| {
            Box::pin(async move {
                sqlx::query(
                    "UPDATE bundle_search_indexes SET pending_state='FAILED', last_error_code=?, updated_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND pending_generation=? AND pending_state='BUILDING'",
                )
                .bind(error_code)
                .bind(bundle_id)
                .bind(target_generation)
                .execute(&mut *conn)
                .await
                .map(|_| ())
                .map_err(AppError::Database)
            })
        },
    )
    .await;
}

#[cfg(feature = "tantivy-search")]
pub(crate) async fn publish_rebuild(
    pool: &SqlitePool,
    claim: &crate::search::rebuild::RebuildClaim,
) -> Result<(), AppError> {
    let artifact_key = artifact_relative_path(&claim.bundle_id, claim.target_generation)?
        .to_string_lossy()
        .into_owned();
    crate::db::write::run(
        pool,
        "publish Tantivy deletion rebuild",
        &(
            claim.bundle_id.as_str(),
            claim.active_generation,
            claim.target_generation,
            claim.target_revision,
            artifact_key.as_str(),
        ),
        |conn, (bundle_id, active_generation, target_generation, target_revision, artifact_key)| {
            Box::pin(async move {
                let changed = sqlx::query(
                    "UPDATE bundle_search_indexes SET generation=pending_generation, artifact_key=?, compacted_revision=pending_revision, state=CASE WHEN visibility_revision=pending_revision THEN 'READY' ELSE 'NEEDS_REBUILD' END, pending_generation=NULL, pending_revision=NULL, pending_state='IDLE', built_at=CURRENT_TIMESTAMP, last_error_code=NULL, updated_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND backend='tantivy' AND generation=? AND pending_generation=? AND pending_revision=? AND pending_state='BUILDING' AND EXISTS (SELECT 1 FROM bundles b JOIN issues i ON i.code=b.issue_code WHERE b.id=bundle_search_indexes.bundle_id AND b.status='READY' AND i.status='ACTIVE')",
                )
                .bind(artifact_key)
                .bind(bundle_id)
                .bind(active_generation)
                .bind(target_generation)
                .bind(target_revision)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?
                .rows_affected();
                if changed != 1 {
                    return Err(AppError::Conflict(
                        "Tantivy rebuild publication generation changed".into(),
                    ));
                }
                sqlx::query(
                    "INSERT OR IGNORE INTO bundle_search_artifacts(bundle_id,generation,state,retired_at) VALUES(?,?,'ACTIVE',NULL)",
                )
                .bind(bundle_id)
                .bind(active_generation)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                sqlx::query(
                    "UPDATE bundle_search_artifacts SET state='RETIRED', retired_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND generation=? AND state='ACTIVE'",
                )
                .bind(bundle_id)
                .bind(active_generation)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                sqlx::query(
                    "INSERT OR REPLACE INTO bundle_search_artifacts(bundle_id,generation,state,retired_at) VALUES(?,?,'ACTIVE',NULL)",
                )
                .bind(bundle_id)
                .bind(target_generation)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                Ok(())
            })
        },
    )
    .await
}

/// Remove only unpublished generations. READY artifacts are intentionally left
/// untouched so a failed rebuild cannot make an existing Bundle disappear
/// from search.
pub async fn cleanup_unpublished_artifacts(
    pool: &SqlitePool,
    data_root: &std::path::Path,
) -> Result<u64, AppError> {
    let rows: Vec<(String, String, i64, Option<i64>, String)> = sqlx::query_as(
        "SELECT bundle_id, state, generation, pending_generation, pending_state FROM bundle_search_indexes WHERE (pending_state='FAILED' AND pending_generation IS NOT NULL) OR (state IN ('BUILDING','FAILED') AND pending_state='IDLE' AND generation>0) OR (pending_state='CLEANING' AND datetime(updated_at) <= datetime('now','-5 minutes'))",
    )
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    let mut removed = 0_u64;
    for (bundle_id, state, generation, pending_generation, pending_state) in rows {
        let generation_to_remove = pending_generation.unwrap_or(generation);
        let claimed = sqlx::query(
            "UPDATE bundle_search_indexes SET pending_state='CLEANING', updated_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND state=? AND generation=? AND pending_state=? AND ((pending_generation IS NULL AND ? IS NULL) OR pending_generation=?) AND (pending_state <> 'CLEANING' OR datetime(updated_at) <= datetime('now','-5 minutes'))",
        )
        .bind(&bundle_id)
        .bind(&state)
        .bind(generation)
        .bind(&pending_state)
        .bind(pending_generation)
        .bind(generation_to_remove)
        .execute(pool)
        .await
        .map_err(AppError::Database)?
        .rows_affected();
        if claimed != 1 {
            continue;
        }
        let cleanup_removed = match cleanup_publication_artifact(
            data_root,
            &bundle_id,
            generation_to_remove,
        )
        .await
        {
            Ok(removed) => removed,
            Err(error) => {
                sqlx::query(
                    "UPDATE bundle_search_indexes SET pending_state=?, updated_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND pending_state='CLEANING' AND generation=?",
                )
                .bind(if pending_state == "CLEANING" {
                    if pending_generation.is_some() {
                        "FAILED"
                    } else {
                        "IDLE"
                    }
                } else {
                    pending_state.as_str()
                })
                .bind(&bundle_id)
                .bind(generation)
                .execute(pool)
                .await
                .map_err(AppError::Database)?;
                return Err(error);
            }
        };
        if cleanup_removed {
            removed = removed.saturating_add(1);
        }
        if pending_generation.is_some() {
            sqlx::query("UPDATE bundle_search_indexes SET pending_state='IDLE', pending_generation=NULL, pending_revision=NULL WHERE bundle_id=? AND pending_state='CLEANING' AND pending_generation=?")
                .bind(&bundle_id)
                .bind(generation_to_remove)
                .execute(pool)
                .await
                .map_err(AppError::Database)?;
        } else {
            sqlx::query("UPDATE bundle_search_indexes SET state='FAILED', generation=0, artifact_key=NULL, last_error_code='RECOVERED_BUILDING', pending_state='IDLE' WHERE bundle_id=? AND state IN ('BUILDING','FAILED') AND pending_state='CLEANING' AND generation=?")
                .bind(&bundle_id)
                .bind(generation)
                .execute(pool)
                .await
                .map_err(AppError::Database)?;
        }
    }
    Ok(removed)
}

pub async fn cleanup_publication_artifact(
    data_root: &std::path::Path,
    bundle_id: &str,
    generation: i64,
) -> Result<bool, AppError> {
    let path = data_root.join(artifact_relative_path(bundle_id, generation)?);
    match tokio::fs::remove_dir_all(&path).await {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(AppError::Io(error)),
    }
}

pub async fn cleanup_deleted_bundle_artifacts(
    pool: &SqlitePool,
    data_root: &std::path::Path,
) -> Result<u64, AppError> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT bundle_id, generation FROM (SELECT a.bundle_id, a.generation FROM bundle_search_artifacts a JOIN bundles b ON b.id=a.bundle_id WHERE b.status='DELETED' UNION SELECT i.bundle_id, i.generation FROM bundle_search_indexes i JOIN bundles b ON b.id=i.bundle_id WHERE b.status='DELETED' AND i.generation>0)",
    )
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    let mut removed = 0_u64;
    for (bundle_id, generation) in rows {
        let claimed = sqlx::query(
            "UPDATE bundle_search_artifacts SET cleanup_claimed_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND generation=? AND state IN ('ACTIVE','RETIRED') AND active_readers=0 AND (cleanup_claimed_at IS NULL OR datetime(cleanup_claimed_at) <= datetime('now','-5 minutes')) AND EXISTS (SELECT 1 FROM bundles WHERE id=? AND status='DELETED')",
        )
        .bind(&bundle_id)
        .bind(generation)
        .bind(&bundle_id)
        .execute(pool)
        .await
        .map_err(AppError::Database)?
        .rows_affected();
        if claimed == 0 {
            let has_artifact: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM bundle_search_artifacts WHERE bundle_id=? AND generation=?)",
            )
            .bind(&bundle_id)
            .bind(generation)
            .fetch_one(pool)
            .await
            .map_err(AppError::Database)?;
            if has_artifact {
                continue;
            }
        }
        let cleanup_removed = match cleanup_publication_artifact(data_root, &bundle_id, generation)
            .await
        {
            Ok(removed) => removed,
            Err(error) => {
                if claimed == 1 {
                    sqlx::query("UPDATE bundle_search_artifacts SET cleanup_claimed_at=NULL WHERE bundle_id=? AND generation=? AND cleanup_claimed_at IS NOT NULL")
                        .bind(&bundle_id)
                        .bind(generation)
                        .execute(pool)
                        .await
                        .map_err(AppError::Database)?;
                }
                return Err(error);
            }
        };
        if cleanup_removed {
            removed = removed.saturating_add(1);
        }
        sqlx::query("DELETE FROM bundle_search_artifacts WHERE bundle_id=? AND generation=? AND (cleanup_claimed_at IS NOT NULL OR NOT EXISTS (SELECT 1 FROM bundles WHERE id=? AND status <> 'DELETED'))")
            .bind(&bundle_id)
            .bind(generation)
            .bind(&bundle_id)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
    }
    Ok(removed)
}

/// Retired generations are kept briefly after publication so in-flight
/// readers that selected the previous generation can finish safely.
pub async fn cleanup_retired_artifacts(
    pool: &SqlitePool,
    data_root: &std::path::Path,
) -> Result<u64, AppError> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT a.bundle_id, a.generation FROM bundle_search_artifacts a JOIN bundles b ON b.id=a.bundle_id WHERE a.state='RETIRED' AND a.active_readers=0 AND (a.cleanup_claimed_at IS NULL OR datetime(a.cleanup_claimed_at) <= datetime('now','-5 minutes')) AND datetime(a.retired_at) <= datetime('now','-10 minutes') AND b.status <> 'DELETED'",
    )
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    let mut removed = 0_u64;
    for (bundle_id, generation) in rows {
        let claimed = sqlx::query(
            "UPDATE bundle_search_artifacts SET cleanup_claimed_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND generation=? AND state='RETIRED' AND active_readers=0 AND (cleanup_claimed_at IS NULL OR datetime(cleanup_claimed_at) <= datetime('now','-5 minutes')) AND datetime(retired_at) <= datetime('now','-10 minutes')",
        )
        .bind(&bundle_id)
        .bind(generation)
        .execute(pool)
        .await
        .map_err(AppError::Database)?
        .rows_affected();
        if claimed != 1 {
            continue;
        }
        let cleanup_removed = match cleanup_publication_artifact(data_root, &bundle_id, generation)
            .await
        {
            Ok(removed) => removed,
            Err(error) => {
                sqlx::query("UPDATE bundle_search_artifacts SET cleanup_claimed_at=NULL WHERE bundle_id=? AND generation=? AND cleanup_claimed_at IS NOT NULL")
                    .bind(&bundle_id)
                    .bind(generation)
                    .execute(pool)
                    .await
                    .map_err(AppError::Database)?;
                return Err(error);
            }
        };
        if cleanup_removed {
            removed = removed.saturating_add(1);
        }
        sqlx::query("DELETE FROM bundle_search_artifacts WHERE bundle_id=? AND generation=? AND state='RETIRED' AND cleanup_claimed_at IS NOT NULL")
            .bind(&bundle_id)
            .bind(generation)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
    }
    Ok(removed)
}

pub async fn acquire_generation_lease(
    pool: &SqlitePool,
    bundle_id: &str,
    generation: i64,
) -> Result<GenerationLease, AppError> {
    let changed = sqlx::query(
        "UPDATE bundle_search_artifacts SET active_readers=active_readers+1 WHERE bundle_id=? AND generation=? AND cleanup_claimed_at IS NULL AND EXISTS (SELECT 1 FROM bundles b JOIN issues i ON i.code=b.issue_code WHERE b.id=bundle_search_artifacts.bundle_id AND b.status='READY' AND i.status='ACTIVE')",
    )
    .bind(bundle_id)
    .bind(generation)
    .execute(pool)
    .await
    .map_err(AppError::Database)?
    .rows_affected();
    if changed != 1 {
        return Err(AppError::Conflict(
            "search generation is no longer available".into(),
        ));
    }
    Ok(GenerationLease {
        pool: pool.clone(),
        bundle_id: bundle_id.to_owned(),
        generation,
        released: false,
    })
}

#[derive(Debug)]
pub struct GenerationLease {
    pool: SqlitePool,
    bundle_id: String,
    generation: i64,
    released: bool,
}

impl GenerationLease {
    pub async fn release(mut self) -> Result<(), AppError> {
        let result = release_generation_lease(&self.pool, &self.bundle_id, self.generation).await;
        if result.is_ok() {
            self.released = true;
        }
        result
    }
}

impl Drop for GenerationLease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let pool = self.pool.clone();
        let bundle_id = self.bundle_id.clone();
        let generation = self.generation;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(error) = release_generation_lease(&pool, &bundle_id, generation).await {
                    tracing::warn!(%error, bundle_id, generation, "failed to release cancelled search generation lease");
                }
            });
        }
    }
}

pub async fn release_generation_lease(
    pool: &SqlitePool,
    bundle_id: &str,
    generation: i64,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE bundle_search_artifacts SET active_readers=active_readers-1 WHERE bundle_id=? AND generation=? AND active_readers>0",
    )
    .bind(bundle_id)
    .bind(generation)
    .execute(pool)
    .await
    .map_err(AppError::Database)?;
    Ok(())
}

pub async fn reset_generation_leases(pool: &SqlitePool) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE bundle_search_artifacts SET active_readers=0, cleanup_claimed_at=NULL WHERE active_readers <> 0 OR cleanup_claimed_at IS NOT NULL",
    )
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(AppError::Database)
}

#[cfg(test)]
mod tests {
    use super::{
        SearchBackendKind, artifact_relative_path, cleanup_deleted_bundle_artifacts,
        cleanup_retired_artifacts, cleanup_unpublished_artifacts, ensure_fresh_tantivy_data,
    };

    #[cfg(feature = "tantivy-search")]
    #[test]
    fn v01_defaults_to_tantivy() {
        assert_eq!(
            SearchBackendKind::parse(None).unwrap(),
            SearchBackendKind::Tantivy
        );
    }

    #[cfg(not(feature = "tantivy-search"))]
    #[test]
    fn no_feature_build_keeps_sqlite_default_for_tooling() {
        assert_eq!(
            SearchBackendKind::parse(None).unwrap(),
            SearchBackendKind::SqliteFts
        );
    }

    #[tokio::test]
    async fn empty_database_is_valid_for_tantivy_first_startup() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        ensure_fresh_tantivy_data(&pool).await.unwrap();
        pool.close().await;
    }

    #[tokio::test]
    async fn sqlite_bundle_is_rejected_for_v01_tantivy_startup() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name) VALUES('LEGACY','Legacy')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-legacy','LEGACY','hash','legacy','READY')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_indexes(bundle_id,backend,state) VALUES('bundle-legacy','sqlite_fts','LEGACY')")
            .execute(&pool)
            .await
            .unwrap();
        let error = ensure_fresh_tantivy_data(&pool).await.unwrap_err();
        assert!(error.to_string().contains("fresh data directory"));
        pool.close().await;
    }

    #[tokio::test]
    async fn tantivy_only_database_is_valid_for_restart() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name) VALUES('V01','v0.1')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-v01','V01','hash','v0.1','READY')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE bundle_search_indexes SET backend='tantivy', state='READY', generation=1 WHERE bundle_id='bundle-v01'")
            .execute(&pool)
            .await
            .unwrap();
        ensure_fresh_tantivy_data(&pool).await.unwrap();
        pool.close().await;
    }

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

    #[tokio::test]
    async fn deleted_bundle_artifacts_are_removed_during_recovery() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name) VALUES('CLEAN','Cleanup')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-clean','CLEAN','hash-clean','cleanup','DELETED')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_indexes(bundle_id,backend,generation,state) VALUES('bundle-clean','tantivy',1,'READY')")
            .execute(&pool)
            .await
            .unwrap();
        let root = std::env::temp_dir().join(format!(
            "rain-search-cleanup-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let artifact = root.join(artifact_relative_path("bundle-clean", 1).unwrap());
        tokio::fs::create_dir_all(&artifact).await.unwrap();
        let removed = cleanup_deleted_bundle_artifacts(&pool, &root)
            .await
            .unwrap();
        assert_eq!(removed, 1);
        assert!(!artifact.exists());
        pool.close().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn stale_initial_building_publication_is_recoverable() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name) VALUES('BUILDING','Building')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-building','BUILDING','hash','building','PROCESSING')")
            .execute(&pool)
            .await
            .unwrap();
        let generation =
            super::claim_publication(&pool, "bundle-building", SearchBackendKind::Tantivy)
                .await
                .unwrap();
        let root = std::env::temp_dir().join(format!(
            "rain-search-building-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let artifact = root.join(artifact_relative_path("bundle-building", generation).unwrap());
        tokio::fs::create_dir_all(&artifact).await.unwrap();

        let removed = cleanup_unpublished_artifacts(&pool, &root).await.unwrap();
        assert_eq!(removed, 1);
        assert!(!artifact.exists());
        let state: (String, i64) = sqlx::query_as(
            "SELECT state,generation FROM bundle_search_indexes WHERE bundle_id='bundle-building'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, ("FAILED".into(), 0));

        sqlx::query("UPDATE bundles SET status='PROCESSING' WHERE id='bundle-building'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            super::claim_publication(&pool, "bundle-building", SearchBackendKind::Tantivy)
                .await
                .is_ok()
        );
        pool.close().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn retired_cleanup_keeps_artifact_with_reader_lease() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name,status) VALUES('LEASE','Lease','ACTIVE')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-lease','LEASE','hash','lease','READY')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_indexes(bundle_id,backend,generation,state) VALUES('bundle-lease','tantivy',1,'READY')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_artifacts(bundle_id,generation,state) VALUES('bundle-lease',1,'ACTIVE')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE bundle_search_artifacts SET state='RETIRED', active_readers=1, retired_at=datetime('now','-1 day') WHERE bundle_id='bundle-lease' AND generation=1")
            .execute(&pool)
            .await
            .unwrap();
        let root = std::env::temp_dir().join(format!(
            "rain-search-lease-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let artifact = root.join(artifact_relative_path("bundle-lease", 1).unwrap());
        tokio::fs::create_dir_all(&artifact).await.unwrap();

        assert_eq!(cleanup_retired_artifacts(&pool, &root).await.unwrap(), 0);
        assert!(artifact.exists());
        pool.close().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn dropped_generation_lease_is_released_after_cancellation() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query(
            "INSERT INTO issues(code,name,status) VALUES('DROPLEASE','Drop lease','ACTIVE')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-droplease','DROPLEASE','hash','drop lease','READY')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_indexes(bundle_id,backend,generation,state) VALUES('bundle-droplease','tantivy',1,'READY')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_artifacts(bundle_id,generation,state) VALUES('bundle-droplease',1,'ACTIVE')")
            .execute(&pool)
            .await
            .unwrap();

        let lease = super::acquire_generation_lease(&pool, "bundle-droplease", 1)
            .await
            .unwrap();
        drop(lease);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let readers: i64 = sqlx::query_scalar(
            "SELECT active_readers FROM bundle_search_artifacts WHERE bundle_id='bundle-droplease' AND generation=1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(readers, 0);
        pool.close().await;
    }

    #[tokio::test]
    async fn cleanup_claim_blocks_new_generation_leases() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query(
            "INSERT INTO issues(code,name,status) VALUES('CLAIMLEASE','Claim lease','ACTIVE')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-claimlease','CLAIMLEASE','hash','claim lease','READY')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_indexes(bundle_id,backend,generation,state) VALUES('bundle-claimlease','tantivy',1,'READY')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_artifacts(bundle_id,generation,state) VALUES('bundle-claimlease',1,'ACTIVE')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE bundle_search_artifacts SET cleanup_claimed_at=CURRENT_TIMESTAMP WHERE bundle_id='bundle-claimlease' AND generation=1")
            .execute(&pool)
            .await
            .unwrap();

        let error = super::acquire_generation_lease(&pool, "bundle-claimlease", 1)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no longer available"));
        pool.close().await;
    }

    #[tokio::test]
    async fn publication_retry_cannot_claim_cleanup_owned_row() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query(
            "INSERT INTO issues(code,name,status) VALUES('RETRYCLAIM','Retry claim','ACTIVE')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-retryclaim','RETRYCLAIM','hash','retry claim','PROCESSING')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_indexes(bundle_id,backend,generation,state,pending_state) VALUES('bundle-retryclaim','tantivy',1,'FAILED','CLEANING')")
            .execute(&pool)
            .await
            .unwrap();

        let error =
            super::claim_publication(&pool, "bundle-retryclaim", SearchBackendKind::Tantivy)
                .await
                .unwrap_err();
        assert!(error.to_string().contains("already being built"));
        pool.close().await;
    }
}
