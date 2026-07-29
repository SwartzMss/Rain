use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use once_cell::sync::Lazy;
use regex::Regex;
use thiserror::Error;

static USERNAME_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[A-Za-z0-9._-]{3,32}$").expect("valid username regex"));
static DUMMY_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZS1maXhlZC1kdW1teS1zYWx0$GzSLOtFquWhNkE8jkbti2JnRZWyLHedKNoDm46kwB9o";

#[derive(Debug, Error)]
pub enum PasswordError {
    #[error("用户名格式无效")]
    InvalidUsername,
    #[error("密码长度必须为 8 到 128 个字符")]
    InvalidPassword,
    #[error("password hashing failed")]
    Hashing,
}

pub fn normalize_username(username: &str) -> String {
    username.to_ascii_lowercase()
}

pub fn validate_username(username: &str) -> Result<(), PasswordError> {
    if USERNAME_PATTERN.is_match(username) {
        Ok(())
    } else {
        Err(PasswordError::InvalidUsername)
    }
}

pub fn validate_password(password: &str) -> Result<(), PasswordError> {
    if (8..=128).contains(&password.chars().count()) {
        Ok(())
    } else {
        Err(PasswordError::InvalidPassword)
    }
}

pub fn hash_password(password: &str) -> Result<String, PasswordError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| PasswordError::Hashing)
}

pub fn verify_password(password: &str, encoded: &str) -> Result<bool, PasswordError> {
    let hash = PasswordHash::new(encoded).map_err(|_| PasswordError::Hashing)?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &hash)
        .is_ok())
}

pub fn verify_dummy_password(password: &str) -> Result<bool, PasswordError> {
    verify_password(password, DUMMY_PASSWORD_HASH)
}

#[cfg(test)]
mod tests {
    use super::{
        hash_password, normalize_username, validate_password, validate_username, verify_password,
    };

    #[test]
    fn validates_and_normalizes_credentials() {
        assert_eq!(normalize_username("Swartz"), "swartz");
        assert!(validate_username("abc").is_ok());
        assert!(validate_username("ab").is_err());
        assert!(validate_username("用户名").is_err());
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password("1234567").is_err());
    }

    #[test]
    fn hashes_and_verifies_passwords_with_argon2id() {
        let hash = hash_password("password123").expect("hash");
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("password123", &hash).expect("verify"));
        assert!(!verify_password("wrong-password", &hash).expect("verify"));
    }
}
