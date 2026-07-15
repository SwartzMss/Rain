use flate2::read::GzDecoder;
use std::{
    collections::HashSet,
    fs::File as StdFile,
    future::Future,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::{fs, task};

use crate::{
    config::{ArchiveConfig, IndexingConfig},
    error::AppError,
    file_classification::{PreviewKind, classify_file, effective_mime_type},
};

const TRUNCATED_LINE_MARKER: &str = " ... [line truncated]";
const LOG_LEVELS: [&str; 7] = [
    "TRACE", "DEBUG", "INFO", "WARN", "WARNING", "ERROR", "FATAL",
];

#[derive(Clone)]
pub struct ArchiveBudget {
    counters: Arc<Mutex<ArchiveCounters>>,
    config: ArchiveConfig,
}

impl Default for ArchiveBudget {
    fn default() -> Self {
        Self::new(ArchiveConfig::default())
    }
}

#[derive(Default)]
struct ArchiveCounters {
    entries: usize,
    extracted_bytes: u64,
}

impl ArchiveBudget {
    pub fn new(config: ArchiveConfig) -> Self {
        Self {
            counters: Arc::new(Mutex::new(ArchiveCounters::default())),
            config,
        }
    }

    fn reserve_entry(&self) -> Result<(), AppError> {
        let mut counters = self
            .counters
            .lock()
            .map_err(|_| AppError::BadRequest("archive budget lock poisoned".into()))?;
        counters.entries = counters
            .entries
            .checked_add(1)
            .ok_or_else(|| AppError::BadRequest("archive entry count overflow".into()))?;
        if counters.entries > self.config.max_entries {
            return Err(AppError::BadRequest(format!(
                "archive bundle has too many entries; max {}",
                self.config.max_entries
            )));
        }
        Ok(())
    }

    fn reserve_bytes(&self, size_bytes: u64) -> Result<(), AppError> {
        let mut counters = self
            .counters
            .lock()
            .map_err(|_| AppError::BadRequest("archive budget lock poisoned".into()))?;
        counters.extracted_bytes = counters
            .extracted_bytes
            .checked_add(size_bytes)
            .ok_or_else(|| AppError::BadRequest("archive extracted size overflow".into()))?;
        if counters.extracted_bytes > self.config.max_extracted_size {
            return Err(AppError::BadRequest(format!(
                "archive bundle exceeds configured extracted size; max bundle size {}",
                format_binary_size(self.config.max_extracted_size)
            )));
        }
        Ok(())
    }

    fn remaining_bytes(&self) -> Result<u64, AppError> {
        let counters = self
            .counters
            .lock()
            .map_err(|_| AppError::BadRequest("archive budget lock poisoned".into()))?;
        Ok(self
            .config
            .max_extracted_size
            .saturating_sub(counters.extracted_bytes))
    }
}

pub struct ProcessFileOptions<'a> {
    pub pool: &'a sqlx::SqlitePool,
    pub bundle_id: &'a str,
    pub bundle_hash: &'a str,
    pub data_root: &'a Path,
    pub storage_name: &'a str,
    pub original_name: &'a str,
    pub display_name: &'a str,
    pub content_type: Option<&'a str>,
    pub source_path: &'a Path,
    pub size_bytes: u64,
    pub archive_budget: ArchiveBudget,
    pub indexing: &'a IndexingConfig,
}

pub async fn process_uploaded_file(options: ProcessFileOptions<'_>) -> Result<(), AppError> {
    let ProcessFileOptions {
        pool,
        bundle_id,
        bundle_hash,
        data_root,
        storage_name,
        original_name,
        display_name,
        content_type,
        source_path,
        size_bytes,
        archive_budget,
        indexing,
    } = options;

    let bundle_dir = data_root.join(bundle_hash);
    fs::create_dir_all(&bundle_dir)
        .await
        .map_err(|error| io_error_at("create bundle staging directory", &bundle_dir, error))?;

    let disk_path = bundle_dir.join(storage_name);
    move_or_copy_file(source_path, &disk_path).await?;
    let mime_type = effective_mime_type(original_name, content_type);
    let preview_kind = classify_file(&disk_path, original_name, mime_type.as_deref()).await?;

    let relative_path = format!("/{bundle_hash}/{storage_name}");
    let meta = serde_json::json!({
        "original_name": original_name,
        "display_name": display_name,
        "storage_name": storage_name,
        "storage_path": disk_path.to_string_lossy(),
        "kind": "uploaded_file",
        "preview_kind": preview_kind.as_str()
    });

    let file_id = insert_file_record(
        pool,
        bundle_id,
        None,
        display_name,
        &relative_path,
        false,
        Some(size_bytes as i64),
        mime_type.as_deref(),
        Some(meta),
    )
    .await?;

    if preview_kind == PreviewKind::Text {
        update_process_stage(pool, bundle_id, "INDEXING").await?;
        ingest_text_file(pool, bundle_id, file_id, &disk_path, size_bytes, indexing).await?;
    }

    if preview_kind == PreviewKind::Archive {
        let extracted_dir_name = format!("{storage_name}_extracted");
        let extracted_dir = bundle_dir.join(&extracted_dir_name);
        fs::create_dir_all(&extracted_dir).await.map_err(|error| {
            io_error_at("create archive extraction directory", &extracted_dir, error)
        })?;

        update_process_stage(pool, bundle_id, "EXTRACTING").await?;
        extract_archive(
            original_name,
            &disk_path,
            &extracted_dir,
            archive_budget.clone(),
        )
        .await?;

        let extracted_relative_path = format!("/{bundle_hash}/{extracted_dir_name}");
        let dir_meta = serde_json::json!({
            "source": original_name,
            "storage_name": extracted_dir_name,
            "storage_path": extracted_dir.to_string_lossy(),
            "kind": "extracted_dir"
        });

        let dir_id = insert_file_record(
            pool,
            bundle_id,
            Some(file_id),
            &format!("{display_name}_extracted"),
            &extracted_relative_path,
            true,
            None,
            None,
            Some(dir_meta),
        )
        .await?;

        update_process_stage(pool, bundle_id, "INDEXING").await?;
        ingest_directory(
            pool,
            bundle_id,
            dir_id,
            extracted_dir,
            format!("{}/{extracted_dir_name}", bundle_hash),
            archive_budget,
            indexing,
            1,
        )
        .await?;
    }

    Ok(())
}

async fn update_process_stage(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    stage: &str,
) -> Result<(), AppError> {
    sqlx::query("UPDATE bundles SET process_stage = ? WHERE id = ?")
        .bind(stage)
        .bind(bundle_id)
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn ingest_directory<'a>(
    pool: &'a sqlx::SqlitePool,
    bundle_id: &'a str,
    parent_id: i64,
    dir_path: PathBuf,
    relative_root: String,
    archive_budget: ArchiveBudget,
    indexing: &'a IndexingConfig,
    archive_depth: usize,
) -> Pin<Box<dyn Future<Output = Result<(), AppError>> + Send + 'a>> {
    Box::pin(async move {
        let mut read_dir = fs::read_dir(&dir_path)
            .await
            .map_err(|error| io_error_at("read extracted directory", &dir_path, error))?;
        let mut entries = Vec::new();
        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|error| io_error_at("read extracted directory entry", &dir_path, error))?
        {
            entries.push(entry.path());
        }
        entries.sort();

        for disk_path in entries {
            let name = disk_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("unknown")
                .to_string();
            let metadata = fs::metadata(&disk_path)
                .await
                .map_err(|error| io_error_at("read extracted entry metadata", &disk_path, error))?;
            let is_dir = metadata.is_dir();
            let size_bytes = metadata.is_file().then_some(metadata.len() as i64);
            let db_path = format!("/{}/{}", relative_root.trim_start_matches('/'), name);
            let mime_type = (!is_dir)
                .then(|| effective_mime_type(&name, None))
                .flatten();
            let preview_kind = if is_dir {
                PreviewKind::Directory
            } else {
                classify_file(&disk_path, &name, mime_type.as_deref()).await?
            };
            let meta = serde_json::json!({
                "storage_path": disk_path.to_string_lossy(),
                "kind": if is_dir { "extracted_dir" } else { "extracted_file" },
                "preview_kind": preview_kind.as_str()
            });

            let record_id = insert_file_record(
                pool,
                bundle_id,
                Some(parent_id),
                &name,
                &db_path,
                is_dir,
                size_bytes,
                mime_type.as_deref(),
                Some(meta),
            )
            .await?;

            if is_dir {
                ingest_directory(
                    pool,
                    bundle_id,
                    record_id,
                    disk_path,
                    format!("{relative_root}/{name}"),
                    archive_budget.clone(),
                    indexing,
                    archive_depth,
                )
                .await?;
                continue;
            }

            if preview_kind == PreviewKind::Text
                && let Some(size) = size_bytes
            {
                ingest_text_file(
                    pool,
                    bundle_id,
                    record_id,
                    &disk_path,
                    size as u64,
                    indexing,
                )
                .await?;
            }

            if preview_kind == PreviewKind::Archive {
                if archive_depth >= archive_budget.config.max_recursion_depth {
                    return Err(AppError::BadRequest(format!(
                        "archive recursion is too deep; max {}: {db_path}",
                        archive_budget.config.max_recursion_depth
                    )));
                }

                let extracted_dir_name = format!("{name}_extracted");
                let extracted_dir = dir_path.join(&extracted_dir_name);
                validate_extracted_path(
                    &extracted_dir,
                    &db_path,
                    archive_budget.config.max_output_path_chars,
                )?;
                if fs::metadata(&extracted_dir).await.is_ok() {
                    return Err(AppError::BadRequest(format!(
                        "archive extraction output already exists: {}",
                        extracted_dir.display()
                    )));
                }
                fs::create_dir_all(&extracted_dir).await.map_err(|error| {
                    io_error_at(
                        "create nested archive extraction directory",
                        &extracted_dir,
                        error,
                    )
                })?;
                extract_archive(&name, &disk_path, &extracted_dir, archive_budget.clone()).await?;

                let extracted_db_path = format!("{db_path}_extracted");
                let dir_meta = serde_json::json!({
                    "source": name,
                    "storage_name": extracted_dir_name,
                    "storage_path": extracted_dir.to_string_lossy(),
                    "kind": "extracted_dir"
                });
                let dir_id = insert_file_record(
                    pool,
                    bundle_id,
                    Some(record_id),
                    &extracted_dir_name,
                    &extracted_db_path,
                    true,
                    None,
                    None,
                    Some(dir_meta),
                )
                .await?;
                ingest_directory(
                    pool,
                    bundle_id,
                    dir_id,
                    extracted_dir,
                    extracted_db_path.trim_start_matches('/').to_string(),
                    archive_budget.clone(),
                    indexing,
                    archive_depth + 1,
                )
                .await?;
            }
        }

        Ok(())
    })
}

#[allow(clippy::too_many_arguments)]
async fn insert_file_record(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    parent_id: Option<i64>,
    name: &str,
    path: &str,
    is_dir: bool,
    size_bytes: Option<i64>,
    mime_type: Option<&str>,
    meta: Option<serde_json::Value>,
) -> Result<i64, AppError> {
    let record_id = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO files (
            bundle_id, parent_id, name, path, is_dir, size_bytes, mime_type, status, meta
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        RETURNING id
        "#,
    )
    .bind(bundle_id)
    .bind(parent_id)
    .bind(name)
    .bind(path)
    .bind(is_dir)
    .bind(size_bytes)
    .bind(mime_type)
    .bind(Some("READY"))
    .bind(meta.map(|value| value.to_string()))
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;

    Ok(record_id)
}

async fn move_or_copy_file(source: &Path, destination: &Path) -> Result<(), AppError> {
    match fs::rename(source, destination).await {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(source, destination)
                .await
                .map_err(|error| io_error_at("copy uploaded file", destination, error))?;
            let _ = fs::remove_file(source).await;
            Ok(())
        }
    }
}

async fn ingest_text_file(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    file_id: i64,
    disk_path: &Path,
    _size_bytes: u64,
    indexing: &IndexingConfig,
) -> Result<(), AppError> {
    let file = fs::File::open(disk_path)
        .await
        .map_err(|error| io_error_at("open log file for indexing", disk_path, error))?;
    let mut reader = BufReader::new(file);
    let mut line_number = 0i64;
    let mut bytes_scanned = 0u64;
    let mut chunk_index = 0i64;
    let mut chunk = LogChunk::new(chunk_index, indexing.chunk_lines);
    let mut line = Vec::new();
    let mut offsets = Vec::new();
    let mut tx = pool.begin().await.map_err(AppError::Database)?;

    loop {
        let line_offset = bytes_scanned;
        let Some((read, truncated)) = read_line_bytes_limited(
            &mut reader,
            &mut line,
            usize::try_from(indexing.max_line_size).map_err(|_| {
                AppError::Config(
                    "RAIN_INDEXING_MAX_LINE_SIZE cannot be represented on this platform".into(),
                )
            })?,
        )
        .await
        .map_err(io_error)?
        else {
            break;
        };

        if line_number % indexing.line_offset_interval == 0 {
            offsets.push((line_number, line_offset as i64));
        }
        bytes_scanned = bytes_scanned.saturating_add(read as u64);

        let cleaned = clean_log_line(&line, truncated);
        if !cleaned.is_empty() {
            chunk.push(line_number, cleaned);

            if chunk.len() >= indexing.chunk_lines {
                flush_log_chunk(&mut tx, bundle_id, file_id, &chunk).await?;
                chunk_index += 1;
                chunk = LogChunk::new(chunk_index, indexing.chunk_lines);
            }
        }

        line_number += 1;
        if line_number % indexing.commit_lines == 0 {
            if !chunk.is_empty() {
                flush_log_chunk(&mut tx, bundle_id, file_id, &chunk).await?;
                chunk_index += 1;
                chunk = LogChunk::new(chunk_index, indexing.chunk_lines);
            }
            tx.commit().await.map_err(AppError::Database)?;
            tx = pool.begin().await.map_err(AppError::Database)?;
        }
    }

    if !chunk.is_empty() {
        flush_log_chunk(&mut tx, bundle_id, file_id, &chunk).await?;
    }

    sqlx::query("DELETE FROM log_line_offsets WHERE file_id = ?")
        .bind(file_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;

    for (line_number, byte_offset) in offsets {
        sqlx::query(
            r#"
            INSERT INTO log_line_offsets (file_id, line_number, byte_offset)
            VALUES (?, ?, ?)
            "#,
        )
        .bind(file_id)
        .bind(line_number)
        .bind(byte_offset)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;
    }

    sqlx::query("UPDATE files SET line_count = ? WHERE id = ?")
        .bind(line_number)
        .bind(file_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;

    tx.commit().await.map_err(AppError::Database)?;
    Ok(())
}

struct LogChunk {
    chunk_index: i64,
    line_start: Option<i64>,
    line_end: Option<i64>,
    lines: Vec<LogLine>,
}

struct LogLine {
    number: i64,
    content: String,
}

struct ParsedLogEvent {
    timestamp: Option<String>,
    level: Option<String>,
    component: Option<String>,
    message: String,
    parser_confidence: f64,
}

impl LogChunk {
    fn new(chunk_index: i64, capacity: usize) -> Self {
        Self {
            chunk_index,
            line_start: None,
            line_end: None,
            lines: Vec::with_capacity(capacity),
        }
    }

    fn push(&mut self, line_number: i64, content: String) {
        if self.line_start.is_none() {
            self.line_start = Some(line_number);
        }
        self.line_end = Some(line_number);
        self.lines.push(LogLine {
            number: line_number,
            content,
        });
    }

    fn len(&self) -> usize {
        self.lines.len()
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    fn content(&self) -> String {
        self.lines
            .iter()
            .map(|line| line.content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

async fn flush_log_chunk(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    bundle_id: &str,
    file_id: i64,
    chunk: &LogChunk,
) -> Result<(), AppError> {
    if chunk.is_empty() {
        return Ok(());
    }

    let content = chunk.content();
    let timeline = Some("all".to_string());
    let segment_id = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO log_segments (
            bundle_id, file_id, timeline, content, line_offset, line_end, chunk_index
        )
        VALUES (?, ?, ?, ?, ?, ?, ?)
        RETURNING id
        "#,
    )
    .bind(bundle_id)
    .bind(file_id)
    .bind(&timeline)
    .bind(&content)
    .bind(chunk.line_start)
    .bind(chunk.line_end)
    .bind(Some(chunk.chunk_index))
    .fetch_one(&mut **tx)
    .await
    .map_err(AppError::Database)?;

    sqlx::query(
        r#"
        INSERT INTO log_segments_fts (content, segment_id, bundle_id, file_id, timeline)
        VALUES (?, ?, ?, ?, ?)
        "#,
    )
    .bind(&content)
    .bind(segment_id)
    .bind(bundle_id)
    .bind(file_id)
    .bind(&timeline)
    .execute(&mut **tx)
    .await
    .map_err(AppError::Database)?;

    for line in &chunk.lines {
        if let Some(event) = parse_log_event(&line.content) {
            sqlx::query(
                r#"
                INSERT INTO log_events (
                    bundle_id, file_id, segment_id, line_number, timestamp, level,
                    component, message, raw, parser_name, parser_confidence
                )
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(bundle_id)
            .bind(file_id)
            .bind(segment_id)
            .bind(line.number)
            .bind(event.timestamp)
            .bind(event.level)
            .bind(event.component)
            .bind(event.message)
            .bind(&line.content)
            .bind("basic-log-line")
            .bind(event.parser_confidence)
            .execute(&mut **tx)
            .await
            .map_err(AppError::Database)?;
        }
    }

    Ok(())
}

fn clean_log_line(line: &[u8], truncated: bool) -> String {
    // SQLite text values should not contain embedded null bytes in this app.
    decode_log_line(line, truncated).trim().replace('\0', "")
}

pub async fn read_line_bytes_limited<R>(
    reader: &mut R,
    output: &mut Vec<u8>,
    max_bytes: usize,
) -> Result<Option<(usize, bool)>, io::Error>
where
    R: AsyncBufRead + Unpin,
{
    output.clear();
    let mut total_read = 0usize;
    let mut truncated = false;

    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if total_read == 0 {
                Ok(None)
            } else {
                Ok(Some((total_read, truncated)))
            };
        }

        let newline_pos = available.iter().position(|byte| *byte == b'\n');
        let consume_len = newline_pos.map_or(available.len(), |pos| pos + 1);
        let chunk = &available[..consume_len];
        total_read = total_read.saturating_add(chunk.len());

        let remaining = max_bytes.saturating_sub(output.len());
        if remaining > 0 {
            let keep_len = remaining.min(chunk.len());
            output.extend_from_slice(&chunk[..keep_len]);
            if keep_len < chunk.len() {
                truncated = true;
            }
        } else {
            truncated = true;
        }

        reader.consume(consume_len);

        if newline_pos.is_some() {
            return Ok(Some((total_read, truncated)));
        }
    }
}

pub fn decode_log_line(line: &[u8], truncated: bool) -> String {
    let mut decoded = String::from_utf8_lossy(line)
        .trim_end_matches(['\r', '\n'])
        .to_string();
    if truncated {
        decoded.push_str(TRUNCATED_LINE_MARKER);
    }
    decoded
}

fn parse_log_event(line: &str) -> Option<ParsedLogEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (timestamp, after_timestamp, timestamp_confidence) = split_timestamp(trimmed);
    let (level, after_level) = split_level(after_timestamp);
    if timestamp.is_none() && level.is_none() {
        return None;
    }

    let (component, message) = split_component(after_level.trim());
    let parser_confidence = timestamp_confidence
        + if level.is_some() { 0.35 } else { 0.0 }
        + if component.is_some() { 0.10 } else { 0.0 };

    Some(ParsedLogEvent {
        timestamp: timestamp.map(str::to_string),
        level: level.map(str::to_string),
        component: component.map(str::to_string),
        message: if message.is_empty() {
            trimmed.to_string()
        } else {
            message.to_string()
        },
        parser_confidence: parser_confidence.min(0.95),
    })
}

fn split_timestamp(line: &str) -> (Option<&str>, &str, f64) {
    if let Some((first, rest)) = line.split_once(' ')
        && looks_like_timestamp(first)
    {
        return (Some(first), rest, 0.45);
    }

    if let Some(candidate) = line.get(..19)
        && looks_like_timestamp(candidate)
    {
        return (
            Some(candidate),
            line.get(19..).unwrap_or("").trim_start(),
            0.45,
        );
    }

    (None, line, 0.0)
}

fn split_level(line: &str) -> (Option<&str>, &str) {
    let trimmed = line.trim_start_matches([' ', '[']);
    for level in LOG_LEVELS {
        if let Some(rest) = trimmed.strip_prefix(level) {
            let rest = rest.trim_start_matches([']', ':', '-', ' ']);
            return (Some(if level == "WARNING" { "WARN" } else { level }), rest);
        }
    }
    (None, line)
}

fn split_component(line: &str) -> (Option<&str>, &str) {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix('[')
        && let Some(end) = rest.find(']')
    {
        let component = rest[..end].trim();
        let message = rest[end + 1..].trim_start_matches([':', '-', ' ']).trim();
        if !component.is_empty() {
            return (Some(component), message);
        }
    }

    if let Some((component, message)) = trimmed.split_once(':') {
        let component = component.trim();
        if !component.is_empty()
            && component.len() <= 64
            && component
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/'))
        {
            return (Some(component), message.trim());
        }
    }

    (None, trimmed)
}

fn looks_like_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() >= 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        return true;
    }

    bytes.len() >= 8
        && bytes[2] == b':'
        && bytes[5] == b':'
        && bytes[..2].iter().all(u8::is_ascii_digit)
        && bytes[3..5].iter().all(u8::is_ascii_digit)
        && bytes[6..8].iter().all(u8::is_ascii_digit)
}

async fn extract_archive(
    name: &str,
    src: &Path,
    dest: &Path,
    archive_budget: ArchiveBudget,
) -> Result<(), AppError> {
    if is_zip_file(name) {
        extract_zip_archive(src, dest, archive_budget).await
    } else if is_tar_gz_file(name) {
        extract_tar_gz_archive(src, dest, archive_budget).await
    } else if is_gzip_file(name) {
        extract_gzip_file(name, src, dest, archive_budget).await
    } else {
        Err(AppError::BadRequest(format!(
            "unsupported archive type: {name}"
        )))
    }
}

async fn extract_zip_archive(
    src: &Path,
    dest: &Path,
    archive_budget: ArchiveBudget,
) -> Result<(), AppError> {
    let src_path = src.to_path_buf();
    let dest_path = dest.to_path_buf();
    task::spawn_blocking(move || -> Result<(), AppError> {
        let file = std::fs::File::open(&src_path)
            .map_err(|error| io_error_at("open zip archive", &src_path, error))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|err| AppError::BadRequest(err.to_string()))?;

        if archive.len() > archive_budget.config.max_entries {
            return Err(AppError::BadRequest(format!(
                "zip has too many entries; max {}",
                archive_budget.config.max_entries
            )));
        }

        let mut seen_paths = HashSet::new();
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|err| AppError::BadRequest(err.to_string()))?;
            let entry_path = sanitize_archive_path(Path::new(entry.name()));
            if entry_path.as_os_str().is_empty() {
                continue;
            }

            let depth = if entry.is_dir() {
                entry_path.components().count()
            } else {
                archive_parent_depth(&entry_path)
            };
            if depth > archive_budget.config.max_path_depth {
                return Err(AppError::BadRequest(format!(
                    "zip entry is too deep: {}",
                    entry.name()
                )));
            }

            let uncompressed_size = entry.size();
            if !entry.is_dir() && uncompressed_size > archive_budget.config.max_entry_size {
                return Err(AppError::BadRequest(format!(
                    "archive entry exceeds configured limit; max entry size {}: {}",
                    format_binary_size(archive_budget.config.max_entry_size),
                    entry.name(),
                )));
            }

            archive_budget.reserve_entry()?;
            archive_budget.reserve_bytes(uncompressed_size)?;

            validate_zip_ratio(
                entry.name(),
                uncompressed_size,
                entry.compressed_size(),
                archive_budget.config.max_compression_ratio,
            )?;

            let out_path = dest_path.join(entry_path);
            validate_extracted_path(
                &out_path,
                entry.name(),
                archive_budget.config.max_output_path_chars,
            )?;
            let normalized_out = normalize_extracted_path(&out_path);
            if !seen_paths.insert(normalized_out) {
                return Err(AppError::BadRequest(format!(
                    "zip contains duplicate normalized path: {}",
                    entry.name()
                )));
            }

            if entry.is_dir() {
                std::fs::create_dir_all(&out_path)
                    .map_err(|error| io_error_at("create extracted directory", &out_path, error))?;
            } else {
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|error| io_error_at("create extraction parent", parent, error))?;
                }
                let mut outfile = std::fs::File::create(&out_path)
                    .map_err(|error| io_error_at("create extracted file", &out_path, error))?;
                let copied = std::io::copy(&mut entry, &mut outfile)
                    .map_err(|error| io_error_at("write extracted file", &out_path, error))?;
                if copied != uncompressed_size {
                    return Err(AppError::BadRequest(format!(
                        "zip entry size mismatch: {}",
                        entry.name()
                    )));
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(|err| io_error(io::Error::other(err.to_string())))??;

    Ok(())
}

async fn extract_tar_gz_archive(
    src: &Path,
    dest: &Path,
    archive_budget: ArchiveBudget,
) -> Result<(), AppError> {
    let src_path = src.to_path_buf();
    let dest_path = dest.to_path_buf();
    task::spawn_blocking(move || -> Result<(), AppError> {
        let compressed_size = std::fs::metadata(&src_path).map_err(io_error)?.len().max(1);
        let file = StdFile::open(&src_path)
            .map_err(|error| io_error_at("open tar.gz archive", &src_path, error))?;
        let decoder = GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        let mut total_uncompressed = 0u64;
        let mut entries_count = 0usize;
        let mut seen_paths = HashSet::new();

        for entry_result in archive.entries().map_err(io_error)? {
            entries_count += 1;
            if entries_count > archive_budget.config.max_entries {
                return Err(AppError::BadRequest(format!(
                    "tar.gz has too many entries; max {}",
                    archive_budget.config.max_entries
                )));
            }

            let mut entry = entry_result.map_err(io_error)?;
            let raw_path = entry.path().map_err(io_error)?.into_owned();
            let entry_path = sanitize_archive_path(&raw_path);
            if entry_path.as_os_str().is_empty() {
                continue;
            }

            let depth = if entry.header().entry_type().is_dir() {
                entry_path.components().count()
            } else {
                archive_parent_depth(&entry_path)
            };
            if depth > archive_budget.config.max_path_depth {
                return Err(AppError::BadRequest(format!(
                    "tar.gz entry is too deep: {}",
                    raw_path.display()
                )));
            }

            let entry_size = entry.header().size().map_err(io_error)?;
            if entry_size > archive_budget.config.max_entry_size {
                return Err(AppError::BadRequest(format!(
                    "archive entry exceeds configured limit; max entry size {}: {}",
                    format_binary_size(archive_budget.config.max_entry_size),
                    raw_path.display(),
                )));
            }

            total_uncompressed = total_uncompressed
                .checked_add(entry_size)
                .ok_or_else(|| AppError::BadRequest("tar.gz extracted size overflow".into()))?;
            archive_budget.reserve_entry()?;
            archive_budget.reserve_bytes(entry_size)?;
            validate_archive_ratio(
                &raw_path.display().to_string(),
                total_uncompressed,
                compressed_size,
                archive_budget.config.max_compression_ratio,
            )?;

            let out_path = dest_path.join(entry_path);
            validate_extracted_path(
                &out_path,
                &raw_path.display().to_string(),
                archive_budget.config.max_output_path_chars,
            )?;
            let normalized_out = normalize_extracted_path(&out_path);
            if !seen_paths.insert(normalized_out) {
                return Err(AppError::BadRequest(format!(
                    "tar.gz contains duplicate normalized path: {}",
                    raw_path.display()
                )));
            }
            if entry.header().entry_type().is_dir() {
                std::fs::create_dir_all(&out_path)
                    .map_err(|error| io_error_at("create extracted directory", &out_path, error))?;
            } else if entry.header().entry_type().is_file() {
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|error| io_error_at("create extraction parent", parent, error))?;
                }
                let mut outfile = StdFile::create(&out_path)
                    .map_err(|error| io_error_at("create extracted file", &out_path, error))?;
                let copied = std::io::copy(&mut entry, &mut outfile)
                    .map_err(|error| io_error_at("write extracted file", &out_path, error))?;
                if copied != entry_size {
                    return Err(AppError::BadRequest(format!(
                        "tar.gz entry size mismatch: {}",
                        raw_path.display()
                    )));
                }
            }
        }

        Ok(())
    })
    .await
    .map_err(|err| io_error(io::Error::other(err.to_string())))??;

    Ok(())
}

async fn extract_gzip_file(
    name: &str,
    src: &Path,
    dest: &Path,
    archive_budget: ArchiveBudget,
) -> Result<(), AppError> {
    let src_path = src.to_path_buf();
    let dest_path = dest.to_path_buf();
    let source_name = name.to_string();
    let output_name = gzip_output_name(name);
    task::spawn_blocking(move || -> Result<(), AppError> {
        archive_budget.reserve_entry()?;
        let remaining = archive_budget.remaining_bytes()?;
        let entry_limit = archive_budget.config.max_entry_size;
        let copy_limit = entry_limit.min(remaining);
        let compressed_size = std::fs::metadata(&src_path).map_err(io_error)?.len().max(1);
        let file = StdFile::open(&src_path)
            .map_err(|error| io_error_at("open gzip archive", &src_path, error))?;
        let mut decoder = GzDecoder::new(file);
        std::fs::create_dir_all(&dest_path)
            .map_err(|error| io_error_at("create gzip extraction directory", &dest_path, error))?;
        let out_path = dest_path.join(output_name);
        validate_extracted_path(
            &out_path,
            &source_name,
            archive_budget.config.max_output_path_chars,
        )?;
        if out_path.exists() {
            return Err(AppError::BadRequest(format!(
                "gzip output path already exists: {}",
                out_path.display()
            )));
        }
        let mut outfile = StdFile::create(&out_path)
            .map_err(|error| io_error_at("create gzip output", &out_path, error))?;
        let copied = copy_with_limit(&mut decoder, &mut outfile, copy_limit).map_err(|error| {
            if matches!(error, AppError::BadRequest(_)) {
                if remaining <= entry_limit {
                    AppError::BadRequest(format!(
                        "archive bundle exceeds configured extracted size; max bundle size {}",
                        format_binary_size(archive_budget.config.max_extracted_size)
                    ))
                } else {
                    AppError::BadRequest(format!(
                        "archive entry exceeds configured limit; max entry size {}",
                        format_binary_size(entry_limit)
                    ))
                }
            } else {
                error
            }
        })?;
        archive_budget.reserve_bytes(copied)?;
        validate_archive_ratio(
            "gzip file",
            copied,
            compressed_size,
            archive_budget.config.max_compression_ratio,
        )?;
        Ok(())
    })
    .await
    .map_err(|err| io_error(io::Error::other(err.to_string())))??;

    Ok(())
}

fn copy_with_limit<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    limit: u64,
) -> Result<u64, AppError> {
    let mut buffer = [0u8; 16 * 1024];
    let mut total = 0u64;
    loop {
        let read = reader.read(&mut buffer).map_err(io_error)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| AppError::BadRequest("gzip extracted size overflow".into()))?;
        if total > limit {
            return Err(AppError::BadRequest(format!(
                "gzip exceeds limit of {limit} bytes"
            )));
        }
        writer.write_all(&buffer[..read]).map_err(io_error)?;
    }
    Ok(total)
}

fn normalize_extracted_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
        .collect::<Vec<_>>()
        .join("/")
}

fn validate_extracted_path(
    path: &Path,
    source_path: &str,
    max_output_path_chars: usize,
) -> Result<(), AppError> {
    let path_chars = path.to_string_lossy().encode_utf16().count();
    if path_chars > max_output_path_chars {
        return Err(AppError::BadRequest(format!(
            "archive output path is too long ({path_chars} > {max_output_path_chars}): {source_path}"
        )));
    }
    Ok(())
}

fn archive_parent_depth(path: &Path) -> usize {
    path.parent()
        .map(|parent| parent.components().count())
        .unwrap_or(0)
}

fn validate_zip_ratio(
    name: &str,
    uncompressed_size: u64,
    compressed_size: u64,
    max_compression_ratio: u64,
) -> Result<(), AppError> {
    validate_archive_ratio(
        name,
        uncompressed_size,
        compressed_size,
        max_compression_ratio,
    )
}

fn validate_archive_ratio(
    name: &str,
    uncompressed_size: u64,
    compressed_size: u64,
    max_compression_ratio: u64,
) -> Result<(), AppError> {
    if uncompressed_size == 0 {
        return Ok(());
    }

    if compressed_size == 0 {
        return Err(AppError::BadRequest(format!(
            "zip entry has invalid compressed size: {name}"
        )));
    }

    if uncompressed_size / compressed_size > max_compression_ratio {
        return Err(AppError::BadRequest(format!(
            "archive compression ratio is too high: {name}"
        )));
    }

    Ok(())
}

fn format_binary_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{} GiB", bytes / 1024 / 1024 / 1024)
    } else if bytes >= 1024 * 1024 {
        format!("{} MiB", bytes / 1024 / 1024)
    } else if bytes >= 1024 {
        format!("{} KiB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

fn sanitize_archive_path(path: &Path) -> PathBuf {
    let mut sanitized = PathBuf::new();
    for component in path.components() {
        if let std::path::Component::Normal(os_str) = component
            && let Some(segment) = os_str.to_str()
        {
            let mut safe = segment
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || "-_.".contains(ch) {
                        ch
                    } else {
                        '_'
                    }
                })
                .collect::<String>();
            while safe.ends_with('.') {
                safe.pop();
            }
            if safe.is_empty() {
                safe.push('_');
            }
            if is_windows_reserved_name(&safe) {
                safe.insert(0, '_');
            }
            sanitized.push(safe);
        }
    }
    sanitized
}

fn is_windows_reserved_name(segment: &str) -> bool {
    let stem = segment
        .split('.')
        .next()
        .unwrap_or(segment)
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem.strip_prefix("COM").is_some_and(|suffix| {
            matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
        })
        || stem.strip_prefix("LPT").is_some_and(|suffix| {
            matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
        })
}

fn is_zip_file(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
}

fn is_tar_gz_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".tar.gz") || lower.ends_with(".tgz")
}

fn is_gzip_file(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".gz") && !is_tar_gz_file(name)
}

fn gzip_output_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let stripped = if lower.ends_with(".gz") {
        name.get(..name.len().saturating_sub(3)).unwrap_or(name)
    } else {
        name
    };
    let sanitized = stripped
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let mut output = if sanitized.is_empty() {
        "decompressed".to_string()
    } else {
        sanitized
    };
    while output.ends_with('.') {
        output.pop();
    }
    if is_windows_reserved_name(&output) {
        output.insert(0, '_');
    }
    output
}

fn io_error(err: std::io::Error) -> AppError {
    AppError::Io(err)
}

fn io_error_at(operation: &str, path: &Path, error: std::io::Error) -> AppError {
    AppError::Io(std::io::Error::new(
        error.kind(),
        format!("{operation} {}: {error}", path.display()),
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        path::{Path, PathBuf},
    };

    use crate::config::ArchiveConfig;
    use flate2::{Compression, write::GzEncoder};

    use super::{
        ArchiveBudget, archive_parent_depth, extract_gzip_file, gzip_output_name,
        sanitize_archive_path, split_timestamp, validate_extracted_path,
    };

    #[test]
    fn archive_depth_counts_only_parent_directories() {
        let path =
            Path::new("org/jetbrains/kotlin/gradle/dsl/HasConfigurableKotlinCompilerOptions.kt");

        assert_eq!(archive_parent_depth(path), 5);
    }

    #[test]
    fn archive_depth_supports_sixteen_parent_directories() {
        let path = Path::new("01/02/03/04/05/06/07/08/09/10/11/12/13/14/15/16/file.log");

        assert_eq!(archive_parent_depth(path), 16);
    }

    #[test]
    fn split_timestamp_does_not_slice_inside_utf8_character() {
        let line = "123456789012345678中 ERROR tail";
        let (timestamp, rest, confidence) = split_timestamp(line);

        assert_eq!(timestamp, None);
        assert_eq!(rest, line);
        assert_eq!(confidence, 0.0);
    }

    #[test]
    fn gzip_output_name_handles_mixed_case_suffix_with_utf8_name() {
        assert_eq!(gzip_output_name("构建日志.Gz"), "____");
    }

    #[test]
    fn gzip_output_name_prefixes_windows_reserved_device_names() {
        assert_eq!(gzip_output_name("NUL.GZ"), "_NUL");
        assert_eq!(gzip_output_name("com1.txt.gz"), "_com1.txt");
    }

    #[test]
    fn archive_paths_prefix_windows_reserved_device_names() {
        assert_eq!(
            sanitize_archive_path(Path::new("NUL.txt")),
            Path::new("_NUL.txt")
        );
        assert_eq!(
            sanitize_archive_path(Path::new("logs/con/output.log")),
            Path::new("logs/_con/output.log")
        );
        assert_eq!(sanitize_archive_path(Path::new("COM1")), Path::new("_COM1"));
        assert_eq!(
            sanitize_archive_path(Path::new("Lpt9.log")),
            Path::new("_Lpt9.log")
        );
    }

    #[test]
    fn archive_output_path_rejects_windows_unsafe_length_with_source_path() {
        let max = ArchiveConfig::default().max_output_path_chars;
        let path = PathBuf::from("x".repeat(max + 1));
        let error = validate_extracted_path(
            &path,
            "logs/too-long.log",
            ArchiveConfig::default().max_output_path_chars,
        )
        .expect_err("overlong archive output path should fail");

        assert!(error.to_string().contains("logs/too-long.log"));
    }

    fn gzip_fixture(name: &str, content: &[u8]) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("rain-ingest-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("sample.log.gz");
        let destination = root.join("out");
        let file = std::fs::File::create(&source).unwrap();
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder.write_all(content).unwrap();
        encoder.finish().unwrap();
        (source, destination)
    }

    #[tokio::test]
    async fn gzip_reports_configured_entry_limit() {
        let (source, destination) = gzip_fixture("entry-limit", b"hello");
        let config = ArchiveConfig {
            max_entry_size: 4,
            max_extracted_size: 8,
            ..ArchiveConfig::default()
        };

        let error = extract_gzip_file(
            "sample.log.gz",
            &source,
            &destination,
            ArchiveBudget::new(config),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("max entry size 4 B"));
        let _ = std::fs::remove_dir_all(source.parent().unwrap());
    }

    #[tokio::test]
    async fn gzip_reports_exhausted_bundle_budget() {
        let (source, destination) = gzip_fixture("bundle-limit", b"hello");
        let config = ArchiveConfig {
            max_entry_size: 6,
            max_extracted_size: 8,
            ..ArchiveConfig::default()
        };
        let budget = ArchiveBudget::new(config);
        budget.reserve_bytes(4).unwrap();

        let error = extract_gzip_file("sample.log.gz", &source, &destination, budget)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("max bundle size 8 B"));
        let _ = std::fs::remove_dir_all(source.parent().unwrap());
    }
}
