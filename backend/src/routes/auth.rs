use actix_web::{HttpRequest, HttpResponse, get, http::StatusCode, post, web};
use chrono::{Duration, Utc};
use std::time::{Duration as StdDuration, Instant};

use crate::{
    AppState, AuthRateLimitBucket, AuthRateLimits,
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

fn current_password_invalid() -> AppError {
    AppError::api(
        StatusCode::UNAUTHORIZED,
        "CURRENT_PASSWORD_INVALID",
        "当前密码错误",
    )
}

fn rate_limited() -> AppError {
    AppError::api(
        StatusCode::TOO_MANY_REQUESTS,
        "TOO_MANY_REQUESTS",
        "认证请求过于频繁，请稍后再试",
    )
}

const LOGIN_IP_WINDOW: StdDuration = StdDuration::from_secs(60);
const LOGIN_USERNAME_FAILURE_WINDOW: StdDuration = StdDuration::from_secs(5 * 60);
const REGISTER_IP_WINDOW: StdDuration = StdDuration::from_secs(60 * 60);
const CHANGE_PASSWORD_ATTEMPT_WINDOW: StdDuration = StdDuration::from_secs(15 * 60);
const CHANGE_PASSWORD_ATTEMPT_LIMIT: usize = 5;
const LOGIN_IP_MAX_BUCKETS: usize = 1024;
const LOGIN_USERNAME_FAILURE_MAX_BUCKETS: usize = 1024;
const REGISTER_IP_MAX_BUCKETS: usize = 1024;
const CHANGE_PASSWORD_ATTEMPT_MAX_BUCKETS: usize = 1024;

#[derive(Clone, Copy)]
enum AuthRateLimitPolicy {
    LoginIp,
    LoginUsernameFailure,
    RegisterIp,
    ChangePasswordUserAttempt,
}

fn client_rate_limit_key(request: &HttpRequest, action: &str) -> String {
    let client = request
        .peer_addr()
        .map(|address| address.ip().to_string())
        .unwrap_or_else(|| "unknown".into());
    format!("{action}:ip:{client}")
}

fn username_failure_key(username: &str) -> String {
    let normalized = if username.len() <= 64 {
        normalize_username(username)
    } else {
        "<invalid>".into()
    };
    format!("login:username:{normalized}")
}

fn password_change_attempt_key(user_id: &str) -> String {
    format!("change-password:user:{user_id}")
}

fn check_rate_limit(
    state: &AppState,
    policy: AuthRateLimitPolicy,
    key: &str,
    limit: usize,
    window: StdDuration,
    record: bool,
) -> Result<(), AppError> {
    check_rate_limit_at(state, policy, key, limit, window, record, Instant::now())
}

fn check_rate_limit_at(
    state: &AppState,
    policy: AuthRateLimitPolicy,
    key: &str,
    limit: usize,
    window: StdDuration,
    record: bool,
    now: Instant,
) -> Result<(), AppError> {
    let mut rate_limits = state
        .auth_rate_limits
        .lock()
        .map_err(|_| internal_auth_error())?;
    let (buckets, max_buckets) = match policy {
        AuthRateLimitPolicy::LoginIp => (&mut rate_limits.login_ip, LOGIN_IP_MAX_BUCKETS),
        AuthRateLimitPolicy::LoginUsernameFailure => (
            &mut rate_limits.login_username_failure,
            LOGIN_USERNAME_FAILURE_MAX_BUCKETS,
        ),
        AuthRateLimitPolicy::RegisterIp => (&mut rate_limits.register_ip, REGISTER_IP_MAX_BUCKETS),
        AuthRateLimitPolicy::ChangePasswordUserAttempt => (
            &mut rate_limits.change_password_user_attempt,
            CHANGE_PASSWORD_ATTEMPT_MAX_BUCKETS,
        ),
    };

    buckets.retain(|_, bucket| {
        bucket.prune(now);
        !bucket.is_empty()
    });

    if let Some(bucket) = buckets.get_mut(key) {
        bucket.set_window(window);
        bucket.prune(now);
        if bucket.len() >= limit {
            return Err(rate_limited());
        }
    }

    if !record {
        return Ok(());
    }

    if !buckets.contains_key(key) && buckets.len() >= max_buckets {
        return Err(rate_limited());
    }

    buckets
        .entry(key.to_owned())
        .or_insert_with(|| AuthRateLimitBucket::new(window))
        .push(now);
    Ok(())
}

struct PasswordChangeGuard {
    rate_limits: std::sync::Arc<std::sync::Mutex<AuthRateLimits>>,
    user_id: String,
}

impl Drop for PasswordChangeGuard {
    fn drop(&mut self) {
        if let Ok(mut rate_limits) = self.rate_limits.lock() {
            rate_limits.change_password_in_flight.remove(&self.user_id);
        }
    }
}

fn acquire_password_change_guard(
    state: &AppState,
    user_id: &str,
) -> Result<PasswordChangeGuard, AppError> {
    let mut rate_limits = state
        .auth_rate_limits
        .lock()
        .map_err(|_| internal_auth_error())?;
    if !rate_limits
        .change_password_in_flight
        .insert(user_id.to_owned())
    {
        return Err(rate_limited());
    }
    drop(rate_limits);
    Ok(PasswordChangeGuard {
        rate_limits: state.auth_rate_limits.clone(),
        user_id: user_id.to_owned(),
    })
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
    let _ = run_argon2(state, move || verify_dummy_password(&password)).await?;
    Ok(())
}

fn dummy_password_for_credentials(password: &str, credentials_valid: bool) -> String {
    if credentials_valid {
        password.to_owned()
    } else {
        "invalid-password".to_owned()
    }
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
    check_rate_limit(
        &state,
        AuthRateLimitPolicy::RegisterIp,
        &client_rate_limit_key(&request, "register"),
        state.auth.register_ip_limit_per_hour,
        REGISTER_IP_WINDOW,
        true,
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
    let username_key = username_failure_key(&payload.username);
    check_rate_limit(
        &state,
        AuthRateLimitPolicy::LoginIp,
        &client_rate_limit_key(&request, "login"),
        state.auth.login_ip_limit_per_minute,
        LOGIN_IP_WINDOW,
        true,
    )?;
    check_rate_limit(
        &state,
        AuthRateLimitPolicy::LoginUsernameFailure,
        &username_key,
        state.auth.login_username_failure_limit_per_5_minutes,
        LOGIN_USERNAME_FAILURE_WINDOW,
        false,
    )?;
    let credentials_valid = validate_username(&payload.username).is_ok()
        && validate_password(&payload.password).is_ok();
    if !credentials_valid {
        burn_dummy_argon2(
            &state,
            dummy_password_for_credentials(&payload.password, credentials_valid),
        )
        .await?;
        check_rate_limit(
            &state,
            AuthRateLimitPolicy::LoginUsernameFailure,
            &username_key,
            state.auth.login_username_failure_limit_per_5_minutes,
            LOGIN_USERNAME_FAILURE_WINDOW,
            true,
        )?;
        return Err(invalid_credentials());
    }
    let normalized = normalize_username(&payload.username);
    let Some(user) = users::find_by_normalized_username(&state.pool, &normalized).await? else {
        burn_dummy_argon2(
            &state,
            dummy_password_for_credentials(&payload.password, credentials_valid),
        )
        .await?;
        check_rate_limit(
            &state,
            AuthRateLimitPolicy::LoginUsernameFailure,
            &username_key,
            state.auth.login_username_failure_limit_per_5_minutes,
            LOGIN_USERNAME_FAILURE_WINDOW,
            true,
        )?;
        return Err(invalid_credentials());
    };
    if user.status != "ACTIVE" {
        burn_dummy_argon2(
            &state,
            dummy_password_for_credentials(&payload.password, credentials_valid),
        )
        .await?;
        check_rate_limit(
            &state,
            AuthRateLimitPolicy::LoginUsernameFailure,
            &username_key,
            state.auth.login_username_failure_limit_per_5_minutes,
            LOGIN_USERNAME_FAILURE_WINDOW,
            true,
        )?;
        return Err(invalid_credentials());
    }

    let password = payload.password.clone();
    let password_hash = user.password_hash.clone();
    let verified = run_argon2(&state, move || verify_password(&password, &password_hash)).await?;
    if !verified {
        check_rate_limit(
            &state,
            AuthRateLimitPolicy::LoginUsernameFailure,
            &username_key,
            state.auth.login_username_failure_limit_per_5_minutes,
            LOGIN_USERNAME_FAILURE_WINDOW,
            true,
        )?;
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

    Ok(HttpResponse::Ok()
        .cookie(session_cookie(token, ttl))
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
        .cookie(cleared_session_cookie())
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
    let _guard = acquire_password_change_guard(&state, &user.0.id)?;
    let attempt_key = password_change_attempt_key(&user.0.id);
    check_rate_limit(
        &state,
        AuthRateLimitPolicy::ChangePasswordUserAttempt,
        &attempt_key,
        CHANGE_PASSWORD_ATTEMPT_LIMIT,
        CHANGE_PASSWORD_ATTEMPT_WINDOW,
        true,
    )?;
    if validate_password(&payload.current_password).is_err() {
        return Err(current_password_invalid());
    }
    let record = users::find_by_id(&state.pool, &user.0.id)
        .await?
        .ok_or_else(internal_auth_error)?;
    let current = payload.current_password.clone();
    let current_hash = record.password_hash;
    let expected_password_hash = current_hash.clone();
    let verified = run_argon2(&state, move || verify_password(&current, &current_hash)).await?;
    if !verified {
        return Err(current_password_invalid());
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
    let current_token_hash = request
        .cookie(SESSION_COOKIE_NAME)
        .map(|cookie| hash_session_token(cookie.value()))
        .ok_or_else(AppError::authentication_required)?;
    let changed = sessions::change_password_and_replace_sessions(
        &state.pool,
        &user.0.id,
        &expected_password_hash,
        &current_token_hash,
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
        .cookie(session_cookie(token, ttl))
        .finish())
}

#[post("/auth/logout-all")]
pub async fn logout_all(
    user: RequireUser,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    sessions::revoke_all_for_user(&state.pool, &user.0.id).await?;
    Ok(HttpResponse::NoContent()
        .cookie(cleared_session_cookie())
        .finish())
}

#[cfg(test)]
mod tests {
    use std::{
        net::{IpAddr, Ipv4Addr, SocketAddr},
        path::PathBuf,
        time::{Duration as StdDuration, Instant},
    };

    use actix_web::test as actix_test;
    use sqlx::sqlite::SqlitePoolOptions;

    use crate::{AppState, config::AppLimits};

    use super::{
        AuthRateLimitPolicy, CHANGE_PASSWORD_ATTEMPT_LIMIT, CHANGE_PASSWORD_ATTEMPT_MAX_BUCKETS,
        CHANGE_PASSWORD_ATTEMPT_WINDOW, LOGIN_USERNAME_FAILURE_MAX_BUCKETS,
        acquire_password_change_guard, check_rate_limit_at, client_rate_limit_key,
        dummy_password_for_credentials, password_change_attempt_key, username_failure_key,
    };

    #[test]
    fn rate_limit_keys_use_socket_peer_and_bound_invalid_usernames() {
        let request = actix_test::TestRequest::default()
            .peer_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1234))
            .insert_header(("x-forwarded-for", "203.0.113.10"))
            .to_http_request();
        assert_eq!(
            client_rate_limit_key(&request, "login"),
            "login:ip:127.0.0.1"
        );
        assert_eq!(
            username_failure_key(&"x".repeat(10_000)),
            "login:username:<invalid>"
        );
    }

    #[test]
    fn invalid_credentials_use_a_bounded_dummy_password() {
        let untrusted = "x".repeat(100_000);
        let dummy = dummy_password_for_credentials(&untrusted, false);
        assert_ne!(dummy, untrusted);
        assert!(dummy.len() <= 128);
        assert_eq!(
            dummy_password_for_credentials("valid-password", true),
            "valid-password"
        );
    }

    #[tokio::test]
    async fn rate_limit_buckets_have_independent_windows_and_check_modes() {
        let pool = SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .expect("pool");
        let state = AppState::new(pool, PathBuf::from("data"), AppLimits::default());
        let start = Instant::now();

        for _ in 0..2 {
            check_rate_limit_at(
                &state,
                AuthRateLimitPolicy::LoginIp,
                "login:ip:127.0.0.1",
                2,
                StdDuration::from_secs(60),
                true,
                start,
            )
            .expect("login IP attempt");
        }
        assert!(
            check_rate_limit_at(
                &state,
                AuthRateLimitPolicy::LoginIp,
                "login:ip:127.0.0.1",
                2,
                StdDuration::from_secs(60),
                true,
                start,
            )
            .is_err()
        );
        check_rate_limit_at(
            &state,
            AuthRateLimitPolicy::LoginIp,
            "login:ip:127.0.0.1",
            2,
            StdDuration::from_secs(60),
            true,
            start + StdDuration::from_secs(60),
        )
        .expect("login IP window expired");

        for _ in 0..20 {
            check_rate_limit_at(
                &state,
                AuthRateLimitPolicy::LoginUsernameFailure,
                "login:username:swartz",
                2,
                StdDuration::from_secs(300),
                false,
                start,
            )
            .expect("successful login does not record username failure");
        }
        for _ in 0..2 {
            check_rate_limit_at(
                &state,
                AuthRateLimitPolicy::LoginUsernameFailure,
                "login:username:swartz",
                2,
                StdDuration::from_secs(300),
                true,
                start,
            )
            .expect("username failure");
        }
        assert!(
            check_rate_limit_at(
                &state,
                AuthRateLimitPolicy::LoginUsernameFailure,
                "login:username:swartz",
                2,
                StdDuration::from_secs(300),
                false,
                start,
            )
            .is_err()
        );
        check_rate_limit_at(
            &state,
            AuthRateLimitPolicy::LoginUsernameFailure,
            "login:username:swartz",
            2,
            StdDuration::from_secs(300),
            false,
            start + StdDuration::from_secs(300),
        )
        .expect("username failure window expired");

        check_rate_limit_at(
            &state,
            AuthRateLimitPolicy::RegisterIp,
            "register:ip:127.0.0.1",
            1,
            StdDuration::from_secs(3600),
            true,
            start,
        )
        .expect("registration attempt");
        assert!(
            check_rate_limit_at(
                &state,
                AuthRateLimitPolicy::RegisterIp,
                "register:ip:127.0.0.1",
                1,
                StdDuration::from_secs(3600),
                true,
                start,
            )
            .is_err()
        );
        check_rate_limit_at(
            &state,
            AuthRateLimitPolicy::RegisterIp,
            "register:ip:127.0.0.1",
            1,
            StdDuration::from_secs(3600),
            true,
            start + StdDuration::from_secs(3600),
        )
        .expect("registration window expired");
    }

    #[tokio::test]
    async fn exhausted_username_buckets_do_not_block_ip_policies() {
        let pool = SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .expect("pool");
        let state = AppState::new(pool, PathBuf::from("data"), AppLimits::default());
        let start = Instant::now();

        for index in 0..LOGIN_USERNAME_FAILURE_MAX_BUCKETS {
            check_rate_limit_at(
                &state,
                AuthRateLimitPolicy::LoginUsernameFailure,
                &format!("login:username:user-{index}"),
                10,
                StdDuration::from_secs(300),
                true,
                start,
            )
            .expect("active bucket");
        }

        assert!(
            check_rate_limit_at(
                &state,
                AuthRateLimitPolicy::LoginUsernameFailure,
                "login:username:new-user",
                10,
                StdDuration::from_secs(300),
                true,
                start,
            )
            .is_err()
        );
        check_rate_limit_at(
            &state,
            AuthRateLimitPolicy::LoginIp,
            "login:ip:198.51.100.10",
            20,
            StdDuration::from_secs(60),
            true,
            start,
        )
        .expect("username bucket capacity must not block a new login IP");
        check_rate_limit_at(
            &state,
            AuthRateLimitPolicy::RegisterIp,
            "register:ip:198.51.100.10",
            10,
            StdDuration::from_secs(3600),
            true,
            start,
        )
        .expect("username bucket capacity must not block a new registration IP");
        assert!(
            state
                .auth_rate_limits
                .lock()
                .expect("rate limits")
                .login_username_failure
                .contains_key("login:username:user-0")
        );
    }

    #[tokio::test]
    async fn password_change_attempt_policy_is_bounded_expiring_and_isolated() {
        let pool = SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .expect("pool");
        let state = AppState::new(pool, PathBuf::from("data"), AppLimits::default());
        let start = Instant::now();
        let key = password_change_attempt_key("user-1");

        for _ in 0..CHANGE_PASSWORD_ATTEMPT_LIMIT {
            check_rate_limit_at(
                &state,
                AuthRateLimitPolicy::ChangePasswordUserAttempt,
                &key,
                CHANGE_PASSWORD_ATTEMPT_LIMIT,
                CHANGE_PASSWORD_ATTEMPT_WINDOW,
                true,
                start,
            )
            .expect("password-change attempt");
        }
        assert!(
            check_rate_limit_at(
                &state,
                AuthRateLimitPolicy::ChangePasswordUserAttempt,
                &key,
                CHANGE_PASSWORD_ATTEMPT_LIMIT,
                CHANGE_PASSWORD_ATTEMPT_WINDOW,
                false,
                start,
            )
            .is_err()
        );
        check_rate_limit_at(
            &state,
            AuthRateLimitPolicy::ChangePasswordUserAttempt,
            &key,
            CHANGE_PASSWORD_ATTEMPT_LIMIT,
            CHANGE_PASSWORD_ATTEMPT_WINDOW,
            false,
            start + CHANGE_PASSWORD_ATTEMPT_WINDOW,
        )
        .expect("password-change attempt window expired");

        check_rate_limit_at(
            &state,
            AuthRateLimitPolicy::ChangePasswordUserAttempt,
            &key,
            CHANGE_PASSWORD_ATTEMPT_LIMIT,
            CHANGE_PASSWORD_ATTEMPT_WINDOW,
            true,
            start + CHANGE_PASSWORD_ATTEMPT_WINDOW,
        )
        .expect("successful request remains recorded");
        assert!(
            state
                .auth_rate_limits
                .lock()
                .expect("rate limits")
                .change_password_user_attempt
                .contains_key(&key)
        );

        for index in 0..CHANGE_PASSWORD_ATTEMPT_MAX_BUCKETS {
            check_rate_limit_at(
                &state,
                AuthRateLimitPolicy::ChangePasswordUserAttempt,
                &password_change_attempt_key(&format!("user-{index}")),
                CHANGE_PASSWORD_ATTEMPT_LIMIT,
                CHANGE_PASSWORD_ATTEMPT_WINDOW,
                true,
                start,
            )
            .expect("independent password-change bucket");
        }
        check_rate_limit_at(
            &state,
            AuthRateLimitPolicy::LoginIp,
            "login:ip:203.0.113.1",
            20,
            StdDuration::from_secs(60),
            true,
            start,
        )
        .expect("password-change capacity must not block login IP");
        check_rate_limit_at(
            &state,
            AuthRateLimitPolicy::RegisterIp,
            "register:ip:203.0.113.1",
            10,
            StdDuration::from_secs(3600),
            true,
            start,
        )
        .expect("password-change capacity must not block registration IP");
    }

    #[tokio::test]
    async fn password_change_guard_allows_only_one_in_flight_request_per_user() {
        let pool = SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .expect("pool");
        let state = AppState::new(pool, PathBuf::from("data"), AppLimits::default());

        let guard = acquire_password_change_guard(&state, "user-1").expect("first request");
        assert!(acquire_password_change_guard(&state, "user-1").is_err());
        acquire_password_change_guard(&state, "user-2").expect("different user");
        drop(guard);
        acquire_password_change_guard(&state, "user-1").expect("guard released");
    }

    #[tokio::test]
    async fn bucket_cleanup_uses_each_policy_window() {
        let pool = SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .expect("pool");
        let state = AppState::new(pool, PathBuf::from("data"), AppLimits::default());
        let start = Instant::now();

        check_rate_limit_at(
            &state,
            AuthRateLimitPolicy::LoginIp,
            "login:ip:short",
            20,
            StdDuration::from_secs(60),
            true,
            start,
        )
        .expect("short bucket");
        check_rate_limit_at(
            &state,
            AuthRateLimitPolicy::RegisterIp,
            "register:ip:long",
            10,
            StdDuration::from_secs(3600),
            true,
            start,
        )
        .expect("long bucket");
        check_rate_limit_at(
            &state,
            AuthRateLimitPolicy::LoginIp,
            "login:ip:trigger",
            20,
            StdDuration::from_secs(60),
            false,
            start + StdDuration::from_secs(60),
        )
        .expect("cleanup trigger");

        let buckets = state.auth_rate_limits.lock().expect("rate limits");
        assert!(!buckets.login_ip.contains_key("login:ip:short"));
        assert!(buckets.register_ip.contains_key("register:ip:long"));
    }
}
