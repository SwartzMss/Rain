use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{auth::AuthenticatedUser, error::AppError};

const LAST_SEEN_UPDATE_INTERVAL_SECONDS: i64 = 300;

fn last_seen_needs_update(last_seen_at: Option<&str>, now: DateTime<Utc>) -> bool {
    let Some(last_seen_at) = last_seen_at else {
        return true;
    };
    let Ok(last_seen_at) = NaiveDateTime::parse_from_str(last_seen_at, "%Y-%m-%d %H:%M:%S") else {
        return true;
    };
    now.signed_duration_since(last_seen_at.and_utc())
        .num_seconds()
        >= LAST_SEEN_UPDATE_INTERVAL_SECONDS
}

pub struct ResolvedSessionUser {
    pub user: AuthenticatedUser,
    pub status: String,
}

pub struct ReplacementSession<'a> {
    pub token_hash: &'a str,
    pub expires_at: DateTime<Utc>,
    pub user_agent: Option<&'a str>,
    pub client_ip: Option<&'a str>,
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

pub async fn create_session_if_password_unchanged(
    pool: &SqlitePool,
    user_id: &str,
    expected_password_hash: &str,
    token_hash: &str,
    expires_at: DateTime<Utc>,
    user_agent: Option<&str>,
    client_ip: Option<&str>,
) -> Result<bool, AppError> {
    let id = Uuid::new_v4().to_string();
    let result = sqlx::query(
        r#"
        INSERT INTO user_sessions (id, user_id, token_hash, expires_at, user_agent, client_ip)
        SELECT ?, id, ?, ?, ?, ?
        FROM users
        WHERE id = ? AND password_hash = ? AND status = 'ACTIVE'
        "#,
    )
    .bind(id)
    .bind(token_hash)
    .bind(expires_at.to_rfc3339())
    .bind(user_agent)
    .bind(client_ip)
    .bind(user_id)
    .bind(expected_password_hash)
    .execute(pool)
    .await
    .map_err(AppError::Database)?;
    Ok(result.rows_affected() == 1)
}

pub async fn change_password_and_replace_sessions(
    pool: &SqlitePool,
    user_id: &str,
    expected_password_hash: &str,
    current_token_hash: &str,
    new_password_hash: &str,
    replacement: ReplacementSession<'_>,
) -> Result<bool, AppError> {
    let mut transaction = pool.begin().await.map_err(AppError::Database)?;
    let updated = sqlx::query(
        r#"
        UPDATE users
        SET password_hash = ?, password_changed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
          AND password_hash = ?
          AND status = 'ACTIVE'
          AND EXISTS (
              SELECT 1
              FROM user_sessions
              WHERE user_sessions.user_id = users.id
                AND user_sessions.token_hash = ?
                AND user_sessions.revoked_at IS NULL
                AND datetime(user_sessions.expires_at) > CURRENT_TIMESTAMP
          )
        "#,
    )
    .bind(new_password_hash)
    .bind(user_id)
    .bind(expected_password_hash)
    .bind(current_token_hash)
    .execute(&mut *transaction)
    .await
    .map_err(AppError::Database)?
    .rows_affected();
    if updated != 1 {
        transaction.rollback().await.map_err(AppError::Database)?;
        return Ok(false);
    }
    sqlx::query(
        "UPDATE user_sessions SET revoked_at = CURRENT_TIMESTAMP WHERE user_id = ? AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut *transaction)
    .await
    .map_err(AppError::Database)?;
    sqlx::query(
        "INSERT INTO user_sessions (id, user_id, token_hash, expires_at, user_agent, client_ip) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(user_id)
    .bind(replacement.token_hash)
    .bind(replacement.expires_at.to_rfc3339())
    .bind(replacement.user_agent)
    .bind(replacement.client_ip)
    .execute(&mut *transaction)
    .await
    .map_err(AppError::Database)?;
    transaction.commit().await.map_err(AppError::Database)?;
    Ok(true)
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
    let resolved = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        r#"
        SELECT users.id, users.username, users.status, user_sessions.last_seen_at
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
    .map_err(AppError::Database)?;

    if resolved.as_ref().is_some_and(|(_, _, _, last_seen_at)| {
        last_seen_needs_update(last_seen_at.as_deref(), Utc::now())
    }) {
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
    Ok(
        resolved.map(|(id, username, status, _)| ResolvedSessionUser {
            user: AuthenticatedUser { id, username },
            status,
        }),
    )
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

pub async fn revoke_all_for_user(pool: &SqlitePool, user_id: &str) -> Result<u64, AppError> {
    Ok(sqlx::query(
        "UPDATE user_sessions SET revoked_at = CURRENT_TIMESTAMP WHERE user_id = ? AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(pool)
    .await
    .map_err(AppError::Database)?
    .rows_affected())
}

pub async fn revoke_others_for_user(
    pool: &SqlitePool,
    user_id: &str,
    current_token_hash: &str,
) -> Result<u64, AppError> {
    Ok(sqlx::query(
        "UPDATE user_sessions SET revoked_at = CURRENT_TIMESTAMP WHERE user_id = ? AND token_hash != ? AND revoked_at IS NULL",
    )
    .bind(user_id)
    .bind(current_token_hash)
    .execute(pool)
    .await
    .map_err(AppError::Database)?
    .rows_affected())
}

pub async fn cleanup_expired_or_revoked(pool: &SqlitePool) -> Result<u64, AppError> {
    Ok(sqlx::query(
        "DELETE FROM user_sessions WHERE datetime(expires_at) <= CURRENT_TIMESTAMP OR revoked_at IS NOT NULL",
    )
    .execute(pool)
    .await
    .map_err(AppError::Database)?
    .rows_affected())
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone, Utc};

    use crate::{db, repositories::users};

    use super::{
        ReplacementSession, change_password_and_replace_sessions, cleanup_expired_or_revoked,
        create_session, create_session_if_password_unchanged, last_seen_needs_update,
        resolve_active_user, revoke_by_token_hash,
    };

    #[test]
    fn last_seen_updates_only_after_the_activity_interval() {
        let now = Utc
            .with_ymd_and_hms(2026, 7, 30, 10, 0, 0)
            .single()
            .expect("fixed time");

        assert!(!last_seen_needs_update(Some("2026-07-30 10:00:00"), now));
        assert!(!last_seen_needs_update(Some("2026-07-30 09:55:01"), now));
        assert!(last_seen_needs_update(Some("2026-07-30 09:55:00"), now));
        assert!(last_seen_needs_update(None, now));
        assert!(last_seen_needs_update(Some("invalid"), now));
    }

    #[tokio::test]
    async fn session_resolution_advances_only_stale_last_seen_values() {
        let pool = db::init_pool("sqlite::memory:").expect("pool");
        db::prepare_schema(&pool, true).await.expect("schema");
        let user = match users::create_user(&pool, "Activity", "hash")
            .await
            .expect("user")
        {
            users::CreateUserOutcome::Created(user) => user,
            users::CreateUserOutcome::DuplicateUsername => panic!("unexpected duplicate"),
        };
        create_session(
            &pool,
            &user.id,
            "activity-hash",
            Utc::now() + Duration::hours(1),
            None,
            None,
        )
        .await
        .expect("session");

        sqlx::query(
            "UPDATE user_sessions SET last_seen_at = CURRENT_TIMESTAMP WHERE token_hash = ?",
        )
        .bind("activity-hash")
        .execute(&pool)
        .await
        .expect("set fresh timestamp");
        let fresh_before: String =
            sqlx::query_scalar("SELECT last_seen_at FROM user_sessions WHERE token_hash = ?")
                .bind("activity-hash")
                .fetch_one(&pool)
                .await
                .expect("fresh timestamp");
        resolve_active_user(&pool, "activity-hash")
            .await
            .expect("resolve fresh session");
        let fresh_after: String =
            sqlx::query_scalar("SELECT last_seen_at FROM user_sessions WHERE token_hash = ?")
                .bind("activity-hash")
                .fetch_one(&pool)
                .await
                .expect("fresh timestamp after resolution");
        assert_eq!(fresh_after, fresh_before);

        sqlx::query(
            "UPDATE user_sessions SET last_seen_at = '2000-01-01 00:00:00' WHERE token_hash = ?",
        )
        .bind("activity-hash")
        .execute(&pool)
        .await
        .expect("set stale timestamp");
        resolve_active_user(&pool, "activity-hash")
            .await
            .expect("resolve stale session");
        let stale_after: String =
            sqlx::query_scalar("SELECT last_seen_at FROM user_sessions WHERE token_hash = ?")
                .bind("activity-hash")
                .fetch_one(&pool)
                .await
                .expect("stale timestamp after resolution");
        assert_ne!(stale_after, "2000-01-01 00:00:00");
    }

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

    #[tokio::test]
    async fn cleanup_removes_expired_and_revoked_sessions() {
        let pool = db::init_pool("sqlite::memory:").expect("pool");
        db::prepare_schema(&pool, true).await.expect("schema");
        let user = match users::create_user(&pool, "Cleaner", "hash")
            .await
            .expect("user")
        {
            users::CreateUserOutcome::Created(user) => user,
            users::CreateUserOutcome::DuplicateUsername => panic!("unexpected duplicate"),
        };
        create_session(
            &pool,
            &user.id,
            "expired",
            Utc::now() - Duration::hours(1),
            None,
            None,
        )
        .await
        .expect("expired");
        create_session(
            &pool,
            &user.id,
            "revoked",
            Utc::now() + Duration::hours(1),
            None,
            None,
        )
        .await
        .expect("revoked");
        revoke_by_token_hash(&pool, "revoked")
            .await
            .expect("revoke");
        assert_eq!(cleanup_expired_or_revoked(&pool).await.expect("cleanup"), 2);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_sessions")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn login_cannot_create_session_after_password_hash_changes() {
        let pool = db::init_pool("sqlite::memory:").expect("pool");
        db::prepare_schema(&pool, true).await.expect("schema");
        let user = match users::create_user(&pool, "Concurrent", "old-hash")
            .await
            .expect("user")
        {
            users::CreateUserOutcome::Created(user) => user,
            users::CreateUserOutcome::DuplicateUsername => panic!("unexpected duplicate"),
        };
        sqlx::query("UPDATE users SET password_hash = 'new-hash' WHERE id = ?")
            .bind(&user.id)
            .execute(&pool)
            .await
            .expect("change password");
        let created = create_session_if_password_unchanged(
            &pool,
            &user.id,
            "old-hash",
            "late-login",
            Utc::now() + Duration::hours(1),
            None,
            None,
        )
        .await
        .expect("conditional session");
        assert!(!created);
    }

    #[tokio::test]
    async fn password_change_cannot_replace_a_revoked_current_session() {
        let pool = db::init_pool("sqlite::memory:").expect("pool");
        db::prepare_schema(&pool, true).await.expect("schema");
        let user = match users::create_user(&pool, "LoggedOut", "old-hash")
            .await
            .expect("user")
        {
            users::CreateUserOutcome::Created(user) => user,
            users::CreateUserOutcome::DuplicateUsername => panic!("unexpected duplicate"),
        };
        create_session(
            &pool,
            &user.id,
            "current-token",
            Utc::now() + Duration::hours(1),
            None,
            None,
        )
        .await
        .expect("session");
        revoke_by_token_hash(&pool, "current-token")
            .await
            .expect("logout all");
        let changed = change_password_and_replace_sessions(
            &pool,
            &user.id,
            "old-hash",
            "current-token",
            "new-hash",
            ReplacementSession {
                token_hash: "replacement-token",
                expires_at: Utc::now() + Duration::hours(1),
                user_agent: None,
                client_ip: None,
            },
        )
        .await
        .expect("conditional password change");
        assert!(!changed);
        let password_hash: String =
            sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
                .bind(&user.id)
                .fetch_one(&pool)
                .await
                .expect("password");
        assert_eq!(password_hash, "old-hash");
        let replacement_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM user_sessions WHERE token_hash = ?")
                .bind("replacement-token")
                .fetch_one(&pool)
                .await
                .expect("replacement count");
        assert_eq!(replacement_count, 0);
    }
}
