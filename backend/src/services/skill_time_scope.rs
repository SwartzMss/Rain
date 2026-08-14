use chrono::{Datelike, Duration, NaiveDateTime, Timelike};
use serde::Deserialize;
use thiserror::Error;

pub const MAX_CONTEXT_EXPANSION_MINUTES: i64 = 15;
const MILLIS_PER_SECOND: i64 = 1_000;

#[derive(Debug, Clone, Deserialize)]
pub struct TimeScopeInput {
    pub start: Option<String>,
    pub end: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillTimeScope {
    pub start: String,
    pub end: String,
    /// Legacy database-compatible field name. This is a packed wall-clock
    /// comparison key, not Unix time, elapsed milliseconds, or a UTC value.
    pub start_ms: i64,
    /// Legacy database-compatible field name. This is a packed wall-clock
    /// comparison key, not Unix time, elapsed milliseconds, or a UTC value.
    pub end_ms: i64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TimeScopeError {
    #[error("time scope timestamps must be valid local wall-clock values")]
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

    if start >= end {
        return Err(TimeScopeError::InvalidRange);
    }

    if end.signed_duration_since(start) > Duration::hours(24) {
        return Err(TimeScopeError::TooLarge);
    }

    let start_ms = wall_clock_comparison_key(start)?;
    let end_ms = wall_clock_comparison_key(end)?;

    Ok(Some(SkillTimeScope {
        start: format_wall_clock(start),
        end: format_wall_clock(end),
        start_ms,
        end_ms,
    }))
}

impl SkillTimeScope {
    pub fn expanded(&self, minutes: i64) -> Result<Self, TimeScopeError> {
        if !(0..=MAX_CONTEXT_EXPANSION_MINUTES).contains(&minutes) {
            return Err(TimeScopeError::InvalidExpansion);
        }

        let start = parse_timestamp(Some(&self.start))?;
        let end = parse_timestamp(Some(&self.end))?;
        let expansion = Duration::minutes(minutes);
        let start = start
            .checked_sub_signed(expansion)
            .ok_or(TimeScopeError::ArithmeticOverflow)?;
        let end = end
            .checked_add_signed(expansion)
            .ok_or(TimeScopeError::ArithmeticOverflow)?;
        let start_ms = wall_clock_comparison_key(start)?;
        let end_ms = wall_clock_comparison_key(end)?;

        Ok(Self {
            start: format_wall_clock(start),
            end: format_wall_clock(end),
            start_ms,
            end_ms,
        })
    }
}

fn parse_timestamp(value: Option<&str>) -> Result<NaiveDateTime, TimeScopeError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Err(TimeScopeError::InvalidTimestamp);
    };

    let parsed = [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
    ]
    .into_iter()
    .find_map(|format| NaiveDateTime::parse_from_str(value.trim(), format).ok())
    .ok_or(TimeScopeError::InvalidTimestamp)?;
    parsed
        .with_nanosecond((parsed.nanosecond() / 1_000_000) * 1_000_000)
        .ok_or(TimeScopeError::InvalidTimestamp)
}

fn format_wall_clock(value: NaiveDateTime) -> String {
    let base = value.format("%Y-%m-%d %H:%M:%S").to_string();
    let millis = value.nanosecond() / 1_000_000;
    if millis == 0 {
        return base;
    }

    format!("{base}.{millis:03}")
}

/// Encodes calendar fields into a sortable integer for wall-clock comparisons.
/// The legacy *_ms storage names are retained, but this value has no Unix
/// epoch, timezone, or UTC meaning and must only be compared with like keys.
fn wall_clock_comparison_key(value: NaiveDateTime) -> Result<i64, TimeScopeError> {
    let date = value.date();
    let time = value.time();
    let millis = i64::from(time.nanosecond() / 1_000_000);
    let mut key = i64::from(date.year());
    for (base, component) in [
        (13_i64, i64::from(date.month())),
        (32, i64::from(date.day())),
        (24, i64::from(time.hour())),
        (60, i64::from(time.minute())),
        (60, i64::from(time.second())),
        (MILLIS_PER_SECOND, millis),
    ] {
        key = key
            .checked_mul(base)
            .and_then(|value| value.checked_add(component))
            .ok_or(TimeScopeError::ArithmeticOverflow)?;
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_input() -> TimeScopeInput {
        TimeScopeInput {
            start: Some("2026-08-14 09:27:15.123".into()),
            end: Some("2026-08-14T09:37:15.456".into()),
        }
    }

    #[test]
    fn preserves_local_wall_clock_values_without_timezone_normalization() {
        let scope = parse_time_scope(Some(TimeScopeInput {
            start: Some("2026-08-14 09:27:15.123".into()),
            end: Some("2026-08-14T09:37:15.456".into()),
        }))
        .unwrap()
        .unwrap();

        assert_eq!(scope.start, "2026-08-14 09:27:15.123");
        assert_eq!(scope.end, "2026-08-14 09:37:15.456");
        assert!(scope.start_ms < scope.end_ms);
    }

    #[test]
    fn accepts_frontend_datetime_local_minute_precision() {
        let scope = parse_time_scope(Some(TimeScopeInput {
            start: Some("2026-08-14T09:27".into()),
            end: Some("2026-08-14T09:37".into()),
        }))
        .unwrap()
        .unwrap();

        assert_eq!(scope.start, "2026-08-14 09:27:00");
        assert_eq!(scope.end, "2026-08-14 09:37:00");
    }

    #[test]
    fn rejects_ranges_that_collapse_to_the_same_millisecond_key() {
        let result = parse_time_scope(Some(TimeScopeInput {
            start: Some("2026-08-14 09:27:15.1231".into()),
            end: Some("2026-08-14 09:27:15.1239".into()),
        }));

        assert!(matches!(result, Err(TimeScopeError::InvalidRange)));
    }

    #[test]
    fn comparison_key_is_monotonic_across_calendar_boundaries() {
        let scope = parse_time_scope(Some(TimeScopeInput {
            start: Some("2026-08-14 23:59:59.999".into()),
            end: Some("2026-08-15 00:00:00.001".into()),
        }))
        .unwrap()
        .unwrap();

        assert!(scope.start_ms < scope.end_ms);
    }

    #[test]
    fn rejects_invalid_timestamps() {
        let invalid = parse_time_scope(Some(TimeScopeInput {
            start: Some("not-a-timestamp".into()),
            end: Some("2026-08-14 09:37:15".into()),
        }));

        assert!(matches!(invalid, Err(TimeScopeError::InvalidTimestamp)));
    }

    #[test]
    fn rejects_missing_or_empty_timestamp_endpoints() {
        for input in [
            TimeScopeInput {
                start: None,
                end: Some("2026-08-14 09:37:15".into()),
            },
            TimeScopeInput {
                start: Some("2026-08-14 09:27:15".into()),
                end: None,
            },
            TimeScopeInput {
                start: Some("  ".into()),
                end: Some("2026-08-14 09:37:15".into()),
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
            start: Some("2026-08-14 10:00:00".into()),
            end: Some("2026-08-14 09:00:00".into()),
        }));
        assert!(matches!(reversed, Err(TimeScopeError::InvalidRange)));

        let too_large = parse_time_scope(Some(TimeScopeInput {
            start: Some("2026-08-14 09:00:00".into()),
            end: Some("2026-08-15 09:00:01".into()),
        }));
        assert!(matches!(too_large, Err(TimeScopeError::TooLarge)));
    }

    #[test]
    fn rejects_timezone_bearing_values_instead_of_interpreting_them_as_absolute_time() {
        let invalid = parse_time_scope(Some(TimeScopeInput {
            start: Some("2026-08-14T09:27:15+08:00".into()),
            end: Some("2026-08-14T09:37:15+08:00".into()),
        }));

        assert!(matches!(invalid, Err(TimeScopeError::InvalidTimestamp)));
    }

    #[test]
    fn none_means_unscoped_and_expansion_is_limited() {
        assert_eq!(parse_time_scope(None).unwrap(), None);
        let scope = parse_time_scope(Some(valid_input())).unwrap().unwrap();
        let expanded = scope.expanded(15).unwrap();
        assert!(expanded.start < scope.start);
        assert!(expanded.end > scope.end);
        assert!(expanded.start_ms < scope.start_ms);
        assert!(expanded.end_ms > scope.end_ms);
        assert!(scope.expanded(16).is_err());
    }
}
