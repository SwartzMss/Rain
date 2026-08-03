use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow)]
pub struct UserSkillRecord {
    pub id: String,
    pub owner_user_id: String,
    pub name: String,
    pub description: Option<String>,
    pub skill_markdown: String,
    pub content_hash: String,
    pub version: i64,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillReview {
    pub overall_score: i64,
    pub grade: String,
    pub dimensions: Value,
    pub warnings: Vec<String>,
    pub suggestions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluated_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UserSkillResponse {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub skill_markdown: String,
    pub content_hash: String,
    pub version: i64,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
    pub review: Option<SkillReview>,
}

#[derive(Debug, Serialize)]
pub struct UserSkillSummaryResponse {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub content_hash: String,
    pub version: i64,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
    pub review: Option<SkillReview>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SkillPayload {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub skill_markdown: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}
