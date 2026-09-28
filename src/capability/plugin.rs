//! Rhai plugin host — the engine's extension seam.
//!
//! The Rust code is the **base**: CDP transport, policy gates, resource
//! lifecycle, secret redaction, page-to-Markdown rendering. Anything that is
//! *site-* or *task*-specific (which selector to read, which endpoint to page,
//! how a natural-language query maps to a mode) belongs in a **plugin** written
//! in Rhai under `plugins/<name>/`.
//!
//! A plugin can do exactly what the host registers for it — nothing more:
//!
//! * `run(host, ctx)` receives the expanded `with` arguments (`ctx["with"]`) and
//!   the workflow state (`ctx["state"]`);
//! * `host` is a small, audited surface: every external effect funnels back
//!   through the engine's own capability code, so `policy.allow_hosts` /
//!   `allow_paths` / `allow_exec` and the "one owner for tab cleanup" invariant
//!   keep applying;
//! * the language itself has no file, network, `eval`, `print` or `import`
//!   access, and is bounded by an operation budget.
//!
//! Plugins are resolved in layers, highest priority first: an explicit `dir` →
//! `$LAYA_PLUGIN_DIR/<name>` → `plugins/<name>` walking up to the git root →
//! `~/.config/laya-workflow/plugins/<name>` (the `plugin install` default) →
//! the copy compiled into this binary. That last layer is why the bundled
//! alphaXiv downloader still works after `sudo install`-ing a single binary.

use anyhow::{anyhow, bail, Result};
use rhai::{Dynamic, Engine, EvalAltResult, Position, Scope};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::util;
use super::{
    bounded_timeout, browser, check_host, secret, stringify, truncate, Capability, Policy,
};

/// Name of the bundled alphaXiv downloader plugin.
pub const ALPHAXIV_PLUGIN: &str = "alphaxiv";

/// A `kind: "plugin"` capability: which plugin to load and how to bound it.
#[derive(Clone, Debug, Default)]
pub struct PluginCap {
    /// Plugin name (`plugins/<name>`), or an absolute/relative path when `dir`
    /// is set.
    pub plugin: String,
    /// Optional explicit plugin directory, bypassing the search layers.
    pub dir: String,
    /// Optional entry file, overriding the manifest.
    pub entry: String,
    /// Optional op (function) name, overriding the manifest's `entry_op`.
    pub op: String,
    /// Name of the `chrome_cdp` capability in the same spec that this plugin may
    /// drive. Empty means the plugin gets no browser handle.
    pub browser: String,
    /// Instruction budget handed to Rhai (0 = the host default).
    pub max_operations: u64,
    /// Per-call timeout used for browser/CDP work (0 = the declared chrome
    /// capability / policy default).
    pub timeout_ms: u64,
}

/// The plugin API version this engine implements. A `plugin.json` may declare
/// `"api": 1`; anything else is refused, so a plugin written for a newer host
/// fails loudly instead of half-working.
pub const PLUGIN_API: u64 = 1;

#[derive(Clone, Debug)]
struct Manifest {
    name: String,
    version: String,
    description: String,
    entry: String,
    entry_op: String,
    max_operations: u64,
}

impl Manifest {
    fn parse(src: &str, fallback: &str) -> Result<Manifest> {
        let v: Value = serde_json::from_str(src)
            .map_err(|e| anyhow!("plugin {fallback:?} has an invalid plugin.json: {e}"))?;
        if let Some(api) = v.get("api").and_then(Value::as_u64) {
            if api != PLUGIN_API {
                bail!(
                    "plugin {fallback:?} targets plugin api v{api}, but this engine implements v{PLUGIN_API}"
                );
            }
        }
        Ok(Manifest {
            name: v
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(fallback)
                .to_string(),
            version: v
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("0.0.0")
                .to_string(),
            description: v
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            entry: v
                .get("entry")
                .and_then(Value::as_str)
                .unwrap_or("main.rhai")
                .to_string(),
            entry_op: v
                .get("entry_op")
                .and_then(Value::as_str)
                .unwrap_or("run")
                .to_string(),
            max_operations: v.get("max_operations").and_then(Value::as_u64).unwrap_or(0),
        })
    }
}

/// The raw text of a plugin: manifest, entry script, its page scripts, and the
/// entry file's name (after any `entry` override, for messages).
#[derive(Debug)]
struct Sources {
    manifest: String,
    entry: String,
    entry_name: String,
    pages: Vec<(String, String)>,
    dir: Option<PathBuf>,
}

/// Plugins compiled into this binary (one layer of the resolution order).
fn builtin(name: &str) -> Option<Sources> {
    let (manifest, entry, pages): (&str, &str, Vec<(&str, &str)>) = match name {
        ALPHAXIV_PLUGIN => (
            include_str!("../../plugins/alphaxiv/plugin.json"),
            include_str!("../../plugins/alphaxiv/main.rhai"),
            vec![
                (
                    "links.js",
                    include_str!("../../plugins/alphaxiv/page/links.js"),
                ),
                (
                    "feed.js",
                    include_str!("../../plugins/alphaxiv/page/feed.js"),
                ),
            ],
        ),
        "textdigest" => (
            include_str!("../../plugins/textdigest/plugin.json"),
            include_str!("../../plugins/textdigest/main.rhai"),
            Vec::new(),
        ),
        "hf-trending" => (
            include_str!("../../plugins/hf-trending/plugin.json"),
            include_str!("../../plugins/hf-trending/main.rhai"),
            Vec::new(),
        ),
        "hackernews" => (
            include_str!("../../plugins/hackernews/plugin.json"),
            include_str!("../../plugins/hackernews/main.rhai"),
            vec![
                (
                    "front.js",
                    include_str!("../../plugins/hackernews/page/front.js"),
                ),
                (
                    "story.js",
                    include_str!("../../plugins/hackernews/page/story.js"),
                ),
            ],
        ),
        "arxiv" => (
            include_str!("../../plugins/arxiv/plugin.json"),
            include_str!("../../plugins/arxiv/main.rhai"),
            vec![
                (
                    "search.js",
                    include_str!("../../plugins/arxiv/page/search.js"),
                ),
                ("abs.js", include_str!("../../plugins/arxiv/page/abs.js")),
            ],
        ),
        _ => return None,
    };
    Some(Sources {
        manifest: manifest.to_string(),
        entry: entry.to_string(),
        entry_name: String::new(),
        pages: pages
            .into_iter()
            .map(|(n, src)| (n.to_string(), src.to_string()))
            .collect(),
        dir: None,
    })
}

/// Names of the plugins compiled into this binary.
pub fn builtin_names() -> &'static [&'static str] {
    &[
        ALPHAXIV_PLUGIN,
        "textdigest",
        "hf-trending",
        "hackernews",
        "arxiv",
    ]
}

// ── installing plugins from a git repo ──────────────────────────────

/// Per-user plugin root: `$LAYA_USER_PLUGIN_DIR` →
/// `$XDG_CONFIG_HOME/laya-workflow/plugins` → `~/.config/laya-workflow/plugins`.
/// Mirrors `spec::user_spec_dir()`; `plugin install` writes here unless
/// `$LAYA_PLUGIN_DIR` (or `--root`) overrides it.
pub fn user_plugin_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("LAYA_USER_PLUGIN_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(d));
    }
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(x).join("laya-workflow").join("plugins"));
    }
    std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|h| {
        PathBuf::from(h)
            .join(".config")
            .join("laya-workflow")
            .join("plugins")
    })
}

/// Where `plugin install` writes: `$LAYA_PLUGIN_DIR` when set (so a checkout can
/// pin installs next to the repo), otherwise the per-user plugin root.
pub fn install_root() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("LAYA_PLUGIN_DIR") {
        if !dir.trim().is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    user_plugin_dir().ok_or_else(|| {
        anyhow!("cannot pick an install root: set LAYA_PLUGIN_DIR or HOME/XDG_CONFIG_HOME")
    })
}

/// The on-disk plugin search path, highest priority first (for `plugin dir`).
pub fn plugin_search_path() -> Vec<(PluginLayer, PathBuf)> {
    plugin_roots()
}

/// A plugin name becomes a directory, so it must be a single, safe segment.
pub fn validate_plugin_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("plugin name must not be empty");
    }
    if name != name.trim() {
        bail!("plugin name {name:?} must not have surrounding whitespace");
    }
    if name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        bail!("plugin name {name:?} must be a single path segment");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        bail!("plugin name {name:?} may only contain [A-Za-z0-9._-]");
    }
    Ok(())
}

/// Normalize a plugin directory *inside* a repo: no leading `/`, no `..`, not
/// empty. Returns the cleaned, `/`-separated path.
pub fn normalize_subdir(path: &str) -> Result<String> {
    let p = path.trim().trim_matches('/');
    if p.is_empty() {
        bail!("--path must name a directory inside the repo (e.g. plugins/alphaxiv)");
    }
    for seg in p.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            bail!("--path {path:?} contains an empty or '.'/'..' segment");
        }
    }
    Ok(p.to_string())
}

/// Turn a repo reference into a clone URL. Accepts `owner/repo`,
/// `https://github.com/owner/repo[.git]`, an `ssh://`/`git@` URL, or any URL that
/// already carries a scheme (passed through unchanged).
pub fn repo_clone_url(repo: &str) -> Result<String> {
    let r = repo.trim();
    if r.is_empty() {
        bail!("repo must not be empty (use e.g. owner/repo)");
    }
    if r.contains("://") || r.starts_with("git@") {
        return Ok(r.to_string());
    }
    let parts: Vec<&str> = r.split('/').collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        let slug = format!("{}/{}", parts[0], parts[1].trim_end_matches(".git"));
        return Ok(format!("https://github.com/{slug}.git"));
    }
    bail!("repo {repo:?} must be `owner/repo` or a git URL (https://, ssh://, git@)")
}

/// Strip any `user:token@` credentials from a URL before printing it.
pub fn redact_url(url: &str) -> String {
    match url.find("://") {
        Some(i) => {
            let (scheme, rest) = url.split_at(i + 3);
            match rest.find('@') {
                Some(at) if !rest[..at].contains('/') => {
                    format!("{scheme}***@{}", &rest[at + 1..])
                }
                _ => url.to_string(),
            }
        }
        None => url.to_string(),
    }
}

/// What a successful install produced.
#[derive(Clone, Debug)]
pub struct InstalledPlugin {
    pub name: String,
    pub version: String,
    pub dest: PathBuf,
    pub files: usize,
}

/// Copy a plugin tree `src` into `root/<name>`. Requires a readable, parseable
/// `plugin.json`. Refuses to overwrite an existing install unless `force`.
pub fn install_from_dir(
    src: &std::path::Path,
    root: &std::path::Path,
    name: &str,
    force: bool,
) -> Result<InstalledPlugin> {
    validate_plugin_name(name)?;
    if !src.is_dir() {
        bail!("plugin source {} is not a directory", src.display());
    }
    let manifest = std::fs::read_to_string(src.join("plugin.json"))
        .map_err(|e| anyhow!("{}: cannot read plugin.json: {e}", src.display()))?;
    let m = Manifest::parse(&manifest, name)?;
    let dest = root.join(name);
    if dest.exists() {
        if !force {
            bail!(
                "plugin {name:?} is already installed at {} (pass --force to overwrite)",
                dest.display()
            );
        }
        std::fs::remove_dir_all(&dest)
            .map_err(|e| anyhow!("cannot clear {}: {e}", dest.display()))?;
    }
    std::fs::create_dir_all(&dest)?;
    let files = copy_tree(src, &dest, true)?;
    Ok(InstalledPlugin {
        name: name.to_string(),
        version: m.version,
        dest,
        files,
    })
}

/// Recursively copy regular files (skipping a top-level `.git`) from `src` to
/// `dst`, creating directories as needed. Returns the file count. Symlinks and
/// other special files are skipped on purpose.
fn copy_tree(src: &std::path::Path, dst: &std::path::Path, skip_git: bool) -> Result<usize> {
    let mut files = 0;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if skip_git && name == ".git" {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        let ty = entry.file_type()?;
        if ty.is_dir() {
            std::fs::create_dir_all(&to)?;
            files += copy_tree(&from, &to, false)?;
        } else if ty.is_file() {
            std::fs::copy(&from, &to)?;
            files += 1;
        }
    }
    Ok(files)
}

/// One discoverable plugin and the layer it resolves from.
#[derive(Clone, Debug)]
pub struct PluginEntry {
    pub name: String,
    pub layer: PluginLayer,
    pub version: String,
    pub description: String,
    pub path: Option<PathBuf>,
}

/// Discover every plugin visible to the engine, highest layer first, each name
/// appearing once (a higher layer shadows lower ones).
pub fn discover_plugins() -> Vec<PluginEntry> {
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for (layer, root) in plugin_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().join("plugin.json").is_file())
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .collect();
        names.sort();
        for name in names {
            if !seen.insert(name.clone()) {
                continue;
            }
            let dir = root.join(&name);
            let (version, description) = std::fs::read_to_string(dir.join("plugin.json"))
                .ok()
                .and_then(|s| Manifest::parse(&s, &name).ok())
                .map(|m| (m.version, m.description))
                .unwrap_or_else(|| ("0.0.0".to_string(), String::new()));
            out.push(PluginEntry {
                name,
                layer,
                version,
                description,
                path: Some(dir),
            });
        }
    }
    for n in builtin_names().iter().copied() {
        if seen.insert(n.to_string()) {
            let (version, description) = builtin(n)
                .and_then(|s| Manifest::parse(&s.manifest, n).ok())
                .map(|m| (m.version, m.description))
                .unwrap_or_else(|| ("0.0.0".to_string(), String::new()));
            out.push(PluginEntry {
                name: n.to_string(),
                layer: PluginLayer::Builtin,
                version,
                description,
                path: None,
            });
        }
    }
    out
}

/// Clone **only** `subdir` from `repo` (a sparse, blob-filtered checkout) into a
/// temp dir, then install it as plugin `name`. Nothing outside `subdir` is
/// materialised on disk.
pub fn install_from_git(
    repo: &str,
    subdir: &str,
    name: &str,
    root: &std::path::Path,
    git_ref: Option<&str>,
    force: bool,
) -> Result<InstalledPlugin> {
    let url = repo_clone_url(repo)?;
    let sub = normalize_subdir(subdir)?;
    validate_plugin_name(name)?;
    let tmp = std::env::temp_dir().join(format!(
        "laya-plugin-install-{}-{}",
        std::process::id(),
        util::now_unix_ms()
    ));
    let run = |args: &[&str], cwd: Option<&std::path::Path>| -> Result<()> {
        let mut cmd = std::process::Command::new("git");
        cmd.args(args);
        if let Some(c) = cwd {
            cmd.current_dir(c);
        }
        let out = cmd
            .output()
            .map_err(|e| anyhow!("failed to run git: {e}"))?;
        if !out.status.success() {
            bail!(
                "git {} failed: {}",
                args.first().copied().unwrap_or(""),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    };
    let tmp_s = tmp.to_string_lossy().to_string();
    let mut clone: Vec<&str> = vec!["clone", "--depth", "1", "--filter=blob:none", "--sparse"];
    if let Some(r) = git_ref.map(str::trim).filter(|r| !r.is_empty()) {
        clone.push("--branch");
        clone.push(r);
    }
    clone.push(&url);
    clone.push(&tmp_s);
    let result = (|| -> Result<InstalledPlugin> {
        run(&clone, None)?;
        run(&["sparse-checkout", "set", &sub], Some(&tmp))?;
        let src = tmp.join(&sub);
        if !src.is_dir() {
            bail!(
                "{sub:?} was not found in {} after checkout",
                redact_url(&url)
            );
        }
        install_from_dir(&src, root, name, force)
    })();
    std::fs::remove_dir_all(&tmp).ok();
    result
}

/// Which layer a plugin was resolved from, highest priority first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginLayer {
    /// An explicit `dir` on the capability (never in the search path).
    Dir,
    /// `$LAYA_PLUGIN_DIR/<name>`.
    Env,
    /// `plugins/<name>` walking up from the cwd to the git root.
    Repo,
    /// `~/.config/laya-workflow/plugins/<name>` (where `plugin install` lands).
    User,
    /// The copy compiled into the binary (`include_str!`).
    Builtin,
}

impl PluginLayer {
    pub fn as_str(self) -> &'static str {
        match self {
            PluginLayer::Dir => "dir",
            PluginLayer::Env => "env",
            PluginLayer::Repo => "repo",
            PluginLayer::User => "user",
            PluginLayer::Builtin => "builtin",
        }
    }
}

/// Candidate on-disk `plugins/` roots, highest priority first: `$LAYA_PLUGIN_DIR`
/// → `plugins/` up to the git root → the per-user install root.
fn plugin_roots() -> Vec<(PluginLayer, PathBuf)> {
    let mut roots = Vec::new();
    if let Ok(dir) = std::env::var("LAYA_PLUGIN_DIR") {
        if !dir.trim().is_empty() {
            roots.push((PluginLayer::Env, PathBuf::from(dir)));
        }
    }
    let mut cur = std::env::current_dir().ok();
    while let Some(dir) = cur {
        roots.push((PluginLayer::Repo, dir.join("plugins")));
        if dir.join(".git").exists() {
            break;
        }
        cur = dir.parent().map(std::path::Path::to_path_buf);
    }
    if let Some(user) = user_plugin_dir() {
        roots.push((PluginLayer::User, user));
    }
    roots
}

/// Load every page script under `<dir>/page/` (sorted). Missing is fine.
fn read_pages(dir: &std::path::Path) -> Vec<(String, String)> {
    let mut pages = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir.join("page")) {
        for e in entries.flatten() {
            if e.path().is_file() {
                if let (Some(name), Ok(src)) = (
                    e.file_name().to_str().map(str::to_string),
                    std::fs::read_to_string(e.path()),
                ) {
                    pages.push((name, src));
                }
            }
        }
    }
    pages.sort();
    pages
}

fn read_dir_sources(
    dir: &std::path::Path,
    fallback: &str,
    entry_override: &str,
) -> Result<Sources> {
    let manifest = std::fs::read_to_string(dir.join("plugin.json")).map_err(|e| {
        anyhow!(
            "plugin {fallback:?}: cannot read {}: {e}",
            dir.join("plugin.json").display()
        )
    })?;
    let m = Manifest::parse(&manifest, fallback)?;
    let entry_name = if entry_override.trim().is_empty() {
        m.entry.clone()
    } else {
        entry_override.trim().to_string()
    };
    if entry_name.is_empty() {
        bail!("plugin {fallback:?}: manifest has an empty 'entry'");
    }
    let entry = std::fs::read_to_string(dir.join(&entry_name))
        .map_err(|e| anyhow!("plugin {fallback:?}: cannot read entry {entry_name:?}: {e}"))?;
    Ok(Sources {
        manifest,
        entry,
        entry_name,
        pages: read_pages(dir),
        dir: Some(dir.to_path_buf()),
    })
}

/// A single `.rhai` file used as its own plugin — no directory and no
/// `plugin.json` needed. The `name` is the file stem, `entry_op` defaults to
/// `run`, and a sibling `page/` directory (if any) is still picked up.
fn read_file_sources(file: &std::path::Path, fallback: &str) -> Result<Sources> {
    let file_name = file
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("plugin script {} has no UTF-8 file name", file.display()))?
        .to_string();
    let name = file
        .file_stem()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or(fallback)
        .to_string();
    let entry = std::fs::read_to_string(file)
        .map_err(|e| anyhow!("plugin script {}: cannot read: {e}", file.display()))?;
    let manifest = serde_json::json!({
        "name": name,
        "entry": file_name,
        "entry_op": "run",
        "api": PLUGIN_API,
    })
    .to_string();
    let parent = file.parent();
    Ok(Sources {
        manifest,
        entry,
        entry_name: file_name,
        pages: parent.map(read_pages).unwrap_or_default(),
        dir: parent.map(std::path::Path::to_path_buf),
    })
}

fn load_sources(cap: &PluginCap) -> Result<Sources> {
    let dir = cap.dir.trim();
    let entry = cap.entry.trim();
    if !dir.is_empty() {
        let path = std::path::Path::new(dir);
        // `dir` may name a single `.rhai` file instead of a plugin directory.
        if path.is_file() {
            let fallback = std::path::Path::new(dir)
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("plugin");
            return read_file_sources(path, fallback);
        }
        // An explicit directory is enough; the name is only used for messages.
        let fallback = if cap.plugin.trim().is_empty() {
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("plugin")
                .to_string()
        } else {
            cap.plugin.clone()
        };
        return read_dir_sources(path, &fallback, entry);
    }
    // No `dir`/`plugin`: a bare `entry` naming an existing file runs it directly.
    if !entry.is_empty() && std::path::Path::new(entry).is_file() {
        return read_file_sources(std::path::Path::new(entry), "plugin");
    }
    if cap.plugin.trim().is_empty() {
        bail!("plugin capability needs 'plugin' (a name), 'dir', or a script 'entry'");
    }
    for (_, root) in plugin_roots() {
        let d = root.join(&cap.plugin);
        if d.join("plugin.json").is_file() {
            return read_dir_sources(&d, &cap.plugin, entry);
        }
    }
    builtin(&cap.plugin).ok_or_else(|| {
        anyhow!(
            "plugin {:?} not found (looked in $LAYA_PLUGIN_DIR, plugins/ above the cwd, and the built-ins)",
            cap.plugin
        )
    })
}

// ── the host handle a plugin can call ───────────────────────────────

struct HostState {
    pages: Vec<(String, String)>,
    dir: Option<PathBuf>,
    policy: Policy,
    browser: Option<browser::BrowserCap>,
    endpoint: Option<String>,
    timeout: Duration,
    /// Targets this run opened; the engine (not the plugin) closes them.
    opened: Vec<String>,
    keep_open: bool,
}

impl HostState {
    fn browser_host(&self) -> Result<browser::BrowserHost<'_>> {
        let cap = self.browser.as_ref().ok_or_else(|| {
            anyhow!(
                "this plugin has no browser handle (declare 'browser' on the plugin capability)"
            )
        })?;
        let endpoint = self
            .endpoint
            .as_ref()
            .ok_or_else(|| anyhow!("the browser endpoint is not ready"))?;
        Ok(browser::BrowserHost {
            cap,
            endpoint: endpoint.clone(),
            policy: &self.policy,
            timeout: self.timeout,
        })
    }
}

/// The value a plugin sees as `host`. Shared by `Arc<Mutex<..>>` so the
/// (cloned) handle passed into the script keeps one piece of state.
#[derive(Clone)]
struct Host(Arc<Mutex<HostState>>);

impl Host {
    fn with<T>(&self, f: impl FnOnce(&mut HostState) -> Result<T>) -> Result<T> {
        let mut guard = self
            .0
            .lock()
            .map_err(|_| anyhow!("plugin host state is poisoned"))?;
        f(&mut guard)
    }
}

fn rt(e: impl std::fmt::Display) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        e.to_string().into(),
        Position::NONE,
    ))
}

fn to_dyn(v: Value) -> Result<Dynamic, Box<EvalAltResult>> {
    rhai::serde::to_dynamic(v).map_err(|e| rt(format!("value is not script-representable: {e}")))
}

fn from_dyn(v: Dynamic) -> Result<Value, Box<EvalAltResult>> {
    rhai::serde::from_dynamic::<Value>(&v).map_err(|e| rt(format!("value is not JSON: {e}")))
}

/// Build the sandboxed engine and register the whole host surface.
///
/// Everything a plugin can reach is registered *here*; the language itself
/// contributes no I/O. Keep this list small and auditable — it is the plugin
/// API.
fn build_engine(max_operations: u64) -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(max_operations.max(100_000));
    engine.set_max_call_levels(32);
    engine.set_max_expr_depths(96, 48);
    engine.set_max_string_size(8 << 20);
    engine.set_max_array_size(200_000);
    engine.set_max_map_size(200_000);
    // No escape hatches: no dynamic eval, no module import, no raw printing
    // (the raw `print`/`debug` would bypass secret redaction).
    for sym in ["eval", "import", "print", "debug", "Fn"] {
        let _ = engine.disable_symbol(sym);
    }

    // ── logs (redacted) ──
    engine.register_fn("log", |_h: &mut Host, msg: &str| {
        eprintln!("[plugin] {}", secret::redact_str(msg));
    });

    // ── pure host utilities ──
    engine.register_fn(
        "now",
        |_h: &mut Host| -> Result<Dynamic, Box<EvalAltResult>> {
            // UTC *and* local, with the offset + zone so the local field is
            // unambiguous (see `util::now_fields`).
            to_dyn(util::now_fields())
        },
    );
    // Turn an upstream timestamp (`2026-09-24T03:39:57.000Z`, `…+08:00`, a
    // bare epoch) into milliseconds; `()` when it cannot be parsed, so a plugin
    // can keep the raw string instead of inventing an instant.
    engine.register_fn("parse_time", |_h: &mut Host, s: &str| -> Dynamic {
        match util::parse_time_to_unix_ms(s) {
            Some(ms) => Dynamic::from(ms),
            None => Dynamic::from(()),
        }
    });
    // Render milliseconds at an explicit offset: `…+08:00` (or `Z` for 0).
    engine.register_fn(
        "time_format",
        |_h: &mut Host, ms: i64, offset_secs: i64| -> String {
            util::rfc3339_from_unix_ms_offset(ms, offset_secs)
        },
    );
    engine.register_fn("urlencode", |_h: &mut Host, s: &str| -> String {
        util::urlencoding_utf8(s)
    });
    engine.register_fn("slug", |_h: &mut Host, url: &str, title: &str| -> String {
        browser::derive_slug(url, title)
    });
    engine.register_fn(
        "timeout_ms",
        |h: &mut Host| -> Result<i64, Box<EvalAltResult>> {
            h.with(|st| Ok(st.timeout.as_millis() as i64)).map_err(rt)
        },
    );
    engine.register_fn(
        "json_parse",
        |_h: &mut Host, s: &str| -> Result<Dynamic, Box<EvalAltResult>> {
            let v: Value = serde_json::from_str(s).map_err(|e| rt(format!("json_parse: {e}")))?;
            to_dyn(v)
        },
    );
    engine.register_fn(
        "json_stringify",
        |_h: &mut Host, v: Dynamic| -> Result<String, Box<EvalAltResult>> {
            let j = from_dyn(v)?;
            serde_json::to_string(&j).map_err(rt)
        },
    );
    engine.register_fn(
        "js",
        |h: &mut Host, name: &str| -> Result<String, Box<EvalAltResult>> {
            h.with(|st| {
                if let Some((_, src)) = st.pages.iter().find(|(n, _)| n == name) {
                    return Ok(src.clone());
                }
                let dir = st
                    .dir
                    .as_ref()
                    .ok_or_else(|| anyhow!("plugin has no page script {name:?}"))?;
                std::fs::read_to_string(dir.join("page").join(name))
                    .map_err(|e| anyhow!("plugin page {name:?}: {e}"))
            })
            .map_err(rt)
        },
    );

    // ── browser: the engine's own primitives, not a second implementation ──
    engine.register_fn(
        "browser_open",
        |h: &mut Host, url: &str| -> Result<String, Box<EvalAltResult>> {
            let id = h.with(|st| st.browser_host()?.open(url)).map_err(rt)?;
            h.with(|st| {
                st.opened.push(id.clone());
                Ok(())
            })
            .map_err(rt)?;
            Ok(id)
        },
    );
    engine.register_fn(
        "browser_navigate",
        |h: &mut Host, id: &str, url: &str| -> Result<(), Box<EvalAltResult>> {
            h.with(|st| st.browser_host()?.navigate(id, url))
                .map_err(rt)
        },
    );
    engine.register_fn(
        "browser_wait_ready",
        |h: &mut Host, id: &str| -> Result<Dynamic, Box<EvalAltResult>> {
            let v = h.with(|st| st.browser_host()?.wait_ready(id)).map_err(rt)?;
            to_dyn(v)
        },
    );
    engine.register_fn(
        "browser_evaluate",
        |h: &mut Host,
         id: &str,
         expression: &str,
         await_promise: bool|
         -> Result<Dynamic, Box<EvalAltResult>> {
            let v = h
                .with(|st| st.browser_host()?.evaluate(id, expression, await_promise))
                .map_err(rt)?;
            to_dyn(v)
        },
    );
    engine.register_fn(
        "browser_release",
        |h: &mut Host, id: &str| -> Result<i64, Box<EvalAltResult>> {
            let (closed, errors) = h
                .with(|st| {
                    let released = st.browser_host()?.release(id);
                    // Drop it from the run scope as well, so the end-of-run
                    // sweep does not try to close the same page twice.
                    st.opened.retain(|open| open != id);
                    Ok(released)
                })
                .map_err(rt)?;
            if let Some(first) = errors.first() {
                eprintln!(
                    "[plugin] release {id} reported {} error(s): {}",
                    errors.len(),
                    stringify(first)
                );
            }
            Ok(closed as i64)
        },
    );
    engine.register_fn(
        "save_article",
        |h: &mut Host, opts: Dynamic| -> Result<Dynamic, Box<EvalAltResult>> {
            let leaf = from_dyn(opts)?;
            let v = h
                .with(|st| st.browser_host()?.save_article(&leaf))
                .map_err(rt)?;
            to_dyn(v)
        },
    );
    // ── policy-gated filesystem: the base owns the gate, the plugin owns the
    //    decision of *what* to persist ──
    engine.register_fn(
        "write_file",
        |h: &mut Host, path: &str, text: &str| -> Result<Dynamic, Box<EvalAltResult>> {
            let v = h
                .with(|st| {
                    let dest = browser::allowed_out_dir(&st.policy, path)?;
                    if text.len() > st.policy.max_output {
                        bail!(
                            "write_file {} bytes > policy.max_output {}",
                            text.len(),
                            st.policy.max_output
                        );
                    }
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&dest, text.as_bytes())?;
                    Ok(json!({ "path": dest.display().to_string(), "bytes": text.len() }))
                })
                .map_err(rt)?;
            to_dyn(v)
        },
    );
    engine.register_fn(
        "read_file",
        |h: &mut Host, path: &str| -> Result<Dynamic, Box<EvalAltResult>> {
            let v = h
                .with(|st| {
                    let src = browser::allowed_out_dir(&st.policy, path)?;
                    if !src.is_file() {
                        return Ok(json!({
                            "path": src.display().to_string(),
                            "exists": false, "bytes": 0, "text": ""
                        }));
                    }
                    let text = truncate(std::fs::read_to_string(&src)?, st.policy.max_output);
                    Ok(json!({
                        "path": src.display().to_string(),
                        "exists": true, "bytes": text.len(), "text": text
                    }))
                })
                .map_err(rt)?;
            to_dyn(v)
        },
    );
    engine.register_fn(
        "http_get",
        |h: &mut Host, url: &str| -> Result<Dynamic, Box<EvalAltResult>> {
            let v = h
                .with(|st| {
                    check_host(url, &st.policy)?;
                    let agent = ureq::AgentBuilder::new()
                        .timeout(bounded_timeout(st.timeout.as_millis() as u64, &st.policy))
                        .build();
                    let resp = agent
                        .get(url)
                        .call()
                        .map_err(|e| anyhow!("http_get {url} failed: {e}"))?;
                    let status = resp.status() as i64;
                    let text = truncate(
                        resp.into_string()
                            .map_err(|e| anyhow!("http_get read: {e}"))?,
                        st.policy.max_output,
                    );
                    Ok(json!({ "status": status, "text": text }))
                })
                .map_err(rt)?;
            to_dyn(v)
        },
    );

    engine
}

// ── entry points ────────────────────────────────────────────────────

/// Run a named built-in plugin (no spec-level capability needed).
///
/// `preopened` is how the browser capability's `op: "alphaxiv"` shim hands the
/// already-resolved singleton endpoint to the plugin.
pub fn run_named(
    name: &str,
    with: &Value,
    state: &Value,
    policy: &Policy,
    preopened: Option<&browser::BrowserHost<'_>>,
) -> Result<Value> {
    let cap = PluginCap {
        plugin: name.to_string(),
        ..Default::default()
    };
    run(&cap, with, state, policy, None, preopened)
}

/// Run a `kind: "plugin"` capability, resolving its browser handle from the
/// same spec's capability registry.
pub fn call_plugin(
    cap: &PluginCap,
    with: &Value,
    state: &Value,
    policy: &Policy,
    caps: &HashMap<String, Capability>,
) -> Result<Value> {
    let mut value = run(cap, with, state, policy, Some(caps), None)?;
    if let Value::Object(map) = &mut value {
        // Identity of the call, not something the script has to remember to add.
        map.insert("capability".to_string(), json!("plugin"));
        // A `dir`/`entry` plugin carries no name on the capability, so whatever
        // the script reported as its own name (ctx["plugin"]) stays.
        if !cap.plugin.trim().is_empty() {
            map.insert("plugin".to_string(), json!(cap.plugin));
        }
    }
    Ok(value)
}

fn run(
    cap: &PluginCap,
    with: &Value,
    state: &Value,
    policy: &Policy,
    caps: Option<&HashMap<String, Capability>>,
    preopened: Option<&browser::BrowserHost<'_>>,
) -> Result<Value> {
    let sources = load_sources(cap)?;
    let manifest = Manifest::parse(&sources.manifest, &cap.plugin)?;
    let entry_name = if sources.entry_name.is_empty() {
        manifest.entry.clone()
    } else {
        sources.entry_name.clone()
    };
    let limit = if cap.max_operations > 0 {
        cap.max_operations
    } else {
        manifest.max_operations
    };

    // Resolve the browser handle: either the caller already opened one, or the
    // plugin names a `chrome_cdp` capability in the same spec.
    let (browser_cap, endpoint, timeout) = match preopened {
        Some(host) => (
            Some(host.cap.clone()),
            Some(host.endpoint.clone()),
            host.timeout,
        ),
        None => match caps {
            Some(caps) if !cap.browser.trim().is_empty() => {
                let name = cap.browser.trim();
                let bc = match caps.get(name) {
                    Some(Capability::Browser(c)) => c.clone(),
                    Some(_) => bail!(
                        "plugin {:?}: 'browser' names {name:?}, which is not a chrome_cdp capability",
                        cap.plugin
                    ),
                    None => bail!(
                        "plugin {:?}: 'browser' names {name:?}, which is not declared in this spec",
                        cap.plugin
                    ),
                };
                let ep = browser::ensure_runtime(&bc, with, state, policy)?;
                let t = if cap.timeout_ms > 0 {
                    Duration::from_millis(cap.timeout_ms)
                } else {
                    Duration::from_millis(bc.timeout_ms)
                };
                (
                    Some(bc),
                    Some(ep),
                    bounded_timeout(t.as_millis() as u64, policy),
                )
            }
            _ => (None, None, bounded_timeout(cap.timeout_ms, policy)),
        },
    };

    let host = Host(Arc::new(Mutex::new(HostState {
        pages: sources.pages.clone(),
        dir: sources.dir.clone(),
        policy: policy.clone(),
        browser: browser_cap,
        endpoint,
        timeout,
        opened: Vec::new(),
        keep_open: with
            .get("keep_open")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })));

    let engine = build_engine(limit);
    let ast = engine.compile(&sources.entry).map_err(|e| {
        anyhow!(
            "plugin {:?} ({entry_name}) did not compile: {e}",
            cap.plugin
        )
    })?;

    // Pick the op: an explicit request that exists in the script, else the
    // manifest's entry op.
    let wanted = if cap.op.trim().is_empty() {
        with.get("op").map(stringify).unwrap_or_default()
    } else {
        cap.op.trim().to_string()
    };
    let pick = |name: &str| ast.iter_functions().any(|f| f.name.to_string() == name);
    let op = if !wanted.is_empty() && wanted != cap.plugin && pick(&wanted) {
        wanted
    } else {
        manifest.entry_op.clone()
    };
    let arity = ast
        .iter_functions()
        .find(|f| f.name.to_string() == op)
        .map(|f| f.params.len())
        .ok_or_else(|| anyhow!("plugin {:?} defines no {op:?} function", cap.plugin))?;

    let ctx = json!({
        "plugin": manifest.name,
        "op": op,
        "with": with,
        "state": state,
    });

    let mut scope = Scope::new();
    scope.push("host", host.clone());
    let ctx_dyn = to_dyn(ctx.clone()).map_err(|e| anyhow!("{e}"))?;
    let value: Dynamic = if arity >= 2 {
        engine.call_fn(
            &mut scope,
            &ast,
            op.as_str(),
            (Dynamic::from(host.clone()), ctx_dyn),
        )
    } else {
        engine.call_fn(&mut scope, &ast, op.as_str(), (ctx_dyn,))
    }
    .map_err(|e| anyhow!("plugin {:?} {op}() failed: {e}", cap.plugin))?;

    let result = {
        let json = from_dyn(value).map_err(|e| anyhow!("plugin {:?}: {e}", cap.plugin))?;
        match json {
            Value::Object(_) => json,
            other => json!({ "value": other }),
        }
    };

    // The engine closes what the plugin opened — never the plugin itself.
    let (opened, keep_open, endpoint, timeout) = host
        .with(|st| {
            Ok((
                std::mem::take(&mut st.opened),
                st.keep_open,
                st.endpoint.clone(),
                st.timeout,
            ))
        })
        .unwrap_or_default();
    if !keep_open && !opened.is_empty() {
        if let Some(endpoint) = endpoint {
            let (closed, errors) = browser::close_target_ids(&endpoint, &opened, timeout);
            if !errors.is_empty() {
                eprintln!(
                    "[plugin] closed {closed} tab(s), {} error(s): {}",
                    errors.len(),
                    stringify(&Value::Array(errors))
                );
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_host() -> Host {
        Host(Arc::new(Mutex::new(HostState {
            pages: builtin(ALPHAXIV_PLUGIN).unwrap().pages,
            dir: None,
            policy: Policy::default(),
            browser: None,
            endpoint: None,
            timeout: Duration::from_secs(30),
            opened: Vec::new(),
            keep_open: false,
        })))
    }

    /// Compile one bundled plugin and return `(engine, ast, host)` for a direct
    /// call — the same engine the host builds, minus the browser/network calls,
    /// so the *script's* behaviour is what gets asserted.
    fn harness(plugin: &str) -> (Engine, rhai::AST, Host) {
        let sources = builtin(plugin).unwrap_or_else(|| panic!("bundled plugin {plugin}"));
        let engine = build_engine(0);
        let ast = engine
            .compile(&sources.entry)
            .unwrap_or_else(|e| panic!("compile {plugin}: {e}"));
        (engine, ast, test_host())
    }

    /// Call `name` with JSON args (host injected into scope, not as an arg).
    fn call_of(plugin: &str, name: &str, args: Vec<Value>) -> Result<Value> {
        let (engine, ast, host) = harness(plugin);
        let mut scope = Scope::new();
        scope.push("host", host);
        let dyns: Vec<Dynamic> = args
            .into_iter()
            .map(|a| to_dyn(a).map_err(|e| anyhow!("{e}")))
            .collect::<Result<_>>()?;
        let out: Dynamic = match dyns.len() {
            1 => engine.call_fn(&mut scope, &ast, name, (dyns[0].clone(),)),
            2 => engine.call_fn(&mut scope, &ast, name, (dyns[0].clone(), dyns[1].clone())),
            _ => bail!("unsupported arity"),
        }
        .map_err(|e| anyhow!("{name}(): {e}"))?;
        from_dyn(out).map_err(|e| anyhow!("{e}"))
    }

    /// Call a function whose first parameter is the host handle.
    fn call_with_host(plugin: &str, name: &str, arg: Value) -> Result<Value> {
        let (engine, ast, host) = harness(plugin);
        let mut scope = Scope::new();
        let dyn_arg = to_dyn(arg).map_err(|e| anyhow!("{e}"))?;
        let out: Dynamic = engine
            .call_fn(&mut scope, &ast, name, (host, dyn_arg))
            .map_err(|e| anyhow!("{name}(): {e}"))?;
        from_dyn(out).map_err(|e| anyhow!("{e}"))
    }

    /// Call a function whose first parameter is the host handle, with 2 more args.
    fn call_host2(plugin: &str, name: &str, a: Value, b: Value) -> Result<Value> {
        let (engine, ast, host) = harness(plugin);
        let mut scope = Scope::new();
        let out: Dynamic = engine
            .call_fn(
                &mut scope,
                &ast,
                name,
                (
                    Dynamic::from(host),
                    to_dyn(a).map_err(|e| anyhow!("{e}"))?,
                    to_dyn(b).map_err(|e| anyhow!("{e}"))?,
                ),
            )
            .map_err(|e| anyhow!("{name}(): {e}"))?;
        from_dyn(out).map_err(|e| anyhow!("{e}"))
    }

    fn call(name: &str, args: Vec<Value>) -> Result<Value> {
        call_of(ALPHAXIV_PLUGIN, name, args)
    }

    fn plan(with: Value) -> Result<Value> {
        call("plan", vec![with])
    }

    #[test]
    fn bundled_plugin_loads_and_parses() {
        let src = builtin(ALPHAXIV_PLUGIN).expect("bundled plugin");
        let m = Manifest::parse(&src.manifest, ALPHAXIV_PLUGIN).unwrap();
        assert_eq!(m.name, "alphaxiv");
        assert_eq!(m.entry, "main.rhai");
        assert_eq!(m.entry_op, "run");
        assert!(m.max_operations > 0);
        assert!(src.pages.iter().any(|(n, _)| n == "links.js"));
        assert!(src.pages.iter().any(|(n, _)| n == "feed.js"));
        // The second bundled plugin (the offline demo) resolves too.
        let demo = builtin("textdigest").expect("bundled textdigest");
        assert!(demo.entry.contains("fn run"));
        assert!(Manifest::parse(&demo.manifest, "textdigest").is_ok());
        assert!(builtin("no-such-builtin").is_none());
        // The page scripts still carry the two collectors the op depends on.
        assert!(src
            .pages
            .iter()
            .any(|(_, s)| s.contains("/abs/") && s.contains("__LAYA_OPTS__")));
        assert!(src
            .pages
            .iter()
            .any(|(_, s)| s.contains("api.alphaxiv.org")));
    }

    #[test]
    fn resolves_plugins_in_layers() {
        // An explicit dir wins and is read from disk.
        let dir = std::env::temp_dir().join(format!("laya-plugin-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("page")).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"mini","entry":"main.rhai"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("main.rhai"), "fn run(host, ctx) { #{ ok: true } }").unwrap();
        let cap = PluginCap {
            plugin: "mini".to_string(),
            dir: dir.display().to_string(),
            ..Default::default()
        };
        let src = load_sources(&cap).unwrap();
        assert_eq!(Manifest::parse(&src.manifest, "mini").unwrap().name, "mini");
        assert!(src.entry.contains("fn run"));
        std::fs::remove_dir_all(&dir).ok();

        // An unknown name is a hard error naming the layers it searched.
        let missing = PluginCap {
            plugin: "no-such-plugin".to_string(),
            ..Default::default()
        };
        let err = load_sources(&missing).unwrap_err().to_string();
        assert!(err.contains("no-such-plugin"), "{err}");
    }

    /// A *tiny* plugin that only returns its inputs proves the host path
    /// end-to-end without touching a browser.
    #[test]
    fn runs_an_inline_plugin_and_bound_operations() {
        let dir = std::env::temp_dir().join(format!("laya-plugin-run-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"echo","entry_op":"run","max_operations":200000}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("main.rhai"),
            r#"
            fn run(host, ctx) {
                let args = ctx["with"];
                #{ echo: args["text"], mode: args["mode"], now: host.now()["rfc3339"] }
            }
            "#,
        )
        .unwrap();
        let cap = PluginCap {
            plugin: "echo".to_string(),
            dir: dir.display().to_string(),
            ..Default::default()
        };
        let out = run(
            &cap,
            &json!({"text": "hi", "mode": "x"}),
            &json!({}),
            &Policy::default(),
            None,
            None,
        )
        .unwrap();
        assert_eq!(out["echo"], json!("hi"));
        assert_eq!(out["mode"], json!("x"));
        assert!(out["now"].as_str().unwrap().ends_with('Z'));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A bare `.rhai` file is a plugin in its own right: no directory and no
    /// `plugin.json`. Reachable either by `dir` = the file or by `entry` = the
    /// file (with no name/dir).
    #[test]
    fn runs_a_single_script_file() {
        let dir = std::env::temp_dir().join(format!("laya-plugin-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("greet.rhai");
        std::fs::write(
            &file,
            r#"fn run(host, ctx) { #{ hi: "there", op: ctx["op"] } }"#,
        )
        .unwrap();
        let path = file.display().to_string();

        for cap in [
            PluginCap {
                dir: path.clone(),
                ..Default::default()
            },
            PluginCap {
                entry: path.clone(),
                ..Default::default()
            },
        ] {
            let out = run(&cap, &json!({}), &json!({}), &Policy::default(), None, None).unwrap();
            assert_eq!(out["hi"], json!("there"));
            assert_eq!(out["op"], json!("run")); // entry_op defaults to `run`
        }

        let src = load_sources(&PluginCap {
            dir: path,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(src.entry_name, "greet.rhai");
        let m = Manifest::parse(&src.manifest, "x").unwrap();
        assert_eq!(m.name, "greet"); // the file stem becomes the plugin name
        assert_eq!(m.entry_op, "run");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The capability's `entry` really selects which file a directory plugin
    /// compiles (the manifest only supplies the default).
    #[test]
    fn entry_overrides_the_manifest_entry() {
        let dir = std::env::temp_dir().join(format!("laya-plugin-entry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"m","entry":"main.rhai"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("main.rhai"),
            r#"fn run(host, ctx) { #{ which: "main" } }"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("alt.rhai"),
            r#"fn run(host, ctx) { #{ which: "alt" } }"#,
        )
        .unwrap();
        let cap = PluginCap {
            plugin: "m".to_string(),
            dir: dir.display().to_string(),
            entry: "alt.rhai".to_string(),
            ..Default::default()
        };
        let out = run(&cap, &json!({}), &json!({}), &Policy::default(), None, None).unwrap();
        assert_eq!(out["which"], json!("alt"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plugin_language_has_no_escape_hatches() {
        let engine = build_engine(0);
        // `eval` and `import` are rejected outright…
        assert!(engine.compile("eval(\"1 + 1\")").is_err());
        assert!(engine.compile("import \"x\" as y;").is_err());
        // …and there is no file or process function to call.
        assert!(engine.compile("read_file(\"/etc/passwd\")").is_ok());
        assert!(engine
            .eval::<Dynamic>("read_file(\"/etc/passwd\")")
            .is_err());
        // A runaway script is stopped by the operation budget, not by luck.
        let bounded = build_engine(1);
        assert!(bounded
            .eval::<Dynamic>("let i = 0; while true { i += 1; }")
            .is_err());
    }

    #[test]
    fn plan_infers_mode_from_natural_language() {
        // Bare search phrase → search, save the top paper.
        let p = plan(json!({"query": "llm memory"})).unwrap();
        assert_eq!(p["mode"], json!("search"));
        assert_eq!(p["count"], json!(1));
        assert_eq!(p["target_url"], json!(""));

        // Trending keywords (english + chinese) → the explore feed.
        for q in [
            "trending",
            "Trending Papers",
            "explore",
            "热门",
            "最新的论文",
        ] {
            let p = plan(json!({"query": q})).unwrap();
            assert_eq!(p["mode"], json!("trending"), "query {q:?}");
            assert_eq!(p["count"], json!(10), "query {q:?}");
        }

        // A paper URL in the query → direct save, no listing.
        let url = "https://www.alphaxiv.org/abs/2609.recurrent-looped-transformer";
        let p = plan(json!({"query": url})).unwrap();
        assert_eq!(p["mode"], json!("url"));
        assert_eq!(p["target_url"], json!(url));

        // `paper_url` wins even when the query reads like a keyword.
        let p = plan(json!({"query": "whatever", "paper_url": url})).unwrap();
        assert_eq!(p["mode"], json!("url"));
        assert_eq!(p["target_url"], json!(url));

        // Explicit mode + count.
        let p = plan(json!({"query": "diffusion", "mode": "trending", "count": 3})).unwrap();
        assert_eq!(p["mode"], json!("trending"));
        assert_eq!(p["count"], json!(3));

        // Trending pages through the feed API, so its count can reach hundreds…
        let p = plan(json!({"query": "trending", "count": 100})).unwrap();
        assert_eq!(p["count"], json!(100));
        // …but never unbounded.
        let p = plan(json!({"query": "trending", "count": 100_000})).unwrap();
        assert_eq!(p["count"], json!(500));

        // Search still has only one rendered page of cards.
        let p = plan(json!({"query": "diffusion", "mode": "search", "count": 99})).unwrap();
        assert_eq!(p["mode"], json!("search"));
        assert_eq!(p["count"], json!(10));
    }

    #[test]
    fn plan_rejects_empty_and_bad_inputs() {
        assert!(plan(json!({})).is_err());
        assert!(plan(json!({"query": "   "})).is_err());
        assert!(plan(json!({"query": "x", "mode": "bogus"})).is_err());
        // url mode demands a concrete URL.
        assert!(plan(json!({"mode": "url"})).is_err());
        // A failed plan surfaces the script's own message.
        let err = plan(json!({})).unwrap_err().to_string();
        assert!(err.contains("alphaxiv needs 'query'"), "{err}");
    }

    #[test]
    fn interval_is_restricted() {
        for (given, want) in [
            (json!("7 Days"), "7 Days"),
            (json!("3 days"), "3 Days"),
            (json!("ALL TIME"), "All time"),
            (json!("90 Days"), "90 Days"),
        ] {
            assert_eq!(
                call("interval_of", vec![json!({"interval": given})]).unwrap(),
                json!(want),
                "{given:?}"
            );
        }
        // Missing or bogus values fall back to the default window.
        assert_eq!(
            call("interval_of", vec![json!({})]).unwrap(),
            json!("7 Days")
        );
        assert_eq!(
            call("interval_of", vec![json!({"interval": "yesterday"})]).unwrap(),
            json!("7 Days")
        );
    }

    #[test]
    fn localizes_abs_urls() {
        let u = "https://www.alphaxiv.org/abs/2609.recurrent-looped-transformer";
        let loc = |url: &str, lang: &str| call("localize", vec![json!(url), json!(lang)]).unwrap();
        assert_eq!(
            loc(u, "zh"),
            json!("https://www.alphaxiv.org/zh/abs/2609.recurrent-looped-transformer")
        );
        // An existing locale prefix is replaced, never doubled.
        assert_eq!(
            loc("https://www.alphaxiv.org/zh/abs/2609.x", "ja"),
            json!("https://www.alphaxiv.org/ja/abs/2609.x")
        );
        assert_eq!(
            loc("https://www.alphaxiv.org/zh/abs/2609.x", "zh"),
            json!("https://www.alphaxiv.org/zh/abs/2609.x")
        );
        // en/off/empty opts out; a query string is dropped when rebuilding.
        assert_eq!(loc(u, "en"), json!(u));
        assert_eq!(loc(u, ""), json!(u));
        assert_eq!(
            loc("https://www.alphaxiv.org/abs/2609.x?foo=1", "zh"),
            json!("https://www.alphaxiv.org/zh/abs/2609.x")
        );
        // Non-paper paths and other hosts are untouched.
        assert_eq!(
            loc("https://www.alphaxiv.org/researchers", "zh"),
            json!("https://www.alphaxiv.org/researchers")
        );
        assert_eq!(
            loc("https://arxiv.org/abs/2307.12307", "zh"),
            json!("https://arxiv.org/abs/2307.12307")
        );
    }

    #[test]
    fn detects_paper_urls_and_trending_words() {
        let truthy = |f: &str, s: &str| call(f, vec![json!(s)]).unwrap() == json!(true);
        assert!(truthy(
            "looks_like_paper_url",
            "https://www.alphaxiv.org/abs/2609.x"
        ));
        assert!(truthy(
            "looks_like_paper_url",
            "https://arxiv.org/abs/2307.12307"
        ));
        assert!(!truthy(
            "looks_like_paper_url",
            "recurrent looped transformer"
        ));
        assert!(truthy("is_trending", "trending"));
        assert!(truthy("is_trending", "show me trending in llm"));
        assert!(truthy("is_trending", "热门论文"));
        assert!(!truthy("is_trending", "llm memory"));
        assert!(!truthy("is_trending", "attention is all you need"));
    }

    #[test]
    fn plugin_kind_parses_from_a_spec() {
        let spec = json!({
            "capabilities": {
                "chrome": { "kind": "chrome_cdp", "endpoint": "http://127.0.0.1:9222" },
                "papers": { "kind": "plugin", "plugin": "alphaxiv", "browser": "chrome" }
            },
            "nodes": []
        });
        let reg = super::super::Registry::from_spec(&spec).unwrap();
        let mut names = reg.names();
        names.sort();
        assert_eq!(names, vec!["chrome".to_string(), "papers".to_string()]);
        // `script` is an accepted alias for the same kind.
        let alias = json!({"capabilities": {"s": {"kind": "script", "plugin": "x"}}});
        assert!(super::super::Registry::from_spec(&alias).is_ok());
    }

    #[test]
    fn plugin_name_and_subdir_validation() {
        for ok in ["alphaxiv", "a.b-c_1"] {
            assert!(validate_plugin_name(ok).is_ok(), "{ok:?}");
        }
        for bad in ["", " ", "a/b", "..", ".", "a\\b", "../x", "a b"] {
            assert!(validate_plugin_name(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            normalize_subdir("plugins/alphaxiv").unwrap(),
            "plugins/alphaxiv"
        );
        assert_eq!(
            normalize_subdir("/plugins/alphaxiv/").unwrap(),
            "plugins/alphaxiv"
        );
        for bad in ["", "/", "plugins/../etc", "./x", "plugins//x"] {
            assert!(normalize_subdir(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn repo_refs_normalize_to_clone_urls() {
        assert_eq!(
            repo_clone_url("oliveagle/agents_group").unwrap(),
            "https://github.com/oliveagle/agents_group.git"
        );
        assert_eq!(
            repo_clone_url("o/r.git").unwrap(),
            "https://github.com/o/r.git"
        );
        assert_eq!(
            repo_clone_url("https://example.com/x.git").unwrap(),
            "https://example.com/x.git"
        );
        assert_eq!(
            repo_clone_url("git@github.com:o/r.git").unwrap(),
            "git@github.com:o/r.git"
        );
        for bad in ["", "just-a-name", "a/b/c"] {
            assert!(repo_clone_url(bad).is_err(), "{bad:?}");
        }
        // Credentials never reach the terminal.
        assert_eq!(
            redact_url("https://user:tok@github.com/o/r.git"),
            "https://***@github.com/o/r.git"
        );
        assert_eq!(
            redact_url("https://github.com/o/r.git"),
            "https://github.com/o/r.git"
        );
    }

    #[test]
    fn installs_a_plugin_tree_from_a_local_dir() {
        let base =
            std::env::temp_dir().join(format!("laya-plugin-install-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src/plugins/demo");
        let root = base.join("root");
        std::fs::create_dir_all(src.join("page")).unwrap();
        std::fs::write(
            src.join("plugin.json"),
            r#"{"name":"demo","version":"1.2.3","entry":"main.rhai"}"#,
        )
        .unwrap();
        std::fs::write(src.join("main.rhai"), "fn run(host, ctx) { #{ ok: true } }").unwrap();
        std::fs::write(src.join("page/p.js"), "1").unwrap();
        // A stray .git dir is skipped, never copied.
        std::fs::create_dir_all(src.join(".git")).unwrap();
        std::fs::write(src.join(".git/HEAD"), "x").unwrap();

        let out = install_from_dir(&src, &root, "demo", false).unwrap();
        assert_eq!(out.version, "1.2.3");
        assert_eq!(out.files, 3); // plugin.json + main.rhai + page/p.js
        assert!(root.join("demo/plugin.json").is_file());
        assert!(root.join("demo/page/p.js").is_file());
        assert!(!root.join("demo/.git").exists());

        // No silent overwrite…
        let err = install_from_dir(&src, &root, "demo", false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("already installed"), "{err}");
        // …unless forced.
        assert!(install_from_dir(&src, &root, "demo", true).is_ok());

        // A source without plugin.json is refused.
        let empty = base.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(install_from_dir(&empty, &root, "nope", false).is_err());

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn discovery_lists_bundled_plugins_once() {
        let found = discover_plugins();
        let names: Vec<String> = found.iter().map(|e| e.name.clone()).collect();
        assert!(names.iter().any(|n| n == "alphaxiv"), "{names:?}");
        assert!(names.iter().any(|n| n == "textdigest"), "{names:?}");
        // A name is never listed twice (higher layers shadow lower ones).
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "{names:?}");
        // The description/version parsed out of plugin.json survive to the listing.
        let ax = found.iter().find(|e| e.name == "alphaxiv").unwrap();
        assert!(!ax.version.is_empty());
        assert!(!ax.description.is_empty());
    }

    #[test]
    fn host_file_io_is_policy_gated() {
        let dir = std::env::temp_dir().join(format!("laya-plugin-fs-{}", std::process::id()));
        let plugin_dir = dir.join("mini");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"name":"mini","entry_op":"run"}"#,
        )
        .unwrap();
        std::fs::write(
            plugin_dir.join("main.rhai"),
            r#"
            fn run(host, ctx) {
                let path = ctx["with"]["path"];
                let wrote = host.write_file(path, "hello");
                let back  = host.read_file(path);
                #{ wrote: wrote, read: back }
            }
            "#,
        )
        .unwrap();
        let cap = PluginCap {
            plugin: "mini".to_string(),
            dir: plugin_dir.display().to_string(),
            ..Default::default()
        };
        // A path with no root allowed is denied (fail-closed), nested parents included.
        let target = dir.join("out/deep/x.txt").display().to_string();
        let err = run(
            &cap,
            &json!({ "path": target }),
            &json!({}),
            &Policy::default(),
            None,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("allow_paths"), "{err}");
        assert!(!dir.join("out").exists());

        // With the root allowed the parents are created and text round-trips.
        let mut policy = Policy::default();
        policy.allow_paths = vec![dir.display().to_string()];
        let out = run(
            &cap,
            &json!({ "path": target }),
            &json!({}),
            &policy,
            None,
            None,
        )
        .unwrap();
        assert_eq!(out["read"]["exists"], json!(true));
        assert_eq!(out["read"]["text"], json!("hello"));
        assert!(dir.join("out/deep/x.txt").is_file());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The arXiv reader's site logic is pure in `plan`, so it is unit-testable
    /// without a browser; a bad Rhai edit is caught here, not at run time.
    #[test]
    fn bundled_arxiv_compiles_and_plans() {
        let src = builtin("arxiv").expect("bundled arxiv plugin");
        let m = Manifest::parse(&src.manifest, "arxiv").unwrap();
        assert_eq!(m.name, "arxiv");
        assert_eq!(m.entry_op, "run");
        assert!(build_engine(0).compile(&src.entry).is_ok());
        let mut names: Vec<_> = src.pages.iter().map(|(n, _)| n.as_str()).collect();
        names.sort();
        assert_eq!(names, ["abs.js", "search.js"]);

        let p = |v: Value| call_of("arxiv", "plan", vec![v]).unwrap();
        // No query at all cannot be planned.
        assert!(call_of("arxiv", "plan", vec![json!({})]).is_err());
        // A phrase searches; a bare id / an arxiv.org URL reads that paper.
        let s = p(json!({ "query": "rust async" }));
        assert_eq!(s["mode"], json!("search"));
        assert_eq!(s["limit"], json!(10));
        let a = p(json!({ "query": "2401.12345" }));
        assert_eq!(a["mode"], json!("abs"));
        assert_eq!(a["id"], json!("2401.12345"));
        let a = p(json!({ "query": "https://arxiv.org/abs/2401.12345v2" }));
        assert_eq!(a["mode"], json!("abs"));
        assert_eq!(a["id"], json!("2401.12345v2"));
        let a = p(json!({ "paper": "2401.12345" }));
        assert_eq!(a["mode"], json!("abs"));
        assert_eq!(a["id"], json!("2401.12345"));
        // An explicit search mode keeps its bounded row count.
        let s = p(json!({ "query": "graph neural nets", "mode": "search", "count": 3 }));
        assert_eq!(s["mode"], json!("search"));
        assert_eq!(s["limit"], json!(3));
        // An unsupported mode fails loudly.
        assert!(call_of("arxiv", "plan", vec![json!({ "mode": "nope" })]).is_err());
    }

    /// The HN reader's site logic is pure in `plan`, so it is unit-testable
    /// without a browser; a bad Rhai edit is caught here, not at run time.
    #[test]
    fn bundled_hackernews_compiles_and_plans() {
        let src = builtin("hackernews").expect("bundled hackernews plugin");
        let m = Manifest::parse(&src.manifest, "hackernews").unwrap();
        assert_eq!(m.name, "hackernews");
        assert_eq!(m.entry_op, "run");
        assert!(build_engine(0).compile(&src.entry).is_ok());
        let mut names: Vec<_> = src.pages.iter().map(|(n, _)| n.as_str()).collect();
        names.sort();
        assert_eq!(names, ["front.js", "story.js"]);

        let p = |v: Value| call_of("hackernews", "plan", vec![v]).unwrap();
        // No query -> the top feed, default 30 rows.
        let t = p(json!({}));
        assert_eq!(t["mode"], json!("top"));
        assert_eq!(t["limit"], json!(30));
        // Feeds by name, including aliases.
        for (q, m) in [
            ("best", "best"),
            ("new", "new"),
            ("newest", "new"),
            ("ask", "ask"),
            ("show hn", "show"),
            ("jobs", "jobs"),
            ("front", "top"),
            ("hot", "best"),
        ] {
            assert_eq!(p(json!({ "query": q }))["mode"], json!(m), "{q}");
        }
        // A plain phrase is a full-text search; an item id / URL is a discussion.
        assert_eq!(p(json!({ "query": "rust async" }))["mode"], json!("search"));
        let it = p(json!({ "query": "https://news.ycombinator.com/item?id=42" }));
        assert_eq!(it["mode"], json!("item"));
        assert_eq!(it["id"], json!("42"));
        let it = p(json!({ "id": "123", "count": 5 }));
        assert_eq!(it["mode"], json!("item"));
        assert_eq!(it["id"], json!("123"));
        assert_eq!(it["limit"], json!(5));
        // An unsupported mode fails loudly.
        assert!(call_of("hackernews", "plan", vec![json!({ "mode": "nope" })]).is_err());
    }

    #[test]
    fn bundled_hf_trending_compiles_and_plans() {
        // A compile failure (bad Rhai syntax) must be caught here, not at run time.
        let src = builtin("hf-trending").expect("bundled hf-trending plugin");
        let m = Manifest::parse(&src.manifest, "hf-trending").unwrap();
        assert_eq!(m.name, "hf-trending");
        assert_eq!(m.entry_op, "run");
        assert!(build_engine(0).compile(&src.entry).is_ok());

        let p = |v: Value| call_of("hf-trending", "plan", vec![v]).unwrap();
        // No query → the trending feed, default 10.
        let t = p(json!({}));
        assert_eq!(t["mode"], json!("trending"));
        assert_eq!(t["limit"], json!(10));
        assert_eq!(t["search"], json!(""));
        assert_eq!(t["sort"], json!("trendingScore"));
        // Trending words (english + chinese) keep the trending mode.
        for q in ["trending", "Trending models", "热门", "最新"] {
            assert_eq!(p(json!({ "query": q }))["mode"], json!("trending"), "{q}");
        }
        // A plain phrase becomes a search, and the phrase is passed through.
        let s = p(json!({ "query": "llm memory" }));
        assert_eq!(s["mode"], json!("search"));
        assert_eq!(s["search"], json!("llm memory"));
        // limit is parsed from strings and clipped to 1..=100.
        assert_eq!(p(json!({ "limit": "250" }))["limit"], json!(100));
        assert_eq!(p(json!({ "limit": 0 }))["limit"], json!(1));
        // Explicit mode wins; a search with no query is an error.
        assert_eq!(
            p(json!({ "query": "x", "mode": "trending" }))["mode"],
            json!("trending")
        );
        assert!(call_of(
            "hf-trending",
            "plan",
            vec![json!({ "query": "x", "mode": "bogus" })]
        )
        .is_err());
        assert!(call_of("hf-trending", "plan", vec![json!({ "mode": "search" })]).is_err());
    }

    #[test]
    fn hf_trending_builds_the_api_url() {
        let plan = |v: Value| call_of("hf-trending", "plan", vec![v]).unwrap();
        let url = |v: Value| {
            call_with_host("hf-trending", "api_url", plan(v))
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        };
        let u = url(json!({ "query": "trending", "limit": 5 }));
        assert!(u.starts_with("https://huggingface.co/api/models?"), "{u}");
        assert!(u.contains("sort=trendingScore"), "{u}");
        assert!(u.contains("direction=-1"), "{u}");
        assert!(u.contains("limit=5"), "{u}");
        assert!(u.contains("full=true"), "{u}");
        assert!(!u.contains("search="), "{u}");
        // A search phrase is url-encoded, not appended raw.
        let s = url(json!({ "query": "llm memory" }));
        assert!(s.contains("search=llm%20memory"), "{s}");
    }

    #[test]
    fn hf_trending_renders_timestamps_in_the_users_zone() {
        // A pinned offset makes the rendering deterministic (no machine zone).
        let p = call_of(
            "hf-trending",
            "plan",
            vec![json!({ "tz_offset_minutes": 480, "tz": "Asia/Shanghai" })],
        )
        .unwrap();
        assert_eq!(p["tz_offset_minutes"], json!(480));
        assert_eq!(p["tz"], json!("Asia/Shanghai"));
        // No override ⇒ unset, so the plugin falls back to the runner's zone.
        let d = call_of("hf-trending", "plan", vec![json!({})]).unwrap();
        assert_eq!(d["tz_offset_minutes"], json!(null));
        assert_eq!(d["tz"], json!(""));

        // Upstream UTC → the same instant in the caller's zone (offset-exact).
        let at =
            |iso: Value, off: i64| call_host2("hf-trending", "local_iso", iso, json!(off)).unwrap();
        assert_eq!(
            at(json!("2026-09-24T03:39:57.000Z"), 8 * 3600),
            json!("2026-09-24T11:39:57.000+08:00")
        );
        assert_eq!(
            at(json!("2026-09-24T03:39:57.000Z"), 0),
            json!("2026-09-24T03:39:57.000Z")
        );
        // Unparseable / empty input yields "" so the raw field stays authoritative.
        assert_eq!(at(json!(""), 0), json!(""));
        assert_eq!(at(json!("not-a-date"), 0), json!(""));

        // The report's short form drops the zone and the seconds.
        assert_eq!(
            call_of(
                "hf-trending",
                "short_local",
                vec![json!("2026-09-24T11:39:57+08:00")]
            )
            .unwrap(),
            json!("2026-09-24 11:39")
        );
    }
}
