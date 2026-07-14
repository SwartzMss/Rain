use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::file_classification::PreviewKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNode {
    pub id: String,
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub preview_kind: PreviewKind,
    pub size_bytes: Option<u64>,
    pub mime_type: Option<String>,
    pub status: Option<String>,
    pub meta: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNodeResponse {
    pub node: FileNode,
    pub children: Vec<FileNode>,
}
