pub mod extractor;
pub mod password;
mod role;
pub mod same_origin;
pub mod session;
mod status;

pub use role::UserRole;
pub use status::UserStatus;

#[derive(Debug, Clone, serde::Serialize)]
pub struct AuthenticatedUser {
    pub id: String,
    pub username: String,
    pub role: UserRole,
}
