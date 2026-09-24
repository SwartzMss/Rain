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
