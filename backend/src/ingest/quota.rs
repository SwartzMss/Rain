use actix_web::http::StatusCode;
use sqlx::SqlitePool;

use crate::error::AppError;

#[derive(Clone)]
pub struct IssueQuota {
    pool: SqlitePool,
    issue_code: String,
    bundle_id: String,
    limit: u64,
}

impl IssueQuota {
    pub fn new(
        pool: SqlitePool,
        issue_code: impl Into<String>,
        bundle_id: impl Into<String>,
        limit: u64,
    ) -> Self {
        Self {
            pool,
            issue_code: issue_code.into(),
            bundle_id: bundle_id.into(),
            limit,
        }
    }

    pub async fn reserve(&self, bytes: u64) -> Result<(), AppError> {
        if bytes == 0 {
            return Ok(());
        }
        let bytes = i64::try_from(bytes)
            .map_err(|_| AppError::BadRequest("Issue 内容大小超出数据库范围".into()))?;
        let limit = i64::try_from(self.limit).map_err(|_| {
            AppError::Config("RAIN_ISSUE_MAX_CONTENT_SIZE exceeds SQLite range".into())
        })?;

        crate::db::write::run(
            &self.pool,
            "reserve issue quota",
            &(self.bundle_id.as_str(), self.issue_code.as_str(), bytes, limit),
            |conn, &(bundle_id, issue_code, bytes, limit)| {
                Box::pin(async move {
                        let result = sqlx::query(
                            r#"
                            UPDATE bundles
                            SET content_size_bytes = content_size_bytes + ?
                            WHERE id = ?
                              AND issue_code = ?
                              AND status = 'PROCESSING'
                              AND (
                                SELECT COALESCE(SUM(content_size_bytes), 0)
                                FROM bundles
                                WHERE issue_code = ? AND status IN ('READY', 'PROCESSING')
                              ) <= ? - ?
                            "#,
                        )
                        .bind(bytes)
                        .bind(bundle_id)
                        .bind(issue_code)
                        .bind(issue_code)
                        .bind(limit)
                        .bind(bytes)
                        .execute(&mut *conn)
                        .await
                        .map_err(AppError::Database)?;

                        if result.rows_affected() == 1 {
                            return Ok(());
                        }

                        let usage: i64 = sqlx::query_scalar(
                            "SELECT COALESCE(SUM(content_size_bytes), 0) FROM bundles WHERE issue_code = ? AND status IN ('READY', 'PROCESSING')",
                        )
                        .bind(issue_code)
                        .fetch_one(&mut *conn)
                        .await
                        .map_err(AppError::Database)?;
                        Err(AppError::public(
                            StatusCode::BAD_REQUEST,
                            "ISSUE_QUOTA_EXCEEDED",
                            format!(
                                "Issue 内容超过 {} 上限；当前已使用 {}，本次新增内容至少 {}",
                                format_binary_size(limit as u64),
                                format_binary_size(usage.max(0) as u64),
                                format_binary_size(bytes as u64)
                            ),
                        ))
                })
            },
        )
        .await
    }

    #[cfg(test)]
    pub async fn reserved_bytes(&self) -> Result<u64, AppError> {
        let bytes: i64 = sqlx::query_scalar("SELECT content_size_bytes FROM bundles WHERE id = ?")
            .bind(&self.bundle_id)
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::Database)?;
        Ok(bytes.max(0) as u64)
    }
}

fn format_binary_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB && bytes.is_multiple_of(GIB) {
        format!("{} GiB", bytes / GIB)
    } else if bytes >= MIB && bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else if bytes >= KIB && bytes.is_multiple_of(KIB) {
        format!("{} KiB", bytes / KIB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn competing_reservations_cannot_exceed_issue_quota() {
        let pool = crate::db::init_pool("sqlite::memory:").unwrap();
        crate::db::prepare_schema(&pool, true).await.unwrap();
        sqlx::query("INSERT INTO issues (code, name) VALUES ('QUOTA', 'Quota')")
            .execute(&pool)
            .await
            .unwrap();
        for id in ["first", "second"] {
            crate::upload::lifecycle::create_processing_bundle(&pool, id, "QUOTA", id, id, 0, None)
                .await
                .unwrap();
        }
        let first = IssueQuota::new(pool.clone(), "QUOTA", "first", 10);
        let second = IssueQuota::new(pool.clone(), "QUOTA", "second", 10);
        let (left, right) = tokio::join!(first.reserve(6), second.reserve(6));
        assert_ne!(left.is_ok(), right.is_ok());
        let error = left.err().or(right.err()).unwrap();
        assert!(matches!(
            error,
            AppError::PublicApi {
                code: "ISSUE_QUOTA_EXCEEDED",
                ..
            }
        ));
        assert_eq!(
            first.reserved_bytes().await.unwrap() + second.reserved_bytes().await.unwrap(),
            6
        );
        // The failed transaction must release admission and leave no quota charge.
        second.reserve(4).await.unwrap();
        assert_eq!(
            first.reserved_bytes().await.unwrap() + second.reserved_bytes().await.unwrap(),
            10
        );
    }
}
