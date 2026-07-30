use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "UPPERCASE")]
#[serde(rename_all = "UPPERCASE")]
pub enum UserRole {
    User,
    Admin,
}

impl fmt::Display for UserRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::User => "USER",
            Self::Admin => "ADMIN",
        })
    }
}

impl FromStr for UserRole {
    type Err = ();
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "USER" => Ok(Self::User),
            "ADMIN" => Ok(Self::Admin),
            _ => Err(()),
        }
    }
}
