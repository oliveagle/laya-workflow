//! Singleton-Chrome browser capability.
//!
//! This is the Laya-side counterpart of a "drive the real browser" extension:
//! workflows can open a page, capture an accessibility-oriented element table,
//! and send real CDP mouse/key events. The important invariant is that a
//! process owns **at most one Chrome instance**. Concurrent workflow threads
//! share that instance and use separate CDP targets; CDP request ids keep
//! replies from different sockets independent.
//!
//! Security follows the other network capabilities:
//! * the CDP endpoint must pass `policy.allow_hosts`;
//! * a non-empty `allow_hosts` is also applied to navigation URLs and the URL
//!   of the target being inspected;
//! * starting Chrome requires `policy.allow_exec`.
//!
//! The bundled extension in `extensions/laya-browser` exposes
//! `window.__layaBrowser` in the page's main world. It adds stable element
//! indexes and optional numbered badges. A plain Runtime.evaluate fallback is
//! used if the extension is unavailable.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::{bounded_timeout, check_host, expand, host_of, stringify, truncate, Policy};

#[derive(Clone, Debug)]
pub struct BrowserCap {
    /// Chrome DevTools HTTP endpoint (for example `http://127.0.0.1:9222`).
    pub endpoint: String,
    pub chrome_binary: String,
    pub profile_dir: String,
    pub extension_path: String,
    /// Start Chrome only when explicitly requested. Connecting to an already
    /// running endpoint never needs `allow_exec`.
    pub launch: bool,
    pub startup_timeout_ms: u64,
    pub timeout_ms: u64,
    pub max_text: usize,
    /// Upper bound for Laya-owned pages, including explicitly pinned pages.
    pub max_owned_pages: usize,
    /// Idle age after which an auto-close page is eligible for GC.
    pub owned_idle_ms: u64,
}

/// Guard for a singleton lock file. The process cannot hold a Chrome profile
/// lock through this file alone, but it prevents two Laya processes racing to
/// launch the same profile.
#[derive(Debug)]
struct SingletonLock {
    file: std::fs::File,
}

impl SingletonLock {
    fn acquire(path: std::path::PathBuf) -> Result<Self> {
        std::fs::create_dir_all(path.parent().unwrap_or_else(|| std::path::Path::new(".")))?;
        for _ in 0..3 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => {
                    let mut guard = Self { file };
                    writeln!(guard.file, "{}", std::process::id())?;
                    guard.file.flush()?;
                    return Ok(guard);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let pid = std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|s| s.trim().parse::<i32>().ok());
                    let alive = pid
                        .map(|p| {
                            // `kill(pid, 0)` is the portable-enough liveness probe on Unix.
                            #[cfg(unix)]
                            unsafe {
                                libc_kill(p, 0) == 0
                            }
                            #[cfg(not(unix))]
                            {
                                let _ = p;
                                true
                            }
                        })
                        .unwrap_or(true);
                    if alive {
                        std::thread::sleep(Duration::from_millis(50));
                    } else {
                        let _ = std::fs::remove_file(&path);
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        bail!(
            "another Laya process holds Chrome singleton lock {}",
            path.display()
        );
    }
}

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

struct BrowserRuntime {
    endpoint: String,
    owner_dir: std::path::PathBuf,
    /// Kept open for the life of the process when this process launched Chrome.
    _lock: Option<Arc<SingletonLock>>,
}

#[derive(Clone, Debug)]
struct OwnedTarget {
    id: String,
    pinned: bool,
    last_used_ms: u128,
}

#[derive(Default)]
struct OwnedTargetRegistry {
    endpoint: Option<String>,
    owner_dir: std::path::PathBuf,
    targets: Vec<OwnedTarget>,
}

static BROWSER_RUNTIME: OnceLock<Mutex<Option<BrowserRuntime>>> = OnceLock::new();

fn runtime_cell() -> &'static Mutex<Option<BrowserRuntime>> {
    BROWSER_RUNTIME.get_or_init(|| Mutex::new(None))
}

static BROWSER_TARGETS: OnceLock<Mutex<OwnedTargetRegistry>> = OnceLock::new();

fn target_registry_cell() -> &'static Mutex<OwnedTargetRegistry> {
    BROWSER_TARGETS.get_or_init(|| Mutex::new(OwnedTargetRegistry::default()))
}

fn current_epoch_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

fn write_json_atomic(path: &std::path::Path, value: &Value) -> Result<()> {
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn save_owned_targets_locked(registry: &OwnedTargetRegistry) {
    let path = registry.owner_dir.join("laya-owned-targets.json");
    let value = json!({
        "owner": "laya-workflow",
        "endpoint": registry.endpoint,
        "targets": registry.targets.iter().map(|t| json!({
            "id": t.id, "pinned": t.pinned, "last_used_ms": t.last_used_ms as u64
        })).collect::<Vec<_>>()
    });
    let _ = write_json_atomic(&path, &value);
}

fn runtime_owner_dir(endpoint: &str) -> Option<std::path::PathBuf> {
    let guard = runtime_cell().lock().ok()?;
    guard
        .as_ref()
        .filter(|rt| rt.endpoint == endpoint)
        .map(|rt| rt.owner_dir.clone())
}

fn initialize_owned_targets(owner_dir: &std::path::Path, endpoint: &str) {
    #[cfg(unix)]
    install_process_exit_cleanup();
    if let Ok(mut registry) = target_registry_cell().lock() {
        registry.endpoint = Some(endpoint.to_string());
        registry.owner_dir = owner_dir.to_path_buf();
        let path = owner_dir.join("laya-owned-targets.json");
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                if value.get("owner").and_then(Value::as_str) == Some("laya-workflow")
                    && value.get("endpoint").and_then(Value::as_str) == Some(endpoint)
                {
                    registry.targets = value
                        .get("targets")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|t| {
                                    Some(OwnedTarget {
                                        id: t.get("id")?.as_str()?.to_string(),
                                        pinned: t.get("pinned").and_then(Value::as_bool)?,
                                        last_used_ms: t
                                            .get("last_used_ms")
                                            .and_then(Value::as_u64)?
                                            as u128,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                }
            }
        }
        save_owned_targets_locked(&registry);
    }
}

#[cfg(unix)]
extern "C" {
    fn atexit(callback: extern "C" fn()) -> i32;
}

#[cfg(unix)]
extern "C" fn close_owned_targets_at_exit() {
    // This callback crosses the C boundary, so it must not unwind. The lock is
    // released before any network I/O to avoid holding it during shutdown.
    let pending = std::panic::catch_unwind(take_auto_close_targets)
        .ok()
        .flatten();
    if let Some((endpoint, ids)) = pending {
        for id in ids {
            let _ = http_close(&endpoint, &id, Duration::from_secs(1));
        }
    }
}

#[cfg(unix)]
fn install_process_exit_cleanup() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    if INSTALLED.set(()).is_ok() {
        unsafe {
            let _ = atexit(close_owned_targets_at_exit);
        }
    }
}

fn track_owned_target(endpoint: &str, id: &str, keep_open: bool) {
    if runtime_owner_dir(endpoint).is_none() {
        return;
    }
    #[cfg(unix)]
    install_process_exit_cleanup();
    if let Ok(mut registry) = target_registry_cell().lock() {
        if registry.endpoint.is_none() {
            registry.endpoint = Some(endpoint.to_string());
        }
        if let Some(target) = registry.targets.iter_mut().find(|t| t.id == id) {
            target.pinned = keep_open;
            target.last_used_ms = current_epoch_ms();
        } else {
            registry.targets.push(OwnedTarget {
                id: id.to_string(),
                pinned: keep_open,
                last_used_ms: current_epoch_ms(),
            });
        }
        save_owned_targets_locked(&registry);
    }
}

fn forget_owned_targets(ids: &[String]) {
    if let Ok(mut registry) = target_registry_cell().lock() {
        registry.targets.retain(|t| !ids.contains(&t.id));
        save_owned_targets_locked(&registry);
    }
}

fn forget_owned_target(id: &str) {
    forget_owned_targets(&[id.to_string()]);
}

fn take_auto_close_targets() -> Option<(String, Vec<String>)> {
    let mut registry = target_registry_cell().lock().ok()?;
    let endpoint = registry.endpoint.clone()?;
    let mut removed = Vec::new();
    registry.targets.retain(|t| {
        if t.pinned {
            true
        } else {
            removed.push(t.id.clone());
            false
        }
    });
    save_owned_targets_locked(&registry);
    Some((endpoint, removed))
}

fn clear_owned_targets() {
    if let Ok(mut registry) = target_registry_cell().lock() {
        registry.endpoint = None;
        registry.targets.clear();
        save_owned_targets_locked(&registry);
    }
}

fn touch_owned_target(endpoint: &str, id: &str) {
    if let Ok(mut registry) = target_registry_cell().lock() {
        if registry.endpoint.as_deref() != Some(endpoint) {
            return;
        }
        if let Some(target) = registry.targets.iter_mut().find(|t| t.id == id) {
            target.last_used_ms = current_epoch_ms();
            save_owned_targets_locked(&registry);
        }
    }
}

fn default_profile_dir() -> String {
    if let Ok(dir) = std::env::var("LAYA_BROWSER_PROFILE") {
        return dir;
    }
    if let Ok(home) = std::env::var("HOME") {
        return format!("{home}/.laya-workflow/chrome");
    }
    ".laya-workflow/chrome".to_string()
}

fn repo_extension_path() -> String {
    if let Ok(path) = std::env::var("LAYA_BROWSER_EXTENSION") {
        return path;
    }
    // The CLI is normally executed from the repository root. Fall back to the
    // source path known at compile time for direct Rust tests.
    if std::path::Path::new("extensions/laya-browser/manifest.json").is_file() {
        return "extensions/laya-browser".to_string();
    }
    concat!(env!("CARGO_MANIFEST_DIR"), "/extensions/laya-browser").to_string()
}

fn default_chrome_binary() -> String {
    if let Ok(path) = std::env::var("LAYA_CHROME_BINARY") {
        if !path.is_empty() {
            return path;
        }
    }
    if cfg!(target_os = "macos") {
        return "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string();
    }
    if cfg!(target_os = "windows") {
        return r"C:\Program Files\Google\Chrome\Application\chrome.exe".to_string();
    }
    for name in [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "chrome",
    ] {
        let candidate = std::path::Path::new(name);
        // PATH is searched by Command::new, but avoid selecting a relative file by accident.
        if candidate.is_absolute() && candidate.is_file() {
            return name.to_string();
        }
    }
    "google-chrome".to_string()
}

/// Chrome 137+ intentionally ignores `--load-extension` in branded builds.
/// The supported replacement is a browser-level CDP `Extensions.loadUnpacked`
/// call after Chrome is launched with `--enable-unsafe-extension-debugging`.
fn load_unpacked_extension(endpoint: &str, extension: &str, timeout: Duration) -> Result<String> {
    let version = http_json(endpoint, "GET", "/json/version", timeout)?;
    let ws = version
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Chrome did not expose a browser websocket"))?;
    let mut socket = tungstenite::connect(ws)
        .map_err(|e| anyhow!("connect Chrome browser websocket failed: {e}"))?
        .0;
    set_ws_timeout(&mut socket, Duration::from_millis(250));
    socket.send(tungstenite::Message::Text(serde_json::to_string(&json!({
        "id": 1,
        "method": "Extensions.loadUnpacked",
        "params": {"path": extension}
    }))?))?;
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            bail!("Extensions.loadUnpacked timed out");
        }
        let message = match socket.read() {
            Ok(message) => message,
            Err(tungstenite::Error::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(e) => return Err(anyhow!("Chrome browser websocket read failed: {e}")),
        };
        let text = match message {
            tungstenite::Message::Text(s) => s,
            tungstenite::Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
            _ => continue,
        };
        let value: Value = serde_json::from_str(&text)?;
        if value.get("id").and_then(Value::as_u64) != Some(1) {
            continue;
        }
        if let Some(error) = value.get("error") {
            bail!("Extensions.loadUnpacked failed: {error}");
        }
        return value
            .pointer("/result/id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("Extensions.loadUnpacked returned no extension id"));
    }
}

fn debug_extension_loaded(extension_id: &str) {
    if std::env::var("LAYA_CDP_DEBUG").is_ok() {
        println!("extension loaded: {extension_id}");
    }
}

fn normalized_endpoint(c: &BrowserCap, with: &Value, state: &Value) -> Result<String> {
    let raw = if c.endpoint.is_empty() {
        std::env::var("LAYA_BROWSER_CDP").unwrap_or_else(|_| "http://127.0.0.1:9222".to_string())
    } else {
        stringify(&expand(&Value::String(c.endpoint.clone()), state, with))
    };
    let endpoint = raw.trim_end_matches('/').to_string();
    if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
        bail!("browser endpoint must start with http:// or https://: {endpoint:?}");
    }
    Ok(endpoint)
}

fn endpoint_port(endpoint: &str) -> Result<u16> {
    endpoint
        .rsplit(':')
        .next()
        .and_then(|port| port.parse::<u16>().ok())
        .ok_or_else(|| anyhow!("browser endpoint has no numeric port: {endpoint}"))
}

fn process_command_contains(needles: &[String]) -> bool {
    #[cfg(unix)]
    {
        let Ok(output) = std::process::Command::new("ps")
            .args(["-axo", "command="])
            .output()
        else {
            return false;
        };
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| needles.iter().all(|needle| line.contains(needle.as_str())))
    }
    #[cfg(not(unix))]
    {
        let _ = needles;
        false
    }
}

fn owner_marker_path(profile_path: &std::path::Path) -> std::path::PathBuf {
    profile_path.join("laya-owner.json")
}

fn write_owner_marker(
    profile_path: &std::path::Path,
    endpoint: &str,
    pid: Option<u32>,
) -> Result<()> {
    let token = format!(
        "{:x}-{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    write_json_atomic(
        &owner_marker_path(profile_path),
        &json!({
            "owner": "laya-workflow",
            "endpoint": endpoint,
            "port": endpoint_port(endpoint)?,
            "profile": profile_path.canonicalize().unwrap_or_else(|_| profile_path.to_path_buf()),
            "owner_token": token,
            "launcher_pid": pid,
            "created_at_ms": current_epoch_ms() as u64
        }),
    )
}

/// An endpoint is reusable only when it belongs to the marked Laya profile and a
/// live process command line still contains that profile/port pair.
fn validate_owned_endpoint(
    endpoint: &str,
    profile_path: &std::path::Path,
    timeout: Duration,
) -> Result<()> {
    http_json(endpoint, "GET", "/json/version", timeout)?;
    let marker_path = owner_marker_path(profile_path);
    let marker = if marker_path.is_file() {
        serde_json::from_str::<Value>(
            &std::fs::read_to_string(&marker_path)
                .map_err(|e| anyhow!("read Laya Chrome owner marker failed: {e}"))?,
        )
        .map_err(|e| anyhow!("Laya Chrome owner marker is invalid: {e}"))?
    } else {
        // Migrate instances launched by earlier Laya builds only when both the
        // Laya lock file and Chrome's command line identify this exact profile.
        let canonical = profile_path
            .canonicalize()
            .unwrap_or_else(|_| profile_path.to_path_buf());
        let needles = vec![
            format!("--remote-debugging-port={}", endpoint_port(endpoint)?),
            canonical.display().to_string(),
        ];
        if !profile_path.join("laya-chrome.lock").is_file() || !process_command_contains(&needles) {
            bail!(
                "CDP endpoint {endpoint} is not a laya-workflow-owned singleton; set launch=true so Laya creates a marked instance"
            );
        }
        write_owner_marker(profile_path, endpoint, None)?;
        serde_json::from_str::<Value>(&std::fs::read_to_string(&marker_path)?)
            .map_err(|e| anyhow!("Laya Chrome owner marker is invalid: {e}"))?
    };

    if marker.get("owner").and_then(Value::as_str) != Some("laya-workflow")
        || marker.get("endpoint").and_then(Value::as_str) != Some(endpoint)
        || marker.get("port").and_then(Value::as_u64) != Some(endpoint_port(endpoint)? as u64)
    {
        bail!("Laya Chrome owner marker does not identify {endpoint}");
    }
    let canonical = profile_path
        .canonicalize()
        .unwrap_or_else(|_| profile_path.to_path_buf());
    let needles = vec![
        format!("--remote-debugging-port={}", endpoint_port(endpoint)?),
        canonical.display().to_string(),
    ];
    if !process_command_contains(&needles) {
        bail!("Laya-owned Chrome process was not found for {endpoint}");
    }
    Ok(())
}

fn ensure_runtime(c: &BrowserCap, with: &Value, state: &Value, policy: &Policy) -> Result<String> {
    let endpoint = normalized_endpoint(c, with, state)?;
    check_host(&endpoint, policy)?;
    let mut guard = runtime_cell()
        .lock()
        .map_err(|_| anyhow!("browser runtime lock poisoned"))?;
    if let Some(rt) = guard.as_ref() {
        if rt.endpoint != endpoint {
            bail!(
                "the Chrome CDP singleton is already bound to {}; refusing to open a second instance at {endpoint}",
                rt.endpoint
            );
        }
        return Ok(endpoint);
    }

    let profile_source = if c.profile_dir.is_empty() {
        default_profile_dir()
    } else {
        c.profile_dir.clone()
    };
    let profile = stringify(&expand(&Value::String(profile_source), state, with));
    if profile.is_empty() {
        bail!("browser launch needs profile_dir");
    }
    let profile_path = std::path::PathBuf::from(&profile);
    if !policy.allow_paths.is_empty() {
        let allowed = policy
            .allow_paths
            .iter()
            .any(|root| std::path::Path::new(&profile).starts_with(root));
        if !allowed {
            bail!("Chrome profile {profile:?} is outside policy.allow_paths");
        }
    }

    // Reuse only an instance that Laya launched and marked in its own profile.
    if http_json(
        &endpoint,
        "GET",
        "/json/version",
        bounded_timeout(c.timeout_ms, policy),
    )
    .is_ok()
    {
        validate_owned_endpoint(
            &endpoint,
            &profile_path,
            bounded_timeout(c.timeout_ms, policy),
        )?;
        initialize_owned_targets(&profile_path, &endpoint);
        *guard = Some(BrowserRuntime {
            endpoint: endpoint.clone(),
            owner_dir: profile_path.clone(),
            _lock: None,
        });
        return Ok(endpoint);
    }
    if !c.launch {
        bail!("Chrome CDP endpoint {endpoint} is not running and launch=false");
    }
    if !policy.allow_exec {
        bail!("browser launch spawns Chrome; set policy.allow_exec = true to enable");
    }

    let extension_source = if c.extension_path.is_empty() {
        repo_extension_path()
    } else {
        c.extension_path.clone()
    };
    let extension = stringify(&expand(&Value::String(extension_source), state, with));
    if extension.is_empty()
        || !std::path::Path::new(&extension)
            .join("manifest.json")
            .is_file()
    {
        bail!("browser extension is missing: {extension}");
    }

    let lock_path = profile_path.join("laya-chrome.lock");
    let lock = match SingletonLock::acquire(lock_path) {
        Ok(lock) => Arc::new(lock),
        Err(lock_err) => {
            let deadline = Instant::now() + bounded_timeout(c.startup_timeout_ms, policy);
            loop {
                if http_json(
                    &endpoint,
                    "GET",
                    "/json/version",
                    Duration::from_millis(500),
                )
                .is_ok()
                {
                    validate_owned_endpoint(
                        &endpoint,
                        &profile_path,
                        bounded_timeout(c.timeout_ms, policy),
                    )?;
                    initialize_owned_targets(&profile_path, &endpoint);
                    *guard = Some(BrowserRuntime {
                        endpoint: endpoint.clone(),
                        owner_dir: profile_path.clone(),
                        _lock: None,
                    });
                    return Ok(endpoint);
                }
                if Instant::now() >= deadline {
                    bail!("{lock_err}");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };

    std::fs::create_dir_all(&profile_path)?;
    let binary = if c.chrome_binary.is_empty() {
        default_chrome_binary()
    } else {
        c.chrome_binary.clone()
    };
    let mut cmd = std::process::Command::new(&binary);
    cmd.arg(format!(
        "--remote-debugging-port={}",
        endpoint.rsplit(':').next().unwrap_or("9222")
    ))
    .arg(format!(
        "--user-data-dir={}",
        profile_path
            .canonicalize()
            .unwrap_or(profile_path.clone())
            .display()
    ))
    .arg("--enable-unsafe-extension-debugging")
    .args([
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-background-networking",
    ])
    .arg("--start-maximized")
    .arg("about:blank")
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null());
    let child = cmd
        .spawn()
        .map_err(|e| anyhow!("spawn Chrome {binary:?} failed: {e}"))?;
    write_owner_marker(&profile_path, &endpoint, Some(child.id()))?;
    let _ = child;

    let deadline = Instant::now() + bounded_timeout(c.startup_timeout_ms, policy);
    loop {
        if http_json(
            &endpoint,
            "GET",
            "/json/version",
            Duration::from_millis(500),
        )
        .is_ok()
        {
            let extension = match load_unpacked_extension(
                &endpoint,
                &extension,
                bounded_timeout(c.timeout_ms, policy),
            ) {
                Ok(extension) => extension,
                Err(e) => {
                    if let Ok(version) =
                        http_json(&endpoint, "GET", "/json/version", Duration::from_secs(1))
                    {
                        if let Some(ws) =
                            version.get("webSocketDebuggerUrl").and_then(Value::as_str)
                        {
                            if let Ok(mut socket) = tungstenite::connect(ws) {
                                let _ = socket.0.send(tungstenite::Message::Text(
                                    serde_json::to_string(
                                        &json!({"id":1,"method":"Browser.close","params":{}}),
                                    )
                                    .unwrap_or_default(),
                                ));
                                let _ = socket.0.read();
                            }
                        }
                    }
                    let _ = std::fs::remove_file(owner_marker_path(&profile_path));
                    bail!(e);
                }
            };
            debug_extension_loaded(&extension);
            validate_owned_endpoint(
                &endpoint,
                &profile_path,
                bounded_timeout(c.timeout_ms, policy),
            )?;
            initialize_owned_targets(&profile_path, &endpoint);
            *guard = Some(BrowserRuntime {
                endpoint: endpoint.clone(),
                owner_dir: profile_path.clone(),
                _lock: Some(lock),
            });
            return Ok(endpoint);
        }
        if Instant::now() >= deadline {
            let _ = std::fs::remove_file(owner_marker_path(&profile_path));
            bail!("Chrome did not expose CDP at {endpoint} before startup timeout");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn http_json(endpoint: &str, method: &str, path: &str, timeout: Duration) -> Result<Value> {
    let url = format!("{endpoint}{path}");
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    let mut req = match method {
        "PUT" => agent.put(&url),
        _ => agent.get(&url),
    };
    req = req.set("accept", "application/json");
    let resp = req
        .call()
        .map_err(|e| anyhow!("Chrome CDP {method} {url} failed: {e}"))?;
    let status = resp.status();
    if status < 200 || status >= 300 {
        bail!("Chrome CDP {method} {url} returned HTTP {status}");
    }
    let text = truncate(
        resp.into_string().map_err(|e| anyhow!("read {url}: {e}"))?,
        4 << 20,
    );
    serde_json::from_str(&text).map_err(|e| anyhow!("Chrome CDP {url} returned invalid JSON: {e}"))
}

fn ws_endpoint(endpoint: &str, path: &str) -> Result<String> {
    let stripped = endpoint
        .strip_prefix("https://")
        .map(|rest| format!("wss://{rest}"))
        .unwrap_or_else(|| {
            endpoint
                .strip_prefix("http://")
                .map(|rest| format!("ws://{rest}"))
                .unwrap_or_else(|| endpoint.to_string())
        });
    Ok(format!("{stripped}{path}"))
}

/// Chrome's `/json/close/{id}` replies with plain text, so use a raw bounded
/// request instead of the JSON-decoding helper.
fn http_close(endpoint: &str, target_id: &str, timeout: Duration) -> Result<()> {
    let url = format!("{endpoint}/json/close/{target_id}");
    let resp = ureq::AgentBuilder::new()
        .timeout(timeout)
        .build()
        .get(&url)
        .set("accept", "text/plain, application/json")
        .call()
        .map_err(|e| anyhow!("Chrome CDP GET {url} failed: {e}"))?;
    let status = resp.status();
    if (200..300).contains(&status) {
        Ok(())
    } else {
        bail!("Chrome CDP GET {url} returned HTTP {status}")
    }
}

fn set_ws_timeout(
    ws: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
    timeout: Duration,
) {
    if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = ws.get_ref() {
        let _ = tcp.set_read_timeout(Some(timeout));
        let _ = tcp.set_write_timeout(Some(timeout));
    }
}

fn cdp_page_call(
    endpoint: &str,
    target_id: &str,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value> {
    touch_owned_target(endpoint, target_id);
    let mut ws = tungstenite::connect(ws_endpoint(
        endpoint,
        &format!("/devtools/page/{target_id}"),
    )?)
    .map_err(|e| anyhow!("connect Chrome page websocket failed: {e}"))?
    .0;
    set_ws_timeout(&mut ws, Duration::from_millis(250));
    // DevTools rejects large f64-like ids (for example nanos as u64), so each
    // short-lived page socket uses a small request id.
    let id = 1u64;
    ws.send(tungstenite::Message::Text(serde_json::to_string(
        &json!({"id": id, "method": method, "params": params}),
    )?))?;

    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("CDP {method} timed out after {}ms", timeout.as_millis());
        }
        // A short socket timeout prevents a stalled peer from bypassing the
        // caller-visible deadline while CDP events arrive.
        let message = match ws.read() {
            Ok(message) => message,
            Err(tungstenite::Error::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(e) => return Err(anyhow!("Chrome websocket read failed: {e}")),
        };
        match message {
            tungstenite::Message::Text(s) => {
                if std::env::var("LAYA_CDP_DEBUG").is_ok() {
                    println!("CDP TEXT: {s}");
                }
                let msg: Value = serde_json::from_str(&s)?;
                if msg.get("id").and_then(Value::as_u64) == Some(id) {
                    if let Some(err) = msg.get("error") {
                        bail!("CDP {method} failed: {err}");
                    }
                    return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
                }
            }
            tungstenite::Message::Ping(p) => ws.send(tungstenite::Message::Pong(p))?,
            tungstenite::Message::Close(c) => bail!("Chrome closed page websocket: {c:?}"),
            tungstenite::Message::Binary(b) => {
                if std::env::var("LAYA_CDP_DEBUG").is_ok() {
                    println!("CDP BINARY: {}", String::from_utf8_lossy(&b));
                }
                if let Ok(msg) = serde_json::from_slice::<Value>(&b) {
                    if msg.get("id").and_then(Value::as_u64) == Some(id) {
                        if let Some(err) = msg.get("error") {
                            bail!("CDP {method} failed: {err}");
                        }
                        return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Run one CDP command on the browser-level websocket. This is required for
/// `Target.createTarget`; page websockets cannot create sibling targets.
fn cdp_browser_call(
    endpoint: &str,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value> {
    // Modern Chrome requires the random browser token exposed by
    // /json/version; the older /devtools/browser alias returns 404.
    let info = http_json(endpoint, "GET", "/json/version", timeout)?;
    let ws_url = info
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Chrome did not expose a browser websocket"))?;
    let mut ws = tungstenite::connect(ws_url)
        .map_err(|e| anyhow!("connect Chrome browser websocket failed: {e}"))?
        .0;
    set_ws_timeout(&mut ws, Duration::from_millis(250));
    ws.send(tungstenite::Message::Text(serde_json::to_string(
        &json!({"id": 1u64, "method": method, "params": params}),
    )?))?;
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("CDP {method} timed out after {}ms", timeout.as_millis());
        }
        let message = match ws.read() {
            Ok(message) => message,
            Err(tungstenite::Error::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(e) => return Err(anyhow!("Chrome browser websocket read failed: {e}")),
        };
        match message {
            tungstenite::Message::Text(text) => {
                let msg: Value = serde_json::from_str(&text)?;
                if msg.get("id").and_then(Value::as_u64) == Some(1) {
                    if let Some(err) = msg.get("error") {
                        bail!("CDP {method} failed: {err}");
                    }
                    return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
                }
            }
            tungstenite::Message::Ping(p) => ws.send(tungstenite::Message::Pong(p))?,
            tungstenite::Message::Close(c) => {
                bail!("Chrome closed browser websocket: {c:?}")
            }
            _ => {}
        }
    }
}

fn list_targets(endpoint: &str, timeout: Duration) -> Result<Vec<Value>> {
    match http_json(endpoint, "GET", "/json/list", timeout)? {
        Value::Array(a) => Ok(a),
        _ => Ok(Vec::new()),
    }
}

/// Chrome can report a new tab as ready while the network response is still
/// being committed. Waiting here prevents the next browser step from selecting
/// an `about:blank` or error target too early.
fn wait_page_ready(
    endpoint: &str,
    target_id: &str,
    timeout: Duration,
    expected_url: Option<&str>,
) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    let expression = match expected_url {
        Some(expected) => format!(
            "(() => {{ const expected = {}; const a = new URL(location.href); const b = new URL(expected); const paramsMatch = [...b.searchParams].every(([k, v]) => a.searchParams.get(k) === v); return {{readyState: document.readyState, url: location.href, expectedMatch: a.origin === b.origin && a.pathname === b.pathname && paramsMatch}}; }})()",
            js_escape(&Value::String(expected.to_string()))
        ),
        None => r#"({readyState: document.readyState, url: location.href})"#.to_string(),
    };
    let mut last = json!({"readyState": "loading", "url": ""});
    loop {
        last = evaluate(
            endpoint,
            target_id,
            &expression,
            timeout.min(Duration::from_secs(2)),
            false,
        )
        .unwrap_or_else(|e| json!({"error": e.to_string()}));
        let ready = last.get("readyState").and_then(Value::as_str) == Some("complete");
        let committed = !last
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .starts_with("chrome-error://");
        let matched = expected_url.is_none()
            || last.get("expectedMatch").and_then(Value::as_bool) == Some(true);
        if ready && committed && matched {
            return Ok(last);
        }
        if Instant::now() >= deadline {
            bail!("Chrome page did not finish loading within timeout (last state: {last})");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn target_allowed(target: &Value, policy: &Policy) -> Result<()> {
    if policy.allow_hosts.is_empty() {
        return Ok(());
    }
    let url = target.get("url").and_then(Value::as_str).unwrap_or("");
    if url.is_empty()
        || url.starts_with("chrome://")
        || url.starts_with("chrome-extension://")
        || url.starts_with("devtools://")
        || url.starts_with("chrome-error://")
        || url.starts_with("about:")
    {
        return Ok(());
    }
    let h = host_of(url);
    if h.is_empty() || policy.allow_hosts.iter().any(|a| a == &h) {
        Ok(())
    } else {
        bail!("target host {h:?} is not in policy.allow_hosts (denied)")
    }
}

fn resolve_target(
    endpoint: &str,
    with: &Value,
    timeout: Duration,
    policy: &Policy,
) -> Result<Value> {
    let requested = with
        .get("target_id")
        .or_else(|| with.get("targetId"))
        .map(stringify);
    let targets = list_targets(endpoint, timeout)?;
    let selected;
    if let Some(id) = requested.filter(|s| !s.is_empty()) {
        selected = targets
            .iter()
            .find(|t| t.get("id").and_then(Value::as_str) == Some(id.as_str()));
        if selected.is_none() {
            bail!("Chrome target {id:?} was not found");
        }
    } else {
        selected = targets.iter().find(|t| {
            t.get("type").and_then(Value::as_str) == Some("page")
                && !t
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .starts_with("devtools://")
        });
    }
    let target = selected.ok_or_else(|| anyhow!("no Chrome page target is available"))?;
    target_allowed(target, policy)?;
    if let Some(id) = target.get("id").and_then(Value::as_str) {
        touch_owned_target(endpoint, id);
    }
    Ok(target.clone())
}

fn target_id(target: &Value) -> Result<String> {
    target
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("Chrome target has no id"))
}

/// Select disposable **page** targets without touching workers, extensions,
/// DevTools, or browser-internal targets. Explicit keeps always win.
fn select_cleanup_targets(
    targets: &[Value],
    close_url_prefixes: &[String],
    keep_url_prefixes: &[String],
    keep_target_ids: &[String],
    limit: usize,
    dedupe_urls: bool,
) -> (Vec<String>, Vec<Value>) {
    const PROTECTED_SCHEMES: [&str; 4] =
        ["chrome://", "chrome-extension://", "devtools://", "about:"];
    let explicitly_protected_scheme = PROTECTED_SCHEMES
        .iter()
        .any(|scheme| close_url_prefixes.iter().any(|p| p.starts_with(scheme)));
    let mut selected = Vec::new();
    let mut kept = Vec::new();
    let mut seen_urls = HashSet::new();

    for target in targets
        .iter()
        .filter(|t| t.get("type").and_then(Value::as_str) == Some("page"))
    {
        let id = target.get("id").and_then(Value::as_str).unwrap_or_default();
        let url = target
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let duplicate = dedupe_urls && !seen_urls.insert(url.to_string());
        let is_dangerous = PROTECTED_SCHEMES.iter().any(|p| url.starts_with(p));
        let keep_reason = if keep_target_ids.iter().any(|x| x == id) {
            Some("target_id")
        } else if keep_url_prefixes
            .iter()
            .any(|p| url.starts_with(p.as_str()))
        {
            Some("url")
        } else if is_dangerous && !explicitly_protected_scheme {
            Some("protected_scheme")
        } else {
            None
        };

        if keep_reason.is_none() && duplicate && selected.len() < limit {
            selected.push(id.to_string());
        } else if keep_reason.is_none()
            && close_url_prefixes
                .iter()
                .any(|p| url.starts_with(p.as_str()))
            && selected.len() < limit
        {
            selected.push(id.to_string());
            seen_urls.insert(url.to_string());
        } else {
            kept.push(json!({
                "target_id": id,
                "url": url,
                "kept": keep_reason.unwrap_or(if duplicate {
                    "limit"
                } else if dedupe_urls {
                    "first_url"
                } else if selected.len() >= limit {
                    "limit"
                } else {
                    "not_matching"
                })
            }));
        }
    }
    (selected, kept)
}

fn close_target_ids(endpoint: &str, ids: &[String], timeout: Duration) -> (usize, Vec<Value>) {
    let mut closed_count = 0usize;
    let mut errors = Vec::new();
    for id in ids {
        if http_close(endpoint, id, timeout).is_ok() {
            closed_count += 1;
        } else {
            errors.push(json!({"target_id": id}));
        }
    }
    forget_owned_targets(ids);
    (closed_count, errors)
}

/// Close tracked tabs even when an operation returns early via `?`. This makes
/// cleanup an invariant instead of something only the happy path remembers.
struct TargetCleanupGuard<'a> {
    endpoint: &'a str,
    ids: Vec<String>,
    disarmed: bool,
}

impl<'a> TargetCleanupGuard<'a> {
    fn new(endpoint: &'a str) -> Self {
        Self {
            endpoint,
            ids: Vec::new(),
            disarmed: false,
        }
    }

    fn track(&mut self, id: impl Into<String>) {
        self.ids.push(id.into());
    }

    fn untrack(&mut self, id: &str) -> bool {
        let before = self.ids.len();
        self.ids.retain(|existing| existing != id);
        before != self.ids.len()
    }

    fn finish(&mut self, close: bool, timeout: Duration) -> (usize, Vec<Value>) {
        let result = if close {
            close_target_ids(self.endpoint, &self.ids, timeout)
        } else {
            (0, Vec::new())
        };
        self.disarmed = true;
        result
    }
}

impl Drop for TargetCleanupGuard<'_> {
    fn drop(&mut self) {
        if !self.disarmed && !self.ids.is_empty() {
            let _ = close_target_ids(self.endpoint, &self.ids, Duration::from_secs(1));
        }
    }
}

/// Enforce a hard ceiling for Laya-owned pages. Stale auto-close pages are
/// reclaimed first; pinned pages are never reclaimed, but they still count
/// against the ceiling so a pin leak cannot grow without a policy error.
fn enforce_owned_target_limit(
    endpoint: &str,
    max_owned_pages: usize,
    owned_idle_ms: u64,
    timeout: Duration,
) -> Result<(usize, Vec<Value>)> {
    let targets = list_targets(endpoint, timeout)?;
    let valid: HashSet<String> = targets
        .iter()
        .filter_map(|t| t.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    {
        let mut registry = target_registry_cell()
            .lock()
            .map_err(|_| anyhow!("browser target registry lock poisoned"))?;
        registry.targets.retain(|t| valid.contains(&t.id));
    }

    let now = current_epoch_ms();
    let mut closed = Vec::new();
    {
        let registry = target_registry_cell()
            .lock()
            .map_err(|_| anyhow!("browser target registry lock poisoned"))?;
        closed.extend(
            registry
                .targets
                .iter()
                .filter(|t| {
                    !t.pinned && now.saturating_sub(t.last_used_ms) >= owned_idle_ms as u128
                })
                .map(|t| t.id.clone()),
        );
    }
    let mut errors = Vec::new();
    if !closed.is_empty() {
        let (count, close_errors) = close_target_ids(endpoint, &closed, timeout);
        errors.extend(close_errors);
        if count != closed.len() {
            let failed: HashSet<String> = errors
                .iter()
                .filter_map(|e| e.get("target_id").and_then(Value::as_str))
                .map(str::to_string)
                .collect();
            closed.retain(|id| !failed.contains(id));
        }
    }

    loop {
        let snapshot = {
            let registry = target_registry_cell()
                .lock()
                .map_err(|_| anyhow!("browser target registry lock poisoned"))?;
            registry
                .targets
                .iter()
                .map(|t| (t.id.clone(), t.pinned, t.last_used_ms))
                .collect::<Vec<_>>()
        };
        if snapshot.len() < max_owned_pages {
            break;
        }
        let victim = snapshot
            .iter()
            .filter(|(_, pinned, _)| !pinned)
            .min_by_key(|(_, _, last_used)| *last_used)
            .map(|(id, _, _)| id.clone());
        let Some(victim) = victim else {
            bail!(
                "Laya-owned page limit reached ({max_owned_pages}); close pinned tabs or raise max_owned_pages"
            );
        };
        let (count, close_errors) = close_target_ids(endpoint, &[victim], timeout);
        errors.extend(close_errors);
        if count == 0 {
            bail!(
                "Laya-owned page limit reached ({max_owned_pages}); reclaiming the oldest scratch tab failed"
            );
        }
    }
    Ok((closed.len(), errors))
}

/// Evaluate in the page main world and return `result.result.value`.
fn evaluate(
    endpoint: &str,
    target_id: &str,
    expression: &str,
    timeout: Duration,
    await_promise: bool,
) -> Result<Value> {
    let result = cdp_page_call(
        endpoint,
        target_id,
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": await_promise,
            "userGesture": true
        }),
        timeout,
    )?;
    let details = result.get("result").cloned().unwrap_or(Value::Null);
    if details.get("subtype").and_then(Value::as_str) == Some("error")
        || details
            .get("className")
            .and_then(Value::as_str)
            .is_some_and(|x| x.ends_with("Error"))
    {
        bail!(
            "page evaluate failed: {}",
            details.get("description").cloned().unwrap_or(details)
        );
    }
    if let Some(exception) = result.get("exceptionDetails") {
        bail!("page evaluate failed: {exception}");
    }
    Ok(details.get("value").cloned().unwrap_or(Value::Null))
}

fn js_escape(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "\"\"".to_string())
}

fn element_from_request(with: &Value) -> Result<String> {
    if let Some(i) = with
        .get("element")
        .or_else(|| with.get("index"))
        .and_then(Value::as_u64)
    {
        return Ok(format!("__layaBrowser.element({i})"));
    }
    if let Some(sel) = with
        .get("selector")
        .map(stringify)
        .filter(|s| !s.is_empty())
    {
        return Ok(format!(
            "document.querySelector({})",
            js_escape(&Value::String(sel))
        ));
    }
    bail!("browser operation needs 'element' (snapshot index) or 'selector'");
}

fn scroll_js(with: &Value) -> String {
    let amount = with.get("amount").and_then(Value::as_i64).unwrap_or(600);
    let expression = with.get("selector").map(stringify).unwrap_or_default();
    if expression.is_empty() {
        format!("window.scrollBy({{top:{amount},left:0,behavior:'instant'}}); true")
    } else {
        format!(
            "(()=>{{const e=document.querySelector({}); if(!e) return false; e.scrollBy({{top:{amount},left:0,behavior:'instant'}}); return true;}})()",
            js_escape(&Value::String(expression))
        )
    }
}

/// Extract Google organic results and mark obvious advertisement rows. The
/// classifier only trusts an explicit Google ad label/landing path; it never
/// treats ordinary pages as ads merely because their text mentions "广告".
const GOOGLE_RESULTS_JS: &str = r#"
(() => {
  const root = document.querySelector('#search') || document.querySelector('#rso') || document;
  const out = [];
  const seen = new Set();
  const adText = (s) => /^\s*(赞助商广告|广告|Sponsored|Ad)\s*$/i.test(String(s || '').trim());
  for (const h3 of root.querySelectorAll('a h3')) {
    const a = h3.closest('a');
    if (!a || !a.href) continue;
    let block = h3.closest('[data-hveid]') || h3.closest('div.g') || h3.closest('div');
    let title = (h3.innerText || h3.textContent || '').trim();
    if (!title || seen.has(title)) continue;
    const rowText = (block?.innerText || '').slice(0, 3000);
    const hasAdLabel = [...(block?.querySelectorAll('span,div,a') || [])]
      .some(e => adText(e.innerText || e.getAttribute('aria-label')));
    const href = a.href;
    const isAd = hasAdLabel || /google\.[^/]+\/aclk|pagead\/aclk|\/aclk\?/i.test(href);
    let url = href;
    try {
      const u = new URL(href);
      if (u.pathname === '/url' && u.searchParams.get('q')) url = u.searchParams.get('q');
    } catch {}
    const snippet = (block?.innerText || '').replace(/\s+/g, ' ').trim().slice(0, 500);
    seen.add(title);
    out.push({ title, url, snippet, is_ad: isAd, position: out.length + 1 });
  }
  return { count: out.length, candidates: out, page_title: document.title, url: location.href };
})()
"#;

/// Dom-to-Markdown extractor for a rendered article page.
///
/// Runs in the page after the SPA has settled. It walks the live DOM (so
/// lazy `getBoundingClientRect` sizes are meaningful), keeps headings,
/// paragraphs, lists, tables, code, links, emphasis, figures and KaTeX math
/// (from the `application/x-tex` annotation), and rewrites every content
/// image to `images/img-N.ext`, returning the matching source URLs so the
/// Rust side can download them next to the Markdown.
const ARTICLE_MD_JS: &str = r##"
(function(){
  const OPTS = (typeof __LAYA_OPTS__ !== 'undefined') ? __LAYA_OPTS__ : {};
  const IMG_DIR = OPTS.image_dir || 'images';
  const images = [];
  const seen = new Map();
  const SKIP_TAGS = new Set(['SCRIPT','STYLE','NOSCRIPT','TEMPLATE','SVG','CANVAS','IFRAME','VIDEO','AUDIO','SOURCE','TRACK','BUTTON','INPUT','SELECT','TEXTAREA','NAV','FOOTER','FORM','DETAILS','SUMMARY']);
  const DROP_SECTIONS = new Set(['AUDIO','SIMILAR PAPERS','DISCUSSION','COMMENTS','CITATION','AI DETECTION','RELATED PAPERS']);

  function absUrl(u){ try { return new URL(u, location.href).href; } catch(e){ return u || ''; } }
  function extOf(u){ try { const m = new URL(u).pathname.match(/\.([a-z0-9]{2,5})$/i); if (m) return '.'+m[1].toLowerCase(); } catch(e){} return '.png'; }
  function imageRef(src, alt){
    const abs = absUrl(src);
    if (seen.has(abs)) return seen.get(abs).name;
    const idx = images.length + 1;
    const name = 'img-' + idx + extOf(abs);
    seen.set(abs, {name, index: idx});
    images.push({index: idx, name, url: abs, alt: alt || ''});
    return name;
  }
  function attr(n, k){ const v = n.getAttribute && n.getAttribute(k); return v == null ? '' : v; }
  function katexTex(n){ const a = n.querySelector('annotation[encoding="application/x-tex"]'); return a ? a.textContent : null; }
  function headingOf(el){ const h = el.querySelector && el.querySelector('h1,h2,h3'); return ((h && h.innerText) || '').trim().toUpperCase(); }
  function hasBlockContent(el){ return !!el.querySelector('p,img,.katex,li,pre,table,code,blockquote,h1,h2,h3,h4'); }
  function isAnchorToSelf(href){
    try { const u = new URL(href, location.href); return u.pathname === location.pathname && u.hash === location.hash && !u.hash; } catch(e){ return false; }
  }

  function shouldSkip(el){
    if (el.nodeType !== 1) return false;
    const tag = el.tagName;
    if (el.classList && el.classList.contains('katex')) return false;
    if (SKIP_TAGS.has(tag)) return true;
    if (el.getAttribute && el.getAttribute('aria-hidden') === 'true') return true;
    const href = attr(el, 'href');
    if (tag === 'A') {
      if (href && (href.indexOf('/pdf/') >= 0 || href.indexOf('#discussion') >= 0)) return true;
      if (href && isAnchorToSelf(href)) return true;
    }
    if (tag === 'SECTION' || tag === 'DETAILS') {
      const id = (el.id || '').toUpperCase();
      const h = headingOf(el);
      if (DROP_SECTIONS.has(h) || DROP_SECTIONS.has(id)) return true;
    }
    if ((tag === 'DIV' || tag === 'SECTION' || tag === 'ASIDE') && !hasBlockContent(el)) return true;
    return false;
  }

  function renderChildren(node, ctx){
    let out = '';
    for (const c of node.childNodes) out += render(c, ctx);
    return out;
  }
  function renderInline(el){
    let out = '';
    for (const c of el.childNodes) out += render(c, {inline:true});
    return out.replace(/[ \t\r\n]+/g, ' ');
  }
  function render(node, ctx){
    ctx = ctx || {};
    if (node.nodeType === 3) {
      const t = node.nodeValue || '';
      return ctx.inline ? t.replace(/[ \t\r\n]+/g, ' ') : t;
    }
    if (node.nodeType !== 1) return '';
    if (shouldSkip(node)) return '';
    const tag = node.tagName;
    if (node.classList && node.classList.contains('katex')) {
      const tex = katexTex(node);
      if (tex == null) return '';
      // KaTeX wraps display math in a *separate* `<span class="katex-display">`
      // around the `.katex` node, so the marker is on the ancestor, not here.
      const display = node.classList.contains('katex-display')
        || !!(node.closest && node.closest('.katex-display'));
      return display ? '\n\n$$' + tex + '$$\n\n' : '$' + tex + '$';
    }
    if (tag === 'BR') return '\n';
    if (tag === 'IMG') {
      const src = attr(node, 'src');
      if (!src) return '';
      if (node.closest && node.closest('a[rel=author],[class*=avatar],[class*=Avatar],[class*=photo]')) return '';
      let w = 0;
      try { w = node.getBoundingClientRect().width; } catch(e) { w = parseFloat(attr(node,'width')) || 0; }
      if (!w) w = parseFloat(attr(node, 'width')) || 0;
      if (w && w < 80) return '';
      const alt = attr(node, 'alt');
      return '![' + alt.replace(/\s+/g,' ') + '](' + IMG_DIR + '/' + imageRef(src, alt) + ')';
    }
    if (tag === 'FIGURE') return '\n\n' + renderChildren(node, ctx).trim() + '\n\n';
    if (tag === 'FIGCAPTION') return '\n\n*' + renderInline(node).trim() + '*\n\n';
    if (/^H[1-6]$/.test(tag)) return '\n\n' + '#'.repeat(parseInt(tag.slice(1),10)) + ' ' + renderInline(node).trim() + '\n\n';
    if (tag === 'P') return '\n\n' + renderInline(node).trim() + '\n\n';
    if (tag === 'STRONG' || tag === 'B') return '**' + renderInline(node).trim() + '**';
    if (tag === 'EM' || tag === 'I') return '*' + renderInline(node).trim() + '*';
    if (tag === 'DEL' || tag === 'S') return '~~' + renderInline(node).trim() + '~~';
    if (tag === 'CODE') {
      if (node.closest && node.closest('pre')) return node.textContent || '';
      return '`' + (node.textContent || '').trim() + '`';
    }
    if (tag === 'PRE') {
      const code = node.querySelector('code');
      let lang = '';
      if (code) { const m = (code.className || '').match(/language-([\w+-]+)/); if (m) lang = m[1]; }
      const body = (code ? code.textContent : node.textContent) || '';
      return '\n\n```' + lang + '\n' + body.replace(/\s+$/, '') + '\n```\n\n';
    }
    if (tag === 'A') {
      const href = absUrl(attr(node, 'href'));
      const label = renderInline(node).trim();
      if (!label) return '';
      if (!href || href === label) return label;
      return '[' + label + '](' + href + ')';
    }
    if (tag === 'SUP') return renderInline(node).trim();
    if (tag === 'HR') return '\n\n---\n\n';
    if (tag === 'BLOCKQUOTE') {
      const body = renderChildren(node, {inline:false}).trim();
      return '\n\n' + body.split('\n').map(l => '> ' + l).join('\n') + '\n\n';
    }
    if (tag === 'UL' || tag === 'OL') {
      const ordered = tag === 'OL';
      let out = '\n\n';
      let i = 1;
      for (const li of node.children) {
        if (li.tagName !== 'LI') continue;
        let body = renderChildren(li, {inline:false}).trim().replace(/\n{2,}/g, '\n');
        out += (ordered ? (i + '. ') : '- ') + body + '\n';
        i++;
      }
      return out + '\n';
    }
    if (tag === 'TABLE') {
      const rows = [...node.querySelectorAll('tr')];
      if (!rows.length) return '';
      let out = '\n\n';
      rows.forEach((tr, ri) => {
        const cells = [...tr.children].map(td => renderInline(td).trim().replace(/\|/g, '\\|'));
        out += '| ' + cells.join(' | ') + ' |\n';
        if (ri === 0) out += '| ' + cells.map(() => '---').join(' | ') + ' |\n';
      });
      return out + '\n';
    }
    if (['DIV','SECTION','ARTICLE','HEADER','MAIN','ASIDE'].includes(tag)) {
      return '\n\n' + renderChildren(node, {inline:false}).trim() + '\n\n';
    }
    return renderChildren(node, ctx);
  }

  // ---- pick content parts ----
  let parts = [];
  if (OPTS.selector) {
    const r = document.querySelector(OPTS.selector);
    if (r) parts.push(r);
  }
  if (!parts.length) {
    const article = document.querySelector('article');
    const header = (article && article.querySelector('header')) || document.querySelector('header');
    if (header) parts.push(header);
    if (article) {
      for (const s of article.querySelectorAll(':scope > section')) {
        const h = s.querySelector('h1,h2,h3');
        const t = ((h && h.innerText) || '').trim().toUpperCase();
        if (/ABSTRACT/.test(t)) parts.push(s);
      }
    }
    const overview = document.querySelector('#overview') || document.querySelector('.markdown-content');
    if (overview) parts.push(overview);
    if (!parts.length) parts = [document.querySelector('article') || document.querySelector('main') || document.body];
  }

  let md = '';
  for (const p of parts) md += '\n\n' + render(p, {inline:false});
  md = md.replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n');
  // Inline links/images that sit side by side in a flex row are separated by
  // CSS `gap`, not by whitespace, so their labels ran together
  // (`[A](a)[B](b)`) once the avatars between them were dropped. Insert a space
  // only when one link/image is immediately followed by another.
  md = md.replace(/(\]\([^()\s]+\))(?=!?\[[^\]]*\]\()/g, '$1 ');
  md = md.trim() + '\n';

  const title = ((document.querySelector('h1') || {}).innerText || document.title || '').trim();
  return { title: title, markdown: md, images: images, chars: md.length, imageCount: images.length };
})()
"##;

/// Extract readable page text and objective structural signals used by the
/// Laya relevance/effectiveness scorer.
const READABLE_PAGE_JS: &str = r#"
(() => {
  const maxChars = 5000;
  const body = document.body;
  const bodyText = body?.innerText || '';
  const selectors = ['article', 'main', '[role=main]', '#content', '.content', '#main'];
  let root = body;
  let rootScore = bodyText.length;
  for (const selector of selectors) {
    for (const el of document.querySelectorAll(selector)) {
      const len = (el.innerText || '').length;
      if (len > 500 && len < rootScore) { root = el; rootScore = len; }
    }
  }
  const text = (root.innerText || '').replace(/[\t\r]+/g, ' ').replace(/\n{3,}/g, '\n\n').trim().slice(0, maxChars);
  const visibleLinks = [...root.querySelectorAll('a[href]')].filter(a => (a.innerText || '').trim().length > 0).length;
  const published = (bodyText.match(/20[12][0-9][-/年][0-9]{1,2}[-/月][0-9]{1,2}|20[12][0-9]/) || [''])[0];
  return {
    title: document.title,
    final_url: location.href,
    host: location.host,
    text,
    excerpt: text.slice(0, 280),
    text_length: text.length,
    headings: root.querySelectorAll('h1,h2,h3,h4,h5,h6').length,
    paragraphs: root.querySelectorAll('p').length,
    code_blocks: root.querySelectorAll('pre code,pre,.highlight').length,
    links: visibleLinks,
    published_hint: published
  };
})()
"#;

/// Simple page snapshot when the plugin is unavailable. It still exposes the
/// same fields expected by a decision model, without numbered badges.
const FALLBACK_SNAPSHOT_JS: &str = r#"
(() => {
  const visible = (e) => {
    const r = e.getBoundingClientRect();
    const s = getComputedStyle(e);
    return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none';
  };
  const els = [...document.querySelectorAll('a,button,input,select,textarea,[role],summary')]
    .filter(visible).slice(0, 300).map((e, i) => ({
      index: i, role: e.tagName.toLowerCase(), name: (e.getAttribute('aria-label') || e.innerText || e.value || e.name || e.id || '').trim().slice(0, 300),
      value: e.value || null, href: e.href || null, section: e.closest('h1,h2,h3,[role=heading]')?.innerText || null,
      selector: e.id ? ('#'+CSS.escape(e.id)) : null
    }));
  return { url: location.href, title: document.title, text: document.body.innerText.slice(0, 6000), elements: els };
})()
"#;

/// Run several CDP commands on the same page websocket. Input sequences must be
/// atomic: closing a socket between mousePressed and mouseReleased can lose the
/// synthesized browser click.
fn cdp_page_sequence(
    endpoint: &str,
    target_id: &str,
    commands: &[(&str, Value)],
    timeout: Duration,
) -> Result<Vec<Value>> {
    let mut ws = tungstenite::connect(ws_endpoint(
        endpoint,
        &format!("/devtools/page/{target_id}"),
    )?)
    .map_err(|e| anyhow!("connect Chrome page websocket failed: {e}"))?
    .0;
    set_ws_timeout(&mut ws, Duration::from_millis(250));
    let deadline = Instant::now() + timeout;
    let mut results = Vec::with_capacity(commands.len());
    for (index, (method, params)) in commands.iter().enumerate() {
        if *method == "SLEEP" {
            let ms = params.as_u64().unwrap_or(0);
            std::thread::sleep(Duration::from_millis(ms));
            continue;
        }
        let id = index as u64 + 1;
        let frame = serde_json::to_string(&json!({"id": id, "method": method, "params": params}))?;
        if std::env::var("LAYA_CDP_DEBUG").is_ok() {
            println!("CDP SEND: {frame}");
        }
        ws.send(tungstenite::Message::Text(frame))?;
        loop {
            if Instant::now() >= deadline {
                bail!(
                    "CDP {method} sequence timed out after {}ms",
                    timeout.as_millis()
                );
            }
            let message = match ws.read() {
                Ok(message) => message,
                Err(tungstenite::Error::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(e) => return Err(anyhow!("Chrome websocket read failed: {e}")),
            };
            let value = match message {
                tungstenite::Message::Text(s) => serde_json::from_str::<Value>(&s)?,
                tungstenite::Message::Binary(b) => serde_json::from_slice::<Value>(&b)?,
                tungstenite::Message::Ping(p) => {
                    ws.send(tungstenite::Message::Pong(p))?;
                    continue;
                }
                tungstenite::Message::Close(c) => bail!("Chrome closed page websocket: {c:?}"),
                _ => continue,
            };
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(err) = value.get("error") {
                bail!("CDP {method} failed: {err}");
            }
            results.push(value.get("result").cloned().unwrap_or(Value::Null));
            break;
        }
    }
    Ok(results)
}

fn mouse_event(kind: &str, x: f64, y: f64) -> Value {
    let press_or_release = kind == "mousePressed" || kind == "mouseReleased";
    json!({
        "type": kind,
        "x": x,
        "y": y,
        "button": if press_or_release { "left" } else { "none" },
        "buttons": if kind == "mousePressed" { 1 } else { 0 },
        "clickCount": if press_or_release { 1 } else { 0 },
        "pointerType": "mouse"
    })
}

fn type_text(endpoint: &str, target: &str, text: &str, timeout: Duration) -> Result<()> {
    // Input.insertText keeps the tab in the background. The former
    // Page.bringToFront + key-event flow stole the user's active window.
    cdp_page_sequence(
        endpoint,
        target,
        &[("Input.insertText", json!({"text": text}))],
        timeout,
    )?;
    Ok(())
}

fn decision_object(decision: &Value) -> Result<Value> {
    if let Some(obj) = decision.as_object() {
        return Ok(Value::Object(obj.clone()));
    }
    let raw = decision.as_str().unwrap_or("").trim();
    if raw.is_empty() {
        bail!("browser agent_step decision is empty");
    }
    let trimmed = raw
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim()
        .trim_end_matches("```")
        .trim();
    let source = if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if start < end {
            &trimmed[start..=end]
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    serde_json::from_str(source)
        .map_err(|e| anyhow!("browser agent_step decision is not JSON: {e}; got {raw:?}"))
}

fn probability(decision: &Value, names: &[&str]) -> Option<f64> {
    names
        .iter()
        .find_map(|name| decision.get(name).and_then(Value::as_f64))
}

/// Execute one typed decision emitted by an OpenAI-compatible planner. This is
/// the Rust execution half of the Jev-style operation/element response.
fn execute_agent_step(
    c: &BrowserCap,
    with: &Value,
    state: &Value,
    policy: &Policy,
    decision: &Value,
) -> Result<Value> {
    let obj = decision_object(decision)?;
    let operation = obj
        .get("operation")
        .or_else(|| obj.get("action"))
        .map(stringify)
        .unwrap_or_default()
        .to_ascii_uppercase();
    let goal = probability(&obj, &["goal_probability", "goal_achieved_probability"]).unwrap_or(1.0);
    let stuck = probability(&obj, &["stuck_probability"]).unwrap_or(0.0);
    let planned =
        json!({"operation": operation, "goal_probability": goal, "stuck_probability": stuck});

    if operation == "DONE" && goal < 0.5 {
        return Ok(json!({
            "capability": "chrome_cdp", "op": "agent_step", "planned": planned,
            "withheld": true, "reason": "DONE was not supported by goal_probability"
        }));
    }
    if operation == "BLOCKED" && stuck < 0.5 {
        return Ok(json!({
            "capability": "chrome_cdp", "op": "agent_step", "planned": planned,
            "withheld": true, "reason": "BLOCKED was not supported by stuck_probability"
        }));
    }

    let mut nested = with.as_object().cloned().unwrap_or_default();
    nested.remove("op");
    nested.remove("decision");
    nested.remove("target_id");
    if let Some(id) = with.get("target_id") {
        nested.insert("target_id".to_string(), id.clone());
    }
    let copy = |names: &[&str]| names.iter().find_map(|n| obj.get(*n)).cloned();
    match operation.as_str() {
        "CLICK" => {
            nested.insert("op".to_string(), json!("click"));
        }
        "TYPE_TEXT" => {
            nested.insert("op".to_string(), json!("type"));
            nested.insert(
                "text".to_string(),
                copy(&["text", "value"]).unwrap_or(Value::Null),
            );
        }
        "SELECT" => {
            nested.insert("op".to_string(), json!("select"));
            nested.insert(
                "value".to_string(),
                copy(&["value", "text"]).unwrap_or(Value::Null),
            );
        }
        "SCROLL_DOWN" | "SCROLL_UP" => {
            nested.insert("op".to_string(), json!("scroll"));
            let n = if operation == "SCROLL_UP" { -1 } else { 1 };
            nested.insert(
                "amount".to_string(),
                json!(n * obj.get("amount").and_then(Value::as_i64).unwrap_or(600)),
            );
        }
        "PRESS_ENTER" | "KEY" => {
            nested.insert("op".to_string(), json!("key"));
            nested.insert(
                "key".to_string(),
                json!(if operation == "PRESS_ENTER" {
                    "Enter".to_string()
                } else {
                    obj.get("key").map(stringify).unwrap_or_default()
                }),
            );
        }
        "WAIT" => {
            let ms = obj
                .get("ms")
                .or_else(|| obj.get("wait_ms"))
                .and_then(Value::as_u64)
                .unwrap_or(500)
                .min(10_000);
            std::thread::sleep(Duration::from_millis(ms));
            return Ok(json!({
                "capability": "chrome_cdp", "op": "agent_step", "planned": planned,
                "waited_ms": ms
            }));
        }
        "DONE" | "BLOCKED" => {
            return Ok(json!({
                "capability": "chrome_cdp", "op": "agent_step", "planned": planned,
                "terminal": operation
            }));
        }
        other => bail!("browser agent_step returned unsupported operation {other:?}"),
    }
    let element = copy(&["element", "index"]).filter(|v| !v.is_null());
    let selector = copy(&["selector"]).filter(|v| !v.is_null());
    let key = if element.is_some() {
        "element"
    } else {
        "selector"
    };
    if let Some(v) = element.or(selector) {
        let v = if key == "element" {
            v.as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .map(|n| json!(n))
                .unwrap_or(v)
        } else {
            v
        };
        nested.insert(key.to_string(), v);
    }
    if nested.get("text").map(Value::is_null).unwrap_or(false) {
        nested.remove("text");
    }
    if nested.get("value").map(Value::is_null).unwrap_or(false) {
        nested.remove("value");
    }
    let mut result = call_browser(c, &Value::Object(nested), state, policy)?;
    if let Some(map) = result.as_object_mut() {
        map.insert("planned".to_string(), planned);
    }
    Ok(result)
}

/// Normalize text for deterministic heuristic matching.
fn research_norm(v: &str) -> String {
    v.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// How many page tabs research / open_many may drive at once.
///
/// Defaults to 5 and is clamped to `1..=10`; `concurrency` is accepted as an
/// alias for `tab_concurrency`.
fn tab_concurrency_of(with: &Value) -> usize {
    with.get("tab_concurrency")
        .or_else(|| with.get("concurrency"))
        .and_then(Value::as_u64)
        .unwrap_or(5)
        .clamp(1, 10) as usize
}

/// Hosts that serve primarily video/audio. Opening them burns a background tab
/// and yields almost no readable research text, so Laya classifies and skips
/// them before spending a target.
const VIDEO_HOSTS: &[&str] = &[
    "youtube.com",
    "youtu.be",
    "music.youtube.com",
    "bilibili.com",
    "b23.tv",
    "vimeo.com",
    "dailymotion.com",
    "twitch.tv",
    "tiktok.com",
    "douyin.com",
    "youku.com",
    "iqiyi.com",
    "v.qq.com",
    "netflix.com",
    "hulu.com",
    "nicovideo.jp",
    "rumble.com",
    "odysee.com",
    "kick.com",
    "ixigua.com",
    "kuaishou.com",
    "xiaohongshu.com",
];

/// Direct media-file extensions that never contain a readable article.
const MEDIA_EXTENSIONS: &[&str] = &[
    ".mp4", ".m4v", ".mov", ".webm", ".mkv", ".avi", ".flv", ".wmv", ".mpeg", ".mpg", ".mp3",
    ".m4a", ".wav", ".ogg", ".flac", ".aac", ".opus",
];

/// Coarse, deterministic Laya URL classification performed *before* opening a
/// page. Returns `"video"` for known video hosts, `"media"` for direct
/// media-file URLs, otherwise `"page"`.
fn url_content_kind(url: &str) -> &'static str {
    let lower = url.to_lowercase();
    let path = lower.split(['?', '#']).next().unwrap_or(&lower);
    if MEDIA_EXTENSIONS.iter().any(|ext| path.ends_with(ext)) {
        return "media";
    }
    let host = host_of(url).to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    if VIDEO_HOSTS
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")))
    {
        return "video";
    }
    "page"
}

/// Content kinds a workflow refuses to open. Video and direct media are skipped
/// by default; `skip_video: false` disables that, and `skip_kinds` overrides the
/// whole set. This is the Laya-side "classify before opening" policy.
fn skip_kinds_of(with: &Value) -> Vec<String> {
    if let Some(list) = with.get("skip_kinds").and_then(Value::as_array) {
        return list
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
    }
    let skip_video = with
        .get("skip_video")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if skip_video {
        vec!["video".to_string(), "media".to_string()]
    } else {
        Vec::new()
    }
}

/// Percent-decode a URL query value (e.g. Google's `q=`), byte-safe.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read one query parameter from a URL, percent-decoded.
fn query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(percent_decode(v));
            }
        }
    }
    None
}

/// Ask an HTTP endpoint where it redirects to, reading only the `Location`
/// header (the response body is discarded). Used to peel Google's opaque
/// result-wrapper so Laya can classify the real destination.
fn redirect_location(url: &str) -> Option<String> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(5))
        .build();
    let response = match agent.get(url).call() {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(_) => return None,
    };
    response.header("location").map(str::to_string)
}

/// Google wraps organic result links in a redirect endpoint (`/url?q=<real>` or
/// `/goto?url=<opaque>`, sometimes the legacy `/interstitial`). The wrapper host
/// is `google.*`, so a host-based classifier would otherwise see every row as a
/// normal page. Decode the `q=` form directly and, for the opaque `url=` form,
/// ask Google where it points (headers only) so Laya can classify the real
/// destination **before** opening a tab. Non-Google URLs are returned unchanged.
fn resolve_destination_url(url: &str) -> String {
    let host = host_of(url).to_lowercase();
    let bare = host.strip_prefix("www.").unwrap_or(&host).to_string();
    if !bare.starts_with("google.") {
        return url.to_string();
    }
    if let Some(q) = query_param(url, "q") {
        if q.starts_with("http://") || q.starts_with("https://") {
            return q;
        }
    }
    if url.contains("/url?") || url.contains("/goto?") || url.contains("/interstitial?") {
        if let Some(location) = redirect_location(url) {
            if location.starts_with("http://") || location.starts_with("https://") {
                return location;
            }
        }
    }
    url.to_string()
}

/// A desktop Chrome user-agent. Some asset CDNs reject requests without one.
const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// Canonicalize as much of `path` as exists, keeping the non-existent suffix.
/// Lets a policy check happen before the output directory is created.
fn canonicalize_lenient(path: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return resolved;
    }
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        let Some(parent) = current.parent() else {
            break;
        };
        if parent.as_os_str().is_empty() {
            break;
        }
        if let Some(name) = current.file_name() {
            suffix.push(name.to_os_string());
        }
        if let Ok(resolved) = std::fs::canonicalize(parent) {
            let mut out = resolved;
            for part in suffix.iter().rev() {
                out.push(part);
            }
            return out;
        }
        current = parent.to_path_buf();
    }
    path.to_path_buf()
}

/// Expand a leading `~/` to `$HOME`, then fail-closed against `policy.allow_paths`.
fn allowed_out_dir(policy: &Policy, raw: &str) -> Result<PathBuf> {
    if policy.allow_paths.is_empty() {
        bail!("save_article out_dir {raw:?} denied: set policy.allow_paths to the allowed root(s)");
    }
    let expanded = if let Some(rest) = raw.strip_prefix("~/") {
        match std::env::var("HOME") {
            Ok(home) => format!("{home}/{rest}"),
            Err(_) => raw.to_string(),
        }
    } else {
        raw.to_string()
    };
    let candidate = canonicalize_lenient(Path::new(&expanded));
    let allowed = policy
        .allow_paths
        .iter()
        .any(|root| candidate.starts_with(canonicalize_lenient(Path::new(root))));
    if !allowed {
        bail!(
            "save_article out_dir {:?} is outside policy.allow_paths {:?}",
            candidate.display().to_string(),
            policy.allow_paths
        );
    }
    Ok(candidate)
}

/// Turn a title or URL tail into a short, filesystem-safe slug.
fn sanitize_slug(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.chars() {
        let keep = if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' {
            Some(ch.to_ascii_lowercase())
        } else if ch.is_alphanumeric() {
            Some(ch)
        } else if ch == '-' || ch.is_whitespace() {
            None
        } else {
            None
        };
        match keep {
            Some(c) => {
                out.push(c);
                last_dash = false;
            }
            None => {
                if !last_dash && !out.is_empty() {
                    out.push('-');
                    last_dash = true;
                }
            }
        }
    }
    let trimmed = out.trim_matches(|c| c == '-' || c == '.').to_string();
    let capped: String = trimmed.chars().take(80).collect();
    capped
}

/// Prefer the URL tail (an alphaXiv id like `2609.recurrent-looped-transformer`),
/// fall back to the page title.
fn derive_slug(url: &str, title: &str) -> String {
    let base = url
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    let tail = base.rsplit('/').next().unwrap_or("");
    let host = host_of(url).to_lowercase();
    let looks_like_id = tail.len() >= 6
        && !tail.chars().any(|c| c.is_whitespace())
        && !tail.eq_ignore_ascii_case(&host)
        && (tail.contains('.') || tail.contains('-') || tail.contains('_'));
    let slug = sanitize_slug(if looks_like_id { tail } else { title });
    if slug.is_empty() {
        "article".to_string()
    } else {
        slug
    }
}

/// Download an image (bytes only) with a bounded size.
fn download_binary(url: &str, timeout: Duration, policy: &Policy) -> Result<Vec<u8>> {
    check_host(url, policy)?;
    let response = ureq::AgentBuilder::new()
        .timeout(timeout)
        .build()
        .get(url)
        .set("user-agent", BROWSER_UA)
        .call()
        .map_err(|e| anyhow!("GET {url} failed: {e}"))?;
    let mut buf = Vec::new();
    response
        .into_reader()
        .take(32 * 1024 * 1024)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

/// Download every extracted image into `dir`, reporting per-image status.
fn download_article_images(
    images: &Value,
    dir: &Path,
    timeout: Duration,
    policy: &Policy,
) -> Vec<Value> {
    let mut out = Vec::new();
    let list = images.as_array().cloned().unwrap_or_default();
    for (i, img) in list.iter().enumerate() {
        let url = img
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if url.is_empty() {
            continue;
        }
        let name = img
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("img-{}.bin", i + 1));
        match download_binary(&url, timeout, policy) {
            Ok(bytes) => {
                let path = dir.join(&name);
                match std::fs::write(&path, &bytes) {
                    Ok(()) => out.push(json!({
                        "name": name, "url": url, "path": path.display().to_string(),
                        "bytes": bytes.len(), "ok": true
                    })),
                    Err(e) => out.push(json!({
                        "name": name, "url": url, "ok": false, "error": e.to_string()
                    })),
                }
            }
            Err(e) => out.push(json!({
                "name": name, "url": url, "ok": false, "error": e.to_string()
            })),
        }
    }
    out
}

/// Poll until the SPA stops growing and its images settle. Never hard-fails:
/// on timeout it returns what it observed so extraction can still proceed.
fn wait_for_render(
    endpoint: &str,
    target_id: &str,
    stable: Duration,
    max_wait: Duration,
    timeout: Duration,
) -> Value {
    let step = timeout.min(Duration::from_secs(3));
    // A full scroll pass reveals below-the-fold lazy figures before we snapshot.
    let _ = evaluate(
        endpoint,
        target_id,
        "(()=>{window.scrollTo(0, document.body.scrollHeight); return true;})()",
        step,
        false,
    );
    std::thread::sleep(Duration::from_millis(350));
    let _ = evaluate(
        endpoint,
        target_id,
        "(()=>{window.scrollTo(0, 0); return true;})()",
        step,
        false,
    );
    let probe = "({len:(document.body?document.body.innerText.length:0),readyState:document.readyState,pendingImages:[...document.images].filter(i=>!i.complete).length})";
    let deadline = Instant::now() + max_wait;
    let mut last_len: i64 = -1;
    let mut stable_since = Instant::now();
    let mut last = json!({});
    loop {
        last = evaluate(endpoint, target_id, probe, step, false)
            .unwrap_or_else(|e| json!({"error": e.to_string()}));
        let len = last.get("len").and_then(Value::as_i64).unwrap_or(-1);
        if len != last_len {
            last_len = len;
            stable_since = Instant::now();
        }
        let ready_state = last.get("readyState").and_then(Value::as_str).unwrap_or("");
        let pending = last
            .get("pendingImages")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        // Off-screen lazy images may never load (related-paper thumbnails), and
        // content images are downloaded from their URLs, not read from the DOM,
        // so "settled" means: document complete and rendered text stopped growing.
        if ready_state == "complete" && len > 0 && stable_since.elapsed() >= stable {
            return json!({"stabilized": true, "text_len": len, "pending_images": pending});
        }
        if Instant::now() >= deadline {
            return json!({
                "stabilized": false, "text_len": len,
                "pending_images": pending, "ready_state": ready_state,
                "reason": "render did not settle within max_wait_ms"
            });
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Open (or reuse) a tab, wait for it to render, extract Markdown + images, and
/// optionally write `<slug>.md`, `<slug>_meta.json` and `images/*` to disk.
fn run_save_article(
    c: &BrowserCap,
    endpoint: &str,
    with: &Value,
    timeout: Duration,
    policy: &Policy,
) -> Result<Value> {
    let keep_open = with
        .get("keep_open")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let image_dir = with
        .get("image_dir")
        .map(stringify)
        .filter(|s| !s.is_empty() && s != "null")
        .unwrap_or_else(|| "images".to_string());
    let stable_ms = with
        .get("stable_ms")
        .and_then(Value::as_u64)
        .unwrap_or(1500);
    let max_wait_ms = with
        .get("max_wait_ms")
        .and_then(Value::as_u64)
        .unwrap_or(45_000)
        .min(policy.max_timeout_ms);
    let selector = with
        .get("selector")
        .map(stringify)
        .filter(|s| !s.is_empty() && s != "null");
    let url = with
        .get("url")
        .map(stringify)
        .filter(|s| !s.is_empty() && s != "null");

    let mut cleanup = TargetCleanupGuard::new(endpoint);
    let id = if let Some(url) = url.clone() {
        check_host(&url, policy)?;
        enforce_owned_target_limit(endpoint, c.max_owned_pages, c.owned_idle_ms, timeout)?;
        let created = cdp_browser_call(
            endpoint,
            "Target.createTarget",
            json!({"url": "about:blank", "background": true}),
            timeout,
        )?;
        let id = created
            .get("targetId")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Chrome Target.createTarget returned no targetId"))?
            .to_string();
        track_owned_target(endpoint, &id, keep_open);
        if !keep_open {
            cleanup.track(id.clone());
        }
        cdp_page_call(endpoint, &id, "Page.navigate", json!({"url": url}), timeout)?;
        id
    } else {
        if with
            .get("target_id")
            .or_else(|| with.get("targetId"))
            .is_none()
        {
            bail!("browser save_article needs 'url' (or an explicit 'target_id')");
        }
        let target = resolve_target(endpoint, with, timeout, policy)?;
        target_id(&target)?
    };

    let ready = wait_page_ready(endpoint, &id, timeout, None).unwrap_or(Value::Null);
    let render = wait_for_render(
        endpoint,
        &id,
        Duration::from_millis(stable_ms),
        Duration::from_millis(max_wait_ms),
        timeout,
    );

    let opts = json!({
        "image_dir": image_dir,
        "selector": selector.clone().unwrap_or_default(),
    });
    let expression = format!(
        "var __LAYA_OPTS__ = {};\n{}",
        serde_json::to_string(&opts)?,
        ARTICLE_MD_JS
    );
    let value = evaluate(endpoint, &id, &expression, timeout, false)?;
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let markdown = value
        .get("markdown")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let images = value.get("images").cloned().unwrap_or_else(|| json!([]));
    let page_url = ready
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| url.clone())
        .unwrap_or_default();

    let mut written = Value::Null;
    if let Some(out_dir) = with
        .get("out_dir")
        .map(stringify)
        .filter(|s| !s.is_empty() && s != "null")
    {
        let dir = allowed_out_dir(policy, &out_dir)?;
        std::fs::create_dir_all(&dir)?;
        let slug = with
            .get("slug")
            .map(stringify)
            .filter(|s| !s.is_empty() && s != "null")
            .unwrap_or_else(|| derive_slug(&page_url, &title));
        let images_dir = dir.join(&image_dir);
        std::fs::create_dir_all(&images_dir)?;
        let downloads = download_article_images(&images, &images_dir, timeout, policy);
        let markdown_path = dir.join(format!("{slug}.md"));
        std::fs::write(&markdown_path, &markdown)?;
        let meta = json!({
            "capability": "chrome_cdp",
            "op": "save_article",
            "title": title,
            "url": page_url,
            "slug": slug,
            "selector": selector,
            "rendered": render,
            "markdown_file": markdown_path.display().to_string(),
            "markdown_chars": markdown.chars().count(),
            "images_dir": images_dir.display().to_string(),
            "images": downloads,
        });
        let meta_path = dir.join(format!("{slug}_meta.json"));
        std::fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)?;
        written = json!({
            "out_dir": dir.display().to_string(),
            "slug": slug,
            "markdown_file": markdown_path.display().to_string(),
            "meta_file": meta_path.display().to_string(),
            "images_dir": images_dir.display().to_string(),
            "images": downloads,
        });
    }

    let (closed, cleanup_errors) = cleanup.finish(!keep_open, timeout);
    Ok(json!({
        "capability": "chrome_cdp",
        "op": "save_article",
        "url": page_url,
        "title": title,
        "selector": selector,
        "rendered": render,
        "markdown_chars": markdown.chars().count(),
        "markdown": truncate(markdown, policy.max_output),
        "images": images,
        "written": written,
        "keep_open": keep_open,
        "closed_count": closed,
        "cleanup_errors": cleanup_errors,
    }))
}

// ── alphaXiv paper discovery (natural-language op) ──────────────────

/// alphaXiv's homepage doubles as the explore/trending feed; every trending card
/// links to `/abs/<id>`.
const ALPHAXIV_HOME: &str = "https://www.alphaxiv.org/";

/// Page-side collector for alphaXiv `/abs/<id>` cards. The feed and the search
/// page are both client-rendered, so this promise polls until `limit` unique
/// links appear or the deadline elapses, then resolves with the rows in DOM
/// order.
const ALPHAXIV_LINKS_JS: &str = r##"(() => new Promise((resolve) => {
  const limit = (__LAYA_OPTS__.limit | 0) || 10;
  const deadline = Date.now() + ((__LAYA_OPTS__.timeout_ms | 0) || 12000);
  const collect = () => {
    const seen = new Set();
    const out = [];
    for (const a of document.querySelectorAll('a[href*="/abs/"]')) {
      const raw = a.href || '';
      const u = raw.split('#')[0].split('?')[0];
      if (!/^https?:\/\/(www\.)?alphaxiv\.org\/abs\//i.test(u)) continue;
      if (seen.has(u)) continue;
      seen.add(u);
      const card = a.closest('article, li, div') || a;
      const text = ((card.innerText || a.innerText || '') + '').replace(/\s+/g, ' ').trim();
      out.push({ url: u, title: text.slice(0, 200) });
      if (out.length >= limit) break;
    }
    return out;
  };
  const tick = () => {
    const links = collect();
    if (links.length >= limit || Date.now() > deadline) {
      resolve({ count: links.length, links: links });
    } else {
      setTimeout(tick, 400);
    }
  };
  tick();
}))()"##;

/// Page-side collector for alphaXiv's public feed API — the same endpoint the
/// Explore/Sort UI calls (`GET /papers/v3/feed`). The site labels `sort=Hot`
/// "Trending"; `interval` is one of `3 Days`/`7 Days`/`30 Days`/`90 Days`/
/// `All time`. This is how trending can page through far more papers than the
/// rendered homepage row shows (the API accepts `pageSize` up to 100).
const ALPHAXIV_FEED_JS: &str = r##"(async () => {
  const OPTS = (typeof __LAYA_OPTS__ !== 'undefined') ? __LAYA_OPTS__ : {};
  const want = Math.max(1, OPTS.count | 0 || 10);
  const sort = OPTS.sort || 'Hot';
  const interval = OPTS.interval || '7 Days';
  const pageSize = Math.min(100, Math.max(1, (OPTS.page_size | 0) || Math.min(100, want)));
  const maxPages = (OPTS.max_pages | 0) || (Math.ceil(want / pageSize) + 3);
  const out = [];
  const seen = new Set();
  let pages = 0;
  for (let page = 1; page <= maxPages && out.length < want; page++) {
    const url = 'https://api.alphaxiv.org/papers/v3/feed'
      + '?pageNum=' + page
      + '&pageSize=' + pageSize
      + '&sort=' + encodeURIComponent(sort)
      + '&interval=' + encodeURIComponent(interval)
      + '&linkBlogs=true&topics=%5B%5D';
    let payload;
    try {
      const r = await fetch(url, { headers: { accept: 'application/json' } });
      if (!r.ok) break;
      payload = await r.json();
    } catch (e) { break; }
    const papers = (payload && payload.papers) || [];
    if (!papers.length) break;
    pages++;
    for (const paper of papers) {
      const id = paper.universal_paper_id || paper.canonical_id;
      if (!id) continue;
      const abs = 'https://www.alphaxiv.org/abs/' + id;
      if (seen.has(abs)) continue;
      seen.add(abs);
      out.push({ url: abs, title: (paper.title || '').slice(0, 200) });
      if (out.length >= want) break;
    }
  }
  return { count: out.length, links: out, pages: pages, sort: sort, interval: interval };
})()"##;

/// True when a natural-language query points at a concrete paper page.
fn looks_like_paper_url(s: &str) -> bool {
    let l = s.trim().to_ascii_lowercase();
    l.starts_with("http://")
        || l.starts_with("https://")
        || l.contains("alphaxiv.org/abs/")
        || l.contains("arxiv.org/abs/")
        || l.contains("arxiv.org/pdf/")
}

/// True for a BCP-47-ish locale path segment (`zh`, `en`, `zh-cn`, `pt-br`).
fn is_locale_segment(segment: &str) -> bool {
    let s = segment.to_ascii_lowercase();
    s.len() == 2 || (s.len() == 5 && s.as_bytes().get(2) == Some(&b'-'))
}

/// Rewrite an alphaXiv `/abs/<id>` URL to a localized one (`/zh/abs/<id>`),
/// which is how alphaXiv serves translated paper pages. Root listing pages
/// (search, explore) have no localized variant, so only paper paths change.
/// Non-alphaXiv URLs, a missing `<id>`, and a `lang` of `en`/`auto`/`off` are
/// returned unchanged.
fn localize_alphaxiv_url(url: &str, lang: &str) -> String {
    let lang = lang.trim().trim_matches('/').to_ascii_lowercase();
    if lang.is_empty() || matches!(lang.as_str(), "en" | "auto" | "none" | "off") {
        return url.to_string();
    }
    let host = host_of(url).to_lowercase();
    if host.strip_prefix("www.").unwrap_or(&host) != "alphaxiv.org" {
        return url.to_string();
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let Some(slash) = rest.find('/') else {
        return url.to_string();
    };
    let (authority, path) = rest.split_at(slash);
    let clean = path.split(['?', '#']).next().unwrap_or(path);
    let mut segments: Vec<&str> = clean.trim_start_matches('/').split('/').collect();
    if segments.len() >= 2 && is_locale_segment(segments[0]) {
        segments.remove(0);
    }
    if segments.first().copied() != Some("abs") {
        return url.to_string();
    }
    let id = segments[1..].join("/");
    if id.is_empty() {
        return url.to_string();
    }
    format!("{scheme}://{authority}/{lang}/abs/{id}")
}

/// True when a natural-language query asks for the trending/explore feed rather
/// than a keyword search.
fn is_trending_request(s: &str) -> bool {
    let l = s.trim().to_ascii_lowercase();
    if l.is_empty() {
        return false;
    }
    if ["热门", "趋势", "推荐", "最新"]
        .iter()
        .any(|k| l.contains(k))
    {
        return true;
    }
    if l.contains("/trending") {
        return true;
    }
    if matches!(l.as_str(), "explore" | "feed" | "trending papers") {
        return true;
    }
    l.split_whitespace().any(|token| token == "trending")
}

/// The alphaXiv feed time window, restricted to the API's allowed values.
/// Anything unrecognised (or absent) falls back to `7 Days`.
fn alphaxiv_interval(with: &Value) -> String {
    const ALLOWED: [&str; 5] = ["3 Days", "7 Days", "30 Days", "90 Days", "All time"];
    let raw = with.get("interval").map(stringify).unwrap_or_default();
    let raw = raw.trim();
    ALLOWED
        .iter()
        .find(|a| a.eq_ignore_ascii_case(raw))
        .map(|s| s.to_string())
        .unwrap_or_else(|| "7 Days".to_string())
}

/// Upper bound on `count`, so a typo cannot kick off an unbounded crawl. Papers
/// are downloaded one by one, so this is deliberately the only limit.
const ALPHAXIV_MAX_COUNT: u64 = 500;

/// Resolve a call's intent into `{mode, query, target_url, count}`.
///
/// `mode` may be forced (`url`/`search`/`trending`); the default `auto` infers
/// it from the inputs: an explicit paper URL → `url`, a trending keyword →
/// `trending`, anything else → `search`. This is what lets the CLI accept a
/// bare `--query "llm memory"` or `--query "trending"` with no JSON state.
fn alphaxiv_plan(with: &Value) -> Result<Value> {
    let pick = |k: &str| -> Option<String> {
        with.get(k)
            .map(stringify)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "null")
    };
    let query = pick("query")
        .or_else(|| pick("q"))
        .or_else(|| pick("topic"))
        .unwrap_or_default();
    let paper_url = pick("paper_url").or_else(|| pick("url"));
    let explicit = pick("mode").unwrap_or_default().to_ascii_lowercase();
    let mode = match explicit.as_str() {
        "" | "auto" => {
            if paper_url.is_some() {
                "url"
            } else if query.is_empty() {
                ""
            } else if looks_like_paper_url(&query) {
                "url"
            } else if is_trending_request(&query) {
                "trending"
            } else {
                "search"
            }
        }
        "url" | "paper" | "direct" | "abs" => "url",
        "trending" | "feed" | "explore" | "hot" | "popular" => "trending",
        "search" | "query" | "find" => "search",
        other => {
            bail!("browser alphaxiv mode {other:?} unsupported (auto|search|trending|url)")
        }
    };
    if mode.is_empty() {
        bail!(
            "browser alphaxiv needs 'query' (search text, a paper URL, or 'trending') \
             or 'paper_url'"
        );
    }
    let target_url = if mode == "url" {
        let url = match paper_url.clone() {
            Some(u) => u,
            None if looks_like_paper_url(&query) => query.clone(),
            None => {
                bail!("browser alphaxiv url mode needs a paper URL in 'paper_url' or 'query'")
            }
        };
        Some(url)
    } else {
        None
    };
    // Trending defaults to a full page of ten. Its count can reach
    // ALPHAXIV_MAX_COUNT because the feed is paged through the API; search is
    // bounded by the single rendered results page (~10 cards).
    let trending = mode == "trending";
    let default_count = if trending { 10 } else { 1 };
    let max_count = if trending { ALPHAXIV_MAX_COUNT } else { 10 };
    let count = with
        .get("count")
        .and_then(Value::as_u64)
        .unwrap_or(default_count)
        .clamp(1, max_count);
    Ok(json!({
        "mode": mode,
        "query": query,
        "target_url": target_url,
        "count": count,
    }))
}

/// Open a listing page (search or trending) in a scratch background tab, read
/// the rendered `/abs/` cards, and close the tab again.
fn collect_alphaxiv_links(
    c: &BrowserCap,
    endpoint: &str,
    page_url: &str,
    limit: usize,
    timeout: Duration,
    policy: &Policy,
) -> Result<Vec<Value>> {
    check_host(page_url, policy)?;
    enforce_owned_target_limit(endpoint, c.max_owned_pages, c.owned_idle_ms, timeout)?;
    let mut guard = TargetCleanupGuard::new(endpoint);
    let id = open_background_target(endpoint, timeout)?;
    track_owned_target(endpoint, &id, false);
    guard.track(id.clone());
    let outcome = (|| -> Result<Vec<Value>> {
        cdp_page_call(
            endpoint,
            &id,
            "Page.navigate",
            json!({"url": page_url}),
            timeout,
        )?;
        let _ = wait_page_ready(endpoint, &id, timeout, None);
        let poll_ms = timeout.as_millis().saturating_sub(4000).clamp(3000, 12000) as u64;
        let opts = json!({"limit": limit, "timeout_ms": poll_ms});
        let expression = format!(
            "var __LAYA_OPTS__ = {};\n{}",
            serde_json::to_string(&opts)?,
            ALPHAXIV_LINKS_JS
        );
        let value = evaluate(endpoint, &id, &expression, timeout, true)?;
        Ok(value
            .get("links")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    })();
    let _ = guard.finish(true, timeout);
    outcome
}

/// Page through alphaXiv's public feed API inside a scratch tab (the browser
/// supplies the first-party origin the API expects) and return `/abs/<id>`
/// rows, so trending can exceed the handful of cards the homepage renders.
fn collect_alphaxiv_feed(
    c: &BrowserCap,
    endpoint: &str,
    count: usize,
    interval: &str,
    timeout: Duration,
    policy: &Policy,
) -> Result<Vec<Value>> {
    check_host(ALPHAXIV_HOME, policy)?;
    enforce_owned_target_limit(endpoint, c.max_owned_pages, c.owned_idle_ms, timeout)?;
    let mut guard = TargetCleanupGuard::new(endpoint);
    let id = open_background_target(endpoint, timeout)?;
    track_owned_target(endpoint, &id, false);
    guard.track(id.clone());
    let outcome = (|| -> Result<Vec<Value>> {
        cdp_page_call(
            endpoint,
            &id,
            "Page.navigate",
            json!({"url": ALPHAXIV_HOME}),
            timeout,
        )?;
        let _ = wait_page_ready(endpoint, &id, timeout, None);
        let opts = json!({"count": count, "sort": "Hot", "interval": interval});
        let expression = format!(
            "var __LAYA_OPTS__ = {};\n{}",
            serde_json::to_string(&opts)?,
            ALPHAXIV_FEED_JS
        );
        let value = evaluate(endpoint, &id, &expression, timeout, true)?;
        Ok(value
            .get("links")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    })();
    let _ = guard.finish(true, timeout);
    outcome
}

/// Natural-language alphaXiv downloader.
///
/// One op turns a `query` (a search phrase, a paper URL, or a trending keyword)
/// into saved papers: it discovers the `/abs/<id>` links, then renders each with
/// `save_article` and writes Markdown + figures under `out_dir`.
fn run_alphaxiv(
    c: &BrowserCap,
    endpoint: &str,
    with: &Value,
    timeout: Duration,
    policy: &Policy,
) -> Result<Value> {
    let plan = alphaxiv_plan(with)?;
    let mode = plan.get("mode").and_then(Value::as_str).unwrap_or("search");
    let query = plan
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let count = plan.get("count").and_then(Value::as_u64).unwrap_or(1) as usize;
    let out_dir = with
        .get("out_dir")
        .map(stringify)
        .filter(|s| !s.is_empty() && s != "null")
        .unwrap_or_else(|| "~/tmp/alphaxiv".to_string());
    // Fail fast on a denied output root before opening any tab.
    let _ = allowed_out_dir(policy, &out_dir)?;
    let selector = with
        .get("selector")
        .map(stringify)
        .filter(|s| !s.is_empty() && s != "null");
    // Content language for the saved papers; alphaXiv serves translated abs
    // pages under `/<lang>/abs/<id>`. Chinese by default.
    let lang = with
        .get("lang")
        .or_else(|| with.get("language"))
        .map(stringify)
        .filter(|s| !s.is_empty() && s != "null")
        .unwrap_or_else(|| "zh".to_string());
    // Base directory for figures; each paper gets its own `<base>/<slug>`
    // folder so a multi-paper run cannot overwrite another paper's downloads.
    let image_base = with
        .get("image_dir")
        .map(stringify)
        .filter(|s| !s.is_empty() && s != "null")
        .unwrap_or_else(|| "images".to_string());
    let stable_ms = with
        .get("stable_ms")
        .and_then(Value::as_u64)
        .unwrap_or(1500);
    let max_wait_ms = with
        .get("max_wait_ms")
        .and_then(Value::as_u64)
        .unwrap_or(45_000)
        .min(policy.max_timeout_ms);
    let keep_open = with
        .get("keep_open")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // Discover a few extra cards so a stray/degenerate first link cannot starve
    // the requested count, then trim to `count`.
    let collect_limit = count.clamp(5, 10);

    let (source_url, discovered, discovery) = match mode {
        "url" => {
            let raw = plan
                .get("target_url")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("alphaxiv url mode resolved no URL"))?;
            let url = localize_alphaxiv_url(raw, &lang);
            (url.clone(), vec![json!({"url": url, "title": ""})], "url")
        }
        "search" => {
            let url = format!(
                "https://www.alphaxiv.org/?query={}",
                urlencoding_utf8(query)
            );
            let links = collect_alphaxiv_links(c, endpoint, &url, collect_limit, timeout, policy)?;
            (url, links, "dom")
        }
        "trending" => {
            // Prefer the public feed API so `count` can exceed a single rendered
            // page; fall back to scraping the homepage row if it is unreachable.
            let url = ALPHAXIV_HOME.to_string();
            let interval = alphaxiv_interval(with);
            match collect_alphaxiv_feed(c, endpoint, count, &interval, timeout, policy) {
                Ok(found) if !found.is_empty() => (url, found, "feed_api"),
                _ => {
                    let links =
                        collect_alphaxiv_links(c, endpoint, &url, collect_limit, timeout, policy)?;
                    (url, links, "dom")
                }
            }
        }
        other => bail!("alphaxiv resolved mode {other:?} is not runnable"),
    };

    let selected: Vec<Value> = discovered.into_iter().take(count).collect();
    if selected.is_empty() {
        bail!(
            "alphaxiv {mode} found no papers at {source_url:?} (query {query:?}); \
             the listing may still have been loading"
        );
    }

    let mut results = Vec::new();
    let mut failures = Vec::new();
    for (index, link) in selected.iter().enumerate() {
        let raw = link.get("url").and_then(Value::as_str).unwrap_or_default();
        let url = localize_alphaxiv_url(raw, &lang);
        let title_hint = link
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let slug = derive_slug(&url, title_hint);
        let mut leaf = json!({
            "op": "save_article",
            "url": url,
            "slug": slug,
            "out_dir": out_dir,
            "image_dir": format!("{image_base}/{slug}"),
            "stable_ms": stable_ms,
            "max_wait_ms": max_wait_ms,
            "keep_open": keep_open,
        });
        if let Some(selector) = selector.as_ref() {
            leaf["selector"] = json!(selector);
        }
        // One retry: over a long page-through a single transient CDP error
        // (e.g. a navigation timeout) should not drop the paper.
        let mut saved = None;
        let mut last_error = None;
        let mut attempts = 0;
        for _ in 0..2 {
            attempts += 1;
            match run_save_article(c, endpoint, &leaf, timeout, policy) {
                Ok(value) => {
                    saved = Some(value);
                    break;
                }
                Err(e) => last_error = Some(e),
            }
        }
        match saved {
            Some(saved) => results.push(json!({
                "rank": index + 1,
                "url": url,
                "title_hint": title_hint,
                "attempts": attempts,
                "title": saved.get("title").cloned().unwrap_or(Value::Null),
                "markdown_chars": saved.get("markdown_chars").cloned().unwrap_or(Value::Null),
                "rendered": saved.get("rendered").cloned().unwrap_or(Value::Null),
                "images": saved.get("images").cloned().unwrap_or(Value::Null),
                "written": saved.get("written").cloned().unwrap_or(Value::Null),
            })),
            None => failures.push(json!({
                "rank": index + 1,
                "url": url,
                "title_hint": title_hint,
                "attempts": attempts,
                "error": last_error
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "unknown error".to_string()),
            })),
        }
    }

    Ok(json!({
        "capability": "chrome_cdp",
        "op": "alphaxiv",
        "mode": mode,
        "query": query,
        "lang": lang,
        "discovery": discovery,
        "source_url": source_url,
        "out_dir": out_dir,
        "requested_count": count,
        "selected_count": selected.len(),
        "saved_count": results.len(),
        "failed_count": failures.len(),
        "results": results,
        "failures": failures,
    }))
}

/// Count case-insensitive token occurrences. Chinese tokens are short strings,
/// so counting a substring is intentional here.
fn research_count(hay: &str, token: &str) -> usize {
    if token.is_empty() {
        return 0;
    }
    let hay = research_norm(hay);
    let token = research_norm(token);
    hay.matches(&token).count()
}

/// Laya deterministic page scorer: semantic overlap drives relevance, while
/// readable length, structure, code, citations and freshness drive usefulness.
fn score_research_page(
    query: &str,
    item: &Value,
    result_url: &str,
) -> (f64, f64, f64, Vec<String>) {
    let title = item
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let search_title = item
        .get("search_title")
        .and_then(Value::as_str)
        .unwrap_or(title);
    let snippet = item
        .get("search_snippet")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let text = item.get("text").and_then(Value::as_str).unwrap_or_default();
    let host = item.get("host").and_then(Value::as_str).unwrap_or_default();
    let headings = item.get("headings").and_then(Value::as_u64).unwrap_or(0);
    let paragraphs = item.get("paragraphs").and_then(Value::as_u64).unwrap_or(0);
    let code_blocks = item.get("code_blocks").and_then(Value::as_u64).unwrap_or(0);
    let links = item.get("links").and_then(Value::as_u64).unwrap_or(0);
    let published = item
        .get("published_hint")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tokens: Vec<String> = query
        .split_whitespace()
        .map(research_norm)
        .filter(|x| !x.is_empty())
        .collect();
    let token_count = tokens.len().max(1) as f64;
    let title_hay = format!("{title} {search_title}");
    let title_hits = tokens
        .iter()
        .filter(|t| research_count(&title_hay, t) > 0)
        .count() as f64;
    let snippet_hits = tokens
        .iter()
        .filter(|t| research_count(snippet, t) > 0)
        .count() as f64;
    let text_hits = tokens
        .iter()
        .filter(|t| research_count(text, t) > 2)
        .count() as f64;
    let phrase =
        !research_norm(query).is_empty() && research_norm(text).contains(&research_norm(query));
    let url_hits = tokens
        .iter()
        .filter(|t| research_count(result_url, t) > 0)
        .count() as f64;
    let relevance = (0.38 * title_hits / token_count
        + 0.17 * snippet_hits / token_count
        + 0.25 * text_hits / token_count
        + 0.08 * if phrase { 1.0 } else { 0.0 }
        + 0.12 * url_hits / token_count)
        .clamp(0.0, 1.0);

    let len = text.len();
    let length_score = if len < 300 {
        0.05
    } else if len < 800 {
        0.16
    } else if len < 2000 {
        0.28
    } else if len < 8000 {
        0.36
    } else {
        0.32
    };
    let structure_score = ((headings.min(10)) as f64 / 10.0 * 0.55
        + (paragraphs.min(15)) as f64 / 15.0 * 0.45)
        .clamp(0.0, 1.0);
    let code_score = if code_blocks > 0 { 1.0 } else { 0.0 };
    let citation_score = if links >= 10 {
        1.0
    } else if links >= 5 {
        0.7
    } else if links >= 1 {
        0.3
    } else {
        0.0
    };
    let authority = if [
        "docs.",
        "github.com",
        "wikipedia.org",
        "developer.mozilla.org",
        "arxiv.org",
        "official",
    ]
    .iter()
    .any(|x| host.to_lowercase().contains(x))
    {
        1.0
    } else {
        0.0
    };
    let fresh = if ["2024", "2025", "2026"]
        .iter()
        .any(|x| published.contains(x))
    {
        1.0
    } else {
        0.0
    };
    let effectiveness = (length_score * 0.34
        + structure_score * 0.24
        + code_score * 0.14
        + citation_score * 0.14
        + authority * 0.08
        + fresh * 0.06)
        .clamp(0.0, 1.0);

    let score = (relevance * 0.66 + effectiveness * 0.34).clamp(0.0, 1.0);
    let mut reasons = vec![
        format!("relevance={relevance:.3}"),
        format!("effectiveness={effectiveness:.3}"),
        format!("text_length={len}"),
        format!("structure=headings:{headings},paragraphs:{paragraphs},code:{code_blocks},links:{links}"),
    ];
    if relevance < 0.45 {
        reasons.push("query overlap is weak".into());
    }
    if len < 600 {
        reasons.push("page text is too short".into());
    }
    (score, relevance, effectiveness, reasons)
}

/// Produce a conservative, deterministic classification for downloaded research
/// pages. This is not an LLM claim: it records URL/title/text signals so callers
/// can filter documentation, source code, papers, discussions, and product pages.
fn classify_research_page(
    query: &str,
    item: &Value,
    page: &Value,
    result_url: &str,
    score: f64,
    relevance: f64,
) -> Value {
    let title = item
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let search_title = item
        .get("search_title")
        .and_then(Value::as_str)
        .unwrap_or(title);
    let text = page.get("text").and_then(Value::as_str).unwrap_or_default();
    let url = result_url.to_lowercase();
    let title_hay = format!("{title} {search_title}");
    let text_hay = format!("{title_hay} {text}");
    let code_blocks = page.get("code_blocks").and_then(Value::as_u64).unwrap_or(0);

    let content_type;
    let mut signals = Vec::new();
    if ["github.com", "gitlab.com", "crates.io", "npmjs.com"]
        .iter()
        .any(|x| url.contains(x))
    {
        content_type = "code";
        signals.push("code hosting URL".into());
    } else if ["arxiv.org", "doi.org/", ".pdf", "aclanthology.org"]
        .iter()
        .any(|x| url.contains(x))
        || ["paper", "arxiv", "proceedings", "abstract"]
            .iter()
            .any(|x| title_hay.to_lowercase().contains(x))
    {
        content_type = "paper";
        signals.push("academic paper signal".into());
    } else if url.contains("docs.")
        || url.contains("developer.")
        || ["docs.", "developer.", "developer.mozilla.org"]
            .iter()
            .any(|x| host_of(result_url).to_lowercase().contains(x))
        || ["documentation", "/docs/", "/reference/", "/api/", "manual"]
            .iter()
            .any(|x| url.contains(x) || title_hay.to_lowercase().contains(x))
    {
        content_type = "documentation";
        signals.push("documentation URL/title".into());
    } else if code_blocks > 0
        || ["example", "tutorial", "guide", "how to", "how-to"]
            .iter()
            .any(|x| title_hay.to_lowercase().contains(x))
    {
        content_type = "tutorial";
        signals.push("example/tutorial or code blocks".into());
    } else if [
        "reddit.com",
        "news.ycombinator.com",
        "stackoverflow.com",
        "forum",
        "discussion",
    ]
    .iter()
    .any(|x| url.contains(x) || title_hay.to_lowercase().contains(x))
    {
        content_type = "discussion";
        signals.push("community discussion URL/title".into());
    } else if ["pricing", "product", "platform", "enterprise", "customers"]
        .iter()
        .any(|x| url.contains(x) || title_hay.to_lowercase().contains(x))
    {
        content_type = "product";
        signals.push("product/pricing signal".into());
    } else if ["blog", "news", "announcement", "release"]
        .iter()
        .any(|x| url.contains(x) || title_hay.to_lowercase().contains(x))
    {
        content_type = "news";
        signals.push("news/blog/release signal".into());
    } else if text.len() >= 800 {
        content_type = "blog";
        signals.push("long-form page".into());
    } else {
        content_type = "unknown";
        signals.push("no strong content-type signal".into());
    }

    let query_tokens: Vec<String> = query
        .split_whitespace()
        .map(research_norm)
        .filter(|x| !x.is_empty())
        .collect();
    let token_hits = query_tokens
        .iter()
        .filter(|token| research_count(&text_hay, token) > 0)
        .count();
    let topic_labels: Vec<String> = query_tokens
        .iter()
        .filter(|token| research_count(&text_hay, token) > 0)
        .cloned()
        .take(8)
        .collect();
    if !topic_labels.is_empty() {
        signals.insert(
            0,
            format!("query term coverage: {token_hits}/{}", query_tokens.len()),
        );
    }

    let relevance_class = if relevance >= 0.70 && score >= 0.65 {
        "highly_relevant"
    } else if relevance >= 0.45 && score >= 0.52 {
        "relevant"
    } else if relevance >= 0.20 || token_hits > 0 {
        "possibly_relevant"
    } else {
        "irrelevant"
    };

    let confidence = (0.38 * score
        + 0.34 * relevance
        + 0.12 * (token_hits as f64 / query_tokens.len().max(1) as f64)
        + if content_type == "unknown" { 0.0 } else { 0.16 })
    .clamp(0.0, 1.0);
    let confidence = (confidence * 1000.0).round() / 1000.0;

    json!({
        "content_type": content_type,
        "topic_labels": topic_labels,
        "relevance_class": relevance_class,
        "confidence": confidence,
        "signals": signals
    })
}

/// Open a background target and return its CDP target id. This never activates
/// the target and therefore never changes the user's visible tab/window.
fn open_background_target(endpoint: &str, timeout: Duration) -> Result<String> {
    let created = cdp_browser_call(
        endpoint,
        "Target.createTarget",
        json!({"url": "about:blank", "background": true}),
        timeout,
    )?;
    let id = created
        .get("targetId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("Chrome Target.createTarget returned no targetId"))?;
    Ok(id)
}

/// Run a complete Google research pass: result extraction, ad filtering,
/// background browsing, text extraction and Laya scoring.
fn run_research(
    c: &BrowserCap,
    endpoint: &str,
    with: &Value,
    timeout: Duration,
    policy: &Policy,
) -> Result<Value> {
    let query = match with.get("query") {
        None | Some(Value::Null) => {
            bail!("browser research needs a non-empty 'query' in state")
        }
        Some(value) => stringify(value),
    };
    if query.trim().is_empty() || query == "null" {
        bail!("browser research needs a non-empty 'query' in state");
    }
    let result_count = with
        .get("result_count")
        .and_then(Value::as_u64)
        .unwrap_or(10)
        .clamp(1, 20) as usize;
    let max_open = with
        .get("max_open")
        .and_then(Value::as_u64)
        .unwrap_or((result_count + 8) as u64)
        .clamp(result_count as u64, 30) as usize;
    let min_score = with
        .get("min_score")
        .and_then(Value::as_f64)
        .unwrap_or(0.52)
        .clamp(0.0, 1.0);
    let max_text = with
        .get("max_text")
        .and_then(Value::as_u64)
        .unwrap_or(c.max_text as u64)
        .clamp(500, 20_000) as usize;
    let page_timeout = timeout.min(Duration::from_secs(12));
    let keep_open_pages = with
        .get("keep_open_pages")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let tab_concurrency = tab_concurrency_of(with);
    let cleanup = Arc::new(Mutex::new(TargetCleanupGuard::new(endpoint)));
    let manually_closed = Arc::new(Mutex::new(0usize));
    enforce_owned_target_limit(endpoint, c.max_owned_pages, c.owned_idle_ms, page_timeout)?;

    let mut search_target: Option<String> = None;
    let mut ads = Vec::new();
    let skip_kinds = skip_kinds_of(with);
    let mut skipped = Vec::new();
    let mut candidates = Vec::new();
    let mut seen_titles = HashSet::new();
    let mut seen_urls = HashSet::new();

    // Google may return fewer than ten visible rows per page. Paginate enough
    // rows to survive deleted links, redirects and duplicate URLs.
    for start in [0u64, 10, 20, 30, 40] {
        let encoded = urlencoding_utf8(&query);
        let url = format!(
            "https://www.google.com/search?q={encoded}&num=20&filter=0&hl=zh-CN&start={start}"
        );
        if search_target.is_none() {
            search_target = Some(open_background_target(endpoint, timeout)?);
            if let Some(id) = search_target.as_ref() {
                if keep_open_pages {
                    track_owned_target(endpoint, id, true);
                } else {
                    cleanup
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .track(id.clone());
                    track_owned_target(endpoint, id, false);
                }
            }
        }
        let id = search_target.as_ref().unwrap();
        cdp_page_call(endpoint, id, "Page.navigate", json!({"url": url}), timeout)?;
        wait_page_ready(endpoint, id, timeout, None)?;
        let value = evaluate(endpoint, id, GOOGLE_RESULTS_JS, timeout, false)?;
        let rows = value
            .get("candidates")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        // Resolve Google's redirect wrapper for the whole page up front: every
        // lookup is an independent header-only request, so a page's rows resolve
        // concurrently instead of one network round trip after another.
        let destinations: Vec<String> = {
            let tasks: Vec<(String, bool)> = rows
                .iter()
                .map(|row| {
                    (
                        row.get("url")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        row.get("is_ad").and_then(Value::as_bool).unwrap_or(false),
                    )
                })
                .collect();
            let mut resolved = vec![String::new(); tasks.len()];
            let workers = 8usize;
            for chunk in (0..tasks.len()).collect::<Vec<_>>().chunks(workers) {
                std::thread::scope(|scope| {
                    let handles: Vec<_> = chunk
                        .iter()
                        .map(|&i| {
                            let (url, is_ad) = tasks[i].clone();
                            scope.spawn(move || {
                                let destination = if is_ad {
                                    url
                                } else {
                                    resolve_destination_url(&url)
                                };
                                (i, destination)
                            })
                        })
                        .collect();
                    for handle in handles {
                        if let Ok((i, destination)) = handle.join() {
                            resolved[i] = destination;
                        }
                    }
                });
            }
            resolved
        };
        for (mut row, destination) in rows.into_iter().zip(destinations) {
            let title = row
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let url = row
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if title.is_empty() || url.is_empty() {
                continue;
            }
            if row.get("is_ad").and_then(Value::as_bool).unwrap_or(false) {
                ads.push(json!({"title": title, "url": url, "reason": "explicit Google ad label/ad landing path"}));
                continue;
            }
            let kind = url_content_kind(&destination);
            if skip_kinds.iter().any(|k| k == kind) {
                skipped.push(json!({
                    "title": title,
                    "url": url,
                    "destination": destination,
                    "host": host_of(&destination),
                    "kind": kind,
                    "reason": "classified before opening; video/media pages are skipped"
                }));
                continue;
            }
            if seen_titles.insert(title.clone()) && seen_urls.insert(url.clone()) {
                row["start"] = json!(start);
                candidates.push(row);
            }
        }
        if candidates.len() >= max_open {
            break;
        }
    }

    let mut pages = Vec::new();
    let mut failed = Vec::new();
    let selected: Vec<(usize, &Value)> = candidates.iter().take(max_open).enumerate().collect();
    for batch in selected.chunks(tab_concurrency) {
        let open_lock = Arc::new(Mutex::new(()));
        let mut batch_outcomes = Vec::with_capacity(batch.len());
        std::thread::scope(|scope| {
            let handles = batch
                .iter()
                .map(|(rank, candidate)| {
                    let cleanup = Arc::clone(&cleanup);
                    let manually_closed = Arc::clone(&manually_closed);
                    let open_lock = Arc::clone(&open_lock);
                    let query = query.clone();
                    scope.spawn(move || -> Result<(Option<Value>, Option<Value>)> {
                        let title = candidate
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let result_url = candidate
                            .get("url")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if check_host(result_url, policy).is_err() {
                            return Ok((None, Some(json!({"title": title, "url": result_url, "error": "policy denied"}))));
                        }

                        // Serialize only the resource-control + create sequence.
                        // Once the target is registered, navigation/extraction can
                        // proceed concurrently on independent page websockets.
                        let opened = {
                            let _permit = open_lock.lock().unwrap_or_else(|e| e.into_inner());
                            enforce_owned_target_limit(
                                endpoint,
                                c.max_owned_pages,
                                c.owned_idle_ms,
                                page_timeout,
                            )?;
                            open_background_target(endpoint, page_timeout)
                        };
                        let target_id = match opened {
                            Ok(x) => x,
                            Err(e) => {
                                return Ok((None, Some(json!({"title": title, "url": result_url, "error": e.to_string()}))));
                            }
                        };
                        if keep_open_pages {
                            track_owned_target(endpoint, &target_id, true);
                        } else {
                            cleanup
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .track(target_id.clone());
                            track_owned_target(endpoint, &target_id, false);
                        }
                        let navigation = cdp_page_call(
                            endpoint,
                            &target_id,
                            "Page.navigate",
                            json!({"url": result_url}),
                            page_timeout,
                        )
                        .and_then(|_| wait_page_ready(endpoint, &target_id, page_timeout, None));
                        if let Err(e) = navigation {
                            let _ = http_close(endpoint, &target_id, Duration::from_secs(1));
                            if cleanup
                                .lock()
                                .unwrap_or_else(|err| err.into_inner())
                                .untrack(&target_id)
                            {
                                *manually_closed.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                            }
                            return Ok((None, Some(json!({"title": title, "url": result_url, "target_id": target_id, "error": e.to_string()}))));
                        }
                        let mut page = match evaluate(
                            endpoint,
                            &target_id,
                            READABLE_PAGE_JS,
                            page_timeout,
                            false,
                        ) {
                            Ok(x) => x,
                            Err(e) => {
                                let _ = http_json(
                                    endpoint,
                                    "GET",
                                    &format!("/json/close/{target_id}"),
                                    Duration::from_secs(1),
                                );
                                if cleanup
                                    .lock()
                                    .unwrap_or_else(|err| err.into_inner())
                                    .untrack(&target_id)
                                {
                                    *manually_closed.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                                }
                                return Ok((None, Some(json!({"title": title, "url": result_url, "target_id": target_id, "error": e.to_string()}))));
                            }
                        };
                        if page
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .len()
                            > max_text
                        {
                            let clipped: String = page
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .chars()
                                .take(max_text)
                                .collect();
                            if let Some(obj) = page.as_object_mut() {
                                obj.insert("text".into(), json!(clipped));
                                obj.insert("content_truncated".into(), json!(true));
                            }
                        } else if let Some(obj) = page.as_object_mut() {
                            obj.insert("content_truncated".into(), json!(false));
                        }
                        page["search_rank"] = json!(rank);
                        page["search_title"] = json!(title);
                        page["search_url"] = json!(result_url);
                        page["search_snippet"] = candidate.get("snippet").cloned().unwrap_or(Value::Null);
                        let final_url = page
                            .get("final_url")
                            .and_then(Value::as_str)
                            .unwrap_or(result_url);
                        let (score, relevance, effectiveness, reasons) =
                            score_research_page(&query, &page, final_url);
                        let content_chars = page
                            .get("text")
                            .and_then(Value::as_str)
                            .map(|x| x.chars().count())
                            .unwrap_or_default();
                        let classification = classify_research_page(
                            &query,
                            candidate,
                            &page,
                            final_url,
                            score,
                            relevance,
                        );
                        let accepted = score >= min_score
                            && relevance >= 0.40
                            && page
                                .get("text")
                                .and_then(Value::as_str)
                                .map(str::len)
                                .unwrap_or_default()
                                >= 600;
                        page["target_id"] = json!(target_id);
                        page["score"] = json!((score * 1000.0).round() / 1000.0);
                        page["relevance"] = json!((relevance * 1000.0).round() / 1000.0);
                        page["effectiveness"] = json!((effectiveness * 1000.0).round() / 1000.0);
                        page["content_chars"] = json!(content_chars);
                        page["classification"] = classification;
                        page["reasons"] = json!(reasons);
                        page["accepted"] = json!(accepted);
                        Ok((Some(page), None))
                    })
                })
            .collect::<Vec<_>>();
            for handle in handles {
                batch_outcomes.push(handle.join());
            }
            Result::<()>::Ok(())
        })?;
        for outcome in batch_outcomes {
            let (page, failure) = match outcome {
                Ok(result) => result?,
                Err(panic) => anyhow::bail!("research worker panicked: {panic:?}"),
            };
            if let Some(page) = page {
                pages.push(page);
            }
            if let Some(failure) = failure {
                failed.push(failure);
            }
        }
    }

    pages.sort_by(|a, b| {
        b.get("score")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .partial_cmp(&a.get("score").and_then(Value::as_f64).unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.get("search_rank")
                    .and_then(Value::as_u64)
                    .unwrap_or(u64::MAX)
                    .cmp(
                        &b.get("search_rank")
                            .and_then(Value::as_u64)
                            .unwrap_or(u64::MAX),
                    )
            })
    });
    let accepted: Vec<Value> = pages
        .iter()
        .filter(|x| x.get("accepted").and_then(Value::as_bool).unwrap_or(false))
        .take(result_count)
        .cloned()
        .collect();
    let collected = accepted.len();
    let (closed_research_targets, cleanup_errors) = cleanup
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .finish(!keep_open_pages, timeout);
    let manually_closed = *manually_closed.lock().unwrap_or_else(|e| e.into_inner());
    Ok(json!({
        "capability": "chrome_cdp",
        "op": "research",
        "engine": "google",
        "query": query,
        "scorer": "laya-browser-heuristic-v1",
        "background_only": true,
        "tab_concurrency": tab_concurrency,
        "result_count_requested": result_count,
        "search_candidates": candidates.len(),
        "ads_found": ads.len(),
        "ads": ads,
        "skipped_found": skipped.len(),
        "skipped": skipped,
        "skip_kinds": skip_kinds,
        "pages_opened_successfully": pages.len(),
        "pages_failed": failed.len(),
        "failures": failed,
        "scored_pages": pages,
        "accepted_pages": accepted,
        "collected_count": collected,
        "min_score": min_score,
        "complete": collected >= result_count,
        "cleanup": {
                "enabled": !keep_open_pages,
                "opened_count": cleanup
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .ids
                    .len()
                    + manually_closed,
            "closed_count": closed_research_targets + manually_closed,
            "errors": cleanup_errors
        }
    }))
}

pub fn call_browser(c: &BrowserCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    let op = with
        .get("op")
        .map(stringify)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "snapshot".to_string());
    let endpoint = ensure_runtime(c, with, state, policy)?;
    let timeout = bounded_timeout(
        with.get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(c.timeout_ms),
        policy,
    );
    let mut out = json!({"capability": "chrome_cdp", "op": op, "endpoint": endpoint});

    match op.as_str() {
        "research" => {
            return run_research(c, &endpoint, with, timeout, policy);
        }
        "save_article" | "save_markdown" | "fetch_article" | "save_page" => {
            return run_save_article(c, &endpoint, with, timeout, policy);
        }
        "alphaxiv" | "alphaxiv_search" | "fetch_papers" | "trending_papers" | "paper_search" => {
            return run_alphaxiv(c, &endpoint, with, timeout, policy);
        }
        "status" | "targets" => {
            let version = http_json(&endpoint, "GET", "/json/version", timeout)?;
            let targets = list_targets(&endpoint, timeout)?;
            for t in &targets {
                target_allowed(t, policy)?;
            }
            out["version"] = version;
            out["targets"] = Value::Array(targets);
        }
        "open" => {
            let url = match with.get("url") {
                Some(Value::Null) => bail!("browser open needs a non-empty 'url'"),
                None => "about:blank".to_string(),
                Some(value) => stringify(value),
            };
            if url.is_empty() || url == "null" {
                bail!("browser open needs a non-empty 'url'");
            }
            if url != "about:blank" {
                check_host(&url, policy)?;
            }
            enforce_owned_target_limit(
                &endpoint,
                c.max_owned_pages,
                c.owned_idle_ms,
                timeout,
            )?;
            // Target.createTarget navigates a background target reliably and
            // avoids the user-focus side effects of Target.activateTarget.
            let created = cdp_browser_call(
                &endpoint,
                "Target.createTarget",
                json!({"url": url, "background": true}),
                timeout,
            )?;
            out["target"] = created.clone();
            if let Some(id) = created.get("targetId").and_then(Value::as_str) {
                out["target_id"] = json!(id);
                let keep_open = with
                    .get("keep_open")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                track_owned_target(&endpoint, id, keep_open);
                out["owner"] = json!("laya-workflow");
                out["owned_singleton"] = json!(true);
                out["keep_open"] = json!(keep_open);
                out["auto_close"] = json!(!keep_open);
                let expected = if url == "about:blank" { None } else { Some(url.as_str()) };
                match wait_page_ready(&endpoint, &id, timeout, expected) {
                    Ok(ready) => out["ready"] = ready,
                    Err(e) => {
                        let _ = http_close(&endpoint, &id, timeout);
                        forget_owned_target(id);
                        bail!("browser open failed while waiting for {url:?}: {e}");
                    }
                }
            }
        }
        "open_many" => {
            let urls = with
                .get("urls")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("browser open_many needs 'urls' (array)"))?;
            if urls.len() > 100 {
                bail!("browser open_many accepts at most 100 URLs");
            }
            let keep_open = with
                .get("keep_open")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let tab_concurrency = tab_concurrency_of(with);
            let skip_kinds = skip_kinds_of(with);
            // Validate every URL first so a policy denial fails the batch without
            // leaving a partial set of tabs behind.
            let mut parsed = Vec::with_capacity(urls.len());
            let mut skipped = Vec::new();
            for (index, value) in urls.iter().enumerate() {
                let url = value.as_str().ok_or_else(|| {
                    anyhow!("browser open_many urls[{index}] must be a string")
                })?;
                check_host(url, policy)?;
                let destination = resolve_destination_url(url);
                let kind = url_content_kind(&destination);
                if skip_kinds.iter().any(|k| k == kind) {
                    skipped.push(json!({
                        "index": index,
                        "url": url,
                        "destination": destination,
                        "host": host_of(&destination),
                        "kind": kind,
                        "reason": "classified before opening; video/media pages are skipped"
                    }));
                    continue;
                }
                parsed.push((index, url.to_string()));
            }
            let cleanup = Arc::new(Mutex::new(TargetCleanupGuard::new(&endpoint)));
            let open_lock = Arc::new(Mutex::new(()));
            let mut opened = Vec::with_capacity(parsed.len());
            for batch in parsed.chunks(tab_concurrency) {
                let results: Vec<Result<Value>> = std::thread::scope(|scope| {
                    let handles: Vec<_> = batch
                        .iter()
                        .map(|(index, url)| {
                            let cleanup = Arc::clone(&cleanup);
                            let open_lock = Arc::clone(&open_lock);
                            let index = *index;
                            let url = url.clone();
                            let endpoint = endpoint.clone();
                            scope.spawn(move || -> Result<Value> {
                                // Only the resource-control + create sequence is
                                // serialized; Chrome handles the create calls.
                                let target_id = {
                                    let _permit =
                                        open_lock.lock().unwrap_or_else(|e| e.into_inner());
                                    enforce_owned_target_limit(
                                        &endpoint,
                                        c.max_owned_pages,
                                        c.owned_idle_ms,
                                        timeout,
                                    )?;
                                    let created = cdp_browser_call(
                                        &endpoint,
                                        "Target.createTarget",
                                        json!({"url": url, "background": true}),
                                        timeout,
                                    )?;
                                    created
                                        .get("targetId")
                                        .and_then(Value::as_str)
                                        .ok_or_else(|| {
                                            anyhow!(
                                                "Chrome Target.createTarget returned no targetId"
                                            )
                                        })?
                                        .to_string()
                                };
                                track_owned_target(&endpoint, &target_id, keep_open);
                                cleanup
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .track(target_id.clone());
                                Ok(json!({
                                    "index": index,
                                    "url": url,
                                    "target_id": target_id,
                                    "opened": true
                                }))
                            })
                        })
                        .collect();
                    handles
                        .into_iter()
                        .map(|handle| {
                            handle
                                .join()
                                .map_err(|panic| anyhow!("open_many worker panicked: {panic:?}"))
                                .and_then(|result| result)
                        })
                        .collect()
                });
                for result in results {
                    opened.push(result?);
                }
            }
            opened.sort_by_key(|value| value.get("index").and_then(Value::as_u64).unwrap_or(0));
            let opened_count = opened.len();
            let (closed_count, cleanup_errors) = if keep_open {
                cleanup
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .finish(false, timeout)
            } else {
                cleanup
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .finish(true, timeout)
            };
            out["opened"] = Value::Array(opened);
            out["opened_count"] = json!(opened_count);
            out["keep_open"] = json!(keep_open);
            out["tab_concurrency"] = json!(tab_concurrency);
            out["skip_kinds"] = json!(skip_kinds);
            out["skipped_count"] = json!(skipped.len());
            out["skipped"] = Value::Array(skipped);
            out["closed_count"] = json!(closed_count);
            out["cleanup_errors"] = Value::Array(cleanup_errors);
        }
        "navigate" => {
            let url_value = with
                .get("url")
                .ok_or_else(|| anyhow!("browser navigate needs 'url'"))?;
            if url_value.is_null() {
                bail!("browser navigate needs a non-empty 'url'");
            }
            let url = stringify(url_value);
            if url.is_empty() || url == "null" {
                bail!("browser navigate needs a non-empty 'url'");
            }
            check_host(&url, policy)?;
            let target = resolve_target(&endpoint, with, timeout, policy)?;
            let id = target_id(&target)?;
            cdp_page_call(&endpoint, &id, "Page.navigate", json!({"url": url}), timeout)?;
            out["target_id"] = json!(id);
            out["ready"] = wait_page_ready(&endpoint, &id, timeout, Some(url.as_str()))?;
        }
        "snapshot" | "observe" => {
            let target = resolve_target(&endpoint, with, timeout, policy)?;
            let id = target_id(&target)?;
            let max_text = with.get("max_text").and_then(Value::as_u64).unwrap_or(c.max_text as u64).min(200_000) as usize;
            let value = evaluate(
                &endpoint,
                &id,
                &format!(
                    "(window.__layaBrowser ? window.__layaBrowser.snapshot({max_text}) : {FALLBACK_SNAPSHOT_JS})"
                ),
                timeout,
                false,
            )?;
            out = merge_result(out, value)?;
            out["target_id"] = json!(id);
        }
        "highlight" => {
            let target = resolve_target(&endpoint, with, timeout, policy)?;
            let id = target_id(&target)?;
            let enabled = with.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            out["shown"] = evaluate(
                &endpoint,
                &id,
                &format!("window.__layaBrowser ? window.__layaBrowser.highlight({enabled}) : false"),
                timeout,
                false,
            )?;
            out["target_id"] = json!(id);
        }
        "agent_step" | "execute_decision" => {
            let decision = with
                .get("decision")
                .or_else(|| with.get("model_response"))
                .ok_or_else(|| anyhow!("browser agent_step needs 'decision'"))?;
            return execute_agent_step(c, with, state, policy, decision);
        }
        "evaluate" => {
            let target = resolve_target(&endpoint, with, timeout, policy)?;
            let id = target_id(&target)?;
            let expression = with
                .get("expression")
                .or_else(|| with.get("js"))
                .map(stringify)
                .ok_or_else(|| anyhow!("browser evaluate needs 'expression'"))?;
            let value = evaluate(&endpoint, &id, &expression, timeout, with.get("await_promise").and_then(Value::as_bool).unwrap_or(false))?;
            out["target_id"] = json!(id);
            out["value"] = value;
        }
        "click" => {
            let target = resolve_target(&endpoint, with, timeout, policy)?;
            let id = target_id(&target)?;
            let target_js = element_from_request(with)?;
            let expression = format!(
                "(()=>{{const e={target_js}; if(!e) return {{ok:false}}; e.scrollIntoView({{block:'center',behavior:'instant'}}); const r=e.getBoundingClientRect(); return {{ok:true,x:r.left+r.width/2,y:r.top+r.height/2}};}})()"
            );
            let coords = evaluate(&endpoint, &id, &expression, timeout, false)?;
            if coords.get("ok").and_then(Value::as_bool) != Some(true) {
                bail!("browser click target was not found or is not visible");
            }
            let x = coords.get("x").and_then(Value::as_f64).unwrap_or_default();
            let y = coords.get("y").and_then(Value::as_f64).unwrap_or_default();
            let click_commands = [
                ("Page.bringToFront", json!({})),
                ("Input.dispatchMouseEvent", mouse_event("mouseMoved", x, y)),
                ("Input.dispatchMouseEvent", mouse_event("mousePressed", x, y)),
                ("SLEEP", json!(100)),
                ("Input.dispatchMouseEvent", mouse_event("mouseReleased", x, y)),
            ];
            cdp_page_sequence(&endpoint, &id, &click_commands, timeout)?;
            out["target_id"] = json!(id);
            out["clicked"] = json!(true);
        }
        "type" | "type_text" => {
            let target = resolve_target(&endpoint, with, timeout, policy)?;
            let id = target_id(&target)?;
            let text = with
                .get("text")
                .map(stringify)
                .ok_or_else(|| anyhow!("browser type needs 'text'"))?;
            let target_js = element_from_request(with)?;
            let expression = format!(
                "(()=>{{const e={target_js}; if(!e) return false; e.focus(); if(e.value!==undefined) e.select?.(); return true;}})()"
            );
            if evaluate(&endpoint, &id, &expression, timeout, false)?.as_bool() != Some(true) {
                bail!("browser type target was not found");
            }
            type_text(&endpoint, &id, &text, timeout)?;
            out["target_id"] = json!(id);
            out["typed"] = json!(text.chars().count());
        }
        "select" => {
            let target = resolve_target(&endpoint, with, timeout, policy)?;
            let id = target_id(&target)?;
            let value = with
                .get("value")
                .cloned()
                .ok_or_else(|| anyhow!("browser select needs 'value'"))?;
            let target_js = element_from_request(with)?;
            let expression = format!(
                "(()=>{{const e={target_js}; if(!e||!e.tagName||e.tagName.toLowerCase()!=='select') return false; e.value={}; e.dispatchEvent(new Event('input',{{bubbles:true}})); e.dispatchEvent(new Event('change',{{bubbles:true}})); return true;}})()",
                js_escape(&value)
            );
            if evaluate(&endpoint, &id, &expression, timeout, false)?.as_bool() != Some(true) {
                bail!("browser select target was not found or is not a <select>");
            }
            out["target_id"] = json!(id);
            out["selected"] = value;
        }
        "scroll" => {
            let target = resolve_target(&endpoint, with, timeout, policy)?;
            let id = target_id(&target)?;
            out["ok"] = evaluate(&endpoint, &id, &scroll_js(with), timeout, false)?;
            out["target_id"] = json!(id);
        }
        "key" | "press" => {
            let target = resolve_target(&endpoint, with, timeout, policy)?;
            let id = target_id(&target)?;
            let key = with
                .get("key")
                .map(stringify)
                .ok_or_else(|| anyhow!("browser key needs 'key'"))?;
            let key_text = if key.len() == 1 { key.clone() } else { String::new() };
            cdp_page_sequence(
                &endpoint,
                &id,
                &[
                    ("Page.bringToFront", json!({})),
                    (
                        "Input.dispatchKeyEvent",
                        json!({"type": "keyDown", "key": key, "code": key, "text": key_text}),
                    ),
                    (
                        "Input.dispatchKeyEvent",
                        json!({"type": "keyUp", "key": key, "code": key, "text": ""}),
                    ),
                ],
                timeout,
            )?;
            out["target_id"] = json!(id);
            out["key"] = json!(key);
        }
        "wait_for" => {
            let target = resolve_target(&endpoint, with, timeout, policy)?;
            let id = target_id(&target)?;
            let selector = with
                .get("selector")
                .map(stringify)
                .ok_or_else(|| anyhow!("browser wait_for needs 'selector'"))?;
            let expression = format!(
                "(()=>{{const e=document.querySelector({}); if(!e) return false; const r=e.getBoundingClientRect(); return r.width>0&&r.height>0;}})()",
                js_escape(&Value::String(selector.clone()))
            );
            let deadline = Instant::now() + timeout;
            loop {
                if evaluate(&endpoint, &id, &expression, timeout.min(Duration::from_secs(2)), false)?.as_bool() == Some(true) {
                    out["target_id"] = json!(id);
                    out["ready"] = json!(true);
                    break;
                }
                if Instant::now() >= deadline {
                    bail!("browser wait_for timed out for {selector:?}");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        "close" => {
            let target = resolve_target(&endpoint, with, timeout, policy)?;
            let id = target_id(&target)?;
            let _ = http_close(&endpoint, &id, timeout);
            forget_owned_target(&id);
            out["target_id"] = json!(id);
            out["closed"] = json!(true);
        }
        "shutdown" => {
            let info = http_json(&endpoint, "GET", "/json/version", timeout)?;
            let ws = info
                .get("webSocketDebuggerUrl")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Chrome did not expose a browser websocket"))?;
            let mut socket = tungstenite::connect(ws)
                .map_err(|e| anyhow!("connect browser websocket failed: {e}"))?
                .0;
            set_ws_timeout(&mut socket, Duration::from_secs(3));
            socket.send(tungstenite::Message::Text(
                serde_json::to_string(&json!({"id":1,"method":"Browser.close","params":{}}))?,
            ))?;
            let _ = socket.read();
            *runtime_cell().lock().map_err(|_| anyhow!("browser runtime lock poisoned"))? = None;
            clear_owned_targets();
            out["shutting_down"] = json!(true);
        }
        "cleanup_tabs" => {
            let strings = |key: &str| -> Result<Vec<String>> {
                    with.get(key)
                        .and_then(Value::as_array)
                        .map(|values| {
                            values
                                .iter()
                                .map(|v| {
                                    v.as_str().map(str::to_string).ok_or_else(|| {
                                        anyhow!("browser cleanup_tabs {key} must contain strings")
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_else(|| Ok(Vec::new()))
            };
            let close_url_prefixes = strings("close_url_prefixes")?;
            let dedupe_urls = with
                .get("dedupe_urls")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if close_url_prefixes.is_empty() {
                if !dedupe_urls {
                    bail!("browser cleanup_tabs requires close_url_prefixes or explicit dedupe_urls=true");
                }
            }
            let keep_url_prefixes = strings("keep_url_prefixes")?;
            let keep_target_ids = strings("keep_target_ids")?;
            let limit = with
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(100)
                .clamp(1, 1_000) as usize;
            let before = list_targets(&endpoint, timeout)?;
            let pages_before = before
                .iter()
                .filter(|t| t.get("type").and_then(Value::as_str) == Some("page"))
                .count();
            let (selected, kept) = select_cleanup_targets(
                &before,
                &close_url_prefixes,
                &keep_url_prefixes,
                &keep_target_ids,
                limit,
                dedupe_urls,
            );
            let (closed_count, errors) = close_target_ids(&endpoint, &selected, timeout);
            let after = list_targets(&endpoint, timeout)?;
            let pages_after = after
                .iter()
                .filter(|t| t.get("type").and_then(Value::as_str) == Some("page"))
                .count();
            out["closed_count"] = json!(closed_count);
            out["closed"] = Value::Array(
                selected
                    .into_iter()
                    .map(Value::String)
                    .collect::<Vec<_>>(),
            );
            out["kept"] = Value::Array(kept);
            out["pages_before"] = json!(pages_before);
            out["pages_after"] = json!(pages_after);
            out["errors"] = Value::Array(errors);
            out["dedupe_urls"] = json!(dedupe_urls);
        }
        other => bail!(
            "browser op {other:?} unsupported (status|open|open_many|research|alphaxiv|save_article|cleanup_tabs|navigate|snapshot|highlight|agent_step|evaluate|click|type|select|scroll|key|wait_for|close|shutdown)"
        ),
    }

    out["capability"] = json!("chrome_cdp");
    Ok(out)
}

fn merge_result(mut envelope: Value, value: Value) -> Result<Value> {
    if !value.is_null() {
        envelope["result"] = value;
    }
    Ok(envelope)
}

/// Encode unsafe query bytes while preserving URL syntax accepted by DevTools.
fn urlencoding_utf8(v: &str) -> String {
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
    use crate::capability::Capability;

    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
        TEST_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn cap(endpoint: &str, launch: bool) -> BrowserCap {
        BrowserCap {
            endpoint: endpoint.to_string(),
            chrome_binary: String::new(),
            profile_dir: String::new(),
            extension_path: String::new(),
            launch,
            startup_timeout_ms: 1_000,
            timeout_ms: 1,
            max_text: 1_000,
            max_owned_pages: 32,
            owned_idle_ms: 60_000,
        }
    }

    #[test]
    fn parses_chrome_cdp_alias() {
        let registry = super::super::registry_from(
            &[(
                "browser",
                json!({"kind": "chrome_cdp", "endpoint": "http://127.0.0.1:9333"}),
            )],
            None,
        )
        .unwrap();
        assert!(matches!(
            registry.get("browser").unwrap(),
            Capability::Browser(_)
        ));
    }

    #[test]
    fn target_hosts_are_policy_gated() {
        let mut policy = Policy::default();
        policy.allow_hosts = vec!["example.com".to_string()];
        let allowed = json!({"id": "t", "type": "page", "url": "https://example.com/x"});
        let denied = json!({"id": "t", "type": "page", "url": "https://other.example/x"});
        assert!(target_allowed(&allowed, &policy).is_ok());
        assert!(target_allowed(&denied, &policy).is_err());
    }

    #[test]
    fn cleanup_selects_only_matching_disposable_pages() {
        let targets = vec![
            json!({"id": "worker", "type": "service_worker", "url": "http://127.0.0.1/x"}),
            json!({"id": "keep-id", "type": "page", "url": "http://127.0.0.1/x"}),
            json!({"id": "keep-url", "type": "page", "url": "http://127.0.0.1/keep"}),
            json!({"id": "protected", "type": "page", "url": "chrome://settings"}),
            json!({"id": "close-me", "type": "page", "url": "http://127.0.0.1/close"}),
            json!({"id": "other", "type": "page", "url": "https://example.com/page"}),
        ];
        let (closed, kept) = select_cleanup_targets(
            &targets,
            &["http://127.0.0.1".to_string()],
            &["http://127.0.0.1/keep".to_string()],
            &["keep-id".to_string()],
            10,
            false,
        );
        assert_eq!(closed, ["close-me"]);
        let kept_ids: Vec<_> = kept
            .iter()
            .filter_map(|x| x.get("target_id").and_then(Value::as_str))
            .collect();
        assert_eq!(kept_ids, ["keep-id", "keep-url", "protected", "other"]);
    }

    #[test]
    fn cleanup_can_explicitly_dedupe_pages() {
        let targets = vec![
            json!({"id": "first", "type": "page", "url": "https://example.com/a"}),
            json!({"id": "second", "type": "page", "url": "https://example.com/a"}),
            json!({"id": "worker", "type": "service_worker", "url": "https://example.com/a"}),
        ];
        let (closed, kept) = select_cleanup_targets(&targets, &[], &[], &[], 10, true);
        assert_eq!(closed, ["second"]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["target_id"], "first");
        assert_eq!(kept[0]["kept"], "first_url");
    }

    #[test]
    fn runtime_is_bound_to_one_endpoint() {
        let _guard = test_guard();
        {
            let mut guard = runtime_cell().lock().unwrap();
            *guard = Some(BrowserRuntime {
                endpoint: "http://127.0.0.1:9222".to_string(),
                owner_dir: std::env::temp_dir(),
                _lock: None,
            });
        }
        let mut policy = Policy::default();
        policy.allow_hosts = vec!["127.0.0.1".to_string()];
        let err = ensure_runtime(
            &cap("http://127.0.0.1:9333", false),
            &json!({}),
            &json!({}),
            &policy,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("already bound"), "unexpected error: {err}");
        *runtime_cell().lock().unwrap() = None;
    }

    #[test]
    fn launch_requires_explicit_exec_policy() {
        let _guard = test_guard();
        let mut policy = Policy::default();
        policy.allow_hosts = vec!["127.0.0.1".to_string()];
        let err = ensure_runtime(
            &cap("http://127.0.0.1:1", true),
            &json!({}),
            &json!({}),
            &policy,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("allow_exec"), "unexpected error: {err}");
    }

    #[test]
    fn scores_relevant_structured_page_as_acceptable() {
        let page = json!({
            "title": "Rust async runtime production guide",
            "text": "Rust async runtime production guide. Tokio tasks, cancellation, backpressure and observability are repeated here.",
            "host": "docs.example.com",
            "text_length": 2400,
            "headings": 6,
            "paragraphs": 12,
            "code_blocks": 2,
            "links": 18,
            "published_hint": "2025-01"
        });
        let (score, relevance, effectiveness, _) = score_research_page(
            "Rust async production",
            &page,
            "https://docs.example.com/rust-async-production",
        );
        assert!(relevance > 0.45);
        assert!(effectiveness > 0.4);
        assert!(
            score > 0.52,
            "score={score}, relevance={relevance}, effectiveness={effectiveness}"
        );
    }

    #[test]
    fn classifies_content_and_relevance_for_research_page() {
        let candidate =
            json!({"title": "Jev AI documentation", "url": "https://example.com/docs/jev"});
        let page = json!({
            "text": "Jev AI documentation. This guide explains configuration, API usage, and examples.",
            "host": "docs.example.com",
            "code_blocks": 2
        });
        let classification = classify_research_page(
            "jev ai",
            &candidate,
            &page,
            "https://docs.example.com/docs/jev",
            0.76,
            0.82,
        );
        assert_eq!(classification["content_type"], json!("documentation"));
        assert_eq!(classification["relevance_class"], json!("highly_relevant"));
        assert_eq!(classification["topic_labels"], json!(["jev", "ai"]));
        assert!(classification["confidence"].as_f64().unwrap() > 0.7);
    }

    #[test]
    fn rejects_missing_or_null_research_query() {
        let policy = Policy::default();
        let err = run_research(
            &cap("http://127.0.0.1:9222", false),
            "http://127.0.0.1:9222",
            &json!({"query": Value::Null}),
            Duration::from_millis(1),
            &policy,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("non-empty 'query'"), "unexpected error: {err}");
    }

    #[test]
    fn tab_concurrency_defaults_to_five_and_clamps() {
        assert_eq!(tab_concurrency_of(&json!({})), 5);
        assert_eq!(tab_concurrency_of(&json!({"tab_concurrency": 1})), 1);
        assert_eq!(tab_concurrency_of(&json!({"tab_concurrency": 8})), 8);
        assert_eq!(tab_concurrency_of(&json!({"tab_concurrency": 0})), 1);
        assert_eq!(tab_concurrency_of(&json!({"tab_concurrency": 99})), 10);
        // `concurrency` is an accepted alias.
        assert_eq!(tab_concurrency_of(&json!({"concurrency": 3})), 3);
    }

    #[test]
    fn classifies_urls_before_opening_tabs() {
        // Known video hosts (with and without subdomains / scheme).
        assert_eq!(
            url_content_kind("https://www.youtube.com/watch?v=abc123"),
            "video"
        );
        assert_eq!(
            url_content_kind("https://m.youtube.com/watch?v=abc"),
            "video"
        );
        assert_eq!(url_content_kind("https://youtu.be/abc123"), "video");
        assert_eq!(
            url_content_kind("https://www.bilibili.com/video/BV1xx411c7mD"),
            "video"
        );
        assert_eq!(
            url_content_kind("https://www.douyin.com/user/self?modal_id=1"),
            "video"
        );
        // Direct media files, including query strings.
        assert_eq!(
            url_content_kind("https://cdn.example.com/clip.mp4"),
            "media"
        );
        assert_eq!(
            url_content_kind("https://cdn.example.com/clip.MP4?token=xyz"),
            "media"
        );
        assert_eq!(
            url_content_kind("https://example.com/audio.opus#t=3"),
            "media"
        );
        // Ordinary pages (a video-like path on a non-video host is still a page).
        assert_eq!(url_content_kind("https://docs.example.com/guide"), "page");
        assert_eq!(
            url_content_kind("https://developer.mozilla.org/en-US/docs/Web/API"),
            "page"
        );
        assert_eq!(url_content_kind("https://example.com/watch?v=1"), "page");
    }

    #[test]
    fn decodes_query_params_and_google_url_wrapper() {
        assert_eq!(percent_decode("%E5%BC%A0hello%20world"), "张hello world");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(
            query_param(
                "https://x/?a=1&q=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3D1",
                "q"
            ),
            Some("https://www.youtube.com/watch?v=1".to_string())
        );
        assert_eq!(query_param("https://x/?a=1", "q"), None);
        // The decodable `/url?q=` form needs no network and reveals the host.
        let resolved = resolve_destination_url(
            "https://www.google.com/url?q=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3Dabc&sa=U",
        );
        assert_eq!(resolved, "https://www.youtube.com/watch?v=abc");
        assert_eq!(url_content_kind(&resolved), "video");
        // Non-Google URLs pass through untouched (no network).
        assert_eq!(
            resolve_destination_url("https://docs.example.com/guide"),
            "https://docs.example.com/guide"
        );
    }

    #[test]
    fn derives_filesystem_safe_slugs() {
        assert_eq!(
            derive_slug(
                "https://www.alphaxiv.org/abs/2609.recurrent-looped-transformer",
                "x"
            ),
            "2609.recurrent-looped-transformer"
        );
        assert_eq!(
            derive_slug("https://example.com/", "Recurrent Looped Transformer!"),
            "recurrent-looped-transformer"
        );
        assert_eq!(sanitize_slug("hello / world  // x"), "hello-world-x");
        assert_eq!(sanitize_slug("!!!"), "");
    }

    #[test]
    fn save_article_out_dir_is_policy_gated() {
        // No allow_paths ⇒ file writes are fail-closed.
        let strict = Policy::default();
        assert!(allowed_out_dir(&strict, "/tmp/whatever").is_err());

        let root = std::env::temp_dir().join("laya-save-article-test");
        std::fs::create_dir_all(&root).unwrap();
        let mut policy = Policy::default();
        policy.allow_paths = vec![root.display().to_string()];
        // Inside the allowed root (even before it exists) is accepted.
        let ok = allowed_out_dir(&policy, &root.join("paper/out").display().to_string());
        assert!(ok.is_ok(), "expected inside-root to be allowed: {ok:?}");
        // A sibling path is rejected.
        assert!(allowed_out_dir(&policy, "/etc/passwd").is_err());
    }

    #[test]
    fn skip_kinds_default_and_overrides() {
        // Default: drop video and direct media before opening a target.
        assert_eq!(
            skip_kinds_of(&json!({})),
            vec!["video".to_string(), "media".to_string()]
        );
        assert_eq!(
            skip_kinds_of(&json!({"skip_video": true})),
            vec!["video".to_string(), "media".to_string()]
        );
        // Explicit opt-out opens everything again.
        assert_eq!(
            skip_kinds_of(&json!({"skip_video": false})),
            Vec::<String>::new()
        );
        // An explicit list replaces the derived default.
        assert_eq!(
            skip_kinds_of(&json!({"skip_kinds": ["media"]})),
            vec!["media".to_string()]
        );
        assert_eq!(
            skip_kinds_of(&json!({"skip_video": false, "skip_kinds": ["video"]})),
            vec!["video".to_string()]
        );
        assert_eq!(
            skip_kinds_of(&json!({"skip_kinds": []})),
            Vec::<String>::new()
        );
    }
    #[test]
    fn alphaxiv_plan_infers_mode_from_natural_language() {
        // Bare search phrase → search, save the top paper.
        let p = alphaxiv_plan(&json!({"query": "llm memory"})).unwrap();
        assert_eq!(p["mode"], json!("search"));
        assert_eq!(p["count"], json!(1));
        assert!(p["target_url"].is_null());

        // Trending keywords (english + chinese) → the explore feed.
        for q in [
            "trending",
            "Trending Papers",
            "explore",
            "热门",
            "最新的论文",
        ] {
            let p = alphaxiv_plan(&json!({"query": q})).unwrap();
            assert_eq!(p["mode"], json!("trending"), "query {q:?}");
            assert_eq!(p["count"], json!(10), "query {q:?}");
        }

        // A paper URL in the query → direct save, no listing.
        let url = "https://www.alphaxiv.org/abs/2609.recurrent-looped-transformer";
        let p = alphaxiv_plan(&json!({"query": url})).unwrap();
        assert_eq!(p["mode"], json!("url"));
        assert_eq!(p["target_url"], json!(url));

        // `paper_url` wins even when the query reads like a keyword.
        let p = alphaxiv_plan(&json!({"query": "whatever", "paper_url": url})).unwrap();
        assert_eq!(p["mode"], json!("url"));
        assert_eq!(p["target_url"], json!(url));

        // Explicit mode + count.
        let p =
            alphaxiv_plan(&json!({"query": "diffusion", "mode": "trending", "count": 3})).unwrap();
        assert_eq!(p["mode"], json!("trending"));
        assert_eq!(p["count"], json!(3));

        // Trending pages through the feed API, so its count can reach hundreds…
        let p = alphaxiv_plan(&json!({"query": "trending", "count": 100})).unwrap();
        assert_eq!(p["count"], json!(100));
        // …but never unbounded.
        let p = alphaxiv_plan(&json!({"query": "trending", "count": 100_000})).unwrap();
        assert_eq!(p["count"], json!(500));

        // Search still has only one rendered page of cards.
        let p =
            alphaxiv_plan(&json!({"query": "diffusion", "mode": "search", "count": 99})).unwrap();
        assert_eq!(p["mode"], json!("search"));
        assert_eq!(p["count"], json!(10));
    }

    #[test]
    fn alphaxiv_interval_is_restricted() {
        for (given, want) in [
            (json!("7 Days"), "7 Days"),
            (json!("3 days"), "3 Days"),
            (json!("ALL TIME"), "All time"),
            (json!("90 Days"), "90 Days"),
        ] {
            let with = json!({"interval": given});
            assert_eq!(alphaxiv_interval(&with), want, "{given:?}");
        }
        // Missing or bogus values fall back to the default window.
        assert_eq!(alphaxiv_interval(&json!({})), "7 Days");
        assert_eq!(
            alphaxiv_interval(&json!({"interval": "yesterday"})),
            "7 Days"
        );
    }

    #[test]
    fn localizes_alphaxiv_abs_urls() {
        let u = "https://www.alphaxiv.org/abs/2609.recurrent-looped-transformer";
        assert_eq!(
            localize_alphaxiv_url(u, "zh"),
            "https://www.alphaxiv.org/zh/abs/2609.recurrent-looped-transformer"
        );
        // An existing locale prefix is replaced, never doubled.
        assert_eq!(
            localize_alphaxiv_url("https://www.alphaxiv.org/zh/abs/2609.x", "ja"),
            "https://www.alphaxiv.org/ja/abs/2609.x"
        );
        assert_eq!(
            localize_alphaxiv_url("https://www.alphaxiv.org/zh/abs/2609.x", "zh"),
            "https://www.alphaxiv.org/zh/abs/2609.x"
        );
        // en/off/empty opts out; query strings are dropped when rebuilding.
        assert_eq!(localize_alphaxiv_url(u, "en"), u);
        assert_eq!(localize_alphaxiv_url(u, ""), u);
        assert_eq!(
            localize_alphaxiv_url("https://www.alphaxiv.org/abs/2609.x?foo=1", "zh"),
            "https://www.alphaxiv.org/zh/abs/2609.x"
        );
        // Non-paper paths and other hosts are untouched.
        assert_eq!(
            localize_alphaxiv_url("https://www.alphaxiv.org/researchers", "zh"),
            "https://www.alphaxiv.org/researchers"
        );
        assert_eq!(
            localize_alphaxiv_url("https://arxiv.org/abs/2307.12307", "zh"),
            "https://arxiv.org/abs/2307.12307"
        );
    }

    #[test]
    fn alphaxiv_plan_rejects_empty_and_bad_inputs() {
        assert!(alphaxiv_plan(&json!({})).is_err());
        assert!(alphaxiv_plan(&json!({"query": "   "})).is_err());
        assert!(alphaxiv_plan(&json!({"query": "x", "mode": "bogus"})).is_err());
        // url mode demands a concrete URL.
        assert!(alphaxiv_plan(&json!({"mode": "url"})).is_err());
    }

    #[test]
    fn detects_paper_urls_and_trending_words() {
        assert!(looks_like_paper_url("https://www.alphaxiv.org/abs/2609.x"));
        assert!(looks_like_paper_url("https://arxiv.org/abs/2307.12307"));
        assert!(!looks_like_paper_url("recurrent looped transformer"));
        assert!(is_trending_request("trending"));
        assert!(is_trending_request("show me trending in llm"));
        assert!(is_trending_request("热门论文"));
        assert!(!is_trending_request("llm memory"));
        assert!(!is_trending_request("attention is all you need"));
    }

    #[test]
    fn parses_model_decision_json_and_code_fence() {
        let object = decision_object(&json!({"operation": "CLICK", "element": 3})).unwrap();
        assert_eq!(object["element"], json!(3));
        let fenced = decision_object(&json!("```json\n{\"operation\":\"DONE\"}\n```")).unwrap();
        assert_eq!(fenced["operation"], json!("DONE"));
    }
}

#[cfg(test)]
mod real_browser_tests {
    use super::*;
    use std::io::{Read, Write};

    fn chrome_binary() -> Option<String> {
        let candidates = [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ];
        candidates
            .iter()
            .find(|p| std::path::Path::new(p).is_file())
            .map(|p| p.to_string())
    }

    /// This test proves the requested invariant against real Chrome: one CDP
    /// instance, one unpacked extension, one localhost server, real CDP
    /// keystrokes/clicks, and two concurrent workers on separate targets. It is
    /// opt-in because CI does not install Chrome.
    #[test]
    #[ignore = "requires Google Chrome; run with --ignored"]
    fn real_chrome_singleton_observes_and_drives_page() {
        let Some(binary) = chrome_binary() else {
            panic!("real Chrome round-trip requires Google Chrome");
        };
        let cdp_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let cdp_port = cdp_listener.local_addr().unwrap().port();
        drop(cdp_listener);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            // Chrome loads the page plus favicon; keep enough accepts for the
            // sequential round-trip and the two concurrent workers below.
            let body = br#"<!doctype html><title>before</title><button id=set onclick="document.title='after'">Set</button><input id=message>"#;
            for _ in 0..12 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
                stream.write_all(body).unwrap();
            }
        });

        let profile = std::env::temp_dir().join(format!("laya-chrome-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&profile);
        let mut policy = Policy::default();
        policy.allow_exec = true;
        policy.allow_hosts = vec!["127.0.0.1".to_string()];
        policy.allow_paths = vec![profile.to_string_lossy().to_string()];
        let cap = BrowserCap {
            endpoint: format!("http://127.0.0.1:{cdp_port}"),
            chrome_binary: binary,
            profile_dir: profile.to_string_lossy().to_string(),
            extension_path: concat!(env!("CARGO_MANIFEST_DIR"), "/extensions/laya-browser")
                .to_string(),
            launch: true,
            startup_timeout_ms: 20_000,
            timeout_ms: 8_000,
            max_text: 6_000,
            max_owned_pages: 32,
            owned_idle_ms: 5 * 60_000,
        };
        // Ensure a concurrent unit-test did not leave a different singleton.
        *runtime_cell().lock().unwrap() = None;
        let result = (|| -> Result<Value> {
            let status = call_browser(&cap, &json!({"op":"status"}), &json!({}), &policy)?;
            println!("real browser: status ok");
            assert_eq!(status["op"], json!("status"));
            let opened = call_browser(
                &cap,
                &json!({"op":"open", "url": format!("http://127.0.0.1:{port}/")}),
                &json!({}),
                &policy,
            )?;
            println!("real browser: opened {:?}", opened["target_id"]);
            let target_id = opened["target_id"].as_str().unwrap().to_string();
            std::thread::sleep(Duration::from_millis(700));
            let snapshot = call_browser(
                &cap,
                &json!({"op":"snapshot", "target_id": target_id}),
                &json!({}),
                &policy,
            )?;
            assert_eq!(
                snapshot["result"]["url"],
                json!(format!("http://127.0.0.1:{port}/"))
            );
            let elements = snapshot["result"]["elements"].as_array().unwrap();
            assert!(elements.len() >= 2);
            // The fallback omits rect; its presence proves the bundled extension
            // content script produced this observation.
            assert!(elements[0].get("rect").and_then(Value::as_object).is_some());

            println!("real browser: type begin");
            let typed = call_browser(
                &cap,
                &json!({"op":"type", "target_id": target_id, "selector": "#message", "text": "hello"}),
                &json!({}),
                &policy,
            )?;
            println!("real browser: type ok");
            assert_eq!(typed["typed"], json!(5));
            let probe = call_browser(
                &cap,
                &json!({"op":"evaluate", "target_id": target_id, "expression": "window.__events=[]; ['mousedown','mouseup','click'].forEach(t=>{document.addEventListener(t,e=>window.__events.push(t+':'+(e.target.id||e.target.tagName)));}); [document.elementFromPoint(26,20.5).id, document.hasFocus(), typeof document.querySelector('#set').onclick]"}),
                &json!({}),
                &policy,
            )?;
            println!("real browser click probe: {}", probe["value"]);
            call_browser(
                &cap,
                &json!({"op":"click", "target_id": target_id, "selector": "#set"}),
                &json!({}),
                &policy,
            )?;
            std::thread::sleep(Duration::from_millis(500));
            println!("real browser: evaluate begin");
            let value = call_browser(
                &cap,
                &json!({"op":"evaluate", "target_id": target_id, "expression": "[document.title, document.querySelector('#message').value, window.__events]"}),
                &json!({}),
                &policy,
            )?;
            println!("real browser click debug: {}", value["value"]);
            assert_eq!(
                value["value"],
                json!([
                    "after",
                    "hello",
                    ["mousedown:set", "mouseup:set", "click:set"]
                ])
            );
            let concurrent_results: Vec<Result<()>> = std::thread::scope(|scope| {
                let mut handles = Vec::new();
                for worker in 0..2 {
                    let cap = &cap;
                    let policy = &policy;
                    let expected = format!("worker-{worker}");
                    let url = format!("http://127.0.0.1:{port}/");
                    handles.push(scope.spawn(move || -> Result<()> {
                        let opened = call_browser(
                            cap,
                            &json!({"op": "open", "url": url}),
                            &json!({}),
                            policy,
                        )?;
                        let target = opened["target_id"].as_str().unwrap().to_string();
                        std::thread::sleep(Duration::from_millis(700));
                        let snapshot = call_browser(
                            cap,
                            &json!({"op": "snapshot", "target_id": target}),
                            &json!({}),
                            policy,
                        )?;
                        assert_eq!(
                            snapshot["result"]["url"],
                            Value::String(url.clone())
                        );
                        call_browser(
                            cap,
                            &json!({"op": "type", "target_id": target, "selector": "#message", "text": expected}),
                            &json!({}),
                            policy,
                        )?;
                        call_browser(
                            cap,
                            &json!({"op": "click", "target_id": target, "selector": "#set"}),
                            &json!({}),
                            policy,
                        )?;
                        std::thread::sleep(Duration::from_millis(300));
                        let value = call_browser(
                            cap,
                            &json!({"op": "evaluate", "target_id": target, "expression": "[document.title, document.querySelector('#message').value]"}),
                            &json!({}),
                            policy,
                        )?;
                        assert_eq!(
                            value["value"],
                            json!(["after", expected]),
                            "concurrent worker output mismatch"
                        );
                        Ok(())
                    }));
                }
                handles
                    .into_iter()
                    .map(|handle| handle.join().unwrap())
                    .collect()
            });
            for result in concurrent_results {
                result?;
            }
            let targets = call_browser(&cap, &json!({"op":"targets"}), &json!({}), &policy)?;
            assert!(targets["targets"].as_array().unwrap().len() >= 3);
            call_browser(&cap, &json!({"op":"shutdown"}), &json!({}), &policy)
        })();
        *runtime_cell().lock().unwrap() = None;
        let _ = std::fs::remove_dir_all(&profile);
        result.unwrap();
    }
}
