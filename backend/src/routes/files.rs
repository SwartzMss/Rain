use actix_files::NamedFile;
use actix_web::{
    HttpResponse, delete, get,
    http::header::{ContentDisposition, DispositionParam, DispositionType},
    web,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    AppState,
    error::AppError,
    file_classification::PreviewKind,
    models::files::{FileNode, FileNodeResponse},
    repositories::files::{fetch_children, fetch_file, resolve_file_path, to_file_node},
    services::{
        file_deletion::delete_file_tree,
        file_reader::{read_file_lines, read_file_preview},
    },
};

use super::helpers::{data_root, ensure_bundle_ready, load_bundle};

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

// scoped under /api in routes::register
#[get("/files/v1/{bundle_id}/files/{file_id}")]
pub async fn get_file_node(
    params: web::Path<FilePath>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.pool, &bundle_id).await?;
    ensure_bundle_ready(&bundle)?;
    let is_root = file_id.eq_ignore_ascii_case("root");

    let node = if is_root {
        FileNode {
            id: "root".into(),
            name: format!("{}_root", bundle.hash),
            path: format!("/{}", bundle.hash),
            is_dir: true,
            preview_kind: PreviewKind::Directory,
            size_bytes: Some(0),
            mime_type: None,
            status: Some("READY".into()),
            meta: Some(json!({
                "bundle_hash": bundle.hash,
                "bundle_name": bundle.name,
                "storage_root": data_root(&state).display().to_string()
            })),
        }
    } else {
        let parsed_id = file_id
            .parse::<i64>()
            .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
        let record = fetch_file(&state.pool, &bundle.id, parsed_id).await?;
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
    let children_records = fetch_children(&state.pool, &bundle.id, parent_id).await?;
    let children = children_records.into_iter().map(to_file_node).collect();

    Ok(HttpResponse::Ok().json(FileNodeResponse { node, children }))
}

#[get("/files/v1/{bundle_id}/files/{file_id}/content")]
pub async fn get_file_content(
    params: web::Path<FilePath>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.pool, &bundle_id).await?;
    ensure_bundle_ready(&bundle)?;
    let parsed_id = file_id
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
    let record = fetch_file(&state.pool, &bundle.id, parsed_id).await?;
    let preview = read_file_preview(&record, &data_root(&state), &state.limits.api).await?;
    Ok(HttpResponse::Ok().json(preview))
}

#[get("/files/v1/{bundle_id}/files/{file_id}/lines")]
pub async fn get_file_lines(
    params: web::Path<FilePath>,
    query: web::Query<LinesQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.pool, &bundle_id).await?;
    ensure_bundle_ready(&bundle)?;
    let parsed_id = file_id
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
    let record = fetch_file(&state.pool, &bundle.id, parsed_id).await?;
    let start = query.start.unwrap_or(0).max(0);
    let limit = query
        .limit
        .unwrap_or(state.limits.api.default_line_page_size)
        .clamp(1, state.limits.api.max_line_page_size);
    let lines = read_file_lines(
        &state.pool,
        &record,
        &data_root(&state),
        &state.limits.indexing,
        start,
        limit,
    )
    .await?;

    Ok(HttpResponse::Ok().json(lines))
}

#[get("/files/v1/{bundle_id}/files/{file_id}/download")]
pub async fn download_file(
    params: web::Path<FilePath>,
    state: web::Data<AppState>,
) -> Result<NamedFile, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.pool, &bundle_id).await?;
    ensure_bundle_ready(&bundle)?;
    let parsed_id = file_id
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
    let record = fetch_file(&state.pool, &bundle.id, parsed_id).await?;
    if record.is_dir {
        return Err(AppError::BadRequest("cannot download directory".into()));
    }

    let disk_path = resolve_file_path(&record, &data_root(&state))?;
    let named = NamedFile::open_async(disk_path)
        .await
        .map_err(AppError::Io)?
        .set_content_disposition(ContentDisposition {
            disposition: DispositionType::Attachment,
            parameters: vec![DispositionParam::Filename(record.name)],
        });
    Ok(named)
}

#[delete("/files/v1/{bundle_id}/files/{file_id}")]
pub async fn delete_file_node(
    params: web::Path<FilePath>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.pool, &bundle_id).await?;
    ensure_bundle_ready(&bundle)?;
    let parsed_id = file_id
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
    let _record = fetch_file(&state.pool, &bundle.id, parsed_id).await?;
    delete_file_tree(&state.pool, &data_root(&state), &bundle.id, parsed_id).await?;

    Ok(HttpResponse::NoContent().finish())
}
