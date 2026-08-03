use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;

use crate::{
    AppState, error::AppError, repositories::files::FileRow, services::file_reader::read_file_lines,
};

const MAX_READ_LINES: i64 = 200;

#[derive(Debug, Clone)]
pub struct SkillRunContext {
    pub run_id: String,
    pub user_id: String,
    pub issue_code: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "tool", content = "arguments", rename_all = "snake_case")]
pub enum SkillToolCall {
    ListFiles,
    SearchLogs { query: String },
    ReadFileLines { file_id: i64, start: i64, end: i64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceRange {
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
}

impl EvidenceLedger {
    pub fn evidence(&self) -> &[EvidenceRange] {
        &self.ranges
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    fn record_bytes(&mut self, count: usize, limit: usize) -> Result<(), AppError> {
        if self.total_bytes.saturating_add(count) > limit {
            return Err(AppError::BadRequest("evidence byte limit reached".into()));
        }
        self.total_bytes += count;
        Ok(())
    }

    fn record_range(&mut self, range: EvidenceRange, max_ranges: usize) -> Result<(), AppError> {
        let intervals = self.reads.entry(range.file_id).or_default();
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
        self.ranges.retain(|item| item.file_id != range.file_id);
        self.ranges
            .extend(intervals.iter().map(|(start, end)| EvidenceRange {
                file_id: range.file_id,
                path: range.path.clone(),
                start_line: *start,
                end_line: *end,
            }));
        if self.ranges.len() > max_ranges {
            return Err(AppError::BadRequest("evidence range limit reached".into()));
        }
        Ok(())
    }

    fn already_read(&self, file_id: i64, start: i64, end: i64) -> bool {
        self.reads.get(&file_id).is_some_and(|ranges| {
            ranges
                .iter()
                .any(|range| start >= range.0 && end <= range.1)
        })
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
            SkillToolCall::ListFiles => self.list_files().await,
            SkillToolCall::SearchLogs { query } => self.search_logs(&query).await,
            SkillToolCall::ReadFileLines {
                file_id,
                start,
                end,
            } => self.read_file_lines(file_id, start, end).await,
        }
    }

    pub async fn list_files(&mut self) -> Result<Value, AppError> {
        #[derive(Serialize, FromRow)]
        struct Row {
            file_id: i64,
            path: String,
            size_bytes: Option<i64>,
            line_count: Option<i64>,
            mime_type: Option<String>,
        }
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT f.id AS file_id,f.path,f.size_bytes,f.line_count,f.mime_type FROM files f JOIN bundles b ON b.id=f.bundle_id WHERE b.issue_code=? AND b.status='READY' ORDER BY f.path LIMIT 2000",
        )
        .bind(&self.context.issue_code)
        .fetch_all(&self.state.db.pool)
        .await
        .map_err(AppError::Database)?;
        let value = json!({ "files": rows });
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
            path: String,
            start_line: i64,
            end_line: i64,
            snippet: String,
        }
        let fts = format!("\"{}\"", query.replace('"', "\"\""));
        let hits: Vec<Hit> = sqlx::query_as(
            "SELECT f.id AS file_id,f.path,ls.line_offset AS start_line,ls.line_end AS end_line,substr(ls.content,1,400) AS snippet FROM log_segments_fts JOIN log_segments ls ON ls.id=log_segments_fts.rowid JOIN bundles b ON b.id=ls.bundle_id JOIN files f ON f.id=ls.file_id WHERE log_segments_fts MATCH ? AND b.issue_code=? AND b.status='READY' ORDER BY rank LIMIT ?",
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
        let mut api = self.state.limits.api.clone();
        api.max_preview_line_size = api.max_preview_line_size.min(128);
        let response = read_file_lines(
            &self.state.db.pool,
            &record,
            self.state.storage.blob_store.as_ref(),
            &api,
            start,
            end - start + 1,
        )
        .await?;
        let value = serde_json::to_value(response)
            .map_err(|_| AppError::Config("failed to serialize file evidence".into()))?;
        self.record_output(&value)?;
        self.ledger.record_range(
            EvidenceRange {
                file_id,
                path: record.path,
                start_line: start,
                end_line: end,
            },
            30,
        )?;
        Ok(value)
    }

    fn record_output(&mut self, value: &Value) -> Result<(), AppError> {
        let bytes = serde_json::to_vec(value)
            .map_err(|_| AppError::Config("failed to serialize tool output".into()))?;
        if bytes.len() > 32 * 1024 {
            return Err(AppError::BadRequest(
                "tool output byte limit reached".into(),
            ));
        }
        self.ledger.record_bytes(bytes.len(), 128 * 1024)
    }
}
