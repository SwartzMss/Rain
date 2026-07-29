use actix_web::{HttpRequest, HttpResponse, get, http::StatusCode, post, web};
use chrono::{Duration, Utc};
use std::time::{Duration as StdDuration, Instant};

use crate::{
    AppState,
    auth::{
        extractor::{OptionalUser, RequireUser},
        password::{
            PasswordError, hash_password, normalize_username, validate_password, validate_username,
            verify_dummy_password, verify_password,
        },
        session::{
            SESSION_COOKIE_NAME, cleared_session_cookie, generate_session_token,
            hash_session_token, session_cookie,
        },
    },
    error::AppError,
    models::auth::{AuthMeResponse, ChangePasswordRequest, CredentialsRequest, PublicUser},
    repositories::{
        sessions::{self, ReplacementSession},
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

fn rate_limited() -> AppError {
    AppError::api(
        StatusCode::TOO_MANY_REQUESTS,
        "AUTH_RATE_LIMITED",
        "认证请求过于频繁，请稍后再试",
    )
}

const AUTH_RATE_LIMIT_WINDOW: StdDuration = StdDuration::from_secs(60);
const AUTH_RATE_LIMIT_MAX_BUCKETS: usize = 1024;

fn auth_rate_limit_keys(request: &HttpRequest, action: &str, username: &str) -> [String; 2] {
    let client = request
        .peer_addr()
        .map(|address| address.ip().to_string())
        .unwrap_or_else(|| "unknown".into());
    let normalized = if username.len() <= 64 {
        normalize_username(username)
    } else {
        "<invalid>".into()
    };
    [
        format!("{action}:ip:{client}"),
        format!("{action}:username:{normalized}"),
    ]
}

fn check_auth_rate_limit(
    state: &AppState,
    keys: [String; 2],
    ip_limit: usize,
    username_limit: usize,
) -> Result<(), AppError> {
    let now = Instant::now();
    let mut buckets = state
        .auth_rate_limits
        .lock()
        .map_err(|_| internal_auth_error())?;

    buckets.retain(|_, bucket| {
        while bucket
            .front()
            .is_some_and(|timestamp| now.duration_since(*timestamp) >= AUTH_RATE_LIMIT_WINDOW)
        {
            bucket.pop_front();
        }
        !bucket.is_empty()
    });

    let missing_keys = keys
        .iter()
        .filter(|key| !buckets.contains_key(*key))
        .count();
    while buckets.len() + missing_keys > AUTH_RATE_LIMIT_MAX_BUCKETS {
        let oldest = buckets
            .iter()
            .filter(|(key, _)| !keys.contains(key))
            .min_by_key(|(_, bucket)| bucket.front().copied())
            .map(|(key, _)| key.clone());
        if let Some(key) = oldest {
            buckets.remove(&key);
        } else {
            break;
        }
    }

    for (key, limit) in keys.iter().zip([ip_limit, username_limit]) {
        let bucket = buckets.entry(key.clone()).or_default();
        if bucket.len() >= limit {
            return Err(rate_limited());
        }
    }
    for key in keys {
        if let Some(bucket) = buckets.get_mut(&key) {
            bucket.push_back(now);
        }
    }
    Ok(())
}

async fn run_argon2<T, F>(state: &AppState, operation: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, PasswordError> + Send + 'static,
{
    let permit = state
        .auth_hash_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| rate_limited())?;
    web::block(move || {
        let _permit = permit;
        operation()
    })
    .await
    .map_err(|_| internal_auth_error())?
    .map_err(validation_error)
}

async fn burn_dummy_argon2(state: &AppState, password: String) -> Result<(), AppError> {
    let _ = run_argon2(state, move || verify_dummy_password(&password))
        .await
        .map_err(|_| invalid_credentials())?;
    Ok(())
}

#[post("/auth/register")]
pub async fn register_user(
    request: HttpRequest,
    state: web::Data<AppState>,
    payload: web::Json<CredentialsRequest>,
) -> Result<HttpResponse, AppError> {
    if !state.auth.allow_registration {
        return Err(AppError::api(
            StatusCode::FORBIDDEN,
            "REGISTRATION_DISABLED",
            "当前未开放注册",
        ));
    }
    check_auth_rate_limit(
        &state,
        auth_rate_limit_keys(&request, "register", &payload.username),
        state.auth.register_rate_limit_per_minute,
        state.auth.register_rate_limit_per_minute,
    )?;
    validate_username(&payload.username).map_err(validation_error)?;
    validate_password(&payload.password).map_err(validation_error)?;
    let password = payload.password.clone();
    let password_hash = run_argon2(&state, move || hash_password(&password)).await?;

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
    check_auth_rate_limit(
        &state,
        auth_rate_limit_keys(&request, "login", &payload.username),
        state.auth.login_rate_limit_per_minute,
        state.auth.login_rate_limit_per_minute,
    )?;
    if validate_username(&payload.username).is_err()
        || validate_password(&payload.password).is_err()
    {
        burn_dummy_argon2(&state, payload.password.clone()).await?;
        return Err(invalid_credentials());
    }
    let normalized = normalize_username(&payload.username);
    let Some(user) = users::find_by_normalized_username(&state.pool, &normalized).await? else {
        burn_dummy_argon2(&state, payload.password.clone()).await?;
        return Err(invalid_credentials());
    };
    if user.status != "ACTIVE" {
        burn_dummy_argon2(&state, payload.password.clone()).await?;
        return Err(invalid_credentials());
    }

    let password = payload.password.clone();
    let password_hash = user.password_hash.clone();
    let verified = run_argon2(&state, move || verify_password(&password, &password_hash))
        .await
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
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let client_ip = request.peer_addr().map(|address| address.ip().to_string());
    let created = sessions::create_session_if_password_unchanged(
        &state.pool,
        &user.id,
        &user.password_hash,
        &token_hash,
        Utc::now()
            .checked_add_signed(Duration::seconds(ttl_i64))
            .ok_or_else(internal_auth_error)?,
        user_agent.as_deref(),
        client_ip.as_deref(),
    )
    .await?;
    if !created {
        return Err(invalid_credentials());
    }
    users::mark_login(&state.pool, &user.id).await?;

    Ok(HttpResponse::Ok()
        .cookie(session_cookie(token, ttl, state.auth.session_cookie_secure))
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
        .cookie(cleared_session_cookie(state.auth.session_cookie_secure))
        .finish())
}

#[post("/auth/change-password")]
pub async fn change_password(
    request: HttpRequest,
    user: RequireUser,
    state: web::Data<AppState>,
    payload: web::Json<ChangePasswordRequest>,
) -> Result<HttpResponse, AppError> {
    validate_password(&payload.new_password).map_err(validation_error)?;
    let record = users::find_by_id(&state.pool, &user.0.id)
        .await?
        .ok_or_else(internal_auth_error)?;
    let current = payload.current_password.clone();
    let current_hash = record.password_hash;
    let expected_password_hash = current_hash.clone();
    let verified = run_argon2(&state, move || verify_password(&current, &current_hash))
        .await
        .map_err(|_| invalid_credentials())?;
    if !verified {
        return Err(AppError::api(
            StatusCode::UNAUTHORIZED,
            "CURRENT_PASSWORD_INVALID",
            "当前密码错误",
        ));
    }
    let new_password = payload.new_password.clone();
    let new_hash = run_argon2(&state, move || hash_password(&new_password)).await?;
    let token = generate_session_token();
    let token_hash = hash_session_token(&token);
    let ttl = state.auth.session_ttl_seconds;
    let expires_at = Utc::now()
        .checked_add_signed(Duration::seconds(i64::try_from(ttl).unwrap_or(i64::MAX)))
        .ok_or_else(internal_auth_error)?;
    let user_agent = request
        .headers()
        .get("user-agent")
        .and_then(|value| value.to_str().ok());
    let client_ip = request.peer_addr().map(|address| address.ip().to_string());
    let changed = sessions::change_password_and_replace_sessions(
        &state.pool,
        &user.0.id,
        &expected_password_hash,
        &new_hash,
        ReplacementSession {
            token_hash: &token_hash,
            expires_at,
            user_agent,
            client_ip: client_ip.as_deref(),
        },
    )
    .await?;
    if !changed {
        return Err(AppError::api(
            StatusCode::CONFLICT,
            "PASSWORD_CHANGED_CONCURRENTLY",
            "密码已被其他请求修改，请重新登录",
        ));
    }
    Ok(HttpResponse::NoContent()
        .cookie(session_cookie(token, ttl, state.auth.session_cookie_secure))
        .finish())
}

#[post("/auth/logout-all")]
pub async fn logout_all(
    user: RequireUser,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    sessions::revoke_all_for_user(&state.pool, &user.0.id).await?;
    Ok(HttpResponse::NoContent()
        .cookie(cleared_session_cookie(state.auth.session_cookie_secure))
        .finish())
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use actix_web::test as actix_test;

    use super::auth_rate_limit_keys;

    #[test]
    fn rate_limit_uses_socket_peer_and_bounds_invalid_username_keys() {
        let request = actix_test::TestRequest::default()
            .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1234))
            .insert_header(("x-forwarded-for", "203.0.113.10"))
            .to_http_request();
        let keys = auth_rate_limit_keys(&request, "login", &"x".repeat(10_000));
        assert_eq!(keys[0], "login:ip:127.0.0.1");
        assert_eq!(keys[1], "login:username:<invalid>");
        assert!(!keys.iter().any(|key| key.contains("203.0.113.10")));
    }
}
