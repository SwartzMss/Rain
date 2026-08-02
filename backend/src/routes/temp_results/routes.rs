use super::*;
use super::{lifecycle::*, service, storage::*};

#[post("/temp-results/preview")]
pub(crate) async fn preview_temp_result(
    request: HttpRequest,
    payload: web::Json<PreviewTempResultRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    service::create_preview_result(request, payload, state).await
}

#[post("/temp-results")]
pub(crate) async fn create_temp_result(
    request: HttpRequest,
    payload: web::Json<CreateTempResultRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    service::create_full_result(request, payload, state).await
}

#[get("/temp-results/{id}")]
pub(crate) async fn get_temp_result(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    cleanup_expired(&state).await?;
    let result = load_and_renew(&state, &id).await?;
    Ok(HttpResponse::Ok().json(to_response(result)))
}

#[get("/temp-results/{id}/lines")]
pub(crate) async fn get_temp_result_lines(
    id: web::Path<String>,
    query: web::Query<LinesQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    cleanup_expired(&state).await?;
    let result = load_and_renew(&state, &id).await?;
    let start = query.start.unwrap_or(0).max(0);
    let limit = query
        .limit
        .unwrap_or(state.limits.api.default_line_page_size)
        .clamp(1, state.limits.api.max_line_page_size);
    let result_path = checked_temp_path(&state, &result.storage_path)?;
    let meta_path = result_path.with_extension("meta");
    let index_path = result_path.with_extension("idx");
    let has_meta = tokio::fs::try_exists(&meta_path)
        .await
        .map_err(AppError::Io)?;
    let has_index = tokio::fs::try_exists(&index_path)
        .await
        .map_err(AppError::Io)?;
    if !has_meta || !has_index {
        return Err(invalid_sidecar(
            "temporary result metadata or index is missing",
        ));
    }
    let lines = read_indexed_lines(
        &result_path,
        &meta_path,
        &index_path,
        start,
        limit,
        result.line_count,
    )
    .await?;
    let next_start = if start
        .checked_add(lines.len() as i64)
        .is_some_and(|end| end < result.line_count)
    {
        Some(checked_page_end(start, limit)?)
    } else {
        None
    };
    Ok(HttpResponse::Ok().json(TempResultLines {
        start,
        limit,
        line_count: result.line_count,
        next_start,
        lines,
    }))
}

#[get("/temp-results/{id}/download")]
pub(crate) async fn download_temp_result(
    _user: RequireUser,
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<NamedFile, AppError> {
    cleanup_expired(&state).await?;
    let result = load_and_renew(&state, &id).await?;
    let file = NamedFile::open_async(checked_temp_path(&state, &result.storage_path)?)
        .await
        .map_err(AppError::Io)?
        .set_content_disposition(header::ContentDisposition {
            disposition: header::DispositionType::Attachment,
            parameters: vec![header::DispositionParam::Filename(result.name)],
        });
    Ok(file)
}

#[delete("/temp-results/{id}")]
pub(crate) async fn delete_temp_result(
    _user: RequireBusinessUser,
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let result = load_record(&state, &id).await?;
    let path = checked_temp_path(&state, &result.storage_path)?;
    remove_result_files(&path).await?;
    repository::delete_temp_result_record(&state, &result.id).await?;
    Ok(HttpResponse::NoContent().finish())
}
