pub(crate) fn json_string_encoded_len(value: &str) -> u64 {
    let mut length = 2_u64;
    for byte in value.bytes() {
        length = length.saturating_add(match byte {
            b'"' | b'\\' | 0x08..=0x0D => 2,
            0x00..=0x1F => 6,
            _ => 1,
        });
    }
    length
}

pub(crate) fn json_optional_string_encoded_len(value: Option<&str>) -> u64 {
    value.map(json_string_encoded_len).unwrap_or(4)
}

#[cfg(test)]
mod tests {
    use super::{json_optional_string_encoded_len, json_string_encoded_len};

    #[test]
    fn counts_json_escaping_without_allocating() {
        assert_eq!(json_string_encoded_len("plain"), 7);
        assert_eq!(json_string_encoded_len("\"\\"), 6);
        assert_eq!(json_string_encoded_len("\0\n"), 10);
        assert_eq!(json_string_encoded_len("中文"), 8);
        assert_eq!(json_optional_string_encoded_len(None), 4);
    }
}
