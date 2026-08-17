use super::service;
use super::*;

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
    _user: RequireUser,
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
    service::get_result(id, state).await
}

#[get("/temp-results/{id}/lines")]
pub(crate) async fn get_temp_result_lines(
    request: HttpRequest,
    id: web::Path<String>,
    query: web::Query<LinesQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    service::get_result_lines(request, id, query, state).await
}

#[get("/temp-results/{id}/download")]
pub(crate) async fn download_temp_result(
    user: RequireUser,
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<NamedFile, AppError> {
    service::open_result_download(user, id, state).await
}

#[delete("/temp-results/{id}")]
pub(crate) async fn delete_temp_result(
    user: RequireBusinessUser,
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    service::delete_result(user, id, state).await
}
