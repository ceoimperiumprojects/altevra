//! Temporal recall windows — the "pre mesec dana" feature.
//!
//! Parses human-friendly window strings (`24h`, `7d`, `30d`, `3mo`, `1y`) and absolute
//! RFC3339 dates into a concrete `(since, until)` UTC range used by `search_turns`
//! and the MCP `recall_window` tool. Pure: no DB, no clock side-effects (caller passes
//! `now` so this is fully deterministic + testable). The parser is intentionally
//! conservative — unknown formats return `None`, never wrong-by-default windows.
//!
//! Examples (with `now = 2026-06-02T00:00:00Z`):
//!   * `parse_window("24h")` → 2026-06-01T00:00:00Z .. 2026-06-02T00:00:00Z
//!   * `parse_window("7d")`  → 2026-05-26T00:00:00Z .. 2026-06-02T00:00:00Z
//!   * `parse_window("3mo")` → 2026-03-02T00:00:00Z .. 2026-06-02T00:00:00Z (≈90d)
//!   * presets `last_24h`, `last_week`, `last_month`, `last_quarter`, `last_year`.

use chrono::{DateTime, Duration, Utc};

/// A concrete time range (`since` inclusive, `until` exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRange {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
}

impl TimeRange {
    /// `now - dur .. now`. Standard "last-N" window.
    pub fn rolling(now: DateTime<Utc>, dur: Duration) -> Self {
        Self {
            since: now - dur,
            until: now,
        }
    }

    /// Inclusive containment: `since <= t < until`.
    pub fn contains(&self, t: DateTime<Utc>) -> bool {
        t >= self.since && t < self.until
    }
}

/// Parse a relative duration string into a chrono `Duration`. Recognized suffixes:
/// `s` (seconds), `m` (minutes), `h` (hours), `d` (days), `w` (weeks),
/// `mo` (~30d), `y` (~365d). Returns `None` for unknown input — fail-closed so a
/// typo never collapses to an unwanted default.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Find the split between number and unit.
    let split = s.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let (num_str, unit) = s.split_at(split);
    let n: i64 = num_str.parse().ok()?;
    if n < 0 {
        return None;
    }
    match unit {
        "s" | "sec" | "secs" | "second" | "seconds" => Some(Duration::seconds(n)),
        "m" | "min" | "mins" | "minute" | "minutes" => Some(Duration::minutes(n)),
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(Duration::hours(n)),
        "d" | "day" | "days" => Some(Duration::days(n)),
        "w" | "wk" | "wks" | "week" | "weeks" => Some(Duration::weeks(n)),
        // mo/y are calendar-fuzzy; we use 30/365-day approximations so windows are
        // deterministic and don't surprise (no month-arithmetic edge cases).
        "mo" | "mon" | "mos" | "month" | "months" => Some(Duration::days(n * 30)),
        "y" | "yr" | "yrs" | "year" | "years" => Some(Duration::days(n * 365)),
        _ => None,
    }
}

/// Parse a window string into a concrete `TimeRange` ending at `now`. Accepts:
///
/// * a duration (`"24h"`, `"30d"`, `"3mo"`) → rolling window ending now
/// * a preset (`"last_24h"`, `"last_week"`, `"last_month"`, `"last_quarter"`, `"last_year"`)
///
/// Returns `None` for unknown input.
pub fn parse_window(s: &str, now: DateTime<Utc>) -> Option<TimeRange> {
    match s.trim() {
        "last_24h" | "last_day" => Some(TimeRange::rolling(now, Duration::hours(24))),
        "last_week" | "last_7d" => Some(TimeRange::rolling(now, Duration::days(7))),
        "last_month" | "last_30d" => Some(TimeRange::rolling(now, Duration::days(30))),
        "last_quarter" | "last_90d" => Some(TimeRange::rolling(now, Duration::days(90))),
        "last_year" | "last_365d" => Some(TimeRange::rolling(now, Duration::days(365))),
        other => parse_duration(other).map(|d| TimeRange::rolling(now, d)),
    }
}

/// Parse an absolute timestamp (RFC3339) OR a relative duration interpreted as
/// "now - duration". Examples: `"2026-05-01T00:00:00Z"`, `"30d"`. This is what
/// `--since` / `--until` flags accept.
pub fn parse_since_until(s: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&Utc));
    }
    // Date-only (YYYY-MM-DD) → start of day UTC.
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return d.and_hms_opt(0, 0, 0).map(|nd| nd.and_utc());
    }
    parse_duration(s).map(|d| now - d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn epoch() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn parses_duration_units() {
        assert_eq!(parse_duration("24h"), Some(Duration::hours(24)));
        assert_eq!(parse_duration("7d"), Some(Duration::days(7)));
        assert_eq!(parse_duration("2w"), Some(Duration::weeks(2)));
        assert_eq!(parse_duration("3mo"), Some(Duration::days(90)));
        assert_eq!(parse_duration("1y"), Some(Duration::days(365)));
        assert_eq!(parse_duration("90minutes"), Some(Duration::minutes(90)));
    }

    #[test]
    fn parse_duration_rejects_garbage() {
        // Fail-closed: a typo never silently maps to a wrong window.
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("garbage"), None);
        assert_eq!(parse_duration("7"), None); // missing unit
        assert_eq!(parse_duration("dh"), None); // missing number
        assert_eq!(parse_duration("-5d"), None); // no negative windows
    }

    #[test]
    fn presets_are_correct_and_end_at_now() {
        let now = epoch();
        let week = parse_window("last_week", now).unwrap();
        assert_eq!(week.until, now);
        assert_eq!(week.since, now - Duration::days(7));
        // Aliases agree.
        assert_eq!(parse_window("last_7d", now), parse_window("last_week", now));
        assert_eq!(
            parse_window("last_month", now),
            parse_window("last_30d", now)
        );
    }

    #[test]
    fn window_accepts_raw_duration() {
        // The user's exact use-case: "pre mesec dana" → 30d window.
        let r = parse_window("30d", epoch()).unwrap();
        assert_eq!(r.since, epoch() - Duration::days(30));
        assert_eq!(r.until, epoch());
    }

    #[test]
    fn since_until_accepts_rfc3339_date_and_duration() {
        let now = epoch();
        // RFC3339.
        let t1 = parse_since_until("2026-05-01T00:00:00Z", now).unwrap();
        assert_eq!(t1.to_rfc3339(), "2026-05-01T00:00:00+00:00");
        // Date-only.
        let t2 = parse_since_until("2026-05-01", now).unwrap();
        assert_eq!(t2, t1);
        // Relative duration → now - dur.
        let t3 = parse_since_until("30d", now).unwrap();
        assert_eq!(t3, now - Duration::days(30));
        // Garbage → None (no silent default).
        assert_eq!(parse_since_until("notatimestamp", now), None);
    }

    #[test]
    fn time_range_contains_is_half_open() {
        let r = TimeRange::rolling(epoch(), Duration::days(7));
        assert!(r.contains(r.since));
        assert!(r.contains(r.until - Duration::seconds(1)));
        assert!(!r.contains(r.until), "until is exclusive");
        assert!(!r.contains(r.since - Duration::seconds(1)));
    }
}
