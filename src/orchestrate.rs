//! Local resource orchestration — the Rust home of the old
//! `scripts/laya-ensure-chrome.sh` / `scripts/laya-ensure-server.py` helpers.
//!
//! These are the generic, repo-agnostic primitives a workflow reaches for
//! *before* it drives a browser: guarantee a browser backend is up (a CDP
//! Chrome today, selected with `--backend`), and guarantee (or stop) a
//! long-running local HTTP server. Both are **idempotent**
//! — "ensure" does nothing when the resource is already up — so a spec can call
//! them on every run.
//!
//! Only `127.0.0.1` is touched; the sole privileged operation is spawning a
//! local process, which is the caller's (`policy.allow_exec`) decision, not
//! ours. State lives under `/tmp` exactly where the old scripts kept it, so
//! mixed old/new invocations still interoperate:
//!
//!   * Chrome:  profile dir `/tmp/laya-chrome-cdp-profile`
//!   * server:  `/tmp/laya-ensure-server-<port>.{pid,base,log}`

use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Default CDP port (`LAYA_CDP_PORT` overrides).
pub const DEFAULT_CDP_PORT: u16 = 9222;
/// Default server port (the old script's `DEFAULT_PORT`).
pub const DEFAULT_SERVER_PORT: u16 = 18766;
/// Default liveness path for the server probe.
pub const DEFAULT_HEALTH_PATH: &str = "/healthz";
/// Default isolated Chrome profile for the CDP endpoint.
pub const DEFAULT_CDP_PROFILE: &str = "/tmp/laya-chrome-cdp-profile";
/// How long a freshly spawned server gets to start answering.
const SPAWN_WAIT: Duration = Duration::from_secs(60);

// ──────────────────────────── browser backends ────────────────────────────

/// Which local browser-automation backend `browser ensure` should bring up.
///
/// Only [`Chrome`](BrowserBackend::Chrome) — a CDP endpoint over Chrome's
/// remote debugging — exists today. The selector is the whole point: a *better*
/// backend can be added later without changing the `browser ensure` call shape a
/// workflow (or a spec capability) already uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum BrowserBackend {
    /// Google Chrome / Chromium exposing a CDP endpoint.
    Chrome,
}

impl BrowserBackend {
    /// Stable lowercase name — exactly the `--backend` value.
    pub fn as_str(self) -> &'static str {
        match self {
            BrowserBackend::Chrome => "chrome",
        }
    }

    /// The port this backend listens on when none is given.
    pub fn default_port(self) -> u16 {
        match self {
            BrowserBackend::Chrome => DEFAULT_CDP_PORT,
        }
    }

    /// The env var this backend reads for a port override, if any.
    fn port_env(self) -> &'static str {
        match self {
            BrowserBackend::Chrome => "LAYA_CDP_PORT",
        }
    }
}

/// Inputs for [`ensure_browser`]: the backend selector plus the per-backend
/// knobs. `None` fields fall back to the env var the old shell script read
/// (`LAYA_CDP_PORT` / `CHROME_BIN` / `LAYA_CDP_PROFILE`).
#[derive(Clone, Debug)]
pub struct BrowserEnsureRequest {
    pub backend: BrowserBackend,
    pub port: Option<u16>,
    pub chrome_bin: Option<String>,
    pub profile: Option<String>,
}

impl BrowserEnsureRequest {
    /// Resolve the request against the environment exactly like the script did.
    pub fn from_env(
        backend: BrowserBackend,
        port: Option<u16>,
        chrome_bin: Option<String>,
        profile: Option<String>,
    ) -> Self {
        let port = port
            .or_else(|| {
                std::env::var(backend.port_env())
                    .ok()
                    .and_then(|v| v.parse().ok())
            })
            .or(Some(backend.default_port()));
        let chrome_bin = chrome_bin
            .or_else(|| nonempty(std::env::var("CHROME_BIN").ok()))
            .or_else(|| nonempty(std::env::var("LAYA_CHROME_BINARY").ok()));
        let profile = profile.or_else(|| nonempty(std::env::var("LAYA_CDP_PROFILE").ok()));
        Self {
            backend,
            port,
            chrome_bin,
            profile,
        }
    }

    fn port(&self) -> u16 {
        self.port.unwrap_or_else(|| self.backend.default_port())
    }

    fn profile(&self) -> String {
        self.profile
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_CDP_PROFILE.to_string())
    }

    fn chrome_bin(&self) -> String {
        self.chrome_bin
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(crate::capability::browser::default_chrome_binary)
    }
}

fn nonempty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.is_empty())
}

/// Ensure the request's browser backend is up and reachable on
/// `127.0.0.1:<port>`. Idempotent: prints an `ok … already listening` line and
/// returns `true` when it is already up.
///
/// Returns `Ok(false)` when it could not be brought up (the failure message is
/// already on stderr), matching the old script's exit code 1.
pub fn ensure_browser(req: &BrowserEnsureRequest) -> Result<bool> {
    match req.backend {
        BrowserBackend::Chrome => ensure_chrome(req),
    }
}

/// The `chrome` backend: launch (or confirm) a CDP Chrome on an isolated profile.
fn ensure_chrome(req: &BrowserEnsureRequest) -> Result<bool> {
    let port = req.port();
    if tcp_up(port, Duration::from_millis(300)) {
        println!("ok Chrome CDP already listening on 127.0.0.1:{port}");
        return Ok(true);
    }

    let chrome_bin = req.chrome_bin();
    let Some(chrome) = resolve_in_path(&chrome_bin) else {
        eprintln!("fail Google Chrome not found at {chrome_bin} (set CHROME_BIN)");
        return Ok(false);
    };

    let profile = req.profile();
    std::fs::create_dir_all(&profile)
        .with_context(|| format!("create Chrome profile dir {profile:?} failed"))?;

    let mut cmd = Command::new(&chrome);
    cmd.arg(format!("--remote-debugging-port={port}"))
        .arg(format!("--user-data-dir={profile}"))
        .args([
            "--no-first-run",
            "--no-default-browser-check",
            "--no-service-autorun",
            "about:blank",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = cmd
        .spawn()
        .with_context(|| format!("spawn Chrome {chrome:?} failed"))?;
    // Do not wait: the child is detached (own process group) and outlives us.
    drop(child);

    for _ in 0..30 {
        if tcp_up(port, Duration::from_millis(300)) {
            println!("ok Chrome CDP up on 127.0.0.1:{port} (profile {profile})");
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    eprintln!("fail Chrome CDP did not come up on :{port}");
    Ok(false)
}

// ───────────────────────── local HTTP server ─────────────────────────

fn server_paths(port: u16) -> (PathBuf, PathBuf, PathBuf) {
    (
        PathBuf::from(format!("/tmp/laya-ensure-server-{port}.pid")),
        PathBuf::from(format!("/tmp/laya-ensure-server-{port}.base")),
        PathBuf::from(format!("/tmp/laya-ensure-server-{port}.log")),
    )
}

/// The healthy base URL for `port`, if any (state from the `/tmp` pid/base
/// files). `None` = nothing healthy is answering.
fn healthy_base(port: u16, health_path: &str) -> Option<String> {
    let (_, basef, _) = server_paths(port);
    let raw = std::fs::read_to_string(&basef).ok()?;
    let base = raw.trim().to_string();
    (!base.is_empty() && probe_http(&base, health_path)).then_some(base)
}

fn print_running(port: u16, base: &str) {
    let (pidf, _, _) = server_paths(port);
    let pid = read_pid(&pidf)
        .map(|p| p.to_string())
        .unwrap_or_else(|| "?".to_string());
    println!("[laya-ensure-server] RUNNING  {base}  (pid {pid})");
}

/// Print `RUNNING`/`STOPPED` and return whether a healthy server answers on
/// `port` (state read from the `/tmp` pid/base files).
pub fn server_status(port: u16, health_path: &str) -> bool {
    match healthy_base(port, health_path) {
        Some(base) => {
            print_running(port, &base);
            true
        }
        None => {
            println!("[laya-ensure-server] STOPPED  (no healthy server on :{port})");
            false
        }
    }
}

/// Ensure a healthy server on `port`; when nothing answers, daemonize `command`
/// and wait for it. Prints `BASE=<url>` on success so a workflow can capture it.
pub fn server_ensure(port: u16, command: Option<&str>, health_path: &str) -> Result<bool> {
    if let Some(base) = healthy_base(port, health_path) {
        print_running(port, &base);
        println!("BASE={base}");
        return Ok(true);
    }
    match command.map(str::trim).filter(|c| !c.is_empty()) {
        Some(cmd) => server_start_daemon(port, cmd, health_path),
        None => {
            eprintln!(
                "[laya-ensure-server] ensure needs --command to spawn when nothing is running"
            );
            Ok(false)
        }
    }
}

/// Start `command` in the foreground as the server, keep this process alive
/// until SIGINT/SIGTERM, then terminate the child and clean the state files.
pub fn server_start_foreground(port: u16, command: &str, health_path: &str) -> Result<bool> {
    if command.trim().is_empty() {
        bail!("server start needs a non-empty --command");
    }
    let (pidf, basef, logf) = server_paths(port);
    let base = format!("http://127.0.0.1:{port}");

    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&logf)
        .with_context(|| format!("open server log {logf:?} failed"))?;
    let log_err = log.try_clone().context("clone server log handle failed")?;

    let mut cmd = shell_command(command);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn server command failed (see {logf:?})"))?;

    std::fs::write(&pidf, std::process::id().to_string())?;
    std::fs::write(&basef, &base)?;
    stop::install();

    let deadline = Instant::now() + SPAWN_WAIT;
    loop {
        if probe_http(&base, health_path) {
            println!("BASE={base}");
            println!(
                "[laya-ensure-server] server up (pid {}) — log {}",
                std::process::id(),
                logf.display()
            );
            break;
        }
        if let Ok(Some(_)) = child.try_wait() {
            eprintln!(
                "[laya-ensure-server] command exited early — see {}",
                logf.display()
            );
            cleanup_state(&pidf, &basef);
            return Ok(false);
        }
        if Instant::now() >= deadline {
            eprintln!(
                "[laya-ensure-server] timed out waiting for server on :{port} — see {}",
                logf.display()
            );
            cleanup_state(&pidf, &basef);
            return Ok(false);
        }
        std::thread::sleep(Duration::from_secs(1));
    }

    // Keep-alive until a stop signal arrives.
    while !stop::requested() {
        std::thread::sleep(Duration::from_millis(200));
    }
    terminate_child(&mut child);
    cleanup_state(&pidf, &basef);
    Ok(true)
}

/// Detach `command` as a daemon (own session, log redirected) and wait until it
/// answers; prints `BASE=<url>` plus the daemon pid.
pub fn server_start_daemon(port: u16, command: &str, health_path: &str) -> Result<bool> {
    let (pidf, basef, logf) = server_paths(port);
    // Clear stale state from a crashed/stopped server.
    if pidf.exists() {
        match read_pid(&pidf) {
            Some(pid) if process_alive(pid) => {}
            _ => cleanup_state(&pidf, &basef),
        }
    }

    let exe = std::env::current_exe().context("resolve current executable failed")?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&logf)
        .with_context(|| format!("open server log {logf:?} failed"))?;
    let log_err = log.try_clone().context("clone server log handle failed")?;

    let mut cmd = Command::new(exe);
    cmd.args([
        "server",
        "start",
        "--port",
        &port.to_string(),
        "--command",
        command,
        "--health-path",
        health_path,
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::from(log))
    .stderr(Stdio::from(log_err));
    detach(&mut cmd);
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn server daemon failed (see {logf:?})"))?;
    let pid = child.id();

    let deadline = Instant::now() + SPAWN_WAIT;
    loop {
        if let Ok(b) = std::fs::read_to_string(&basef) {
            let b = b.trim();
            if !b.is_empty() && probe_http(b, health_path) {
                println!("BASE={b}");
                println!(
                    "[laya-ensure-server] server up (pid {pid}) — log {}",
                    logf.display()
                );
                return Ok(true);
            }
        }
        if let Ok(Some(_)) = child.try_wait() {
            eprintln!(
                "[laya-ensure-server] daemon child exited early — see {}",
                logf.display()
            );
            return Ok(false);
        }
        if Instant::now() >= deadline {
            eprintln!(
                "[laya-ensure-server] timed out waiting for server — see {}",
                logf.display()
            );
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Terminate a daemon-started server (SIGTERM + wait + clean state files).
pub fn server_stop(port: u16) -> Result<bool> {
    let (pidf, basef, _) = server_paths(port);
    if !pidf.exists() {
        println!("[laya-ensure-server] no server on :{port} (pid file absent)");
        return Ok(true);
    }
    let pid = read_pid(&pidf).unwrap_or(-1);
    if pid > 0 {
        terminate_pid(pid);
        let deadline = Instant::now() + Duration::from_secs(15);
        while process_alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    cleanup_state(&pidf, &basef);
    println!("[laya-ensure-server] stopped server pid {pid}");
    Ok(true)
}

fn cleanup_state(pidf: &Path, basef: &Path) {
    let _ = std::fs::remove_file(pidf);
    let _ = std::fs::remove_file(basef);
}

fn read_pid(pidf: &Path) -> Option<i32> {
    std::fs::read_to_string(pidf).ok()?.trim().parse().ok()
}

// ───────────────────────── process / net helpers ─────────────────────────

/// A shell command string, run through `sh -c` (unix) / `cmd /C` (windows) so
/// pipes and env prefixes in `--command` behave as the docs promise.
fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(command);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    }
}

/// Put the child in its own session so it survives our exit and the terminal.
fn detach(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid` is async-signal-safe and the closure does no
        // allocation — the documented constraint on `pre_exec`.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    {
        let _ = cmd;
    }
}

fn terminate_child(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn terminate_pid(pid: i32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}

fn process_alive(pid: i32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Resolve an executable: an explicit path is checked as-is; a bare name is
/// searched on `PATH`. Returns `None` when nothing executable was found.
fn resolve_in_path(bin: &str) -> Option<PathBuf> {
    let p = PathBuf::from(bin);
    if bin.contains('/') || bin.contains('\\') {
        return is_executable(&p).then_some(p);
    }
    if is_executable(&p) {
        return Some(p);
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(bin);
        if is_executable(&cand) {
            return Some(cand);
        }
    }
    None
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(p) {
            Ok(m) => m.is_file() && m.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

fn tcp_up(port: u16, timeout: Duration) -> bool {
    connect_tcp("127.0.0.1", port, timeout).is_some()
}

fn connect_tcp(host: &str, port: u16, timeout: Duration) -> Option<TcpStream> {
    let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs().ok()?.collect();
    for addr in addrs {
        if let Ok(s) = TcpStream::connect_timeout(&addr, timeout) {
            return Some(s);
        }
    }
    None
}

/// Liveness probe: any HTTP response (2xx–5xx) counts as up; only a refused /
/// timed-out connection means down. Matches the script's semantics.
fn probe_http(base: &str, health_path: &str) -> bool {
    let Some((host, port)) = split_base(base) else {
        return false;
    };
    let Some(mut stream) = connect_tcp(&host, port, Duration::from_secs(2)) else {
        return false;
    };
    let path = if health_path.is_empty() {
        "/"
    } else {
        health_path
    };
    let req = format!("GET {path} HTTP/1.0\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 32];
    match stream.read(&mut buf) {
        Ok(n) if n >= 5 => buf[..n].starts_with(b"HTTP/"),
        _ => false,
    }
}

/// Split `http://host[:port]` (or a bare `host:port`) into `(host, port)`.
fn split_base(base: &str) -> Option<(String, u16)> {
    let s = base.trim();
    if s.is_empty() {
        return None;
    }
    let rest = s
        .strip_prefix("http://")
        .or_else(|| s.strip_prefix("https://"))
        .unwrap_or(s);
    let authority = rest.split('/').next().unwrap_or(rest);
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => (h.to_string(), p.parse::<u16>().ok()?),
        _ => (authority.to_string(), 80),
    };
    // A host with whitespace can never be a real endpoint; reject it before we
    // hand it to the resolver (which may resolve anything on a search domain).
    if host.is_empty() || host.chars().any(char::is_whitespace) {
        return None;
    }
    Some((host, port))
}

/// SIGTERM/SIGINT latch used by the foreground keep-alive loop.
mod stop {
    #[cfg(unix)]
    use std::sync::atomic::{AtomicBool, Ordering};

    #[cfg(unix)]
    static STOP: AtomicBool = AtomicBool::new(false);

    #[cfg(unix)]
    extern "C" fn handler(_sig: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
    }

    #[cfg(unix)]
    pub fn install() {
        unsafe {
            libc::signal(libc::SIGTERM, handler as *const () as libc::sighandler_t);
            libc::signal(libc::SIGINT, handler as *const () as libc::sighandler_t);
        }
    }

    #[cfg(unix)]
    pub fn requested() -> bool {
        STOP.load(Ordering::SeqCst)
    }

    #[cfg(not(unix))]
    pub fn install() {}

    #[cfg(not(unix))]
    pub fn requested() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_paths_are_per_port() {
        let (pid, base, log) = server_paths(18766);
        assert_eq!(pid, PathBuf::from("/tmp/laya-ensure-server-18766.pid"));
        assert_eq!(base, PathBuf::from("/tmp/laya-ensure-server-18766.base"));
        assert_eq!(log, PathBuf::from("/tmp/laya-ensure-server-18766.log"));
        let (pid2, _, _) = server_paths(1);
        assert_ne!(pid, pid2);
    }

    #[test]
    fn split_base_parses_urls() {
        assert_eq!(
            split_base("http://127.0.0.1:18766"),
            Some(("127.0.0.1".to_string(), 18766))
        );
        assert_eq!(
            split_base("http://127.0.0.1:18766/"),
            Some(("127.0.0.1".to_string(), 18766))
        );
        assert_eq!(split_base("localhost"), Some(("localhost".to_string(), 80)));
        assert_eq!(
            split_base("https://example.com:443/x"),
            Some(("example.com".to_string(), 443))
        );
        assert_eq!(split_base("   "), None);
        assert_eq!(split_base(""), None);
    }

    #[test]
    fn browser_request_defaults_and_env_override() {
        // Pure defaults, independent of ambient env.
        let req = BrowserEnsureRequest {
            backend: BrowserBackend::Chrome,
            port: None,
            chrome_bin: Some("/bin/sh".to_string()),
            profile: Some("/tmp/x".to_string()),
        };
        assert_eq!(req.port(), DEFAULT_CDP_PORT);
        assert_eq!(req.backend.default_port(), DEFAULT_CDP_PORT);
        assert_eq!(req.backend.as_str(), "chrome");
        assert_eq!(req.profile(), "/tmp/x");
        assert_eq!(req.chrome_bin(), "/bin/sh");
    }

    #[test]
    fn resolve_in_path_finds_shell() {
        // `sh` exists on every unix dev box / CI image we run on.
        #[cfg(unix)]
        {
            let found = resolve_in_path("sh").expect("sh on PATH");
            assert!(is_executable(&found));
            assert!(resolve_in_path("/definitely/not/here").is_none());
        }
    }

    #[test]
    fn browser_ensure_is_idempotent_when_something_listens() {
        // Bind an ephemeral port, then ask `ensure_browser` to "ensure" it. It
        // must notice the listener and return true without spawning a browser.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let req = BrowserEnsureRequest {
            backend: BrowserBackend::Chrome,
            port: Some(port),
            chrome_bin: Some("/nonexistent/chrome".to_string()),
            profile: None,
        };
        assert!(ensure_browser(&req).expect("ensure ok"));
    }

    #[test]
    fn server_stop_without_pid_file_is_ok() {
        // Pick a port with no state files; stop must be a no-op success.
        let port = 59991;
        let (pidf, basef, _) = server_paths(port);
        let _ = std::fs::remove_file(&pidf);
        let _ = std::fs::remove_file(&basef);
        assert!(server_stop(port).expect("stop ok"));
    }

    #[test]
    fn probe_http_reports_down_for_dead_port() {
        // Nothing listens here → down. (Ports in the ephemeral range that we just
        // released are a good "definitely nothing" bet.)
        assert!(!probe_http("http://127.0.0.1:59992", "/"));
        assert!(!probe_http("not a url", "/"));
    }
}
