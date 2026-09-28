//! Small **pure, generic** helpers shared by capabilities and the plugin host.
//!
//! These were previously private to `browser.rs`, but they are not
//! browser-specific: anything that stamps a record with a time, or that needs to
//! URL-encode a query value, should not have to re-implement them. Keeping them
//! here is also what lets a plugin reach them through the documented host API
//! (`host.now()`, `host.urlencode(...)`) instead of getting a second, drifting
//! copy.
//!
//! Timestamps are stored as **UTC** (the canonical, sortable form) and *also*
//! rendered in the **local timezone of whoever runs the engine** — resolved
//! through `localtime_r`, so DST and half-hour zones are the OS's problem, not
//! ours. A caller that does not want the ambient zone (CI, a shared report) can
//! pass an explicit offset instead.

use serde_json::{json, Value};

/// Current time as Unix epoch milliseconds (0 if the clock predates 1970).
pub fn now_unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

// ── civil <-> days (Howard Hinnant, "chrono-Compatible Low-Level Date
//    Algorithms") ──
//
// Hand-rolled so no date crate is needed anywhere in the engine.

/// `(year, month, day)` for a count of days since 1970-01-01.
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Days since 1970-01-01 for a civil date (inverse of [`civil_from_days`]).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = ((m + 9) % 12) as i64; // Mar = 0
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The `±HH:MM` (or `Z`) suffix for a UTC offset in seconds.
fn offset_suffix(off: i64) -> String {
    if off == 0 {
        return "Z".to_string();
    }
    let sign = if off < 0 { '-' } else { '+' };
    let a = off.abs();
    format!("{sign}{:02}:{:02}", a / 3600, (a % 3600) / 60)
}

fn format_at(ms: i128, offset_secs: i64) -> String {
    let shifted = ms + (offset_secs as i128) * 1000;
    let secs = shifted.div_euclid(1000);
    let millis = shifted.rem_euclid(1000) as u32;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days as i64);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!(
        "{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{millis:03}{}",
        offset_suffix(offset_secs)
    )
}

/// Format Unix epoch milliseconds as a UTC RFC 3339 timestamp, e.g.
/// `2026-09-28T07:12:03.123Z`. This stamps every saved paper for monitoring.
pub fn rfc3339_utc_from_unix_ms(ms: u128) -> String {
    format_at(ms as i128, 0)
}

/// Format Unix epoch milliseconds at an explicit UTC offset, e.g.
/// `2026-09-28T15:12:03.123+08:00` (`Z` when the offset is 0).
pub fn rfc3339_from_unix_ms_offset(ms: i64, offset_secs: i64) -> String {
    format_at(ms as i128, offset_secs)
}

/// Format Unix epoch milliseconds in the *local* timezone of this process.
pub fn rfc3339_local_from_unix_ms(ms: u128) -> String {
    format_at(
        ms as i128,
        local_offset_secs_at((ms / 1000) as i64).unwrap_or(0),
    )
}

/// Seconds **east** of UTC that `unix_secs` falls at in the local timezone
/// (so `+08:00` is `28_800`). `None` when the platform/OS cannot say.
#[cfg(unix)]
pub fn local_offset_secs_at(unix_secs: i64) -> Option<i64> {
    let t = unix_secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    if ok {
        Some(tm.tm_gmtoff as i64)
    } else {
        None
    }
}

#[cfg(not(unix))]
pub fn local_offset_secs_at(_unix_secs: i64) -> Option<i64> {
    None
}

/// The local timezone abbreviation at the current instant, e.g. `CST`, `UTC`.
#[cfg(unix)]
pub fn local_tz_abbrev() -> Option<String> {
    let t = (now_unix_ms() / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    if !ok || tm.tm_zone.is_null() {
        return None;
    }
    let s = unsafe { std::ffi::CStr::from_ptr(tm.tm_zone) }
        .to_string_lossy()
        .to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(not(unix))]
pub fn local_tz_abbrev() -> Option<String> {
    None
}

/// The local IANA zone name, e.g. `Asia/Shanghai`. Prefers `$TZ`, then the
/// `/etc/localtime` symlink (`…/zoneinfo/<Zone>`, the same on macOS and Linux).
/// `None` when the zone cannot be named — the offset is still available.
pub fn local_tz_name() -> Option<String> {
    if let Ok(tz) = std::env::var("TZ") {
        let tz = tz.trim().trim_start_matches(':');
        if !tz.is_empty() {
            return Some(tz.to_string());
        }
    }
    let link = std::fs::read_link("/etc/localtime").ok()?;
    let s = link.to_string_lossy();
    let name = s
        .split("zoneinfo/")
        .nth(1)
        .unwrap_or("")
        .trim_start_matches('/');
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// The full set of time fields a caller (or a plugin) may want for "now":
/// UTC *and* local, with the offset and zone name so the local field is
/// unambiguous.
pub fn now_fields() -> Value {
    now_fields_at(now_unix_ms())
}

pub fn now_fields_at(ms: u128) -> Value {
    let off = local_offset_secs_at((ms / 1000) as i64);
    json!({
        "unix_ms": ms as u64,
        "rfc3339": rfc3339_utc_from_unix_ms(ms),
        "rfc3339_local": rfc3339_from_unix_ms_offset(ms as i64, off.unwrap_or(0)),
        "utc_offset_secs": off.unwrap_or(0),
        "tz": local_tz_name().unwrap_or_default(),
        "tz_abbrev": local_tz_abbrev().unwrap_or_default(),
    })
}

/// Parse a timestamp to Unix epoch **milliseconds**.
///
/// Accepts RFC 3339 (`2026-09-24T03:39:57.000Z`, `…+08:00`, `…-0500`, a space
/// instead of `T`, fractional seconds optional, no zone ⇒ UTC) and a bare
/// integer (seconds, or milliseconds when it has 12+ digits). Returns `None`
/// for anything else, so a caller can keep the raw string instead of guessing.
pub fn parse_time_to_unix_ms(s: &str) -> Option<i64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    if t.bytes().all(|b| b.is_ascii_digit()) {
        let n: i64 = t.parse().ok()?;
        return Some(if t.len() >= 12 { n } else { n * 1000 });
    }
    let b = t.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |lo: usize, hi: usize| -> Option<i64> { t.get(lo..hi)?.parse().ok() };
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    if b[4] != b'-'
        || b[7] != b'-'
        || (b[10] != b'T' && b[10] != b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (h, mi, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut rest = &t[19..];
    let mut frac_ms = 0i64;
    if let Some(r) = rest.strip_prefix('.') {
        let digits: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return None;
        }
        let mut padded = digits.clone();
        padded.truncate(3);
        while padded.len() < 3 {
            padded.push('0');
        }
        frac_ms = padded.parse().ok()?;
        rest = &r[digits.len()..];
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let (sign, hhmm) = match rest.as_bytes()[0] {
                b'+' => (1i64, &rest[1..]),
                b'-' => (-1i64, &rest[1..]),
                _ => return None,
            };
            let digits: String = hhmm.chars().filter(|c| c.is_ascii_digit()).collect();
            let (oh, om) = match digits.len() {
                2 => (digits.parse::<i64>().ok()?, 0),
                4 => (
                    digits.get(0..2)?.parse().ok()?,
                    digits.get(2..4)?.parse().ok()?,
                ),
                _ => return None,
            };
            sign * (oh * 3600 + om * 60)
        }
    };
    let secs = days_from_civil(y, mo as u32, d as u32) * 86_400 + h * 3600 + mi * 60 + sec - offset;
    Some(secs * 1000 + frac_ms)
}

/// Stamp a record with the current download time — UTC (canonical) **and** local.
pub fn stamp_downloaded(record: &mut Value) {
    let ms = now_unix_ms();
    record["downloaded_at"] = json!(rfc3339_utc_from_unix_ms(ms));
    record["downloaded_at_local"] = json!(rfc3339_local_from_unix_ms(ms));
    record["downloaded_at_unix_ms"] = json!(ms as u64);
}

/// Encode unsafe query bytes while preserving URL syntax accepted by DevTools.
pub fn urlencoding_utf8(v: &str) -> String {
    v.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'-' | b'_'
                        | b'.'
                        | b'~'
                        | b':'
                        | b'/'
                        | b'?'
                        | b'#'
                        | b'['
                        | b']'
                        | b'@'
                        | b'!'
                        | b'$'
                        | b'&'
                        | b'\''
                        | b'('
                        | b')'
                        | b'*'
                        | b'+'
                        | b','
                        | b';'
                        | b'='
                )
            {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_unix_ms_as_rfc3339() {
        assert_eq!(rfc3339_utc_from_unix_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339_utc_from_unix_ms(1_500), "1970-01-01T00:00:01.500Z");
        assert_eq!(
            rfc3339_utc_from_unix_ms(1_000_000_000_000),
            "2001-09-09T01:46:40.000Z"
        );
        assert_eq!(
            rfc3339_utc_from_unix_ms(1_234_567_890_123),
            "2009-02-13T23:31:30.123Z"
        );
        // A leap day.
        assert_eq!(
            rfc3339_utc_from_unix_ms(1_582_934_400_000),
            "2020-02-29T00:00:00.000Z"
        );
    }

    #[test]
    fn formats_at_an_explicit_offset() {
        let ms = 1_234_567_890_123i64; // 2009-02-13T23:31:30.123Z
        assert_eq!(
            rfc3339_from_unix_ms_offset(ms, 8 * 3600),
            "2009-02-14T07:31:30.123+08:00"
        );
        assert_eq!(
            rfc3339_from_unix_ms_offset(0, -5 * 3600),
            "1969-12-31T19:00:00.000-05:00"
        );
        // Half-hour and 45-minute zones keep their minutes.
        assert_eq!(
            rfc3339_from_unix_ms_offset(0, 5 * 3600 + 1800),
            "1970-01-01T05:30:00.000+05:30"
        );
        assert_eq!(
            rfc3339_from_unix_ms_offset(0, 5 * 3600 + 2700),
            "1970-01-01T05:45:00.000+05:45"
        );
        // Zero offset is rendered as `Z`.
        assert_eq!(
            rfc3339_from_unix_ms_offset(0, 0),
            "1970-01-01T00:00:00.000Z"
        );
    }

    #[test]
    fn parses_rfc3339_to_epoch_ms() {
        // The exact shape the HuggingFace API returns.
        let ms = parse_time_to_unix_ms("2026-09-24T03:39:57.000Z").unwrap();
        assert_eq!(
            rfc3339_utc_from_unix_ms(ms as u128),
            "2026-09-24T03:39:57.000Z"
        );
        // The same instant with an explicit offset.
        assert_eq!(parse_time_to_unix_ms("2026-09-24T11:39:57+08:00"), Some(ms));
        assert_eq!(parse_time_to_unix_ms("2026-09-24T11:39:57+0800"), Some(ms));
        // Optional fraction, and a space instead of `T`.
        assert_eq!(parse_time_to_unix_ms("2026-09-24 03:39:57Z"), Some(ms));
        assert_eq!(
            parse_time_to_unix_ms("2026-09-24T03:39:57.5Z"),
            Some(ms + 500)
        );
        // No zone ⇒ UTC; bare integers are seconds, or ms when long enough.
        assert_eq!(parse_time_to_unix_ms("2026-09-24T03:39:57"), Some(ms));
        assert_eq!(parse_time_to_unix_ms("1000000000"), Some(1_000_000_000_000));
        assert_eq!(
            parse_time_to_unix_ms("1000000000000"),
            Some(1_000_000_000_000)
        );
        // Junk is rejected rather than guessed.
        assert_eq!(parse_time_to_unix_ms(""), None);
        assert_eq!(parse_time_to_unix_ms("not-a-date"), None);
        assert_eq!(parse_time_to_unix_ms("2026-13-24T03:39:57Z"), None);
    }

    #[test]
    fn local_fields_agree_with_the_utc_field() {
        let now = now_fields();
        assert!(now["rfc3339"].as_str().unwrap().ends_with('Z'));
        let off = now["utc_offset_secs"].as_i64().unwrap();
        assert_eq!(now["unix_ms"].as_u64().is_some(), true);
        // The local rendering must parse back to the same instant.
        let local = now["rfc3339_local"].as_str().unwrap();
        assert_eq!(
            parse_time_to_unix_ms(local),
            Some(now["unix_ms"].as_u64().unwrap() as i64)
        );
        assert_eq!(
            local,
            rfc3339_from_unix_ms_offset(now["unix_ms"].as_u64().unwrap() as i64, off)
        );
        // A bare `Z` local field is only correct at offset 0.
        assert_eq!(local.ends_with('Z'), off == 0);
    }

    #[cfg(unix)]
    #[test]
    fn resolves_the_local_offset() {
        // The process zone is whatever the machine says; on unix we must get an
        // answer (the value itself is intentionally not asserted).
        assert!(local_offset_secs_at(1_700_000_000).is_some());
        let local = rfc3339_local_from_unix_ms(1_700_000_000_000);
        assert!(
            local.ends_with('Z') || local.contains('+') || local.contains('-'),
            "{local}"
        );
    }

    #[test]
    fn stamps_downloaded_time() {
        let mut record = json!({"a": 1});
        stamp_downloaded(&mut record);
        assert!(record["downloaded_at"].as_str().unwrap().ends_with('Z'));
        assert!(record["downloaded_at_local"].as_str().unwrap().len() >= 25);
        assert!(record["downloaded_at_unix_ms"].as_u64().unwrap() > 1_600_000_000_000);
    }

    #[test]
    fn encodes_utf8_query_values() {
        assert_eq!(urlencoding_utf8("llm memory"), "llm%20memory");
        assert_eq!(urlencoding_utf8("张量"), "%E5%BC%A0%E9%87%8F");
        assert_eq!(urlencoding_utf8("a/b"), "a/b");
    }
}
