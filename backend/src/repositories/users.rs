use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

use crate::{auth::password::normalize_username, error::AppError};

#[derive(Debug, Clone, FromRow)]
pub struct UserRecord {
    pub id: String,
    pub username: String,
    pub username_normalized: String,
    pub password_hash: String,
    pub status: String,
    pub role: String,
    pub password_changed_at: Option<String>,
}

#[derive(Debug)]
pub enum CreateUserOutcome {
    Created(UserRecord),
    DuplicateUsername,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationOutcome {
    Created { user_id: String },
    DuplicateUsername,
    RegistrationDisabled,
    InviteRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationPreflight {
    Ready,
    RegistrationDisabled,
    InviteRequired,
    InviteInvalid,
}

/// Performs inexpensive registration checks before password hashing. This is
/// only a preflight: `register_user` repeats every policy and invite check in
/// its write transaction to remain safe against concurrent changes.
pub async fn registration_preflight(
    pool: &SqlitePool,
    invite_code_hash: Option<&str>,
) -> Result<RegistrationPreflight, AppError> {
    let (allow_registration, requires_invite): (i64, i64) = sqlx::query_as(
        "SELECT allow_registration,registration_requires_invite FROM system_settings WHERE id=1",
    )
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;

    if allow_registration == 0 {
        return Ok(RegistrationPreflight::RegistrationDisabled);
    }
    if requires_invite == 0 {
        return Ok(RegistrationPreflight::Ready);
    }

    let Some(invite_code_hash) = invite_code_hash else {
        return Ok(RegistrationPreflight::InviteRequired);
    };
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM invitations WHERE code_hash=? AND used_at IS NULL AND revoked_at IS NULL AND (expires_at IS NULL OR datetime(expires_at)>CURRENT_TIMESTAMP))",
    )
    .bind(invite_code_hash)
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;
    if !valid {
        return Ok(RegistrationPreflight::InviteInvalid);
    }

    Ok(RegistrationPreflight::Ready)
}

/// Creates the account and redeems its invitation in one replayable SQLite
/// transaction. The persisted setting row is the source of truth for policy.
pub async fn register_user(
    pool: &SqlitePool,
    user_id: &str,
    username: &str,
    password_hash: &str,
    invite_code_hash: Option<&str>,
    client_ip: Option<&str>,
    user_agent: Option<&str>,
) -> Result<RegistrationOutcome, AppError> {
    let user_id = user_id.to_owned();
    let username = username.to_owned();
    let username_normalized = normalize_username(&username);
    let password_hash = password_hash.to_owned();
    let invite_code_hash = invite_code_hash.map(str::to_owned);
    let client_ip = client_ip.map(str::to_owned);
    let user_agent = user_agent.map(str::to_owned);
    let operation_id = Uuid::new_v4().to_string();
    let audit_id = Uuid::new_v4().to_string();

    crate::db::write::transaction(pool, "register user", move |conn| {
        let user_id = user_id.clone();
        let username = username.clone();
        let username_normalized = username_normalized.clone();
        let password_hash = password_hash.clone();
        let invite_code_hash = invite_code_hash.clone();
        let client_ip = client_ip.clone();
        let user_agent = user_agent.clone();
        let operation_id = operation_id.clone();
        let audit_id = audit_id.clone();
        Box::pin(async move {
            let (allow_registration, requires_invite): (i64, i64) = sqlx::query_as(
                "SELECT allow_registration,registration_requires_invite FROM system_settings WHERE id=1",
            )
            .fetch_one(&mut *conn)
            .await
            .map_err(AppError::Database)?;
            if allow_registration == 0 {
                return Ok(RegistrationOutcome::RegistrationDisabled);
            }
            if requires_invite != 0 && invite_code_hash.is_none() {
                return Ok(RegistrationOutcome::InviteRequired);
            }

            let invitation_id: Option<String> = if requires_invite != 0 {
                sqlx::query_scalar::<_, String>(
                    "SELECT id FROM invitations WHERE code_hash=? AND used_at IS NULL AND revoked_at IS NULL AND (expires_at IS NULL OR datetime(expires_at)>CURRENT_TIMESTAMP)",
                )
                .bind(invite_code_hash.as_deref().unwrap_or_default())
                .fetch_optional(&mut *conn)
                .await
                .map_err(AppError::Database)?
                .ok_or_else(|| {
                    AppError::api(
                        actix_web::http::StatusCode::BAD_REQUEST,
                        "INVITE_CODE_INVALID",
                        "邀请码无效或已失效，请联系管理员",
                    )
                })?
                .into()
            } else {
                None
            };

            let inserted = sqlx::query(
                "INSERT INTO users(id,username,username_normalized,password_hash) VALUES(?,?,?,?) ON CONFLICT(username_normalized) DO NOTHING",
            )
            .bind(&user_id)
            .bind(&username)
            .bind(&username_normalized)
            .bind(&password_hash)
            .execute(&mut *conn)
            .await
            .map_err(AppError::Database)?
            .rows_affected();
            if inserted == 0 {
                return Ok(RegistrationOutcome::DuplicateUsername);
            }

            if let Some(invitation_id) = invitation_id {
                let redeemed = sqlx::query(
                    "UPDATE invitations SET used_at=CURRENT_TIMESTAMP,used_by=? WHERE id=? AND used_at IS NULL AND revoked_at IS NULL AND (expires_at IS NULL OR datetime(expires_at)>CURRENT_TIMESTAMP)",
                )
                .bind(&user_id)
                .bind(&invitation_id)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?
                .rows_affected();
                if redeemed != 1 {
                    return Err(AppError::api(
                        actix_web::http::StatusCode::BAD_REQUEST,
                        "INVITE_CODE_INVALID",
                        "邀请码无效或已失效，请联系管理员",
                    ));
                }

                let details = serde_json::json!({
                    "invitation_id": invitation_id,
                    "user_id": user_id,
                })
                .to_string();
                sqlx::query("INSERT INTO admin_audit_logs(id,actor_type,actor_user_id,target_user_id,action,operation_id,details_json,client_ip,user_agent) VALUES(?,'USER',?,?, 'INVITATION_REDEEMED',?,?,?,?)")
                    .bind(&audit_id)
                    .bind(&user_id)
                    .bind(&user_id)
                    .bind(&operation_id)
                    .bind(details)
                    .bind(client_ip.as_deref())
                    .bind(user_agent.as_deref())
                    .execute(&mut *conn)
                    .await
                    .map_err(AppError::Database)?;
            }
            Ok(RegistrationOutcome::Created { user_id })
        })
    })
    .await
}

pub async fn create_user(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
) -> Result<CreateUserOutcome, AppError> {
    let id = Uuid::new_v4().to_string();
    let normalized = normalize_username(username);
    let input = (
        id.clone(),
        username.to_owned(),
        normalized.clone(),
        password_hash.to_owned(),
    );
    let result = crate::db::write::run(
        pool,
        "create user",
        &input,
        |conn, (id, username, normalized, password_hash)| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO users (id, username, username_normalized, password_hash) VALUES (?, ?, ?, ?)",
                )
                .bind(id)
                .bind(username)
                .bind(normalized)
                .bind(password_hash)
                .execute(conn)
                .await
                .map(|_| ())
                .map_err(crate::error::AppError::Database)
            })
        },
    )
    .await;

    match result {
        Ok(_) => Ok(CreateUserOutcome::Created(
            find_by_id(pool, &id)
                .await?
                .expect("newly created user should exist"),
        )),
        Err(AppError::Database(sqlx::Error::Database(error))) if error.is_unique_violation() => {
            Ok(CreateUserOutcome::DuplicateUsername)
        }
        Err(error) => Err(error),
    }
}

pub async fn find_by_normalized_username(
    pool: &SqlitePool,
    username_normalized: &str,
) -> Result<Option<UserRecord>, AppError> {
    sqlx::query_as(
        "SELECT id, username, username_normalized, password_hash, status, role, password_changed_at FROM users WHERE username_normalized = ?",
    )
    .bind(username_normalized)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)
}

pub async fn find_by_id(pool: &SqlitePool, id: &str) -> Result<Option<UserRecord>, AppError> {
    sqlx::query_as(
        "SELECT id, username, username_normalized, password_hash, status, role, password_changed_at FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use uuid::Uuid;

    use crate::{auth::password::normalize_username, db};

    use super::{CreateUserOutcome, create_user, find_by_normalized_username};

    #[tokio::test]
    async fn creates_finds_and_case_insensitively_deduplicates_users() {
        let pool = db::init_pool("sqlite::memory:").expect("pool");
        db::prepare_schema(&pool, true).await.expect("schema");

        let created = create_user(&pool, "Swartz", "hash").await.expect("create");
        assert!(matches!(created, CreateUserOutcome::Created(_)));

        let found = find_by_normalized_username(&pool, &normalize_username("swartz"))
            .await
            .expect("find")
            .expect("user");
        assert_eq!(found.username, "Swartz");

        let duplicate = create_user(&pool, "SWARTZ", "other")
            .await
            .expect("duplicate");
        assert!(matches!(duplicate, CreateUserOutcome::DuplicateUsername));
    }

    #[tokio::test]
    async fn create_user_retries_after_external_writer_releases() {
        let root = std::env::temp_dir().join(format!("rain-users-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let options = SqliteConnectOptions::new()
            .filename(root.join("rain.db"))
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_millis(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options.clone())
            .await
            .unwrap();
        let external = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        db::prepare_schema(&pool, true).await.unwrap();

        let mut blocker = external.begin().await.unwrap();
        sqlx::query(
            "INSERT INTO users (id, username, username_normalized, password_hash) VALUES (?, ?, ?, ?)",
        )
        .bind("external-user")
        .bind("External")
        .bind("external")
        .bind("hash")
        .execute(&mut *blocker)
        .await
        .unwrap();
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(75)).await;
            blocker.rollback().await.unwrap();
        });

        let result =
            tokio::time::timeout(Duration::from_secs(2), create_user(&pool, "Retry", "hash"))
                .await
                .unwrap()
                .unwrap();
        release.await.unwrap();
        assert!(matches!(result, CreateUserOutcome::Created(_)));

        pool.close().await;
        external.close().await;
        crate::db::write::remove_fixture_dir(root).await;
    }
}
