pub(crate) const RESPONSE_TRUNCATED_LINE_MARKER: &str = " ... [response truncated]";

pub(crate) fn json_string_encoded_len(value: &str) -> u64 {
    let mut length = 2_u64;
    for byte in value.bytes() {
        length = length.saturating_add(match byte {
            b'"' | b'\\' | b'\x08' | b'\t' | b'\n' | b'\x0C' | b'\r' => 2,
            0x00..=0x1F => 6,
            _ => 1,
        });
    }
    length
}

fn json_char_content_len(character: char) -> u64 {
    let mut buffer = [0_u8; 4];
    let encoded = character.encode_utf8(&mut buffer);
    json_string_encoded_len(encoded).saturating_sub(2)
}

fn json_string_prefix_to_content_budget(value: &str, budget: u64) -> String {
    let mut result = String::new();
    let mut used = 0_u64;
    for character in value.chars() {
        let cost = json_char_content_len(character);
        if used.saturating_add(cost) > budget {
            break;
        }
        result.push(character);
        used = used.saturating_add(cost);
    }
    result
}

pub(crate) fn truncate_json_string_to_budget(
    value: &str,
    max_encoded_bytes: u64,
    marker: &str,
) -> String {
    if json_string_encoded_len(value) <= max_encoded_bytes {
        return value.to_owned();
    }

    let content_budget = max_encoded_bytes.saturating_sub(2);
    let marker_content_len = json_string_encoded_len(marker).saturating_sub(2);
    let marker = if marker_content_len <= content_budget {
        marker.to_owned()
    } else {
        json_string_prefix_to_content_budget(marker, content_budget)
    };
    let marker_content_len = json_string_encoded_len(&marker).saturating_sub(2);
    let prefix = json_string_prefix_to_content_budget(
        value,
        content_budget.saturating_sub(marker_content_len),
    );
    let mut result = prefix;
    result.push_str(&marker);
    result
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

    #[test]
    fn encoded_length_matches_serde_json_for_ascii_bytes() {
        for byte in 0u8..=0x7f {
            let value = String::from_utf8(vec![byte]).unwrap();
            assert_eq!(
                json_string_encoded_len(&value),
                serde_json::to_string(&value).unwrap().len() as u64,
                "byte = 0x{byte:02x}"
            );
        }
    }

    #[test]
    fn truncates_json_string_to_encoded_budget_with_utf8_boundary() {
        let marker = " ... [response truncated]";
        let value = "中\"".repeat(128);
        let truncated = super::truncate_json_string_to_budget(&value, 64, marker);

        assert!(truncated.ends_with(marker));
        assert!(json_string_encoded_len(&truncated) <= 64);
        assert!(std::str::from_utf8(truncated.as_bytes()).is_ok());
    }

    #[test]
    fn truncation_still_returns_a_bounded_string_when_marker_does_not_fit() {
        let truncated = super::truncate_json_string_to_budget("value", 4, "marker");
        assert!(json_string_encoded_len(&truncated) <= 4);
    }
}
