#![cfg(feature = "tantivy-search")]

use std::{collections::HashSet, path::Path, time::Duration};

use sqlx::SqlitePool;
use tokio::{fs, time};
use uuid::Uuid;

use crate::{
    error::AppError,
    search::{
        publication::{self, artifact_relative_path},
        tantivy,
        visibility::snapshot_file_ids,
    },
};

use super::resource::SearchResourceBudget;

#[derive(Debug, Clone)]
pub(crate) struct RebuildClaim {
    pub(crate) bundle_id: String,
    pub(crate) active_generation: i64,
    pub(crate) target_generation: i64,
    pub(crate) target_revision: i64,
    pub(crate) claim_token: String,
}

/// Run at most one deletion compaction. The active generation remains READY
/// for readers while the new generation is built.
pub async fn run_once(
    pool: &SqlitePool,
    data_root: &Path,
    budget: SearchResourceBudget,
) -> Result<(), AppError> {
    let Some(claim) = claim_next(pool).await? else {
        return Ok(());
    };
    let _lifecycle = publication::lock_bundle_lifecycle(&claim.bundle_id).await;
    if !publication::rebuild_claim_is_current(pool, &claim).await? {
        return Ok(());
    }
    let lease = match publication::acquire_generation_lease(
        pool,
        &claim.bundle_id,
        claim.active_generation,
    )
    .await
    {
        Ok(lease) => lease,
        Err(error) => {
            let _ = publication::cleanup_rebuild_artifacts_if_owned(pool, data_root, &claim).await;
            publication::mark_rebuild_failed(pool, &claim, error_code(&error)).await;
            return Err(error);
        }
    };
    let heartbeat_pool = pool.clone();
    let heartbeat_claim = claim.clone();
    let heartbeat = tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            match publication::refresh_rebuild_heartbeat(&heartbeat_pool, &heartbeat_claim).await {
                Ok(true) => {}
                Ok(false) | Err(_) => break,
            }
        }
    });
    let visible = match snapshot_file_ids(pool, &claim.bundle_id).await {
        Ok(visible) => visible,
        Err(error) => {
            heartbeat.abort();
            let _ = publication::cleanup_rebuild_artifacts_if_owned(pool, data_root, &claim).await;
            publication::mark_rebuild_failed(pool, &claim, error_code(&error)).await;
            let _ = lease.release().await;
            return Err(error);
        }
    };
    let source = data_root.join(artifact_relative_path(
        &claim.bundle_id,
        claim.active_generation,
    )?);
    let final_path = data_root.join(artifact_relative_path(
        &claim.bundle_id,
        claim.target_generation,
    )?);
    let staging = data_root
        .join(".search-rebuild")
        .join(&claim.bundle_id)
        .join(claim.target_generation.to_string())
        .join(&claim.claim_token);
    let result =
        build_and_publish(pool, &claim, source, staging, final_path, visible, budget).await;
    heartbeat.abort();
    if let Err(error) = &result {
        if let Err(cleanup_error) =
            publication::cleanup_rebuild_artifacts_if_owned(pool, data_root, &claim).await
        {
            tracing::warn!(
                bundle_id = %claim.bundle_id,
                generation = claim.target_generation,
                %cleanup_error,
                "failed to remove cancelled Tantivy rebuild artifact"
            );
        }
        publication::mark_rebuild_failed(pool, &claim, error_code(error)).await;
    }
    let release_result = lease.release().await;
    match (result, release_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
    }
}

async fn build_and_publish(
    pool: &SqlitePool,
    claim: &RebuildClaim,
    source: std::path::PathBuf,
    staging: std::path::PathBuf,
    final_path: std::path::PathBuf,
    visible: HashSet<i64>,
    budget: SearchResourceBudget,
) -> Result<(), AppError> {
    if !fs::try_exists(&source).await.map_err(AppError::Io)? {
        return Err(AppError::NotFound(format!(
            "active Tantivy generation {} for Bundle {}",
            claim.active_generation, claim.bundle_id
        )));
    }
    if fs::try_exists(&staging).await.map_err(AppError::Io)? {
        fs::remove_dir_all(&staging).await.map_err(AppError::Io)?;
    }
    let parent = staging
        .parent()
        .ok_or_else(|| AppError::Config("Tantivy rebuild staging has no parent".into()))?;
    fs::create_dir_all(parent).await.map_err(AppError::Io)?;
    let permit = budget.acquire().await?;
    let heap = budget.writer_heap_size_bytes();
    let staging_for_blocking = staging.clone();
    let source_for_blocking = source.clone();
    tokio::task::spawn_blocking(move || {
        tantivy::rebuild_visible_index(source_for_blocking, staging_for_blocking, &visible, heap)
    })
    .await
    .map_err(|error| AppError::Config(format!("Tantivy rebuild task failed: {error}")))??;
    drop(permit);

    if !publication::rebuild_claim_is_current(pool, claim).await? {
        return Err(AppError::Conflict(
            "Tantivy rebuild was cancelled while building".into(),
        ));
    }

    if !publication::begin_rebuild_publication(pool, claim).await? {
        return Err(AppError::Conflict(
            "Tantivy rebuild was claimed by another worker before publication".into(),
        ));
    }

    // The durable PUBLISHING phase prevents another worker or recovery pass
    // from taking ownership while this shared final path is replaced.
    if fs::try_exists(&final_path).await.map_err(AppError::Io)? {
        fs::remove_dir_all(&final_path)
            .await
            .map_err(AppError::Io)?;
    }
    if let Some(parent) = final_path.parent() {
        fs::create_dir_all(parent).await.map_err(AppError::Io)?;
    }
    fs::rename(&staging, &final_path)
        .await
        .map_err(AppError::Io)?;
    let verify_path = final_path.clone();
    tokio::task::spawn_blocking(move || tantivy::writer::open_committed(verify_path))
        .await
        .map_err(|error| {
            AppError::Config(format!("Tantivy rebuild verification failed: {error}"))
        })??;
    if !publication::rebuild_claim_is_current(pool, claim).await? {
        return Err(AppError::Conflict(
            "Tantivy rebuild was cancelled before publication".into(),
        ));
    }
    publication::publish_rebuild(pool, claim).await
}

async fn claim_next(pool: &SqlitePool) -> Result<Option<RebuildClaim>, AppError> {
    crate::db::write::run(pool, "claim Tantivy deletion rebuild", &(), |conn, &()| {
        Box::pin(async move {
            let row: Option<(String, i64, i64)> = sqlx::query_as(
                "SELECT i.bundle_id, i.generation, i.visibility_revision FROM bundle_search_indexes i JOIN bundles b ON b.id=i.bundle_id JOIN issues issue ON issue.code=b.issue_code WHERE i.backend='tantivy' AND b.status='READY' AND issue.status='ACTIVE' AND i.state='NEEDS_REBUILD' AND i.generation > 0 AND i.visibility_revision > i.compacted_revision AND (i.pending_state IN ('IDLE','FAILED') OR (i.pending_state='BUILDING' AND datetime(i.updated_at) <= datetime('now','-5 minutes'))) ORDER BY i.updated_at, i.bundle_id LIMIT 1",
            )
            .fetch_optional(&mut *conn)
            .await
            .map_err(AppError::Database)?;
            let Some((bundle_id, active_generation, target_revision)) = row else {
                return Ok(None);
            };
            let target_generation = active_generation.saturating_add(1);
            let claim_token = Uuid::new_v4().to_string();
            let changed = sqlx::query(
                "UPDATE bundle_search_indexes SET pending_generation=?, pending_revision=?, pending_owner=?, pending_phase='BUILDING', pending_state='BUILDING', updated_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND state='NEEDS_REBUILD' AND generation=? AND visibility_revision=? AND (pending_state IN ('IDLE','FAILED') OR (pending_state='BUILDING' AND pending_phase='BUILDING' AND datetime(updated_at) <= datetime('now','-5 minutes'))) ",
            )
            .bind(target_generation)
            .bind(target_revision)
            .bind(&claim_token)
            .bind(&bundle_id)
            .bind(active_generation)
            .bind(target_revision)
            .execute(&mut *conn)
            .await
            .map_err(AppError::Database)?
            .rows_affected();
            if changed != 1 {
                return Ok(None);
            }
            Ok(Some(RebuildClaim {
                bundle_id,
                active_generation,
                target_generation,
                target_revision,
                claim_token,
            }))
        })
    })
    .await
}

fn error_code(error: &AppError) -> &'static str {
    match error {
        AppError::Database(_) => "DATABASE",
        AppError::Io(_) => "IO",
        AppError::NotFound(_) => "NOT_FOUND",
        AppError::Conflict(_) => "CONFLICT",
        _ => "REBUILD",
    }
}
