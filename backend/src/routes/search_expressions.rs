use actix_web::{HttpResponse, http::StatusCode, post, web};
use serde::{Deserialize, Serialize};

use crate::{error::AppError, log_expression};

#[derive(Deserialize)]
pub struct ValidateSearchExpressionRequest {
    expression: String,
}

#[derive(Serialize)]
struct ValidateSearchExpressionResponse {
    valid: bool,
}

fn invalid_expression(error: log_expression::ParseError) -> AppError {
    AppError::public(
        StatusCode::BAD_REQUEST,
        "SEARCH_EXPRESSION_INVALID",
        log_expression::parse_error_message(&error),
    )
}

#[post("/search/validate-expression")]
pub async fn validate_expression(
    payload: web::Json<ValidateSearchExpressionRequest>,
) -> Result<HttpResponse, AppError> {
    log_expression::parse(payload.expression.trim()).map_err(invalid_expression)?;
    Ok(HttpResponse::Ok().json(ValidateSearchExpressionResponse { valid: true }))
}

#[cfg(test)]
mod tests {
    use actix_web::{App, http::StatusCode, test};
    use serde_json::{Value, json};

    use super::validate_expression;

    #[actix_web::test]
    async fn validates_full_boolean_expressions_and_reports_parser_location() {
        let app = test::init_service(App::new().service(validate_expression)).await;

        let valid = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/search/validate-expression")
                .set_json(json!({ "expression": "ping AND (error OR timeout) AND NOT retry" }))
                .to_request(),
        )
        .await;
        assert_eq!(valid.status(), StatusCode::OK);
        let valid_body: Value = test::read_body_json(valid).await;
        assert_eq!(valid_body["valid"], true);

        let invalid = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/search/validate-expression")
                .set_json(json!({ "expression": "ping AND (error OR" }))
                .to_request(),
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        let invalid_body: Value = test::read_body_json(invalid).await;
        assert_eq!(invalid_body["code"], "SEARCH_EXPRESSION_INVALID");
        assert!(invalid_body["message"].as_str().unwrap().contains("位置"));
    }
}
