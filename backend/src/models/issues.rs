use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UploadStatus {
    Ready,
    Processing,
    Failed,
    Pending,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadSummary {
    pub hash: String,
    pub name: String,
    pub status: UploadStatusWrapper,
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

impl UploadStatus {
    pub fn from_db_value(value: &str) -> Self {
        if value.eq_ignore_ascii_case("READY") {
            UploadStatus::Ready
        } else if value.eq_ignore_ascii_case("PROCESSING") {
            UploadStatus::Processing
        } else if value.eq_ignore_ascii_case("FAILED") {
            UploadStatus::Failed
        } else {
            UploadStatus::Pending
        }
    }
}
