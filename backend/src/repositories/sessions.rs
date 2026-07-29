use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{auth::AuthenticatedUser, error::AppError};

const LAST_SEEN_UPDATE_INTERVAL_SECONDS: i64 = 300;

pub struct ResolvedSessionUser {
    pub user: AuthenticatedUser,
    pub status: String,
}

pub async fn create_session(
    pool: &SqlitePool,
    user_id: &str,
    token_hash: &str,
    expires_at: DateTime<Utc>,
    user_agent: Option<&str>,
    client_ip: Option<&str>,
) -> Result<String, AppError> {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO user_sessions (id, user_id, token_hash, expires_at, user_agent, client_ip) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(token_hash)
    .bind(expires_at.to_rfc3339())
    .bind(user_agent)
    .bind(client_ip)
    .execute(pool)
    .await
    .map_err(AppError::Database)?;
    Ok(id)
}

pub async fn resolve_active_user(
    pool: &SqlitePool,
    token_hash: &str,
) -> Result<Option<AuthenticatedUser>, AppError> {
    Ok(resolve_session_user(pool, token_hash)
        .await?
        .filter(|resolved| resolved.status == "ACTIVE")
        .map(|resolved| resolved.user))
}

pub async fn resolve_session_user(
    pool: &SqlitePool,
    token_hash: &str,
) -> Result<Option<ResolvedSessionUser>, AppError> {
    let user = sqlx::query_as::<_, (String, String, String)>(
        r#"
        SELECT users.id, users.username, users.status
        FROM user_sessions
        JOIN users ON users.id = user_sessions.user_id
        WHERE user_sessions.token_hash = ?
          AND user_sessions.revoked_at IS NULL
          AND datetime(user_sessions.expires_at) > CURRENT_TIMESTAMP
        "#,
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)?
    .map(|(id, username, status)| ResolvedSessionUser {
        user: AuthenticatedUser { id, username },
        status,
    });

    if user.is_some() {
        let _ = sqlx::query(
            r#"
            UPDATE user_sessions
            SET last_seen_at = CURRENT_TIMESTAMP
            WHERE token_hash = ?
              AND (
                last_seen_at IS NULL
                OR datetime(last_seen_at) <= datetime(CURRENT_TIMESTAMP, ?)
              )
            "#,
        )
        .bind(token_hash)
        .bind(format!("-{LAST_SEEN_UPDATE_INTERVAL_SECONDS} seconds"))
        .execute(pool)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to update session last_seen_at");
            error
        });
    }
    Ok(user)
}

pub async fn revoke_by_token_hash(pool: &SqlitePool, token_hash: &str) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE user_sessions SET revoked_at = CURRENT_TIMESTAMP WHERE token_hash = ? AND revoked_at IS NULL",
    )
    .bind(token_hash)
    .execute(pool)
    .await
    .map_err(AppError::Database)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use crate::{db, repositories::users};

    use super::{create_session, resolve_active_user, revoke_by_token_hash};

    #[tokio::test]
    async fn resolves_only_active_unexpired_unrevoked_sessions() {
        let pool = db::init_pool("sqlite::memory:").expect("pool");
        db::prepare_schema(&pool, true).await.expect("schema");
        let user = match users::create_user(&pool, "Swartz", "hash")
            .await
            .expect("user")
        {
            users::CreateUserOutcome::Created(user) => user,
            users::CreateUserOutcome::DuplicateUsername => panic!("unexpected duplicate"),
        };

        create_session(
            &pool,
            &user.id,
            "valid-hash",
            Utc::now() + Duration::hours(1),
            None,
            None,
        )
        .await
        .expect("session");
        assert_eq!(
            resolve_active_user(&pool, "valid-hash")
                .await
                .expect("resolve")
                .expect("active")
                .username,
            "Swartz"
        );

        revoke_by_token_hash(&pool, "valid-hash")
            .await
            .expect("revoke");
        assert!(
            resolve_active_user(&pool, "valid-hash")
                .await
                .expect("resolve")
                .is_none()
        );

        create_session(
            &pool,
            &user.id,
            "expired-hash",
            Utc::now() - Duration::hours(1),
            None,
            None,
        )
        .await
        .expect("expired session");
        assert!(
            resolve_active_user(&pool, "expired-hash")
                .await
                .expect("resolve")
                .is_none()
        );
    }
}
