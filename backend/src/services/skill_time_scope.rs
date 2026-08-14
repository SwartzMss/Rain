use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use serde::Deserialize;
use thiserror::Error;

pub const MAX_SCOPE_MILLIS: i64 = 24 * 60 * 60 * 1000;
pub const MAX_CONTEXT_EXPANSION_MINUTES: i64 = 15;
const MILLIS_PER_MINUTE: i64 = 60 * 1000;

#[derive(Debug, Clone, Deserialize)]
pub struct TimeScopeInput {
    pub start: Option<String>,
    pub end: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillTimeScope {
    pub start: String,
    pub end: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TimeScopeError {
    #[error("time scope timestamps must be valid RFC3339 values")]
    InvalidTimestamp,
    #[error("time scope start must be before end")]
    InvalidRange,
    #[error("time scope cannot exceed 24 hours")]
    TooLarge,
    #[error("context expansion must be between 0 and 15 minutes")]
    InvalidExpansion,
    #[error("time scope expansion overflowed the supported timestamp range")]
    ArithmeticOverflow,
}

pub fn parse_time_scope(
    input: Option<TimeScopeInput>,
) -> Result<Option<SkillTimeScope>, TimeScopeError> {
    let Some(input) = input else {
        return Ok(None);
    };

    let start = parse_timestamp(input.start.as_deref())?;
    let end = parse_timestamp(input.end.as_deref())?;
    let start_ms = start.timestamp_millis();
    let end_ms = end.timestamp_millis();

    if start_ms >= end_ms {
        return Err(TimeScopeError::InvalidRange);
    }

    let duration = end_ms
        .checked_sub(start_ms)
        .ok_or(TimeScopeError::ArithmeticOverflow)?;
    if duration > MAX_SCOPE_MILLIS {
        return Err(TimeScopeError::TooLarge);
    }

    Ok(Some(SkillTimeScope {
        start: format_timestamp(start),
        end: format_timestamp(end),
        start_ms,
        end_ms,
    }))
}

impl SkillTimeScope {
    pub fn expanded(&self, minutes: i64) -> Result<Self, TimeScopeError> {
        if !(0..=MAX_CONTEXT_EXPANSION_MINUTES).contains(&minutes) {
            return Err(TimeScopeError::InvalidExpansion);
        }

        let expansion_ms = minutes
            .checked_mul(MILLIS_PER_MINUTE)
            .ok_or(TimeScopeError::ArithmeticOverflow)?;
        let start_ms = self
            .start_ms
            .checked_sub(expansion_ms)
            .ok_or(TimeScopeError::ArithmeticOverflow)?;
        let end_ms = self
            .end_ms
            .checked_add(expansion_ms)
            .ok_or(TimeScopeError::ArithmeticOverflow)?;

        Ok(Self {
            start: format_timestamp_from_millis(start_ms)?,
            end: format_timestamp_from_millis(end_ms)?,
            start_ms,
            end_ms,
        })
    }
}

fn parse_timestamp(value: Option<&str>) -> Result<DateTime<Utc>, TimeScopeError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Err(TimeScopeError::InvalidTimestamp);
    };
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| TimeScopeError::InvalidTimestamp)
}

fn format_timestamp(timestamp: DateTime<Utc>) -> String {
    timestamp.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn format_timestamp_from_millis(millis: i64) -> Result<String, TimeScopeError> {
    Utc.timestamp_millis_opt(millis)
        .single()
        .map(format_timestamp)
        .ok_or(TimeScopeError::ArithmeticOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_input() -> TimeScopeInput {
        TimeScopeInput {
            start: Some("2026-08-14T01:27:15Z".into()),
            end: Some("2026-08-14T01:37:15Z".into()),
        }
    }

    #[test]
    fn canonicalizes_offsets_to_utc_and_milliseconds() {
        let scope = parse_time_scope(Some(TimeScopeInput {
            start: Some("2026-08-14T09:27:15+08:00".into()),
            end: Some("2026-08-14T09:37:15+08:00".into()),
        }))
        .unwrap()
        .unwrap();

        assert_eq!(scope.start, "2026-08-14T01:27:15.000Z");
        assert_eq!(scope.end, "2026-08-14T01:37:15.000Z");
        assert_eq!(scope.end_ms - scope.start_ms, 10 * 60 * 1000);
    }

    #[test]
    fn rejects_invalid_timestamps() {
        let invalid = parse_time_scope(Some(TimeScopeInput {
            start: Some("not-a-timestamp".into()),
            end: Some("2026-08-14T01:37:15Z".into()),
        }));

        assert!(matches!(invalid, Err(TimeScopeError::InvalidTimestamp)));
    }

    #[test]
    fn rejects_missing_or_empty_timestamp_endpoints() {
        for input in [
            TimeScopeInput {
                start: None,
                end: Some("2026-08-14T01:37:15Z".into()),
            },
            TimeScopeInput {
                start: Some("2026-08-14T01:27:15Z".into()),
                end: None,
            },
            TimeScopeInput {
                start: Some("  ".into()),
                end: Some("2026-08-14T01:37:15Z".into()),
            },
        ] {
            assert!(matches!(
                parse_time_scope(Some(input)),
                Err(TimeScopeError::InvalidTimestamp)
            ));
        }
    }

    #[test]
    fn rejects_invalid_order_and_windows_over_24_hours() {
        let reversed = parse_time_scope(Some(TimeScopeInput {
            start: Some("2026-08-14T02:00:00Z".into()),
            end: Some("2026-08-14T01:00:00Z".into()),
        }));
        assert!(matches!(reversed, Err(TimeScopeError::InvalidRange)));

        let too_large = parse_time_scope(Some(TimeScopeInput {
            start: Some("2026-08-14T00:00:00Z".into()),
            end: Some("2026-08-15T00:00:01Z".into()),
        }));
        assert!(matches!(too_large, Err(TimeScopeError::TooLarge)));
    }

    #[test]
    fn none_means_unscoped_and_expansion_is_limited() {
        assert_eq!(parse_time_scope(None).unwrap(), None);
        let scope = parse_time_scope(Some(valid_input())).unwrap().unwrap();
        let expanded = scope.expanded(15).unwrap();
        assert_eq!(expanded.start_ms, scope.start_ms - 15 * 60 * 1000);
        assert_eq!(expanded.end_ms, scope.end_ms + 15 * 60 * 1000);
        assert!(scope.expanded(16).is_err());
    }
}
