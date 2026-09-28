//! Host/system capabilities — read-only introspection plus a local notifier.
//!
//! * `metrics` — memory / load / disk / uptime (reads `/proc`, no shell needed)
//! * `notify`  — local notification: a **macOS Notification Center banner**
//!   (`channel: "macos"`, via `osascript`), and/or a log line, and/or a bell
//!
//! `metrics` is read-only (no `allow_exec` needed; it reads /proc directly and
//! never writes). `notify` writes to a path (needs `policy.allow_paths`) and, for
//! the macOS banner, spawns `osascript` (needs `policy.allow_exec`).

use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::io::Write;

use super::store::resolve_store_path;
use super::{call_exec, expand, stringify, ExecCap, Policy};

// ── metrics ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct MetricsCap {
    /// Comma-separated subset of: `mem`, `load`, `uptime`, `cpu`, `disk`
    pub what: String,
    /// Path used by the `disk` metric (defaults to "/"; needs no allow_paths
    /// because only aggregate space numbers are returned).
    pub disk_path: String,
}

pub fn call_metrics(c: &MetricsCap, with: &Value, state: &Value) -> Result<Value> {
    let want: Vec<&str> = if c.what.is_empty() {
        vec!["mem", "load", "uptime", "cpu"]
    } else {
        c.what.split(',').map(str::trim).collect()
    };
    let mut out = serde_json::Map::new();
    out.insert("capability".into(), json!("metrics"));

    if want.contains(&"uptime") {
        if let Ok(t) = std::fs::read_to_string("/proc/uptime") {
            if let Some(secs) = t
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<f64>().ok())
            {
                out.insert("uptime_secs".into(), json!(secs));
            }
        }
    }
    if want.contains(&"load") {
        if let Ok(t) = std::fs::read_to_string("/proc/loadavg") {
            let f: Vec<&str> = t.split_whitespace().collect();
            out.insert(
                "loadavg".into(),
                json!({
                    "1m": f.first().and_then(|s| s.parse::<f64>().ok()),
                    "5m": f.get(1).and_then(|s| s.parse::<f64>().ok()),
                    "15m": f.get(2).and_then(|s| s.parse::<f64>().ok()),
                }),
            );
        }
    }
    if want.contains(&"mem") {
        if let Ok(t) = std::fs::read_to_string("/proc/meminfo") {
            let mut kb = std::collections::HashMap::new();
            for line in t.lines() {
                if let Some((k, rest)) = line.split_once(':') {
                    if let Some(v) = rest
                        .split_whitespace()
                        .next()
                        .and_then(|s| s.parse::<u64>().ok())
                    {
                        kb.insert(k.trim().to_string(), v);
                    }
                }
            }
            let total = kb.get("MemTotal").copied().unwrap_or(0);
            let avail = kb.get("MemAvailable").copied().unwrap_or(0);
            out.insert(
                "mem".into(),
                json!({
                    "total_kb": total, "available_kb": avail, "used_kb": total.saturating_sub(avail),
                    "used_pct": if total > 0 { ((total - avail) as f64 / total as f64 * 100.0).round() } else { 0.0 },
                }),
            );
        }
    }
    if want.contains(&"cpu") {
        let n = std::fs::read_to_string("/proc/cpuinfo")
            .map(|t| t.lines().filter(|l| l.starts_with("processor")).count())
            .unwrap_or(0);
        out.insert("cpu_count".into(), json!(n));
    }
    if want.contains(&"disk") {
        let p = if c.disk_path.is_empty() {
            with.get("disk_path")
                .map(stringify)
                .unwrap_or_else(|| "/".to_string())
        } else {
            c.disk_path.clone()
        };
        let p = stringify(&expand(&Value::String(p), state, with));
        // statvfs via `df -Pk` would need a shell; read /proc/self/mountinfo is
        // messy, so use libc-free fallback: report the mount point only.
        out.insert("disk_path".into(), json!(p));
        out.insert(
            "disk_note".into(),
            json!("use the `shell` or `exec` capability for exact df numbers"),
        );
    }
    Ok(Value::Object(out))
}

// ── notify (local + macOS) ──────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct NotifyLocalCap {
    /// Target log file (path-allow-listed) for the `log` channel.
    pub path: String,
    /// Emit a terminal bell on stderr as well.
    pub bell: bool,
    /// Include a timestamp prefix in the log line.
    pub timestamp: bool,
    /// Delivery channel: `log` (default) | `macos` | `both` | `auto`.
    pub channel: String,
    /// Banner title (macOS `macos`/`both` channels; default `laya-workflow`).
    pub title: String,
    /// Banner subtitle (macOS).
    pub subtitle: String,
    /// Banner sound name (macOS), e.g. `Glass`; empty ⇒ silent.
    pub sound: String,
    pub timeout_ms: u64,
}

pub fn call_notify_local(
    c: &NotifyLocalCap,
    with: &Value,
    state: &Value,
    policy: &Policy,
) -> Result<Value> {
    let event = with
        .get("event")
        .map(stringify)
        .unwrap_or_else(|| "notify".to_string());
    let message = with.get("message").map(stringify).unwrap_or_default();
    let channel = resolve_channel(c, with, state);

    let deliver_macos = matches!(channel.as_str(), "macos" | "both");
    let deliver_log = matches!(channel.as_str(), "log" | "both");
    if !deliver_macos && !deliver_log {
        bail!("notify channel {channel:?} unsupported (log | macos | both | auto)");
    }

    let mut out = json!({
        "capability": "notify", "channel": channel, "event": event, "message": message,
    });

    if deliver_macos {
        out["macos"] = notify_macos(c, with, state, policy, &message)?;
    }

    if deliver_log {
        let raw_path = if c.path.is_empty() {
            with.get("path").map(stringify).unwrap_or_default()
        } else {
            stringify(&expand(&Value::String(c.path.clone()), state, with))
        };
        if raw_path.trim().is_empty() {
            // No log file configured: a bell is the only local side effect.
            // (When the macOS banner already fired, this is a legitimate no-op.)
            if !deliver_macos && !c.bell {
                bail!(
                    "notify channel \"log\" needs 'path' (a log file) — or use channel \"macos\""
                );
            }
            out["path"] = json!("");
            out["bytes"] = json!(0);
        } else {
            let path = resolve_store_path(&raw_path, policy)?;
            let line = if c.timestamp {
                let t = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                format!("{t}\t{event}\t{message}\n")
            } else {
                format!("{event}\t{message}\n")
            };
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;
            f.write_all(line.as_bytes())?;
            out["path"] = json!(path.display().to_string());
            out["bytes"] = json!(line.len());
        }
        out["bell"] = json!(c.bell);
        if c.bell {
            let _ = std::io::stderr().write_all(b"\x07");
        }
    }

    Ok(out)
}

/// Resolve the channel: `with.channel` (call-time) wins, then the capability's
/// `channel`, then `log`. `auto` = `macos` on macOS, else `log`.
fn resolve_channel(c: &NotifyLocalCap, with: &Value, state: &Value) -> String {
    let raw = with
        .get("channel")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| stringify(&expand(&Value::String(c.channel.clone()), state, with)));
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "log" | "file" => "log".to_string(),
        "macos" | "os" | "osx" | "notification" => "macos".to_string(),
        "both" => "both".to_string(),
        "auto" => if cfg!(target_os = "macos") {
            "macos"
        } else {
            "log"
        }
        .to_string(),
        other => other.to_string(),
    }
}

/// Post a real Notification Center banner via `osascript` (macOS only). It runs
/// through `call_exec`, so `policy.allow_exec` governs it.
fn notify_macos(
    c: &NotifyLocalCap,
    with: &Value,
    state: &Value,
    policy: &Policy,
    message: &str,
) -> Result<Value> {
    if !cfg!(target_os = "macos") {
        bail!(
            "notify channel \"macos\" requires macOS (this host is {})",
            std::env::consts::OS
        );
    }
    let title =
        str_field(&c.title, with, state, "title").unwrap_or_else(|| "laya-workflow".to_string());
    let subtitle = str_field(&c.subtitle, with, state, "subtitle").unwrap_or_default();
    let sound = str_field(&c.sound, with, state, "sound").unwrap_or_default();
    let script = applescript_display(&title, &subtitle, message, &sound);
    let cap = ExecCap {
        argv: vec!["osascript".to_string(), "-e".to_string(), script.clone()],
        cwd: None,
        env: serde_json::Map::new(),
        timeout_ms: if c.timeout_ms == 0 {
            10_000
        } else {
            c.timeout_ms
        },
        max_output: policy.max_output,
    };
    let out = call_exec(&cap, &json!({}), &json!({}), policy)?;
    Ok(json!({
        "title": title, "subtitle": subtitle, "sound": sound,
        "delivered": out["ok"], "exit_code": out["exit_code"],
        "stderr": out["stderr"], "script": script,
    }))
}

/// `with.<key>` wins over the capability value; empty/absent ⇒ `None`.
fn str_field(cap: &str, with: &Value, state: &Value, key: &str) -> Option<String> {
    with.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let expanded = stringify(&expand(&Value::String(cap.to_string()), state, with));
            (!expanded.is_empty()).then_some(expanded)
        })
}

/// The AppleScript `display notification … with title …` for one banner.
fn applescript_display(title: &str, subtitle: &str, message: &str, sound: &str) -> String {
    let mut s = format!(
        "display notification {} with title {}",
        asc(message),
        asc(title)
    );
    if !subtitle.is_empty() {
        s.push_str(&format!(" subtitle {}", asc(subtitle)));
    }
    if !sound.is_empty() {
        s.push_str(&format!(" sound name {}", asc(sound)));
    }
    s
}

/// Quote a string as an AppleScript literal (escape `\` and `"`).
fn asc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applescript_escapes_quotes_and_backslashes() {
        assert_eq!(asc("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(
            applescript_display("T", "", "hi", "Glass"),
            "display notification \"hi\" with title \"T\" sound name \"Glass\""
        );
        assert_eq!(
            applescript_display("T", "Sub", "hi", ""),
            "display notification \"hi\" with title \"T\" subtitle \"Sub\""
        );
    }

    #[test]
    fn channel_resolution_prefers_with_and_handles_auto() {
        let c = NotifyLocalCap {
            channel: "log".to_string(),
            ..Default::default()
        };
        assert_eq!(resolve_channel(&c, &json!({}), &json!({})), "log");
        assert_eq!(
            resolve_channel(&c, &json!({"channel": "macos"}), &json!({})),
            "macos"
        );
        let auto = NotifyLocalCap {
            channel: "auto".to_string(),
            ..Default::default()
        };
        let expect = if cfg!(target_os = "macos") {
            "macos"
        } else {
            "log"
        };
        assert_eq!(resolve_channel(&auto, &json!({}), &json!({})), expect);
    }
}
