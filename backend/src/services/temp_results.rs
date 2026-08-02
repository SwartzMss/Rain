use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
};

use crate::{error::AppError, log_expression::Expression};

pub struct TempSource {
    pub path: PathBuf,
    pub metadata_path: Option<PathBuf>,
    pub label: String,
    pub bundle_hash: Option<String>,
    pub file_id: Option<String>,
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
        let mut matched = 0_i64;
        let mut lines = Vec::new();
        let mut log_offset = 0_u64;
        let mut meta_offset = 0_u64;
        let mut total_output_bytes = 0_u64;
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
                if reader
                    .read_until(b'\n', &mut bytes)
                    .await
                    .map_err(AppError::Io)?
                    == 0
                {
                    break;
                }
                let line = String::from_utf8_lossy(&bytes);
                let content = line.trim_end_matches(['\r', '\n']);
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
                if expression.matches(content) {
                    let metadata = inherited_metadata.unwrap_or_else(|| MatchMetadata {
                        bundle_hash: source.bundle_hash.clone(),
                        file_id: source.file_id.clone(),
                        path: source.label.clone(),
                        line_number: source_line,
                    });
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
                    let line_bytes = bytes.len() as u64 + u64::from(!bytes.ends_with(b"\n"));
                    let next_output_size =
                        log_offset.checked_add(line_bytes).ok_or_else(too_large)?;
                    ensure_output_capacity(total_output_bytes, line_bytes, max_output_bytes)?;
                    output.write_all(&bytes).await.map_err(AppError::Io)?;
                    log_offset = next_output_size;
                    total_output_bytes = total_output_bytes
                        .checked_add(line_bytes)
                        .ok_or_else(too_large)?;
                    if !bytes.ends_with(b"\n") {
                        output.write_all(b"\n").await.map_err(AppError::Io)?;
                    }
                    meta_offset += write_json_line(
                        metadata_output,
                        &metadata,
                        max_output_bytes,
                        &mut total_output_bytes,
                    )
                    .await?;
                    let page_end = from
                        .checked_add(size)
                        .ok_or_else(|| AppError::BadRequest("分页参数超出支持范围".into()))?;
                    if matched >= from && matched < page_end {
                        lines.push(PreviewLine {
                            bundle_hash: metadata.bundle_hash.clone(),
                            file_id: metadata.file_id.clone(),
                            path: metadata.path.clone(),
                            line_number: metadata.line_number,
                            content: content.to_string(),
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

fn too_large() -> AppError {
    AppError::public(
        actix_web::http::StatusCode::PAYLOAD_TOO_LARGE,
        "TEMP_RESULT_TOO_LARGE",
        "临时结果超过大小限制",
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
    use std::path::PathBuf;

    use tokio::fs::File;
    use uuid::Uuid;

    use super::{SparseCheckpoint, TempResultExecutor, TempSource, select_checkpoint};
    use crate::log_expression;

    fn test_path(suffix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rain-temp-result-{}-{suffix}", Uuid::new_v4()))
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
            u64::MAX,
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
