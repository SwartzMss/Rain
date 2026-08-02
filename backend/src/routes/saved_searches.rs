use actix_web::{HttpResponse, delete, get, http::StatusCode, patch, post, web};

use crate::{
    AppState,
    auth::extractor::RequireBusinessUser,
    error::AppError,
    models::saved_searches::{SavedSearchListQuery, SavedSearchPayload, SavedSearchResponse},
    repositories::saved_searches,
};

fn normalize_and_validate(payload: &SavedSearchPayload) -> Result<SavedSearchPayload, AppError> {
    if payload.name.trim().is_empty()
        || payload.name.chars().count() > 80
        || payload.query_text.trim().is_empty()
        || payload.query_text.chars().count() > 4096
        || !matches!(payload.search_type.as_str(), "FILENAME" | "DETAIL")
        || !payload.options.is_object()
    {
        return Err(AppError::api(
            StatusCode::BAD_REQUEST,
            "SAVED_SEARCH_INVALID",
            "搜索条件无效",
        ));
    }
    if payload.search_type == "DETAIL" && crate::log_expression::parse(&payload.query_text).is_err()
    {
        return Err(AppError::api(
            StatusCode::BAD_REQUEST,
            "SAVED_SEARCH_EXPRESSION_INVALID",
            "详细搜索表达式语法无效",
        ));
    }
    Ok(payload.clone())
}

fn map_database_error(error: AppError) -> AppError {
    if matches!(&error, AppError::Database(sqlx::Error::Database(db)) if db.is_unique_violation()) {
        AppError::api(
            StatusCode::CONFLICT,
            "SAVED_SEARCH_NAME_EXISTS",
            "已存在同名搜索条件",
        )
    } else {
        error
    }
}

#[get("/me/saved-searches")]
pub async fn list(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    _query: web::Query<SavedSearchListQuery>,
) -> Result<HttpResponse, AppError> {
    let items = saved_searches::list(&state.db.pool, &user.0.id).await?;
    Ok(HttpResponse::Ok().json(
        items
            .into_iter()
            .map(SavedSearchResponse::from)
            .collect::<Vec<_>>(),
    ))
}

#[post("/me/saved-searches")]
pub async fn create(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    payload: web::Json<SavedSearchPayload>,
) -> Result<HttpResponse, AppError> {
    let payload = normalize_and_validate(&payload)?;
    let item = saved_searches::create(&state.db.pool, &user.0.id, &payload)
        .await
        .map_err(map_database_error)?;
    Ok(HttpResponse::Created().json(SavedSearchResponse::from(item)))
}

#[patch("/me/saved-searches/{id}")]
pub async fn update(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    id: web::Path<String>,
    payload: web::Json<SavedSearchPayload>,
) -> Result<HttpResponse, AppError> {
    let payload = normalize_and_validate(&payload)?;
    let item = saved_searches::update(&state.db.pool, &user.0.id, &id, &payload)
        .await
        .map_err(map_database_error)?
        .ok_or_else(|| {
            AppError::api(
                StatusCode::NOT_FOUND,
                "SAVED_SEARCH_NOT_FOUND",
                "搜索条件不存在",
            )
        })?;
    Ok(HttpResponse::Ok().json(SavedSearchResponse::from(item)))
}

#[delete("/me/saved-searches/{id}")]
pub async fn delete(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    id: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    if !saved_searches::delete(&state.db.pool, &user.0.id, &id).await? {
        return Err(AppError::api(
            StatusCode::NOT_FOUND,
            "SAVED_SEARCH_NOT_FOUND",
            "搜索条件不存在",
        ));
    }
    Ok(HttpResponse::NoContent().finish())
}

#[post("/me/saved-searches/{id}/use")]
pub async fn mark_used(
    user: RequireBusinessUser,
    state: web::Data<AppState>,
    id: web::Path<String>,
) -> Result<HttpResponse, AppError> {
    if !saved_searches::mark_used(&state.db.pool, &user.0.id, &id).await? {
        return Err(AppError::api(
            StatusCode::NOT_FOUND,
            "SAVED_SEARCH_NOT_FOUND",
            "搜索条件不存在",
        ));
    }
    Ok(HttpResponse::NoContent().finish())
}
