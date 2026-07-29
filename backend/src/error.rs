use actix_web::{HttpResponse, ResponseError, http::StatusCode};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("{message}")]
    Api {
        status: StatusCode,
        code: &'static str,
        message: &'static str,
    },
}

impl AppError {
    pub fn api(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self::Api {
            status,
            code,
            message,
        }
    }

    pub fn authentication_required() -> Self {
        Self::api(
            StatusCode::UNAUTHORIZED,
            "AUTHENTICATION_REQUIRED",
            "此操作需要登录",
        )
    }
}

impl ResponseError for AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            AppError::Config(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::Database(_) => StatusCode::SERVICE_UNAVAILABLE,
            AppError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Api { status, .. } => *status,
        }
    }

    fn error_response(&self) -> HttpResponse {
        match self {
            AppError::Api { code, message, .. } => {
                HttpResponse::build(self.status_code()).json(serde_json::json!({
                    "code": code,
                    "message": message
                }))
            }
            _ => HttpResponse::build(self.status_code()).json(serde_json::json!({
                "error": self.to_string()
            })),
        }
    }
}
