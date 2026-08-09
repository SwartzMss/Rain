use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;

use crate::{
    AppState,
    error::AppError,
    file_classification::PreviewKind,
    repositories::files::{FileRow, preview_kind_for_record},
    services::file_reader::read_file_lines,
};

const MAX_READ_LINES: i64 = 200;
const MAX_TOOL_OUTPUT_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone)]
pub struct SkillRunContext {
    pub run_id: String,
    pub user_id: String,
    pub issue_code: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "tool", content = "arguments", rename_all = "snake_case")]
pub enum SkillToolCall {
    ListFiles {
        cursor: Option<i64>,
        prefix: Option<String>,
    },
    SearchLogs {
        query: String,
    },
    ReadFileLines {
        file_id: i64,
        start: i64,
        end: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceRange {
    pub bundle_hash: String,
    pub file_id: i64,
    pub path: String,
    pub start_line: i64,
    pub end_line: i64,
}

#[derive(Default)]
pub struct EvidenceLedger {
    searches: HashSet<String>,
    ranges: Vec<EvidenceRange>,
    total_bytes: usize,
    reads: HashMap<i64, Vec<(i64, i64)>>,
    line_content: HashMap<(i64, i64), String>,
}

impl EvidenceLedger {
    pub fn evidence(&self) -> &[EvidenceRange] {
        &self.ranges
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn supports_evidence(
        &self,
        bundle_hash: &str,
        file_id: i64,
        path: &str,
        start: i64,
        end: i64,
        excerpt: &str,
    ) -> bool {
        if start < 0 || end < start || excerpt.trim().is_empty() {
            return false;
        }
        let range_matches = self.ranges.iter().any(|range| {
            range.bundle_hash == bundle_hash
                && range.file_id == file_id
                && range.path == path
                && start >= range.start_line
                && end <= range.end_line
        });
        if !range_matches {
            return false;
        }
        let content = (start..=end)
            .filter_map(|line| self.line_content.get(&(file_id, line)))
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        content.contains(excerpt.trim())
    }

    fn record_bytes(&mut self, count: usize, limit: usize) -> Result<(), AppError> {
        if self.total_bytes.saturating_add(count) > limit {
            return Err(AppError::BadRequest("evidence byte limit reached".into()));
        }
        self.total_bytes += count;
        Ok(())
    }

    fn record_range(&mut self, range: EvidenceRange, max_ranges: usize) -> Result<(), AppError> {
        let mut next_reads = self.reads.clone();
        let intervals = next_reads.entry(range.file_id).or_default();
        intervals.push((range.start_line, range.end_line));
        intervals.sort_unstable();
        let mut merged: Vec<(i64, i64)> = Vec::new();
        for (start, end) in intervals.drain(..) {
            if let Some(last) = merged.last_mut()
                && start <= last.1.saturating_add(1)
            {
                last.1 = last.1.max(end);
                continue;
            }
            merged.push((start, end));
        }
        *intervals = merged;
        let mut next_ranges = self.ranges.clone();
        next_ranges.retain(|item| item.file_id != range.file_id);
        next_ranges.extend(intervals.iter().map(|(start, end)| EvidenceRange {
            bundle_hash: range.bundle_hash.clone(),
            file_id: range.file_id,
            path: range.path.clone(),
            start_line: *start,
            end_line: *end,
        }));
        if next_ranges.len() > max_ranges {
            return Err(AppError::BadRequest("evidence range limit reached".into()));
        }
        self.reads = next_reads;
        self.ranges = next_ranges;
        Ok(())
    }

    fn already_read(&self, file_id: i64, start: i64, end: i64) -> bool {
        self.unseen_ranges(file_id, start, end).is_empty()
    }

    fn unseen_ranges(&self, file_id: i64, start: i64, end: i64) -> Vec<(i64, i64)> {
        let mut unseen = Vec::new();
        let mut cursor = start;
        for &(seen_start, seen_end) in self.reads.get(&file_id).into_iter().flatten() {
            if seen_end < cursor {
                continue;
            }
            if seen_start > end {
                break;
            }
            if seen_start > cursor {
                unseen.push((cursor, end.min(seen_start - 1)));
            }
            cursor = cursor.max(seen_end.saturating_add(1));
            if cursor > end {
                break;
            }
        }
        if cursor <= end {
            unseen.push((cursor, end));
        }
        unseen
    }
}

pub struct SkillToolExecutor<'a> {
    state: &'a AppState,
    pub context: SkillRunContext,
    pub ledger: EvidenceLedger,
}

impl<'a> SkillToolExecutor<'a> {
    pub fn new(state: &'a AppState, context: SkillRunContext) -> Self {
        Self {
            state,
            context,
            ledger: EvidenceLedger::default(),
        }
    }

    pub async fn execute(&mut self, call: SkillToolCall) -> Result<Value, AppError> {
        match call {
            SkillToolCall::ListFiles { cursor, prefix } => {
                self.list_files(cursor, prefix.as_deref()).await
            }
            SkillToolCall::SearchLogs { query } => self.search_logs(&query).await,
            SkillToolCall::ReadFileLines {
                file_id,
                start,
                end,
            } => self.read_file_lines(file_id, start, end).await,
        }
    }

    pub async fn list_files(
        &mut self,
        cursor: Option<i64>,
        prefix: Option<&str>,
    ) -> Result<Value, AppError> {
        #[derive(Serialize, FromRow)]
        struct Row {
            file_id: i64,
            bundle_hash: String,
            path: String,
            path_truncated: bool,
            is_dir: bool,
            size_bytes: Option<i64>,
            line_count: Option<i64>,
            mime_type: Option<String>,
            meta: Option<String>,
        }
        let cursor = cursor.unwrap_or(0);
        if cursor < 0 {
            return Err(AppError::BadRequest("invalid file cursor".into()));
        }
        let prefix = prefix.map(str::trim).filter(|value| !value.is_empty());
        if prefix.is_some_and(|value| value.chars().count() > 512) {
            return Err(AppError::BadRequest("file prefix is too long".into()));
        }
        let prefix_pattern = prefix.map(|value| {
            format!(
                "{}%",
                value
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            )
        });
        let mut rows: Vec<Row> = sqlx::query_as(
            "SELECT f.id AS file_id,b.hash AS bundle_hash,substr(f.path,1,4096) AS path,length(f.path)>4096 AS path_truncated,f.is_dir,f.size_bytes,f.line_count,f.mime_type,f.meta FROM files f JOIN bundles b ON b.id=f.bundle_id WHERE b.issue_code=? AND b.status='READY' AND f.id>? AND (? IS NULL OR f.path LIKE ? ESCAPE '\\') ORDER BY f.id LIMIT 501",
        )
        .bind(&self.context.issue_code)
        .bind(cursor)
        .bind(prefix_pattern.as_deref())
        .bind(prefix_pattern.as_deref())
        .fetch_all(&self.state.db.pool)
        .await
        .map_err(AppError::Database)?;
        let mut has_more = rows.len() > 500;
        if has_more {
            rows.pop();
        }
        let value = loop {
            let next_cursor = has_more
                .then(|| rows.last().map(|row| row.file_id))
                .flatten();
            let candidate = {
                let files = rows.iter().map(|row| {
                let preview_kind = preview_kind_for_record(&FileRow {
                    id: row.file_id, parent_id: None, name: row.path.rsplit('/').next().unwrap_or(&row.path).to_owned(), path: row.path.clone(), is_dir: row.is_dir, size_bytes: row.size_bytes, line_count: row.line_count, mime_type: row.mime_type.clone(), status: None, meta: row.meta.clone(), blob_id: None, storage_backend: None, storage_key: None, blob_state: None,
                });
                json!({ "file_id": row.file_id, "bundle_hash": row.bundle_hash, "path": row.path, "path_truncated": row.path_truncated, "is_dir": row.is_dir, "size_bytes": row.size_bytes, "line_count": row.line_count, "mime_type": row.mime_type, "preview_kind": preview_kind, "text_readable": preview_kind == PreviewKind::Text })
                }).collect::<Vec<_>>();
                json!({ "files": files, "next_cursor": next_cursor, "truncated": has_more })
            };
            let size = serde_json::to_vec(&candidate)
                .map_err(|_| AppError::Config("failed to serialize tool output".into()))?
                .len();
            if size <= MAX_TOOL_OUTPUT_BYTES || rows.len() <= 1 {
                break candidate;
            }
            rows.pop();
            has_more = true;
        };
        self.record_output(&value)?;
        Ok(value)
    }

    pub async fn search_logs(&mut self, query: &str) -> Result<Value, AppError> {
        let query = query.trim();
        if query.chars().count() < 3 || query.chars().count() > 200 {
            return Err(AppError::BadRequest(
                "search query must contain 3 to 200 characters".into(),
            ));
        }
        let key = query.to_ascii_lowercase();
        if !self.ledger.searches.insert(key) {
            return Ok(json!({ "duplicate": true, "hits": [] }));
        }
        #[derive(Serialize, FromRow)]
        struct Hit {
            file_id: i64,
            bundle_hash: String,
            path: String,
            start_line: i64,
            end_line: i64,
            snippet: String,
        }
        let fts = format!("\"{}\"", query.replace('"', "\"\""));
        let hits: Vec<Hit> = sqlx::query_as(
            "SELECT f.id AS file_id,b.hash AS bundle_hash,f.path,ls.line_offset AS start_line,ls.line_end AS end_line,substr(ls.content,1,400) AS snippet FROM log_segments_fts JOIN log_segments ls ON ls.id=log_segments_fts.rowid JOIN bundles b ON b.id=ls.bundle_id JOIN files f ON f.id=ls.file_id WHERE log_segments_fts MATCH ? AND b.issue_code=? AND b.status='READY' ORDER BY rank LIMIT ?",
        )
        .bind(fts)
        .bind(&self.context.issue_code)
        .bind(self.state.limits.api.max_search_results.min(20))
        .fetch_all(&self.state.db.pool)
        .await
        .map_err(AppError::Database)?;
        let value = json!({ "hits": hits });
        self.record_output(&value)?;
        Ok(value)
    }

    pub async fn read_file_lines(
        &mut self,
        file_id: i64,
        start: i64,
        end: i64,
    ) -> Result<Value, AppError> {
        if start < 0 || end < start || end.saturating_sub(start) >= MAX_READ_LINES {
            return Err(AppError::BadRequest("invalid file line range".into()));
        }
        let unseen = self.ledger.unseen_ranges(file_id, start, end);
        if self.ledger.already_read(file_id, start, end) {
            return Ok(json!({ "duplicate": true, "lines": [] }));
        }
        let record: FileRow = sqlx::query_as(
            "SELECT f.id,f.parent_id,f.name,f.path,f.is_dir,f.size_bytes,f.line_count,f.mime_type,f.status,f.meta,f.blob_id,bl.storage_backend,bl.storage_key,bl.state AS blob_state FROM files f JOIN bundles b ON b.id=f.bundle_id LEFT JOIN blobs bl ON bl.id=f.blob_id WHERE f.id=? AND b.issue_code=? AND b.status='READY' LIMIT 1",
        )
        .bind(file_id)
        .bind(&self.context.issue_code)
        .fetch_optional(&self.state.db.pool)
        .await
        .map_err(AppError::Database)?
        .ok_or_else(|| AppError::NotFound("file is outside the run Issue".into()))?;
        let bundle_hash: String = sqlx::query_scalar(
            "SELECT b.hash FROM files f JOIN bundles b ON b.id=f.bundle_id WHERE f.id=? AND b.issue_code=? AND b.status='READY' LIMIT 1",
        )
        .bind(file_id)
        .bind(&self.context.issue_code)
        .fetch_one(&self.state.db.pool)
        .await
        .map_err(AppError::Database)?;
        if record.is_dir {
            let value = json!({
                "bundle_hash": bundle_hash,
                "path": record.path,
                "is_dir": true,
                "error": "FILE_IS_DIRECTORY",
                "lines": []
            });
            self.record_output(&value)?;
            return Ok(value);
        }
        if preview_kind_for_record(&record) != PreviewKind::Text {
            let value = json!({
                "bundle_hash": bundle_hash,
                "path": record.path,
                "is_dir": false,
                "error": "FILE_NOT_TEXT",
                "lines": []
            });
            self.record_output(&value)?;
            return Ok(value);
        }
        let mut api = self.state.limits.api.clone();
        api.max_preview_line_size = api.max_preview_line_size.min(128);
        let mut lines = Vec::new();
        for (unseen_start, unseen_end) in unseen {
            let response = read_file_lines(
                &self.state.db.pool,
                &record,
                self.state.storage.blob_store.as_ref(),
                &api,
                unseen_start,
                unseen_end - unseen_start + 1,
            )
            .await?;
            let response_value = serde_json::to_value(response)
                .map_err(|_| AppError::Config("failed to serialize file evidence".into()))?;
            if let Some(response_lines) = response_value.get("lines").and_then(Value::as_array) {
                lines.extend(response_lines.iter().cloned());
            }
        }
        let mut truncated = false;
        while serde_json::to_vec(
            &json!({ "bundle_hash": &bundle_hash, "path": &record.path, "is_dir": false, "lines": &lines, "truncated": truncated }),
        )
        .map_err(|_| AppError::Config("failed to serialize file evidence".into()))?
        .len()
            > MAX_TOOL_OUTPUT_BYTES
        {
            if lines.pop().is_none() {
                break;
            }
            truncated = true;
        }
        let value = json!({ "bundle_hash": bundle_hash, "path": record.path, "is_dir": false, "lines": lines, "truncated": truncated });
        self.record_output(&value)?;
        if let Some(returned_lines) = value.get("lines").and_then(Value::as_array)
            && let (Some(actual_start), Some(actual_end)) = (
                returned_lines
                    .first()
                    .and_then(|line| line.get("line_number"))
                    .and_then(Value::as_i64),
                returned_lines
                    .last()
                    .and_then(|line| line.get("line_number"))
                    .and_then(Value::as_i64),
            )
        {
            for line in returned_lines {
                if let (Some(line_number), Some(content)) = (
                    line.get("line_number").and_then(Value::as_i64),
                    line.get("content").and_then(Value::as_str),
                ) {
                    self.ledger
                        .line_content
                        .insert((file_id, line_number), content.to_owned());
                }
            }
            self.ledger.record_range(
                EvidenceRange {
                    bundle_hash: value["bundle_hash"].as_str().unwrap_or_default().to_owned(),
                    file_id,
                    path: record.path,
                    start_line: actual_start,
                    end_line: actual_end,
                },
                30,
            )?;
        }
        Ok(value)
    }

    fn record_output(&mut self, value: &Value) -> Result<(), AppError> {
        let bytes = serde_json::to_vec(value)
            .map_err(|_| AppError::Config("failed to serialize tool output".into()))?;
        if bytes.len() > MAX_TOOL_OUTPUT_BYTES {
            return Err(AppError::BadRequest(
                "tool output byte limit reached".into(),
            ));
        }
        self.ledger.record_bytes(bytes.len(), 128 * 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::{EvidenceLedger, EvidenceRange};

    #[test]
    fn overlapping_reads_return_only_unseen_intervals() {
        let mut ledger = EvidenceLedger::default();
        ledger
            .record_range(
                EvidenceRange {
                    bundle_hash: "bundle-a".into(),
                    file_id: 7,
                    path: "/app.log".into(),
                    start_line: 10,
                    end_line: 20,
                },
                30,
            )
            .unwrap();
        assert_eq!(ledger.unseen_ranges(7, 5, 25), vec![(5, 9), (21, 25)]);
        assert!(ledger.unseen_ranges(7, 12, 18).is_empty());
    }

    #[test]
    fn evidence_requires_the_recorded_path_and_excerpt() {
        let mut ledger = EvidenceLedger::default();
        ledger
            .record_range(
                EvidenceRange {
                    bundle_hash: "bundle-a".into(),
                    file_id: 7,
                    path: "/app.log".into(),
                    start_line: 10,
                    end_line: 10,
                },
                30,
            )
            .unwrap();
        ledger
            .line_content
            .insert((7, 10), "database timeout after 30s".into());
        assert!(ledger.supports_evidence("bundle-a", 7, "/app.log", 10, 10, "timeout after 30s"));
        assert!(!ledger.supports_evidence("bundle-b", 7, "/app.log", 10, 10, "timeout after 30s"));
        assert!(!ledger.supports_evidence(
            "bundle-a",
            7,
            "/forged.log",
            10,
            10,
            "timeout after 30s"
        ));
        assert!(!ledger.supports_evidence("bundle-a", 7, "/app.log", 10, 10, "root password"));
    }
}
