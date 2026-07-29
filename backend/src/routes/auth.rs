use actix_web::{
    HttpRequest, HttpResponse, http::StatusCode, post, get, web,
};
use chrono::{Duration, Utc};

use crate::{
    AppState,
    auth::{
        extractor::OptionalUser,
        password::{
            PasswordError, hash_password, normalize_username, validate_password,
            validate_username, verify_password,
        },
        session::{
            SESSION_COOKIE_NAME, cleared_session_cookie, generate_session_token,
            hash_session_token, session_cookie,
        },
    },
    error::AppError,
    models::auth::{AuthMeResponse, CredentialsRequest, PublicUser},
    repositories::{
        sessions,
        users::{self, CreateUserOutcome},
    },
};

fn validation_error(error: PasswordError) -> AppError {
    match error {
        PasswordError::InvalidUsername => AppError::api(
            StatusCode::BAD_REQUEST,
            "USERNAME_INVALID",
            "用户名格式无效",
        ),
        PasswordError::InvalidPassword => AppError::api(
            StatusCode::BAD_REQUEST,
            "PASSWORD_TOO_WEAK",
            "密码长度必须为 8 到 128 个字符",
        ),
        PasswordError::Hashing => internal_auth_error(),
    }
}

fn internal_auth_error() -> AppError {
    AppError::api(
        StatusCode::INTERNAL_SERVER_ERROR,
        "AUTHENTICATION_FAILED",
        "认证服务暂时不可用",
    )
}

fn invalid_credentials() -> AppError {
    AppError::api(
        StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
        "用户名或密码错误",
    )
}

#[post("/auth/register")]
pub async fn register_user(
    state: web::Data<AppState>,
    payload: web::Json<CredentialsRequest>,
) -> Result<HttpResponse, AppError> {
    validate_username(&payload.username).map_err(validation_error)?;
    validate_password(&payload.password).map_err(validation_error)?;
    let password = payload.password.clone();
    let password_hash = web::block(move || hash_password(&password))
        .await
        .map_err(|_| internal_auth_error())?
        .map_err(validation_error)?;

    match users::create_user(&state.pool, &payload.username, &password_hash).await? {
        CreateUserOutcome::Created(user) => Ok(HttpResponse::Created().json(PublicUser {
            id: user.id,
            username: user.username,
        })),
        CreateUserOutcome::DuplicateUsername => Err(AppError::api(
            StatusCode::CONFLICT,
            "USERNAME_ALREADY_EXISTS",
            "用户名已存在",
        )),
    }
}

#[post("/auth/login")]
pub async fn login(
    request: HttpRequest,
    state: web::Data<AppState>,
    payload: web::Json<CredentialsRequest>,
) -> Result<HttpResponse, AppError> {
    let normalized = normalize_username(&payload.username);
    let Some(user) = users::find_by_normalized_username(&state.pool, &normalized).await? else {
        return Err(invalid_credentials());
    };
    if user.status != "ACTIVE" {
        return Err(invalid_credentials());
    }

    let password = payload.password.clone();
    let password_hash = user.password_hash.clone();
    let verified = web::block(move || verify_password(&password, &password_hash))
        .await
        .map_err(|_| internal_auth_error())?
        .map_err(|_| invalid_credentials())?;
    if !verified {
        return Err(invalid_credentials());
    }

    let token = generate_session_token();
    let token_hash = hash_session_token(&token);
    let ttl = state.auth.session_ttl_seconds;
    let ttl_i64 = i64::try_from(ttl).unwrap_or(i64::MAX);
    let user_agent = request
        .headers()
        .get("user-agent")
        .and_then(|value| value.to_str().ok());
    let connection_info = request.connection_info();
    let client_ip = connection_info.realip_remote_addr();
    sessions::create_session(
        &state.pool,
        &user.id,
        &token_hash,
        Utc::now() + Duration::seconds(ttl_i64),
        user_agent,
        client_ip,
    )
    .await?;
    users::mark_login(&state.pool, &user.id).await?;

    Ok(HttpResponse::Ok()
        .cookie(session_cookie(
            token,
            ttl,
            state.auth.session_cookie_secure,
        ))
        .json(PublicUser {
            id: user.id,
            username: user.username,
        }))
}

#[get("/auth/me")]
pub async fn me(user: OptionalUser) -> HttpResponse {
    HttpResponse::Ok().json(AuthMeResponse {
        authenticated: user.0.is_some(),
        user: user.0.map(PublicUser::from),
    })
}

#[post("/auth/logout")]
pub async fn logout(
    request: HttpRequest,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    if let Some(cookie) = request.cookie(SESSION_COOKIE_NAME) {
        sessions::revoke_by_token_hash(&state.pool, &hash_session_token(cookie.value())).await?;
    }
    Ok(HttpResponse::NoContent()
        .cookie(cleared_session_cookie(
            state.auth.session_cookie_secure,
        ))
        .finish())
}
