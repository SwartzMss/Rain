use serde::{Deserialize, Serialize};

use crate::auth::{AuthenticatedUser, UserRole};

#[derive(Debug, Deserialize)]
pub struct CredentialsRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Serialize)]
pub struct PublicUser {
    pub id: String,
    pub username: String,
    pub role: UserRole,
}

impl From<AuthenticatedUser> for PublicUser {
    fn from(user: AuthenticatedUser) -> Self {
        Self {
            id: user.id,
            username: user.username,
            role: user.role,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AuthMeResponse {
    pub authenticated: bool,
    pub user: Option<PublicUser>,
}
