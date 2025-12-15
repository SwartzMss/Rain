use actix_multipart::{Field, Multipart};
use actix_web::{HttpResponse, get, post, web};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    ingest::{ProcessFileOptions, process_uploaded_file},
    models::{
        files::{FileNode, FileNodeResponse},
        issues::{IssueBundlesResponse, UploadStatus, UploadStatusWrapper},
        logs::{LogSearchHit, LogSearchResponse},
    },
};

const MAX_LOG_RESULTS: i64 = 50;

pub fn register(cfg: &mut web::ServiceConfig) {
    cfg.service(health).service(
        web::scope("/api")
            .service(get_issue_bundles)
            .service(get_file_node)
            .service(search_logs)
            .service(upload_logs),
    );
}

#[get("/healthz")]
async fn health() -> HttpResponse {
    HttpResponse::Ok().json(json!({
        "status": "ok",
        "service": "rain-backend",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

#[get("/api/issues/{issue_id}")]
async fn get_issue_bundles(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let issue_code = path.into_inner();
    let issue =
        sqlx::query_as::<_, IssueRow>("SELECT code, name FROM issues WHERE code = $1 LIMIT 1")
            .bind(&issue_code)
            .fetch_optional(&state.pool)
            .await
            .map_err(AppError::Database)?
            .ok_or_else(|| AppError::NotFound(format!("issue {issue_code}")))?;

    let rows = sqlx::query_as::<_, BundleRow>(
        "SELECT id, hash, name, status FROM bundles WHERE issue_code = $1 ORDER BY created_at DESC",
    )
    .bind(&issue.code)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;

    let response = IssueBundlesResponse {
        name: issue.name,
        log_bundles: rows
            .into_iter()
            .map(|bundle| crate::models::issues::UploadSummary {
                hash: bundle.hash,
                name: bundle.name,
                status: UploadStatusWrapper {
                    upload_status: UploadStatus::from_db_value(&bundle.status),
                },
            })
            .collect(),
    };

    Ok(HttpResponse::Ok().json(response))
}

#[derive(Deserialize)]
struct FilePath {
    bundle_id: String,
    file_id: String,
}

#[get("/api/files/v1/{bundle_id}/files/{file_id}")]
async fn get_file_node(
    params: web::Path<FilePath>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.pool, &bundle_id).await?;
    let is_root = file_id.eq_ignore_ascii_case("root");

    let node = if is_root {
        FileNode {
            id: "root".into(),
            name: format!("{}_root", bundle.hash),
            path: format!("/{}", bundle.hash),
            is_dir: true,
            size_bytes: Some(0),
            mime_type: None,
            status: Some("READY".into()),
            meta: Some(json!({
                "bundle_hash": bundle.hash,
                "bundle_name": bundle.name,
                "storage_root": state.data_root.display().to_string()
            })),
        }
    } else {
        let parsed_id = file_id
            .parse::<i64>()
            .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
        let record = fetch_file(&state.pool, bundle.id, parsed_id).await?;
        to_file_node(record)
    };

    let parent_id = if is_root {
        None
    } else {
        Some(
            file_id
                .parse::<i64>()
                .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?,
        )
    };
    let children_records = fetch_children(&state.pool, bundle.id, parent_id).await?;
    let children = children_records.into_iter().map(to_file_node).collect();

    Ok(HttpResponse::Ok().json(FileNodeResponse { node, children }))
}

#[derive(Deserialize)]
struct LogQuery {
    q: String,
    timeline: Option<String>,
}

#[get("/api/log/v2/{bundle_id}/search")]
async fn search_logs(
    path: web::Path<String>,
    query: web::Query<LogQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let bundle_hash = path.into_inner();
    let term = query.into_inner();
    let search_term = term.q.trim();
    if search_term.is_empty() {
        return Err(AppError::BadRequest("query parameter q is required".into()));
    }

    let bundle = load_bundle(&state.pool, &bundle_hash).await?;
    let like_pattern = format!("%{}%", search_term);
    let timeline = term.timeline.and_then(|value| {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });

    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM log_segments ls
        WHERE ls.bundle_id = $1
          AND ls.content ILIKE $2
          AND ($3::text IS NULL OR ls.timeline = $3)
        "#,
    )
    .bind(bundle.id)
    .bind(&like_pattern)
    .bind(&timeline)
    .fetch_one(&state.pool)
    .await
    .map_err(AppError::Database)?;

    let rows = sqlx::query_as::<_, LogRow>(
        r#"
        SELECT ls.file_id, f.path, ls.timeline, ls.offset, ls.content
        FROM log_segments ls
        JOIN files f ON f.id = ls.file_id
        WHERE ls.bundle_id = $1
          AND ls.content ILIKE $2
          AND ($3::text IS NULL OR ls.timeline = $3)
        ORDER BY ls.offset NULLS FIRST, ls.id
        LIMIT $4
        "#,
    )
    .bind(bundle.id)
    .bind(&like_pattern)
    .bind(&timeline)
    .bind(MAX_LOG_RESULTS)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;

    let needle = search_term.to_ascii_lowercase();
    let hits = rows
        .into_iter()
        .map(|row| LogSearchHit {
            file_id: row.file_id.to_string(),
            path: row.path,
            snippet: build_snippet(&row.content, &needle),
            timeline: row.timeline,
            offset: row.offset,
        })
        .collect();

    Ok(HttpResponse::Ok().json(LogSearchResponse {
        total: total.max(0) as u64,
        hits,
    }))
}

#[post("/api/uploads")]
async fn upload_logs(
    state: web::Data<AppState>,
    mut payload: Multipart,
) -> Result<HttpResponse, AppError> {
    let mut issue_code_field: Option<String> = None;
    let mut bundle_name_field: Option<String> = None;
    let mut files: Vec<UploadedFile> = Vec::new();

    while let Some(mut field) = payload
        .try_next()
        .await
        .map_err(|err| AppError::BadRequest(format!("multipart error: {err}")))?
    {
        let content_disposition = field.content_disposition().clone();
        let field_name = content_disposition.get_name().unwrap_or("").to_string();

        match field_name.as_str() {
            "issue_code" => {
                let value = collect_text_field(&mut field).await?;
                issue_code_field = Some(value);
            }
            "bundle_name" => {
                let value = collect_text_field(&mut field).await?;
                bundle_name_field = Some(value);
            }
            "files" => {
                let filename = content_disposition
                    .get_filename()
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| "upload.log".into());

                let content_type = field.content_type().map(|mime| mime.to_string());
                let bytes = collect_binary_field(&mut field).await?;

                if !bytes.is_empty() {
                    let sanitized = sanitize_filename(&filename);
                    files.push(UploadedFile {
                        original_name: filename,
                        sanitized_name: sanitized,
                        bytes,
                        content_type,
                    });
                }
            }
            _ => {
                // Ignore unknown fields
                collect_binary_field(&mut field).await?;
            }
        }
    }

    let issue_code = issue_code_field
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest("issue_code is required".into()))?;

    if files.is_empty() {
        return Err(AppError::BadRequest("no files provided".into()));
    }

    let bundle_hash = Uuid::new_v4().simple().to_string();
    let fallback_name = files
        .first()
        .map(|file| file.original_name.clone())
        .unwrap_or_else(|| format!("bundle-{bundle_hash}"));
    let bundle_name = bundle_name_field
        .and_then(|value| {
            let trimmed = value.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        })
        .unwrap_or(fallback_name);

    ensure_issue(&state.pool, &issue_code).await?;

    let total_bytes: i64 = files.iter().map(|file| file.bytes.len() as i64).sum();

    let bundle_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO bundles (issue_code, hash, name, status, size_bytes)
        VALUES ($1, $2, $3, 'READY', $4)
        RETURNING id
        "#,
    )
    .bind(&issue_code)
    .bind(&bundle_hash)
    .bind(&bundle_name)
    .bind(Some(total_bytes))
    .fetch_one(&state.pool)
    .await
    .map_err(AppError::Database)?;

    for uploaded in &files {
        process_uploaded_file(ProcessFileOptions {
            pool: &state.pool,
            bundle_id,
            bundle_hash: &bundle_hash,
            data_root: &state.data_root,
            file_name: &uploaded.sanitized_name,
            original_name: &uploaded.original_name,
            content_type: uploaded.content_type.as_deref(),
            bytes: &uploaded.bytes,
        })
        .await?;
    }

    Ok(HttpResponse::Ok().json(UploadResponse {
        issue_code,
        bundle_hash,
        bundle_name,
        file_count: files.len() as u64,
        total_bytes: total_bytes as u64,
    }))
}

#[derive(FromRow)]
struct IssueRow {
    code: String,
    name: String,
}

#[derive(FromRow)]
struct BundleRow {
    id: Uuid,
    hash: String,
    name: String,
    status: String,
}

#[derive(FromRow)]
struct FileRow {
    id: i64,
    name: String,
    path: String,
    is_dir: bool,
    size_bytes: Option<i64>,
    mime_type: Option<String>,
    status: Option<String>,
    meta: Option<serde_json::Value>,
}

#[derive(FromRow)]
struct LogRow {
    file_id: i64,
    path: String,
    timeline: Option<String>,
    offset: Option<i64>,
    content: String,
}

async fn load_bundle(pool: &sqlx::PgPool, hash: &str) -> Result<BundleRow, AppError> {
    sqlx::query_as::<_, BundleRow>(
        "SELECT id, hash, name, status FROM bundles WHERE hash = $1 LIMIT 1",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("bundle {hash}")))
}

async fn fetch_file(
    pool: &sqlx::PgPool,
    bundle_id: Uuid,
    file_id: i64,
) -> Result<FileRow, AppError> {
    sqlx::query_as::<_, FileRow>(
        r#"
        SELECT id, name, path, is_dir, size_bytes, mime_type, status, meta
        FROM files
        WHERE bundle_id = $1 AND id = $2
        LIMIT 1
        "#,
    )
    .bind(bundle_id)
    .bind(file_id)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("file {file_id}")))
}

async fn fetch_children(
    pool: &sqlx::PgPool,
    bundle_id: Uuid,
    parent_id: Option<i64>,
) -> Result<Vec<FileRow>, AppError> {
    if let Some(parent) = parent_id {
        sqlx::query_as::<_, FileRow>(
            r#"
            SELECT id, name, path, is_dir, size_bytes, mime_type, status, meta
            FROM files
            WHERE bundle_id = $1 AND parent_id = $2
            ORDER BY is_dir DESC, name ASC
            "#,
        )
        .bind(bundle_id)
        .bind(parent)
        .fetch_all(pool)
        .await
    } else {
        sqlx::query_as::<_, FileRow>(
            r#"
            SELECT id, name, path, is_dir, size_bytes, mime_type, status, meta
            FROM files
            WHERE bundle_id = $1 AND parent_id IS NULL
            ORDER BY is_dir DESC, name ASC
            "#,
        )
        .bind(bundle_id)
        .fetch_all(pool)
        .await
    }
    .map_err(AppError::Database)
}

fn to_file_node(record: FileRow) -> FileNode {
    FileNode {
        id: record.id.to_string(),
        name: record.name,
        path: record.path,
        is_dir: record.is_dir,
        size_bytes: record.size_bytes.map(|value| value as u64),
        mime_type: record.mime_type,
        status: record.status,
        meta: record.meta,
    }
}

fn build_snippet(content: &str, needle: &str) -> String {
    if needle.is_empty() {
        return content.to_string();
    }

    let lower = content.to_ascii_lowercase();
    if let Some(pos) = lower.find(needle) {
        let start = pos.saturating_sub(40);
        let end = (pos + needle.len() + 40).min(content.len());
        if let Some(window) = content.get(start..end) {
            let mut snippet = String::new();
            if start > 0 {
                snippet.push_str("...");
            }
            snippet.push_str(window);
            if end < content.len() {
                snippet.push_str("...");
            }
            snippet
        } else {
            content.chars().take(120).collect()
        }
    } else {
        content.chars().take(120).collect()
    }
}

#[derive(Serialize)]
struct UploadResponse {
    issue_code: String,
    bundle_hash: String,
    bundle_name: String,
    file_count: u64,
    total_bytes: u64,
}

struct UploadedFile {
    original_name: String,
    sanitized_name: String,
    bytes: Vec<u8>,
    content_type: Option<String>,
}

async fn collect_text_field(field: &mut Field) -> Result<String, AppError> {
    let bytes = collect_binary_field(field).await?;
    let value = String::from_utf8(bytes)
        .map_err(|_| AppError::BadRequest("field is not valid UTF-8".into()))?;
    Ok(value.trim().to_string())
}

async fn collect_binary_field(field: &mut Field) -> Result<Vec<u8>, AppError> {
    let mut data = Vec::new();
    while let Some(chunk) = field
        .try_next()
        .await
        .map_err(|err| AppError::BadRequest(format!("failed to read field: {err}")))?
    {
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

async fn ensure_issue(pool: &sqlx::PgPool, code: &str) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO issues (code, name)
        VALUES ($1, $1)
        ON CONFLICT (code) DO NOTHING
        "#,
    )
    .bind(code)
    .execute(pool)
    .await
    .map_err(AppError::Database)?;
    Ok(())
}

fn sanitize_filename(name: &str) -> String {
    use std::path::Path;
    let fallback = "upload.log";
    let file_name = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(fallback);
    let sanitized: String = file_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        fallback.into()
    } else {
        sanitized
    }
}
