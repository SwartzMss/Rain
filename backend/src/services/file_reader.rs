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
    services::json_size::{
        JsonLinePageDecision, RESPONSE_TRUNCATED_LINE_MARKER, fit_json_line_to_page,
        json_string_encoded_len,
    },
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

    if record.line_count.is_some_and(|count| start >= count) {
        return Ok(FileLinesResponse {
            path: record.path.clone(),
            size_bytes: record.size_bytes,
            line_count: record.line_count,
            start,
            limit,
            next_start: None,
            lines: Vec::new(),
        });
    }

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
    let page_base_bytes = json_string_encoded_len(&record.path).saturating_add(256);
    let mut page_bytes = page_base_bytes;
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
            let decoded_content = decode_log_line(&buffer, truncated);
            let fixed_line_bytes = 256_u64;
            let decision = fit_json_line_to_page(
                &decoded_content,
                fixed_line_bytes,
                page_base_bytes,
                page_bytes,
                api.max_line_page_bytes,
                RESPONSE_TRUNCATED_LINE_MARKER,
            );
            let (content, line_bytes, response_truncated) = match decision {
                JsonLinePageDecision::Include {
                    content,
                    line_bytes,
                    response_truncated,
                } => (content, line_bytes, response_truncated),
                JsonLinePageDecision::Defer => {
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
                JsonLinePageDecision::TooLarge => {
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
            };
            page_bytes = page_bytes.saturating_add(line_bytes);
            let truncated = truncated || response_truncated;
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

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::read_file_lines;
    use crate::{
        blob_store::{BlobStore, LocalCasBlobStore},
        config::ApiConfig,
        repositories::files::FileRow,
    };

    async fn read_test_file(
        content: String,
        max_page_bytes: u64,
    ) -> (
        crate::services::file_reader::FileLinesResponse,
        FileRow,
        LocalCasBlobStore,
        sqlx::SqlitePool,
        std::path::PathBuf,
    ) {
        let root = std::env::temp_dir().join(format!("rain-file-page-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let source = root.join("source.log");
        tokio::fs::write(&source, content).await.unwrap();
        let store = LocalCasBlobStore::new(root.clone());
        let stored = store.put(&source).await.unwrap();
        let record = FileRow {
            id: 1,
            parent_id: None,
            name: "app.log".into(),
            path: "app.log".into(),
            is_dir: false,
            size_bytes: None,
            line_count: Some(3),
            mime_type: Some("text/plain".into()),
            status: Some("READY".into()),
            meta: None,
            blob_id: Some(1),
            storage_backend: Some(stored.storage_backend.into()),
            storage_key: Some(stored.storage_key),
            blob_state: Some("READY".into()),
        };
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE log_line_offsets (file_id INTEGER, line_number INTEGER, byte_offset INTEGER)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut api = ApiConfig::default();
        api.max_line_page_bytes = max_page_bytes;
        api.max_preview_line_size = 1024;
        let response = read_file_lines(&pool, &record, &store, &api, 0, 3)
            .await
            .unwrap();
        (response, record, store, pool, root)
    }

    #[tokio::test]
    async fn page_budget_defers_complete_file_line_that_fits_on_an_empty_page() {
        let medium = "m".repeat(70);
        let (first_page, record, store, pool, root) =
            read_test_file(format!("small\n{medium}\ntail\n"), 600).await;
        assert_eq!(first_page.lines.len(), 1);
        assert_eq!(first_page.lines[0].content, "small");
        assert!(!first_page.lines[0].truncated);
        assert_eq!(first_page.next_start, Some(1));

        let second_page = read_file_lines(
            &pool,
            &record,
            &store,
            &ApiConfig {
                max_line_page_bytes: 600,
                max_preview_line_size: 1024,
                ..ApiConfig::default()
            },
            1,
            1,
        )
        .await
        .unwrap();
        assert_eq!(second_page.lines.len(), 1);
        assert_eq!(second_page.lines[0].content, medium);
        assert!(!second_page.lines[0].truncated);

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn oversized_file_line_is_truncated_without_blocking_tail() {
        let (first_page, record, store, pool, root) =
            read_test_file(format!("small\n{}\ntail\n", "\"".repeat(200)), 600).await;
        assert_eq!(first_page.lines.len(), 1);
        assert_eq!(first_page.next_start, Some(1));

        let api = ApiConfig {
            max_line_page_bytes: 600,
            max_preview_line_size: 1024,
            ..ApiConfig::default()
        };
        let second_page = read_file_lines(&pool, &record, &store, &api, 1, 1)
            .await
            .unwrap();
        assert_eq!(second_page.lines.len(), 1);
        assert!(
            second_page.lines[0]
                .content
                .ends_with("[response truncated]")
        );
        assert!(second_page.lines[0].truncated);

        let third_page = read_file_lines(&pool, &record, &store, &api, 2, 1)
            .await
            .unwrap();
        assert_eq!(third_page.lines[0].content, "tail");
        assert!(!third_page.lines[0].truncated);

        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
