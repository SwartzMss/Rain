use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "UPPERCASE")]
#[serde(rename_all = "UPPERCASE")]
pub enum UserStatus {
    Active,
    Disabled,
}

impl fmt::Display for UserStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Active => "ACTIVE",
            Self::Disabled => "DISABLED",
        })
    }
}

impl FromStr for UserStatus {
    type Err = ();
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "ACTIVE" => Ok(Self::Active),
            "DISABLED" => Ok(Self::Disabled),
            _ => Err(()),
        }
    }
}
