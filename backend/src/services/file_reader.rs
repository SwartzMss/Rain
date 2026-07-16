use serde::Serialize;
use serde_json::json;
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, BufReader},
};

use crate::{
    config::ApiConfig,
    error::AppError,
    ingest::{decode_log_line, read_line_bytes_limited},
    repositories::files::{FileRow, ensure_text_preview, nearest_line_offset, resolve_file_path},
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
}

pub async fn read_file_preview(
    record: &FileRow,
    data_root: &std::path::Path,
    api: &ApiConfig,
) -> Result<serde_json::Value, AppError> {
    if record.is_dir {
        return Err(AppError::BadRequest("cannot read directory content".into()));
    }
    ensure_text_preview(record)?;

    let disk_path = resolve_file_path(record, data_root)?;
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
    data_root: &std::path::Path,
    api: &ApiConfig,
    start: i64,
    limit: i64,
) -> Result<FileLinesResponse, AppError> {
    if record.is_dir {
        return Err(AppError::BadRequest("cannot read directory content".into()));
    }
    ensure_text_preview(record)?;

    let (base_line, byte_offset) = nearest_line_offset(pool, record.id, start).await?;
    let disk_path = resolve_file_path(record, data_root)?;

    let mut file = File::open(&disk_path).await.map_err(AppError::Io)?;
    file.seek(std::io::SeekFrom::Start(byte_offset as u64))
        .await
        .map_err(AppError::Io)?;
    let mut reader = BufReader::new(file);
    let mut current_line = base_line;
    let end_line = start.saturating_add(limit);
    let mut lines = Vec::new();
    let mut buffer = Vec::new();

    while current_line < end_line {
        let Some((_read, truncated)) = read_line_bytes_limited(
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
            lines.push(FileLine {
                line_number: current_line,
                content: decode_log_line(&buffer, truncated),
                truncated,
            });
        }
        current_line += 1;
    }

    let next_start = if lines.len() as i64 == limit {
        Some(start + limit)
    } else {
        None
    };

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
