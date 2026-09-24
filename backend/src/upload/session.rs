use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

use crate::error::AppError;

pub const CHUNK_SIZE_BYTES: u64 = 8 * 1024 * 1024;
pub const SESSION_MIN_FILE_SIZE_BYTES: u64 = 64 * 1024 * 1024;
pub const SESSION_MAX_AGE_SECONDS: i64 = 7 * 24 * 60 * 60;
pub const SESSION_IDLE_AGE_SECONDS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SessionStatus {
    Open,
    Finalizing,
    Delivered,
    Cancelled,
    Expired,
    Failed,
}

impl SessionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::Finalizing => "FINALIZING",
            Self::Delivered => "DELIVERED",
            Self::Cancelled => "CANCELLED",
            Self::Expired => "EXPIRED",
            Self::Failed => "FAILED",
        }
    }
}

impl TryFrom<&str> for SessionStatus {
    type Error = AppError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "OPEN" => Ok(Self::Open),
            "FINALIZING" => Ok(Self::Finalizing),
            "DELIVERED" => Ok(Self::Delivered),
            "CANCELLED" => Ok(Self::Cancelled),
            "EXPIRED" => Ok(Self::Expired),
            "FAILED" => Ok(Self::Failed),
            _ => Err(AppError::Config(format!(
                "unknown upload session status {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct UploadSession {
    pub id: String,
    pub issue_code: String,
    pub owner_user_id: String,
    pub idempotency_key: String,
    pub file_name: String,
    pub file_size_bytes: u64,
    pub last_modified_ms: Option<i64>,
    pub chunk_size_bytes: u64,
    pub committed_offset: u64,
    pub next_chunk_index: u64,
    pub status: SessionStatus,
    pub input_path: String,
    pub bundle_id: Option<String>,
    pub failure_code: Option<String>,
    pub failure_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone)]
pub struct CreateSessionRow {
    pub id: String,
    pub issue_code: String,
    pub owner_user_id: String,
    pub idempotency_key: String,
    pub file_name: String,
    pub file_size_bytes: u64,
    pub last_modified_ms: Option<i64>,
    pub chunk_size_bytes: u64,
    pub input_path: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct UploadSessionChunk {
    pub session_id: String,
    pub chunk_index: i64,
    pub offset_bytes: i64,
    pub size_bytes: i64,
    pub sha256: String,
}

#[derive(Debug, Clone, FromRow)]
struct UploadSessionRow {
    id: String,
    issue_code: String,
    owner_user_id: String,
    idempotency_key: String,
    file_name: String,
    file_size_bytes: i64,
    last_modified_ms: Option<i64>,
    chunk_size_bytes: i64,
    committed_offset: i64,
    next_chunk_index: i64,
    status: String,
    input_path: String,
    bundle_id: Option<String>,
    failure_code: Option<String>,
    failure_reason: Option<String>,
    created_at: String,
    updated_at: String,
    expires_at: String,
}

impl TryFrom<UploadSessionRow> for UploadSession {
    type Error = AppError;

    fn try_from(row: UploadSessionRow) -> Result<Self, Self::Error> {
        let unsigned = |field: &'static str, value: i64| {
            u64::try_from(value)
                .map_err(|_| AppError::Config(format!("upload session {field} is negative")))
        };
        Ok(Self {
            id: row.id,
            issue_code: row.issue_code,
            owner_user_id: row.owner_user_id,
            idempotency_key: row.idempotency_key,
            file_name: row.file_name,
            file_size_bytes: unsigned("file_size_bytes", row.file_size_bytes)?,
            last_modified_ms: row.last_modified_ms,
            chunk_size_bytes: unsigned("chunk_size_bytes", row.chunk_size_bytes)?,
            committed_offset: unsigned("committed_offset", row.committed_offset)?,
            next_chunk_index: unsigned("next_chunk_index", row.next_chunk_index)?,
            status: SessionStatus::try_from(row.status.as_str())?,
            input_path: row.input_path,
            bundle_id: row.bundle_id,
            failure_code: row.failure_code,
            failure_reason: row.failure_reason,
            created_at: row.created_at,
            updated_at: row.updated_at,
            expires_at: row.expires_at,
        })
    }
}

pub fn expected_chunk_size(file_size: u64, chunk_size: u64, index: u64) -> Result<u64, AppError> {
    if file_size == 0 || chunk_size == 0 {
        return Err(AppError::BadRequest(
            "upload session chunk geometry is invalid".into(),
        ));
    }
    let offset = chunk_size
        .checked_mul(index)
        .ok_or_else(|| AppError::BadRequest("upload session chunk index is too large".into()))?;
    if offset >= file_size {
        return Err(AppError::BadRequest(
            "upload session chunk index is out of range".into(),
        ));
    }
    Ok(chunk_size.min(file_size - offset))
}

pub async fn create_session(
    pool: &SqlitePool,
    input: CreateSessionRow,
) -> Result<UploadSession, AppError> {
    let file_size = i64::try_from(input.file_size_bytes)
        .map_err(|_| AppError::BadRequest("upload session file size is too large".into()))?;
    let chunk_size = i64::try_from(input.chunk_size_bytes)
        .map_err(|_| AppError::BadRequest("upload session chunk size is too large".into()))?;
    crate::db::write::run(
        pool,
        "create upload session",
        &(input, file_size, chunk_size),
        |conn, (input, file_size, chunk_size)| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO upload_sessions (id, issue_code, owner_user_id, idempotency_key, file_name, file_size_bytes, last_modified_ms, chunk_size_bytes, status, input_path, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'OPEN', ?, ?)",
                )
                .bind(&input.id)
                .bind(&input.issue_code)
                .bind(&input.owner_user_id)
                .bind(&input.idempotency_key)
                .bind(&input.file_name)
                .bind(*file_size)
                .bind(input.last_modified_ms)
                .bind(*chunk_size)
                .bind(&input.input_path)
                .bind(&input.expires_at)
                .execute(&mut *conn)
                .await
                .map_err(|error| {
                    if is_unique_violation(&error) {
                        AppError::Conflict("upload session already exists".into())
                    } else {
                        AppError::Database(error)
                    }
                })?;
                load_by_id(conn, &input.id).await
            })
        },
    )
    .await
}

pub async fn get_session(pool: &SqlitePool, session_id: &str) -> Result<UploadSession, AppError> {
    let row = sqlx::query_as::<_, UploadSessionRow>(
        "SELECT id, issue_code, owner_user_id, idempotency_key, file_name, file_size_bytes, last_modified_ms, chunk_size_bytes, committed_offset, next_chunk_index, status, input_path, bundle_id, failure_code, failure_reason, created_at, updated_at, expires_at FROM upload_sessions WHERE id = ?",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound("upload session not found".into()))?;
    row.try_into()
}

pub async fn find_by_idempotency(
    pool: &SqlitePool,
    owner_user_id: &str,
    issue_code: &str,
    idempotency_key: &str,
) -> Result<UploadSession, AppError> {
    let row = sqlx::query_as::<_, UploadSessionRow>(
        "SELECT id, issue_code, owner_user_id, idempotency_key, file_name, file_size_bytes, last_modified_ms, chunk_size_bytes, committed_offset, next_chunk_index, status, input_path, bundle_id, failure_code, failure_reason, created_at, updated_at, expires_at FROM upload_sessions WHERE owner_user_id = ? AND issue_code = ? AND idempotency_key = ?",
    )
    .bind(owner_user_id)
    .bind(issue_code)
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound("upload session not found".into()))?;
    row.try_into()
}

pub async fn list_sessions(
    pool: &SqlitePool,
    owner_user_id: &str,
    issue_code: &str,
) -> Result<Vec<UploadSession>, AppError> {
    let rows = sqlx::query_as::<_, UploadSessionRow>(
        "SELECT id, issue_code, owner_user_id, idempotency_key, file_name, file_size_bytes, last_modified_ms, chunk_size_bytes, committed_offset, next_chunk_index, status, input_path, bundle_id, failure_code, failure_reason, created_at, updated_at, expires_at FROM upload_sessions WHERE owner_user_id = ? AND issue_code = ? AND status IN ('OPEN', 'FINALIZING') ORDER BY updated_at DESC",
    )
    .bind(owner_user_id)
    .bind(issue_code)
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    rows.into_iter().map(TryInto::try_into).collect()
}

pub async fn cancel_session(
    pool: &SqlitePool,
    session_id: &str,
    owner_user_id: &str,
) -> Result<UploadSession, AppError> {
    let existing = get_session(pool, session_id).await?;
    if existing.owner_user_id != owner_user_id {
        return Err(AppError::NotFound("upload session not found".into()));
    }
    if existing.status == SessionStatus::Delivered {
        return Err(AppError::public(
            actix_web::http::StatusCode::CONFLICT,
            "UPLOAD_SESSION_DELIVERED",
            format!(
                "upload session already delivered as {}",
                existing.bundle_id.as_deref().unwrap_or("unknown task")
            ),
        ));
    }
    if matches!(
        existing.status,
        SessionStatus::Cancelled | SessionStatus::Expired | SessionStatus::Failed
    ) {
        return Ok(existing);
    }
    crate::db::write::run(
        pool,
        "cancel upload session",
        &(session_id, owner_user_id),
        |conn, (session_id, owner_user_id)| {
            Box::pin(async move {
                sqlx::query(
                    "UPDATE upload_sessions SET status='CANCELLED', updated_at=CURRENT_TIMESTAMP WHERE id=? AND owner_user_id=? AND status IN ('OPEN', 'FINALIZING')",
                )
                .bind(*session_id)
                .bind(*owner_user_id)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                Ok(())
            })
        },
    )
    .await?;
    get_session(pool, session_id).await
}

pub fn same_create_metadata(session: &UploadSession, input: &CreateSessionRow) -> bool {
    session.file_name == input.file_name
        && session.file_size_bytes == input.file_size_bytes
        && session.last_modified_ms == input.last_modified_ms
        && session.chunk_size_bytes == input.chunk_size_bytes
}

pub async fn active_declared_bytes(pool: &SqlitePool) -> Result<u64, AppError> {
    let total: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(file_size_bytes), 0) FROM upload_sessions WHERE status IN ('OPEN', 'FINALIZING')",
    )
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;
    u64::try_from(total)
        .map_err(|_| AppError::Config("upload session byte total is negative".into()))
}

pub async fn find_chunk(
    pool: &SqlitePool,
    session_id: &str,
    chunk_index: u64,
) -> Result<Option<UploadSessionChunk>, AppError> {
    let chunk_index = i64::try_from(chunk_index)
        .map_err(|_| AppError::BadRequest("upload session chunk index is too large".into()))?;
    sqlx::query_as::<_, UploadSessionChunk>(
        "SELECT session_id, chunk_index, offset_bytes, size_bytes, sha256 FROM upload_session_chunks WHERE session_id=? AND chunk_index=?",
    )
    .bind(session_id)
    .bind(chunk_index)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)
}

pub async fn record_chunk(
    pool: &SqlitePool,
    session_id: &str,
    chunk_index: u64,
    offset_bytes: u64,
    size_bytes: u64,
    sha256: &str,
) -> Result<UploadSession, AppError> {
    let chunk_index = i64::try_from(chunk_index)
        .map_err(|_| AppError::BadRequest("upload session chunk index is too large".into()))?;
    let offset_bytes = i64::try_from(offset_bytes)
        .map_err(|_| AppError::BadRequest("upload session offset is too large".into()))?;
    let size_bytes = i64::try_from(size_bytes)
        .map_err(|_| AppError::BadRequest("upload session chunk is too large".into()))?;
    crate::db::write::run(
        pool,
        "record upload session chunk",
        &(session_id, chunk_index, offset_bytes, size_bytes, sha256),
        |conn, (session_id, chunk_index, offset_bytes, size_bytes, sha256)| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO upload_session_chunks (session_id, chunk_index, offset_bytes, size_bytes, sha256) VALUES (?, ?, ?, ?, ?)",
                )
                .bind(*session_id)
                .bind(*chunk_index)
                .bind(*offset_bytes)
                .bind(*size_bytes)
                .bind(*sha256)
                .execute(&mut *conn)
                .await
                .map_err(|error| {
                    if is_unique_violation(&error) {
                        AppError::Conflict("upload session chunk already exists".into())
                    } else {
                        AppError::Database(error)
                    }
                })?;
                let updated = sqlx::query(
                    "UPDATE upload_sessions SET committed_offset=committed_offset + ?, next_chunk_index=next_chunk_index + 1, updated_at=CURRENT_TIMESTAMP WHERE id=? AND status='OPEN' AND committed_offset=? AND next_chunk_index=?",
                )
                .bind(*size_bytes)
                .bind(*session_id)
                .bind(*offset_bytes)
                .bind(*chunk_index)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                if updated.rows_affected() != 1 {
                    return Err(AppError::Conflict(
                        "upload session offset changed; retry from the authoritative offset"
                            .into(),
                    ));
                }
                load_by_id(conn, session_id).await
            })
        },
    )
    .await
}

pub async fn list_finalizing(pool: &SqlitePool) -> Result<Vec<UploadSession>, AppError> {
    let rows = sqlx::query_as::<_, UploadSessionRow>(
        "SELECT id, issue_code, owner_user_id, idempotency_key, file_name, file_size_bytes, last_modified_ms, chunk_size_bytes, committed_offset, next_chunk_index, status, input_path, bundle_id, failure_code, failure_reason, created_at, updated_at, expires_at FROM upload_sessions WHERE status='FINALIZING' ORDER BY updated_at ASC",
    )
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    rows.into_iter().map(TryInto::try_into).collect()
}

pub async fn list_recoverable(pool: &SqlitePool) -> Result<Vec<UploadSession>, AppError> {
    let rows = sqlx::query_as::<_, UploadSessionRow>(
        "SELECT id, issue_code, owner_user_id, idempotency_key, file_name, file_size_bytes, last_modified_ms, chunk_size_bytes, committed_offset, next_chunk_index, status, input_path, bundle_id, failure_code, failure_reason, created_at, updated_at, expires_at FROM upload_sessions WHERE status IN ('OPEN', 'FINALIZING') ORDER BY updated_at ASC",
    )
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    rows.into_iter().map(TryInto::try_into).collect()
}

pub async fn expire_sessions(pool: &SqlitePool) -> Result<Vec<UploadSession>, AppError> {
    let candidates = sqlx::query_as::<_, UploadSessionRow>(
        "SELECT id, issue_code, owner_user_id, idempotency_key, file_name, file_size_bytes, last_modified_ms, chunk_size_bytes, committed_offset, next_chunk_index, status, input_path, bundle_id, failure_code, failure_reason, created_at, updated_at, expires_at FROM upload_sessions WHERE status IN ('OPEN', 'FINALIZING') AND (datetime(expires_at) <= CURRENT_TIMESTAMP OR datetime(updated_at) <= datetime('now', '-24 hours'))",
    )
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    let mut expired = Vec::new();
    for row in candidates {
        let session: UploadSession = row.try_into()?;
        let updated = crate::db::write::run(
            pool,
            "expire upload session",
            &(&session.id,),
            |conn, (session_id,)| {
                Box::pin(async move {
                    let result = sqlx::query(
                        "UPDATE upload_sessions SET status='EXPIRED', updated_at=CURRENT_TIMESTAMP WHERE id=? AND status IN ('OPEN', 'FINALIZING')",
                    )
                    .bind(*session_id)
                    .execute(&mut *conn)
                    .await
                    .map_err(AppError::Database)?;
                    Ok(result.rows_affected() == 1)
                })
            },
        )
        .await?;
        if updated {
            expired.push(session);
        }
    }
    Ok(expired)
}

pub async fn mark_finalizing(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<UploadSession, AppError> {
    crate::db::write::run(pool, "mark upload session finalizing", &(session_id,), |conn, (session_id,)| {
        Box::pin(async move {
            sqlx::query("UPDATE upload_sessions SET status='FINALIZING', updated_at=CURRENT_TIMESTAMP WHERE id=? AND status='OPEN'")
                .bind(*session_id)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
            Ok(())
        })
    })
    .await?;
    get_session(pool, session_id).await
}

pub async fn mark_delivered(
    pool: &SqlitePool,
    session_id: &str,
    bundle_id: &str,
) -> Result<UploadSession, AppError> {
    crate::db::write::run(
        pool,
        "mark upload session delivered",
        &(session_id, bundle_id),
        |conn, (session_id, bundle_id)| {
            Box::pin(async move {
                let updated = sqlx::query(
                    "UPDATE upload_sessions SET status='DELIVERED', bundle_id=COALESCE(bundle_id, ?), updated_at=CURRENT_TIMESTAMP WHERE id=? AND status='FINALIZING'",
                )
                .bind(*bundle_id)
                .bind(*session_id)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                if updated.rows_affected() != 1 {
                    return Err(AppError::Conflict(
                        "upload session is no longer finalizing".into(),
                    ));
                }
                Ok(())
            })
        },
    )
    .await?;
    get_session(pool, session_id).await
}

pub async fn mark_failed(
    pool: &SqlitePool,
    session_id: &str,
    code: &str,
    reason: &str,
) -> Result<(), AppError> {
    crate::db::write::run(
        pool,
        "mark upload session failed",
        &(session_id, code, reason),
        |conn, (session_id, code, reason)| {
            Box::pin(async move {
                sqlx::query(
                    "UPDATE upload_sessions SET status='FAILED', failure_code=?, failure_reason=?, updated_at=CURRENT_TIMESTAMP WHERE id=? AND status IN ('OPEN', 'FINALIZING')",
                )
                .bind(*code)
                .bind(*reason)
                .bind(*session_id)
                .execute(&mut *conn)
                .await
                .map(|_| ())
                .map_err(AppError::Database)
            })
        },
    )
    .await
}

pub async fn attach_processing_bundle(
    pool: &SqlitePool,
    session_id: &str,
    bundle_id: &str,
    bundle_hash: &str,
) -> Result<UploadSession, AppError> {
    crate::db::write::run(
        pool,
        "attach processing bundle to upload session",
        &(session_id, bundle_id, bundle_hash),
        |conn, (session_id, bundle_id, bundle_hash)| {
            Box::pin(async move {
                let session: Option<(String, String, String, i64, Option<String>)> =
                    sqlx::query_as(
                        "SELECT issue_code, owner_user_id, file_name, file_size_bytes, bundle_id FROM upload_sessions WHERE id=? AND status='FINALIZING'",
                    )
                    .bind(*session_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(AppError::Database)?;
                let Some((issue_code, owner_user_id, file_name, file_size_bytes, existing_bundle)) =
                    session
                else {
                    return Err(AppError::Conflict(
                        "upload session is not ready for finalization".into(),
                    ));
                };
                if let Some(existing_bundle) = existing_bundle {
                    if existing_bundle != *bundle_id {
                        return Err(AppError::Conflict(
                            "upload session is attached to another bundle".into(),
                        ));
                    }
                    return load_by_id(conn, session_id).await;
                }
                let inserted = sqlx::query(
                    "INSERT INTO bundles (id, issue_code, hash, name, status, process_stage, uploader_user_id, size_bytes) SELECT ?, issue_code, ?, file_name, 'PROCESSING', 'RECEIVING', owner_user_id, file_size_bytes FROM upload_sessions WHERE id=? AND status='FINALIZING' AND bundle_id IS NULL AND EXISTS (SELECT 1 FROM issues WHERE code=? AND status='ACTIVE')",
                )
                .bind(*bundle_id)
                .bind(*bundle_hash)
                .bind(*session_id)
                .bind(&issue_code)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                if inserted.rows_affected() != 1 {
                    return Err(AppError::Conflict(
                        "issue is missing or being deleted".into(),
                    ));
                }
                sqlx::query(
                    "INSERT OR IGNORE INTO bundle_search_indexes (bundle_id, backend, state) VALUES (?, 'sqlite_fts', 'BUILDING')",
                )
                .bind(*bundle_id)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                let updated = sqlx::query(
                    "UPDATE upload_sessions SET bundle_id=?, updated_at=CURRENT_TIMESTAMP WHERE id=? AND status='FINALIZING' AND bundle_id IS NULL",
                )
                .bind(*bundle_id)
                .bind(*session_id)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                if updated.rows_affected() != 1 {
                    return Err(AppError::Conflict(
                        "upload session changed during finalization".into(),
                    ));
                }
                let _ = (owner_user_id, file_name, file_size_bytes);
                load_by_id(conn, session_id).await
            })
        },
    )
    .await
}

async fn load_by_id(
    conn: &mut sqlx::SqliteConnection,
    session_id: &str,
) -> Result<UploadSession, AppError> {
    let row = sqlx::query_as::<_, UploadSessionRow>(
        "SELECT id, issue_code, owner_user_id, idempotency_key, file_name, file_size_bytes, last_modified_ms, chunk_size_bytes, committed_offset, next_chunk_index, status, input_path, bundle_id, failure_code, failure_reason, created_at, updated_at, expires_at FROM upload_sessions WHERE id = ?",
    )
    .bind(session_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(AppError::Database)?;
    row.try_into()
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database_error) if database_error.message().contains("UNIQUE") || database_error.message().contains("unique"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_chunk_size_rejects_overflow_and_accepts_short_final_chunk() {
        assert_eq!(
            expected_chunk_size(17 * 1024 * 1024, 8 * 1024 * 1024, 0).unwrap(),
            8 * 1024 * 1024
        );
        assert_eq!(
            expected_chunk_size(17 * 1024 * 1024, 8 * 1024 * 1024, 2).unwrap(),
            1024 * 1024
        );
        assert!(expected_chunk_size(17 * 1024 * 1024, 8 * 1024 * 1024, 3).is_err());
    }

    #[test]
    fn session_status_round_trips_database_values() {
        for status in [
            SessionStatus::Open,
            SessionStatus::Finalizing,
            SessionStatus::Delivered,
            SessionStatus::Cancelled,
            SessionStatus::Expired,
            SessionStatus::Failed,
        ] {
            assert_eq!(SessionStatus::try_from(status.as_str()).unwrap(), status);
        }
    }

    #[test]
    fn invalid_geometry_returns_a_bad_request() {
        let error = expected_chunk_size(1, 0, 0).unwrap_err();
        assert!(matches!(error, AppError::BadRequest(_)));
    }
}
