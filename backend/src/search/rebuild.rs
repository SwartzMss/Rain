#![cfg(feature = "tantivy-search")]

use std::{collections::HashSet, path::Path};

use sqlx::SqlitePool;
use tokio::fs;

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
    let visible = snapshot_file_ids(pool, &claim.bundle_id).await?;
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
        .join(claim.target_generation.to_string());
    let result =
        build_and_publish(pool, &claim, source, staging, final_path, visible, budget).await;
    if let Err(error) = &result {
        publication::mark_rebuild_failed(pool, &claim, error_code(error)).await;
    }
    result
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
    if fs::try_exists(&final_path).await.map_err(AppError::Io)? {
        fs::remove_dir_all(&final_path)
            .await
            .map_err(AppError::Io)?;
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
            let changed = sqlx::query(
                "UPDATE bundle_search_indexes SET pending_generation=?, pending_revision=?, pending_state='BUILDING', updated_at=CURRENT_TIMESTAMP WHERE bundle_id=? AND state='NEEDS_REBUILD' AND generation=? AND visibility_revision=? AND (pending_state IN ('IDLE','FAILED') OR (pending_state='BUILDING' AND datetime(updated_at) <= datetime('now','-5 minutes'))) ",
            )
            .bind(target_generation)
            .bind(target_revision)
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
