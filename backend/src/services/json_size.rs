pub(crate) const RESPONSE_TRUNCATED_LINE_MARKER: &str = " ... [response truncated]";

pub(crate) enum JsonLinePageDecision {
    Include {
        content: String,
        line_bytes: u64,
        response_truncated: bool,
    },
    Defer,
    TooLarge,
}

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

pub(crate) fn fit_json_line_to_page(
    value: &str,
    fixed_line_bytes: u64,
    page_base_bytes: u64,
    page_bytes: u64,
    max_page_bytes: u64,
    marker: &str,
) -> JsonLinePageDecision {
    let full_line_bytes = fixed_line_bytes.saturating_add(json_string_encoded_len(value));
    let remaining_page_bytes = max_page_bytes.saturating_sub(page_bytes);

    if full_line_bytes <= remaining_page_bytes {
        return JsonLinePageDecision::Include {
            content: value.to_owned(),
            line_bytes: full_line_bytes,
            response_truncated: false,
        };
    }

    // A line that fits on an empty page belongs on the next page intact. Only
    // a line that cannot fit on any page may be represented by a bounded
    // response prefix.
    let empty_page_remaining = max_page_bytes.saturating_sub(page_base_bytes);
    if full_line_bytes <= empty_page_remaining {
        return JsonLinePageDecision::Defer;
    }

    // Do not expose a partially written truncation marker. A page budget that
    // cannot hold the marker plus the fixed line fields cannot represent this
    // line safely and must be reported as an unrepresentable page.
    if fixed_line_bytes.saturating_add(json_string_encoded_len(marker)) > empty_page_remaining {
        return JsonLinePageDecision::TooLarge;
    }

    let content_budget = empty_page_remaining.saturating_sub(fixed_line_bytes);
    let content = truncate_json_string_to_budget(value, content_budget, marker);
    let line_bytes = fixed_line_bytes.saturating_add(json_string_encoded_len(&content));
    if line_bytes > max_page_bytes {
        JsonLinePageDecision::TooLarge
    } else if line_bytes > remaining_page_bytes {
        // Even a bounded representation should start on the next page when
        // the current page cannot hold that representation. The truncation is
        // still based on an empty page, so the marker remains complete.
        JsonLinePageDecision::Defer
    } else {
        JsonLinePageDecision::Include {
            content,
            line_bytes,
            response_truncated: true,
        }
    }
}

pub(crate) fn json_optional_string_encoded_len(value: Option<&str>) -> u64 {
    value.map(json_string_encoded_len).unwrap_or(4)
}

#[cfg(test)]
mod tests {
    use super::{
        JsonLinePageDecision, RESPONSE_TRUNCATED_LINE_MARKER, fit_json_line_to_page,
        json_optional_string_encoded_len, json_string_encoded_len,
    };

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

    #[test]
    fn defers_a_line_that_fits_on_an_empty_page() {
        let decision =
            fit_json_line_to_page("medium", 100, 0, 190, 256, " ... [response truncated]");
        assert!(matches!(decision, JsonLinePageDecision::Defer));
    }

    #[test]
    fn truncates_only_a_line_that_exceeds_an_empty_page() {
        let decision = fit_json_line_to_page(
            &"\"".repeat(200),
            100,
            0,
            0,
            256,
            RESPONSE_TRUNCATED_LINE_MARKER,
        );
        let JsonLinePageDecision::Include {
            content,
            response_truncated,
            line_bytes,
        } = decision
        else {
            panic!("oversized line should be represented");
        };
        assert!(response_truncated);
        assert!(content.ends_with(RESPONSE_TRUNCATED_LINE_MARKER));
        assert!(line_bytes <= 256);
    }

    #[test]
    fn rejects_a_page_that_cannot_hold_the_complete_marker() {
        let decision =
            fit_json_line_to_page("oversized", 100, 0, 0, 100, RESPONSE_TRUNCATED_LINE_MARKER);
        assert!(matches!(decision, JsonLinePageDecision::TooLarge));
    }
}
