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
