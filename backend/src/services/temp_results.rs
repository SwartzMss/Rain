use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncSeekExt, AsyncWriteExt, BufReader, SeekFrom},
};

use crate::{
    config::MAX_TEMP_RESULT_LOGICAL_LINE_BYTES,
    error::AppError,
    ingest::{LimitedLine, decode_log_line, read_line_bytes_limited_with_budget_and_callback},
    log_expression::Expression,
};

pub struct TempSource {
    pub path: PathBuf,
    pub metadata_path: Option<PathBuf>,
    pub label: String,
    pub bundle_hash: Option<String>,
    pub file_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineRange {
    pub start: i64,
    pub end: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CandidateScanPlan {
    pub ranges: Vec<LineRange>,
    pub seek_line: i64,
    pub seek_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceSearchPlan {
    Raw { reason: &'static str },
    Tantivy(CandidateScanPlan),
}

pub struct MaterializedPreview {
    pub total: i64,
    pub lines: Vec<PreviewLine>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct MatchMetadata {
    pub bundle_hash: Option<String>,
    pub file_id: Option<String>,
    pub path: String,
    pub line_number: i64,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SparseCheckpoint {
    pub result_line: i64,
    pub log_offset: u64,
    pub meta_offset: u64,
}

#[derive(Serialize)]
pub struct PreviewLine {
    pub bundle_hash: Option<String>,
    pub file_id: Option<String>,
    pub path: String,
    pub line_number: i64,
    pub content: String,
}

pub struct TempResultExecutor;

impl TempResultExecutor {
    #[allow(clippy::too_many_arguments)]
    pub async fn materialize_preview(
        sources: &[TempSource],
        expression: &Expression,
        from: i64,
        size: i64,
        max_output_bytes: u64,
        output: &mut File,
        metadata_output: &mut File,
        index_output: &mut File,
    ) -> Result<MaterializedPreview, AppError> {
        let page_end = from
            .checked_add(size)
            .ok_or_else(|| AppError::BadRequest("分页参数超出支持范围".into()))?;
        let mut matched = 0_i64;
        let mut lines = Vec::new();
        let mut log_offset = 0_u64;
        let mut meta_offset = 0_u64;
        let mut total_output_bytes = 0_u64;
        let max_logical_line_bytes =
            usize::try_from(MAX_TEMP_RESULT_LOGICAL_LINE_BYTES).map_err(|_| {
                AppError::Config(
                    "MAX_TEMP_RESULT_LOGICAL_LINE_BYTES cannot be represented on this platform"
                        .into(),
                )
            })?;
        let mut matcher = expression.chunk_matcher();
        for source in sources {
            let file = File::open(&source.path).await.map_err(AppError::Io)?;
            let mut reader = BufReader::new(file);
            let mut source_metadata_reader = match source.metadata_path.as_ref() {
                Some(path) => Some(BufReader::new(
                    File::open(path).await.map_err(AppError::Io)?,
                )),
                None => None,
            };
            let mut bytes = Vec::new();
            let mut source_metadata_line = String::new();
            let mut source_line = 0_i64;
            loop {
                bytes.clear();
                matcher.reset();
                let truncated = match read_line_bytes_limited_with_budget_and_callback(
                    &mut reader,
                    &mut bytes,
                    max_logical_line_bytes,
                    // No cumulative scan cap; the caller enforces a deadline.
                    // Keep the retained line prefix bounded independently.
                    usize::MAX,
                    |chunk| matcher.feed_bytes(chunk),
                )
                .await
                .map_err(AppError::Io)?
                {
                    LimitedLine::EndOfFile => break,
                    LimitedLine::Line { truncated, .. } => truncated,
                    LimitedLine::ScanLimit { .. } => {
                        return Err(AppError::Io(std::io::Error::other(
                            "logical line exceeds platform size",
                        )));
                    }
                };
                matcher.finish();
                let inherited_metadata = if let Some(reader) = source_metadata_reader.as_mut() {
                    source_metadata_line.clear();
                    if reader
                        .read_line(&mut source_metadata_line)
                        .await
                        .map_err(AppError::Io)?
                        == 0
                    {
                        return Err(invalid_sidecar(
                            "temporary result metadata ended before its content",
                        ));
                    }
                    Some(decode_json_line::<MatchMetadata>(
                        source_metadata_line.trim_end(),
                    )?)
                } else {
                    None
                };
                if matcher.matches(expression) {
                    let content = decode_log_line(&bytes, truncated);
                    let mut metadata = inherited_metadata.unwrap_or_else(|| MatchMetadata {
                        bundle_hash: source.bundle_hash.clone(),
                        file_id: source.file_id.clone(),
                        path: source.label.clone(),
                        line_number: source_line,
                        truncated,
                    });
                    metadata.truncated |= truncated;
                    if matched % 1_000 == 0 {
                        let checkpoint = SparseCheckpoint {
                            result_line: matched,
                            log_offset,
                            meta_offset,
                        };
                        write_json_line(
                            index_output,
                            &checkpoint,
                            max_output_bytes,
                            &mut total_output_bytes,
                        )
                        .await?;
                    }
                    let line_bytes = content.len() as u64 + 1;
                    let next_output_size =
                        log_offset.checked_add(line_bytes).ok_or_else(too_large)?;
                    ensure_output_capacity(total_output_bytes, line_bytes, max_output_bytes)?;
                    output
                        .write_all(content.as_bytes())
                        .await
                        .map_err(AppError::Io)?;
                    log_offset = next_output_size;
                    total_output_bytes = total_output_bytes
                        .checked_add(line_bytes)
                        .ok_or_else(too_large)?;
                    output.write_all(b"\n").await.map_err(AppError::Io)?;
                    meta_offset += write_json_line(
                        metadata_output,
                        &metadata,
                        max_output_bytes,
                        &mut total_output_bytes,
                    )
                    .await?;
                    if matched >= from && matched < page_end {
                        lines.push(PreviewLine {
                            bundle_hash: metadata.bundle_hash.clone(),
                            file_id: metadata.file_id.clone(),
                            path: metadata.path.clone(),
                            line_number: metadata.line_number,
                            content,
                        });
                    }
                    matched += 1;
                }
                source_line += 1;
            }
            if let Some(reader) = source_metadata_reader.as_mut() {
                source_metadata_line.clear();
                if reader
                    .read_line(&mut source_metadata_line)
                    .await
                    .map_err(AppError::Io)?
                    != 0
                {
                    return Err(invalid_sidecar(
                        "temporary result metadata contains more records than its content",
                    ));
                }
            }
        }
        output.flush().await.map_err(AppError::Io)?;
        metadata_output.flush().await.map_err(AppError::Io)?;
        index_output.flush().await.map_err(AppError::Io)?;
        Ok(MaterializedPreview {
            total: matched,
            lines,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn materialize_preview_with_plans(
        sources: &[TempSource],
        plans: &[SourceSearchPlan],
        expression: &Expression,
        from: i64,
        size: i64,
        max_output_bytes: u64,
        output: &mut File,
        metadata_output: &mut File,
        index_output: &mut File,
    ) -> Result<MaterializedPreview, AppError> {
        if sources.len() != plans.len() {
            return Err(AppError::Config(
                "temporary result source and search plan counts differ".into(),
            ));
        }
        let page_end = from
            .checked_add(size)
            .ok_or_else(|| AppError::BadRequest("分页参数超出支持范围".into()))?;
        let mut matched = 0_i64;
        let mut lines = Vec::new();
        let mut log_offset = 0_u64;
        let mut meta_offset = 0_u64;
        let mut total_output_bytes = 0_u64;
        let max_logical_line_bytes =
            usize::try_from(MAX_TEMP_RESULT_LOGICAL_LINE_BYTES).map_err(|_| {
                AppError::Config(
                    "MAX_TEMP_RESULT_LOGICAL_LINE_BYTES cannot be represented on this platform"
                        .into(),
                )
            })?;
        let mut matcher = expression.chunk_matcher();
        for (source, plan) in sources.iter().zip(plans) {
            let (candidate_ranges, seek_line, seek_offset) = match plan {
                SourceSearchPlan::Raw { .. } => (None, 0_i64, 0_u64),
                SourceSearchPlan::Tantivy(candidate) if candidate.ranges.is_empty() => {
                    continue;
                }
                SourceSearchPlan::Tantivy(candidate) => (
                    Some(candidate.ranges.as_slice()),
                    candidate.seek_line.min(candidate.ranges[0].start),
                    candidate.seek_offset,
                ),
            };
            let mut file = File::open(&source.path).await.map_err(AppError::Io)?;
            if candidate_ranges.is_some() {
                file.seek(SeekFrom::Start(seek_offset))
                    .await
                    .map_err(AppError::Io)?;
            }
            let mut reader = BufReader::new(file);
            let mut source_metadata_reader = match source.metadata_path.as_ref() {
                Some(path) => Some(BufReader::new(
                    File::open(path).await.map_err(AppError::Io)?,
                )),
                None => None,
            };
            let mut bytes = Vec::new();
            let mut source_metadata_line = String::new();
            let mut source_line = seek_line;
            let mut range_index = 0_usize;
            loop {
                if let Some(ranges) = candidate_ranges {
                    while ranges
                        .get(range_index)
                        .is_some_and(|range| source_line > range.end)
                    {
                        range_index += 1;
                    }
                    if range_index == ranges.len() {
                        break;
                    }
                }
                let selected = candidate_ranges.is_none_or(|ranges| {
                    ranges
                        .get(range_index)
                        .is_some_and(|range| source_line >= range.start)
                });
                bytes.clear();
                let line = if selected {
                    matcher.reset();
                    read_line_bytes_limited_with_budget_and_callback(
                        &mut reader,
                        &mut bytes,
                        max_logical_line_bytes,
                        // No cumulative scan cap; the caller enforces a deadline.
                        // Keep the retained line prefix bounded independently.
                        usize::MAX,
                        |chunk| matcher.feed_bytes(chunk),
                    )
                    .await
                    .map_err(AppError::Io)?
                } else {
                    read_line_bytes_limited_with_budget_and_callback(
                        &mut reader,
                        &mut bytes,
                        max_logical_line_bytes,
                        // No cumulative scan cap; the caller enforces a deadline.
                        // Keep the retained line prefix bounded independently.
                        usize::MAX,
                        |_| {},
                    )
                    .await
                    .map_err(AppError::Io)?
                };
                let truncated = match line {
                    LimitedLine::EndOfFile => break,
                    LimitedLine::Line { truncated, .. } => truncated,
                    LimitedLine::ScanLimit { .. } => {
                        return Err(AppError::Io(std::io::Error::other(
                            "logical line exceeds platform size",
                        )));
                    }
                };
                let inherited_metadata = if let Some(reader) = source_metadata_reader.as_mut() {
                    source_metadata_line.clear();
                    if reader
                        .read_line(&mut source_metadata_line)
                        .await
                        .map_err(AppError::Io)?
                        == 0
                    {
                        return Err(invalid_sidecar(
                            "temporary result metadata ended before its content",
                        ));
                    }
                    if selected {
                        Some(decode_json_line::<MatchMetadata>(
                            source_metadata_line.trim_end(),
                        )?)
                    } else {
                        None
                    }
                } else {
                    None
                };
                if selected {
                    matcher.finish();
                }
                if selected && matcher.matches(expression) {
                    let content = decode_log_line(&bytes, truncated);
                    let mut metadata = inherited_metadata.unwrap_or_else(|| MatchMetadata {
                        bundle_hash: source.bundle_hash.clone(),
                        file_id: source.file_id.clone(),
                        path: source.label.clone(),
                        line_number: source_line,
                        truncated,
                    });
                    metadata.truncated |= truncated;
                    if matched % 1_000 == 0 {
                        let checkpoint = SparseCheckpoint {
                            result_line: matched,
                            log_offset,
                            meta_offset,
                        };
                        write_json_line(
                            index_output,
                            &checkpoint,
                            max_output_bytes,
                            &mut total_output_bytes,
                        )
                        .await?;
                    }
                    let line_bytes = content.len() as u64 + 1;
                    let next_output_size =
                        log_offset.checked_add(line_bytes).ok_or_else(too_large)?;
                    ensure_output_capacity(total_output_bytes, line_bytes, max_output_bytes)?;
                    output
                        .write_all(content.as_bytes())
                        .await
                        .map_err(AppError::Io)?;
                    log_offset = next_output_size;
                    total_output_bytes = total_output_bytes
                        .checked_add(line_bytes)
                        .ok_or_else(too_large)?;
                    output.write_all(b"\n").await.map_err(AppError::Io)?;
                    meta_offset += write_json_line(
                        metadata_output,
                        &metadata,
                        max_output_bytes,
                        &mut total_output_bytes,
                    )
                    .await?;
                    if matched >= from && matched < page_end {
                        lines.push(PreviewLine {
                            bundle_hash: metadata.bundle_hash.clone(),
                            file_id: metadata.file_id.clone(),
                            path: metadata.path.clone(),
                            line_number: metadata.line_number,
                            content,
                        });
                    }
                    matched += 1;
                }
                source_line += 1;
            }
            if let Some(reader) = source_metadata_reader.as_mut() {
                source_metadata_line.clear();
                if reader
                    .read_line(&mut source_metadata_line)
                    .await
                    .map_err(AppError::Io)?
                    != 0
                {
                    return Err(invalid_sidecar(
                        "temporary result metadata contains more records than its content",
                    ));
                }
            }
        }
        output.flush().await.map_err(AppError::Io)?;
        metadata_output.flush().await.map_err(AppError::Io)?;
        index_output.flush().await.map_err(AppError::Io)?;
        Ok(MaterializedPreview {
            total: matched,
            lines,
        })
    }

    pub async fn write_matches(
        sources: &[TempSource],
        expression: &Expression,
        output: &mut File,
        metadata_output: &mut File,
        index_output: &mut File,
        max_output_bytes: u64,
    ) -> Result<i64, AppError> {
        // Full materialization uses the same scan, metadata, index, newline, and
        // size-limit pipeline as preview; a zero-sized window suppresses only
        // collecting preview lines.
        Ok(Self::materialize_preview(
            sources,
            expression,
            0,
            0,
            max_output_bytes,
            output,
            metadata_output,
            index_output,
        )
        .await?
        .total)
    }
}

pub(crate) fn merge_line_ranges(mut ranges: Vec<LineRange>) -> Vec<LineRange> {
    ranges.retain(|range| range.start >= 0 && range.start <= range.end);
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<LineRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(current) = merged.last_mut()
            && range.start <= current.end.saturating_add(1)
        {
            current.end = current.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

fn too_large() -> AppError {
    AppError::public(
        actix_web::http::StatusCode::PAYLOAD_TOO_LARGE,
        "TEMP_RESULT_TOO_LARGE",
        "临时结果超过大小限制",
    )
}

pub fn scan_timeout() -> AppError {
    AppError::public(
        actix_web::http::StatusCode::REQUEST_TIMEOUT,
        "TEMP_RESULT_SCAN_TIMEOUT",
        "临时结果扫描超时",
    )
}

fn ensure_output_capacity(current: u64, additional: u64, limit: u64) -> Result<(), AppError> {
    if current
        .checked_add(additional)
        .is_none_or(|size| size > limit)
    {
        return Err(too_large());
    }
    Ok(())
}

async fn write_json_line<T: Serialize>(
    output: &mut File,
    value: &T,
    max_output_bytes: u64,
    total_output_bytes: &mut u64,
) -> Result<u64, AppError> {
    let mut bytes = serde_json::to_vec(value).map_err(|error| {
        AppError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })?;
    bytes.push(b'\n');
    ensure_output_capacity(*total_output_bytes, bytes.len() as u64, max_output_bytes)?;
    output.write_all(&bytes).await.map_err(AppError::Io)?;
    *total_output_bytes = total_output_bytes
        .checked_add(bytes.len() as u64)
        .ok_or_else(too_large)?;
    Ok(bytes.len() as u64)
}

fn decode_json_line<T: serde::de::DeserializeOwned>(line: &str) -> Result<T, AppError> {
    serde_json::from_str(line)
        .map_err(|error| AppError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error)))
}

fn invalid_sidecar(message: &str) -> AppError {
    AppError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    ))
}

pub fn select_checkpoint(
    checkpoints: &[SparseCheckpoint],
    start: i64,
) -> Option<&SparseCheckpoint> {
    checkpoints
        .iter()
        .rev()
        .find(|checkpoint| checkpoint.result_line <= start)
}

#[cfg(test)]
mod tests {
    const TEST_MAX_OUTPUT_BYTES: u64 = usize::MAX as u64;

    use std::path::PathBuf;

    use tokio::fs::File;
    use uuid::Uuid;

    use super::{
        CandidateScanPlan, LineRange, SourceSearchPlan, SparseCheckpoint, TempResultExecutor,
        TempSource, merge_line_ranges, select_checkpoint,
    };
    use crate::{error::AppError, ingest::TRUNCATED_LINE_MARKER, log_expression};

    fn test_path(suffix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rain-temp-result-{}-{suffix}", Uuid::new_v4()))
    }

    #[tokio::test]
    async fn materializes_only_exact_matches_inside_candidate_ranges() {
        let source_path = test_path("candidate-source.log");
        let log_path = test_path("candidate-result.log");
        let meta_path = test_path("candidate-result.meta");
        let index_path = test_path("candidate-result.idx");
        let source_content = (0..=20)
            .map(|line| {
                if line == 3 {
                    "ERROR candidate three".to_string()
                } else if line == 17 {
                    "ERROR candidate seventeen".to_string()
                } else {
                    format!("INFO line {line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        tokio::fs::write(&source_path, source_content)
            .await
            .unwrap();
        let sources = vec![TempSource {
            path: source_path.clone(),
            metadata_path: None,
            label: "app.log".into(),
            bundle_hash: Some("bundle-1".into()),
            file_id: Some("42".into()),
        }];
        let plans = vec![SourceSearchPlan::Tantivy(CandidateScanPlan {
            ranges: vec![
                LineRange { start: 0, end: 5 },
                LineRange { start: 15, end: 20 },
            ],
            seek_line: 0,
            seek_offset: 0,
        })];
        let expression = log_expression::parse("ERROR").unwrap();
        let mut log = File::create(&log_path).await.unwrap();
        let mut meta = File::create(&meta_path).await.unwrap();
        let mut index = File::create(&index_path).await.unwrap();

        let preview = TempResultExecutor::materialize_preview_with_plans(
            &sources,
            &plans,
            &expression,
            0,
            10,
            TEST_MAX_OUTPUT_BYTES,
            &mut log,
            &mut meta,
            &mut index,
        )
        .await
        .unwrap();

        assert_eq!(preview.total, 2);
        assert_eq!(
            preview
                .lines
                .iter()
                .map(|line| line.line_number)
                .collect::<Vec<_>>(),
            vec![3, 17]
        );
        assert_eq!(preview.lines[0].content, "ERROR candidate three");
        assert_eq!(preview.lines[1].content, "ERROR candidate seventeen");

        for path in [source_path, log_path, meta_path, index_path] {
            let _ = tokio::fs::remove_file(path).await;
        }
    }

    #[tokio::test]
    async fn empty_candidate_ranges_do_not_open_or_scan_the_source() {
        let source_path = test_path("empty-candidate-missing-source.log");
        let log_path = test_path("empty-candidate-result.log");
        let meta_path = test_path("empty-candidate-result.meta");
        let index_path = test_path("empty-candidate-result.idx");
        let sources = vec![TempSource {
            path: source_path,
            metadata_path: None,
            label: "missing.log".into(),
            bundle_hash: None,
            file_id: None,
        }];
        let plans = vec![SourceSearchPlan::Tantivy(CandidateScanPlan {
            ranges: Vec::new(),
            seek_line: 0,
            seek_offset: 0,
        })];
        let expression = log_expression::parse("ERROR").unwrap();
        let mut log = File::create(&log_path).await.unwrap();
        let mut meta = File::create(&meta_path).await.unwrap();
        let mut index = File::create(&index_path).await.unwrap();

        let preview = TempResultExecutor::materialize_preview_with_plans(
            &sources,
            &plans,
            &expression,
            0,
            10,
            TEST_MAX_OUTPUT_BYTES,
            &mut log,
            &mut meta,
            &mut index,
        )
        .await
        .unwrap();

        assert_eq!(preview.total, 0);
        assert!(preview.lines.is_empty());
        for path in [log_path, meta_path, index_path] {
            let _ = tokio::fs::remove_file(path).await;
        }
    }

    #[test]
    fn candidate_ranges_are_sorted_and_merged_without_overlap() {
        assert_eq!(
            merge_line_ranges(vec![
                LineRange { start: 5, end: 9 },
                LineRange { start: 0, end: 4 },
                LineRange { start: 8, end: 12 },
            ]),
            vec![LineRange { start: 0, end: 12 }]
        );
    }

    #[tokio::test]
    async fn rejects_overflowing_preview_page_before_opening_sources() {
        let source_path = test_path("overflow-source.log");
        let log_path = test_path("overflow-result.log");
        let meta_path = test_path("overflow-result.meta");
        let index_path = test_path("overflow-result.idx");
        let sources = vec![TempSource {
            path: source_path.clone(),
            metadata_path: None,
            label: "missing.log".into(),
            bundle_hash: None,
            file_id: None,
        }];
        let expression = log_expression::parse("ERROR").unwrap();
        let mut log = File::create(&log_path).await.unwrap();
        let mut meta = File::create(&meta_path).await.unwrap();
        let mut index = File::create(&index_path).await.unwrap();

        let error = match TempResultExecutor::materialize_preview(
            &sources,
            &expression,
            i64::MAX,
            1,
            TEST_MAX_OUTPUT_BYTES,
            &mut log,
            &mut meta,
            &mut index,
        )
        .await
        {
            Ok(_) => panic!("overflowing page parameters must be rejected before scanning"),
            Err(error) => error,
        };

        assert!(
            matches!(error, AppError::BadRequest(message) if message == "分页参数超出支持范围")
        );
        for path in [source_path, log_path, meta_path, index_path] {
            let _ = tokio::fs::remove_file(path).await;
        }
    }

    #[tokio::test]
    async fn materializes_matches_with_source_metadata_and_sparse_checkpoints() {
        let source_path = test_path("source.log");
        let log_path = test_path("result.log");
        let meta_path = test_path("result.meta");
        let index_path = test_path("result.idx");
        let mut source_content = String::new();
        for line in 0..1_005 {
            source_content.push_str(&format!("ERROR line {line}\n"));
        }
        tokio::fs::write(&source_path, source_content)
            .await
            .unwrap();
        let sources = vec![TempSource {
            path: source_path.clone(),
            metadata_path: None,
            label: "app.log".into(),
            bundle_hash: Some("bundle-1".into()),
            file_id: Some("42".into()),
        }];
        let expression = log_expression::parse("ERROR").unwrap();
        let mut log = File::create(&log_path).await.unwrap();
        let mut meta = File::create(&meta_path).await.unwrap();
        let mut index = File::create(&index_path).await.unwrap();

        let preview = TempResultExecutor::materialize_preview(
            &sources,
            &expression,
            0,
            2,
            TEST_MAX_OUTPUT_BYTES,
            &mut log,
            &mut meta,
            &mut index,
        )
        .await
        .unwrap();

        assert_eq!(preview.total, 1_005);
        assert_eq!(preview.lines.len(), 2);
        assert_eq!(preview.lines[1].line_number, 1);
        assert_eq!(preview.lines[1].path, "app.log");
        let metadata = tokio::fs::read_to_string(&meta_path).await.unwrap();
        assert_eq!(metadata.lines().count(), 1_005);
        assert!(
            metadata
                .lines()
                .next()
                .unwrap()
                .contains("\"file_id\":\"42\"")
        );
        let checkpoints: Vec<SparseCheckpoint> = tokio::fs::read_to_string(&index_path)
            .await
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(checkpoints.len(), 2);
        assert_eq!(checkpoints[0].result_line, 0);
        assert_eq!(checkpoints[1].result_line, 1_000);

        for path in [source_path, log_path, meta_path, index_path] {
            let _ = tokio::fs::remove_file(path).await;
        }
    }

    #[tokio::test]
    async fn scans_unterminated_line_without_a_byte_budget() {
        let source_path = test_path("scan-limit-source.log");
        let log_path = test_path("scan-limit-result.log");
        let meta_path = test_path("scan-limit-result.meta");
        let index_path = test_path("scan-limit-result.idx");
        tokio::fs::write(&source_path, "ERROR ".repeat(1024))
            .await
            .unwrap();
        let sources = vec![TempSource {
            path: source_path.clone(),
            metadata_path: None,
            label: "app.log".into(),
            bundle_hash: None,
            file_id: None,
        }];
        let expression = log_expression::parse("ERROR").unwrap();
        let mut log = File::create(&log_path).await.unwrap();
        let mut meta = File::create(&meta_path).await.unwrap();
        let mut index = File::create(&index_path).await.unwrap();

        let result = TempResultExecutor::materialize_preview(
            &sources,
            &expression,
            0,
            1,
            TEST_MAX_OUTPUT_BYTES,
            &mut log,
            &mut meta,
            &mut index,
        )
        .await
        .unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.lines[0].content, "ERROR ".repeat(1024));
        for path in [source_path, log_path, meta_path, index_path] {
            let _ = tokio::fs::remove_file(path).await;
        }
    }

    #[tokio::test]
    async fn scans_source_larger_than_output_budget() {
        let source_path = test_path("exact-scan-source.log");
        let log_path = test_path("exact-scan-result.log");
        let meta_path = test_path("exact-scan-result.meta");
        let index_path = test_path("exact-scan-result.idx");
        let source_content = format!("{}ERROR tail\n", "INFO no match\n".repeat(100_000));
        tokio::fs::write(&source_path, source_content)
            .await
            .unwrap();
        let sources = vec![TempSource {
            path: source_path.clone(),
            metadata_path: None,
            label: "app.log".into(),
            bundle_hash: None,
            file_id: None,
        }];
        let expression = log_expression::parse("ERROR").unwrap();
        let mut log = File::create(&log_path).await.unwrap();
        let mut meta = File::create(&meta_path).await.unwrap();
        let mut index = File::create(&index_path).await.unwrap();

        let preview = TempResultExecutor::materialize_preview(
            &sources,
            &expression,
            0,
            1,
            1024,
            &mut log,
            &mut meta,
            &mut index,
        )
        .await
        .unwrap();

        assert_eq!(preview.total, 1);
        assert_eq!(preview.lines.len(), 1);
        for path in [source_path, log_path, meta_path, index_path] {
            let _ = tokio::fs::remove_file(path).await;
        }
    }

    #[tokio::test]
    async fn marks_temp_result_lines_truncated_at_the_logical_line_limit() {
        let source_path = test_path("truncated-source.log");
        let log_path = test_path("truncated-result.log");
        let meta_path = test_path("truncated-result.meta");
        let index_path = test_path("truncated-result.idx");
        let source_content = format!("{} LATE\n", "x".repeat(8 * 1024 * 1024));
        tokio::fs::write(&source_path, source_content)
            .await
            .unwrap();
        let sources = vec![TempSource {
            path: source_path.clone(),
            metadata_path: None,
            label: "app.log".into(),
            bundle_hash: None,
            file_id: None,
        }];
        let expression = log_expression::parse("LATE").unwrap();
        let mut log = File::create(&log_path).await.unwrap();
        let mut meta = File::create(&meta_path).await.unwrap();
        let mut index = File::create(&index_path).await.unwrap();

        let preview = TempResultExecutor::materialize_preview(
            &sources,
            &expression,
            0,
            1,
            TEST_MAX_OUTPUT_BYTES,
            &mut log,
            &mut meta,
            &mut index,
        )
        .await
        .unwrap();

        assert_eq!(preview.total, 1);
        assert!(preview.lines[0].content.ends_with(TRUNCATED_LINE_MARKER));
        drop(log);
        drop(meta);
        drop(index);
        let result = tokio::fs::read_to_string(&log_path).await.unwrap();
        assert!(result.ends_with(&format!("{TRUNCATED_LINE_MARKER}\n")));
        for path in [source_path, log_path, meta_path, index_path] {
            let _ = tokio::fs::remove_file(path).await;
        }
    }

    #[tokio::test]
    async fn full_and_preview_write_identical_artifacts() {
        let source_path = test_path("shared-source.log");
        tokio::fs::write(&source_path, "ERROR one\nINFO skip\nERROR two\n")
            .await
            .unwrap();
        let sources = vec![TempSource {
            path: source_path.clone(),
            metadata_path: None,
            label: "app.log".into(),
            bundle_hash: Some("bundle".into()),
            file_id: Some("1".into()),
        }];
        let expression = log_expression::parse("ERROR").unwrap();
        let full_paths = [
            test_path("full.log"),
            test_path("full.meta"),
            test_path("full.idx"),
        ];
        let preview_paths = [
            test_path("preview.log"),
            test_path("preview.meta"),
            test_path("preview.idx"),
        ];
        let mut full = (
            File::create(&full_paths[0]).await.unwrap(),
            File::create(&full_paths[1]).await.unwrap(),
            File::create(&full_paths[2]).await.unwrap(),
        );
        let mut preview = (
            File::create(&preview_paths[0]).await.unwrap(),
            File::create(&preview_paths[1]).await.unwrap(),
            File::create(&preview_paths[2]).await.unwrap(),
        );
        let total = TempResultExecutor::write_matches(
            &sources,
            &expression,
            &mut full.0,
            &mut full.1,
            &mut full.2,
            u64::MAX,
        )
        .await
        .unwrap();
        let outcome = TempResultExecutor::materialize_preview(
            &sources,
            &expression,
            1,
            1,
            u64::MAX,
            &mut preview.0,
            &mut preview.1,
            &mut preview.2,
        )
        .await
        .unwrap();
        assert_eq!(total, outcome.total);
        assert_eq!(outcome.lines.len(), 1);
        drop(full);
        drop(preview);
        for (full_path, preview_path) in full_paths.iter().zip(preview_paths.iter()) {
            assert_eq!(
                tokio::fs::read(full_path).await.unwrap(),
                tokio::fs::read(preview_path).await.unwrap()
            );
        }
        for path in full_paths
            .into_iter()
            .chain(preview_paths)
            .chain([source_path])
        {
            let _ = tokio::fs::remove_file(path).await;
        }
    }

    #[tokio::test]
    async fn rematerializing_an_indexed_result_preserves_original_metadata() {
        let source_path = test_path("source.log");
        let first_log_path = test_path("first.log");
        let first_meta_path = test_path("first.meta");
        let first_index_path = test_path("first.idx");
        tokio::fs::write(&source_path, "ERROR first\nERROR second\n")
            .await
            .unwrap();
        let sources = vec![TempSource {
            path: source_path.clone(),
            metadata_path: None,
            label: "original.log".into(),
            bundle_hash: Some("bundle-1".into()),
            file_id: Some("42".into()),
        }];
        let expression = log_expression::parse("ERROR").unwrap();
        let mut first_log = File::create(&first_log_path).await.unwrap();
        let mut first_meta = File::create(&first_meta_path).await.unwrap();
        let mut first_index = File::create(&first_index_path).await.unwrap();
        TempResultExecutor::materialize_preview(
            &sources,
            &expression,
            0,
            10,
            u64::MAX,
            &mut first_log,
            &mut first_meta,
            &mut first_index,
        )
        .await
        .unwrap();

        let second_log_path = test_path("second.log");
        let second_meta_path = test_path("second.meta");
        let second_index_path = test_path("second.idx");
        let nested_sources = vec![TempSource {
            path: first_log_path.clone(),
            metadata_path: Some(first_meta_path.clone()),
            label: "filtered.log".into(),
            bundle_hash: None,
            file_id: None,
        }];
        let nested_expression = log_expression::parse("second").unwrap();
        let mut second_log = File::create(&second_log_path).await.unwrap();
        let mut second_meta = File::create(&second_meta_path).await.unwrap();
        let mut second_index = File::create(&second_index_path).await.unwrap();
        let preview = TempResultExecutor::materialize_preview(
            &nested_sources,
            &nested_expression,
            0,
            10,
            u64::MAX,
            &mut second_log,
            &mut second_meta,
            &mut second_index,
        )
        .await
        .unwrap();

        assert_eq!(preview.lines[0].bundle_hash.as_deref(), Some("bundle-1"));
        assert_eq!(preview.lines[0].file_id.as_deref(), Some("42"));
        assert_eq!(preview.lines[0].path, "original.log");
        assert_eq!(preview.lines[0].line_number, 1);

        for path in [
            source_path,
            first_log_path,
            first_meta_path,
            first_index_path,
            second_log_path,
            second_meta_path,
            second_index_path,
        ] {
            let _ = tokio::fs::remove_file(path).await;
        }
    }

    #[test]
    fn selects_nearest_checkpoint_before_requested_line() {
        let checkpoints = vec![
            SparseCheckpoint {
                result_line: 0,
                log_offset: 0,
                meta_offset: 0,
            },
            SparseCheckpoint {
                result_line: 1_000,
                log_offset: 8_000,
                meta_offset: 20_000,
            },
            SparseCheckpoint {
                result_line: 2_000,
                log_offset: 16_000,
                meta_offset: 40_000,
            },
        ];

        assert_eq!(
            select_checkpoint(&checkpoints, 1_999),
            Some(&checkpoints[1])
        );
    }
}
