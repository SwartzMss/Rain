use sqlx::FromRow;

use actix_web::http::StatusCode;

use crate::{
    AppState,
    error::{AppError, codes},
};

#[derive(FromRow)]
pub struct BundleRow {
    pub id: String,
    pub hash: String,
    pub name: String,
    pub status: String,
    pub issue_code: String,
}

pub async fn load_bundle(pool: &sqlx::SqlitePool, hash: &str) -> Result<BundleRow, AppError> {
    sqlx::query_as::<_, BundleRow>(
        "SELECT b.id, b.hash, b.name, b.status, b.issue_code FROM bundles b JOIN issues i ON i.code = b.issue_code WHERE b.hash = ? AND b.deleted_at IS NULL AND i.status = 'ACTIVE' LIMIT 1",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("bundle {hash}")))
}

pub fn ensure_bundle_ready(bundle: &BundleRow) -> Result<(), AppError> {
    match bundle.status.as_str() {
        status if status.eq_ignore_ascii_case("READY") => Ok(()),
        status
            if status.eq_ignore_ascii_case("PROCESSING")
                || status.eq_ignore_ascii_case("PENDING")
                || matches!(
                    status,
                    "RECEIVING" | "VALIDATING" | "EXTRACTING" | "INDEXING" | "PUBLISHING"
                ) =>
        {
            Err(AppError::api(
                StatusCode::CONFLICT,
                codes::BUNDLE_PROCESSING,
                "文件仍在处理中，请稍后重试",
            ))
        }
        status if status.eq_ignore_ascii_case("FAILED") => Err(AppError::api(
            StatusCode::CONFLICT,
            codes::BUNDLE_PROCESSING_FAILED,
            "文件处理失败，请重新上传或删除",
        )),
        _ => Err(AppError::Conflict("invalid bundle status".into())),
    }
}

pub fn data_root(state: &actix_web::web::Data<AppState>) -> std::path::PathBuf {
    state.storage.data_root.clone()
}

#[cfg(test)]
mod tests {
    use actix_web::{ResponseError, body::to_bytes, http::StatusCode};
    use serde_json::Value;

    use super::{BundleRow, ensure_bundle_ready};

    fn bundle(status: &str) -> BundleRow {
        BundleRow {
            id: "bundle-id".into(),
            hash: "bundle-hash".into(),
            name: "bundle".into(),
            status: status.into(),
            issue_code: "ISSUE".into(),
        }
    }

    async fn error_payload(error: &crate::error::AppError) -> Value {
        let body = to_bytes(error.error_response().into_body())
            .await
            .expect("error response body");
        serde_json::from_slice(&body).expect("error response JSON")
    }

    #[actix_web::test]
    async fn processing_and_failed_bundle_states_have_actionable_public_errors() {
        let processing = ensure_bundle_ready(&bundle("PROCESSING")).expect_err("processing error");
        assert_eq!(processing.status_code(), StatusCode::CONFLICT);
        let processing_payload = error_payload(&processing).await;
        assert_eq!(processing_payload["code"], "BUNDLE_PROCESSING");
        assert_eq!(processing_payload["message"], "文件仍在处理中，请稍后重试");

        let failed = ensure_bundle_ready(&bundle("FAILED")).expect_err("failed error");
        assert_eq!(failed.status_code(), StatusCode::CONFLICT);
        let failed_payload = error_payload(&failed).await;
        assert_eq!(failed_payload["code"], "BUNDLE_PROCESSING_FAILED");
        assert_eq!(failed_payload["message"], "文件处理失败，请重新上传或删除");
    }

    #[actix_web::test]
    async fn unknown_bundle_states_remain_generic() {
        let error = ensure_bundle_ready(&bundle("MYSTERY")).expect_err("unknown state error");
        assert!(matches!(error, crate::error::AppError::Conflict(_)));
        let payload = error_payload(&error).await;
        assert_eq!(payload["code"], "CONFLICT");
        assert_eq!(payload["message"], "请求冲突");
    }
}
