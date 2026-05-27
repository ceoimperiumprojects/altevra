//! Shared helpers for converting between SQLite TEXT columns and rich Rust
//! types. SQLite has no native UUID or timestamp type so we encode UUIDs as
//! 36-char dashed strings and timestamps as RFC-3339 / ISO-8601 strings.

use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

/// Parse a UUID from a SQLite TEXT cell, falling back to `Uuid::nil()` for
/// malformed data. SQLite never gives us a typed UUID so callers should
/// always round-trip through this helper.
pub fn uuid_from_text(s: impl AsRef<str>) -> Uuid {
    Uuid::parse_str(s.as_ref()).unwrap_or_else(|_| Uuid::nil())
}

/// Parse an optional UUID (NULL column or empty string => None).
pub fn opt_uuid_from_text(s: Option<String>) -> Option<Uuid> {
    let raw = s?;
    if raw.is_empty() {
        return None;
    }
    Uuid::parse_str(&raw).ok()
}

/// Parse a `DateTime<Utc>` from an ISO-8601 / RFC-3339 SQLite TEXT cell. If
/// the value is malformed we fall back to `Utc::now()` to keep the pipeline
/// resilient — corrupt rows are rare and rejecting them would break listing.
pub fn ts_from_text(s: impl AsRef<str>) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s.as_ref())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

/// Optional timestamp parsing — NULL column => None.
pub fn opt_ts_from_text(s: Option<String>) -> Option<DateTime<Utc>> {
    let raw = s?;
    if raw.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(&raw)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

/// Parse a `NaiveDate` (YYYY-MM-DD) from a SQLite TEXT cell.
pub fn opt_date_from_text(s: Option<String>) -> Option<NaiveDate> {
    let raw = s?;
    if raw.is_empty() {
        return None;
    }
    NaiveDate::parse_from_str(&raw, "%Y-%m-%d").ok()
}

/// Encode a `DateTime<Utc>` to the canonical SQLite TEXT format. We use
/// RFC-3339 with millisecond precision to stay friendly to SQLite's
/// `strftime` defaults.
pub fn ts_to_text(ts: &DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Encode a `NaiveDate` to ISO date format (YYYY-MM-DD).
pub fn date_to_text(d: &NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}
