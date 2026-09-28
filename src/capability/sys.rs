//! Host/system capabilities — read-only introspection plus a local notifier.
//!
//! * `metrics` — memory / load / disk / uptime (reads `/proc`, no shell needed)
//! * `notify`  — local notification: append to a log file and/or emit a bell
//!
//! `metrics` is read-only (no `allow_exec` needed; it reads /proc directly and
//! never writes). `notify` writes to a path, so it requires `policy.allow_paths`.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::io::Write;

use super::store::resolve_store_path;
use super::{expand, stringify, Policy};

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

// ── notify (local) ──────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct NotifyLocalCap {
    /// Target log file (path-allow-listed).
    pub path: String,
    /// Emit a terminal bell on stderr as well.
    pub bell: bool,
    /// Include a timestamp prefix.
    pub timestamp: bool,
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
    let raw_path = if c.path.is_empty() {
        with.get("path").map(stringify).unwrap_or_default()
    } else {
        stringify(&expand(&Value::String(c.path.clone()), state, with))
    };
    if raw_path.is_empty() {
        bail!("notify needs 'path' (a log file) — or use it with `bell` only");
    }
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
    if c.bell {
        let _ = std::io::stderr().write_all(b"\x07");
    }
    let _ = anyhow!(""); // keep the import used across cfg
    Ok(json!({
        "capability": "notify", "event": event, "message": message,
        "path": path.display().to_string(), "bytes": line.len(), "bell": c.bell,
    }))
}
