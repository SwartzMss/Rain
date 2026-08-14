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
    services::skill_time_scope::{MAX_CONTEXT_EXPANSION_MINUTES, SkillTimeScope},
};

const MAX_READ_LINES: i64 = 200;
const MAX_SKILL_LINE_BYTES: u64 = 4 * 1024;
const MAX_TOOL_OUTPUT_BYTES: usize = 32 * 1024;
const MAX_TOTAL_TOOL_OUTPUT_BYTES: usize = 128 * 1024;
const MAX_MANIFEST_BUNDLES: usize = 50;
const MAX_MANIFEST_EXTENSIONS: usize = 30;
const MAX_MANIFEST_PREFIXES: usize = 30;
const MAX_MANIFEST_LARGEST_FILES: usize = 20;

#[derive(Debug, Clone)]
pub struct SkillRunContext {
    pub run_id: String,
    pub user_id: String,
    pub issue_code: String,
    pub time_scope: Option<SkillTimeScope>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "tool", content = "arguments", rename_all = "snake_case")]
pub enum SkillToolCall {
    GetIssueManifest,
    ListFiles {
        cursor: Option<i64>,
        prefix: Option<String>,
    },
    SearchLogs {
        query: String,
        path_prefix: Option<String>,
        bundle_hash: Option<String>,
        file_id: Option<i64>,
        context_expansion_minutes: Option<i64>,
    },
    ReadFileLines {
        file_id: i64,
        start: i64,
        limit: i64,
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

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
enum SearchMode {
    Fts,
    ShortLiteral,
}

impl SearchMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fts => "fts",
            Self::ShortLiteral => "short_literal",
        }
    }
}

#[derive(Debug, Hash, PartialEq, Eq)]
struct SearchKey {
    query: String,
    path_prefix: Option<String>,
    bundle_hash: Option<String>,
    file_id: Option<i64>,
    context_expansion_minutes: i64,
    mode: SearchMode,
}

#[derive(Default)]
pub struct EvidenceLedger {
    searches: HashSet<SearchKey>,
    ranges: Vec<EvidenceRange>,
    retrieval_bytes: usize,
    reads: HashMap<i64, Vec<(i64, i64)>>,
    line_content: HashMap<(i64, i64), String>,
}

impl EvidenceLedger {
    pub fn evidence(&self) -> &[EvidenceRange] {
        &self.ranges
    }

    pub fn total_bytes(&self) -> usize {
        self.retrieval_bytes
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
        if self.retrieval_bytes.saturating_add(count) > limit {
            return Err(AppError::BadRequest("retrieval byte limit reached".into()));
        }
        self.retrieval_bytes += count;
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
    manifest_cache: Option<Value>,
}

impl<'a> SkillToolExecutor<'a> {
    pub fn new(state: &'a AppState, context: SkillRunContext) -> Self {
        Self {
            state,
            context,
            ledger: EvidenceLedger::default(),
            manifest_cache: None,
        }
    }

    pub async fn execute(&mut self, call: SkillToolCall) -> Result<Value, AppError> {
        match call {
            SkillToolCall::GetIssueManifest => self.get_issue_manifest().await,
            SkillToolCall::ListFiles { cursor, prefix } => {
                self.list_files(cursor, prefix.as_deref()).await
            }
            SkillToolCall::SearchLogs {
                query,
                path_prefix,
                bundle_hash,
                file_id,
                context_expansion_minutes,
            } => {
                self.search_logs_with_expansion(
                    &query,
                    path_prefix.as_deref(),
                    bundle_hash.as_deref(),
                    file_id,
                    context_expansion_minutes,
                )
                .await
            }
            SkillToolCall::ReadFileLines {
                file_id,
                start,
                limit,
            } => self.read_file_lines(file_id, start, limit).await,
        }
    }

    pub async fn get_issue_manifest(&mut self) -> Result<Value, AppError> {
        if let Some(value) = self.manifest_cache.clone() {
            self.record_output(&value)?;
            return Ok(value);
        }

        #[derive(Serialize, FromRow)]
        struct BundleRow {
            hash: String,
            name: String,
            file_count: i64,
            indexed_text_file_count: i64,
            content_bytes: i64,
        }
        #[derive(Serialize, FromRow)]
        struct CountRow {
            extension: String,
            file_count: i64,
        }
        #[derive(Serialize, FromRow)]
        struct PrefixRow {
            prefix: String,
            file_count: i64,
        }
        #[derive(Serialize, FromRow)]
        struct LargestFileRow {
            file_id: i64,
            bundle_hash: String,
            path: String,
            size_bytes: i64,
            line_count: Option<i64>,
        }

        let issue_code = &self.context.issue_code;
        let bundle_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM bundles WHERE issue_code=?")
                .bind(issue_code)
                .fetch_one(&self.state.db.pool)
                .await
                .map_err(AppError::Database)?;
        let ready_bundle_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM bundles WHERE issue_code=? AND status='READY'",
        )
        .bind(issue_code)
        .fetch_one(&self.state.db.pool)
        .await
        .map_err(AppError::Database)?;
        let (file_count, directory_count, indexed_text_file_count): (i64, i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(CASE WHEN f.is_dir=0 THEN 1 ELSE 0 END),0), COALESCE(SUM(CASE WHEN f.is_dir=1 THEN 1 ELSE 0 END),0), COALESCE(SUM(CASE WHEN f.is_dir=0 AND f.line_count IS NOT NULL THEN 1 ELSE 0 END),0) FROM files f JOIN bundles b ON b.id=f.bundle_id WHERE b.issue_code=? AND b.status='READY'",
        )
        .bind(issue_code)
        .fetch_one(&self.state.db.pool)
        .await
        .map_err(AppError::Database)?;
        let total_content_bytes: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(content_size_bytes),0) FROM bundles WHERE issue_code=? AND status='READY'",
        )
        .bind(issue_code)
        .fetch_one(&self.state.db.pool)
        .await
        .map_err(AppError::Database)?;
        let mut bundles: Vec<BundleRow> = sqlx::query_as(
            "SELECT b.hash,substr(b.name,1,512) AS name,COALESCE(SUM(CASE WHEN f.is_dir=0 THEN 1 ELSE 0 END),0) AS file_count,COALESCE(SUM(CASE WHEN f.is_dir=0 AND f.line_count IS NOT NULL THEN 1 ELSE 0 END),0) AS indexed_text_file_count,b.content_size_bytes AS content_bytes FROM bundles b LEFT JOIN files f ON f.bundle_id=b.id WHERE b.issue_code=? AND b.status='READY' GROUP BY b.id ORDER BY b.hash ASC LIMIT ?",
        )
        .bind(issue_code)
        .bind((MAX_MANIFEST_BUNDLES + 1) as i64)
        .fetch_all(&self.state.db.pool)
        .await
        .map_err(AppError::Database)?;
        let mut truncated = bundles.len() > MAX_MANIFEST_BUNDLES;
        bundles.truncate(MAX_MANIFEST_BUNDLES);
        let mut extensions: Vec<CountRow> = sqlx::query_as(
            "SELECT lower('.' || json_extract('[' || replace(json_quote(f.name),'.','\",\"') || ']','$[#-1]')) AS extension,COUNT(*) AS file_count FROM files f JOIN bundles b ON b.id=f.bundle_id WHERE b.issue_code=? AND b.status='READY' AND f.is_dir=0 AND instr(f.name,'.')>1 AND substr(f.name,-1)<>'.' GROUP BY extension ORDER BY file_count DESC,extension ASC LIMIT ?",
        )
        .bind(issue_code)
        .bind((MAX_MANIFEST_EXTENSIONS + 1) as i64)
        .fetch_all(&self.state.db.pool)
        .await
        .map_err(AppError::Database)?;
        truncated |= extensions.len() > MAX_MANIFEST_EXTENSIONS;
        extensions.truncate(MAX_MANIFEST_EXTENSIONS);
        let mut prefixes: Vec<PrefixRow> = sqlx::query_as(
            "SELECT CASE WHEN f.path LIKE '/%' AND instr(substr(f.path,2),'/')>0 THEN rtrim(substr(f.path,1,instr(substr(f.path,2),'/')+1),'/') WHEN f.path LIKE '/%' THEN '/' WHEN instr(f.path,'/')>0 THEN '/' || substr(f.path,1,instr(f.path,'/')-1) ELSE '/' END AS prefix,COUNT(*) AS file_count FROM files f JOIN bundles b ON b.id=f.bundle_id WHERE b.issue_code=? AND b.status='READY' AND f.is_dir=0 GROUP BY prefix ORDER BY file_count DESC,prefix ASC LIMIT ?",
        )
        .bind(issue_code)
        .bind((MAX_MANIFEST_PREFIXES + 1) as i64)
        .fetch_all(&self.state.db.pool)
        .await
        .map_err(AppError::Database)?;
        truncated |= prefixes.len() > MAX_MANIFEST_PREFIXES;
        prefixes.truncate(MAX_MANIFEST_PREFIXES);
        let mut largest_files: Vec<LargestFileRow> = sqlx::query_as(
            "SELECT f.id AS file_id,b.hash AS bundle_hash,substr(f.path,1,1024) AS path,COALESCE(f.size_bytes,0) AS size_bytes,f.line_count FROM files f JOIN bundles b ON b.id=f.bundle_id WHERE b.issue_code=? AND b.status='READY' AND f.is_dir=0 ORDER BY COALESCE(f.size_bytes,0) DESC,f.id ASC LIMIT ?",
        )
        .bind(issue_code)
        .bind((MAX_MANIFEST_LARGEST_FILES + 1) as i64)
        .fetch_all(&self.state.db.pool)
        .await
        .map_err(AppError::Database)?;
        truncated |= largest_files.len() > MAX_MANIFEST_LARGEST_FILES;
        largest_files.truncate(MAX_MANIFEST_LARGEST_FILES);

        let mut value = json!({
            "issue": {
                "code": issue_code,
                "bundle_count": bundle_count,
                "ready_bundle_count": ready_bundle_count,
                "file_count": file_count,
                "directory_count": directory_count,
                "indexed_text_file_count": indexed_text_file_count,
                "total_content_bytes": total_content_bytes
            },
            "bundles": bundles,
            "extensions": extensions,
            "top_path_prefixes": prefixes,
            "largest_files": largest_files,
            "truncated": truncated
        });
        trim_manifest(&mut value)?;
        self.manifest_cache = Some(value.clone());
        self.record_output(&value)?;
        Ok(value)
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

    pub async fn search_logs(
        &mut self,
        query: &str,
        path_prefix: Option<&str>,
        bundle_hash: Option<&str>,
        file_id: Option<i64>,
    ) -> Result<Value, AppError> {
        self.search_logs_with_expansion(query, path_prefix, bundle_hash, file_id, None)
            .await
    }

    pub async fn search_logs_with_expansion(
        &mut self,
        query: &str,
        path_prefix: Option<&str>,
        bundle_hash: Option<&str>,
        file_id: Option<i64>,
        context_expansion_minutes: Option<i64>,
    ) -> Result<Value, AppError> {
        let context_expansion_minutes = context_expansion_minutes.unwrap_or(0);
        if !(0..=MAX_CONTEXT_EXPANSION_MINUTES).contains(&context_expansion_minutes) {
            return Err(AppError::BadRequest(
                "context expansion must be between 0 and 15 minutes".into(),
            ));
        }
        let applied_scope = self
            .context
            .time_scope
            .as_ref()
            .map(|scope| {
                scope
                    .expanded(context_expansion_minutes)
                    .map_err(|error| AppError::BadRequest(error.to_string()))
            })
            .transpose()?;
        let query = query.trim();
        let query_chars = query.chars().count();
        if !(2..=200).contains(&query_chars) {
            return Err(AppError::BadRequest(
                "search query must contain 2 to 200 characters".into(),
            ));
        }
        if file_id.is_some_and(|value| value <= 0) {
            return Err(AppError::BadRequest("invalid search file_id".into()));
        }
        if query_chars == 2 && file_id.is_none() {
            return Err(AppError::BadRequest(
                "2-character search requires file_id".into(),
            ));
        }
        let path_prefix = normalize_search_filter(path_prefix, 512, "path_prefix")?;
        let bundle_hash = normalize_search_filter(bundle_hash, 128, "bundle_hash")?;
        let search_mode = if query_chars == 2 {
            SearchMode::ShortLiteral
        } else {
            SearchMode::Fts
        };
        let key = SearchKey {
            query: match search_mode {
                SearchMode::Fts => query.to_lowercase(),
                SearchMode::ShortLiteral => query.to_ascii_lowercase(),
            },
            path_prefix: path_prefix.as_deref().map(str::to_ascii_lowercase),
            bundle_hash: bundle_hash.as_deref().map(str::to_ascii_lowercase),
            file_id,
            context_expansion_minutes,
            mode: search_mode,
        };
        if !self.ledger.searches.insert(key) {
            return Ok(
                json!({ "search_mode": search_mode.as_str(), "duplicate": true, "hits": [], "truncated": false, "time_scope": applied_scope.as_ref().map(time_scope_json).unwrap_or(Value::Null), "time_index_coverage": Value::Null }),
            );
        }
        #[derive(FromRow)]
        struct HitRow {
            file_id: i64,
            bundle_hash: String,
            path: String,
            start_line: i64,
            end_line: i64,
            snippet: String,
        }
        #[derive(Serialize)]
        struct Hit {
            file_id: i64,
            bundle_hash: String,
            path: String,
            start_line: i64,
            end_line: i64,
            snippet: String,
        }
        let path_pattern = path_prefix
            .as_deref()
            .map(|value| format!("{}%", escape_like_pattern(value)));
        let max_hits = self.state.limits.api.max_search_results.clamp(1, 20);
        let fetch_limit = max_hits.saturating_add(1);
        let has_unindexed_matches = if applied_scope.is_some()
            && search_mode == SearchMode::ShortLiteral
        {
            let literal_pattern = format!("%{}%", escape_like_pattern(query));
            let marker: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM log_segments ls JOIN bundles b ON b.id=ls.bundle_id JOIN files f ON f.id=ls.file_id WHERE b.issue_code=? AND b.status='READY' AND ls.file_id=? AND (? IS NULL OR b.hash=? COLLATE NOCASE) AND (? IS NULL OR f.path LIKE ? ESCAPE '\\') AND (ls.event_time_start_ms IS NULL OR ls.event_time_end_ms IS NULL) AND ls.content LIKE ? ESCAPE '\\' COLLATE NOCASE LIMIT 1",
            )
            .bind(&self.context.issue_code)
            .bind(file_id.expect("short search file_id was validated"))
            .bind(bundle_hash.as_deref())
            .bind(bundle_hash.as_deref())
            .bind(path_pattern.as_deref())
            .bind(path_pattern.as_deref())
            .bind(literal_pattern)
            .fetch_optional(&self.state.db.pool)
            .await
            .map_err(AppError::Database)?;
            marker.is_some()
        } else if applied_scope.is_some() {
            let fts = format!("\"{}\"", query.replace('"', "\"\""));
            let marker: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM log_segments_fts JOIN log_segments ls ON ls.id=log_segments_fts.rowid JOIN bundles b ON b.id=ls.bundle_id JOIN files f ON f.id=ls.file_id WHERE log_segments_fts MATCH ? AND b.issue_code=? AND b.status='READY' AND (? IS NULL OR b.hash=? COLLATE NOCASE) AND (? IS NULL OR f.path LIKE ? ESCAPE '\\') AND (? IS NULL OR f.id=?) AND (ls.event_time_start_ms IS NULL OR ls.event_time_end_ms IS NULL) LIMIT 1",
            )
            .bind(fts)
            .bind(&self.context.issue_code)
            .bind(bundle_hash.as_deref())
            .bind(bundle_hash.as_deref())
            .bind(path_pattern.as_deref())
            .bind(path_pattern.as_deref())
            .bind(file_id)
            .bind(file_id)
            .fetch_optional(&self.state.db.pool)
            .await
            .map_err(AppError::Database)?;
            marker.is_some()
        } else {
            false
        };
        let rows: Vec<HitRow> = if search_mode == SearchMode::ShortLiteral {
            let literal_pattern = format!("%{}%", escape_like_pattern(query));
            sqlx::query_as(
                "SELECT f.id AS file_id,b.hash AS bundle_hash,substr(f.path,1,4096) AS path,ls.line_offset AS start_line,ls.line_end AS end_line,substr(ls.content,max(1,instr(lower(ls.content),lower(?))-96),400) AS snippet FROM log_segments ls JOIN bundles b ON b.id=ls.bundle_id JOIN files f ON f.id=ls.file_id WHERE b.issue_code=? AND b.status='READY' AND ls.file_id=? AND (? IS NULL OR b.hash=? COLLATE NOCASE) AND (? IS NULL OR f.path LIKE ? ESCAPE '\\') AND (? IS NULL OR (ls.event_time_start_ms IS NOT NULL AND ls.event_time_end_ms IS NOT NULL AND ls.event_time_end_ms >= ? AND ls.event_time_start_ms <= ?)) AND ls.content LIKE ? ESCAPE '\\' COLLATE NOCASE ORDER BY ls.id LIMIT ?",
            )
            .bind(query)
            .bind(&self.context.issue_code)
            .bind(file_id.expect("short search file_id was validated"))
            .bind(bundle_hash.as_deref())
            .bind(bundle_hash.as_deref())
            .bind(path_pattern.as_deref())
            .bind(path_pattern.as_deref())
            .bind(applied_scope.as_ref().map(|scope| scope.start_ms))
            .bind(applied_scope.as_ref().map(|scope| scope.start_ms))
            .bind(applied_scope.as_ref().map(|scope| scope.end_ms))
            .bind(literal_pattern)
            .bind(fetch_limit)
            .fetch_all(&self.state.db.pool)
            .await
            .map_err(AppError::Database)?
        } else {
            let fts = format!("\"{}\"", query.replace('"', "\"\""));
            sqlx::query_as(
                "SELECT f.id AS file_id,b.hash AS bundle_hash,substr(f.path,1,4096) AS path,ls.line_offset AS start_line,ls.line_end AS end_line,snippet(log_segments_fts,0,'','','',64) AS snippet FROM log_segments_fts JOIN log_segments ls ON ls.id=log_segments_fts.rowid JOIN bundles b ON b.id=ls.bundle_id JOIN files f ON f.id=ls.file_id WHERE log_segments_fts MATCH ? AND b.issue_code=? AND b.status='READY' AND (? IS NULL OR b.hash=? COLLATE NOCASE) AND (? IS NULL OR f.path LIKE ? ESCAPE '\\') AND (? IS NULL OR f.id=?) AND (? IS NULL OR (ls.event_time_start_ms IS NOT NULL AND ls.event_time_end_ms IS NOT NULL AND ls.event_time_end_ms >= ? AND ls.event_time_start_ms <= ?)) ORDER BY rank LIMIT ?",
            )
            .bind(fts)
            .bind(&self.context.issue_code)
            .bind(bundle_hash.as_deref())
            .bind(bundle_hash.as_deref())
            .bind(path_pattern.as_deref())
            .bind(path_pattern.as_deref())
            .bind(file_id)
            .bind(file_id)
            .bind(applied_scope.as_ref().map(|scope| scope.start_ms))
            .bind(applied_scope.as_ref().map(|scope| scope.start_ms))
            .bind(applied_scope.as_ref().map(|scope| scope.end_ms))
            .bind(fetch_limit)
            .fetch_all(&self.state.db.pool)
            .await
            .map_err(AppError::Database)?
        };
        let mut truncated = rows.len() > max_hits as usize;
        let mut hits: Vec<Hit> = rows
            .into_iter()
            .take(max_hits as usize)
            .map(|row| Hit {
                file_id: row.file_id,
                bundle_hash: row.bundle_hash,
                path: row.path,
                start_line: row.start_line,
                end_line: row.end_line,
                snippet: bounded_snippet_bytes(&row.snippet),
            })
            .collect();
        let value = loop {
            let candidate = json!({ "search_mode": search_mode.as_str(), "hits": hits, "truncated": truncated, "time_scope": applied_scope.as_ref().map(time_scope_json).unwrap_or(Value::Null), "time_index_coverage": applied_scope.as_ref().map(|_| json!({ "complete": !has_unindexed_matches, "excluded_unindexed_matches": has_unindexed_matches })).unwrap_or(Value::Null) });
            let size = serde_json::to_vec(&candidate)
                .map_err(|_| AppError::Config("failed to serialize tool output".into()))?
                .len();
            if size <= MAX_TOOL_OUTPUT_BYTES || hits.len() <= 1 {
                break candidate;
            }
            hits.pop();
            truncated = true;
        };
        self.record_output(&value)?;
        Ok(value)
    }

    pub async fn read_file_lines(
        &mut self,
        file_id: i64,
        start: i64,
        limit: i64,
    ) -> Result<Value, AppError> {
        if file_id <= 0 || start < 0 || !(1..=MAX_READ_LINES).contains(&limit) {
            return Err(AppError::BadRequest("invalid file line range".into()));
        }
        let end = start
            .checked_add(limit - 1)
            .ok_or_else(|| AppError::BadRequest("invalid file line range".into()))?;
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
        api.max_preview_line_size = api.max_preview_line_size.min(MAX_SKILL_LINE_BYTES);
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
        self.ledger
            .record_bytes(bytes.len(), MAX_TOTAL_TOOL_OUTPUT_BYTES)
    }
}

fn trim_manifest(value: &mut Value) -> Result<(), AppError> {
    let serialized_len = |value: &Value| {
        serde_json::to_vec(value)
            .map(|bytes| bytes.len())
            .map_err(|_| AppError::Config("failed to serialize issue manifest".into()))
    };
    if serialized_len(value)? <= MAX_TOOL_OUTPUT_BYTES {
        return Ok(());
    }
    if let Some(object) = value.as_object_mut() {
        object.insert("truncated".into(), Value::Bool(true));
    }
    for key in [
        "largest_files",
        "top_path_prefixes",
        "extensions",
        "bundles",
    ] {
        loop {
            if serialized_len(value)? <= MAX_TOOL_OUTPUT_BYTES {
                return Ok(());
            }
            let removed = value
                .get_mut(key)
                .and_then(Value::as_array_mut)
                .and_then(Vec::pop)
                .is_some();
            if !removed {
                break;
            }
        }
    }
    if serialized_len(value)? > MAX_TOOL_OUTPUT_BYTES {
        return Err(AppError::BadRequest(
            "issue manifest output exceeds the tool limit".into(),
        ));
    }
    Ok(())
}

fn normalize_search_filter(
    value: Option<&str>,
    max_chars: usize,
    name: &str,
) -> Result<Option<String>, AppError> {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    if value.is_some_and(|value| value.chars().count() > max_chars) {
        return Err(AppError::BadRequest(format!("search {name} is too long")));
    }
    Ok(value.map(str::to_owned))
}

fn escape_like_pattern(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn bounded_snippet_bytes(snippet: &str) -> String {
    const MAX_BYTES: usize = 400;
    let mut end = snippet.len().min(MAX_BYTES);
    while !snippet.is_char_boundary(end) {
        end -= 1;
    }
    snippet[..end].to_owned()
}

fn time_scope_json(scope: &SkillTimeScope) -> Value {
    json!({
        "start": scope.start,
        "end": scope.end,
        "start_ms": scope.start_ms,
        "end_ms": scope.end_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::{EvidenceLedger, EvidenceRange, bounded_snippet_bytes};

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

    #[test]
    fn snippet_byte_limit_preserves_valid_utf8() {
        let snippet = bounded_snippet_bytes(&"前".repeat(200));
        assert!(snippet.len() <= 400);
        assert!(snippet.is_char_boundary(snippet.len()));
    }
}
