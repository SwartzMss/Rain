use actix_web::cookie::{Cookie, SameSite, time::Duration};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{RngCore, rngs::OsRng};
use sha2::{Digest, Sha256};

pub const SESSION_COOKIE_NAME: &str = "rain_session";

pub fn generate_session_token() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn hash_session_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn session_cookie(token: String, ttl_seconds: u64) -> Cookie<'static> {
    Cookie::build(SESSION_COOKIE_NAME, token)
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(Duration::seconds(ttl_seconds.min(i64::MAX as u64) as i64))
        .finish()
}

pub fn cleared_session_cookie() -> Cookie<'static> {
    Cookie::build(SESSION_COOKIE_NAME, "")
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(Duration::ZERO)
        .finish()
}

#[cfg(test)]
mod tests {
    use super::{generate_session_token, hash_session_token};

    #[test]
    fn generates_opaque_tokens_and_stable_hashes() {
        let token = generate_session_token();
        assert!(token.len() >= 43);
        let hash = hash_session_token(&token);
        assert_eq!(hash.len(), 64);
        assert_ne!(token, hash);
        assert_eq!(hash, hash_session_token(&token));
    }
}
