//! The one place that answers "where does laya-workflow keep its own files?".
//!
//! Everything this CLI persists for the user — installed plugins, per-user DSL,
//! the Chrome profile it launches, the laya-mem SQLite store — used to be spread
//! across three different conventions: `$XDG_CONFIG_HOME/laya-workflow`,
//! `~/.config/laya-workflow`, and `~/tmp/laya_mem`. Three roots for one tool's
//! state is three places to look when something is missing and three to migrate
//! when the layout changes.
//!
//! They now share one root, [`state_dir`]: `~/.laya-workflow` (or `$LAYA_HOME`).
//! It sits next to the existing `~/.laya-workflow/chrome` browser profile, so the
//! tool's footprint is a single dot-directory in `$HOME`.
//!
//! ```text
//! ~/.laya-workflow/
//!   chrome/          Chrome profile for chrome_cdp
//!   dsl/             per-user specs        (LAYA_USER_DSL_DIR)
//!   plugins/         installed plugins     (LAYA_USER_PLUGIN_DIR, `plugin install`)
//!   websites/        site-scoped plugins, sibling of plugins/
//!   laya-mem/
//!     codex.sqlite   memory store          (LAYA_MEM_SQLITE)
//!     specs/         the System-One specs  (LAYA_MEM_SPEC_DIR)
//! ```
//!
//! Precedence is always the same: an explicit per-purpose env var wins, then
//! `$LAYA_HOME`, then `$HOME/.laya-workflow`. Nothing here reads
//! `XDG_CONFIG_HOME` — a dot-directory in `$HOME` is deliberate, so the state
//! does not move when a user switches XDG roots, and it sits beside the
//! pre-existing `~/.laya-workflow/chrome` instead of splitting the tool across
//! two trees.

use std::path::PathBuf;

/// The tool's state root: `$LAYA_HOME` → `$HOME/.laya-workflow`.
///
/// `None` only when neither `$LAYA_HOME` nor `$HOME` is set, which is a broken
/// environment rather than a supported configuration.
pub fn state_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("LAYA_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(d));
    }
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(|h| PathBuf::from(h).join(".laya-workflow"))
}

/// [`state_dir`] with a fallback, for call sites that need a usable path even in
/// an environment without `$HOME`.
pub fn state_dir_or_cwd() -> PathBuf {
    state_dir().unwrap_or_else(|| PathBuf::from(".laya-workflow"))
}

/// Per-user spec root: `$LAYA_USER_DSL_DIR` → `<state>/dsl`.
pub fn user_dsl_dir() -> Option<PathBuf> {
    per_purpose("LAYA_USER_DSL_DIR").map(|d| d.join("dsl"))
}

/// Serializes tests that mutate process-wide env vars (`HOME`, `LAYA_HOME`,
/// `LAYA_USER_DSL_DIR`, `LAYA_USER_PLUGIN_DIR`, `LAYA_MEM_SQLITE`,
/// `LAYA_MEM_SPEC_DIR`). Parallel Rust test threads share one process, so any
/// two tests that both touch the same variable race — which is exactly how the
/// db-path tests started reading a `/tmp/laya-state-x` root set by a sibling
/// thread. Every test that rewrites one of these variables must hold this lock
/// (via [`Env`] or an explicit `.lock()`) for the whole mutation+read window.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Per-user plugin root: `$LAYA_USER_PLUGIN_DIR` → `<state>/plugins`.
///
/// This is also where `plugin install` writes.
pub fn user_plugin_dir() -> Option<PathBuf> {
    per_purpose("LAYA_USER_PLUGIN_DIR").map(|d| d.join("plugins"))
}

/// Site-scoped plugin root, the `websites/` sibling of the plugin root.
pub fn user_websites_dir() -> Option<PathBuf> {
    per_purpose("LAYA_USER_PLUGIN_DIR").map(|d| d.join("websites"))
}

/// Where laya-mem keeps its SQLite store: `$LAYA_MEM_SQLITE` →
/// `<state>/laya-mem/codex.sqlite`.
///
/// Nested under `laya-mem/` rather than flat in the state root so the gate's
/// specs and its store stay together as one unit. An earlier commit (a8f23e0)
/// put the store at `~/.laya-workflow/mem.sqlite` — same root, but a lone entry
/// next to `chrome/` and `plugins/`, with its specs elsewhere entirely.
pub fn laya_mem_db() -> PathBuf {
    match std::env::var_os("LAYA_MEM_SQLITE").filter(|v| !v.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => state_dir_or_cwd().join("laya-mem").join("codex.sqlite"),
    }
}

/// Where laya-mem keeps its System-One DSL specs: `$LAYA_MEM_SPEC_DIR` →
/// `<state>/laya-mem/specs`.
///
/// Only a *user* override lands here. The default an installed binary runs with
/// is its own compiled-in copy, because a binary installed to `/usr/local/bin`
/// has no source tree to read specs from — see
/// [`crate::laya_mem::LayaMemTools::default_spec_dir`].
pub fn laya_mem_spec_dir() -> Option<PathBuf> {
    match std::env::var_os("LAYA_MEM_SPEC_DIR").filter(|v| !v.is_empty()) {
        Some(p) => Some(PathBuf::from(p)),
        None => state_dir().map(|s| s.join("laya-mem").join("specs")),
    }
}

/// Chrome profile for `chrome_cdp`: `$LAYA_BROWSER_PROFILE` → `<state>/chrome`.
pub fn browser_profile_dir() -> Option<PathBuf> {
    per_purpose("LAYA_BROWSER_PROFILE").map(|d| d.join("chrome"))
}

/// A per-purpose override if set, else the state root.
fn per_purpose(var: &str) -> Option<PathBuf> {
    match std::env::var_os(var).filter(|v| !v.is_empty()) {
        Some(p) => Some(PathBuf::from(p)),
        None => state_dir(),
    }
}

/// Create `dir` (and parents). Idempotent, and never fails on an existing dir.
pub fn ensure_dir(dir: &std::path::Path) -> std::io::Result<()> {
    match std::fs::create_dir_all(dir) {
        Ok(()) => Ok(()),
        // A concurrent creator winning the race is success, not a failure.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && dir.is_dir() => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every env var these functions read, so a test can restore the real
    /// environment instead of leaking a fake `HOME` into the next test.
    const VARS: [&str; 4] = [
        "LAYA_HOME",
        "LAYA_USER_DSL_DIR",
        "LAYA_USER_PLUGIN_DIR",
        "LAYA_MEM_SQLITE",
    ];

    struct Env {
        saved: Vec<(String, Option<std::ffi::OsString>)>,
        /// Held for the struct's lifetime so sibling tests cannot interleave.
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Env {
        fn new() -> Self {
            // `into_inner` also recovers a poisoned lock: a panicking sibling
            // must not wedge every later test.
            let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let saved = VARS
                .iter()
                .map(|v| (v.to_string(), std::env::var_os(v)))
                .collect();
            for v in VARS {
                std::env::remove_var(v);
            }
            Env { saved, _lock }
        }
    }

    impl Drop for Env {
        fn drop(&mut self) {
            for (name, value) in std::mem::take(&mut self.saved) {
                match value {
                    Some(v) => std::env::set_var(&name, v),
                    None => std::env::remove_var(&name),
                }
            }
        }
    }

    #[test]
    fn home_is_the_default_root() {
        let _env = Env::new();
        std::env::set_var("HOME", "/tmp/laya-state-home");
        assert_eq!(
            state_dir(),
            Some(PathBuf::from("/tmp/laya-state-home/.laya-workflow"))
        );
        // The browser profile already lived here; the state roots join it rather
        // than creating a second dot-directory.
        assert_eq!(
            browser_profile_dir(),
            Some(PathBuf::from("/tmp/laya-state-home/.laya-workflow/chrome"))
        );
    }

    #[test]
    fn laya_home_overrides_home() {
        let _env = Env::new();
        std::env::set_var("HOME", "/tmp/laya-state-home");
        std::env::set_var("LAYA_HOME", "/tmp/laya-state-explicit");
        assert_eq!(state_dir(), Some(PathBuf::from("/tmp/laya-state-explicit")));
        assert_eq!(
            laya_mem_db(),
            PathBuf::from("/tmp/laya-state-explicit/laya-mem/codex.sqlite")
        );
    }

    #[test]
    fn per_purpose_vars_win_over_the_root() {
        let _env = Env::new();
        std::env::set_var("LAYA_HOME", "/tmp/laya-state-explicit");
        std::env::set_var("LAYA_USER_DSL_DIR", "/tmp/laya-dsl");
        std::env::set_var("LAYA_USER_PLUGIN_DIR", "/tmp/laya-plugins");
        std::env::set_var("LAYA_MEM_SQLITE", "/tmp/laya-mem.sqlite");
        assert_eq!(user_dsl_dir(), Some(PathBuf::from("/tmp/laya-dsl/dsl")));
        assert_eq!(user_plugin_dir(), Some(PathBuf::from("/tmp/laya-plugins/plugins")));
        // websites/ tracks the plugin root so an override moves both together.
        assert_eq!(user_websites_dir(), Some(PathBuf::from("/tmp/laya-plugins/websites")));
        assert_eq!(laya_mem_db(), PathBuf::from("/tmp/laya-mem.sqlite"));
    }

    /// An empty override is not an override: it must fall through to the root
    /// rather than producing a path like `/dsl`.
    #[test]
    fn empty_overrides_fall_through() {
        let _env = Env::new();
        std::env::set_var("LAYA_HOME", "/tmp/laya-state-explicit");
        std::env::set_var("LAYA_USER_DSL_DIR", "");
        std::env::set_var("LAYA_MEM_SQLITE", "");
        assert_eq!(user_dsl_dir(), Some(PathBuf::from("/tmp/laya-state-explicit/dsl")));
        assert_eq!(
            laya_mem_db(),
            PathBuf::from("/tmp/laya-state-explicit/laya-mem/codex.sqlite")
        );
    }

    /// No `$HOME` is a broken environment, not a config to guess at — except for
    /// the two helpers documented to produce a usable path regardless.
    #[test]
    fn no_home_degrades_predictably() {
        let _env = Env::new();
        let saved_home = std::env::var_os("HOME");
        std::env::remove_var("HOME");
        assert_eq!(state_dir(), None);
        assert_eq!(user_dsl_dir(), None);
        assert_eq!(state_dir_or_cwd(), PathBuf::from(".laya-workflow"));
        assert_eq!(
            laya_mem_db(),
            PathBuf::from(".laya-workflow/laya-mem/codex.sqlite")
        );
        if let Some(h) = saved_home {
            std::env::set_var("HOME", h);
        }
    }
}
