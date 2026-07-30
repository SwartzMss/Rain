use std::future::{Ready, ready};

use actix_web::{
    Error, FromRequest, HttpMessage, HttpRequest,
    body::MessageBody,
    dev::{Payload, ServiceRequest, ServiceResponse},
    http::StatusCode,
    middleware::Next,
    web,
};
use futures_util::future::LocalBoxFuture;

use crate::{
    AppState,
    auth::{
        AuthenticatedUser,
        session::{SESSION_COOKIE_NAME, hash_session_token},
    },
    error::AppError,
    repositories::sessions,
};

pub struct OptionalUser(pub Option<AuthenticatedUser>);
#[derive(Clone, Copy)]
pub struct InvalidSessionCookie;

pub async fn clear_invalid_session_cookie(
    request: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<impl MessageBody>, Error> {
    let mut response = next.call(request).await?;
    if response
        .request()
        .extensions()
        .get::<InvalidSessionCookie>()
        .is_some()
    {
        response
            .response_mut()
            .add_cookie(&crate::auth::session::cleared_session_cookie())?;
    }
    Ok(response)
}

impl FromRequest for OptionalUser {
    type Error = AppError;
    type Future = LocalBoxFuture<'static, Result<Self, Self::Error>>;

    fn from_request(request: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        let state = request.app_data::<web::Data<AppState>>().cloned();
        let request = request.clone();
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
            if user.is_none() {
                request.extensions_mut().insert(InvalidSessionCookie);
            }
            Ok(Self(user))
        })
    }
}

pub struct RequireUser(pub AuthenticatedUser);

impl FromRequest for RequireUser {
    type Error = AppError;
    type Future = LocalBoxFuture<'static, Result<Self, Self::Error>>;

    fn from_request(request: &HttpRequest, payload: &mut Payload) -> Self::Future {
        let state = request.app_data::<web::Data<AppState>>().cloned();
        let request = request.clone();
        let token = request
            .cookie(SESSION_COOKIE_NAME)
            .map(|cookie| cookie.value().to_owned());
        let _ = payload;
        Box::pin(async move {
            let Some(state) = state else {
                return Err(AppError::Config("missing application state".into()));
            };
            let Some(token) = token.filter(|value| !value.is_empty()) else {
                return Err(AppError::authentication_required());
            };
            let Some(resolved) =
                sessions::resolve_session_user(&state.pool, &hash_session_token(&token)).await?
            else {
                request.extensions_mut().insert(InvalidSessionCookie);
                return Err(AppError::authentication_required());
            };
            if resolved.status != "ACTIVE" {
                return Err(AppError::api(
                    StatusCode::FORBIDDEN,
                    "ACCOUNT_DISABLED",
                    "账户已停用",
                ));
            }
            Ok(Self(resolved.user))
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

    use actix_web::{App, HttpResponse, cookie::Cookie, http::StatusCode, test, web};
    use chrono::{Duration, Utc};

    use crate::{
        AppState,
        auth::session::{SESSION_COOKIE_NAME, hash_session_token},
        config::AppLimits,
        db,
        repositories::{sessions, users},
    };

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

    #[actix_web::test]
    async fn disabled_session_is_forbidden_when_user_is_required() {
        let pool = db::init_pool("sqlite::memory:").expect("pool");
        db::prepare_schema(&pool, true).await.expect("schema");
        let user = match users::create_user(&pool, "Disabled", "hash")
            .await
            .expect("user")
        {
            users::CreateUserOutcome::Created(user) => user,
            users::CreateUserOutcome::DuplicateUsername => panic!("unexpected duplicate"),
        };
        let token = "disabled-session-token";
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
        sqlx::query("UPDATE users SET status = 'DISABLED' WHERE id = ?")
            .bind(&user.id)
            .execute(&pool)
            .await
            .expect("disable user");

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(AppState::new(
                    pool,
                    PathBuf::from("data"),
                    AppLimits::default(),
                )))
                .route("/required", web::get().to(required)),
        )
        .await;
        let response = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/required")
                .cookie(Cookie::new(SESSION_COOKIE_NAME, token))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
