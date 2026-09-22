use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::AppError;

const FILE_DELETION_BATCH_SIZE: i64 = 100;
const FILE_DELETION_MAX_STEPS_PER_TURN: usize = 24;

#[derive(Debug, Clone, FromRow)]
pub struct FileDeletionJob {
    pub id: String,
    pub bundle_id: String,
    pub root_file_id: i64,
    pub state: String,
    pub phase: String,
    pub cursor_file_id: Option<i64>,
    pub attempts: i64,
    pub next_retry_at: Option<String>,
    pub last_error_code: Option<String>,
    pub reconcile_after_id: i64,
    pub reconcile_bytes: i64,
    pub deleted_files: i64,
    pub deleted_offsets: i64,
    pub deleted_segments: i64,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FileDeletionJobResponse {
    pub job_id: String,
    pub status: String,
    pub phase: String,
    pub deleted_files: i64,
    pub deleted_offsets: i64,
    pub deleted_segments: i64,
    pub attempts: i64,
    pub next_retry_at: Option<String>,
    pub last_error_code: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

impl From<FileDeletionJob> for FileDeletionJobResponse {
    fn from(job: FileDeletionJob) -> Self {
        Self {
            job_id: job.id,
            status: job.state,
            phase: job.phase,
            deleted_files: job.deleted_files,
            deleted_offsets: job.deleted_offsets,
            deleted_segments: job.deleted_segments,
            attempts: job.attempts,
            next_retry_at: job.next_retry_at,
            last_error_code: job.last_error_code,
            created_at: job.created_at,
            updated_at: job.updated_at,
            finished_at: job.finished_at,
        }
    }
}

enum EnqueueResult {
    Created(FileDeletionJob),
    Existing(FileDeletionJob),
}

pub async fn enqueue_file_deletion(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    root_file_id: i64,
    requested_by_user_id: &str,
) -> Result<FileDeletionJob, AppError> {
    let input = (
        Uuid::new_v4().to_string(),
        bundle_id.to_owned(),
        root_file_id,
        requested_by_user_id.to_owned(),
    );
    let result = crate::db::write::run(
        pool,
        "enqueue file tree deletion",
        &input,
        |conn, (job_id, bundle_id, root_file_id, requested_by_user_id)| {
            Box::pin(async move {
                let parent_ready: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM bundles b JOIN issues i ON i.code=b.issue_code WHERE b.id=? AND b.status='READY' AND i.status='ACTIVE')",
                )
                .bind(bundle_id)
                .fetch_one(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                if !parent_ready {
                    return Err(AppError::Conflict(
                        "bundle is no longer ready for file deletion".into(),
                    ));
                }

                let existing = sqlx::query_as::<_, FileDeletionJob>(
                    "SELECT id,bundle_id,root_file_id,state,phase,cursor_file_id,attempts,next_retry_at,last_error_code,reconcile_after_id,reconcile_bytes,deleted_files,deleted_offsets,deleted_segments,created_at,updated_at,finished_at FROM file_deletion_jobs WHERE bundle_id=? AND state IN ('QUEUED','RUNNING','RETRY_WAIT') ORDER BY created_at LIMIT 1",
                )
                .bind(bundle_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                if let Some(existing) = existing {
                    if existing.root_file_id == *root_file_id {
                        return Ok(EnqueueResult::Existing(existing));
                    }
                    return Err(AppError::Conflict(format!(
                        "bundle already has a file deletion job: {}",
                        existing.id
                    )));
                }

                let target_exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM files WHERE bundle_id=? AND id=? AND status IS NOT 'DELETING')",
                )
                .bind(bundle_id)
                .bind(root_file_id)
                .fetch_one(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                if !target_exists {
                    return Err(AppError::NotFound(format!("file {root_file_id}")));
                }

                sqlx::query("INSERT INTO file_deletion_jobs(id,bundle_id,root_file_id,requested_by_user_id,state,phase,cursor_file_id) VALUES(?,?,?,?,'QUEUED','WALK',?)")
                    .bind(job_id)
                    .bind(bundle_id)
                    .bind(root_file_id)
                    .bind(requested_by_user_id)
                    .bind(root_file_id)
                    .execute(&mut *conn)
                    .await
                    .map_err(AppError::Database)?;
                let job = sqlx::query_as::<_, FileDeletionJob>(
                    "SELECT id,bundle_id,root_file_id,state,phase,cursor_file_id,attempts,next_retry_at,last_error_code,reconcile_after_id,reconcile_bytes,deleted_files,deleted_offsets,deleted_segments,created_at,updated_at,finished_at FROM file_deletion_jobs WHERE id=?",
                )
                .bind(job_id)
                .fetch_one(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                Ok(EnqueueResult::Created(job))
            })
        },
    )
    .await?;
    Ok(match result {
        EnqueueResult::Created(job) | EnqueueResult::Existing(job) => job,
    })
}

pub async fn load_file_deletion_job(
    pool: &sqlx::SqlitePool,
    job_id: &str,
    user_id: &str,
) -> Result<FileDeletionJob, AppError> {
    sqlx::query_as::<_, FileDeletionJob>(
        "SELECT j.id,j.bundle_id,j.root_file_id,j.state,j.phase,j.cursor_file_id,j.attempts,j.next_retry_at,j.last_error_code,j.reconcile_after_id,j.reconcile_bytes,j.deleted_files,j.deleted_offsets,j.deleted_segments,j.created_at,j.updated_at,j.finished_at FROM file_deletion_jobs j JOIN bundles b ON b.id=j.bundle_id JOIN issues i ON i.code=b.issue_code WHERE j.id=? AND i.owner_user_id=? LIMIT 1",
    )
    .bind(job_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("file deletion job {job_id}")))
}

pub async fn process_file_deletion_jobs(pool: &sqlx::SqlitePool) -> Result<usize, AppError> {
    let mut processed = 0;
    let mut yielded_jobs = Vec::new();
    for _ in 0..4 {
        let Some(job) = claim_next_job(pool).await? else {
            break;
        };
        let mut completed_turn = false;
        for _ in 0..FILE_DELETION_MAX_STEPS_PER_TURN {
            match process_job_step(pool, &job).await {
                Ok(StepResult::Continue) => continue,
                Ok(StepResult::Finished) | Ok(StepResult::Superseded) => {
                    completed_turn = true;
                    break;
                }
                Err(error) => {
                    schedule_retry(pool, &job.id, &job.lease_token, &error).await;
                    completed_turn = true;
                    break;
                }
            }
        }
        if !completed_turn {
            yielded_jobs.push(job.clone());
            tracing::debug!(
                job_id = job.id,
                "file deletion worker yielded after batch turn"
            );
        }
        processed += 1;
    }
    for job in yielded_jobs {
        yield_job(pool, &job.id, &job.lease_token).await;
    }
    Ok(processed)
}

#[derive(Debug, Clone, FromRow)]
struct ClaimedJob {
    id: String,
    lease_token: String,
}

async fn claim_next_job(pool: &sqlx::SqlitePool) -> Result<Option<ClaimedJob>, AppError> {
    let candidate: Option<String> = sqlx::query_scalar(
        "SELECT id FROM file_deletion_jobs WHERE (state='QUEUED' OR (state='RETRY_WAIT' AND (next_retry_at IS NULL OR datetime(next_retry_at) <= datetime('now'))) OR (state='RUNNING' AND (lease_until IS NULL OR datetime(lease_until) <= datetime('now')))) ORDER BY COALESCE(next_retry_at,''), created_at, id LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)?;
    let Some(job_id) = candidate else {
        return Ok(None);
    };
    let token = Uuid::new_v4().to_string();
    let input = (job_id.as_str(), token.as_str());
    let claimed = crate::db::write::run(
        pool,
        "claim file deletion job",
        &input,
        |conn, (job_id, token)| {
            Box::pin(async move {
                let affected = sqlx::query("UPDATE file_deletion_jobs SET state='RUNNING', lease_token=?, lease_until=datetime('now', '+60 seconds'), next_retry_at=NULL, updated_at=CURRENT_TIMESTAMP WHERE id=? AND (state='QUEUED' OR (state='RETRY_WAIT' AND (next_retry_at IS NULL OR datetime(next_retry_at) <= datetime('now'))) OR (state='RUNNING' AND (lease_until IS NULL OR datetime(lease_until) <= datetime('now'))))")
                    .bind(token)
                    .bind(job_id)
                    .execute(conn)
                    .await
                    .map_err(AppError::Database)?
                    .rows_affected();
                Ok(affected == 1)
            })
        },
    )
    .await?;
    Ok(claimed.then_some(ClaimedJob {
        id: job_id,
        lease_token: token,
    }))
}

enum StepResult {
    Continue,
    Finished,
    Superseded,
}

async fn yield_job(pool: &sqlx::SqlitePool, job_id: &str, token: &str) {
    let input = (job_id, token);
    let _ = crate::db::write::run(pool, "yield file deletion job", &input, |conn, (job_id, token)| {
        Box::pin(async move {
            sqlx::query("UPDATE file_deletion_jobs SET state='QUEUED', lease_token=NULL, lease_until=NULL, updated_at=CURRENT_TIMESTAMP WHERE id=? AND state='RUNNING' AND lease_token=?")
                .bind(job_id)
                .bind(token)
                .execute(conn)
                .await
                .map_err(AppError::Database)?;
            Ok(())
        })
    }).await;
}

async fn process_job_step(
    pool: &sqlx::SqlitePool,
    job: &ClaimedJob,
) -> Result<StepResult, AppError> {
    let input = (job.id.as_str(), job.lease_token.as_str());
    crate::db::write::run(pool, "process file deletion batch", &input, |conn, (job_id, token)| {
        Box::pin(async move {
            let parent_state: Option<(String, String)> = sqlx::query_as(
                "SELECT b.status,i.status FROM file_deletion_jobs j JOIN bundles b ON b.id=j.bundle_id JOIN issues i ON i.code=b.issue_code WHERE j.id=? AND j.state='RUNNING' AND j.lease_token=? AND (j.lease_until IS NULL OR datetime(j.lease_until) > datetime('now'))",
            )
            .bind(job_id)
            .bind(token)
            .fetch_optional(&mut *conn)
            .await
            .map_err(AppError::Database)?;
            let Some((bundle_status, issue_status)) = parent_state else {
                return Ok(StepResult::Finished);
            };
            if bundle_status != "READY" || issue_status != "ACTIVE" {
                sqlx::query("UPDATE file_deletion_jobs SET state='SUPERSEDED', lease_token=NULL, lease_until=NULL, finished_at=CURRENT_TIMESTAMP, updated_at=CURRENT_TIMESTAMP WHERE id=? AND state='RUNNING' AND lease_token=?")
                    .bind(job_id)
                    .bind(token)
                    .execute(&mut *conn)
                    .await
                    .map_err(AppError::Database)?;
                return Ok(StepResult::Superseded);
            }
            let row: (String, Option<i64>, i64, i64, i64) = sqlx::query_as(
                "SELECT phase,cursor_file_id,reconcile_after_id,reconcile_bytes,root_file_id FROM file_deletion_jobs WHERE id=? AND state='RUNNING' AND lease_token=?",
            )
            .bind(job_id)
            .bind(token)
            .fetch_one(&mut *conn)
            .await
            .map_err(AppError::Database)?;
            let (phase, cursor, reconcile_after, reconcile_bytes, root_file_id) = row;
            let cursor = cursor.unwrap_or_default();
            match phase.as_str() {
                "WALK" => {
                    let child: Option<i64> = sqlx::query_scalar("SELECT id FROM files WHERE bundle_id=(SELECT bundle_id FROM file_deletion_jobs WHERE id=?) AND parent_id=? ORDER BY id LIMIT 1")
                        .bind(job_id)
                        .bind(cursor)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(AppError::Database)?;
                    if let Some(child) = child {
                        sqlx::query("UPDATE file_deletion_jobs SET cursor_file_id=?, updated_at=CURRENT_TIMESTAMP WHERE id=? AND lease_token=?")
                            .bind(child).bind(job_id).bind(token).execute(&mut *conn).await.map_err(AppError::Database)?;
                    } else {
                        sqlx::query("UPDATE file_deletion_jobs SET phase='OFFSETS', updated_at=CURRENT_TIMESTAMP WHERE id=? AND lease_token=?")
                            .bind(job_id).bind(token).execute(&mut *conn).await.map_err(AppError::Database)?;
                    }
                }
                "OFFSETS" => {
                    let affected = sqlx::query("DELETE FROM log_line_offsets WHERE rowid IN (SELECT rowid FROM log_line_offsets WHERE file_id=? LIMIT ?)")
                        .bind(cursor).bind(FILE_DELETION_BATCH_SIZE).execute(&mut *conn).await.map_err(AppError::Database)?.rows_affected();
                    if affected == 0 {
                        sqlx::query("UPDATE file_deletion_jobs SET phase='SEGMENTS', updated_at=CURRENT_TIMESTAMP WHERE id=? AND lease_token=?").bind(job_id).bind(token).execute(&mut *conn).await.map_err(AppError::Database)?;
                    } else {
                        sqlx::query("UPDATE file_deletion_jobs SET deleted_offsets=deleted_offsets+?, updated_at=CURRENT_TIMESTAMP WHERE id=? AND lease_token=?").bind(affected as i64).bind(job_id).bind(token).execute(&mut *conn).await.map_err(AppError::Database)?;
                    }
                }
                "SEGMENTS" => {
                    let affected = sqlx::query("DELETE FROM log_segments WHERE rowid IN (SELECT rowid FROM log_segments WHERE file_id=? LIMIT ?)")
                        .bind(cursor).bind(FILE_DELETION_BATCH_SIZE).execute(&mut *conn).await.map_err(AppError::Database)?.rows_affected();
                    if affected == 0 {
                        sqlx::query("UPDATE file_deletion_jobs SET phase='REMOVE_NODE', updated_at=CURRENT_TIMESTAMP WHERE id=? AND lease_token=?").bind(job_id).bind(token).execute(&mut *conn).await.map_err(AppError::Database)?;
                    } else {
                        sqlx::query("UPDATE file_deletion_jobs SET deleted_segments=deleted_segments+?, updated_at=CURRENT_TIMESTAMP WHERE id=? AND lease_token=?").bind(affected as i64).bind(job_id).bind(token).execute(&mut *conn).await.map_err(AppError::Database)?;
                    }
                }
                "REMOVE_NODE" => {
                    let node: Option<(Option<i64>, i64, i64, i64)> = sqlx::query_as("SELECT parent_id,(SELECT COUNT(*) FROM files child WHERE child.parent_id=files.id),(SELECT COUNT(*) FROM log_line_offsets WHERE file_id=files.id),(SELECT COUNT(*) FROM log_segments WHERE file_id=files.id) FROM files WHERE id=? AND bundle_id=(SELECT bundle_id FROM file_deletion_jobs WHERE id=?)")
                        .bind(cursor).bind(job_id).fetch_optional(&mut *conn).await.map_err(AppError::Database)?;
                    let Some((parent_id, children, offsets, segments)) = node else {
                        return Err(AppError::NotFound(format!("file {cursor}")));
                    };
                    if children != 0 || offsets != 0 || segments != 0 {
                        return Err(AppError::Conflict("file deletion node is not a leaf".into()));
                    }
                    sqlx::query("DELETE FROM files WHERE id=? AND bundle_id=(SELECT bundle_id FROM file_deletion_jobs WHERE id=?)")
                        .bind(cursor).bind(job_id).execute(&mut *conn).await.map_err(AppError::Database)?;
                    if cursor == root_file_id {
                        sqlx::query("UPDATE file_deletion_jobs SET phase='RECONCILE', cursor_file_id=NULL, deleted_files=deleted_files+1, updated_at=CURRENT_TIMESTAMP WHERE id=? AND lease_token=?").bind(job_id).bind(token).execute(&mut *conn).await.map_err(AppError::Database)?;
                    } else {
                        sqlx::query("UPDATE file_deletion_jobs SET phase='WALK', cursor_file_id=?, deleted_files=deleted_files+1, updated_at=CURRENT_TIMESTAMP WHERE id=? AND lease_token=?").bind(parent_id).bind(job_id).bind(token).execute(&mut *conn).await.map_err(AppError::Database)?;
                    }
                }
                "RECONCILE" => {
                    let rows: Vec<(i64, Option<i64>, i64, Option<String>)> = sqlx::query_as("SELECT id,size_bytes,is_dir,meta FROM files WHERE bundle_id=(SELECT bundle_id FROM file_deletion_jobs WHERE id=?) AND id>? ORDER BY id LIMIT ?")
                        .bind(job_id).bind(reconcile_after).bind(FILE_DELETION_BATCH_SIZE).fetch_all(&mut *conn).await.map_err(AppError::Database)?;
                    if rows.is_empty() {
                        let affected = sqlx::query("UPDATE bundles SET content_size_bytes=? WHERE id=(SELECT bundle_id FROM file_deletion_jobs WHERE id=?) AND status='READY'")
                            .bind(reconcile_bytes).bind(job_id).execute(&mut *conn).await.map_err(AppError::Database)?.rows_affected();
                        if affected != 1 { return Err(AppError::Conflict("bundle changed during file deletion".into())); }
                        sqlx::query("UPDATE file_deletion_jobs SET state='SUCCEEDED', lease_token=NULL, lease_until=NULL, finished_at=CURRENT_TIMESTAMP, updated_at=CURRENT_TIMESTAMP WHERE id=? AND lease_token=?")
                            .bind(job_id).bind(token).execute(&mut *conn).await.map_err(AppError::Database)?;
                        return Ok(StepResult::Finished);
                    }
                    let mut bytes = reconcile_bytes;
                    for (_, size, is_dir, meta) in &rows {
                        if *is_dir == 0
                            && !meta
                                .as_deref()
                                .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
                                .and_then(|m| {
                                    m.get("preview_kind")
                                        .and_then(|v| v.as_str())
                                        .map(|v| v == "archive")
                                })
                                .unwrap_or(false)
                        {
                            bytes = bytes.saturating_add(size.unwrap_or(0));
                        }
                    }
                    sqlx::query("UPDATE file_deletion_jobs SET reconcile_after_id=?, reconcile_bytes=?, updated_at=CURRENT_TIMESTAMP WHERE id=? AND lease_token=?")
                        .bind(rows.last().map(|row| row.0).unwrap_or(reconcile_after)).bind(bytes).bind(job_id).bind(token).execute(&mut *conn).await.map_err(AppError::Database)?;
                }
                _ => return Err(AppError::Conflict("invalid file deletion phase".into())),
            }
            let renewed = sqlx::query("UPDATE file_deletion_jobs SET lease_until=datetime('now', '+60 seconds'), updated_at=CURRENT_TIMESTAMP WHERE id=? AND state='RUNNING' AND lease_token=? AND (lease_until IS NULL OR datetime(lease_until) > datetime('now'))")
                .bind(job_id)
                .bind(token)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?
                .rows_affected();
            if renewed != 1 {
                return Err(AppError::Conflict("file deletion lease was lost".into()));
            }
            Ok(StepResult::Continue)
        })
    }).await
}

async fn schedule_retry(pool: &sqlx::SqlitePool, job_id: &str, token: &str, error: &AppError) {
    let input = (job_id, token, error_code(error));
    let _ = crate::db::write::run(pool, "schedule file deletion retry", &input, |conn, (job_id, token, error_code)| {
        Box::pin(async move {
            sqlx::query("UPDATE file_deletion_jobs SET state='RETRY_WAIT', lease_token=NULL, lease_until=NULL, attempts=attempts+1, next_retry_at=datetime('now', '+' || MIN((attempts + 1) * 60, 3600) || ' seconds'), last_error_code=?, updated_at=CURRENT_TIMESTAMP WHERE id=? AND state='RUNNING' AND lease_token=?")
                .bind(error_code).bind(job_id).bind(token).execute(conn).await.map_err(AppError::Database)?;
            Ok(())
        })
    }).await;
    tracing::warn!(
        job_id,
        error_code = error_code(error),
        "file deletion batch failed; retry scheduled"
    );
}

fn error_code(error: &AppError) -> &'static str {
    match error {
        AppError::Database(_) => "DATABASE",
        AppError::Io(_) => "IO",
        AppError::NotFound(_) => "NOT_FOUND",
        AppError::Conflict(_) => "CONFLICT",
        _ => "CLEANUP",
    }
}

pub async fn delete_file_tree(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    root_file_id: i64,
) -> Result<(), AppError> {
    let input = (bundle_id, root_file_id);
    crate::db::write::run(pool, "delete file tree", &input, |conn, &(bundle_id, root_file_id)| {
        Box::pin(async move {
            let deleted_file = sqlx::query_scalar::<_, i64>(
                "DELETE FROM files WHERE bundle_id = ? AND id = ? RETURNING id",
            )
            .bind(bundle_id)
            .bind(root_file_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(AppError::Database)?;
            if deleted_file.is_none() {
                return Err(AppError::NotFound(format!("file {root_file_id}")));
            }

            sqlx::query(
                "UPDATE bundles SET content_size_bytes = (SELECT COALESCE(SUM(CASE WHEN json_extract(meta, '$.preview_kind') = 'archive' THEN 0 ELSE size_bytes END), 0) FROM files WHERE bundle_id = ? AND is_dir = 0) WHERE id = ?",
            )
            .bind(bundle_id)
            .bind(bundle_id)
            .execute(&mut *conn)
            .await
            .map_err(AppError::Database)?;
            Ok(())
        })
    }).await
}

#[cfg(test)]
mod tests {
    use super::{delete_file_tree, enqueue_file_deletion, process_file_deletion_jobs};

    #[tokio::test]
    async fn delete_file_tree_uses_cascade_for_a_large_tree() {
        let pool = crate::db::init_pool("sqlite::memory:").expect("init pool");
        crate::db::prepare_schema(&pool, true)
            .await
            .expect("prepare schema");

        sqlx::query("INSERT INTO issues (code, name) VALUES ('DELETE', 'DELETE')")
            .execute(&pool)
            .await
            .expect("insert issue");
        sqlx::query(
            "INSERT INTO bundles (id, issue_code, hash, name, status, content_size_bytes) VALUES ('delete-bundle', 'DELETE', 'delete-hash', 'delete', 'READY', 0), ('other-bundle', 'DELETE', 'other-hash', 'other', 'READY', 0)",
        )
        .execute(&pool)
        .await
        .expect("insert bundles");

        let mut tx = pool.begin().await.expect("begin fixture transaction");
        let root_id: i64 = sqlx::query_scalar(
            "INSERT INTO files (bundle_id, name, path, is_dir) VALUES ('delete-bundle', 'root', '/root', 1) RETURNING id",
        )
        .fetch_one(&mut *tx)
        .await
        .expect("insert root");
        let retained_id: i64 = sqlx::query_scalar(
            "INSERT INTO files (bundle_id, name, path, is_dir, size_bytes) VALUES ('delete-bundle', 'retained.log', '/retained.log', 0, 7) RETURNING id",
        )
        .fetch_one(&mut *tx)
        .await
        .expect("insert retained file");
        sqlx::query(
            "INSERT INTO log_segments (bundle_id, file_id, content) VALUES ('delete-bundle', ?, 'retained content')",
        )
        .bind(retained_id)
        .execute(&mut *tx)
        .await
        .expect("insert retained segment");

        for index in 0..10_000 {
            let file_id: i64 = sqlx::query_scalar(
                "INSERT INTO files (bundle_id, parent_id, name, path, is_dir, size_bytes) VALUES ('delete-bundle', ?, ?, ?, 0, 1) RETURNING id",
            )
            .bind(root_id)
            .bind(format!("file-{index}.log"))
            .bind(format!("/root/file-{index}.log"))
            .fetch_one(&mut *tx)
            .await
            .expect("insert child file");
            sqlx::query(
                "INSERT INTO log_line_offsets (file_id, line_number, byte_offset) VALUES (?, 0, 0)",
            )
            .bind(file_id)
            .execute(&mut *tx)
            .await
            .expect("insert line offset");
            sqlx::query(
                "INSERT INTO log_segments (bundle_id, file_id, content) VALUES ('delete-bundle', ?, ?)",
            )
            .bind(file_id)
            .bind(format!("cascade-child-{index}"))
            .execute(&mut *tx)
            .await
            .expect("insert child segment");
        }
        tx.commit().await.expect("commit fixture transaction");

        let error = delete_file_tree(&pool, "other-bundle", root_id)
            .await
            .expect_err("a file from another bundle must not be deleted");
        assert!(matches!(error, crate::error::AppError::NotFound(_)));

        delete_file_tree(&pool, "delete-bundle", root_id)
            .await
            .expect("delete file tree");

        let counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
            r#"
            SELECT
                (SELECT COUNT(*) FROM files WHERE bundle_id = 'delete-bundle'),
                (SELECT COUNT(*) FROM log_line_offsets),
                (SELECT COUNT(*) FROM log_segments),
                (SELECT COUNT(*) FROM log_segments_fts),
                (SELECT content_size_bytes FROM bundles WHERE id = 'delete-bundle')
            "#,
        )
        .fetch_one(&pool)
        .await
        .expect("count cascaded rows");
        assert_eq!(counts, (1, 0, 1, 1, 7));
    }

    #[tokio::test]
    async fn async_deletion_hides_the_subtree_and_finishes_in_bounded_steps() {
        let pool = crate::db::init_pool("sqlite::memory:").expect("init pool");
        crate::db::prepare_schema(&pool, true)
            .await
            .expect("prepare schema");
        sqlx::query("INSERT INTO issues (code, name) VALUES ('ASYNC', 'ASYNC')")
            .execute(&pool)
            .await
            .expect("insert issue");
        sqlx::query("INSERT INTO users(id,username,username_normalized,password_hash) VALUES('owner','owner','owner','hash')")
            .execute(&pool)
            .await
            .expect("insert owner");
        sqlx::query("INSERT INTO bundles (id, issue_code, hash, name, status, content_size_bytes) VALUES ('async-bundle', 'ASYNC', 'async-hash', 'async', 'READY', 3)")
            .execute(&pool)
            .await
            .expect("insert bundle");
        let root_id: i64 = sqlx::query_scalar(
            "INSERT INTO files (bundle_id, name, path, is_dir) VALUES ('async-bundle', 'root', '/root', 1) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .expect("insert root");
        let child_id: i64 = sqlx::query_scalar(
            "INSERT INTO files (bundle_id, parent_id, name, path, is_dir, size_bytes) VALUES ('async-bundle', ?, 'child.log', '/root/child.log', 0, 3) RETURNING id",
        )
        .bind(root_id)
        .fetch_one(&pool)
        .await
        .expect("insert child");
        sqlx::query("INSERT INTO log_line_offsets(file_id,line_number,byte_offset) VALUES(?,?,?)")
            .bind(child_id)
            .bind(0_i64)
            .bind(0_i64)
            .execute(&pool)
            .await
            .expect("insert offset");
        sqlx::query("INSERT INTO log_segments(bundle_id,file_id,content) VALUES(?,?,?)")
            .bind("async-bundle")
            .bind(child_id)
            .bind("line")
            .execute(&pool)
            .await
            .expect("insert segment");

        sqlx::query("UPDATE issues SET owner_user_id='owner' WHERE code='ASYNC'")
            .execute(&pool)
            .await
            .expect("assign owner");
        let job = enqueue_file_deletion(&pool, "async-bundle", root_id, "owner")
            .await
            .expect("enqueue deletion");
        let repeated = enqueue_file_deletion(&pool, "async-bundle", root_id, "owner")
            .await
            .expect("repeat enqueue is idempotent");
        assert_eq!(repeated.id, job.id);
        assert!(
            enqueue_file_deletion(&pool, "async-bundle", child_id, "owner")
                .await
                .is_err()
        );
        let visible: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM visible_files WHERE bundle_id='async-bundle'")
                .fetch_one(&pool)
                .await
                .expect("visible file count");
        assert_eq!(visible, 0);

        for _ in 0..4 {
            process_file_deletion_jobs(&pool)
                .await
                .expect("process deletion");
        }
        let state: (String, i64, i64, i64) = sqlx::query_as(
            "SELECT state,deleted_files,deleted_offsets,deleted_segments FROM file_deletion_jobs WHERE id=?",
        )
        .bind(&job.id)
        .fetch_one(&pool)
        .await
        .expect("job state");
        assert_eq!(state, ("SUCCEEDED".into(), 2, 1, 1));
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE bundle_id='async-bundle'")
                .fetch_one(&pool)
                .await
                .expect("remaining files");
        assert_eq!(remaining, 0);
        let content_size: i64 =
            sqlx::query_scalar("SELECT content_size_bytes FROM bundles WHERE id='async-bundle'")
                .fetch_one(&pool)
                .await
                .expect("content size");
        assert_eq!(content_size, 0);
    }
}
