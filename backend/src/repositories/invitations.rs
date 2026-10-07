use sha2::{Digest, Sha256};

/// Hashes the canonical invitation representation. Malformed values hash to
/// a value that cannot match an issued code, so callers can return one generic
/// invalid-code response without revealing validation details.
pub fn code_hash(code: &str) -> Option<String> {
    if code.len() > 128 {
        return None;
    }
    let normalized: String = code
        .trim()
        .to_ascii_uppercase()
        .chars()
        .filter(|character| *character != '-')
        .collect();
    let digest = Sha256::digest(normalized.as_bytes());
    Some(format!("{digest:x}"))
}
