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
