use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{RngCore, rngs::OsRng};

use crate::error::AppError;

const ENVELOPE_VERSION: &str = "v1";

pub struct SecretCipher {
    key: [u8; 32],
}

impl SecretCipher {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    pub fn encrypt(&self, plaintext: &str) -> Result<String, AppError> {
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|_| AppError::Config("invalid AI secret encryption key".into()))?;
        let mut nonce_bytes = [0_u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_bytes())
            .map_err(|_| AppError::Config("failed to encrypt AI provider secret".into()))?;
        Ok(format!(
            "{ENVELOPE_VERSION}:{}:{}",
            URL_SAFE_NO_PAD.encode(nonce_bytes),
            URL_SAFE_NO_PAD.encode(ciphertext)
        ))
    }

    pub fn decrypt(&self, envelope: &str) -> Result<String, AppError> {
        let mut parts = envelope.split(':');
        let version = parts.next();
        let nonce = parts.next();
        let ciphertext = parts.next();
        if version != Some(ENVELOPE_VERSION)
            || nonce.is_none()
            || ciphertext.is_none()
            || parts.next().is_some()
        {
            return Err(AppError::Config(
                "invalid AI provider secret envelope".into(),
            ));
        }
        let nonce = URL_SAFE_NO_PAD
            .decode(nonce.unwrap())
            .map_err(|_| AppError::Config("invalid AI provider secret envelope".into()))?;
        let nonce: [u8; 12] = nonce
            .try_into()
            .map_err(|_| AppError::Config("invalid AI provider secret envelope".into()))?;
        let ciphertext = URL_SAFE_NO_PAD
            .decode(ciphertext.unwrap())
            .map_err(|_| AppError::Config("invalid AI provider secret envelope".into()))?;
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|_| AppError::Config("invalid AI secret encryption key".into()))?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
            .map_err(|_| AppError::Config("unable to decrypt AI provider secret".into()))?;
        String::from_utf8(plaintext)
            .map_err(|_| AppError::Config("invalid AI provider secret text".into()))
    }
}
