use crate::error::AppError;

pub async fn create_processing_bundle(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    issue_code: &str,
    bundle_hash: &str,
    bundle_name: &str,
    total_bytes: u64,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO bundles (id, issue_code, hash, name, status, process_stage, size_bytes)
        VALUES (?, ?, ?, ?, 'RECEIVING', 'RECEIVING', ?)
        "#,
    )
    .bind(bundle_id)
    .bind(issue_code)
    .bind(bundle_hash)
    .bind(bundle_name)
    .bind(Some(total_bytes as i64))
    .execute(pool)
    .await
    .map_err(AppError::Database)?;
    Ok(())
}

pub(crate) async fn advance_bundle_stage(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    stage: &str,
) -> Result<(), AppError> {
    let rank = |value: &str| match value {
        "PENDING" => 0,
        "RECEIVING" => 1,
        "EXTRACTING" => 2,
        "INDEXING" => 3,
        "PUBLISHING" => 4,
        "READY" => 5,
        _ => -1,
    };
    let current: String = sqlx::query_scalar("SELECT process_stage FROM bundles WHERE id = ?")
        .bind(bundle_id)
        .fetch_one(pool)
        .await
        .map_err(AppError::Database)?;
    if rank(stage) < rank(&current) {
        return Ok(());
    }
    if rank(stage) < 0 {
        return Err(AppError::Config(format!("invalid bundle stage: {stage}")));
    }
    sqlx::query("UPDATE bundles SET status = ?, process_stage = ? WHERE id = ?")
        .bind(stage)
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

    use super::user_facing_failure_reason;

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
