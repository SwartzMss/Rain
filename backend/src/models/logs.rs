use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSearchHit {
    pub file_id: String,
    pub path: String,
    pub snippet: String,
    pub timeline: Option<String>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSearchResponse {
    pub total: u64,
    pub hits: Vec<LogSearchHit>,
}
