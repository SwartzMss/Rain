use async_trait::async_trait;
use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool};
use std::collections::HashMap;

const LINE_OFFSET_BATCH_SIZE: usize = 500;
const SEGMENT_BATCH_SIZE: usize = 100;

use crate::{error::AppError, search::*};

#[derive(Clone)]
pub struct SqliteFtsSearchIndex {
    pool: SqlitePool,
}

impl SqliteFtsSearchIndex {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    async fn commit_batch_inner(&self, batch: IndexBatch) -> Result<(), AppError> {
        crate::db::write::run(self.pool(), "index-batch", &batch, |conn, batch| {
            Box::pin(async move {
                flush_log_chunks(conn, &batch.bundle_id, batch.file_id, &batch.chunks).await?;
                insert_line_offsets(conn, batch.file_id, &batch.offsets).await?;
                if let Some(line_count) = batch.final_line_count {
                    sqlx::query("UPDATE files SET line_count = ? WHERE id = ?")
                        .bind(line_count)
                        .bind(batch.file_id)
                        .execute(conn)
                        .await
                        .map_err(AppError::Database)?;
                }
                Ok(())
            })
        })
        .await
    }
}

#[derive(FromRow)]
struct BundleRow {
    file_id: i64,
    path: String,
    timeline: Option<String>,
    offset: Option<i64>,
    line_end: Option<i64>,
    chunk_index: Option<i64>,
    content: String,
}

#[derive(FromRow)]
struct IssueRow {
    file_id: i64,
    path: String,
    offset: Option<i64>,
    line_end: Option<i64>,
    chunk_index: Option<i64>,
    content: String,
    bundle_hash: String,
}

#[derive(FromRow)]
struct FilenameRow {
    file_id: i64,
    name: String,
    path: String,
    bundle_hash: String,
}

fn escape_like_pattern(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn build_fts_query(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

async fn search_bundle(
    pool: &SqlitePool,
    request: ContentSearchRequest,
    bundle_id: &str,
    timeline: Option<&str>,
    file_id: Option<i64>,
) -> Result<ContentSearchResult, AppError> {
    let query_chars = request.query.chars().count();
    let path_pattern = request
        .path_like
        .as_deref()
        .filter(|v| !v.is_empty())
        .map(|v| format!("%{v}%"));
    if query_chars < 3 {
        return Err(AppError::BadRequest("搜索关键词至少需要 3 个字符".into()));
    }
    let fts = build_fts_query(&request.query);
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM log_segments ls JOIN log_segments_fts ON log_segments_fts.rowid=ls.id JOIN visible_files f ON f.id=ls.file_id WHERE log_segments_fts MATCH ? AND ls.bundle_id=? AND (? IS NULL OR ls.timeline=?) AND (? IS NULL OR f.path LIKE ?) AND (? IS NULL OR ls.file_id=?)",
    )
    .bind(&fts).bind(bundle_id).bind(timeline).bind(timeline)
    .bind(&path_pattern).bind(&path_pattern).bind(file_id).bind(file_id)
    .fetch_one(pool).await.map_err(AppError::Database)?;
    let rows = sqlx::query_as::<_, BundleRow>(
        "SELECT ls.file_id,f.path,ls.timeline,ls.line_offset AS offset,ls.line_end,ls.chunk_index,ls.content FROM log_segments ls JOIN log_segments_fts ON log_segments_fts.rowid=ls.id JOIN visible_files f ON f.id=ls.file_id WHERE log_segments_fts MATCH ? AND ls.bundle_id=? AND (? IS NULL OR ls.timeline=?) AND (? IS NULL OR f.path LIKE ?) AND (? IS NULL OR ls.file_id=?) ORDER BY ls.line_offset NULLS FIRST,ls.id LIMIT ? OFFSET ?",
    )
    .bind(&fts).bind(bundle_id).bind(timeline).bind(timeline)
    .bind(&path_pattern).bind(&path_pattern).bind(file_id).bind(file_id)
    .bind(request.size).bind(request.from).fetch_all(pool).await.map_err(AppError::Database)?;
    Ok(ContentSearchResult {
        total,
        truncated: false,
        rows: rows
            .into_iter()
            .map(|row| ContentSearchRow {
                file_id: row.file_id,
                path: row.path,
                bundle_hash: None,
                timeline: row.timeline,
                offset: row.offset,
                line_end: row.line_end,
                chunk_index: row.chunk_index,
                content: row.content,
            })
            .collect(),
    })
}

async fn search_issue(
    pool: &SqlitePool,
    request: ContentSearchRequest,
    issue_code: &str,
) -> Result<ContentSearchResult, AppError> {
    if request.query.chars().count() < 3 {
        return Err(AppError::BadRequest("搜索关键词至少需要 3 个字符".into()));
    }
    let path_pattern = request
        .path_like
        .as_deref()
        .filter(|v| !v.is_empty())
        .map(|v| format!("%{v}%"));
    let fts = build_fts_query(&request.query);
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_segments ls JOIN log_segments_fts ON log_segments_fts.rowid=ls.id JOIN bundles b ON b.id=ls.bundle_id JOIN issues i ON i.code=b.issue_code JOIN visible_files f ON f.id=ls.file_id WHERE log_segments_fts MATCH ? AND b.issue_code=? AND i.status='ACTIVE' AND b.status='READY' AND (? IS NULL OR f.path LIKE ?)")
        .bind(&fts).bind(issue_code).bind(&path_pattern).bind(&path_pattern).fetch_one(pool).await.map_err(AppError::Database)?;
    let rows = sqlx::query_as::<_, IssueRow>("SELECT ls.file_id,f.path,ls.line_offset AS offset,ls.line_end,ls.chunk_index,ls.content,b.hash AS bundle_hash FROM log_segments ls JOIN log_segments_fts ON log_segments_fts.rowid=ls.id JOIN bundles b ON b.id=ls.bundle_id JOIN issues i ON i.code=b.issue_code JOIN visible_files f ON f.id=ls.file_id WHERE log_segments_fts MATCH ? AND b.issue_code=? AND i.status='ACTIVE' AND b.status='READY' AND (? IS NULL OR f.path LIKE ?) ORDER BY ls.line_offset NULLS FIRST,ls.id LIMIT ? OFFSET ?")
        .bind(&fts).bind(issue_code).bind(&path_pattern).bind(&path_pattern).bind(request.size).bind(request.from).fetch_all(pool).await.map_err(AppError::Database)?;
    Ok(ContentSearchResult {
        total,
        truncated: false,
        rows: rows
            .into_iter()
            .map(|row| ContentSearchRow {
                file_id: row.file_id,
                path: row.path,
                bundle_hash: Some(row.bundle_hash),
                timeline: None,
                offset: row.offset,
                line_end: row.line_end,
                chunk_index: row.chunk_index,
                content: row.content,
            })
            .collect(),
    })
}

async fn search_skill_sqlite(
    pool: &SqlitePool,
    request: SkillSearchRequest,
) -> Result<SkillSearchResult, AppError> {
    let SkillSearchRequest {
        issue_code,
        query,
        mode: search_mode,
        path_prefix,
        bundle_hash,
        file_id,
        time_window,
        fetch_limit,
    } = request;
    let query = query.as_str();
    #[derive(FromRow)]
    struct HitRow {
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
    // The legacy event_time_*_ms columns contain packed wall-clock
    // comparison keys. They are only comparable with the matching
    // SkillTimeScope keys; they are not Unix or UTC timestamps.
    let has_unindexed_matches = if time_window.is_some()
        && search_mode == SkillSearchMode::ShortLiteral
    {
        let literal_pattern = format!("%{}%", escape_like_pattern(query));
        let marker: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM log_segments ls JOIN bundles b ON b.id=ls.bundle_id JOIN visible_files f ON f.id=ls.file_id WHERE b.issue_code=? AND b.status='READY' AND ls.file_id=? AND (? IS NULL OR b.hash=? COLLATE NOCASE) AND (? IS NULL OR f.path LIKE ? ESCAPE '\\') AND (ls.event_time_indexed != 1 OR ls.event_time_start_ms IS NULL OR ls.event_time_end_ms IS NULL) AND ls.content LIKE ? ESCAPE '\\' COLLATE NOCASE LIMIT 1",
            )
            .bind(&issue_code)
            .bind(file_id.ok_or_else(|| AppError::BadRequest("2-character search requires file_id".into()))?)
            .bind(bundle_hash.as_deref())
            .bind(bundle_hash.as_deref())
            .bind(path_pattern.as_deref())
            .bind(path_pattern.as_deref())
            .bind(literal_pattern)
            .fetch_optional(pool)
            .await
            .map_err(AppError::Database)?;
        marker.is_some()
    } else if time_window.is_some() {
        let fts = format!("\"{}\"", query.replace('"', "\"\""));
        let marker: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM log_segments_fts JOIN log_segments ls ON ls.id=log_segments_fts.rowid JOIN bundles b ON b.id=ls.bundle_id JOIN visible_files f ON f.id=ls.file_id WHERE log_segments_fts MATCH ? AND b.issue_code=? AND b.status='READY' AND (? IS NULL OR b.hash=? COLLATE NOCASE) AND (? IS NULL OR f.path LIKE ? ESCAPE '\\') AND (? IS NULL OR f.id=?) AND (ls.event_time_indexed != 1 OR ls.event_time_start_ms IS NULL OR ls.event_time_end_ms IS NULL) LIMIT 1",
            )
            .bind(fts)
            .bind(&issue_code)
            .bind(bundle_hash.as_deref())
            .bind(bundle_hash.as_deref())
            .bind(path_pattern.as_deref())
            .bind(path_pattern.as_deref())
            .bind(file_id)
            .bind(file_id)
            .fetch_optional(pool)
            .await
            .map_err(AppError::Database)?;
        marker.is_some()
    } else {
        false
    };
    let rows: Vec<HitRow> = if search_mode == SkillSearchMode::ShortLiteral {
        let literal_pattern = format!("%{}%", escape_like_pattern(query));
        sqlx::query_as(
                "SELECT f.id AS file_id,b.hash AS bundle_hash,substr(f.path,1,4096) AS path,ls.line_offset AS start_line,ls.line_end AS end_line,substr(ls.content,max(1,instr(lower(ls.content),lower(?))-96),400) AS snippet FROM log_segments ls JOIN bundles b ON b.id=ls.bundle_id JOIN visible_files f ON f.id=ls.file_id WHERE b.issue_code=? AND b.status='READY' AND ls.file_id=? AND (? IS NULL OR b.hash=? COLLATE NOCASE) AND (? IS NULL OR f.path LIKE ? ESCAPE '\\') AND (? IS NULL OR (ls.event_time_indexed = 1 AND ls.event_time_start_ms IS NOT NULL AND ls.event_time_end_ms IS NOT NULL AND ls.event_time_end_ms >= ? AND ls.event_time_start_ms <= ?)) AND ls.content LIKE ? ESCAPE '\\' COLLATE NOCASE ORDER BY ls.id LIMIT ?",
            )
            .bind(query)
            .bind(&issue_code)
            .bind(file_id.ok_or_else(|| AppError::BadRequest("2-character search requires file_id".into()))?)
            .bind(bundle_hash.as_deref())
            .bind(bundle_hash.as_deref())
            .bind(path_pattern.as_deref())
            .bind(path_pattern.as_deref())
            .bind(time_window.as_ref().map(|scope| scope.start_key))
            .bind(time_window.as_ref().map(|scope| scope.start_key))
            .bind(time_window.as_ref().map(|scope| scope.end_key))
            .bind(literal_pattern)
            .bind(fetch_limit)
            .fetch_all(pool)
            .await
            .map_err(AppError::Database)?
    } else {
        let fts = format!("\"{}\"", query.replace('"', "\"\""));
        sqlx::query_as(
                "SELECT f.id AS file_id,b.hash AS bundle_hash,substr(f.path,1,4096) AS path,ls.line_offset AS start_line,ls.line_end AS end_line,snippet(log_segments_fts,0,'','','',64) AS snippet FROM log_segments_fts JOIN log_segments ls ON ls.id=log_segments_fts.rowid JOIN bundles b ON b.id=ls.bundle_id JOIN visible_files f ON f.id=ls.file_id WHERE log_segments_fts MATCH ? AND b.issue_code=? AND b.status='READY' AND (? IS NULL OR b.hash=? COLLATE NOCASE) AND (? IS NULL OR f.path LIKE ? ESCAPE '\\') AND (? IS NULL OR f.id=?) AND (? IS NULL OR (ls.event_time_indexed = 1 AND ls.event_time_start_ms IS NOT NULL AND ls.event_time_end_ms IS NOT NULL AND ls.event_time_end_ms >= ? AND ls.event_time_start_ms <= ?)) ORDER BY rank LIMIT ?",
            )
            .bind(fts)
            .bind(&issue_code)
            .bind(bundle_hash.as_deref())
            .bind(bundle_hash.as_deref())
            .bind(path_pattern.as_deref())
            .bind(path_pattern.as_deref())
            .bind(file_id)
            .bind(file_id)
            .bind(time_window.as_ref().map(|scope| scope.start_key))
            .bind(time_window.as_ref().map(|scope| scope.start_key))
            .bind(time_window.as_ref().map(|scope| scope.end_key))
            .bind(fetch_limit)
            .fetch_all(pool)
            .await
            .map_err(AppError::Database)?
    };

    Ok(SkillSearchResult {
        rows: rows
            .into_iter()
            .map(|row| SkillSearchRow {
                file_id: row.file_id,
                bundle_hash: row.bundle_hash,
                path: row.path,
                start_line: row.start_line,
                end_line: row.end_line,
                snippet: row.snippet,
            })
            .collect(),
        has_unindexed_matches,
    })
}

#[async_trait]
impl SearchIndex for SqliteFtsSearchIndex {
    async fn search_content(
        &self,
        request: ContentSearchRequest,
    ) -> Result<ContentSearchResult, AppError> {
        match request.scope.clone() {
            ContentSearchScope::Bundle {
                bundle_id,
                timeline,
                file_id,
            } => {
                search_bundle(
                    self.pool(),
                    request,
                    &bundle_id,
                    timeline.as_deref(),
                    file_id,
                )
                .await
            }
            ContentSearchScope::Issue { issue_code } => {
                search_issue(self.pool(), request, &issue_code).await
            }
        }
    }

    async fn search_filenames(
        &self,
        request: FilenameSearchRequest,
    ) -> Result<FilenameSearchResult, AppError> {
        let pattern = format!("%{}%", escape_like_pattern(&request.query));
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM visible_files f JOIN bundles b ON b.id=f.bundle_id JOIN issues i ON i.code=b.issue_code WHERE b.issue_code=? AND i.status='ACTIVE' AND b.status='READY' AND f.is_dir=0 AND (f.name LIKE ? ESCAPE '\\' COLLATE NOCASE OR f.path LIKE ? ESCAPE '\\' COLLATE NOCASE)")
            .bind(&request.issue_code).bind(&pattern).bind(&pattern).fetch_one(self.pool()).await.map_err(AppError::Database)?;
        let rows = sqlx::query_as::<_, FilenameRow>("SELECT f.id AS file_id,f.name,CASE WHEN f.parent_id IS NULL THEN f.name ELSE f.path END AS path,b.hash AS bundle_hash FROM visible_files f JOIN bundles b ON b.id=f.bundle_id JOIN issues i ON i.code=b.issue_code WHERE b.issue_code=? AND i.status='ACTIVE' AND b.status='READY' AND f.is_dir=0 AND (f.name LIKE ? ESCAPE '\\' COLLATE NOCASE OR f.path LIKE ? ESCAPE '\\' COLLATE NOCASE) ORDER BY CASE WHEN f.name=? COLLATE NOCASE THEN 0 ELSE 1 END,f.name COLLATE NOCASE,f.path COLLATE NOCASE LIMIT ? OFFSET ?")
            .bind(&request.issue_code).bind(&pattern).bind(&pattern).bind(&request.query).bind(request.size).bind(request.from).fetch_all(self.pool()).await.map_err(AppError::Database)?;
        Ok(FilenameSearchResult {
            total,
            rows: rows
                .into_iter()
                .map(|row| FilenameSearchRow {
                    file_id: row.file_id,
                    name: row.name,
                    path: row.path,
                    bundle_hash: row.bundle_hash,
                })
                .collect(),
        })
    }

    async fn search_skill(
        &self,
        request: SkillSearchRequest,
    ) -> Result<SkillSearchResult, AppError> {
        search_skill_sqlite(self.pool(), request).await
    }

    async fn commit_batch(&self, batch: IndexBatch) -> Result<(), AppError> {
        self.commit_batch_inner(batch).await
    }
}

#[async_trait]
impl IngestIndex for SqliteFtsSearchIndex {
    async fn commit_ingest_batch(&self, batch: IndexBatch) -> Result<(), AppError> {
        self.commit_batch_inner(batch).await
    }
}

async fn insert_line_offsets(
    tx: &mut sqlx::SqliteConnection,
    file_id: i64,
    offsets: &[(i64, i64)],
) -> Result<(), AppError> {
    for batch in offsets.chunks(LINE_OFFSET_BATCH_SIZE) {
        let mut builder = QueryBuilder::<Sqlite>::new(
            "INSERT INTO log_line_offsets (file_id, line_number, byte_offset) ",
        );
        builder.push_values(batch, |mut row, (line_number, byte_offset)| {
            row.push_bind(file_id)
                .push_bind(*line_number)
                .push_bind(*byte_offset);
        });
        builder
            .build()
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;
    }
    Ok(())
}

async fn flush_log_chunks(
    tx: &mut sqlx::SqliteConnection,
    bundle_id: &str,
    file_id: i64,
    chunks: &[IndexChunk],
) -> Result<(), AppError> {
    for batch in chunks.chunks(SEGMENT_BATCH_SIZE) {
        let mut segments = QueryBuilder::<Sqlite>::new(
            "INSERT INTO log_segments (bundle_id, file_id, timeline, content, line_offset, line_end, chunk_index, event_time_start_ms, event_time_end_ms, event_time_indexed) ",
        );
        segments.push_values(batch, |mut row, chunk| {
            row.push_bind(bundle_id)
                .push_bind(file_id)
                .push_bind("all")
                .push_bind(&chunk.content)
                .push_bind(chunk.line_start)
                .push_bind(chunk.line_end)
                .push_bind(chunk.chunk_index)
                .push_bind(chunk.event_time_start_ms)
                .push_bind(chunk.event_time_end_ms)
                .push_bind(1_i64);
        });
        segments.push(" RETURNING id, chunk_index");
        let returned = segments
            .build_query_as::<(i64, i64)>()
            .fetch_all(&mut *tx)
            .await
            .map_err(AppError::Database)?;
        let mut segment_ids = HashMap::with_capacity(returned.len());
        for (segment_id, chunk_index) in returned {
            if segment_ids.insert(chunk_index, segment_id).is_some() {
                return Err(AppError::Database(sqlx::Error::Protocol(format!(
                    "duplicate returned log chunk index {chunk_index}"
                ))));
            }
        }
        if segment_ids.len() != batch.len() {
            return Err(AppError::Database(sqlx::Error::Protocol(
                "log segment insert did not return every chunk".into(),
            )));
        }
        if batch
            .iter()
            .any(|chunk| !segment_ids.contains_key(&chunk.chunk_index))
        {
            return Err(AppError::Database(sqlx::Error::Protocol(
                "log segment insert returned an unexpected chunk index".into(),
            )));
        }

        // log_segments_fts is an external-content table maintained by triggers.
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (SqlitePool, i64) {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, false).await.unwrap();
        sqlx::query("INSERT INTO issues(code,name) VALUES('SEARCH','Search')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('bundle','SEARCH','hash','Bundle','READY')").execute(&pool).await.unwrap();
        let file = sqlx::query_scalar("INSERT INTO files(bundle_id,name,path,is_dir) VALUES('bundle','app.log','/app.log',0) RETURNING id").fetch_one(&pool).await.unwrap();
        (pool, file)
    }
    fn batch(file_id: i64) -> IndexBatch {
        IndexBatch {
            bundle_id: "bundle".into(),
            file_id,
            path: "/app.log".into(),
            chunks: vec![IndexChunk {
                chunk_index: 0,
                line_start: Some(0),
                line_end: Some(1),
                event_time_start_ms: Some(100),
                event_time_end_ms: Some(200),
                content: "marker rv 中文连续文本".into(),
            }],
            offsets: vec![(0, 0)],
            final_line_count: Some(2),
        }
    }
    #[tokio::test]
    async fn adapter_preserves_content_filename_and_skill_results() {
        let (pool, file) = fixture().await;
        let index = SqliteFtsSearchIndex::new(pool.clone());
        index.commit_batch(batch(file)).await.unwrap();
        let content = index
            .search_content(ContentSearchRequest {
                scope: ContentSearchScope::Issue {
                    issue_code: "SEARCH".into(),
                },
                query: "marker".into(),
                path_like: Some("app".into()),
                from: 0,
                size: 10,
            })
            .await
            .unwrap();
        assert_eq!(content.total, 1);
        assert_eq!(content.rows[0].file_id, file);
        assert_eq!(content.rows[0].offset, Some(0));
        assert_eq!(content.rows[0].line_end, Some(1));
        let names = index
            .search_filenames(FilenameSearchRequest {
                issue_code: "SEARCH".into(),
                query: "app.log".into(),
                from: 0,
                size: 10,
            })
            .await
            .unwrap();
        assert_eq!(names.total, 1);
        assert_eq!(names.rows[0].path, "app.log");
        for (mode, query) in [
            (SkillSearchMode::Fts, "marker"),
            (SkillSearchMode::ShortLiteral, "rv"),
        ] {
            let result = index
                .search_skill(SkillSearchRequest {
                    issue_code: "SEARCH".into(),
                    query: query.into(),
                    mode,
                    path_prefix: Some("/app".into()),
                    bundle_hash: Some("HASH".into()),
                    file_id: Some(file),
                    time_window: Some(SearchTimeWindow {
                        start_key: 150,
                        end_key: 160,
                    }),
                    fetch_limit: 21,
                })
                .await
                .unwrap();
            assert_eq!(result.rows.len(), 1);
            assert_eq!(result.rows[0].file_id, file);
            assert!(!result.has_unindexed_matches);
        }
        pool.close().await;
    }
    #[tokio::test]
    async fn ignored_chunk_insert_rolls_back_the_whole_batch() {
        let (pool, file) = fixture().await;
        sqlx::query("CREATE TRIGGER ignore_test_chunk BEFORE INSERT ON log_segments WHEN new.chunk_index=1 BEGIN SELECT RAISE(IGNORE); END").execute(&pool).await.unwrap();
        let index = SqliteFtsSearchIndex::new(pool.clone());
        let mut batch = batch(file);
        let mut ignored = batch.chunks[0].clone();
        ignored.chunk_index = 1;
        batch.chunks.push(ignored);
        assert!(index.commit_batch(batch).await.is_err());
        let segments: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_segments")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(segments, 0);
        let offsets: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_line_offsets")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(offsets, 0);
        pool.close().await;
    }
    #[tokio::test]
    async fn batch_boundaries_and_pagination_preserve_all_rows() {
        let (pool, file) = fixture().await;
        let index = SqliteFtsSearchIndex::new(pool.clone());
        let mut batch = batch(file);
        batch.chunks = (0..201)
            .map(|n| IndexChunk {
                chunk_index: n,
                line_start: Some(n),
                line_end: Some(n),
                event_time_start_ms: None,
                event_time_end_ms: None,
                content: format!("marker chunk {n}"),
            })
            .collect();
        batch.offsets = (0..501).map(|n| (n, n * 100)).collect();
        batch.final_line_count = Some(501);
        index.commit_batch(batch).await.unwrap();
        let rows: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM log_segments),(SELECT COUNT(*) FROM log_line_offsets)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rows, (201, 501));
        let result = index
            .search_content(ContentSearchRequest {
                scope: ContentSearchScope::Bundle {
                    bundle_id: "bundle".into(),
                    timeline: Some("all".into()),
                    file_id: Some(file),
                },
                query: "marker".into(),
                path_like: None,
                from: 100,
                size: 2,
            })
            .await
            .unwrap();
        assert_eq!(result.total, 201);
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0].chunk_index, Some(100));
        assert_eq!(result.rows[1].offset, Some(101));
        assert!(!result.truncated);
        let line_count: i64 = sqlx::query_scalar("SELECT line_count FROM files WHERE id=?")
            .bind(file)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(line_count, 501);
        pool.close().await;
    }
}
