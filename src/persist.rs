//! Per-node persistent store for the Laya workflow engine.
//!
//! Every time `ResilientWorkflow` runs with a `NodeStore`, one atomic record is
//! written per node execution (`0001.json`, `0002.json`, …). Each record holds
//! the complete observable state of that step so any later process can:
//!
//!  * **Track** — list every node that ran, when, with which answer, and its
//!    full state-after snapshot.
//!  * **Rewind** — materialise the exact state that existed at any iteration.
//!  * **Replay** — re-run a single node from its materialised `state_before`.
//!
//! ## File layout
//! ```text
//! <dir>/
//!   manifest.json        # workflow name + start node + spec version (optional)
//!   runs/
//!     0001.json          # iteration 1 node record
//!     0002.json          # iteration 2 node record
//!     …
//! ```
//!
//! ## Atomicity
//! Each write is a temp file + `rename()` so a crash mid-write leaves the
//! previous record intact. `read_all` sorts by iteration, so a gap (missing
//! file) is simply skipped rather than causing a hole in the materialised
//! state.

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// A single workflow node's complete observable state after one execution.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeRecord {
    pub node: String,
    pub iteration: u64,
    pub timestamp_ms: i64,
    /// Complete state snapshot **before** this node's payload merged.
    pub state_before: Value,
    /// Complete state snapshot **after** this node's payload merged.
    pub state_after: Value,
    /// Node action payload (from the app runner or a `call` capability).
    pub payload: Option<Value>,
    /// `route` | `execute` | `stop` | `escalate` | `retry`
    pub action: String,
    pub edge_answer: Value,
    pub confidence: f64,
    pub latency_ms: f64,
    pub next_node: Option<String>,
    pub detail: Option<String>,
    pub error: Option<String>,
}

impl NodeRecord {
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("node".into(), json!(self.node));
        m.insert("iteration".into(), json!(self.iteration));
        m.insert("timestamp_ms".into(), json!(self.timestamp_ms));
        m.insert("state_before".into(), self.state_before.clone());
        m.insert("state_after".into(), self.state_after.clone());
        m.insert("payload".into(), self.payload.clone().unwrap_or(Value::Null));
        m.insert("action".into(), json!(self.action));
        m.insert("edge_answer".into(), self.edge_answer.clone());
        m.insert("confidence".into(), json!(self.confidence));
        m.insert("latency_ms".into(), json!(self.latency_ms));
        m.insert("next_node".into(), self.next_node.clone().map(|v| json!(v)).unwrap_or(Value::Null));
        m.insert("detail".into(), self.detail.clone().map(|v| json!(v)).unwrap_or(Value::Null));
        m.insert("error".into(), self.error.clone().map(|v| json!(v)).unwrap_or(Value::Null));
        Value::Object(m)
    }

    pub fn from_json(v: &Value) -> Result<Self> {
        let obj = v
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("node record: expected object"))?;
        Ok(Self {
            node: obj.get("node").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            iteration: obj.get("iteration").and_then(|x| x.as_u64()).unwrap_or(0),
            timestamp_ms: obj.get("timestamp_ms").and_then(|x| x.as_i64()).unwrap_or(0),
            state_before: obj.get("state_before").cloned().unwrap_or(Value::Object(Map::new())),
            state_after: obj.get("state_after").cloned().unwrap_or(Value::Object(Map::new())),
            payload: obj.get("payload").filter(|x| !x.is_null()).cloned(),
            action: obj.get("action").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            edge_answer: obj.get("edge_answer").cloned().unwrap_or(Value::Null),
            confidence: obj.get("confidence").and_then(|x| x.as_f64()).unwrap_or(0.0),
            latency_ms: obj.get("latency_ms").and_then(|x| x.as_f64()).unwrap_or(0.0),
            next_node: obj.get("next_node").and_then(|x| x.as_str()).map(|s| s.to_string()),
            detail: obj.get("detail").and_then(|x| x.as_str()).map(|s| s.to_string()),
            error: obj.get("error").and_then(|x| x.as_str()).map(|s| s.to_string()),
        })
    }
}

/// A durable, directory-backed, per-node workflow store.
pub struct NodeStore {
    pub dir: PathBuf,
    pub runs_dir: PathBuf,
    pub manifest_path: PathBuf,
}

impl NodeStore {
    pub fn open(dir: &str) -> Result<Self> {
        let d = PathBuf::from(dir);
        let runs = d.join("runs");
        let manifest = d.join("manifest.json");
        std::fs::create_dir_all(&runs).with_context(|| format!("creating store dir {d:?}"))?;
        Ok(Self {
            dir: d,
            runs_dir: runs,
            manifest_path: manifest,
        })
    }

    /// Write a workflow-level manifest (idempotent; safe to call many times).
    pub fn write_manifest(&self, workflow: &str, start: &str, state: &Value) -> Result<()> {
        let m = json!({
            "workflow": workflow,
            "start": start,
            "created_at_ms": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
            "initial_state": state,
        });
        self.atomic_write(&self.manifest_path, &m)
    }

    /// Write one node record (atomic; will not corrupt an existing record).
    pub fn write(&self, rec: &NodeRecord) -> Result<()> {
        if rec.iteration == 0 {
            bail!("node record iteration must be >= 1");
        }
        let p = self.runs_dir.join(format!("{:04}.json", rec.iteration));
        self.atomic_write(&p, &rec.to_json())
    }

    pub fn read(&self, iteration: u64) -> Result<Option<NodeRecord>> {
        if iteration == 0 {
            return Ok(None);
        }
        let p = self.runs_dir.join(format!("{:04}.json", iteration));
        if !p.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&p).with_context(|| format!("reading {p:?}"))?;
        let v: Value = serde_json::from_str(&raw).with_context(|| format!("parsing {p:?}"))?;
        Ok(Some(NodeRecord::from_json(&v)?))
    }

    /// Every stored record, sorted by iteration ascending.
    pub fn read_all(&self) -> Result<Vec<NodeRecord>> {
        let mut entries: Vec<(u64, PathBuf)> = Vec::new();
        for e in std::fs::read_dir(&self.runs_dir)?.filter_map(|e| e.ok()) {
            let p = e.path();
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if let Ok(n) = stem.parse::<u64>() {
                if stem.len() == 4 {
                    entries.push((n, p));
                }
            }
        }
        entries.sort_by_key(|(n, _)| *n);
        let mut out = Vec::with_capacity(entries.len());
        for (_, p) in entries {
            let raw = std::fs::read_to_string(&p)
                .with_context(|| format!("reading {p:?}"))?;
            let v: Value = serde_json::from_str(&raw).with_context(|| format!("parsing {p:?}"))?;
            out.push(NodeRecord::from_json(&v)?);
        }
        Ok(out)
    }

    /// The highest iteration currently stored (None when the store is empty).
    pub fn last_iteration(&self) -> Result<Option<u64>> {
        let mut best: Option<u64> = None;
        for e in std::fs::read_dir(&self.runs_dir)?.filter_map(|e| e.ok()) {
            let stem = e.file_name();
            let s = stem.to_string_lossy();
            if let Ok(n) = s.trim_end_matches(".json").parse::<u64>() {
                if best.map(|b| n > b).unwrap_or(true) {
                    best = Some(n);
                }
            }
        }
        Ok(best)
    }

    /// All iterations that have a stored record, ascending.
    pub fn iterations(&self) -> Result<Vec<u64>> {
        let mut v: Vec<u64> = Vec::new();
        for e in std::fs::read_dir(&self.runs_dir)?.filter_map(|e| e.ok()) {
            let stem = e.file_name();
            let s = stem.to_string_lossy();
            if let Ok(n) = s.trim_end_matches(".json").parse::<u64>() {
                if n > 0 {
                    v.push(n);
                }
            }
        }
        v.sort_unstable();
        Ok(v)
    }

    /// Delete a single record.
    pub fn delete(&self, iteration: u64) -> Result<()> {
        let p = self.runs_dir.join(format!("{:04}.json", iteration));
        if p.exists() {
            std::fs::remove_file(&p)?;
        }
        Ok(())
    }

    /// Delete a record **and every record after it** (rewind support).
    pub fn delete_from(&self, from_iteration: u64) -> Result<usize> {
        let mut removed = 0usize;
        for n in self.iterations()? {
            if n >= from_iteration {
                self.delete(n)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Remove all run records (manifest is left alone).
    pub fn clear(&self) -> Result<usize> {
        let its = self.iterations()?;
        let n = its.len();
        for i in its {
            self.delete(i)?;
        }
        Ok(n)
    }

    /// Rebuild the workflow state as of a given iteration (inclusive).
    /// Iterations that are missing are silently skipped (gap-tolerant).
    pub fn materialise_state(&self, through: u64) -> Result<Value> {
        let all = self.read_all()?;
        let mut state = Value::Object(Map::new());
        // Replay in order so later iterations overwrite earlier ones, matching
        // how the engine merges node payloads.
        for r in &all {
            if r.iteration > through {
                continue;
            }
            if let (Some(obj), Some(p)) = (
                state.as_object_mut(),
                r.state_after.as_object(),
            ) {
                for (k, v) in p {
                    obj.insert(k.clone(), v.clone());
                }
            } else if !r.state_after.is_object() && r.state_after != Value::Null {
                state = r.state_after.clone();
            }
        }
        Ok(state)
    }

    /// Convenience: state as of the last committed record (or `{}`).
    pub fn latest_state(&self) -> Result<Value> {
        match self.last_iteration()? {
            Some(n) => self.materialise_state(n),
            None => Ok(Value::Object(Map::new())),
        }
    }

    fn atomic_write(&self, path: &Path, value: &Value) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = path.with_extension(format!("tmp.{}.{}", pid, nanos));
        std::fs::write(&tmp, serde_json::to_string_pretty(value)?)?;
        match std::fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e).with_context(|| format!("renaming {tmp:?} → {path:?}"))
            }
        }
    }
}
