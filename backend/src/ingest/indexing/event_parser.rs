const LOG_LEVELS: [&str; 7] = [
    "TRACE", "DEBUG", "INFO", "WARN", "WARNING", "ERROR", "FATAL",
];

pub(crate) struct ParsedLogEvent {
    pub(crate) timestamp: Option<String>,
    pub(crate) level: Option<String>,
    pub(crate) component: Option<String>,
    pub(crate) message: String,
    pub(crate) parser_confidence: f64,
}

pub(crate) fn parse_log_event(line: &str) -> Option<ParsedLogEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (timestamp, after_timestamp, timestamp_confidence) = split_timestamp(trimmed);
    let (level, after_level) = split_level(after_timestamp);
    if timestamp.is_none() && level.is_none() {
        return None;
    }

    let (component, message) = split_component(after_level.trim());
    let parser_confidence = timestamp_confidence
        + if level.is_some() { 0.35 } else { 0.0 }
        + if component.is_some() { 0.10 } else { 0.0 };

    Some(ParsedLogEvent {
        timestamp: timestamp.map(str::to_string),
        level: level.map(str::to_string),
        component: component.map(str::to_string),
        message: if message.is_empty() {
            trimmed.to_string()
        } else {
            message.to_string()
        },
        parser_confidence: parser_confidence.min(0.95),
    })
}

pub(crate) fn split_timestamp(line: &str) -> (Option<&str>, &str, f64) {
    if let Some((first, rest)) = line.split_once(' ')
        && looks_like_timestamp(first)
    {
        return (Some(first), rest, 0.45);
    }

    if let Some(candidate) = line.get(..19)
        && looks_like_timestamp(candidate)
    {
        return (
            Some(candidate),
            line.get(19..).unwrap_or("").trim_start(),
            0.45,
        );
    }

    (None, line, 0.0)
}

fn split_level(line: &str) -> (Option<&str>, &str) {
    let trimmed = line.trim_start_matches([' ', '[']);
    for level in LOG_LEVELS {
        if let Some(rest) = trimmed.strip_prefix(level) {
            let rest = rest.trim_start_matches([']', ':', '-', ' ']);
            return (Some(if level == "WARNING" { "WARN" } else { level }), rest);
        }
    }
    (None, line)
}

fn split_component(line: &str) -> (Option<&str>, &str) {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix('[')
        && let Some(end) = rest.find(']')
    {
        let component = rest[..end].trim();
        let message = rest[end + 1..].trim_start_matches([':', '-', ' ']).trim();
        if !component.is_empty() {
            return (Some(component), message);
        }
    }

    if let Some((component, message)) = trimmed.split_once(':') {
        let component = component.trim();
        if !component.is_empty()
            && component.len() <= 64
            && component
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/'))
        {
            return (Some(component), message.trim());
        }
    }

    (None, trimmed)
}

fn looks_like_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() >= 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        return true;
    }

    bytes.len() >= 8
        && bytes[2] == b':'
        && bytes[5] == b':'
        && bytes[..2].iter().all(u8::is_ascii_digit)
        && bytes[3..5].iter().all(u8::is_ascii_digit)
        && bytes[6..8].iter().all(u8::is_ascii_digit)
}
