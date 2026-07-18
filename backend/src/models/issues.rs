use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UploadStatus {
    Ready,
    Processing,
    Failed,
    Pending,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UploadStage {
    Pending,
    Receiving,
    Extracting,
    Indexing,
    Publishing,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadSummary {
    pub hash: String,
    pub name: String,
    pub status: UploadStatusWrapper,
    pub stage: UploadStage,
    pub failure_reason: Option<String>,
    pub failure_stage: Option<String>,
    pub failure_code: Option<String>,
    pub retryable: Option<bool>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadStatusWrapper {
    pub upload_status: UploadStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueBundlesResponse {
    pub name: String,
    #[serde(rename = "log_bundles")]
    pub log_bundles: Vec<UploadSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct IssueSummary {
    pub code: String,
    pub name: String,
    pub bundle_count: i64,
}

impl UploadStatus {
    pub fn from_db_value(value: &str) -> Self {
        if value.eq_ignore_ascii_case("READY") {
            UploadStatus::Ready
        } else if value.eq_ignore_ascii_case("FAILED") {
            UploadStatus::Failed
        } else if value.eq_ignore_ascii_case("PROCESSING") {
            UploadStatus::Processing
        } else {
            UploadStatus::Pending
        }
    }
}

impl UploadStage {
    pub fn from_db_value(value: &str) -> Self {
        if value.eq_ignore_ascii_case("RECEIVING") {
            Self::Receiving
        } else if value.eq_ignore_ascii_case("EXTRACTING") {
            Self::Extracting
        } else if value.eq_ignore_ascii_case("INDEXING") {
            Self::Indexing
        } else if value.eq_ignore_ascii_case("PUBLISHING") {
            Self::Publishing
        } else if value.eq_ignore_ascii_case("READY") {
            Self::Ready
        } else if value.eq_ignore_ascii_case("FAILED") {
            Self::Failed
        } else {
            Self::Pending
        }
    }
}
