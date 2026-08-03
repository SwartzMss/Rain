use serde::Serialize;
use sqlx::FromRow;

#[derive(Debug, Clone)]
pub struct NewSkillRun {
    pub user_id: String,
    pub issue_code: String,
    pub skill_id: String,
    pub skill_version: i64,
    pub skill_name: String,
    pub skill_snapshot_markdown: String,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct SkillRunRecord {
    pub id: String,
    pub user_id: String,
    pub issue_code: String,
    pub skill_id: String,
    pub skill_version: i64,
    pub skill_name: String,
    #[serde(skip_serializing)]
    pub skill_snapshot_markdown: String,
    pub status: String,
    pub iteration_count: i64,
    pub tool_call_count: i64,
    pub cancel_requested: bool,
    #[serde(skip_serializing)]
    pub result_json: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub created_at: String,
}
