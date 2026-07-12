use actix_multipart::{Field, Multipart};
use actix_web::{HttpResponse, post, web};
use futures_util::TryStreamExt;
use serde::Serialize;
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    ingest::{ProcessFileOptions, process_uploaded_file},
};

use super::issues::ensure_issue;

const MAX_UPLOAD_FILES: usize = 100;
const MAX_UPLOAD_FILE_BYTES: usize = 50 * 1024 * 1024;
const MAX_UPLOAD_TOTAL_BYTES: usize = 200 * 1024 * 1024;
const MAX_TEXT_FIELD_BYTES: usize = 64 * 1024;

// scoped under /api in routes::register, so use relative path
#[post("/uploads")]
pub async fn upload_logs(
    state: web::Data<AppState>,
    mut payload: Multipart,
) -> Result<HttpResponse, AppError> {
    let mut issue_code_field: Option<String> = None;
    let mut files: Vec<UploadedFile> = Vec::new();

    while let Some(mut field) = payload
        .try_next()
        .await
        .map_err(|err| AppError::BadRequest(format!("multipart error: {err}")))?
    {
        let content_disposition = field.content_disposition().clone();
        let field_name = content_disposition.get_name().unwrap_or("").to_string();

        match field_name.as_str() {
            "issue_code" => {
                let value = collect_text_field(&mut field).await?;
                issue_code_field = Some(value);
            }
            "files" => {
                let filename = content_disposition
                    .get_filename()
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| "upload.log".into());

                let content_type = field.content_type().map(|mime| mime.to_string());

                let bytes =
                    collect_binary_field(&mut field, MAX_UPLOAD_FILE_BYTES, &filename).await?;

                if !bytes.is_empty() {
                    if files.len() >= MAX_UPLOAD_FILES {
                        return Err(AppError::BadRequest(format!(
                            "too many files; max {MAX_UPLOAD_FILES} files per upload"
                        )));
                    }
                    let sanitized = sanitize_filename(&filename);
                    files.push(UploadedFile {
                        original_name: filename,
                        sanitized_name: sanitized,
                        bytes,
                        content_type,
                    });
                }
            }
            _ => {
                // Ignore unknown fields
                collect_binary_field(&mut field, MAX_TEXT_FIELD_BYTES, &field_name).await?;
            }
        }
    }

    let issue_code = issue_code_field
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest("issue_code is required".into()))?;

    if files.is_empty() {
        return Err(AppError::BadRequest("no files provided".into()));
    }

    let bundle_hash = Uuid::new_v4().simple().to_string();
    let bundle_name = format!("bundle-{bundle_hash}");

    ensure_issue(&state.pool, &issue_code).await?;

    let total_bytes: i64 = files.iter().map(|file| file.bytes.len() as i64).sum();
    if total_bytes as usize > MAX_UPLOAD_TOTAL_BYTES {
        return Err(AppError::BadRequest(format!(
            "upload is too large; max total size is {} MB",
            MAX_UPLOAD_TOTAL_BYTES / 1024 / 1024
        )));
    }

    let bundle_id = Uuid::new_v4().simple().to_string();

    sqlx::query(
        r#"
        INSERT INTO bundles (id, issue_code, hash, name, status, size_bytes)
        VALUES (?, ?, ?, ?, 'PROCESSING', ?)
        "#,
    )
    .bind(&bundle_id)
    .bind(&issue_code)
    .bind(&bundle_hash)
    .bind(&bundle_name)
    .bind(Some(total_bytes))
    .execute(&state.pool)
    .await
    .map_err(AppError::Database)?;

    let process_result = async {
        for uploaded in &files {
            process_uploaded_file(ProcessFileOptions {
                pool: &state.pool,
                bundle_id: &bundle_id,
                bundle_hash: &bundle_hash,
                data_root: &state.data_root,
                file_name: &uploaded.sanitized_name,
                original_name: &uploaded.original_name,
                content_type: uploaded.content_type.as_deref(),
                bytes: &uploaded.bytes,
            })
            .await?;
        }
        Ok::<(), AppError>(())
    }
    .await;

    if let Err(error) = process_result {
        update_bundle_status(&state.pool, &bundle_id, "FAILED").await?;
        return Err(error);
    }

    update_bundle_status(&state.pool, &bundle_id, "READY").await?;

    Ok(HttpResponse::Ok().json(UploadResponse {
        issue_code,
        bundle_hash,
        file_count: files.len() as u64,
        total_bytes: total_bytes as u64,
    }))
}

#[derive(Serialize)]
struct UploadResponse {
    issue_code: String,
    bundle_hash: String,
    file_count: u64,
    total_bytes: u64,
}

struct UploadedFile {
    original_name: String,
    sanitized_name: String,
    bytes: Vec<u8>,
    content_type: Option<String>,
}

async fn collect_text_field(field: &mut Field) -> Result<String, AppError> {
    let bytes = collect_binary_field(field, MAX_TEXT_FIELD_BYTES, "text field").await?;
    let value = String::from_utf8(bytes)
        .map_err(|_| AppError::BadRequest("field is not valid UTF-8".into()))?;
    Ok(value.trim().to_string())
}

async fn collect_binary_field(
    field: &mut Field,
    limit: usize,
    label: &str,
) -> Result<Vec<u8>, AppError> {
    let mut data = Vec::new();
    while let Some(chunk) = field
        .try_next()
        .await
        .map_err(|err| AppError::BadRequest(format!("failed to read field: {err}")))?
    {
        if data.len() + chunk.len() > limit {
            return Err(AppError::BadRequest(format!(
                "{label} is too large; max size is {}",
                format_bytes(limit)
            )));
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

async fn update_bundle_status(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    status: &str,
) -> Result<(), AppError> {
    sqlx::query("UPDATE bundles SET status = ? WHERE id = ?")
        .bind(status)
        .bind(bundle_id)
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
    Ok(())
}

fn format_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{} MB", bytes / 1024 / 1024)
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

fn sanitize_filename(name: &str) -> String {
    use std::path::Path;
    let fallback = "upload.log";
    let file_name = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(fallback);
    let sanitized: String = file_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        fallback.into()
    } else {
        sanitized
    }
}
