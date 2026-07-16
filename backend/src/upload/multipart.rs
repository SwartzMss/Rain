use std::path::{Path, PathBuf};

use actix_multipart::{Field, Multipart};
use futures_util::TryStreamExt;
use tokio::{fs, io::AsyncWriteExt};

use crate::{
    error::AppError,
    ingest::limits::{MAX_MULTIPART_TEXT_FIELD_SIZE, MAX_UPLOAD_FILES},
};

use super::filename::{format_bytes, sanitize_filename, unique_storage_name};

pub struct UploadedFile {
    pub original_name: String,
    pub display_name: String,
    pub storage_name: String,
    pub temp_path: PathBuf,
    pub size_bytes: u64,
    pub content_type: Option<String>,
}

pub struct MultipartUpload {
    pub files: Vec<UploadedFile>,
    pub total_bytes: u64,
}

pub async fn collect_multipart_upload(
    mut payload: Multipart,
    temp_dir: &Path,
) -> Result<MultipartUpload, AppError> {
    let mut files: Vec<UploadedFile> = Vec::new();
    let mut total_bytes: u64 = 0;

    while let Some(mut field) = payload
        .try_next()
        .await
        .map_err(|err| AppError::BadRequest(format!("multipart error: {err}")))?
    {
        let content_disposition = field.content_disposition().clone();
        let field_name = content_disposition.get_name().unwrap_or("").to_string();

        match field_name.as_str() {
            "issue_code" => {
                collect_text_field(&mut field, MAX_MULTIPART_TEXT_FIELD_SIZE).await?;
            }
            "files" => {
                let filename = content_disposition
                    .get_filename()
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| "upload.log".into());

                let content_type = field.content_type().map(|mime| mime.to_string());
                let display_name = sanitize_filename(&filename);
                let storage_name = unique_storage_name(&filename);
                let temp_name = format!("{}-{storage_name}", files.len());
                let temp_path = temp_dir.join(temp_name);
                let size_bytes =
                    collect_file_field(&mut field, &temp_path, u64::MAX, &filename).await?;

                if size_bytes > 0 {
                    if files.len() >= MAX_UPLOAD_FILES {
                        return Err(AppError::BadRequest(format!(
                            "too many files; max {} files per upload",
                            MAX_UPLOAD_FILES
                        )));
                    }
                    total_bytes = total_bytes.saturating_add(size_bytes);
                    files.push(UploadedFile {
                        original_name: filename,
                        display_name,
                        storage_name,
                        temp_path,
                        size_bytes,
                        content_type,
                    });
                } else {
                    let _ = fs::remove_file(&temp_path).await;
                }
            }
            _ => {
                // Ignore unknown fields.
                collect_binary_field(&mut field, MAX_MULTIPART_TEXT_FIELD_SIZE, &field_name)
                    .await?;
            }
        }
    }

    if files.is_empty() {
        return Err(AppError::BadRequest("no files provided".into()));
    }

    Ok(MultipartUpload { files, total_bytes })
}

async fn collect_text_field(field: &mut Field, limit: u64) -> Result<String, AppError> {
    let bytes = collect_binary_field(field, limit, "text field").await?;
    let value = String::from_utf8(bytes)
        .map_err(|_| AppError::BadRequest("field is not valid UTF-8".into()))?;
    Ok(value.trim().to_string())
}

async fn collect_binary_field(
    field: &mut Field,
    limit: u64,
    label: &str,
) -> Result<Vec<u8>, AppError> {
    let mut data = Vec::new();
    while let Some(chunk) = field
        .try_next()
        .await
        .map_err(|err| AppError::BadRequest(format!("failed to read field: {err}")))?
    {
        if (data.len() as u64).saturating_add(chunk.len() as u64) > limit {
            return Err(AppError::BadRequest(format!(
                "{label} is too large; max size is {}",
                format_bytes(limit)
            )));
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

async fn collect_file_field(
    field: &mut Field,
    path: &Path,
    limit: u64,
    label: &str,
) -> Result<u64, AppError> {
    let mut file = fs::File::create(path).await.map_err(AppError::Io)?;
    let mut written = 0u64;
    while let Some(chunk) = field
        .try_next()
        .await
        .map_err(|err| AppError::BadRequest(format!("failed to read field: {err}")))?
    {
        written = written.saturating_add(chunk.len() as u64);
        if written > limit {
            return Err(AppError::BadRequest(format!(
                "{label} is too large; max size is {}",
                format_bytes(limit)
            )));
        }
        file.write_all(&chunk).await.map_err(AppError::Io)?;
    }
    file.flush().await.map_err(AppError::Io)?;
    Ok(written)
}
