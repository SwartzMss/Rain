use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::auth::UserStatus;
use crate::settings::AdaptiveModes;

#[derive(Debug, Deserialize)]
pub struct AdminListQuery {
    pub query: Option<String>,
    pub status: Option<String>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct AdminUser {
    pub id: String,
    pub username: String,
    pub status: UserStatus,
    pub created_at: String,
    pub updated_at: String,
    pub last_login_at: Option<String>,
    pub active_session_count: i64,
    pub issue_count: i64,
    pub storage_bytes: i64,
}

#[derive(Debug, Serialize)]
pub struct AdminUserPage {
    pub items: Vec<AdminUser>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChangeStatus {
    pub status: String,
}
#[derive(Debug, Serialize)]
pub struct RevokedSessions {
    pub revoked_sessions: u64,
}

#[derive(Debug, Serialize)]
pub struct RegistrationSettings {
    pub allow_registration: i64,
    pub updated_at: String,
    pub updated_by_username: Option<String>,
    pub login_ip_limit_per_minute: i64,
    pub login_username_failure_limit_per_5_minutes: i64,
    pub issue_inactive_days: i64,
    pub cleanup_exempt_usernames: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRegistrationSettings {
    /// Decimal string in the v2 API. Numbers are accepted for backwards
    /// compatibility with early clients, but responses always use strings.
    pub expected_revision: Option<serde_json::Value>,
    pub changes: Option<serde_json::Map<String, serde_json::Value>>,
    pub modes: Option<AdaptiveModes>,
    pub allow_registration: Option<bool>,
    pub login_ip_limit_per_minute: Option<usize>,
    pub login_username_failure_limit_per_5_minutes: Option<usize>,
    pub issue_inactive_days: Option<serde_json::Value>,
    /// Outer `None` means omitted; inner `None` means an explicit JSON null.
    pub cleanup_exempt_usernames: Option<Option<Vec<String>>>,
}

#[derive(Debug, Serialize)]
pub struct AuthRateLimitEntry {
    pub key: String,
    pub username: Option<String>,
    pub ip: Option<String>,
    pub current_count: usize,
    pub limit: usize,
    pub window_seconds: u64,
    pub last_event_at: Option<String>,
    pub retry_after_seconds: u64,
    pub limited: bool,
}

#[derive(Debug, Deserialize)]
pub struct AuditListQuery {
    pub action: Option<String>,
    pub target_user_id: Option<String>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct AuditLog {
    pub id: String,
    pub actor_type: String,
    pub actor_user_id: Option<String>,
    pub target_user_id: Option<String>,
    pub target_username: Option<String>,
    pub action: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct AuditLogPage {
    pub items: Vec<AuditLog>,
    pub next_cursor: Option<String>,
}
