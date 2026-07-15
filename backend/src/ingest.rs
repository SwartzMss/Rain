use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::fs;
use tokio::io::BufReader;

use crate::{
    config::IndexingConfig,
    error::AppError,
    file_classification::{PreviewKind, classify_file, effective_mime_type},
};

mod archive;
mod indexing;

pub use archive::ArchiveBudget;
#[cfg(test)]
use archive::{archive_parent_depth, extract_gzip_file, gzip_output_name, sanitize_archive_path};
use archive::{extract_archive, validate_extracted_path};
#[cfg(test)]
use indexing::event_parser::split_timestamp;
pub use indexing::line_reader::{decode_log_line, read_line_bytes_limited};
use indexing::{clean_log_line, parse_log_event};

#[derive(Clone)]
pub struct EventBudget {
    inner: Arc<EventBudgetInner>,
}

struct EventBudgetInner {
    used: AtomicUsize,
    cap_reported: AtomicBool,
    limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventReservation {
    Reserved,
    CapReached,
    Exhausted,
}

impl EventBudget {
    pub fn new(limit: usize) -> Self {
        Self {
            inner: Arc::new(EventBudgetInner {
                used: AtomicUsize::new(0),
                cap_reported: AtomicBool::new(false),
                limit,
            }),
        }
    }

    fn reserve(&self) -> EventReservation {
        if self
            .inner
            .used
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                (used < self.inner.limit).then_some(used + 1)
            })
            .is_ok()
        {
            EventReservation::Reserved
        } else if !self.inner.cap_reported.swap(true, Ordering::Relaxed) {
            EventReservation::CapReached
        } else {
            EventReservation::Exhausted
        }
    }

    fn limit(&self) -> usize {
        self.inner.limit
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
    pub event_budget: EventBudget,
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
        event_budget,
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
        ingest_text_file(
            pool,
            bundle_id,
            file_id,
            &disk_path,
            size_bytes,
            indexing,
            &event_budget,
        )
        .await?;
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
            event_budget,
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
    event_budget: EventBudget,
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
                    event_budget.clone(),
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
                    &event_budget,
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
                    event_budget.clone(),
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
    event_budget: &EventBudget,
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
                flush_log_chunk(&mut tx, bundle_id, file_id, &chunk, event_budget).await?;
                chunk_index += 1;
                chunk = LogChunk::new(chunk_index, indexing.chunk_lines);
            }
        }

        line_number += 1;
        if line_number % indexing.commit_lines == 0 {
            if !chunk.is_empty() {
                flush_log_chunk(&mut tx, bundle_id, file_id, &chunk, event_budget).await?;
                chunk_index += 1;
                chunk = LogChunk::new(chunk_index, indexing.chunk_lines);
            }
            tx.commit().await.map_err(AppError::Database)?;
            tx = pool.begin().await.map_err(AppError::Database)?;
        }
    }

    if !chunk.is_empty() {
        flush_log_chunk(&mut tx, bundle_id, file_id, &chunk, event_budget).await?;
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
    event_budget: &EventBudget,
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
            match event_budget.reserve() {
                EventReservation::Reserved => {}
                EventReservation::CapReached => {
                    tracing::warn!(
                        bundle_id,
                        max_events_per_bundle = event_budget.limit(),
                        "structured event indexing limit reached; full-text indexing continues"
                    );
                    continue;
                }
                EventReservation::Exhausted => continue,
            }
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
    fn event_budget_is_shared_across_clones_and_reports_cap_once() {
        let budget = super::EventBudget::new(2);
        let clone = budget.clone();

        assert_eq!(budget.reserve(), super::EventReservation::Reserved);
        assert_eq!(clone.reserve(), super::EventReservation::Reserved);
        assert_eq!(budget.reserve(), super::EventReservation::CapReached);
        assert_eq!(clone.reserve(), super::EventReservation::Exhausted);
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
