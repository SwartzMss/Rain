use crate::error::AppError;

pub async fn create_processing_bundle(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    issue_code: &str,
    bundle_hash: &str,
    bundle_name: &str,
    total_bytes: u64,
) -> Result<(), AppError> {
    let result = sqlx::query(
        r#"
        INSERT INTO bundles (id, issue_code, hash, name, status, process_stage, size_bytes)
        SELECT ?, code, ?, ?, 'PROCESSING', 'RECEIVING', ?
        FROM issues
        WHERE code = ? AND status = 'ACTIVE'
        "#,
    )
    .bind(bundle_id)
    .bind(bundle_hash)
    .bind(bundle_name)
    .bind(Some(total_bytes as i64))
    .bind(issue_code)
    .execute(pool)
    .await
    .map_err(AppError::Database)?;
    if result.rows_affected() == 0 {
        return Err(AppError::Conflict(format!(
            "issue {issue_code} is missing or being deleted"
        )));
    }
    Ok(())
}

pub(crate) async fn set_bundle_stage(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    stage: &str,
) -> Result<(), AppError> {
    if !matches!(
        stage,
        "RECEIVING" | "EXTRACTING" | "INDEXING" | "PUBLISHING"
    ) {
        return Err(AppError::Config(format!("invalid bundle stage: {stage}")));
    }
    sqlx::query("UPDATE bundles SET process_stage = ? WHERE id = ? AND status = 'PROCESSING'")
        .bind(stage)
        .bind(bundle_id)
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
    Ok(())
}

pub(crate) struct FailureDetails {
    pub code: &'static str,
    pub reason: String,
    pub retryable: bool,
}

pub(crate) fn failure_details(error: &AppError) -> FailureDetails {
    let (code, retryable) = match error {
        AppError::Config(_) => ("CONFIGURATION_ERROR", false),
        AppError::Database(_) => ("DATABASE_UNAVAILABLE", true),
        AppError::Io(_) => ("STORAGE_IO_ERROR", true),
        AppError::NotFound(_) => ("RESOURCE_NOT_FOUND", false),
        AppError::BadRequest(_) => ("INVALID_CONTENT", false),
        AppError::Conflict(_) => ("CONFLICT", true),
    };
    FailureDetails {
        code,
        reason: user_facing_failure_reason(error),
        retryable,
    }
}

pub(crate) fn user_facing_failure_reason(error: &AppError) -> String {
    match error {
        AppError::BadRequest(message) | AppError::Conflict(message) => message.clone(),
        _ => "上传处理失败，请删除后重试".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use crate::error::AppError;

    use super::{create_processing_bundle, set_bundle_stage, user_facing_failure_reason};

    #[tokio::test]
    async fn bundle_creation_requires_an_active_issue_atomically() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query("INSERT INTO issues (code, name, status) VALUES ('RACE', 'Race', 'DELETING')")
            .execute(&pool)
            .await
            .unwrap();

        assert!(
            create_processing_bundle(&pool, "bundle", "RACE", "hash", "name", 1)
                .await
                .is_err()
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bundles")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn stage_tracks_current_operation_even_when_it_moves_back_to_extracting() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query("INSERT INTO issues (code, name) VALUES ('MIXED', 'Mixed')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO bundles (id, issue_code, hash, name, status, process_stage) VALUES ('mixed', 'MIXED', 'mixed-hash', 'mixed', 'PROCESSING', 'INDEXING')")
            .execute(&pool)
            .await
            .unwrap();

        set_bundle_stage(&pool, "mixed", "EXTRACTING")
            .await
            .unwrap();
        let state: (String, String) =
            sqlx::query_as("SELECT status, process_stage FROM bundles WHERE id = 'mixed'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, ("PROCESSING".into(), "EXTRACTING".into()));
    }

    #[test]
    fn preserves_actionable_bad_request_failure_reason() {
        let error = AppError::BadRequest("压缩包条目超过配置上限".into());
        assert_eq!(user_facing_failure_reason(&error), "压缩包条目超过配置上限");
    }

    #[test]
    fn hides_internal_failure_details() {
        let error = AppError::Io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "/secret/path",
        ));
        let reason = user_facing_failure_reason(&error);
        assert_eq!(reason, "上传处理失败，请删除后重试");
        assert!(!reason.contains("/secret/path"));
    }
}
