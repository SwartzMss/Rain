use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{
    auth::password::{hash_password, normalize_username, validate_password, validate_username},
    error::AppError,
};

pub async fn bootstrap_admin(
    pool: &SqlitePool,
    username: &str,
    password: &str,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await.map_err(AppError::Database)?;
    let (administrators, active_administrators, users): (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE role = 'ADMIN'), COUNT(*) FILTER (WHERE role = 'ADMIN' AND status = 'ACTIVE'), COUNT(*) FROM users",
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(AppError::Database)?;
    if administrators == 1 && active_administrators == 1 {
        tx.commit().await.map_err(AppError::Database)?;
        return Ok(());
    }
    if administrators != 0 {
        return Err(AppError::Config(
            "ADMIN_INVARIANT_VIOLATION: database must contain exactly one ACTIVE administrator"
                .into(),
        ));
    }
    if password.is_empty() {
        return Err(AppError::Config(
            "BOOTSTRAP_ADMIN_REQUIRED: bootstrap administrator password is required".into(),
        ));
    }
    validate_username(username).map_err(|_| {
        AppError::Config(
            "BOOTSTRAP_ADMIN_REQUIRED: invalid bootstrap administrator username".into(),
        )
    })?;
    validate_password(password).map_err(|_| {
        AppError::Config(
            "BOOTSTRAP_ADMIN_REQUIRED: invalid bootstrap administrator password".into(),
        )
    })?;
    let normalized = normalize_username(username);
    let occupied: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE username_normalized = ?")
            .bind(&normalized)
            .fetch_one(&mut *tx)
            .await
            .map_err(AppError::Database)?;
    if occupied > 0 {
        return Err(AppError::Config(
            "BOOTSTRAP_ADMIN_CONFLICT: bootstrap administrator username is occupied".into(),
        ));
    }
    if users > 0 {
        return Err(AppError::Config(
            "ADMIN_INVARIANT_VIOLATION: an initialized database cannot be missing its administrator"
                .into(),
        ));
    }
    let id = Uuid::new_v4().to_string();
    let password_hash = hash_password(password).map_err(|_| {
        AppError::Config(
            "BOOTSTRAP_ADMIN_REQUIRED: bootstrap administrator password could not be hashed".into(),
        )
    })?;
    sqlx::query("INSERT INTO users (id, username, username_normalized, password_hash, role, status) VALUES (?, ?, ?, ?, 'ADMIN', 'ACTIVE')")
        .bind(&id).bind(username).bind(normalized).bind(password_hash)
        .execute(&mut *tx).await.map_err(AppError::Database)?;
    sqlx::query("INSERT INTO admin_audit_logs (id, actor_type, target_user_id, action) VALUES (?, 'SYSTEM', ?, 'ADMIN_BOOTSTRAPPED')")
        .bind(Uuid::new_v4().to_string()).bind(&id)
        .execute(&mut *tx).await.map_err(AppError::Database)?;
    tx.commit().await.map_err(AppError::Database)
}
