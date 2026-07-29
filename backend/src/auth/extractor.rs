use std::future::{Ready, ready};

use actix_web::{FromRequest, HttpRequest, dev::Payload, web};
use futures_util::future::LocalBoxFuture;

use crate::{
    AppState,
    auth::{AuthenticatedUser, session::{SESSION_COOKIE_NAME, hash_session_token}},
    error::AppError,
    repositories::sessions,
};

pub struct OptionalUser(pub Option<AuthenticatedUser>);

impl FromRequest for OptionalUser {
    type Error = AppError;
    type Future = LocalBoxFuture<'static, Result<Self, Self::Error>>;

    fn from_request(request: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        let state = request.app_data::<web::Data<AppState>>().cloned();
        let token = request
            .cookie(SESSION_COOKIE_NAME)
            .map(|cookie| cookie.value().to_owned());
        Box::pin(async move {
            let Some(state) = state else {
                return Err(AppError::Config("missing application state".into()));
            };
            let Some(token) = token.filter(|value| !value.is_empty()) else {
                return Ok(Self(None));
            };
            let user =
                sessions::resolve_active_user(&state.pool, &hash_session_token(&token)).await?;
            Ok(Self(user))
        })
    }
}

pub struct RequireUser(pub AuthenticatedUser);

impl FromRequest for RequireUser {
    type Error = AppError;
    type Future = LocalBoxFuture<'static, Result<Self, Self::Error>>;

    fn from_request(request: &HttpRequest, payload: &mut Payload) -> Self::Future {
        let future = OptionalUser::from_request(request, payload);
        Box::pin(async move {
            future
                .await?
                .0
                .map(Self)
                .ok_or_else(AppError::authentication_required)
        })
    }
}

pub struct GuestOnly;

impl FromRequest for GuestOnly {
    type Error = AppError;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(_request: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        ready(Ok(Self))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use actix_web::{App, HttpResponse, http::StatusCode, test, web};

    use crate::{AppState, config::AppLimits, db};

    use super::{OptionalUser, RequireUser};

    async fn optional(user: OptionalUser) -> HttpResponse {
        HttpResponse::Ok().json(serde_json::json!({"authenticated": user.0.is_some()}))
    }

    async fn required(_user: RequireUser) -> HttpResponse {
        HttpResponse::NoContent().finish()
    }

    #[actix_web::test]
    async fn guest_is_optional_but_rejected_when_required() {
        let pool = db::init_pool("sqlite::memory:").expect("pool");
        db::prepare_schema(&pool, true).await.expect("schema");
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(AppState::new(
                    pool,
                    PathBuf::from("data"),
                    AppLimits::default(),
                )))
                .route("/optional", web::get().to(optional))
                .route("/required", web::get().to(required)),
        )
        .await;

        let optional_response =
            test::call_service(&app, test::TestRequest::get().uri("/optional").to_request()).await;
        assert_eq!(optional_response.status(), StatusCode::OK);

        let required_response =
            test::call_service(&app, test::TestRequest::get().uri("/required").to_request()).await;
        assert_eq!(required_response.status(), StatusCode::UNAUTHORIZED);
        let body: serde_json::Value = test::read_body_json(required_response).await;
        assert_eq!(body["code"], "AUTHENTICATION_REQUIRED");
    }
}
