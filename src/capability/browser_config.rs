//! The browser profile's declared state, read from `<state>/browser.json`.
//!
//! A Chrome profile the engine launches is not just "a directory": it carries
//! the unpacked extensions and themes a workflow expects, plus any extra
//! Chrome arguments the host needs. Encoding that in the *capability spec*
//! means every spec repeats it, and nothing survives the process. Encoding it
//! here means the engine applies the same profile state on every launch (and
//! on every reuse, so edits take effect without a browser restart).
//!
//! Precedence is the same as [`crate::state::browser_profile_dir`]: an
//! explicit env var wins, then the shared state root. Missing file, missing
//! fields and malformed JSON are all non-fatal: profile config is a
//! convenience, not a precondition — the reason the whole module refuses to
//! fail is that a user with one typo should not lose their whole session.

use serde::Deserialize;
use std::path::PathBuf;

/// `$LAYA_BROWSER_CONFIG` → `<state>/browser.json`.
pub fn config_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LAYA_BROWSER_CONFIG").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    crate::state::state_dir().map(|d| d.join("browser.json"))
}

/// The declared profile state. Empty fields mean "nothing declared".
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BrowserProfileConfig {
    /// Unpacked extension / theme directories to load via CDP
    /// `Extensions.loadUnpacked` on every launch. Re-applied on reuse so
    /// edits take effect without a restart; idempotent because Chrome derives
    /// the same id for the same path.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Extra Chrome command-line arguments appended to the engine's own set.
    #[serde(default)]
    pub args: Vec<String>,
}

/// Read the profile config. A missing file is the empty config, not an error;
/// malformed JSON is a warning plus the empty config.
pub fn load() -> BrowserProfileConfig {
    let Some(path) = config_path() else {
        return BrowserProfileConfig::default();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return BrowserProfileConfig::default();
    };
    match serde_json::from_str::<BrowserProfileConfig>(&text) {
        Ok(config) => config,
        Err(e) => {
            eprintln!(
                "[browser] warning: could not parse {}: {e}; ignoring profile config",
                path.display()
            );
            BrowserProfileConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that mutate process-wide env vars must serialize: parallel Rust
    /// test threads share one process, so two `set_var("HOME", …)` racing make
    /// each other flaky. The sibling `state` module's `ENV_LOCK` guards that.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::state::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn explicit_env_var_wins_over_state_dir() {
        let _lock = env_lock();
        let saved = std::env::var_os("LAYA_BROWSER_CONFIG");
        std::env::set_var("LAYA_BROWSER_CONFIG", "/tmp/laya-test-browser-config.json");
        assert_eq!(
            config_path(),
            Some(PathBuf::from("/tmp/laya-test-browser-config.json"))
        );
        match saved {
            Some(v) => std::env::set_var("LAYA_BROWSER_CONFIG", v),
            None => std::env::remove_var("LAYA_BROWSER_CONFIG"),
        }
    }

    #[test]
    fn defaults_to_state_dir_browser_json() {
        let _lock = env_lock();
        let saved_home = std::env::var_os("HOME");
        let saved_laya_home = std::env::var_os("LAYA_HOME");
        let saved_cfg = std::env::var_os("LAYA_BROWSER_CONFIG");
        std::env::remove_var("LAYA_BROWSER_CONFIG");
        std::env::remove_var("LAYA_HOME");
        std::env::set_var("HOME", "/tmp/laya-state-home-test");
        assert_eq!(
            config_path(),
            Some(PathBuf::from(
                "/tmp/laya-state-home-test/.laya-workflow/browser.json"
            ))
        );
        match saved_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match saved_laya_home {
            Some(v) => std::env::set_var("LAYA_HOME", v),
            None => std::env::remove_var("LAYA_HOME"),
        }
        match saved_cfg {
            Some(v) => std::env::set_var("LAYA_BROWSER_CONFIG", v),
            None => std::env::remove_var("LAYA_BROWSER_CONFIG"),
        }
    }

    #[test]
    fn load_returns_default_when_file_missing() {
        // Point at a path that cannot exist so the test is hermetic even on a
        // machine that has a real ~/.laya-workflow/browser.json.
        let _lock = env_lock();
        let saved = std::env::var_os("LAYA_BROWSER_CONFIG");
        std::env::set_var("LAYA_BROWSER_CONFIG", "/nonexistent/laya/browser.json");
        assert_eq!(load(), BrowserProfileConfig::default());
        match saved {
            Some(v) => std::env::set_var("LAYA_BROWSER_CONFIG", v),
            None => std::env::remove_var("LAYA_BROWSER_CONFIG"),
        }
    }

    #[test]
    fn load_parses_extensions_and_args() {
        let dir = std::env::temp_dir().join("laya-browser-cfg-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("browser.json");
        std::fs::write(
            &path,
            r#"{"extensions":["~/ext/a","${env.HOME}/ext/b"],"args":["--window-size=1280,900"]}"#,
        )
        .unwrap();
        let saved = std::env::var_os("LAYA_BROWSER_CONFIG");
        std::env::set_var("LAYA_BROWSER_CONFIG", &path);
        let cfg = load();
        assert_eq!(cfg.extensions, vec!["~/ext/a", "${env.HOME}/ext/b"]);
        assert_eq!(cfg.args, vec!["--window-size=1280,900"]);
        match saved {
            Some(v) => std::env::set_var("LAYA_BROWSER_CONFIG", v),
            None => std::env::remove_var("LAYA_BROWSER_CONFIG"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_field_fails_parsing_to_a_warning() {
        // deny_unknown_fields turns a typo into a visible warning instead of
        // a silently dropped entry, so the whole config is rejected rather
        // than half-applied.
        let dir = std::env::temp_dir().join("laya-browser-cfg-bad");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("browser.json");
        std::fs::write(&path, r#"{"extensons":["/x"]}"#).unwrap();
        let saved = std::env::var_os("LAYA_BROWSER_CONFIG");
        std::env::set_var("LAYA_BROWSER_CONFIG", &path);
        let cfg = load();
        assert_eq!(cfg, BrowserProfileConfig::default());
        match saved {
            Some(v) => std::env::set_var("LAYA_BROWSER_CONFIG", v),
            None => std::env::remove_var("LAYA_BROWSER_CONFIG"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
