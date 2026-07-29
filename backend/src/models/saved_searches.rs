use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct SavedSearchRecord {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub search_type: String,
    pub query_text: String,
    pub scope_type: String,
    pub scope_key: Option<String>,
    #[serde(skip)]
    pub options_json: String,
    pub is_pinned: bool,
    pub sort_order: i64,
    pub created_at: String,
    pub updated_at: String,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SavedSearchResponse {
    pub id: String,
    pub name: String,
    pub search_type: String,
    pub query_text: String,
    pub scope_type: String,
    pub scope_key: Option<String>,
    pub options: Value,
    pub is_pinned: bool,
    pub sort_order: i64,
    pub created_at: String,
    pub updated_at: String,
    pub last_used_at: Option<String>,
}

impl From<SavedSearchRecord> for SavedSearchResponse {
    fn from(value: SavedSearchRecord) -> Self {
        Self {
            id: value.id,
            name: value.name,
            search_type: value.search_type,
            query_text: value.query_text,
            scope_type: value.scope_type,
            scope_key: value.scope_key,
            options: serde_json::from_str(&value.options_json)
                .unwrap_or_else(|_| Value::Object(Default::default())),
            is_pinned: value.is_pinned,
            sort_order: value.sort_order,
            created_at: value.created_at,
            updated_at: value.updated_at,
            last_used_at: value.last_used_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SavedSearchPayload {
    pub name: String,
    pub search_type: String,
    pub query_text: String,
    pub scope_type: String,
    pub scope_key: Option<String>,
    #[serde(default)]
    pub options: Value,
    #[serde(default)]
    pub is_pinned: bool,
    #[serde(default)]
    pub sort_order: i64,
}

#[derive(Debug, Deserialize)]
pub struct SavedSearchListQuery {
    pub issue_code: Option<String>,
}
