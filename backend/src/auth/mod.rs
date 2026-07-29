pub mod extractor;
pub mod password;
pub mod session;

#[derive(Debug, Clone, serde::Serialize)]
pub struct AuthenticatedUser {
    pub id: String,
    pub username: String,
}
