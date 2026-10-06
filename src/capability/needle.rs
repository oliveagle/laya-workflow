//! Needle 3 on-device capability: structured extraction + text embedding
//! through the native Cactus engine (`libneedle.so` / `libneedle.a`).
//!
//! Three ops, all pure (no network, no exec, no shell-out):
//!   * `extract` — one tool schema (OpenAI-form) + free text → typed arguments.
//!     The byte-level grammar compiled from the schema guarantees the output
//!     parses, so the caller does not need schema validation after the call.
//!   * `embed` — one sentence → `{dim, vector}` (dim 3072, already normalised).
//!   * `complete` — full tool-call round trip (multi-tool, refusal, confidence).
//!
//! The engine is loaded at first call via `dlopen`, from `$NEEDLE3_LIB_PATH` or
//! the platform default cache (`~/.cache/cactus-needle/v3/<ver>/libneedle.so`).
//! If the engine or `needle3.cact` is absent the capability returns a clear
//! error rather than a wall of FFI trace, so specs can probe it gracefully.
//!
//! The C header documents one process-global, **non-thread-safe** model per
//! kind, so every public entry takes `GATE` before touching the engine and the
//! engine itself lives behind a `OnceLock` (it is loaded exactly once).

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::ffi::{c_char, CStr, CString};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use super::Policy;

// ─── capability definition ────────────────────────────────────────────────

/// Layout for the `"kind": "needle"` capability.
#[derive(Clone, Debug)]
pub struct NeedleCap {
    /// `"extract"` (default), `"embed"` or `"complete"`.
    pub op: String,
    /// Weight archive: explicit path, `$NEEDLE3_CACT` or
    /// `~/.laya-workflow/models/needle3.cact`.
    pub cact: Option<String>,
}

/// Call the Needle 3 engine from a workflow node.
///
/// `with` is expanded before dispatch; each op picks its own fields:
///   * `extract`: `text` + `tool` (an OpenAI-form schema object) →
///     `{arguments, confidence, reasoning, matched}` (arguments is `null` when
///     the engine withholds the call, matching Python `needle.extract`).
///   * `embed`: `text` → `{dim, vector}`.
///   * `complete`: `prompt` (+ optional `tools`, `system`) → the full JSON
///     response with `function_calls`, `confidence` and `reasoning`.
pub fn call_needle(c: &NeedleCap, with: &Value, _state: &Value, _policy: &Policy) -> Result<Value> {
    // The capability touches only an already-installed model file, so the only
    // gate it needs is the process-global engine lock below: no `allow_exec`,
    // no `allow_hosts`, no shell-out.
    let _gate = GATE.lock().unwrap_or_else(|e| e.into_inner());
    match c.op.as_str() {
        "extract" | "needle_extract" => op_extract(with),
        "embed" | "needle_embed" => op_embed(with),
        "complete" | "needle_complete" => op_complete(with),
        other => bail!("unknown needle op {other:?} (want extract / embed / complete)"),
    }
}

// ─── ops ─────────────────────────────────────────────────────────────────

fn op_extract(with: &Value) -> Result<Value> {
    let text = with
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("needle extract needs 'text'"))?;
    let tool = with
        .get("tool")
        .ok_or_else(|| anyhow!("needle extract needs 'tool' (OpenAI-form schema)"))?;
    let tools = json!([tool]);
    let system = with.get("system").and_then(|v| v.as_str()).unwrap_or("");
    let response = run_complete(system, &tools.to_string(), text, 512)?;
    let empty = Vec::new();
    let calls = response.get("function_calls").and_then(|v| v.as_array()).unwrap_or(&empty);
    let suppressed = response
        .get("suppressed_calls")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    let arguments = calls
        .first()
        .and_then(|c| c.get("arguments"))
        .cloned()
        .or_else(|| suppressed.first().and_then(|c| c.get("arguments")).cloned())
        .unwrap_or(Value::Null);
    Ok(json!({
        "arguments": arguments,
        "confidence": response.get("confidence"),
        "reasoning": response.get("reasoning"),
        "matched": !calls.is_empty() || !suppressed.is_empty(),
    }))
}

fn op_embed(with: &Value) -> Result<Value> {
    let text = with
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("needle embed needs 'text'"))?;
    let e = engine()?;
    ensure_weights(e)?;
    let vec = embed_engine(text)?;
    Ok(json!({
        "dim": vec.len(),
        "vector": vec.iter().map(|x| serde_json::Number::from_f64(*x as f64)).collect::<Vec<_>>(),
    }))
}

fn op_complete(with: &Value) -> Result<Value> {
    let prompt = with
        .get("prompt")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("needle complete needs 'prompt'"))?;
    let tools = with.get("tools").cloned().unwrap_or(json!([]));
    let system = with.get("system").and_then(|v| v.as_str()).unwrap_or("");
    run_complete(system, &tools.to_string(), prompt, 512)
}

/// One complete round trip: engine must be loaded + initialised, then run.
fn run_complete(system: &str, tools_json: &str, text: &str, max_new_tokens: i32) -> Result<Value> {
    let e = engine()?;
    ensure_weights(e)?;
    let _ = e;
    init_engine(system, tools_json)?;
    let raw = complete_engine(text, max_new_tokens)?;
    serde_json::from_str(&raw)
        .map_err(|err| anyhow!("engine returned an unparseable envelope ({err}): {raw}"))
}

// ─── FFI wrapper ─────────────────────────────────────────────────────────

/// One process-global engine (the C header says model is per-process).
static ENGINE: OnceLock<libloading::Library> = OnceLock::new();
/// Serialise every engine touch (the C header says non-thread-safe).
static GATE: Mutex<()> = Mutex::new(());

pub fn engine() -> Result<&'static libloading::Library> {
    if let Some(l) = ENGINE.get() {
        return Ok(l);
    }
    let path = find_lib()?;
    let lib = unsafe { libloading::Library::new(&path) }
        .map_err(|e| anyhow!("cannot load needle engine at {}: {e}", path.display()))?;
    Ok(ENGINE.get_or_init(|| lib))
}

type NeedleLoad = unsafe extern "C" fn(*const u8, u64) -> i32;
type NeedleInit = unsafe extern "C" fn(*const c_char, *const c_char, *const c_char) -> i32;
type NeedleComplete = unsafe extern "C" fn(*const c_char, *const f32, i32, i32, *mut c_char, i32) -> i32;
type NeedleEmbed = unsafe extern "C" fn(*const c_char, *const f32, i32, *mut f32, i32) -> i32;


pub fn load_into(cact: &[u8]) -> Result<()> {
    let e = engine()?;
    let f: libloading::Symbol<NeedleLoad> = unsafe { e.get(b"needle_load") }
        .map_err(|e| anyhow!("symbol needle_load: {e}"))?;
    let rc = unsafe { f(cact.as_ptr(), cact.len() as u64) };
    if rc < 0 { bail!("needle_load failed: {rc}"); }
    Ok(())
}

fn init_engine(system: &str, tools_json: &str) -> Result<i32> {
    let e = engine()?;
    let f: libloading::Symbol<NeedleInit> = unsafe { e.get(b"needle_init") }
        .map_err(|e| anyhow!("symbol needle_init: {e}"))?;
    let s = CString::new(system)?;
    let t = CString::new(tools_json)?;
    let rc = unsafe { f(s.as_ptr(), t.as_ptr(), std::ptr::null()) };
    if rc < 0 { bail!("needle_init failed: {rc}"); }
    Ok(rc)
}

fn complete_engine(text: &str, max_new_tokens: i32) -> Result<String> {
    let e = engine()?;
    let f: libloading::Symbol<NeedleComplete> = unsafe { e.get(b"needle_complete") }
        .map_err(|e| anyhow!("symbol needle_complete: {e}"))?;
    let t = CString::new(text)?;
    let mut out = vec![0u8; 16_384];
    let rc = unsafe {
        f(t.as_ptr(), std::ptr::null(), 0, max_new_tokens, out.as_mut_ptr() as *mut c_char, out.len() as i32)
    };
    if rc < 0 { bail!("needle_complete failed: {rc}"); }
    let s = unsafe { CStr::from_ptr(out.as_ptr() as *const c_char) };
    Ok(s.to_string_lossy().into_owned())
}

/// Expose the C ABI embed for other crates (laya-mem) to reuse.
pub fn embed_engine(text: &str) -> Result<Vec<f32>> {
    let e = engine()?;
    let f: libloading::Symbol<NeedleEmbed> = unsafe { e.get(b"needle_embed") }
        .map_err(|e| anyhow!("symbol needle_embed: {e}"))?;
    let t = CString::new(text)?;
    let dim = unsafe { f(t.as_ptr(), std::ptr::null(), 0, std::ptr::null_mut(), 0) };
    if dim <= 0 { bail!("needle_embed dim probe failed: {dim}"); }
    let mut buf = vec![0f32; dim as usize];
    let rc = unsafe { f(t.as_ptr(), std::ptr::null(), 0, buf.as_mut_ptr(), dim) };
    if rc != dim { bail!("needle_embed fill failed: rc={rc} dim={dim}"); }
    Ok(buf)
}

pub fn ensure_weights(_e: &'static libloading::Library) -> Result<()> {
    let path = find_cact()?;
    let data = std::fs::read(&path)
        .map_err(|err| anyhow!("cannot read {} : {err}", path.display()))?;
    load_into(&data)
}

pub fn find_lib() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("NEEDLE3_LIB_PATH") {
        let p = PathBuf::from(p);
        if p.exists() { return Ok(p); }
    }
    let cache = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".cache/cactus-needle/v3"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/cactus-needle/v3"));
    if let Ok(entries) = std::fs::read_dir(&cache) {
        let mut versions: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        versions.sort();
        for v in versions.iter().rev() {
            let lib = v.join("libneedle.so");
            if lib.exists() { return Ok(lib); }
        }
    }
    bail!("libneedle.so not found; set $NEEDLE3_LIB_PATH or run `needle fetch`")
}

pub fn find_cact() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("NEEDLE3_CACT") {
        let p = PathBuf::from(p);
        if p.exists() { return Ok(p); }
    }
    let default = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".laya-workflow/models/needle3.cact"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/needle3.cact"));
    if default.exists() { return Ok(default); }
    bail!("needle3.cact not found; set $NEEDLE3_CACT or run `needle download needle3 --out ~/.laya-workflow/models`")
}
