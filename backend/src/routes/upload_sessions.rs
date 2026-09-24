use actix_web::{
    HttpRequest, HttpResponse, delete, get,
    http::header::{CONTENT_LENGTH, HeaderName},
    http::{StatusCode, header::CACHE_CONTROL},
    post, put, web,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    fs::{self, OpenOptions},
    io::{AsyncSeekExt, AsyncWriteExt, SeekFrom},
};
use uuid::Uuid;

use crate::{
    AppState,
    auth::extractor::RequireBusinessUser,
    error::AppError,
    upload::{
        multipart::TempBudget,
        session::{
            CHUNK_SIZE_BYTES, CreateSessionRow, SESSION_MIN_FILE_SIZE_BYTES, SessionStatus,
            UploadSession, cancel_session, create_session, expected_chunk_size,
            find_by_idempotency, find_chunk, list_sessions, mark_finalizing, record_chunk,
            same_create_metadata,
        },
    },
};

use super::issues::{normalize_issue_code, require_issue_owner};

#[derive(Debug, Deserialize)]
pub struct CreateUploadSessionRequest {
    pub file_name: String,
    pub file_size_bytes: u64,
    pub last_modified_ms: Option<i64>,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize)]
struct UploadSessionResponse {
    session_id: String,
    issue_code: String,
    file_name: String,
    file_size_bytes: u64,
    chunk_size_bytes: u64,
    committed_offset: u64,
    next_chunk_index: u64,
    status: SessionStatus,
    bundle_id: Option<String>,
    failure_code: Option<String>,
    failure_reason: Option<String>,
    expires_at: String,
}

fn session_response(session: UploadSession, status: StatusCode) -> HttpResponse {
    HttpResponse::build(status)
        .insert_header((CACHE_CONTROL, "no-store, private"))
        .json(UploadSessionResponse {
            session_id: session.id,
            issue_code: session.issue_code,
            file_name: session.file_name,
            file_size_bytes: session.file_size_bytes,
            chunk_size_bytes: session.chunk_size_bytes,
            committed_offset: session.committed_offset,
            next_chunk_index: session.next_chunk_index,
            status: session.status,
            bundle_id: session.bundle_id,
            failure_code: session.failure_code,
            failure_reason: session.failure_reason,
            expires_at: session.expires_at,
        })
}

#[post("/issues/{issue_code}/upload-sessions")]
pub async fn create_upload_session(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    path: web::Path<String>,
    payload: web::Json<CreateUploadSessionRequest>,
) -> Result<HttpResponse, AppError> {
    let issue_code = normalize_issue_code(&path.into_inner())?;
    require_issue_owner(&state.db.pool, &issue_code, &user.0.id).await?;
    validate_request(&payload)?;
    let settings = state.settings.snapshot().await;
    let max_upload_bytes = settings.effective.issue_max_content_size.saturating_mul(2);
    if payload.file_size_bytes > max_upload_bytes {
        return Err(AppError::BadRequest(format!(
            "upload request exceeds the maximum size of {}",
            crate::upload::filename::format_bytes(max_upload_bytes)
        )));
    }

    if let Ok(existing) = find_by_idempotency(
        &state.db.pool,
        &user.0.id,
        &issue_code,
        &payload.idempotency_key,
    )
    .await
    {
        if !same_create_metadata(
            &existing,
            &CreateSessionRow {
                id: String::new(),
                issue_code: issue_code.clone(),
                owner_user_id: user.0.id.clone(),
                idempotency_key: payload.idempotency_key.clone(),
                file_name: payload.file_name.trim().to_owned(),
                file_size_bytes: payload.file_size_bytes,
                last_modified_ms: payload.last_modified_ms,
                chunk_size_bytes: CHUNK_SIZE_BYTES,
                input_path: String::new(),
                expires_at: String::new(),
            },
        ) {
            return Err(AppError::Conflict(
                "idempotency key was reused with different file metadata".into(),
            ));
        }
        return Ok(session_response(existing, StatusCode::OK));
    }

    TempBudget::reserve_persistent(
        state.upload.tmp_bytes.clone(),
        state.upload.tmp_max_bytes.clone(),
        payload.file_size_bytes,
    )?;
    let session_id = Uuid::new_v4().simple().to_string();
    let session_dir = state.storage.data_root.join(".uploads").join(&session_id);
    let input_path = format!(".uploads/{session_id}/input.part");
    if let Err(error) = fs::create_dir_all(&session_dir).await {
        TempBudget::release_persistent(&state.upload.tmp_bytes, payload.file_size_bytes);
        return Err(AppError::Io(error));
    }
    if let Err(error) = fs::File::create(session_dir.join("input.part")).await {
        let _ = fs::remove_dir_all(&session_dir).await;
        TempBudget::release_persistent(&state.upload.tmp_bytes, payload.file_size_bytes);
        return Err(AppError::Io(error));
    }
    let expires_at = (chrono::Utc::now()
        + chrono::Duration::seconds(crate::upload::session::SESSION_MAX_AGE_SECONDS))
    .format("%Y-%m-%d %H:%M:%S")
    .to_string();
    let input = CreateSessionRow {
        id: session_id,
        issue_code,
        owner_user_id: user.0.id.clone(),
        idempotency_key: payload.idempotency_key.clone(),
        file_name: payload.file_name.trim().to_owned(),
        file_size_bytes: payload.file_size_bytes,
        last_modified_ms: payload.last_modified_ms,
        chunk_size_bytes: CHUNK_SIZE_BYTES,
        input_path,
        expires_at,
    };
    let session = match create_session(&state.db.pool, input.clone()).await {
        Ok(session) => session,
        Err(error) => {
            let _ = fs::remove_dir_all(&session_dir).await;
            TempBudget::release_persistent(&state.upload.tmp_bytes, input.file_size_bytes);
            if matches!(error, AppError::Conflict(_)) {
                if let Ok(existing) = find_by_idempotency(
                    &state.db.pool,
                    &input.owner_user_id,
                    &input.issue_code,
                    &input.idempotency_key,
                )
                .await
                {
                    if same_create_metadata(&existing, &input) {
                        return Ok(session_response(existing, StatusCode::OK));
                    }
                }
            }
            return Err(error);
        }
    };
    Ok(session_response(session, StatusCode::CREATED))
}

#[get("/issues/{issue_code}/upload-sessions")]
pub async fn list_upload_sessions(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let issue_code = require_issue_owner(&state.db.pool, &path.into_inner(), &user.0.id).await?;
    let sessions = list_sessions(&state.db.pool, &user.0.id, &issue_code).await?;
    Ok(HttpResponse::Ok()
        .insert_header((CACHE_CONTROL, "no-store, private"))
        .json(
            sessions
                .into_iter()
                .map(UploadSessionResponse::from)
                .collect::<Vec<_>>(),
        ))
}

#[get("/upload-sessions/{session_id}")]
pub async fn get_upload_session(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let session = crate::upload::session::get_session(&state.db.pool, &path.into_inner()).await?;
    if session.owner_user_id != user.0.id {
        return Err(AppError::NotFound("upload session not found".into()));
    }
    Ok(session_response(session, StatusCode::OK))
}

#[delete("/upload-sessions/{session_id}")]
pub async fn delete_upload_session(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let session_id = path.into_inner();
    let lock = state.upload.session_lock(&session_id);
    let _guard = lock.lock().await;
    let before = crate::upload::session::get_session(&state.db.pool, &session_id).await?;
    if before.owner_user_id != user.0.id {
        return Err(AppError::NotFound("upload session not found".into()));
    }
    let session = cancel_session(&state.db.pool, &session_id, &user.0.id).await?;
    if before.status != session.status && matches!(session.status, SessionStatus::Cancelled) {
        cleanup_cancelled_session(&state, &before).await;
    }
    Ok(session_response(session, StatusCode::OK))
}

pub(crate) async fn cancel_issue_sessions(
    state: &web::Data<AppState>,
    issue_code: &str,
    owner_user_id: &str,
) -> Result<(), AppError> {
    let sessions =
        crate::upload::session::list_issue_sessions(&state.db.pool, owner_user_id, issue_code)
            .await?;
    for session in sessions {
        let lock = state.upload.session_lock(&session.id);
        let _guard = lock.lock().await;
        let before = crate::upload::session::get_session(&state.db.pool, &session.id).await?;
        if before.owner_user_id != owner_user_id {
            continue;
        }
        let cancelled =
            crate::upload::session::cancel_session(&state.db.pool, &session.id, owner_user_id)
                .await?;
        if cancelled.status == SessionStatus::Cancelled && before.status != SessionStatus::Cancelled
        {
            cleanup_cancelled_session(state, &before).await;
        }
    }
    Ok(())
}

async fn cleanup_cancelled_session(state: &web::Data<AppState>, session: &UploadSession) {
    let path = state.storage.data_root.join(&session.input_path);
    let cleanup_dir = path.parent().unwrap_or(&path).to_path_buf();
    let reservation = crate::upload::multipart::ReceiveReservation::adopt_persistent(
        state.upload.tmp_bytes.clone(),
        state.upload.tmp_max_bytes.clone(),
        session.file_size_bytes,
    );
    match fs::remove_dir_all(&cleanup_dir).await {
        Ok(()) => drop(reservation),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => drop(reservation),
        Err(error) => {
            tracing::warn!(path = %cleanup_dir.display(), %error, "upload session cleanup deferred");
            state
                .upload
                .temp_cleanup_queue
                .enqueue(cleanup_dir, reservation);
        }
    }
}

#[put("/upload-sessions/{session_id}/chunks/{chunk_index}")]
pub async fn upload_session_chunk(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    path: web::Path<(String, u64)>,
    req: HttpRequest,
    mut payload: web::Payload,
) -> Result<HttpResponse, AppError> {
    let (session_id, chunk_index) = path.into_inner();
    let offset = required_u64_header(&req, "X-Upload-Offset")?;
    let expected_hash = required_sha256_header(&req)?;
    let session = crate::upload::session::get_session(&state.db.pool, &session_id).await?;
    if session.owner_user_id != user.0.id {
        return Err(AppError::NotFound("upload session not found".into()));
    }
    if session.status != SessionStatus::Open {
        return Err(AppError::Conflict(format!(
            "upload session is {}",
            session.status.as_str()
        )));
    }
    let expected_size = expected_chunk_size(
        session.file_size_bytes,
        session.chunk_size_bytes,
        chunk_index,
    )?;
    let lock = state.upload.session_lock(&session_id);
    let _guard = lock.lock().await;
    let session = crate::upload::session::get_session(&state.db.pool, &session_id).await?;
    if session.owner_user_id != user.0.id {
        return Err(AppError::NotFound("upload session not found".into()));
    }
    if session.status != SessionStatus::Open {
        return Err(AppError::Conflict(format!(
            "upload session is {}",
            session.status.as_str()
        )));
    }
    if chunk_index < session.next_chunk_index {
        let stored = find_chunk(&state.db.pool, &session_id, chunk_index)
            .await?
            .ok_or_else(|| AppError::Conflict("upload chunk history is incomplete".into()))?;
        if stored.offset_bytes == i64::try_from(offset).unwrap_or(-1)
            && stored.size_bytes == i64::try_from(expected_size).unwrap_or(-1)
            && stored.sha256 == expected_hash
        {
            return Ok(session_response(session, StatusCode::OK));
        }
        return Err(AppError::Conflict(
            "upload chunk conflicts with the committed chunk".into(),
        ));
    }
    if chunk_index != session.next_chunk_index || offset != session.committed_offset {
        return Err(offset_conflict(session.committed_offset));
    }
    let content_length = req
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| AppError::BadRequest("Content-Length header is required".into()))?;
    if content_length != expected_size {
        return Err(AppError::BadRequest(format!(
            "chunk Content-Length must be {expected_size}"
        )));
    }

    let input_path = state.storage.data_root.join(&session.input_path);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&input_path)
        .await
        .map_err(AppError::Io)?;
    let current_length = file.metadata().await.map_err(AppError::Io)?.len();
    if current_length > offset {
        file.set_len(offset).await.map_err(AppError::Io)?;
    } else if current_length < offset {
        return Err(AppError::Conflict(
            "upload storage is behind the authoritative offset".into(),
        ));
    }
    file.seek(SeekFrom::Start(offset))
        .await
        .map_err(AppError::Io)?;

    let mut hasher = Sha256::new();
    let mut written = 0_u64;
    while let Some(item) = payload.next().await {
        let bytes = match item {
            Ok(bytes) => bytes,
            Err(error) => {
                truncate_after_failure(&mut file, offset).await;
                return Err(AppError::BadRequest(format!(
                    "failed to read chunk: {error}"
                )));
            }
        };
        let next = written
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| AppError::BadRequest("upload chunk is too large".into()))?;
        if next > expected_size {
            truncate_after_failure(&mut file, offset).await;
            return Err(AppError::BadRequest("upload chunk is too large".into()));
        }
        hasher.update(&bytes);
        if let Err(error) = file.write_all(&bytes).await {
            truncate_after_failure(&mut file, offset).await;
            return Err(AppError::Io(error));
        }
        written = next;
    }
    if written != expected_size {
        truncate_after_failure(&mut file, offset).await;
        return Err(AppError::BadRequest(
            "upload chunk body is incomplete".into(),
        ));
    }
    let actual_hash = format_digest(hasher.finalize().as_slice());
    if actual_hash != expected_hash {
        truncate_after_failure(&mut file, offset).await;
        return Err(AppError::BadRequest(
            "upload chunk SHA-256 does not match".into(),
        ));
    }
    if let Err(error) = file.sync_data().await {
        truncate_after_failure(&mut file, offset).await;
        return Err(AppError::Io(error));
    }
    drop(file);

    let committed = match record_chunk(
        &state.db.pool,
        &session_id,
        chunk_index,
        offset,
        expected_size,
        &expected_hash,
    )
    .await
    {
        Ok(session) => session,
        Err(error) => {
            let mut rollback = OpenOptions::new()
                .write(true)
                .open(&input_path)
                .await
                .map_err(AppError::Io)?;
            truncate_after_failure(&mut rollback, offset).await;
            return Err(error);
        }
    };
    Ok(session_response(committed, StatusCode::OK))
}

#[post("/upload-sessions/{session_id}/complete")]
pub async fn complete_upload_session(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    let session_id = path.into_inner();
    let lock = state.upload.session_lock(&session_id);
    let _guard = lock.lock().await;
    let session = crate::upload::session::get_session(&state.db.pool, &session_id).await?;
    if session.owner_user_id != user.0.id {
        return Err(AppError::NotFound("upload session not found".into()));
    }
    match session.status {
        SessionStatus::Open => {
            let expected_chunks = session
                .file_size_bytes
                .saturating_add(session.chunk_size_bytes - 1)
                / session.chunk_size_bytes;
            if session.committed_offset != session.file_size_bytes
                || session.next_chunk_index != expected_chunks
            {
                return Err(offset_conflict(session.committed_offset));
            }
            let finalizing = mark_finalizing(&state.db.pool, &session_id).await?;
            Ok(session_response(finalizing, StatusCode::ACCEPTED))
        }
        SessionStatus::Finalizing => Ok(session_response(session, StatusCode::ACCEPTED)),
        SessionStatus::Delivered => Ok(session_response(session, StatusCode::OK)),
        _ => Err(AppError::Conflict(format!(
            "upload session is {}",
            session.status.as_str()
        ))),
    }
}

fn required_u64_header(req: &HttpRequest, name: &'static str) -> Result<u64, AppError> {
    let header = HeaderName::from_static(match name {
        "X-Upload-Offset" => "x-upload-offset",
        _ => return Err(AppError::Config("unsupported upload header".into())),
    });
    req.headers()
        .get(header)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| AppError::BadRequest(format!("{name} header is required")))
}

fn required_sha256_header(req: &HttpRequest) -> Result<String, AppError> {
    let value = req
        .headers()
        .get("X-Chunk-SHA256")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(|value| value.to_ascii_lowercase())
        .ok_or_else(|| AppError::BadRequest("X-Chunk-SHA256 header is required".into()))?;
    Ok(value)
}

fn offset_conflict(offset: u64) -> AppError {
    AppError::public(
        StatusCode::CONFLICT,
        "UPLOAD_OFFSET_CONFLICT",
        format!("authoritative committed offset is {offset}"),
    )
}

async fn truncate_after_failure(file: &mut tokio::fs::File, offset: u64) {
    if let Err(error) = file.set_len(offset).await {
        tracing::error!(offset, %error, "failed to roll back upload chunk after failed append");
    }
}

fn format_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn validate_request(payload: &CreateUploadSessionRequest) -> Result<(), AppError> {
    let name = payload.file_name.trim();
    if name.is_empty() || name.chars().count() > 255 {
        return Err(AppError::BadRequest(
            "file_name must contain 1-255 characters".into(),
        ));
    }
    if payload.file_size_bytes < SESSION_MIN_FILE_SIZE_BYTES {
        return Err(AppError::BadRequest(
            "resumable upload requires a file of at least 64 MiB".into(),
        ));
    }
    if payload.idempotency_key.is_empty() || payload.idempotency_key.len() > 128 {
        return Err(AppError::BadRequest(
            "idempotency_key must contain 1-128 bytes".into(),
        ));
    }
    Ok(())
}

impl From<UploadSession> for UploadSessionResponse {
    fn from(session: UploadSession) -> Self {
        Self {
            session_id: session.id,
            issue_code: session.issue_code,
            file_name: session.file_name,
            file_size_bytes: session.file_size_bytes,
            chunk_size_bytes: session.chunk_size_bytes,
            committed_offset: session.committed_offset,
            next_chunk_index: session.next_chunk_index,
            status: session.status,
            bundle_id: session.bundle_id,
            failure_code: session.failure_code,
            failure_reason: session.failure_reason,
            expires_at: session.expires_at,
        }
    }
}
