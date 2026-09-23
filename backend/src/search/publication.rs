//! Durable publication identifiers shared by future backend coordinators.
//! This module does not mark Bundles READY; callers must validate an artifact
//! before updating `bundle_search_indexes` in the same controlled transaction.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex, OnceLock, Weak},
};

use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::error::AppError;

use super::generation_lease::{
    GenerationLease as InMemoryGenerationLease, GenerationLeaseRegistry,
};

pub const SQLITE_FTS_SCHEMA_VERSION: i64 = 1;
pub const TANTIVY_SCHEMA_VERSION: i64 = 1;
pub const TANTIVY_TOKENIZER_VERSION: i64 = 2;

type BundleLifecycleLock = Mutex<()>;

static BUNDLE_LIFECYCLE_LOCKS: OnceLock<StdMutex<HashMap<String, Weak<BundleLifecycleLock>>>> =
    OnceLock::new();

/// Serialize rebuild publication and filesystem cleanup for one Bundle inside
/// a process. Persisted `CLEANING` state remains the cross-restart recovery
/// boundary when no worker survives to hold this lock.
pub(crate) async fn lock_bundle_lifecycle(bundle_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let locks = BUNDLE_LIFECYCLE_LOCKS.get_or_init(|| StdMutex::new(HashMap::new()));
        let mut locks = locks.lock().expect("bundle lifecycle lock map poisoned");
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(bundle_id).and_then(Weak::upgrade) {
            lock
        } else {
            let lock = Arc::new(BundleLifecycleLock::new(()));
            locks.insert(bundle_id.to_owned(), Arc::downgrade(&lock));
            lock
        }
    };
    lock.lock_owned().await
}

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

async fn remove_directory_if_present(path: &Path) -> Result<bool, AppError> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(AppError::Io(error)),
    }
}

async fn remove_path_if_present(path: &Path) -> Result<bool, AppError> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotADirectory => {
            match tokio::fs::remove_file(path).await {
                Ok(()) => Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(AppError::Io(error)),
            }
        }
        Err(error) => Err(AppError::Io(error)),
    }
}

/// Remove every on-disk location owned by one unpublished generation.
/// Missing locations are expected after a crash and therefore are harmless.
pub(crate) async fn cleanup_generation_artifacts(
    data_root: &Path,
    bundle_id: &str,
    generation: i64,
) -> Result<bool, AppError> {
    let artifact = data_root.join(artifact_relative_path(bundle_id, generation)?);
    let staging = data_root
        .join(".search-rebuild")
        .join(bundle_id)
        .join(generation.to_string());
    let mut removed = remove_directory_if_present(&artifact).await?;
    removed |= remove_directory_if_present(&staging).await?;

    if let Some(parent) = staging.parent() {
        let _ = tokio::fs::remove_dir(parent).await;
    }
    if let Some(parent) = artifact.parent() {
        let _ = tokio::fs::remove_dir(parent).await;
    }
    Ok(removed)
}

async fn cleanup_staging_bundle(data_root: &Path, bundle_id: &str) -> Result<bool, AppError> {
    let path = data_root.join(".search-rebuild").join(bundle_id);
    let removed = remove_directory_if_present(&path).await?;
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::remove_dir(parent).await;
    }
    Ok(removed)
}

/// Remove a pending generation together with every owner-specific staging
/// directory left under that generation after a crash.
async fn cleanup_pending_generation_artifacts(
    data_root: &Path,
    bundle_id: &str,
    generation: i64,
) -> Result<bool, AppError> {
    let artifact = data_root.join(artifact_relative_path(bundle_id, generation)?);
    let staging = data_root
        .join(".search-rebuild")
        .join(bundle_id)
        .join(generation.to_string());
    let mut removed = remove_directory_if_present(&artifact).await?;
    removed |= remove_directory_if_present(&staging).await?;
    if let Some(parent) = staging.parent() {
        let _ = tokio::fs::remove_dir(parent).await;
    }
    if let Some(parent) = artifact.parent() {
        let _ = tokio::fs::remove_dir(parent).await;
    }
    Ok(removed)
}

async fn cleanup_orphan_staging_generations(
    data_root: &Path,
    bundle_id: &str,
    protected: Option<(i64, Option<String>)>,
) -> Result<u64, AppError> {
    let bundle_root = data_root.join(".search-rebuild").join(bundle_id);
    let mut generations = match tokio::fs::read_dir(&bundle_root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(AppError::Io(error)),
    };
    let mut removed = 0_u64;
    while let Some(generation_entry) = generations.next_entry().await.map_err(AppError::Io)? {
        let generation_name = generation_entry.file_name();
        let Ok(generation) = generation_name.to_string_lossy().parse::<i64>() else {
            continue;
        };
        if let Some((protected_generation, protected_owner)) = &protected
            && generation == *protected_generation
        {
            let Some(protected_owner) = protected_owner else {
                continue;
            };
            let mut owners = match tokio::fs::read_dir(generation_entry.path()).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(AppError::Io(error)),
            };
            while let Some(owner_entry) = owners.next_entry().await.map_err(AppError::Io)? {
                if owner_entry.file_name().to_string_lossy() == protected_owner.as_str() {
                    continue;
                }
                if remove_path_if_present(&owner_entry.path()).await? {
                    removed = removed.saturating_add(1);
                }
            }
            let _ = tokio::fs::remove_dir(generation_entry.path()).await;
        } else if remove_directory_if_present(&generation_entry.path()).await? {
            removed = removed.saturating_add(1);
        }
    }
    if tokio::fs::read_dir(&bundle_root).await.is_ok() {
        let _ = tokio::fs::remove_dir(&bundle_root).await;
    }
    if let Some(parent) = bundle_root.parent() {
        let _ = tokio::fs::remove_dir(parent).await;
    }
    Ok(removed)
}

async fn cleanup_orphan_final_generations(
    data_root: &Path,
    bundle_id: &str,
    protected_generations: &HashSet<i64>,
) -> Result<u64, AppError> {
    let bundle_root = data_root.join("search").join(bundle_id);
    let mut entries = match tokio::fs::read_dir(&bundle_root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(AppError::Io(error)),
    };
    let mut removed = 0_u64;
    while let Some(entry) = entries.next_entry().await.map_err(AppError::Io)? {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Ok(generation) = name.parse::<i64>() else {
            continue;
        };
        if generation <= 0 || protected_generations.contains(&generation) {
            continue;
        }
        if remove_directory_if_present(&entry.path()).await? {
            removed = removed.saturating_add(1);
        }
    }
    let _ = tokio::fs::remove_dir(&bundle_root).await;
    if let Some(parent) = bundle_root.parent() {
        let _ = tokio::fs::remove_dir(parent).await;
    }
    Ok(removed)
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

/// Keep a long-running initial publication distinguishable from a crashed one.
/// Cleanup only reclaims BUILDING generations whose heartbeat has gone stale.
pub async fn refresh_publication_heartbeat(
    pool: &SqlitePool,
    bundle_id: &str,
    generation: i64,
) -> Result<(), AppError> {
    let changed = crate::db::write::run(
        pool,
        "heartbeat search publication",
        &(bundle_id, generation),
        |conn, (bundle_id, generation)| {
            Box::pin(async move {
                sqlx::query(
                    "UPDATE bundle_search_indexes SET updated_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND backend='tantivy' AND generation=? AND state='BUILDING' AND pending_state='IDLE' AND pending_generation IS NULL",
                )
                .bind(bundle_id)
                .bind(generation)
                .execute(&mut *conn)
                .await
                .map(|result| result.rows_affected())
                .map_err(AppError::Database)
            })
        },
    )
    .await?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "search publication generation changed while building".into(),
        ));
    }
    Ok(())
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
        &(
            claim.bundle_id.as_str(),
            claim.target_generation,
            claim.claim_token.as_str(),
            error_code,
        ),
        |conn, (bundle_id, target_generation, claim_token, error_code)| {
            Box::pin(async move {
                sqlx::query(
                    "UPDATE bundle_search_indexes SET pending_state='FAILED', pending_phase='BUILDING', last_error_code=?, updated_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND pending_generation=? AND pending_owner=? AND pending_state IN ('BUILDING','CLEANING')",
                )
                .bind(error_code)
                .bind(bundle_id)
                .bind(target_generation)
                .bind(claim_token)
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
pub(crate) async fn rebuild_claim_is_current(
    pool: &SqlitePool,
    claim: &crate::search::rebuild::RebuildClaim,
) -> Result<bool, AppError> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM bundle_search_indexes i JOIN bundles b ON b.id=i.bundle_id JOIN issues issue ON issue.code=b.issue_code WHERE i.bundle_id=? AND i.backend='tantivy' AND b.status='READY' AND issue.status='ACTIVE' AND i.state='NEEDS_REBUILD' AND i.generation=? AND i.visibility_revision>i.compacted_revision AND i.pending_generation=? AND i.pending_revision=? AND i.pending_owner=? AND i.pending_state='BUILDING' AND i.pending_phase IN ('BUILDING','PUBLISHING'))",
    )
    .bind(&claim.bundle_id)
    .bind(claim.active_generation)
    .bind(claim.target_generation)
    .bind(claim.target_revision)
    .bind(&claim.claim_token)
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)
}

#[cfg(feature = "tantivy-search")]
pub(crate) async fn cleanup_rebuild_artifacts_if_owned(
    pool: &SqlitePool,
    data_root: &Path,
    claim: &crate::search::rebuild::RebuildClaim,
) -> Result<bool, AppError> {
    // The owner-specific staging path is never shared with another claim, so
    // it is safe for an obsolete worker to remove its own abandoned staging
    // directory even after a newer worker has taken ownership. The final
    // generation is deliberately left to the durable CLEANING recovery path;
    // removing that shared path here could race a newer publisher.
    let current = rebuild_claim_is_current(pool, claim).await?;
    let staging = data_root
        .join(".search-rebuild")
        .join(&claim.bundle_id)
        .join(claim.target_generation.to_string())
        .join(&claim.claim_token);
    let removed = remove_directory_if_present(&staging).await?;
    Ok(current && removed)
}

#[cfg(feature = "tantivy-search")]
pub(crate) async fn refresh_rebuild_heartbeat(
    pool: &SqlitePool,
    claim: &crate::search::rebuild::RebuildClaim,
) -> Result<bool, AppError> {
    let changed = sqlx::query(
        "UPDATE bundle_search_indexes SET updated_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND pending_generation=? AND pending_revision=? AND pending_owner=? AND pending_state='BUILDING' AND pending_phase IN ('BUILDING','PUBLISHING')",
    )
    .bind(&claim.bundle_id)
    .bind(claim.target_generation)
    .bind(claim.target_revision)
    .bind(&claim.claim_token)
    .execute(pool)
    .await
    .map_err(AppError::Database)?
    .rows_affected();
    Ok(changed == 1)
}

#[cfg(feature = "tantivy-search")]
pub(crate) async fn begin_rebuild_publication(
    pool: &SqlitePool,
    claim: &crate::search::rebuild::RebuildClaim,
) -> Result<bool, AppError> {
    let changed = crate::db::write::run(
        pool,
        "begin Tantivy deletion rebuild publication",
        &(
            claim.bundle_id.as_str(),
            claim.active_generation,
            claim.target_generation,
            claim.target_revision,
            claim.claim_token.as_str(),
        ),
        |conn, (bundle_id, active_generation, target_generation, target_revision, claim_token)| {
            Box::pin(async move {
                let changed = sqlx::query(
                    "UPDATE bundle_search_indexes SET pending_phase='PUBLISHING', updated_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND backend='tantivy' AND generation=? AND pending_generation=? AND pending_revision=? AND pending_owner=? AND pending_state='BUILDING' AND pending_phase='BUILDING' AND EXISTS (SELECT 1 FROM bundles b JOIN issues i ON i.code=b.issue_code WHERE b.id=bundle_search_indexes.bundle_id AND b.status='READY' AND i.status='ACTIVE')",
                )
                .bind(bundle_id)
                .bind(active_generation)
                .bind(target_generation)
                .bind(target_revision)
                .bind(claim_token)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?
                .rows_affected();
                Ok(changed == 1)
            })
        },
    )
    .await?;
    Ok(changed)
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
            claim.claim_token.as_str(),
            artifact_key.as_str(),
        ),
        |conn, (
            bundle_id,
            active_generation,
            target_generation,
            target_revision,
            claim_token,
            artifact_key,
        )| {
            Box::pin(async move {
                let changed = sqlx::query(
                    "UPDATE bundle_search_indexes SET generation=pending_generation, artifact_key=?, compacted_revision=pending_revision, state=CASE WHEN visibility_revision=pending_revision THEN 'READY' ELSE 'NEEDS_REBUILD' END, pending_generation=NULL, pending_revision=NULL, pending_owner=NULL, pending_phase='BUILDING', pending_state='IDLE', built_at=CURRENT_TIMESTAMP, last_error_code=NULL, updated_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND backend='tantivy' AND generation=? AND pending_generation=? AND pending_revision=? AND pending_owner=? AND pending_phase='PUBLISHING' AND pending_state='BUILDING' AND EXISTS (SELECT 1 FROM bundles b JOIN issues i ON i.code=b.issue_code WHERE b.id=bundle_search_indexes.bundle_id AND b.status='READY' AND i.status='ACTIVE')",
                )
                .bind(artifact_key)
                .bind(bundle_id)
                .bind(active_generation)
                .bind(target_generation)
                .bind(target_revision)
                .bind(claim_token)
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
    data_root: &Path,
) -> Result<u64, AppError> {
    let rows: Vec<(String, String, i64, Option<i64>, String)> = sqlx::query_as(
        "SELECT i.bundle_id, i.state, i.generation, i.pending_generation, i.pending_state FROM bundle_search_indexes i JOIN bundles b ON b.id=i.bundle_id WHERE b.status NOT IN ('DELETING','DELETED') AND ((i.pending_state='FAILED' AND i.pending_generation IS NOT NULL) OR (i.pending_state='BUILDING' AND i.pending_phase='PUBLISHING' AND i.pending_generation IS NOT NULL AND datetime(i.updated_at) <= datetime('now','-5 minutes')) OR (i.state='FAILED' AND i.pending_state='IDLE' AND i.generation>0) OR (i.state='BUILDING' AND i.pending_state='IDLE' AND i.generation>0 AND datetime(i.updated_at) <= datetime('now','-5 minutes')) OR (i.pending_state='CLEANING' AND datetime(i.updated_at) <= datetime('now','-5 minutes')))",
    )
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    let mut removed = 0_u64;
    for (bundle_id, state, generation, pending_generation, pending_state) in rows {
        let _lifecycle = lock_bundle_lifecycle(&bundle_id).await;
        let generation_to_remove = pending_generation.unwrap_or(generation);
        let claimed = sqlx::query(
            "UPDATE bundle_search_indexes SET pending_state='CLEANING', updated_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND state=? AND generation=? AND pending_state=? AND ((pending_generation IS NULL AND ? IS NULL) OR pending_generation=?) AND EXISTS (SELECT 1 FROM bundles WHERE id=? AND status <> 'DELETED') AND (pending_state <> 'CLEANING' OR datetime(updated_at) <= datetime('now','-5 minutes'))",
        )
        .bind(&bundle_id)
        .bind(&state)
        .bind(generation)
        .bind(&pending_state)
        .bind(pending_generation)
        .bind(generation_to_remove)
        .bind(&bundle_id)
        .execute(pool)
        .await
        .map_err(AppError::Database)?
        .rows_affected();
        if claimed != 1 {
            continue;
        }
        let cleanup_removed = match if pending_generation.is_some() {
            cleanup_pending_generation_artifacts(data_root, &bundle_id, generation_to_remove).await
        } else {
            cleanup_generation_artifacts(data_root, &bundle_id, generation_to_remove).await
        } {
            Ok(removed) => removed,
            Err(error) => return Err(error),
        };
        if cleanup_removed {
            removed = removed.saturating_add(1);
        }
        if pending_generation.is_some() {
            sqlx::query("UPDATE bundle_search_indexes SET pending_state='IDLE', pending_generation=NULL, pending_revision=NULL, pending_owner=NULL, pending_phase='BUILDING' WHERE bundle_id=? AND pending_state='CLEANING' AND pending_generation=?")
                .bind(&bundle_id)
                .bind(generation_to_remove)
                .execute(pool)
                .await
                .map_err(AppError::Database)?;
        } else {
            sqlx::query("UPDATE bundle_search_indexes SET state='FAILED', generation=0, artifact_key=NULL, last_error_code='RECOVERED_BUILDING', pending_owner=NULL, pending_phase='BUILDING', pending_state='IDLE' WHERE bundle_id=? AND state IN ('BUILDING','FAILED') AND pending_state='CLEANING' AND generation=?")
                .bind(&bundle_id)
                .bind(generation)
                .execute(pool)
                .await
                .map_err(AppError::Database)?;
        }
    }

    let bundle_ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM bundles WHERE status NOT IN ('DELETING','DELETED')")
            .fetch_all(pool)
            .await
            .map_err(AppError::Database)?;
    for bundle_id in bundle_ids {
        let _lifecycle = lock_bundle_lifecycle(&bundle_id).await;
        let pending: Option<(i64, Option<i64>, Option<String>, String)> = sqlx::query_as(
            "SELECT generation,pending_generation,pending_owner,pending_state FROM bundle_search_indexes WHERE bundle_id=? AND backend='tantivy'",
        )
        .bind(&bundle_id)
        .fetch_optional(pool)
        .await
        .map_err(AppError::Database)?;
        let protected = pending.map(|(generation, pending_generation, owner, state)| {
            if let Some(pending_generation) = pending_generation
                && (state == "BUILDING" || state == "PUBLISHING")
            {
                (pending_generation, owner)
            } else {
                // A new claim always targets active_generation + 1. Protect
                // that predicted directory while the DB snapshot and the
                // filesystem scan are not one atomic operation.
                (generation.saturating_add(1), None)
            }
        });
        removed = removed.saturating_add(
            cleanup_orphan_staging_generations(data_root, &bundle_id, protected).await?,
        );
    }
    Ok(removed)
}

pub async fn cleanup_publication_artifact(
    data_root: &Path,
    bundle_id: &str,
    generation: i64,
) -> Result<bool, AppError> {
    remove_directory_if_present(&data_root.join(artifact_relative_path(bundle_id, generation)?))
        .await
}

pub async fn cleanup_deleted_bundle_artifacts(
    pool: &SqlitePool,
    data_root: &Path,
) -> Result<u64, AppError> {
    let registry = GenerationLeaseRegistry::shared();
    cleanup_deleted_bundle_artifacts_with_registry(pool, data_root, &registry).await
}

pub(crate) async fn cleanup_deleted_bundle_artifacts_with_registry(
    pool: &SqlitePool,
    data_root: &Path,
    registry: &GenerationLeaseRegistry,
) -> Result<u64, AppError> {
    let bundle_ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM bundles WHERE status='DELETED'")
            .fetch_all(pool)
            .await
            .map_err(AppError::Database)?;
    let mut removed = 0_u64;
    for bundle_id in bundle_ids {
        let _lifecycle = lock_bundle_lifecycle(&bundle_id).await;
        let index: Option<(i64, Option<i64>, String, Option<String>)> = sqlx::query_as(
            "SELECT generation,pending_generation,pending_state,pending_owner FROM bundle_search_indexes WHERE bundle_id=? AND backend='tantivy'",
        )
        .bind(&bundle_id)
        .fetch_optional(pool)
        .await
        .map_err(AppError::Database)?;
        let artifact_rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT generation,CASE WHEN cleanup_claimed_at IS NOT NULL AND datetime(cleanup_claimed_at) > datetime('now','-5 minutes') THEN 1 ELSE 0 END FROM bundle_search_artifacts WHERE bundle_id=?",
        )
        .bind(&bundle_id)
        .fetch_all(pool)
        .await
        .map_err(AppError::Database)?;

        let pending_generation = index.as_ref().and_then(|(_, pending, _, _)| *pending);
        if let Some(pending_generation) = pending_generation {
            sqlx::query(
                "UPDATE bundle_search_indexes SET pending_state='CLEANING', updated_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND backend='tantivy' AND pending_generation=? AND pending_state IN ('BUILDING','FAILED','CLEANING') AND EXISTS (SELECT 1 FROM bundles WHERE id=? AND status='DELETED')",
            )
            .bind(&bundle_id)
            .bind(pending_generation)
            .bind(&bundle_id)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
            if cleanup_pending_generation_artifacts(data_root, &bundle_id, pending_generation)
                .await?
            {
                removed = removed.saturating_add(1);
            }
            sqlx::query(
                "UPDATE bundle_search_indexes SET pending_generation=NULL, pending_revision=NULL, pending_owner=NULL, pending_phase='BUILDING', pending_state='IDLE', updated_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND backend='tantivy' AND pending_generation=? AND pending_state='CLEANING'",
            )
            .bind(&bundle_id)
            .bind(pending_generation)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
        }

        let mut generations = HashSet::new();
        let mut protected_generations = HashSet::new();
        let mut unremoved_artifacts = HashSet::new();
        if let Some((generation, _, _, _)) = index
            && generation > 0
        {
            generations.insert(generation);
        }
        for (generation, cleanup_claimed) in artifact_rows {
            generations.insert(generation);
            unremoved_artifacts.insert(generation);
            if cleanup_claimed != 0 {
                protected_generations.insert(generation);
            }
        }
        if let Some(pending_generation) = pending_generation {
            generations.remove(&pending_generation);
            protected_generations.remove(&pending_generation);
        }

        for generation in generations {
            if protected_generations.contains(&generation) {
                continue;
            }
            let Some(_cleanup_lease) = registry.try_claim_cleanup(&bundle_id, generation) else {
                protected_generations.insert(generation);
                continue;
            };
            let claimed = sqlx::query(
                "UPDATE bundle_search_artifacts SET cleanup_claimed_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND generation=? AND state IN ('ACTIVE','RETIRED') AND (cleanup_claimed_at IS NULL OR datetime(cleanup_claimed_at) <= datetime('now','-5 minutes')) AND EXISTS (SELECT 1 FROM bundles WHERE id=? AND status='DELETED')",
            )
            .bind(&bundle_id)
            .bind(generation)
            .bind(&bundle_id)
            .execute(pool)
            .await
            .map_err(AppError::Database)?
            .rows_affected();
            let has_artifact: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM bundle_search_artifacts WHERE bundle_id=? AND generation=?)",
            )
            .bind(&bundle_id)
            .bind(generation)
            .fetch_one(pool)
            .await
            .map_err(AppError::Database)?;
            if claimed == 0 && has_artifact {
                protected_generations.insert(generation);
                continue;
            }
            let cleanup_removed = match cleanup_generation_artifacts(
                data_root, &bundle_id, generation,
            )
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
            if claimed == 1 {
                sqlx::query("DELETE FROM bundle_search_artifacts WHERE bundle_id=? AND generation=? AND cleanup_claimed_at IS NOT NULL")
                    .bind(&bundle_id)
                    .bind(generation)
                    .execute(pool)
                    .await
                    .map_err(AppError::Database)?;
                unremoved_artifacts.remove(&generation);
            }
        }

        protected_generations.extend(unremoved_artifacts);

        removed = removed.saturating_add(
            cleanup_orphan_final_generations(data_root, &bundle_id, &protected_generations).await?,
        );
        if cleanup_staging_bundle(data_root, &bundle_id).await? {
            removed = removed.saturating_add(1);
        }
        sqlx::query(
            "UPDATE bundle_search_indexes SET state='FAILED', generation=0, artifact_key=NULL, pending_generation=NULL, pending_revision=NULL, pending_owner=NULL, pending_phase='BUILDING', pending_state='IDLE', last_error_code='RECOVERED_DELETED', updated_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND backend='tantivy'",
        )
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
    let registry = GenerationLeaseRegistry::shared();
    cleanup_retired_artifacts_with_registry(pool, data_root, &registry).await
}

pub(crate) async fn cleanup_retired_artifacts_with_registry(
    pool: &SqlitePool,
    data_root: &std::path::Path,
    registry: &GenerationLeaseRegistry,
) -> Result<u64, AppError> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT a.bundle_id, a.generation FROM bundle_search_artifacts a JOIN bundles b ON b.id=a.bundle_id WHERE a.state='RETIRED' AND (a.cleanup_claimed_at IS NULL OR datetime(a.cleanup_claimed_at) <= datetime('now','-5 minutes')) AND datetime(a.retired_at) <= datetime('now','-10 minutes') AND b.status <> 'DELETED'",
    )
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    let mut removed = 0_u64;
    for (bundle_id, generation) in rows {
        let Some(cleanup_lease) = registry.try_claim_cleanup(&bundle_id, generation) else {
            continue;
        };
        let claimed = sqlx::query(
            "UPDATE bundle_search_artifacts SET cleanup_claimed_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND generation=? AND state='RETIRED' AND (cleanup_claimed_at IS NULL OR datetime(cleanup_claimed_at) <= datetime('now','-5 minutes')) AND datetime(retired_at) <= datetime('now','-10 minutes')",
        )
        .bind(&bundle_id)
        .bind(generation)
        .execute(pool)
        .await
        .map_err(AppError::Database)?
        .rows_affected();
        if claimed != 1 {
            drop(cleanup_lease);
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
    let registry = GenerationLeaseRegistry::shared();
    let lease =
        acquire_generation_lease_with_registry(&registry, pool, bundle_id, generation).await?;
    Ok(GenerationLease { inner: lease })
}

/// Compatibility no-op for callers that used to release a SQLite counter.
/// The RAII guard returned by `acquire_generation_lease` now owns release.
pub async fn release_generation_lease(
    _pool: &SqlitePool,
    _bundle_id: &str,
    _generation: i64,
) -> Result<(), AppError> {
    Ok(())
}

#[derive(Debug)]
pub struct GenerationLease {
    inner: InMemoryGenerationLease,
}

impl GenerationLease {
    pub async fn release(self) -> Result<(), AppError> {
        drop(self);
        Ok(())
    }
}

impl Drop for GenerationLease {
    fn drop(&mut self) {
        let _ = &self.inner;
    }
}

pub(crate) async fn acquire_generation_lease_with_registry(
    registry: &GenerationLeaseRegistry,
    pool: &SqlitePool,
    bundle_id: &str,
    generation: i64,
) -> Result<InMemoryGenerationLease, AppError> {
    let lease = registry
        .try_acquire(bundle_id, generation)
        .ok_or_else(|| AppError::Conflict("search generation is no longer available".into()))?;
    let available: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM bundle_search_artifacts a JOIN bundles b ON b.id=a.bundle_id JOIN issues i ON i.code=b.issue_code WHERE a.bundle_id=? AND a.generation=? AND a.state IN ('ACTIVE','RETIRED') AND a.cleanup_claimed_at IS NULL AND b.status='READY' AND i.status='ACTIVE')",
    )
    .bind(bundle_id)
    .bind(generation)
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;
    if !available {
        drop(lease);
        return Err(AppError::Conflict(
            "search generation is no longer available".into(),
        ));
    }
    Ok(lease)
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
        cleanup_retired_artifacts_with_registry, cleanup_unpublished_artifacts,
        ensure_fresh_tantivy_data,
    };
    use crate::search::generation_lease::GenerationLeaseRegistry;

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
    async fn deleted_bundle_reclaims_pending_generation_and_staging_idempotently() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name,status) VALUES('PENDINGDELETE','Pending delete','ACTIVE')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-pending-delete','PENDINGDELETE','hash','pending delete','DELETED')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_indexes(bundle_id,backend,generation,state,pending_generation,pending_revision,pending_state) VALUES('bundle-pending-delete','tantivy',3,'READY',4,7,'BUILDING')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_artifacts(bundle_id,generation,state) VALUES('bundle-pending-delete',2,'RETIRED'),('bundle-pending-delete',3,'ACTIVE')")
            .execute(&pool)
            .await
            .unwrap();

        let root = std::env::temp_dir().join(format!(
            "rain-search-pending-delete-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let retired = root.join(artifact_relative_path("bundle-pending-delete", 2).unwrap());
        let active = root.join(artifact_relative_path("bundle-pending-delete", 3).unwrap());
        let pending = root.join(artifact_relative_path("bundle-pending-delete", 4).unwrap());
        let staging = root
            .join(".search-rebuild")
            .join("bundle-pending-delete")
            .join("4");
        for path in [&retired, &active, &pending, &staging] {
            tokio::fs::create_dir_all(path).await.unwrap();
        }

        let removed = cleanup_deleted_bundle_artifacts(&pool, &root)
            .await
            .unwrap();
        assert_eq!(removed, 3);
        assert!(!retired.exists());
        assert!(!active.exists());
        assert!(!pending.exists());
        assert!(!staging.exists());
        let metadata: (i64, Option<i64>, Option<i64>, String) = sqlx::query_as(
            "SELECT generation,pending_generation,pending_revision,pending_state FROM bundle_search_indexes WHERE bundle_id='bundle-pending-delete'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(metadata, (0, None, None, "IDLE".into()));

        assert_eq!(
            cleanup_deleted_bundle_artifacts(&pool, &root)
                .await
                .unwrap(),
            0
        );
        pool.close().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn deleted_bundle_pending_cleanup_does_not_remove_leased_artifact() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query(
            "INSERT INTO issues(code,name,status) VALUES('LEASEDELETE','Lease delete','ACTIVE')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-lease-delete','LEASEDELETE','hash','lease delete','DELETED')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_indexes(bundle_id,backend,generation,state,pending_generation,pending_revision,pending_state) VALUES('bundle-lease-delete','tantivy',3,'READY',4,7,'BUILDING')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_artifacts(bundle_id,generation,state,active_readers) VALUES('bundle-lease-delete',3,'ACTIVE',1)")
            .execute(&pool)
            .await
            .unwrap();

        let root = std::env::temp_dir().join(format!(
            "rain-search-lease-delete-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let active = root.join(artifact_relative_path("bundle-lease-delete", 3).unwrap());
        let pending = root.join(artifact_relative_path("bundle-lease-delete", 4).unwrap());
        let staging = root
            .join(".search-rebuild")
            .join("bundle-lease-delete")
            .join("4");
        for path in [&active, &pending, &staging] {
            tokio::fs::create_dir_all(path).await.unwrap();
        }
        let registry = GenerationLeaseRegistry::new();
        let reader = registry
            .try_acquire("bundle-lease-delete", 3)
            .expect("reader lease");

        assert_eq!(
            super::cleanup_deleted_bundle_artifacts_with_registry(&pool, &root, &registry)
                .await
                .unwrap(),
            1
        );
        assert!(active.exists());
        assert!(!pending.exists());
        assert!(!staging.exists());
        let pending_state: (Option<i64>, Option<i64>, String) = sqlx::query_as(
            "SELECT pending_generation,pending_revision,pending_state FROM bundle_search_indexes WHERE bundle_id='bundle-lease-delete'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pending_state, (None, None, "IDLE".into()));

        drop(reader);
        assert_eq!(
            super::cleanup_deleted_bundle_artifacts_with_registry(&pool, &root, &registry)
                .await
                .unwrap(),
            1
        );
        assert!(!active.exists());
        pool.close().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[cfg(feature = "tantivy-search")]
    #[tokio::test]
    async fn rebuild_claim_is_invalidated_when_bundle_is_deleted() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query(
            "INSERT INTO issues(code,name,status) VALUES('CLAIMDELETE','Claim delete','ACTIVE')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-claim-delete','CLAIMDELETE','hash','claim delete','READY')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_indexes(bundle_id,backend,generation,state,visibility_revision,compacted_revision,pending_generation,pending_revision,pending_owner,pending_state) VALUES('bundle-claim-delete','tantivy',1,'NEEDS_REBUILD',2,1,2,2,'test-owner','BUILDING')")
            .execute(&pool)
            .await
            .unwrap();
        let claim = crate::search::rebuild::RebuildClaim {
            bundle_id: "bundle-claim-delete".into(),
            active_generation: 1,
            target_generation: 2,
            target_revision: 2,
            claim_token: "test-owner".into(),
        };
        assert!(
            super::rebuild_claim_is_current(&pool, &claim)
                .await
                .unwrap()
        );
        sqlx::query("UPDATE bundles SET status='DELETED' WHERE id='bundle-claim-delete'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !super::rebuild_claim_is_current(&pool, &claim)
                .await
                .unwrap()
        );
        pool.close().await;
    }

    #[cfg(feature = "tantivy-search")]
    #[tokio::test]
    async fn stale_rebuild_failure_does_not_remove_a_published_target() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query(
            "INSERT INTO issues(code,name,status) VALUES('STALEPUBLISH','Stale publish','ACTIVE')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-stale-publish','STALEPUBLISH','hash','stale publish','READY')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_indexes(bundle_id,backend,generation,state,visibility_revision,compacted_revision) VALUES('bundle-stale-publish','tantivy',2,'READY',2,2)")
            .execute(&pool)
            .await
            .unwrap();
        let claim = crate::search::rebuild::RebuildClaim {
            bundle_id: "bundle-stale-publish".into(),
            active_generation: 1,
            target_generation: 2,
            target_revision: 2,
            claim_token: "test-owner".into(),
        };
        let root = std::env::temp_dir().join(format!(
            "rain-search-stale-publish-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let target = root.join(artifact_relative_path("bundle-stale-publish", 2).unwrap());
        tokio::fs::create_dir_all(&target).await.unwrap();

        assert!(
            !super::cleanup_rebuild_artifacts_if_owned(&pool, &root, &claim)
                .await
                .unwrap()
        );
        assert!(target.exists());
        pool.close().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[cfg(feature = "tantivy-search")]
    #[tokio::test]
    async fn cleanup_unpublished_reclaims_orphaned_rebuild_owner_staging() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query(
            "INSERT INTO issues(code,name,status) VALUES('ORPHANSTAGING','Orphan staging','ACTIVE')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-orphan-staging','ORPHANSTAGING','hash','orphan staging','READY')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_indexes(bundle_id,backend,generation,state) VALUES('bundle-orphan-staging','tantivy',2,'READY')")
            .execute(&pool)
            .await
            .unwrap();
        let root = std::env::temp_dir().join(format!(
            "rain-search-orphan-staging-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let staging = root
            .join(".search-rebuild")
            .join("bundle-orphan-staging")
            .join("2")
            .join("old-owner");
        tokio::fs::create_dir_all(&staging).await.unwrap();
        assert_eq!(
            cleanup_unpublished_artifacts(&pool, &root).await.unwrap(),
            1
        );
        assert!(!staging.exists());
        pool.close().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[cfg(feature = "tantivy-search")]
    #[tokio::test]
    async fn cleanup_unpublished_recovers_a_crashed_publishing_phase() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query(
            "INSERT INTO issues(code,name,status) VALUES('PUBLISHCRASH','Publish crash','ACTIVE')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-publish-crash','PUBLISHCRASH','hash','publish crash','READY')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_indexes(bundle_id,backend,generation,state,visibility_revision,compacted_revision,pending_generation,pending_revision,pending_owner,pending_phase,pending_state,updated_at) VALUES('bundle-publish-crash','tantivy',1,'NEEDS_REBUILD',2,1,2,2,'owner-crashed','PUBLISHING','BUILDING',datetime('now','-10 minutes'))")
            .execute(&pool)
            .await
            .unwrap();
        let root = std::env::temp_dir().join(format!(
            "rain-search-publishing-crash-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let final_path = root.join(artifact_relative_path("bundle-publish-crash", 2).unwrap());
        let staging = root
            .join(".search-rebuild")
            .join("bundle-publish-crash")
            .join("2")
            .join("owner-crashed");
        tokio::fs::create_dir_all(&final_path).await.unwrap();
        tokio::fs::create_dir_all(&staging).await.unwrap();

        assert_eq!(
            cleanup_unpublished_artifacts(&pool, &root).await.unwrap(),
            1
        );
        assert!(!final_path.exists());
        assert!(!staging.exists());
        let pending: (Option<i64>, Option<String>, String) = sqlx::query_as(
            "SELECT pending_generation,pending_owner,pending_state FROM bundle_search_indexes WHERE bundle_id='bundle-publish-crash'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pending, (None, None, "IDLE".to_owned()));
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
        let staging = root
            .join(".search-rebuild")
            .join("bundle-building")
            .join(generation.to_string());
        tokio::fs::create_dir_all(&staging).await.unwrap();
        sqlx::query("UPDATE bundle_search_indexes SET updated_at=datetime('now','-10 minutes') WHERE bundle_id='bundle-building'")
            .execute(&pool)
            .await
            .unwrap();

        let removed = cleanup_unpublished_artifacts(&pool, &root).await.unwrap();
        assert_eq!(removed, 1);
        assert!(!artifact.exists());
        assert!(!staging.exists());
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
    async fn active_initial_building_publication_is_not_cleaned() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name) VALUES('ACTIVEBUILD','Active build')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-activebuild','ACTIVEBUILD','hash','active build','PROCESSING')")
            .execute(&pool)
            .await
            .unwrap();
        let generation =
            super::claim_publication(&pool, "bundle-activebuild", SearchBackendKind::Tantivy)
                .await
                .unwrap();
        let root = std::env::temp_dir().join(format!(
            "rain-search-active-building-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let artifact = root.join(artifact_relative_path("bundle-activebuild", generation).unwrap());
        tokio::fs::create_dir_all(&artifact).await.unwrap();

        assert_eq!(
            cleanup_unpublished_artifacts(&pool, &root).await.unwrap(),
            0
        );
        assert!(artifact.exists());
        let state: (String, i64, String) = sqlx::query_as(
            "SELECT state,generation,pending_state FROM bundle_search_indexes WHERE bundle_id='bundle-activebuild'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, ("BUILDING".into(), generation, "IDLE".into()));
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
        let registry = GenerationLeaseRegistry::new();
        let reader =
            super::acquire_generation_lease_with_registry(&registry, &pool, "bundle-lease", 1)
                .await
                .unwrap();
        let root = std::env::temp_dir().join(format!(
            "rain-search-lease-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let artifact = root.join(artifact_relative_path("bundle-lease", 1).unwrap());
        tokio::fs::create_dir_all(&artifact).await.unwrap();

        assert_eq!(
            cleanup_retired_artifacts_with_registry(&pool, &root, &registry)
                .await
                .unwrap(),
            0
        );
        assert!(artifact.exists());
        drop(reader);
        assert_eq!(
            cleanup_retired_artifacts_with_registry(&pool, &root, &registry)
                .await
                .unwrap(),
            1
        );
        assert!(!artifact.exists());
        pool.close().await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn dropped_generation_lease_does_not_write_sqlite_counter() {
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
        sqlx::query("UPDATE bundle_search_artifacts SET active_readers=7 WHERE bundle_id='bundle-droplease' AND generation=1")
            .execute(&pool)
            .await
            .unwrap();

        let registry = GenerationLeaseRegistry::new();
        let lease =
            super::acquire_generation_lease_with_registry(&registry, &pool, "bundle-droplease", 1)
                .await
                .unwrap();
        drop(lease);
        let readers: i64 = sqlx::query_scalar(
            "SELECT active_readers FROM bundle_search_artifacts WHERE bundle_id='bundle-droplease' AND generation=1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(readers, 7);
        let cleanup = registry
            .try_claim_cleanup("bundle-droplease", 1)
            .expect("dropped lease releases the in-memory reader");
        drop(cleanup);
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

        let registry = GenerationLeaseRegistry::new();
        let error =
            super::acquire_generation_lease_with_registry(&registry, &pool, "bundle-claimlease", 1)
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

    #[tokio::test]
    async fn publication_heartbeat_refreshes_a_building_generation() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query(
            "INSERT INTO issues(code,name,status) VALUES('HEARTBEAT','Heartbeat','ACTIVE')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-heartbeat','HEARTBEAT','hash','heartbeat','PROCESSING')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_indexes(bundle_id,backend,generation,state,updated_at) VALUES('bundle-heartbeat','tantivy',1,'BUILDING',datetime('now','-10 minutes'))")
            .execute(&pool)
            .await
            .unwrap();

        super::refresh_publication_heartbeat(&pool, "bundle-heartbeat", 1)
            .await
            .unwrap();
        let stale: i64 = sqlx::query_scalar(
            "SELECT datetime(updated_at) <= datetime('now','-5 minutes') FROM bundle_search_indexes WHERE bundle_id='bundle-heartbeat'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stale, 0);
        pool.close().await;
    }

    #[cfg(feature = "tantivy-search")]
    #[tokio::test]
    async fn rebuild_heartbeat_refreshes_only_the_current_owner() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query(
            "INSERT INTO issues(code,name,status) VALUES('REBUILDHEARTBEAT','Rebuild heartbeat','ACTIVE')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle-rebuild-heartbeat','REBUILDHEARTBEAT','hash','rebuild heartbeat','READY')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundle_search_indexes(bundle_id,backend,generation,state,visibility_revision,compacted_revision,pending_generation,pending_revision,pending_owner,pending_state,updated_at) VALUES('bundle-rebuild-heartbeat','tantivy',1,'NEEDS_REBUILD',2,1,2,2,'owner-a','BUILDING',datetime('now','-10 minutes'))")
            .execute(&pool)
            .await
            .unwrap();
        let claim = crate::search::rebuild::RebuildClaim {
            bundle_id: "bundle-rebuild-heartbeat".into(),
            active_generation: 1,
            target_generation: 2,
            target_revision: 2,
            claim_token: "owner-a".into(),
        };
        assert!(
            super::refresh_rebuild_heartbeat(&pool, &claim)
                .await
                .unwrap()
        );
        let stale: i64 = sqlx::query_scalar(
            "SELECT datetime(updated_at) <= datetime('now','-5 minutes') FROM bundle_search_indexes WHERE bundle_id='bundle-rebuild-heartbeat'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stale, 0);
        claim_token_mismatch_does_not_refresh(&pool, &claim).await;
        pool.close().await;
    }

    #[cfg(feature = "tantivy-search")]
    async fn claim_token_mismatch_does_not_refresh(
        pool: &sqlx::SqlitePool,
        claim: &crate::search::rebuild::RebuildClaim,
    ) {
        let mut stale_claim = claim.clone();
        stale_claim.claim_token = "owner-b".into();
        assert!(
            !super::refresh_rebuild_heartbeat(pool, &stale_claim)
                .await
                .unwrap()
        );
    }
}
