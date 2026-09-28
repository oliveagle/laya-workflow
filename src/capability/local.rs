//! Pure / local capabilities that need no network and no extra dependencies.
//!
//! * `datetime` — date/time arithmetic and formatting (deterministic helpers)
//! * `text`     — text transforms (regex extract/replace, split, hash, base64…)
//! * `file`     — read / write / append, guarded by a path allow-list
//! * `sqlite`   — query a SQLite file via the `sqlite3` CLI (read-only by default)
//! * `shell`    — run a shell command with stdout/stderr/exit-code semantics

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use super::{bounded_timeout, effective_op, expand, stringify, truncate, ExecCap, Policy};

// ── datetime ────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct DatetimeCap {
    /// `now` | `format` | `parse` | `add`
    pub op: String,
    pub format: String,
    pub offset_secs: i64,
}

pub fn call_datetime(c: &DatetimeCap, with: &Value, state: &Value) -> Result<Value> {
    let op = if c.op.is_empty() {
        "now"
    } else {
        c.op.as_str()
    };
    let with = expand(with, state, with);
    let epoch = with.get("epoch").and_then(|v| v.as_i64());
    match op {
        "now" => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            Ok(
                json!({ "capability": "datetime", "op": "now", "epoch": now, "offset_secs": c.offset_secs }),
            )
        }
        "add" => {
            let base = epoch.unwrap_or(0) + c.offset_secs;
            Ok(
                json!({ "capability": "datetime", "op": "add", "epoch": base, "offset_secs": c.offset_secs }),
            )
        }
        "format" => {
            // Formatting uses the civil-date algorithm in `civil_from_days`
            // (Howard Hinnant's days_from_civil inverse), so no chrono needed.
            let e = epoch.ok_or_else(|| anyhow!("datetime.format needs 'epoch'"))? + c.offset_secs;
            let (y, m, d, hh, mm, ss) = civil_from_epoch(e);
            let s = render_time(&c.format, y, m, d, hh, mm, ss);
            Ok(json!({ "capability": "datetime", "op": "format", "epoch": e, "text": s }))
        }
        "parse" => {
            // Minimal: accept an epoch number, or a bare ISO-8601 UTC date.
            let raw = with
                .get("text")
                .map(stringify)
                .ok_or_else(|| anyhow!("datetime.parse needs 'text'"))?;
            let epoch = raw.trim().parse::<i64>().or_else(|_| iso_to_epoch(&raw))?;
            Ok(json!({ "capability": "datetime", "op": "parse", "epoch": epoch }))
        }
        other => bail!("datetime op {other:?} unsupported (now | add | format | parse)"),
    }
}

pub fn civil_from_epoch_pub(epoch: i64) -> (i64, u32, u32, u32, u32, u32) {
    civil_from_epoch(epoch)
}

fn civil_from_epoch(epoch: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    let (hh, mm, ss) = (
        (secs / 3600) as u32,
        ((secs % 3600) / 60) as u32,
        (secs % 60) as u32,
    );
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, hh, mm, ss)
}

fn render_time(fmt: &str, y: i64, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> String {
    let f = if fmt.is_empty() {
        "%Y-%m-%dT%H:%M:%SZ"
    } else {
        fmt
    };
    f.replace("%Y", &format!("{y:04}"))
        .replace("%m", &format!("{m:02}"))
        .replace("%d", &format!("{d:02}"))
        .replace("%H", &format!("{hh:02}"))
        .replace("%M", &format!("{mm:02}"))
        .replace("%S", &format!("{ss:02}"))
}

fn iso_to_epoch(s: &str) -> Result<i64> {
    let t = s.trim();
    if t.len() < 10 {
        bail!("datetime.parse: expected YYYY-MM-DD, got {s:?}");
    }
    let y: i64 = t[0..4].parse().map_err(|_| anyhow!("bad year in {s:?}"))?;
    let m: i64 = t[5..7].parse().map_err(|_| anyhow!("bad month in {s:?}"))?;
    let d: i64 = t[8..10].parse().map_err(|_| anyhow!("bad day in {s:?}"))?;
    let (hh, mm, ss) = if t.len() >= 19 {
        (
            t[11..13].parse::<i64>().unwrap_or(0),
            t[14..16].parse::<i64>().unwrap_or(0),
            t[17..19].parse::<i64>().unwrap_or(0),
        )
    } else {
        (0, 0, 0)
    };
    Ok(days_from_civil(y, m as u32, d as u32) * 86_400 + hh * 3600 + mm * 60 + ss)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// ── text ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct TextCap {
    /// `extract` | `replace` | `split` | `length` | `upper` | `lower` | `trim`
    /// | `hash` | `base64_encode` | `base64_decode` | `json_get`
    pub op: String,
    pub pattern: String,
    pub replacement: String,
    pub separator: String,
    pub hash: String,
}

pub fn call_text(c: &TextCap, with: &Value, state: &Value) -> Result<Value> {
    let input = with
        .get("text")
        .map(stringify)
        .ok_or_else(|| anyhow!("text capability needs 'text' in 'with'"))?;
    let pat = stringify(&expand(&Value::String(c.pattern.clone()), state, with));
    let op = effective_op(&c.op, with, "now");
    match op.as_str() {
        "length" => Ok(json!({ "capability": "text", "op": "length",
                              "chars": input.chars().count(), "bytes": input.len() })),
        "upper" => Ok(json!({ "capability": "text", "op": "upper", "text": input.to_uppercase() })),
        "lower" => Ok(json!({ "capability": "text", "op": "lower", "text": input.to_lowercase() })),
        "trim" => Ok(json!({ "capability": "text", "op": "trim", "text": input.trim() })),
        "split" => {
            let sep = if c.separator.is_empty() {
                "\n".to_string()
            } else {
                c.separator.clone()
            };
            let parts: Vec<&str> = if sep.is_empty() {
                input.split_whitespace().collect()
            } else {
                input.split(sep.as_str()).collect()
            };
            Ok(json!({ "capability": "text", "op": "split", "parts": parts, "count": parts.len() }))
        }
        "extract" => {
            let rx = regex_lite::Regex::new(&pat)
                .map_err(|e| anyhow!("text.extract bad pattern: {e}"))?;
            let caps: Vec<Value> = rx
                .captures_iter(&input)
                .map(|c| {
                    if c.len() > 1 {
                        Value::String(c.get(1).map(|m| m.as_str().to_string()).unwrap_or_default())
                    } else {
                        Value::String(c.get(0).map(|m| m.as_str().to_string()).unwrap_or_default())
                    }
                })
                .collect();
            let count = caps.len();
            Ok(json!({ "capability": "text", "op": "extract", "matches": caps, "count": count }))
        }
        "replace" => {
            let rx = regex_lite::Regex::new(&pat)
                .map_err(|e| anyhow!("text.replace bad pattern: {e}"))?;
            let out = rx.replace_all(&input, c.replacement.as_str()).to_string();
            Ok(
                json!({ "capability": "text", "op": "replace", "text": out, "changed": out != input }),
            )
        }
        "hash" => {
            let algo = if c.hash.is_empty() {
                "fnv1a64"
            } else {
                c.hash.as_str()
            };
            let h = match algo {
                "fnv1a64" => fnv1a64(input.as_bytes()),
                other => bail!("text.hash algo {other:?} unsupported (fnv1a64)"),
            };
            Ok(
                json!({ "capability": "text", "op": "hash", "algo": algo, "hex": format!("{h:016x}") }),
            )
        }
        "base64_encode" => Ok(json!({ "capability": "text", "op": "base64_encode",
                                     "text": base64_encode(input.as_bytes()) })),
        "base64_decode" => {
            let bytes = base64_decode(&input).map_err(|e| anyhow!("base64_decode: {e}"))?;
            let s = String::from_utf8_lossy(&bytes).to_string();
            Ok(json!({ "capability": "text", "op": "base64_decode", "text": s }))
        }
        "json_get" => {
            let v: Value =
                serde_json::from_str(&input).map_err(|e| anyhow!("text.json_get: {e}"))?;
            let mut cur = &v;
            for seg in pat
                .trim_start_matches('/')
                .split('/')
                .filter(|s| !s.is_empty())
            {
                cur = match cur {
                    Value::Object(o) => o
                        .get(seg)
                        .ok_or_else(|| anyhow!("json_get: {seg} missing"))?,
                    Value::Array(a) => a
                        .get(
                            seg.parse::<usize>()
                                .map_err(|_| anyhow!("json_get: bad index {seg}"))?,
                        )
                        .ok_or_else(|| anyhow!("json_get: index {seg} out of range"))?,
                    _ => bail!("json_get: {seg} not traversable"),
                };
            }
            Ok(json!({ "capability": "text", "op": "json_get", "value": cur }))
        }
        other => bail!("text op {other:?} unsupported"),
    }
}

fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn b64_encode(data: &[u8]) -> String {
    base64_encode(data)
}

fn base64_encode(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn base64_decode(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let clean: Vec<u8> = s
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
        .collect();
    let val = |c: u8| -> Result<u8> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            other => bail!("invalid base64 char {:?}", other as char),
        }
    };
    for chunk in clean.chunks(4) {
        if chunk.len() < 2 {
            break;
        }
        let mut n: u32 = 0;
        let mut bits = 0;
        for &c in chunk {
            n = (n << 6) | val(c)? as u32;
            bits += 6;
        }
        let bytes = match chunk.len() {
            2 => vec![(n >> 4) as u8],
            3 => vec![(n >> 10) as u8, (n >> 2) as u8],
            _ => vec![(n >> 16) as u8, (n >> 8) as u8, n as u8],
        };
        let _ = bits;
        out.extend(bytes);
    }
    Ok(out)
}

// ── file ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct FileCap {
    /// `read` | `write` | `append` | `stat` | `list`
    pub op: String,
    /// Allowed root directories; empty ⇒ nothing is allowed (fail-closed).
    pub allow_roots: Vec<PathBuf>,
    pub max_bytes: usize,
}

fn check_path(c: &FileCap, p: &str, policy: &Policy) -> Result<PathBuf> {
    let path = Path::new(p);
    let canon_parent = path
        .parent()
        .map(|d| d.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let canon = if path.exists() {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    } else {
        std::fs::canonicalize(&canon_parent)
            .map(|d| d.join(path.file_name().unwrap_or_default()))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let roots: Vec<PathBuf> = if c.allow_roots.is_empty() {
        policy.allow_paths.iter().map(PathBuf::from).collect()
    } else {
        c.allow_roots.clone()
    };
    if roots.is_empty() {
        bail!("file capability has no allowed roots (set capability allow_roots or policy.allow_paths)");
    }
    for r in &roots {
        let rc = std::fs::canonicalize(r).unwrap_or_else(|_| r.clone());
        if canon.starts_with(&rc) {
            return Ok(canon);
        }
    }
    bail!("path {p:?} is outside the allowed roots {roots:?} (denied)")
}

pub fn call_file(c: &FileCap, with: &Value, _state: &Value, policy: &Policy) -> Result<Value> {
    let raw_path = with
        .get("path")
        .map(stringify)
        .ok_or_else(|| anyhow!("file capability needs 'path' in 'with'"))?;
    let path = check_path(c, &raw_path, policy)?;
    let limit = c.max_bytes.min(policy.max_output);
    let op = effective_op(&c.op, with, "now");
    match op.as_str() {
        "read" => {
            let meta = std::fs::metadata(&path)?;
            if meta.len() as usize > limit {
                bail!("file {:?} is {} bytes > cap {limit}", raw_path, meta.len());
            }
            let s = truncate(std::fs::read_to_string(&path)?, limit);
            let bytes = s.len();
            Ok(
                json!({ "capability": "file", "op": "read", "path": path.display().to_string(),
                       "text": s, "bytes": bytes }),
            )
        }
        "write" | "append" => {
            let text = with
                .get("text")
                .map(stringify)
                .ok_or_else(|| anyhow!("file.{:?} needs 'text'", c.op))?;
            if text.len() > limit {
                bail!("write payload {} bytes > cap {limit}", text.len());
            }
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(c.op == "append")
                .truncate(c.op == "write")
                .open(&path)?;
            f.write_all(text.as_bytes())?;
            Ok(
                json!({ "capability": "file", "op": c.op, "path": path.display().to_string(),
                       "bytes": text.len() }),
            )
        }
        "stat" => {
            let m = std::fs::metadata(&path)?;
            Ok(
                json!({ "capability": "file", "op": "stat", "path": path.display().to_string(),
                       "bytes": m.len(), "is_dir": m.is_dir() }),
            )
        }
        "list" => {
            let mut names: Vec<String> = std::fs::read_dir(&path)?
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            names.sort();
            Ok(
                json!({ "capability": "file", "op": "list", "path": path.display().to_string(),
                       "entries": names, "count": names.len() }),
            )
        }
        other => bail!("file op {other:?} unsupported (read | write | append | stat | list)"),
    }
}

// ── sqlite (via the sqlite3 CLI; no Rust driver dependency) ─────────

#[derive(Clone, Debug, Default)]
pub struct SqliteCap {
    pub db: String,
    /// `query` (SELECT-only unless allow_write) | `exec`
    pub op: String,
    pub readonly: bool,
    pub timeout_ms: u64,
}

pub fn call_sqlite(c: &SqliteCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let sql = with
        .get("sql")
        .map(stringify)
        .ok_or_else(|| anyhow!("sqlite capability needs 'sql' in 'with'"))?;
    let sql = stringify(&expand(&Value::String(sql), state, with));
    let db = stringify(&expand(&Value::String(c.db.clone()), state, with));
    if db.is_empty() {
        bail!("sqlite capability needs 'db' (path to the database file)");
    }
    let readonly = c.readonly || c.op == "query";
    if readonly {
        let head = sql.trim_start().to_ascii_lowercase();
        let mutating = [
            "insert", "update", "delete", "drop", "alter", "create", "replace", "attach", "pragma",
        ]
        .iter()
        .any(|k| head.starts_with(k));
        if mutating {
            bail!("sqlite is read-only (set capability readonly=false and op=exec to write)");
        }
    }
    // `sqlite3 -json <db> <sql>` prints a JSON array; `-readonly` guards writes.
    let mut argv: Vec<String> = vec!["sqlite3".to_string()];
    if readonly {
        argv.push("-readonly".to_string());
    }
    argv.push("-json".to_string());
    argv.push(db.clone());
    argv.push(sql.clone());
    let cap = ExecCap {
        argv,
        cwd: None,
        env: serde_json::Map::new(),
        timeout_ms: if c.timeout_ms == 0 {
            10_000
        } else {
            c.timeout_ms
        },
        max_output: policy.max_output,
    };
    // sqlite is a process spawn → governed by allow_exec
    let out = super::call_exec(&cap, &json!({}), &json!({}), policy)?;
    let text = out["stdout"].as_str().unwrap_or("").to_string();
    let rows: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok(json!({
        "capability": "sqlite", "op": if readonly {"query"} else {"exec"},
        "db": db, "readonly": readonly, "exit_code": out["exit_code"],
        "rows": rows, "raw": text, "sql": sql,
    }))
}

// ── shell (exec + explicit shell semantics) ─────────────────────────

#[derive(Clone, Debug, Default)]
pub struct ShellCap {
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_ms: u64,
}

pub fn call_shell(c: &ShellCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    if !policy.allow_exec {
        bail!("shell capabilities are disabled (set policy.allow_exec = true to enable)");
    }
    let cmd = stringify(&expand(&Value::String(c.command.clone()), state, with));
    let cap = ExecCap {
        argv: vec!["/bin/sh".to_string(), "-c".to_string(), cmd.clone()],
        cwd: c.cwd.clone(),
        env: serde_json::Map::new(),
        timeout_ms: if c.timeout_ms == 0 {
            30_000
        } else {
            c.timeout_ms
        },
        max_output: policy.max_output,
    };
    let out = super::call_exec(&cap, &json!({}), &json!({}), policy)?;
    let _ = bounded_timeout(cap.timeout_ms, policy);
    Ok(json!({
        "capability": "shell",
        "command": cmd,
        "exit_code": out["exit_code"],
        "ok": out["ok"],
        "stdout": out["stdout"],
        "stderr": out["stderr"],
    }))
}
