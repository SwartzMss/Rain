use chrono::{Duration, NaiveDateTime};
use serde::Deserialize;
use thiserror::Error;

use super::wall_clock;

pub const MAX_CONTEXT_EXPANSION_MINUTES: i64 = 15;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TimeScopeInput {
    pub start: Option<String>,
    pub end: Option<String>,
    pub incident_time: Option<String>,
    pub before_minutes: Option<i64>,
    pub after_minutes: Option<i64>,
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

    let has_range_fields = input.start.is_some() || input.end.is_some();
    let has_incident_fields = input.incident_time.is_some()
        || input.before_minutes.is_some()
        || input.after_minutes.is_some();

    let (start, end) = if has_incident_fields {
        if has_range_fields
            || input.incident_time.is_none()
            || input.before_minutes.is_none()
            || input.after_minutes.is_none()
        {
            return Err(TimeScopeError::InvalidTimestamp);
        }

        let incident = parse_timestamp(input.incident_time.as_deref())?;
        let before_minutes = input.before_minutes.unwrap();
        let after_minutes = input.after_minutes.unwrap();
        if before_minutes < 0 || after_minutes < 0 {
            return Err(TimeScopeError::InvalidExpansion);
        }

        let before = Duration::try_minutes(before_minutes)
            .ok_or(TimeScopeError::ArithmeticOverflow)?;
        let after = Duration::try_minutes(after_minutes)
            .ok_or(TimeScopeError::ArithmeticOverflow)?;
        let start = incident
            .checked_sub_signed(before)
            .ok_or(TimeScopeError::ArithmeticOverflow)?;
        let end = incident
            .checked_add_signed(after)
            .ok_or(TimeScopeError::ArithmeticOverflow)?;
        (start, end)
    } else {
        (
            parse_timestamp(input.start.as_deref())?,
            parse_timestamp(input.end.as_deref())?,
        )
    };

    if start >= end {
        return Err(TimeScopeError::InvalidRange);
    }

    if end.signed_duration_since(start) > Duration::hours(24) {
        return Err(TimeScopeError::TooLarge);
    }

    let start_ms = wall_clock::comparison_key(start).ok_or(TimeScopeError::ArithmeticOverflow)?;
    let end_ms = wall_clock::comparison_key(end).ok_or(TimeScopeError::ArithmeticOverflow)?;

    Ok(Some(SkillTimeScope {
        start: wall_clock::format(start),
        end: wall_clock::format(end),
        start_ms,
        end_ms,
    }))
}

impl SkillTimeScope {
    pub fn expanded(&self, minutes: i64) -> Result<Self, TimeScopeError> {
        if !(0..=MAX_CONTEXT_EXPANSION_MINUTES).contains(&minutes) {
            return Err(TimeScopeError::InvalidExpansion);
        }

        let start = wall_clock::parse(&self.start).ok_or(TimeScopeError::InvalidTimestamp)?;
        let end = wall_clock::parse(&self.end).ok_or(TimeScopeError::InvalidTimestamp)?;
        let expansion = Duration::try_minutes(minutes)
            .ok_or(TimeScopeError::ArithmeticOverflow)?;
        let start = start
            .checked_sub_signed(expansion)
            .ok_or(TimeScopeError::ArithmeticOverflow)?;
        let end = end
            .checked_add_signed(expansion)
            .ok_or(TimeScopeError::ArithmeticOverflow)?;
        let start_ms = wall_clock::comparison_key(start).ok_or(TimeScopeError::ArithmeticOverflow)?;
        let end_ms = wall_clock::comparison_key(end).ok_or(TimeScopeError::ArithmeticOverflow)?;

        Ok(Self {
            start: wall_clock::format(start),
            end: wall_clock::format(end),
            start_ms,
            end_ms,
        })
    }
}

fn parse_timestamp(value: Option<&str>) -> Result<NaiveDateTime, TimeScopeError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Err(TimeScopeError::InvalidTimestamp);
    };

    wall_clock::parse(value).ok_or(TimeScopeError::InvalidTimestamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_input() -> TimeScopeInput {
        TimeScopeInput {
            start: Some("2026-08-14 09:27:15.123".into()),
            end: Some("2026-08-14T09:37:15.456".into()),
            ..Default::default()
        }
    }

    #[test]
    fn preserves_local_wall_clock_values_without_timezone_normalization() {
        let scope = parse_time_scope(Some(TimeScopeInput {
            start: Some("2026-08-14 09:27:15.123".into()),
            end: Some("2026-08-14T09:37:15.456".into()),
            ..Default::default()
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
            ..Default::default()
        }))
        .unwrap()
        .unwrap();

        assert_eq!(scope.start, "2026-08-14 09:27:00");
        assert_eq!(scope.end, "2026-08-14 09:37:00");
    }

    #[test]
    fn canonicalizes_incident_time_into_a_wall_clock_window() {
        let input: TimeScopeInput = serde_json::from_value(serde_json::json!({
            "incident_time": "2026-08-14T00:00:00.9999",
            "before_minutes": 5,
            "after_minutes": 10,
        }))
        .unwrap();

        let scope = parse_time_scope(Some(input)).unwrap().unwrap();

        assert_eq!(scope.start, "2026-08-13 23:55:00.999");
        assert_eq!(scope.end, "2026-08-14 00:10:00.999");
        assert!(scope.start_ms < scope.end_ms);
    }

    #[test]
    fn incident_window_requires_non_negative_minutes_and_respects_24_hour_limit() {
        let negative = serde_json::from_value::<TimeScopeInput>(serde_json::json!({
            "incident_time": "2026-08-14 09:00:00",
            "before_minutes": -1,
            "after_minutes": 1,
        }))
        .unwrap();
        assert!(matches!(
            parse_time_scope(Some(negative)),
            Err(TimeScopeError::InvalidExpansion)
        ));

        let too_large = serde_json::from_value::<TimeScopeInput>(serde_json::json!({
            "incident_time": "2026-08-14 09:00:00",
            "before_minutes": 1_440,
            "after_minutes": 1,
        }))
        .unwrap();
        assert!(matches!(
            parse_time_scope(Some(too_large)),
            Err(TimeScopeError::TooLarge)
        ));
    }

    #[test]
    fn incident_window_rejects_i64_max_minutes_without_panicking() {
        let input: TimeScopeInput = serde_json::from_value(serde_json::json!({
            "incident_time": "2026-08-14 09:00:00",
            "before_minutes": i64::MAX,
            "after_minutes": 0,
        }))
        .unwrap();

        assert!(matches!(
            parse_time_scope(Some(input)),
            Err(TimeScopeError::ArithmeticOverflow)
        ));
    }

    #[test]
    fn range_and_incident_fields_cannot_be_mixed() {
        let input: TimeScopeInput = serde_json::from_value(serde_json::json!({
            "start": "2026-08-14 09:00:00",
            "end": "2026-08-14 09:01:00",
            "incident_time": "2026-08-14 09:00:30",
            "before_minutes": 1,
            "after_minutes": 1,
        }))
        .unwrap();

        assert!(matches!(
            parse_time_scope(Some(input)),
            Err(TimeScopeError::InvalidTimestamp)
        ));
    }

    #[test]
    fn rejects_ranges_that_collapse_to_the_same_millisecond_key() {
        let result = parse_time_scope(Some(TimeScopeInput {
            start: Some("2026-08-14 09:27:15.1231".into()),
            end: Some("2026-08-14 09:27:15.1239".into()),
            ..Default::default()
        }));

        assert!(matches!(result, Err(TimeScopeError::InvalidRange)));
    }

    #[test]
    fn comparison_key_is_monotonic_across_calendar_boundaries() {
        let scope = parse_time_scope(Some(TimeScopeInput {
            start: Some("2026-08-14 23:59:59.999".into()),
            end: Some("2026-08-15 00:00:00.001".into()),
            ..Default::default()
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
            ..Default::default()
        }));

        assert!(matches!(invalid, Err(TimeScopeError::InvalidTimestamp)));
    }

    #[test]
    fn rejects_missing_or_empty_timestamp_endpoints() {
        for input in [
            TimeScopeInput {
                start: None,
                end: Some("2026-08-14 09:37:15".into()),
                ..Default::default()
            },
            TimeScopeInput {
                start: Some("2026-08-14 09:27:15".into()),
                end: None,
                ..Default::default()
            },
            TimeScopeInput {
                start: Some("  ".into()),
                end: Some("2026-08-14 09:37:15".into()),
                ..Default::default()
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
            ..Default::default()
        }));
        assert!(matches!(reversed, Err(TimeScopeError::InvalidRange)));

        let too_large = parse_time_scope(Some(TimeScopeInput {
            start: Some("2026-08-14 09:00:00".into()),
            end: Some("2026-08-15 09:00:01".into()),
            ..Default::default()
        }));
        assert!(matches!(too_large, Err(TimeScopeError::TooLarge)));
    }

    #[test]
    fn rejects_timezone_bearing_values_instead_of_interpreting_them_as_absolute_time() {
        let invalid = parse_time_scope(Some(TimeScopeInput {
            start: Some("2026-08-14T09:27:15+08:00".into()),
            end: Some("2026-08-14T09:37:15+08:00".into()),
            ..Default::default()
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
