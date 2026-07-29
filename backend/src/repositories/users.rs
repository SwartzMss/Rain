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
}

#[derive(Debug)]
pub enum CreateUserOutcome {
    Created(UserRecord),
    DuplicateUsername,
}

pub async fn create_user(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
) -> Result<CreateUserOutcome, AppError> {
    let id = Uuid::new_v4().to_string();
    let normalized = normalize_username(username);
    let result = sqlx::query(
        "INSERT INTO users (id, username, username_normalized, password_hash) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(username)
    .bind(&normalized)
    .bind(password_hash)
    .execute(pool)
    .await;

    match result {
        Ok(_) => Ok(CreateUserOutcome::Created(
            find_by_id(pool, &id)
                .await?
                .expect("newly created user should exist"),
        )),
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
            Ok(CreateUserOutcome::DuplicateUsername)
        }
        Err(error) => Err(AppError::Database(error)),
    }
}

pub async fn find_by_normalized_username(
    pool: &SqlitePool,
    username_normalized: &str,
) -> Result<Option<UserRecord>, AppError> {
    sqlx::query_as(
        "SELECT id, username, username_normalized, password_hash, status, role FROM users WHERE username_normalized = ?",
    )
    .bind(username_normalized)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)
}

pub async fn find_by_id(pool: &SqlitePool, id: &str) -> Result<Option<UserRecord>, AppError> {
    sqlx::query_as(
        "SELECT id, username, username_normalized, password_hash, status, role FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)
}

pub async fn mark_login(pool: &SqlitePool, id: &str) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE users SET last_login_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(id)
    .execute(pool)
    .await
    .map_err(AppError::Database)?;
    Ok(())
}

#[cfg(test)]
mod tests {
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
}
