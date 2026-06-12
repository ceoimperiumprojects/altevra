//! ICS calendar connector (PLAN-EXTEND §E1.5).
//!
//! Parses iCalendar `VEVENT` blocks from a local `.ics` file path OR a
//! private-ICS URL (the same shape Google Calendar exposes as its "Secret
//! address in iCal format" — ZERO OAuth). Emits today/tomorrow events as
//! [`ConnectorPayload::CalendarEvent`] so the daily brief's Calendar section
//! lights up. Read-only; no credential needed for a public/secret URL.

use super::{
    AuthMode, Connector, ConnectorCtx, ConnectorDescriptor, ConnectorHealth, ConnectorItem,
    ConnectorPayload, ItemProvenance,
};
use altevra_core::domain::Domain;
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, TimeZone, Utc};

pub struct IcsConnector;

impl IcsConnector {
    pub fn new() -> Self {
        IcsConnector
    }
}

impl Default for IcsConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl Connector for IcsConnector {
    fn descriptor(&self) -> ConnectorDescriptor {
        ConnectorDescriptor {
            name: "ics".into(),
            kind: "calendar".into(),
            auth_mode: AuthMode::IcsUrl,
            domains: vec![Domain::Personal],
            description: "iCalendar feed (file path or private-ICS URL; today/tomorrow events)"
                .into(),
        }
    }

    fn pull(&self, ctx: &ConnectorCtx) -> anyhow::Result<Vec<ConnectorItem>> {
        let raw = read_ics_source(ctx)?;
        let domain = ctx
            .config
            .domain
            .as_deref()
            .map(|d| d.parse().unwrap_or(Domain::Personal))
            .unwrap_or(Domain::Personal);
        let events = parse_vevents(&raw);
        let today = ctx.now.date_naive();
        let tomorrow = today.succ_opt().unwrap_or(today);

        let items = events
            .into_iter()
            .filter(|e| {
                let d = e.start.date_naive();
                d == today || d == tomorrow
            })
            .map(|e| ConnectorItem {
                provenance: ItemProvenance {
                    connector: "ics".into(),
                    external_id: e.uid.clone(),
                    ts: e.start,
                },
                domain: domain.clone(),
                payload: ConnectorPayload::CalendarEvent {
                    title: e.summary,
                    start: e.start,
                    end: e.end,
                    location: e.location,
                    notes: e.description,
                },
            })
            .collect();
        Ok(items)
    }

    fn health(&self, ctx: &ConnectorCtx) -> ConnectorHealth {
        if !ctx.config.enabled {
            return ConnectorHealth::disabled("ics");
        }
        match source_kind(ctx) {
            Some(IcsSource::File(p)) => {
                if std::path::Path::new(&p).is_file() {
                    ConnectorHealth::green("ics", format!("file {p}"))
                } else {
                    ConnectorHealth::red("ics", format!("file not found: {p}"))
                }
            }
            Some(IcsSource::Url(u)) => {
                ConnectorHealth::green("ics", format!("url configured ({} chars)", u.len()))
            }
            None => ConnectorHealth::unconfigured("ics", "set params.path or params.url"),
        }
    }
}

enum IcsSource {
    File(String),
    Url(String),
}

/// Resolve where the ICS comes from: explicit `params.path`, `params.url`, or —
/// for `auth_mode = ics_url` — the secret value (the URL itself is the secret).
fn source_kind(ctx: &ConnectorCtx) -> Option<IcsSource> {
    if let Some(p) = ctx.config.param("path") {
        return Some(IcsSource::File(p.to_string()));
    }
    if let Some(u) = ctx.config.param("url") {
        return Some(IcsSource::Url(u.to_string()));
    }
    // The URL may be stored as a keyring secret (private calendar address).
    if let Some(v) = &ctx.auth_value {
        if v.starts_with("http") {
            return Some(IcsSource::Url(v.clone()));
        }
        if std::path::Path::new(v).is_file() {
            return Some(IcsSource::File(v.clone()));
        }
    }
    None
}

fn read_ics_source(ctx: &ConnectorCtx) -> anyhow::Result<String> {
    match source_kind(ctx) {
        Some(IcsSource::File(p)) => Ok(std::fs::read_to_string(&p)?),
        Some(IcsSource::Url(u)) => {
            // Blocking fetch (connectors are pulled from a blocking job context).
            let resp = reqwest::blocking::get(&u)?;
            if !resp.status().is_success() {
                anyhow::bail!("ICS url returned HTTP {}", resp.status());
            }
            Ok(resp.text()?)
        }
        None => anyhow::bail!("ICS connector: no params.path / params.url configured"),
    }
}

/// A parsed VEVENT (the subset we surface).
#[derive(Debug, Clone)]
pub struct VEvent {
    pub uid: String,
    pub summary: String,
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
    pub location: Option<String>,
    pub description: Option<String>,
}

/// Parse all VEVENT blocks from raw ICS text. Tolerant: skips events with no
/// parseable DTSTART; unfolds RFC 5545 line folding (a continuation line starts
/// with a space/tab). Handles `DTSTART:20260612T140000Z`,
/// `DTSTART;TZID=...:...` (naive, treated as UTC), and all-day
/// `DTSTART;VALUE=DATE:20260612`.
pub fn parse_vevents(raw: &str) -> Vec<VEvent> {
    let unfolded = unfold(raw);
    let mut out = Vec::new();
    let mut in_event = false;
    let mut uid = String::new();
    let mut summary = String::new();
    let mut start: Option<DateTime<Utc>> = None;
    let mut end: Option<DateTime<Utc>> = None;
    let mut location: Option<String> = None;
    let mut description: Option<String> = None;
    let mut uid_counter = 0usize;

    for line in unfolded.lines() {
        let line = line.trim_end_matches('\r');
        if line == "BEGIN:VEVENT" {
            in_event = true;
            uid = String::new();
            summary = String::new();
            start = None;
            end = None;
            location = None;
            description = None;
            continue;
        }
        if line == "END:VEVENT" {
            if in_event {
                if let Some(s) = start {
                    if uid.is_empty() {
                        uid_counter += 1;
                        uid = format!("nouid-{}-{}", s.timestamp(), uid_counter);
                    }
                    out.push(VEvent {
                        uid: uid.clone(),
                        summary: if summary.is_empty() {
                            "(no title)".into()
                        } else {
                            summary.clone()
                        },
                        start: s,
                        end,
                        location: location.clone(),
                        description: description.clone(),
                    });
                }
            }
            in_event = false;
            continue;
        }
        if !in_event {
            continue;
        }
        let Some((name_part, value)) = line.split_once(':') else {
            continue;
        };
        // The property name is everything before the first `;` (params follow).
        let name = name_part.split(';').next().unwrap_or(name_part);
        match name {
            "UID" => uid = value.trim().to_string(),
            "SUMMARY" => summary = unescape(value),
            "LOCATION" => {
                let v = unescape(value);
                if !v.is_empty() {
                    location = Some(v);
                }
            }
            "DESCRIPTION" => {
                let v = unescape(value);
                if !v.is_empty() {
                    description = Some(v);
                }
            }
            "DTSTART" => start = parse_dt(name_part, value),
            "DTEND" => end = parse_dt(name_part, value),
            _ => {}
        }
    }
    out
}

/// RFC 5545 line unfolding: a line beginning with a space or tab is a
/// continuation of the previous line.
fn unfold(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for (i, line) in raw.split('\n').enumerate() {
        let l = line.trim_end_matches('\r');
        if i > 0 {
            if l.starts_with(' ') || l.starts_with('\t') {
                out.push_str(&l[1..]);
                continue;
            }
            out.push('\n');
        }
        out.push_str(l);
    }
    out
}

fn unescape(s: &str) -> String {
    s.trim()
        .replace("\\n", "\n")
        .replace("\\N", "\n")
        .replace("\\,", ",")
        .replace("\\;", ";")
        .replace("\\\\", "\\")
}

/// Parse a DTSTART/DTEND value. `name_part` carries params (e.g.
/// `DTSTART;VALUE=DATE`). All-day → midnight UTC. Local floating / TZID times
/// are treated as UTC (good enough for today/tomorrow bucketing).
fn parse_dt(name_part: &str, value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    let is_date_only =
        name_part.to_uppercase().contains("VALUE=DATE") || (value.len() == 8 && !value.contains('T'));
    if is_date_only {
        let d = NaiveDate::parse_from_str(value, "%Y%m%d").ok()?;
        return Some(Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?));
    }
    // 20260612T140000Z  or  20260612T140000
    let trimmed = value.trim_end_matches('Z');
    let ndt = NaiveDateTime::parse_from_str(trimmed, "%Y%m%dT%H%M%S").ok()?;
    Some(Utc.from_utc_datetime(&ndt))
}

/// Helper for the brief: is this datetime "today" relative to `now`?
pub fn is_today(dt: &DateTime<Utc>, now: &DateTime<Utc>) -> bool {
    dt.date_naive() == now.date_naive()
}

/// Helper: day-of-month label for compact brief rendering.
pub fn day_label(dt: &DateTime<Utc>) -> u32 {
    dt.day()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::config::ConnectorConfig;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    const FIXTURE: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:evt-today-1\r\n\
SUMMARY:Standup with ReVesta\r\n\
LOCATION:Zoom\r\n\
DTSTART:20260612T090000Z\r\n\
DTEND:20260612T093000Z\r\n\
DESCRIPTION:Daily sync\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:evt-tomorrow-1\r\n\
SUMMARY:Call with Srdjan\r\n\
DTSTART:20260613T160000Z\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:evt-far-future\r\n\
SUMMARY:Way later\r\n\
DTSTART:20270101T100000Z\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    fn ctx_with_path(path: &str, now: DateTime<Utc>) -> ConnectorCtx {
        let mut params = BTreeMap::new();
        params.insert("path".to_string(), path.to_string());
        ConnectorCtx {
            config: ConnectorConfig {
                enabled: true,
                auth_secret: String::new(),
                cadence_minutes: 60,
                domain: None,
                params,
            },
            auth_value: None,
            now,
        }
    }

    fn now_2026_06_12() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 12, 12, 0, 0).unwrap()
    }

    #[test]
    fn parses_vevents_with_folding_and_escapes() {
        // RFC 5545: a continuation line's FIRST whitespace char is the fold
        // marker (stripped); subsequent chars are literal. So `\r\n folded`
        // unfolds to `folded` appended directly.
        let folded = "BEGIN:VEVENT\r\nUID:x\r\nSUMMARY:Long title that is\r\n folded here\r\nDTSTART:20260612T090000Z\r\nEND:VEVENT\r\n";
        let evts = parse_vevents(folded);
        assert_eq!(evts.len(), 1);
        assert_eq!(evts[0].summary, "Long title that isfolded here");
    }

    #[test]
    fn pull_returns_only_today_and_tomorrow() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("cal.ics");
        std::fs::write(&p, FIXTURE).unwrap();
        let c = IcsConnector::new();
        let items = c
            .pull(&ctx_with_path(p.to_str().unwrap(), now_2026_06_12()))
            .unwrap();
        // today (evt-today-1) + tomorrow (evt-tomorrow-1); far future dropped.
        assert_eq!(items.len(), 2);
        let ids: Vec<String> = items.iter().map(|i| i.provenance.external_id.clone()).collect();
        assert!(ids.contains(&"evt-today-1".to_string()));
        assert!(ids.contains(&"evt-tomorrow-1".to_string()));
        assert!(!ids.contains(&"evt-far-future".to_string()));
        // typed payload + domain + provenance.
        match &items[0].payload {
            ConnectorPayload::CalendarEvent { title, .. } => assert!(!title.is_empty()),
            _ => panic!("expected calendar event"),
        }
        assert_eq!(items[0].provenance.connector, "ics");
    }

    #[test]
    fn all_day_event_parses() {
        let ics = "BEGIN:VEVENT\r\nUID:allday\r\nSUMMARY:Holiday\r\nDTSTART;VALUE=DATE:20260612\r\nEND:VEVENT\r\n";
        let evts = parse_vevents(ics);
        assert_eq!(evts.len(), 1);
        assert_eq!(evts[0].start.date_naive(), NaiveDate::from_ymd_opt(2026, 6, 12).unwrap());
    }

    #[test]
    fn health_reflects_file_presence_and_disabled() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("cal.ics");
        std::fs::write(&p, FIXTURE).unwrap();
        let c = IcsConnector::new();
        assert!(c.health(&ctx_with_path(p.to_str().unwrap(), now_2026_06_12())).is_green());
        // missing file → red
        let h = c.health(&ctx_with_path("/no/such/file.ics", now_2026_06_12()));
        assert_eq!(h.status, "red");
        // disabled config → disabled
        let mut ctx = ctx_with_path(p.to_str().unwrap(), now_2026_06_12());
        ctx.config.enabled = false;
        assert_eq!(c.health(&ctx).status, "disabled");
    }
}
