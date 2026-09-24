use std::{path::Path, time::Instant};

use sha2::{Digest, Sha256};
use tokio::{
    fs::{self, File, OpenOptions},
    io::AsyncReadExt,
};
use tracing::warn;
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    upload::{
        job::{UploadJob, spawn_upload_job},
        multipart::{ReceiveReservation, TempBudget, UploadedFile},
        session::{
            SessionStatus, UploadSession, attach_processing_bundle, expire_sessions, get_session,
            list_finalizing, list_recoverable, mark_delivered, mark_failed,
        },
    },
};

const HASH_BUFFER_SIZE: usize = 1024 * 1024;

pub fn spawn(state: actix_web::web::Data<AppState>) -> tokio::task::JoinHandle<()> {
    crate::spawn_periodic_job(
        "resumable-upload-finalizer",
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(5),
        move || {
            let state = state.clone();
            async move {
                run_once(&state)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }
        },
    )
}

pub async fn run_once(state: &AppState) -> Result<u64, AppError> {
    let expired = expire_sessions(&state.db.pool).await?;
    for session in expired {
        TempBudget::release_persistent(&state.upload.tmp_bytes, session.file_size_bytes);
        let _ =
            fs::remove_dir_all(state.storage.data_root.join(".uploads").join(&session.id)).await;
        let _ = fs::remove_dir_all(state.storage.data_root.join(".tmp").join(&session.id)).await;
    }
    let sessions = list_finalizing(&state.db.pool).await?;
    let count = sessions.len() as u64;
    for session in sessions {
        if let Err(error) = finalize_one(state, &session.id).await {
            warn!(session_id = %session.id, %error, "resumable upload finalization failed; will retry");
        }
    }
    Ok(count)
}

pub async fn finalize_one(state: &AppState, session_id: &str) -> Result<(), AppError> {
    let lock = state.upload.session_lock(session_id);
    let _guard = lock.lock().await;
    finalize_one_locked(state, session_id).await
}

async fn finalize_one_locked(state: &AppState, session_id: &str) -> Result<(), AppError> {
    let mut session = match get_session(&state.db.pool, session_id).await {
        Ok(session) => session,
        Err(AppError::NotFound(_)) => return Ok(()),
        Err(error) => return Err(error),
    };
    if session.status != SessionStatus::Finalizing {
        return Ok(());
    }
    if session.committed_offset != session.file_size_bytes {
        return Err(AppError::Conflict(
            "upload session is not fully committed".into(),
        ));
    }

    let persistent_path = state.storage.data_root.join(&session.input_path);
    let storage_name = deterministic_storage_name(&session);
    let temp_dir = state.storage.data_root.join(".tmp").join(&session.id);
    let temp_path = temp_dir.join(&storage_name);

    if session.bundle_id.is_none() {
        let metadata = fs::metadata(&persistent_path).await.map_err(AppError::Io)?;
        if metadata.len() != session.file_size_bytes {
            return fail_session(
                state,
                &session,
                "UPLOAD_FILE_LENGTH_MISMATCH",
                "上传文件长度与已提交分片不一致",
                Some(&persistent_path),
                None,
            )
            .await;
        }
        let _file_hash = sha256_file(&persistent_path).await?;
        let bundle_id = Uuid::new_v4().simple().to_string();
        let bundle_hash = Uuid::new_v4().simple().to_string();
        session =
            attach_processing_bundle(&state.db.pool, &session.id, &bundle_id, &bundle_hash).await?;
    }

    let bundle_id = session
        .bundle_id
        .clone()
        .ok_or_else(|| AppError::Config("finalizing upload has no bundle id".into()))?;
    let (bundle_hash, bundle_status, bundle_stage) =
        load_bundle(&state.db.pool, &bundle_id).await?;
    if matches!(bundle_status.as_str(), "READY" | "FAILED")
        || (bundle_status == "PROCESSING" && bundle_stage != "RECEIVING")
    {
        mark_delivered(&state.db.pool, &session.id, &bundle_id).await?;
        return Ok(());
    }

    fs::create_dir_all(&temp_dir).await.map_err(AppError::Io)?;
    if fs::metadata(&temp_path).await.is_err() {
        match fs::rename(&persistent_path, &temp_path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return fail_session(
                    state,
                    &session,
                    "UPLOAD_INPUT_MISSING",
                    "上传文件暂存内容丢失",
                    None,
                    Some(&temp_dir),
                )
                .await;
            }
            Err(error) => return Err(AppError::Io(error)),
        }
    }
    let temp_metadata = fs::metadata(&temp_path).await.map_err(AppError::Io)?;
    if temp_metadata.len() != session.file_size_bytes {
        return fail_session(
            state,
            &session,
            "UPLOAD_FILE_LENGTH_MISMATCH",
            "上传文件长度与已提交分片不一致",
            None,
            Some(&temp_dir),
        )
        .await;
    }

    let settings_snapshot = state.settings.snapshot().await;
    let receive_reservation = ReceiveReservation::adopt_persistent(
        state.upload.tmp_bytes.clone(),
        state.upload.tmp_max_bytes.clone(),
        session.file_size_bytes,
    );
    let job = UploadJob {
        received_at: Instant::now(),
        pool: state.db.pool.clone(),
        data_root: state.storage.data_root.clone(),
        blob_store: state.storage.blob_store.clone(),
        temp_dir: temp_dir.clone(),
        staging_root: temp_dir.join("staging"),
        processing_permits: state.upload.processing_permits.clone(),
        archive_config: crate::config::ArchiveConfig::for_content_limit_with_working_size(
            settings_snapshot.effective.issue_max_content_size,
            settings_snapshot.effective.archive_max_working_size,
        ),
        indexing_config: crate::config::IndexingConfig {
            max_indexed_line_size: settings_snapshot.effective.indexing_max_indexed_line_size,
        },
        request_id: None,
        issue_code: session.issue_code.clone(),
        issue_max_content_size: settings_snapshot.effective.issue_max_content_size,
        settings: state.settings.clone(),
        bundle_id: bundle_id.clone(),
        bundle_hash,
        files: vec![UploadedFile {
            original_name: session.file_name.clone(),
            display_name: crate::upload::filename::sanitize_filename(&session.file_name),
            storage_name,
            temp_path,
            size_bytes: session.file_size_bytes,
            content_type: None,
        }],
        receive_reservation,
        temp_cleanup_queue: state.upload.temp_cleanup_queue.clone(),
        search_backend: state.search_backend,
        search_resource_budget: state.search.tantivy_budget.clone(),
    };
    spawn_upload_job(job);
    mark_delivered(&state.db.pool, &session.id, &bundle_id).await?;
    Ok(())
}

pub async fn reconcile_startup(pool: &sqlx::SqlitePool, data_root: &Path) -> Result<u64, AppError> {
    let sessions = list_recoverable(pool).await?;
    let mut changed = 0;
    for session in sessions {
        let path = data_root.join(&session.input_path);
        let Ok(metadata) = fs::metadata(&path).await else {
            continue;
        };
        if metadata.len() > session.committed_offset {
            let file = OpenOptions::new()
                .write(true)
                .open(&path)
                .await
                .map_err(AppError::Io)?;
            file.set_len(session.committed_offset)
                .await
                .map_err(AppError::Io)?;
            changed += 1;
        } else if metadata.len() < session.committed_offset {
            mark_failed(
                pool,
                &session.id,
                "UPLOAD_STORAGE_BEHIND",
                "上传临时文件短于数据库已提交偏移",
            )
            .await?;
            changed += 1;
        }
    }
    Ok(changed)
}

async fn fail_session(
    state: &AppState,
    session: &UploadSession,
    code: &str,
    reason: &str,
    persistent_path: Option<&Path>,
    temp_dir: Option<&Path>,
) -> Result<(), AppError> {
    mark_failed(&state.db.pool, &session.id, code, reason).await?;
    TempBudget::release_persistent(&state.upload.tmp_bytes, session.file_size_bytes);
    if let Some(path) = persistent_path {
        let _ = fs::remove_file(path).await;
    }
    if let Some(path) = temp_dir {
        let _ = fs::remove_dir_all(path).await;
    }
    Ok(())
}

async fn load_bundle(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
) -> Result<(String, String, String), AppError> {
    sqlx::query_as::<_, (String, String, String)>(
        "SELECT hash, status, process_stage FROM bundles WHERE id=?",
    )
    .bind(bundle_id)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound("processing bundle not found".into()))
}

fn deterministic_storage_name(session: &UploadSession) -> String {
    format!(
        "{}-{}",
        session.id,
        crate::upload::filename::sanitize_filename(&session.file_name)
    )
}

async fn sha256_file(path: &Path) -> Result<String, AppError> {
    let mut file = File::open(path).await.map_err(AppError::Io)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];
    loop {
        let read = file.read(&mut buffer).await.map_err(AppError::Io)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
