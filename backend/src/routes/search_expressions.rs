use actix_web::{HttpResponse, http::StatusCode, post, web};
use serde::{Deserialize, Serialize};

use crate::{auth::extractor::OptionalUser, error::AppError, log_expression};

#[derive(Deserialize)]
pub struct ValidateSearchExpressionRequest {
    expression: String,
}

#[derive(Serialize)]
struct ValidateSearchExpressionResponse {
    valid: bool,
}

fn invalid_expression(expression: &str, error: log_expression::ParseError) -> AppError {
    AppError::public(
        StatusCode::BAD_REQUEST,
        "SEARCH_EXPRESSION_INVALID",
        log_expression::parse_error_message(expression, &error),
    )
}

#[post("/search/validate-expression")]
pub async fn validate_expression(
    _user: OptionalUser,
    payload: web::Json<ValidateSearchExpressionRequest>,
) -> Result<HttpResponse, AppError> {
    let expression = payload.expression.trim();
    log_expression::parse(expression).map_err(|error| invalid_expression(expression, error))?;
    Ok(HttpResponse::Ok().json(ValidateSearchExpressionResponse { valid: true }))
}

#[cfg(test)]
mod tests {
    use actix_web::{App, cookie::Cookie, http::StatusCode, test, web};
    use chrono::{Duration, Utc};
    use serde_json::{Value, json};

    use crate::{
        AppState,
        auth::session::{SESSION_COOKIE_NAME, hash_session_token},
        config::AppLimits,
        db,
        repositories::{sessions, users},
    };

    use super::validate_expression;

    #[actix_web::test]
    async fn validates_full_boolean_expressions_and_reports_parser_location() {
        let pool = db::init_pool("sqlite::memory:").expect("pool");
        db::prepare_schema(&pool, true).await.expect("schema");
        let user = match users::create_user(&pool, "Search validator", "hash")
            .await
            .expect("user")
        {
            users::CreateUserOutcome::Created(user) => user,
            users::CreateUserOutcome::DuplicateUsername => panic!("duplicate user"),
        };
        let token = "search-validator-session";
        sessions::create_session(
            &pool,
            &user.id,
            &hash_session_token(token),
            Utc::now() + Duration::hours(1),
            None,
            None,
        )
        .await
        .expect("session");
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(AppState::new(
                    pool,
                    std::path::PathBuf::from("data"),
                    AppLimits::default(),
                )))
                .service(validate_expression),
        )
        .await;

        let guest = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/search/validate-expression")
                .set_json(json!({ "expression": "ping AND (error OR timeout)" }))
                .to_request(),
        )
        .await;
        assert_eq!(guest.status(), StatusCode::OK);
        let guest_body: Value = test::read_body_json(guest).await;
        assert_eq!(guest_body["valid"], true);

        let valid = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/search/validate-expression")
                .cookie(Cookie::new(SESSION_COOKIE_NAME, token))
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
                .cookie(Cookie::new(SESSION_COOKIE_NAME, token))
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
