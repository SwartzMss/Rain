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
        VALUES (?, ?, ?, ?, 'PROCESSING', 'PENDING', ?)
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

pub(crate) async fn update_bundle_status(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    status: &str,
    stage: &str,
    failure_reason: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE bundles SET status = ?, process_stage = ?, failure_reason = ? WHERE id = ?",
    )
    .bind(status)
    .bind(stage)
    .bind(failure_reason)
    .bind(bundle_id)
    .execute(pool)
    .await
    .map_err(AppError::Database)?;
    Ok(())
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
