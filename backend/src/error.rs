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
    #[error("{message}")]
    PublicApi {
        status: StatusCode,
        code: &'static str,
        message: String,
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

    pub fn public(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self::PublicApi {
            status,
            code,
            message: message.into(),
        }
    }
}

impl From<crate::skill_schema::SkillFormatError> for AppError {
    fn from(error: crate::skill_schema::SkillFormatError) -> Self {
        Self::public(
            StatusCode::BAD_REQUEST,
            "SKILL_FORMAT_INVALID",
            error.to_string(),
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
            AppError::Api { status, .. } | AppError::PublicApi { status, .. } => *status,
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
            AppError::PublicApi { code, message, .. } => HttpResponse::build(self.status_code())
                .json(serde_json::json!({
                    "code": code,
                    "message": message
                })),
            AppError::Database(_) => {
                tracing::error!(error = %self, "database request failed");
                HttpResponse::build(self.status_code()).json(serde_json::json!({
                    "code": "DATABASE_UNAVAILABLE",
                    "message": "服务暂时不可用"
                }))
            }
            AppError::Config(_) | AppError::Io(_) => {
                tracing::error!(error = %self, "internal request failed");
                HttpResponse::build(self.status_code()).json(serde_json::json!({
                    "code": "INTERNAL_ERROR",
                    "message": "服务暂时不可用"
                }))
            }
            AppError::NotFound(_) => {
                HttpResponse::build(self.status_code()).json(serde_json::json!({
                    "code": "RESOURCE_NOT_FOUND",
                    "message": "资源不存在"
                }))
            }
            AppError::BadRequest(_) => {
                HttpResponse::build(self.status_code()).json(serde_json::json!({
                    "code": "BAD_REQUEST",
                    "message": "请求无效"
                }))
            }
            AppError::Conflict(_) => {
                HttpResponse::build(self.status_code()).json(serde_json::json!({
                    "code": "CONFLICT",
                    "message": "请求冲突"
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use actix_web::{ResponseError, body::to_bytes, http::StatusCode};

    use super::AppError;

    #[actix_web::test]
    async fn internal_and_generic_errors_return_stable_sanitized_payloads() {
        let cases = [
            (
                AppError::Config("secret-config".into()),
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "secret-config",
            ),
            (
                AppError::Database(sqlx::Error::Protocol("secret-table".into())),
                StatusCode::SERVICE_UNAVAILABLE,
                "DATABASE_UNAVAILABLE",
                "secret-table",
            ),
            (
                AppError::Io(io::Error::other("/secret/server/path")),
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "/secret/server/path",
            ),
            (
                AppError::NotFound("secret-resource".into()),
                StatusCode::NOT_FOUND,
                "RESOURCE_NOT_FOUND",
                "secret-resource",
            ),
            (
                AppError::BadRequest("secret-input".into()),
                StatusCode::BAD_REQUEST,
                "BAD_REQUEST",
                "secret-input",
            ),
            (
                AppError::Conflict("secret-conflict".into()),
                StatusCode::CONFLICT,
                "CONFLICT",
                "secret-conflict",
            ),
        ];

        for (error, status, code, secret) in cases {
            let response = error.error_response();
            assert_eq!(response.status(), status);
            let body = to_bytes(response.into_body()).await.expect("response body");
            let payload: serde_json::Value = serde_json::from_slice(&body).expect("JSON response");
            assert_eq!(payload["code"], code);
            assert!(payload["message"].is_string());
            assert!(!String::from_utf8_lossy(&body).contains(secret));
            assert!(payload.get("error").is_none());
        }
    }

    #[actix_web::test]
    async fn explicit_api_errors_keep_their_public_contract() {
        let response =
            AppError::api(StatusCode::IM_A_TEAPOT, "PUBLIC_CODE", "公开文案").error_response();
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
        let body = to_bytes(response.into_body()).await.expect("response body");
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("JSON response");
        assert_eq!(payload["code"], "PUBLIC_CODE");
        assert_eq!(payload["message"], "公开文案");
    }

    #[actix_web::test]
    async fn public_business_errors_keep_controlled_owned_messages() {
        let response = AppError::public(
            StatusCode::BAD_REQUEST,
            "SEARCH_EXPRESSION_INVALID",
            format!("搜索条件无效（位置 {}）", 12),
        )
        .error_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body()).await.expect("response body");
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("JSON response");
        assert_eq!(payload["code"], "SEARCH_EXPRESSION_INVALID");
        assert_eq!(payload["message"], "搜索条件无效（位置 12）");
    }

    #[actix_web::test]
    async fn skill_format_errors_have_a_stable_public_contract() {
        let response = AppError::from(
            crate::skill_schema::SkillFormatError::MissingRequiredSection("关键日志"),
        )
        .error_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body()).await.expect("response body");
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("JSON response");
        assert_eq!(payload["code"], "SKILL_FORMAT_INVALID");
        assert_eq!(payload["message"], "缺少必填章节：关键日志");
    }
}
