use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::SystemTime,
};

use actix_files::NamedFile;
use actix_web::{
    HttpRequest, HttpResponse, delete, get, http::StatusCode, http::header, post, web,
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::time::Duration as StdDuration;
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncSeekExt, BufReader, SeekFrom},
};
use uuid::Uuid;

use crate::{
    AppState, AuthRateLimitBucket,
    auth::extractor::{RequireBusinessUser, RequireUser},
    error::AppError,
    log_expression,
    repositories::files::{FileRow, ensure_text_preview, fetch_file, resolve_file_path},
    services::temp_results::{
        MatchMetadata, PreviewLine, SparseCheckpoint, TempResultExecutor, TempSource,
        select_checkpoint,
    },
};

use super::{
    helpers::{data_root, ensure_bundle_ready, load_bundle},
    issues::normalize_issue_code,
};

const RETENTION_DAYS: i64 = 7;
const ORPHAN_GRACE_PERIOD: StdDuration = StdDuration::from_secs(10 * 60);
const TEMP_RESULT_RATE_LIMIT: usize = 10;
const TEMP_RESULT_RATE_WINDOW: StdDuration = StdDuration::from_secs(60);

struct StagingLease {
    id: String,
    registry: std::sync::Arc<std::sync::Mutex<HashSet<String>>>,
}

impl Drop for StagingLease {
    fn drop(&mut self) {
        if let Ok(mut staging) = self.registry.lock() {
            staging.remove(&self.id);
        }
    }
}

fn register_staging_lease(state: &web::Data<AppState>, id: &str) -> StagingLease {
    if let Ok(mut staging) = state.temp_result_staging.lock() {
        staging.insert(id.to_string());
    }
    StagingLease {
        id: id.to_string(),
        registry: state.temp_result_staging.clone(),
    }
}

fn invalid_expression(error: log_expression::ParseError) -> AppError {
    AppError::public(
        StatusCode::BAD_REQUEST,
        "SEARCH_EXPRESSION_INVALID",
        format!(
            "搜索条件无效，请检查 AND/OR/NOT 前后是否都有关键词（位置 {}：{}）",
            error.offset, error.message
        ),
    )
}

#[derive(Deserialize)]
pub struct CreateTempResultRequest {
    expression: String,
    bundle_hash: Option<String>,
    file_id: Option<String>,
    issue_code: Option<String>,
    source_temp_id: Option<String>,
}

#[derive(Deserialize)]
pub struct PreviewTempResultRequest {
    expression: String,
    bundle_hash: Option<String>,
    file_id: Option<String>,
    issue_code: Option<String>,
    source_temp_id: Option<String>,
    from: Option<i64>,
    size: Option<i64>,
}

#[derive(Serialize, FromRow)]
pub struct TempResult {
    id: String,
    name: String,
    expression: String,
    source_label: String,
    line_count: i64,
    size_bytes: i64,
    created_at: String,
    expires_at: String,
}

#[derive(FromRow)]
pub(crate) struct TempResultRecord {
    id: String,
    name: String,
    expression: String,
    source_label: String,
    storage_path: String,
    line_count: i64,
    size_bytes: i64,
    created_at: String,
    expires_at: String,
}

#[derive(Deserialize)]
struct LinesQuery {
    start: Option<i64>,
    limit: Option<i64>,
}

#[derive(Serialize)]
struct TempResultLines {
    start: i64,
    limit: i64,
    line_count: i64,
    next_start: Option<i64>,
    lines: Vec<TempLine>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TempLine {
    bundle_hash: Option<String>,
    file_id: Option<String>,
    path: Option<String>,
    line_number: i64,
    content: String,
}

#[derive(Serialize)]
struct MaterializedPreviewResponse {
    result_id: String,
    total: i64,
    lines: Vec<PreviewLine>,
}

mod lifecycle;
mod repository;
mod routes;
mod service;
mod storage;

pub(crate) use lifecycle::cleanup_expired;
pub(crate) use routes::{
    create_temp_result, delete_temp_result, download_temp_result, get_temp_result,
    get_temp_result_lines, preview_temp_result,
};

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use actix_web::web;
    use chrono::{Duration, Utc};
    use sqlx::sqlite::SqlitePoolOptions;
    use uuid::Uuid;

    use crate::{AppState, config::AppLimits, db};

    use super::{
        lifecycle::{checked_page_end, preview_page_size, staging_path},
        storage::{ensure_temp_result_capacity, read_indexed_lines},
    };

    #[test]
    fn pagination_end_rejects_i64_overflow() {
        assert_eq!(checked_page_end(10, 5).unwrap(), 15);
        assert!(checked_page_end(i64::MAX, 1).is_err());
    }

    #[test]
    fn preview_supports_log_viewer_page_sizes() {
        assert_eq!(preview_page_size(None), 5_000);
        assert_eq!(preview_page_size(Some(5_000)), 5_000);
        assert_eq!(preview_page_size(Some(10_000)), 10_000);
        assert_eq!(preview_page_size(Some(20_000)), 10_000);
    }

    #[test]
    fn preview_uses_distinct_staging_paths_before_publication() {
        let final_path = std::path::Path::new("temp-results/result.log");

        assert_eq!(
            staging_path(final_path),
            std::path::PathBuf::from("temp-results/result.log.part")
        );
    }

    #[tokio::test]
    async fn publishing_staging_result_does_not_count_it_twice() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        db::prepare_schema(&pool, false).await.unwrap();
        let mut limits = AppLimits::default();
        limits.temp_results.max_records = 1;
        let state = web::Data::new(AppState::new(pool.clone(), PathBuf::from("data"), limits));
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO temp_results (id, status, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at) VALUES ('current', 'STAGING', 'current.log', 'x', 'x', 'data/temp-results/current.log', 0, 0, ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .unwrap();

        assert!(
            ensure_temp_result_capacity(&state, 10, Some("current"))
                .await
                .is_ok()
        );

        sqlx::query(
            "INSERT INTO temp_results (id, status, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at) VALUES ('other', 'ACTIVE', 'other.log', 'x', 'x', 'data/temp-results/other.log', 0, 0, ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            ensure_temp_result_capacity(&state, 10, Some("current"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn cleanup_recovers_from_each_deletion_checkpoint_after_restart() {
        let root = std::env::temp_dir().join(format!("rain-temp-cleanup-{}", Uuid::new_v4()));
        let temp_root = root.join("temp-results");
        tokio::fs::create_dir_all(&temp_root).await.unwrap();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        db::prepare_schema(&pool, false).await.unwrap();
        let state = web::Data::new(AppState::new(
            pool.clone(),
            PathBuf::from(&root),
            AppLimits::default(),
        ));
        let expired = (Utc::now() - Duration::days(1)).to_rfc3339();
        let created = (Utc::now() - Duration::days(2)).to_rfc3339();

        sqlx::query(
            "INSERT INTO temp_results (id, status, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at) VALUES ('active-expired', 'ACTIVE', 'a.log', 'x', 'x', ?, 0, 10, ?, ?)",
        )
        .bind(temp_root.join("active-expired.log").to_string_lossy().to_string())
        .bind(&created)
        .bind(&expired)
        .execute(&pool)
        .await
        .unwrap();

        let deleting_log = temp_root.join("deleting.log");
        tokio::fs::write(&deleting_log, "stale\n").await.unwrap();
        tokio::fs::write(temp_root.join("deleting.meta"), "{}\n")
            .await
            .unwrap();
        tokio::fs::write(temp_root.join("deleting.idx"), "{}\n")
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO temp_results (id, status, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at) VALUES ('deleting-files', 'DELETING', 'd.log', 'x', 'x', ?, 1, 10, ?, ?)",
        )
        .bind(deleting_log.to_string_lossy().to_string())
        .bind(&created)
        .bind(&expired)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO temp_results (id, status, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at) VALUES ('deleting-missing', 'DELETING', 'm.log', 'x', 'x', ?, 0, 10, ?, ?)",
        )
        .bind(temp_root.join("deleting-missing.log").to_string_lossy().to_string())
        .bind(&created)
        .bind(&expired)
        .execute(&pool)
        .await
        .unwrap();

        let active_staging_log = temp_root.join("active-staging.log");
        tokio::fs::write(&active_staging_log, "still generating\n")
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO temp_results (id, status, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at) VALUES ('active-staging', 'STAGING', 's.log', 'x', 'x', ?, 0, 0, ?, ?)",
        )
        .bind(active_staging_log.to_string_lossy().to_string())
        .bind(&created)
        .bind(&expired)
        .execute(&pool)
        .await
        .unwrap();
        let lease = super::register_staging_lease(&state, "active-staging");

        super::cleanup_expired(&state).await.unwrap();

        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM temp_results")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 1);
        assert!(!deleting_log.exists());
        assert!(active_staging_log.exists());
        drop(lease);
        super::cleanup_expired(&state).await.unwrap();
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM temp_results")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0);
        assert!(!active_staging_log.exists());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn indexed_reader_seeks_to_deep_pages_and_preserves_metadata() {
        let root = std::env::temp_dir().join(format!("rain-index-read-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let log = root.join("result.log");
        let meta = root.join("result.meta");
        let index = root.join("result.idx");
        let mut log_content = String::new();
        let mut meta_content = String::new();
        for line in 0..1_005 {
            log_content.push_str(&format!("line {line}\n"));
            meta_content.push_str(&format!(
                "{{\"bundle_hash\":\"bundle\",\"file_id\":\"42\",\"path\":\"app.log\",\"line_number\":{line}}}\n"
            ));
        }
        tokio::fs::write(&log, log_content).await.unwrap();
        tokio::fs::write(&meta, meta_content).await.unwrap();
        let log_offset = (0..1_000)
            .map(|line| format!("line {line}\n").len())
            .sum::<usize>();
        let meta_offset = (0..1_000)
            .map(|line| {
                format!(
                    "{{\"bundle_hash\":\"bundle\",\"file_id\":\"42\",\"path\":\"app.log\",\"line_number\":{line}}}\n"
                )
                .len()
            })
            .sum::<usize>();
        tokio::fs::write(
            &index,
            format!(
                "{{\"result_line\":0,\"log_offset\":0,\"meta_offset\":0}}\n{{\"result_line\":1000,\"log_offset\":{log_offset},\"meta_offset\":{meta_offset}}}\n"
            ),
        )
        .await
        .unwrap();

        let lines = read_indexed_lines(&log, &meta, &index, 1_002, 2, 1_005)
            .await
            .unwrap();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].content, "line 1002");
        assert_eq!(lines[0].line_number, 1_002);
        assert_eq!(lines[0].path.as_deref(), Some("app.log"));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn indexed_reader_rejects_jointly_truncated_content_and_metadata() {
        let root = std::env::temp_dir().join(format!("rain-index-corrupt-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let log = root.join("result.log");
        let meta = root.join("result.meta");
        let index = root.join("result.idx");
        tokio::fs::write(&log, "only one\n").await.unwrap();
        tokio::fs::write(
            &meta,
            "{\"bundle_hash\":null,\"file_id\":null,\"path\":\"app.log\",\"line_number\":0}\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            &index,
            "{\"result_line\":0,\"log_offset\":0,\"meta_offset\":0}\n",
        )
        .await
        .unwrap();

        let error = read_indexed_lines(&log, &meta, &index, 0, 2, 2)
            .await
            .expect_err("truncated sidecars must fail");

        assert!(error.to_string().contains("ended before expected line"));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn indexed_reader_rejects_empty_index_for_nonempty_result() {
        let root = std::env::temp_dir().join(format!("rain-index-empty-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let log = root.join("result.log");
        let meta = root.join("result.meta");
        let index = root.join("result.idx");
        tokio::fs::write(&log, "line\n").await.unwrap();
        tokio::fs::write(
            &meta,
            "{\"bundle_hash\":null,\"file_id\":null,\"path\":\"app.log\",\"line_number\":0}\n",
        )
        .await
        .unwrap();
        tokio::fs::write(&index, "").await.unwrap();

        let error = read_indexed_lines(&log, &meta, &index, 0, 1, 1)
            .await
            .expect_err("nonempty results require checkpoint zero");

        assert!(error.to_string().contains("index is empty"));
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
