//! Stateful local capabilities — file/in-memory state with TTL and ordering.
//!
//! * `keyvalue` — KV store: get / set / del / incr / list / has
//! * `cache`    — KV with TTL: get / set / ttl / purge
//! * `queue`    — FIFO queue: push / pop / peek / length / clear
//!
//! All three are **path-scoped** (they live in a single file chosen by the
//! spec) and therefore honour `policy.allow_paths` exactly like `file`.
//! They are pure Rust (no Redis/DB service) and survive process restarts.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use super::{effective_op, expand, stringify, Policy};

/// Per-store-file lock, so the read-modify-write in every store operation is
/// atomic.
///
/// The stores load a file, mutate the value and write it back. Without a lock
/// two concurrent writers read the same snapshot and the second write silently
/// discards the first — measured directly: 8 threads × 25 pushes into one queue
/// left 23 of 200 items. The lock is keyed by the *resolved* path so different
/// stores do not contend, and it is held for the whole load→mutate→save cycle.
fn path_lock(path: &Path) -> &'static Mutex<()> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, &'static Mutex<()>>>> = OnceLock::new();
    let table = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut t = table.lock().unwrap_or_else(|e| e.into_inner());
    t.entry(path.to_path_buf())
        .or_insert_with(|| Box::leak(Box::new(Mutex::new(()))))
}

/// Run `f` while holding this store file's lock.
fn with_lock<T>(path: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let m = path_lock(path);
    let guard = m.lock().unwrap_or_else(|e| e.into_inner());
    let out = f();
    drop(guard);
    out
}

/// Resolve + allow-list a store path (same rules as the `file` capability).
pub fn resolve_store_path(raw: &str, policy: &Policy) -> Result<PathBuf> {
    if raw.is_empty() {
        bail!("capability needs 'path' (the store file)");
    }
    let p = Path::new(raw);
    let cand = if p.exists() {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
    } else {
        let parent = p
            .parent()
            .map(|d| d.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        std::fs::canonicalize(&parent)
            .map(|d| d.join(p.file_name().unwrap_or_default()))
            .unwrap_or_else(|_| p.to_path_buf())
    };
    if policy.allow_paths.is_empty() {
        bail!("store capabilities need policy.allow_paths (empty ⇒ denied)");
    }
    for r in &policy.allow_paths {
        let rc = std::fs::canonicalize(r).unwrap_or_else(|_| PathBuf::from(r));
        if cand.starts_with(&rc) {
            return Ok(cand);
        }
    }
    bail!(
        "path {raw:?} is outside the allowed roots {:?} (denied)",
        policy.allow_paths
    )
}

/// Load a JSON object store.
///
/// A **missing** file is legitimately empty. A file that exists but cannot be
/// read or parsed is an error, not an empty store: silently treating it as `{}`
/// let a subsequent write overwrite the user's data (observed: a corrupt queue
/// file became `[null]` and the push reported success).
fn load(path: &Path) -> Result<Map<String, Value>> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let v: Value = serde_json::from_str(&s)
                .map_err(|e| anyhow!("store {path:?} is not valid JSON: {e}"))?;
            v.as_object()
                .cloned()
                .ok_or_else(|| anyhow!("store {path:?} is not a JSON object"))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(anyhow!("store {path:?} could not be read: {e}")),
    }
}

/// Load a JSON array store (queue / backlog).
///
/// Same contract as [`load`]: missing = empty, present-but-broken = error.
fn load_array(path: &Path) -> Result<Vec<Value>> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let v: Value = serde_json::from_str(&s)
                .map_err(|e| anyhow!("queue {path:?} is not valid JSON: {e}"))?;
            v.as_array()
                .cloned()
                .ok_or_else(|| anyhow!("queue {path:?} is not a JSON array"))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(anyhow!("queue {path:?} could not be read: {e}")),
    }
}

/// Write the value to `path` atomically (temp file + rename).
///
/// A plain `fs::write` truncates in place, so a reader racing the writer could
/// observe an empty or half-written file. Rename is atomic on the same
/// filesystem, so readers see either the old or the new content.
///
/// The temp name must be unique **per write**, not per process: threads share a
/// PID, so a PID-based suffix made every concurrent writer reuse one temp path
/// and corrupt each other's bytes between `write` and `rename`.
fn write_atomic(path: &Path, body: &str) -> Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp{}.{}", std::process::id(), n));
    std::fs::write(&tmp, body)?;
    // If the rename fails, do not leave the temp file behind.
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

fn store(path: &Path, m: &Map<String, Value>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_atomic(
        path,
        &serde_json::to_string_pretty(&Value::Object(m.clone()))?,
    )?;
    Ok(())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── keyvalue ────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct KeyValueCap {
    pub path: String,
    /// `get` | `set` | `del` | `incr` | `list` | `has`
    pub op: String,
    pub key: String,
}

pub fn call_keyvalue(
    c: &KeyValueCap,
    with: &Value,
    state: &Value,
    policy: &Policy,
) -> Result<Value> {
    let raw = stringify(&expand(
        &Value::String(if c.path.is_empty() {
            with.get("path").map(stringify).unwrap_or_default()
        } else {
            c.path.clone()
        }),
        state,
        with,
    ));
    let path = resolve_store_path(&raw, policy)?;
    with_lock(&path, || {
        let mut m = load(&path)?;
        let key = if c.key.is_empty() {
            with.get("key").map(stringify).unwrap_or_default()
        } else {
            c.key.clone()
        };
        let op = effective_op(&c.op, with, "length");
        match op.as_str() {
            "get" => Ok(json!({
                "capability": "keyvalue", "op": "get", "key": key,
                "found": m.contains_key(&key), "value": m.get(&key).cloned().unwrap_or(Value::Null),
            })),
            "has" => Ok(
                json!({ "capability": "keyvalue", "op": "has", "key": key, "found": m.contains_key(&key) }),
            ),
            "set" => {
                if key.is_empty() {
                    bail!("keyvalue.set needs 'key'");
                }
                let v = with.get("value").cloned().unwrap_or(Value::Null);
                let v = expand(&v, state, with);
                m.insert(key.clone(), v.clone());
                store(&path, &m)?;
                Ok(
                    json!({ "capability": "keyvalue", "op": "set", "key": key, "value": v, "size": m.len() }),
                )
            }
            "del" => {
                let existed = m.remove(&key).is_some();
                store(&path, &m)?;
                Ok(
                    json!({ "capability": "keyvalue", "op": "del", "key": key, "removed": existed, "size": m.len() }),
                )
            }
            "incr" => {
                if key.is_empty() {
                    bail!("keyvalue.incr needs 'key'");
                }
                let by = with.get("by").and_then(|v| v.as_i64()).unwrap_or(1);
                let cur = m.get(&key).and_then(|v| v.as_i64()).unwrap_or(0);
                let next = cur + by;
                m.insert(key.clone(), json!(next));
                store(&path, &m)?;
                Ok(
                    json!({ "capability": "keyvalue", "op": "incr", "key": key, "from": cur, "to": next }),
                )
            }
            "list" => {
                let mut keys: Vec<String> = m.keys().cloned().collect();
                keys.sort();
                Ok(
                    json!({ "capability": "keyvalue", "op": "list", "keys": keys, "count": keys.len() }),
                )
            }
            other => bail!("keyvalue op {other:?} unsupported (get|set|del|incr|list|has)"),
        }
    })
}

// ── cache (TTL) ─────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct CacheCap {
    pub path: String,
    /// `get` | `set` | `ttl` | `purge`
    pub op: String,
    pub ttl_secs: i64,
}

/// Cache entries are stored as `{"v": <value>, "e": <expiry_epoch>}`.
pub fn call_cache(c: &CacheCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let raw = stringify(&expand(
        &Value::String(if c.path.is_empty() {
            with.get("path").map(stringify).unwrap_or_default()
        } else {
            c.path.clone()
        }),
        state,
        with,
    ));
    let path = resolve_store_path(&raw, policy)?;
    with_lock(&path, || {
        let mut m = load(&path)?;
        let key = with.get("key").map(stringify).unwrap_or_default();
        if key.is_empty() {
            bail!("cache needs 'key'");
        }
        let t = now();
        let op = effective_op(&c.op, with, "length");
        match op.as_str() {
            "get" => {
                let entry = m.get(&key);
                let live = entry
                    .and_then(|e| e.get("e"))
                    .and_then(|e| e.as_i64())
                    .map(|e| e > t)
                    .unwrap_or(false);
                Ok(json!({
                    "capability": "cache", "op": "get", "key": key, "hit": live,
                    "value": if live { entry.and_then(|e| e.get("v")).cloned().unwrap_or(Value::Null) } else { Value::Null },
                    "expires_at": entry.and_then(|e| e.get("e")).cloned().unwrap_or(Value::Null),
                }))
            }
            "set" => {
                let ttl = if c.ttl_secs == 0 {
                    with.get("ttl_secs").and_then(|v| v.as_i64()).unwrap_or(60)
                } else {
                    c.ttl_secs
                };
                let v = expand(
                    &with.get("value").cloned().unwrap_or(Value::Null),
                    state,
                    with,
                );
                m.insert(key.clone(), json!({ "v": v, "e": t + ttl }));
                store(&path, &m)?;
                Ok(
                    json!({ "capability": "cache", "op": "set", "key": key, "ttl_secs": ttl, "expires_at": t + ttl }),
                )
            }
            "ttl" => {
                let e = m
                    .get(&key)
                    .and_then(|x| x.get("e"))
                    .and_then(|x| x.as_i64());
                Ok(json!({
                    "capability": "cache", "op": "ttl", "key": key,
                    "ttl_secs": e.map(|e| (e - t).max(0)), "expired": e.map(|e| e <= t).unwrap_or(true),
                }))
            }
            "purge" => {
                let before = m.len();
                m.retain(|_, v| {
                    v.get("e")
                        .and_then(|e| e.as_i64())
                        .map(|e| e > t)
                        .unwrap_or(true)
                });
                let removed = before - m.len();
                store(&path, &m)?;
                Ok(
                    json!({ "capability": "cache", "op": "purge", "removed": removed, "remaining": m.len() }),
                )
            }
            other => bail!("cache op {other:?} unsupported (get | set | ttl | purge)"),
        }
    })
}

// ── queue ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct QueueCap {
    pub path: String,
    /// `push` | `pop` | `peek` | `length` | `clear`
    pub op: String,
}

pub fn call_queue(c: &QueueCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let raw = stringify(&expand(
        &Value::String(if c.path.is_empty() {
            with.get("path").map(stringify).unwrap_or_default()
        } else {
            c.path.clone()
        }),
        state,
        with,
    ));
    let path = resolve_store_path(&raw, policy)?;
    with_lock(&path, || {
        // Missing file => empty queue. A present-but-corrupt file is an error: the
        // old `.ok()` chain treated it as empty and the following write then
        // destroyed it (observed: a corrupt file became `[null]`, reporting success).
        let mut items = load_array(&path)?;
        let save = |items: &Vec<Value>| -> Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            write_atomic(&path, &serde_json::to_string(&Value::Array(items.clone()))?)?;
            Ok(())
        };
        let op = effective_op(&c.op, with, "length");
        match op.as_str() {
            "push" => {
                let v = expand(
                    &with.get("value").cloned().unwrap_or(Value::Null),
                    state,
                    with,
                );
                items.push(v.clone());
                save(&items)?;
                Ok(
                    json!({ "capability": "queue", "op": "push", "value": v, "length": items.len() }),
                )
            }
            "pop" => {
                let had = !items.is_empty();
                let v = if had { items.remove(0) } else { Value::Null };
                save(&items)?;
                Ok(
                    json!({ "capability": "queue", "op": "pop", "found": had, "value": v, "length": items.len() }),
                )
            }
            "peek" => Ok(json!({
                "capability": "queue", "op": "peek",
                "value": items.first().cloned().unwrap_or(Value::Null), "length": items.len(),
            })),
            "length" => Ok(json!({ "capability": "queue", "op": "length", "length": items.len() })),
            "clear" => {
                let n = items.len();
                items.clear();
                save(&items)?;
                Ok(json!({ "capability": "queue", "op": "clear", "removed": n, "length": 0 }))
            }
            other => bail!("queue op {other:?} unsupported (push|pop|peek|length|clear)"),
        }
    })
}
