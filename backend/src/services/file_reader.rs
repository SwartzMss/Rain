use serde::Serialize;
use serde_json::json;
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, BufReader},
};

use crate::{
    blob_store::BlobStore,
    config::ApiConfig,
    error::AppError,
    ingest::{decode_log_line, read_line_bytes_limited},
    repositories::files::{FileRow, ensure_text_preview, nearest_line_offset, resolve_file_path},
    services::json_size::json_string_encoded_len,
};

#[derive(Serialize)]
pub struct FileLinesResponse {
    path: String,
    size_bytes: Option<i64>,
    line_count: Option<i64>,
    start: i64,
    limit: i64,
    next_start: Option<i64>,
    lines: Vec<FileLine>,
}

#[derive(Serialize)]
struct FileLine {
    line_number: i64,
    content: String,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_length: Option<usize>,
}

pub async fn read_file_preview(
    record: &FileRow,
    blob_store: &dyn BlobStore,
    api: &ApiConfig,
) -> Result<serde_json::Value, AppError> {
    if record.is_dir {
        return Err(AppError::BadRequest("cannot read directory content".into()));
    }
    ensure_text_preview(record)?;

    let disk_path = resolve_file_path(record, blob_store).await?;
    let metadata = tokio::fs::metadata(&disk_path)
        .await
        .map_err(AppError::Io)?;
    let size_bytes = metadata.len();
    let file = File::open(&disk_path).await.map_err(AppError::Io)?;
    let mut buffer = Vec::new();
    let mut limited = file.take(api.file_preview_size);
    limited
        .read_to_end(&mut buffer)
        .await
        .map_err(AppError::Io)?;

    let preview = String::from_utf8_lossy(&buffer).to_string();
    let truncated = size_bytes > api.file_preview_size;

    Ok(json!({
        "path": record.path,
        "size_bytes": record.size_bytes.unwrap_or(size_bytes as i64),
        "mime_type": record.mime_type,
        "preview": preview,
        "truncated": truncated,
    }))
}

pub async fn read_file_lines(
    pool: &sqlx::SqlitePool,
    record: &FileRow,
    blob_store: &dyn BlobStore,
    api: &ApiConfig,
    start: i64,
    limit: i64,
) -> Result<FileLinesResponse, AppError> {
    if record.is_dir {
        return Err(AppError::BadRequest("cannot read directory content".into()));
    }
    ensure_text_preview(record)?;

    let (base_line, byte_offset) = nearest_line_offset(pool, record.id, start).await?;
    let disk_path = resolve_file_path(record, blob_store).await?;

    let mut file = File::open(&disk_path).await.map_err(AppError::Io)?;
    file.seek(std::io::SeekFrom::Start(byte_offset as u64))
        .await
        .map_err(AppError::Io)?;
    let mut reader = BufReader::new(file);
    let mut current_line = base_line;
    let end_line = start.saturating_add(limit);
    let mut lines = Vec::new();
    let mut buffer = Vec::new();
    let mut page_bytes = json_string_encoded_len(&record.path).saturating_add(256);
    let mut stopped_by_page_bytes = false;

    while current_line < end_line {
        let Some((_read, original_length, truncated)) = read_line_bytes_limited(
            &mut reader,
            &mut buffer,
            usize::try_from(api.max_preview_line_size).map_err(|_| {
                AppError::Config(
                    "RAIN_API_MAX_PREVIEW_LINE_SIZE cannot be represented on this platform".into(),
                )
            })?,
        )
        .await
        .map_err(AppError::Io)?
        else {
            break;
        };

        if current_line >= start {
            let content = decode_log_line(&buffer, truncated);
            let line_bytes = json_string_encoded_len(&content).saturating_add(256);
            if line_bytes > api.max_line_page_bytes
                || page_bytes.saturating_add(line_bytes) > api.max_line_page_bytes
            {
                if lines.is_empty() {
                    return Err(AppError::public(
                        actix_web::http::StatusCode::PAYLOAD_TOO_LARGE,
                        "LINE_PAGE_TOO_LARGE",
                        "单行或行分页结果超过字节限制",
                    ));
                }
                stopped_by_page_bytes = true;
                break;
            }
            page_bytes = page_bytes.saturating_add(line_bytes);
            lines.push(FileLine {
                line_number: current_line,
                content,
                truncated,
                original_length: truncated.then_some(original_length),
            });
        }
        current_line += 1;
    }

    let next_start = start.checked_add(lines.len() as i64).filter(|next| {
        (lines.len() as i64 == limit || stopped_by_page_bytes)
            && record.line_count.is_none_or(|count| *next < count)
    });

    Ok(FileLinesResponse {
        path: record.path.clone(),
        size_bytes: record.size_bytes,
        line_count: record.line_count,
        start,
        limit,
        next_start,
        lines,
    })
}
