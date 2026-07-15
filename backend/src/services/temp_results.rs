use std::path::PathBuf;

use serde::Serialize;
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
};

use crate::{error::AppError, log_expression::Expression};

pub struct TempSource {
    pub path: PathBuf,
    pub label: String,
    pub bundle_hash: Option<String>,
    pub file_id: Option<String>,
}

#[derive(Serialize)]
pub struct TempPreview {
    pub total: i64,
    pub lines: Vec<PreviewLine>,
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
    pub async fn scan_preview(
        sources: &[TempSource],
        expression: &Expression,
        from: i64,
        size: i64,
    ) -> Result<TempPreview, AppError> {
        let mut matched = 0_i64;
        let mut lines = Vec::new();
        for source in sources {
            let file = File::open(&source.path).await.map_err(AppError::Io)?;
            let mut reader = BufReader::new(file);
            let mut bytes = Vec::new();
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
                if expression.matches(content) {
                    if matched >= from && matched < from + size {
                        lines.push(PreviewLine {
                            bundle_hash: source.bundle_hash.clone(),
                            file_id: source.file_id.clone(),
                            path: source.label.clone(),
                            line_number: source_line,
                            content: content.to_string(),
                        });
                    }
                    matched += 1;
                }
                source_line += 1;
            }
        }
        Ok(TempPreview {
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
