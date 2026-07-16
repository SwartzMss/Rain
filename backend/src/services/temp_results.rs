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
    pub async fn materialize_preview(
        sources: &[TempSource],
        expression: &Expression,
        from: i64,
        size: i64,
        output: &mut File,
        metadata_output: &mut File,
        index_output: &mut File,
    ) -> Result<MaterializedPreview, AppError> {
        let mut matched = 0_i64;
        let mut lines = Vec::new();
        let mut log_offset = 0_u64;
        let mut meta_offset = 0_u64;
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
                        write_json_line(index_output, &checkpoint).await?;
                    }
                    output.write_all(&bytes).await.map_err(AppError::Io)?;
                    log_offset += bytes.len() as u64;
                    if !bytes.ends_with(b"\n") {
                        output.write_all(b"\n").await.map_err(AppError::Io)?;
                        log_offset += 1;
                    }
                    meta_offset += write_json_line(metadata_output, &metadata).await?;
                    if matched >= from && matched < from + size {
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
    ) -> Result<i64, AppError> {
        let mut matching_lines = 0_i64;
        for source in sources {
            let file = File::open(&source.path).await.map_err(AppError::Io)?;
            let mut reader = BufReader::new(file);
            let mut bytes = Vec::new();
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
                if expression.matches(line.trim_end_matches(['\r', '\n'])) {
                    output.write_all(&bytes).await.map_err(AppError::Io)?;
                    if !bytes.ends_with(b"\n") {
                        output.write_all(b"\n").await.map_err(AppError::Io)?;
                    }
                    matching_lines += 1;
                }
            }
        }
        output.flush().await.map_err(AppError::Io)?;
        Ok(matching_lines)
    }
}

async fn write_json_line<T: Serialize>(output: &mut File, value: &T) -> Result<u64, AppError> {
    let mut bytes = serde_json::to_vec(value).map_err(|error| {
        AppError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })?;
    bytes.push(b'\n');
    output.write_all(&bytes).await.map_err(AppError::Io)?;
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
