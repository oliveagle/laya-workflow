//! Small **pure, generic** helpers shared by capabilities and the plugin host.
//!
//! These were previously private to `browser.rs`, but they are not
//! browser-specific: anything that stamps a record with a time, or that needs to
//! URL-encode a query value, should not have to re-implement them. Keeping them
//! here is also what lets a plugin reach them through the documented host API
//! (`host.now()`, `host.urlencode(...)`) instead of getting a second, drifting
//! copy.

use serde_json::{json, Value};

/// Current time as Unix epoch milliseconds (0 if the clock predates 1970).
pub fn now_unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Format Unix epoch milliseconds as a UTC RFC 3339 timestamp, e.g.
/// `2026-09-28T07:12:03.123Z`. Hand-rolled (civil-from-days) so no date crate is
/// needed; this stamps every saved paper for trending monitoring.
pub fn rfc3339_utc_from_unix_ms(ms: u128) -> String {
    let secs = (ms / 1000) as i64;
    let millis = (ms % 1000) as u32;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let h = (rem / 3600) as u32;
    let mi = ((rem % 3600) / 60) as u32;
    let s = (rem % 60) as u32;
    // Howard Hinnant, "chrono-Compatible Low-Level Date Algorithms".
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{millis:03}Z")
}

/// Stamp a record with the current UTC download time.
pub fn stamp_downloaded(record: &mut Value) {
    let ms = now_unix_ms();
    record["downloaded_at"] = json!(rfc3339_utc_from_unix_ms(ms));
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
    fn stamps_downloaded_time() {
        let mut record = json!({"a": 1});
        stamp_downloaded(&mut record);
        assert!(record["downloaded_at"].as_str().unwrap().ends_with('Z'));
        assert!(record["downloaded_at_unix_ms"].as_u64().unwrap() > 1_600_000_000_000);
    }

    #[test]
    fn encodes_utf8_query_values() {
        assert_eq!(urlencoding_utf8("llm memory"), "llm%20memory");
        assert_eq!(urlencoding_utf8("张量"), "%E5%BC%A0%E9%87%8F");
        assert_eq!(urlencoding_utf8("a/b"), "a/b");
    }
}
