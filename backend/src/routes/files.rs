use actix_files::NamedFile;
use actix_web::{
    HttpResponse, delete, get,
    http::header::{Charset, ContentDisposition, DispositionParam, DispositionType, ExtendedValue},
    post, web,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    AppState,
    auth::extractor::{RequireBusinessUser, RequireUser},
    error::AppError,
    file_classification::PreviewKind,
    models::files::{FileNode, FileNodeResponse},
    repositories::files::{fetch_children, fetch_file, resolve_file_path, to_file_node},
    services::{
        file_deletion::{
            FileDeletionBatchItemInput, FileDeletionJobResponse, enqueue_file_deletion,
            enqueue_file_deletion_batch, load_file_deletion_batch, load_file_deletion_job,
        },
        file_reader::{read_file_lines, read_file_preview},
    },
};

use super::helpers::{ensure_bundle_ready, load_bundle};
use super::issues::{require_issue_owner, touch_issue_activity_best_effort};
use super::temp_results::request_client_key;

#[derive(Deserialize)]
struct FilePath {
    bundle_id: String,
    file_id: String,
}

#[derive(Deserialize)]
struct LinesQuery {
    start: Option<i64>,
    limit: Option<i64>,
}

#[derive(Deserialize)]
struct FileDeletionBatchRequest {
    items: Vec<FileDeletionBatchItemInput>,
}

// scoped under /api in routes::register
#[get("/files/v1/{bundle_id}/files/{file_id}")]
pub async fn get_file_node(
    params: web::Path<FilePath>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.db.pool, &bundle_id).await?;
    ensure_bundle_ready(&bundle)?;
    let is_root = file_id.eq_ignore_ascii_case("root");

    let node = if is_root {
        FileNode {
            id: "root".into(),
            parent_id: None,
            name: format!("{}_root", bundle.hash),
            path: format!("/{}", bundle.hash),
            is_dir: true,
            preview_kind: PreviewKind::Directory,
            size_bytes: Some(0),
            mime_type: None,
            status: Some("READY".into()),
            meta: Some(json!({
                "bundle_hash": bundle.hash,
                "bundle_name": bundle.name
            })),
        }
    } else {
        let parsed_id = file_id
            .parse::<i64>()
            .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
        let record = fetch_file(&state.db.pool, &bundle.id, parsed_id).await?;
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
    let children_records = fetch_children(&state.db.pool, &bundle.id, parent_id).await?;
    let children = children_records.into_iter().map(to_file_node).collect();
    touch_issue_activity_best_effort(&state.db.pool, &bundle.issue_code, "file tree read").await;

    Ok(HttpResponse::Ok().json(FileNodeResponse { node, children }))
}

#[get("/files/v1/{bundle_id}/files/{file_id}/content")]
pub async fn get_file_content(
    params: web::Path<FilePath>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let settings = state.settings.snapshot().await;
    let mut runtime_limits = state.limits.clone();
    let mut runtime_auth = state.auth_runtime.config.clone();
    settings
        .effective
        .apply_to_config(&mut runtime_limits, &mut runtime_auth);
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.db.pool, &bundle_id).await?;
    ensure_bundle_ready(&bundle)?;
    let parsed_id = file_id
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
    let record = fetch_file(&state.db.pool, &bundle.id, parsed_id).await?;
    let preview = read_file_preview(
        &record,
        state.storage.blob_store.as_ref(),
        &runtime_limits.api,
    )
    .await?;
    touch_issue_activity_best_effort(&state.db.pool, &bundle.issue_code, "file content read").await;
    Ok(HttpResponse::Ok().json(preview))
}

#[get("/files/v1/{bundle_id}/files/{file_id}/lines")]
pub async fn get_file_lines(
    request: actix_web::HttpRequest,
    params: web::Path<FilePath>,
    query: web::Query<LinesQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let settings = state.settings.snapshot().await;
    let mut runtime_limits = state.limits.clone();
    let mut runtime_auth = state.auth_runtime.config.clone();
    settings
        .effective
        .apply_to_config(&mut runtime_limits, &mut runtime_auth);
    let _line_read = state.acquire_line_read(&request_client_key(&request))?;
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.db.pool, &bundle_id).await?;
    ensure_bundle_ready(&bundle)?;
    let parsed_id = file_id
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
    let record = fetch_file(&state.db.pool, &bundle.id, parsed_id).await?;
    let start = query.start.unwrap_or(0).max(0);
    let limit = query
        .limit
        .unwrap_or(runtime_limits.api.default_line_page_size)
        .clamp(1, runtime_limits.api.max_line_page_size);
    let lines = read_file_lines(
        &state.db.pool,
        &record,
        state.storage.blob_store.as_ref(),
        &runtime_limits.api,
        start,
        limit,
    )
    .await?;
    touch_issue_activity_best_effort(&state.db.pool, &bundle.issue_code, "file lines read").await;

    Ok(HttpResponse::Ok().json(lines))
}

#[get("/files/v1/{bundle_id}/files/{file_id}/download")]
pub async fn download_file(
    _user: RequireUser,
    params: web::Path<FilePath>,
    state: web::Data<AppState>,
) -> Result<NamedFile, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.db.pool, &bundle_id).await?;
    ensure_bundle_ready(&bundle)?;
    let parsed_id = file_id
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
    let record = fetch_file(&state.db.pool, &bundle.id, parsed_id).await?;
    if record.is_dir {
        return Err(AppError::BadRequest("cannot download directory".into()));
    }

    let disk_path = resolve_file_path(&record, state.storage.blob_store.as_ref()).await?;
    let fallback_name = ascii_filename_fallback(&record.name);
    let named = NamedFile::open_async(disk_path)
        .await
        .map_err(AppError::Io)?
        .set_content_disposition(ContentDisposition {
            disposition: DispositionType::Attachment,
            parameters: vec![
                DispositionParam::FilenameExt(ExtendedValue {
                    charset: Charset::Ext("UTF-8".into()),
                    language_tag: None,
                    value: record.name.as_bytes().to_vec(),
                }),
                DispositionParam::Filename(fallback_name),
            ],
        });
    touch_issue_activity_best_effort(&state.db.pool, &bundle.issue_code, "file download").await;
    Ok(named)
}

fn ascii_filename_fallback(name: &str) -> String {
    let fallback: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if fallback.trim_matches('_').is_empty() {
        "download".into()
    } else {
        fallback
    }
}

#[delete("/files/v1/{bundle_id}/files/{file_id}")]
pub async fn delete_file_node(
    user: RequireBusinessUser,
    params: web::Path<FilePath>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.db.pool, &bundle_id).await?;
    require_issue_owner(&state.db.pool, &bundle.issue_code, &user.0.id).await?;
    ensure_bundle_ready(&bundle)?;
    let parsed_id = file_id
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
    let job = enqueue_file_deletion(&state.db.pool, &bundle.id, parsed_id, &user.0.id).await?;
    state.file_deletion_notify.notify_one();
    touch_issue_activity_best_effort(&state.db.pool, &bundle.issue_code, "file deletion").await;

    Ok(HttpResponse::Accepted().json(FileDeletionJobResponse::from(job)))
}

#[get("/file-deletion-jobs/{job_id}")]
pub async fn get_file_deletion_job(
    user: RequireBusinessUser,
    job_id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let job = load_file_deletion_job(&state.db.pool, &job_id, &user.0.id).await?;
    Ok(HttpResponse::Ok().json(FileDeletionJobResponse::from(job)))
}

#[post("/file-deletion-batches")]
pub async fn create_file_deletion_batch(
    user: RequireBusinessUser,
    payload: web::Json<FileDeletionBatchRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let mut issue_codes = std::collections::HashSet::new();
    for item in &payload.items {
        let bundle = load_bundle(&state.db.pool, &item.bundle_id).await?;
        require_issue_owner(&state.db.pool, &bundle.issue_code, &user.0.id).await?;
        ensure_bundle_ready(&bundle)?;
        issue_codes.insert(bundle.issue_code);
    }
    let batch = enqueue_file_deletion_batch(&state.db.pool, &user.0.id, &payload.items).await?;
    state.file_deletion_notify.notify_one();
    for issue_code in issue_codes {
        touch_issue_activity_best_effort(&state.db.pool, &issue_code, "file deletion batch").await;
    }
    Ok(HttpResponse::Accepted().json(batch))
}

#[get("/file-deletion-batches/{batch_id}")]
pub async fn get_file_deletion_batch(
    user: RequireBusinessUser,
    batch_id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let batch = load_file_deletion_batch(&state.db.pool, &batch_id, &user.0.id).await?;
    Ok(HttpResponse::Ok().json(batch))
}
