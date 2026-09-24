use chrono::{Datelike, NaiveDateTime, Timelike};

/// Parses a local log wall-clock value and truncates precision to milliseconds.
///
/// This deliberately does not accept timezone-bearing values. The returned
/// value has no UTC or Unix-time interpretation.
pub(crate) fn parse(value: &str) -> Option<NaiveDateTime> {
    let value = value.trim().replace(',', ".");
    [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
    ]
    .into_iter()
    .find_map(|format| NaiveDateTime::parse_from_str(&value, format).ok())
    .and_then(|datetime| datetime.with_nanosecond((datetime.nanosecond() / 1_000_000) * 1_000_000))
}

/// Encodes calendar fields into a sortable wall-clock comparison key.
///
/// The legacy database `*_ms` columns retain their names, but this value is
/// not Unix time, elapsed milliseconds, or a UTC timestamp. It is only valid
/// for comparisons with keys produced by this same function.
pub(crate) fn comparison_key(value: NaiveDateTime) -> Option<i64> {
    let date = value.date();
    let time = value.time();
    let components = [
        (13_i64, i64::from(date.month())),
        (32, i64::from(date.day())),
        (24, i64::from(time.hour())),
        (60, i64::from(time.minute())),
        (60, i64::from(time.second())),
        (1_000, i64::from(time.nanosecond() / 1_000_000)),
    ];

    components
        .into_iter()
        .try_fold(i64::from(date.year()), |key, (base, component)| {
            key.checked_mul(base)?.checked_add(component)
        })
}
