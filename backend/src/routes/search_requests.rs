use std::time::{Duration, Instant};

use actix_web::{HttpRequest, HttpResponse, delete, http::StatusCode, post, web};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    AppState, AuthRateLimitBucket,
    auth::extractor::OptionalUser,
    error::AppError,
    routes::temp_results::{check_temp_result_rate_limit, request_client_key},
    services::search_execution::{CancelResult, ReservationError, TerminalStatus},
};

pub(crate) const CANCEL_TOKEN_HEADER: &str = "X-Search-Cancel-Token";
const SEARCH_CANCEL_RATE_LIMIT: usize = 120;
const SEARCH_CANCEL_RATE_WINDOW: Duration = Duration::from_secs(60);
const SEARCH_CANCEL_MAX_BUCKETS: usize = 1024;

#[derive(Debug, Deserialize)]
pub(crate) struct ReserveSearchRequest {
    search_id: String,
}

#[derive(Debug, Serialize)]
struct ReserveSearchResponse {
    search_id: String,
    cancel_token: String,
    expires_in_ms: u64,
}

#[derive(Debug, Serialize)]
struct SearchStatusResponse {
    status: &'static str,
}

fn owner_id(user: &OptionalUser) -> Option<&str> {
    user.0.as_ref().map(|user| user.id.as_str())
}

fn parse_search_id(value: &str) -> Result<String, AppError> {
    Uuid::parse_str(value)
        .map(|id| id.to_string())
        .map_err(|_| AppError::BadRequest("search_id 必须是有效 UUID".into()))
}

#[post("/search-requests")]
pub(crate) async fn reserve_search_request(
    user: OptionalUser,
    request: HttpRequest,
    payload: web::Json<ReserveSearchRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    check_temp_result_rate_limit(&state, &request)?;
    let search_id = parse_search_id(&payload.search_id)?;
    let client_key = request_client_key(&request);
    let reservation = state
        .temp_results
        .search_executions
        .reserve(&search_id, owner_id(&user), &client_key)
        .map_err(|error| match error {
            ReservationError::Conflict => AppError::api(
                StatusCode::CONFLICT,
                "SEARCH_REQUEST_CONFLICT",
                "搜索请求已存在",
            ),
            ReservationError::OwnerBusy | ReservationError::Capacity => AppError::api(
                StatusCode::TOO_MANY_REQUESTS,
                "SEARCH_REQUEST_BUSY",
                "搜索请求过多，请稍后重试",
            ),
            ReservationError::InvalidId
            | ReservationError::Unauthorized
            | ReservationError::Unavailable => AppError::api(
                StatusCode::BAD_REQUEST,
                "SEARCH_REQUEST_INVALID",
                "搜索请求无效",
            ),
        })?;
    Ok(HttpResponse::Created()
        .insert_header(("Cache-Control", "no-store, private"))
        .json(ReserveSearchResponse {
            search_id: reservation.search_id,
            cancel_token: reservation.cancel_token,
            expires_in_ms: reservation.expires_in_ms,
        }))
}

#[delete("/search-requests/{search_id}")]
pub(crate) async fn cancel_search_request(
    user: OptionalUser,
    request: HttpRequest,
    search_id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let cancel_token = request
        .headers()
        .get(CANCEL_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let parsed_search_id = Uuid::parse_str(&search_id).map(|id| id.to_string()).ok();
    let owner = owner_id(&user);
    let key = parsed_search_id
        .as_deref()
        .and_then(|search_id| {
            state
                .temp_results
                .search_executions
                .authorized_capability_scope(search_id, cancel_token, owner)
        })
        .map(|scope| format!("cap:{scope}"))
        .unwrap_or_else(|| format!("ip:{}", request_client_key(&request)));
    check_cancel_rate_limit(&state, &key)?;
    let Some(search_id) = parsed_search_id else {
        return Ok(HttpResponse::NoContent().finish());
    };
    if cancel_token.is_empty() {
        return Ok(HttpResponse::NoContent().finish());
    }
    match state
        .temp_results
        .search_executions
        .cancel(&search_id, cancel_token, owner)
    {
        CancelResult::Unknown => Ok(HttpResponse::NoContent().finish()),
        CancelResult::Cancelling => {
            let status = state
                .temp_results
                .search_executions
                .wait_for_terminal(&search_id, std::time::Duration::from_secs(1))
                .await;
            match status {
                Some(status) => terminal_response(status),
                None => Ok(HttpResponse::Accepted().json(SearchStatusResponse {
                    status: "cancelling",
                })),
            }
        }
        CancelResult::Finishing => Ok(HttpResponse::Accepted().json(SearchStatusResponse {
            status: "finishing",
        })),
        CancelResult::Terminal(status) => terminal_response(status),
    }
}

fn check_cancel_rate_limit(state: &web::Data<AppState>, key: &str) -> Result<(), AppError> {
    let now = Instant::now();
    let mut limits = state
        .temp_results
        .search_cancel_limits
        .lock()
        .map_err(|_| {
            AppError::api(
                StatusCode::SERVICE_UNAVAILABLE,
                "RATE_LIMIT_UNAVAILABLE",
                "服务暂时不可用",
            )
        })?;
    limits.retain(|_, bucket| {
        bucket.prune(now);
        !bucket.is_empty()
    });
    if !limits.contains_key(key) && limits.len() >= SEARCH_CANCEL_MAX_BUCKETS {
        return Err(AppError::api(
            StatusCode::TOO_MANY_REQUESTS,
            "SEARCH_CANCEL_RATE_LIMITED",
            "取消请求过于频繁，请稍后重试",
        ));
    }
    let bucket = limits
        .entry(key.to_owned())
        .or_insert_with(|| AuthRateLimitBucket::new(SEARCH_CANCEL_RATE_WINDOW));
    if bucket.len() >= SEARCH_CANCEL_RATE_LIMIT {
        return Err(AppError::api(
            StatusCode::TOO_MANY_REQUESTS,
            "SEARCH_CANCEL_RATE_LIMITED",
            "取消请求过于频繁，请稍后重试",
        ));
    }
    bucket.push(now);
    Ok(())
}

fn terminal_response(status: TerminalStatus) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok().json(SearchStatusResponse {
        status: status.as_str(),
    }))
}
