//! Rhai plugin host — the engine's extension seam.
//!
//! The Rust code is the **base**: CDP transport, policy gates, resource
//! lifecycle, secret redaction, page-to-Markdown rendering. Anything that is
//! *site-* or *task*-specific (which selector to read, which endpoint to page,
//! how a natural-language query maps to a mode) belongs in a **plugin** written
//! in Rhai — a site under `websites/<domain>/plugin/`, a tool under
//! `plugins/<name>/`.
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
//! Layout: a **site** gets a folder `websites/<domain>/` that holds its plugin
//! under `websites/<domain>/plugin/` (the site may also keep docs, fixtures or
//! several plugins there later); non-site/tool plugins live under
//! `plugins/<name>/`. Both are directories of the same shape.
//!
//! Every plugin has a `group/name` id (e.g. `websites/hackernews`,
//! `plugins/textdigest`): `name` is the plugin's `plugin.json` name and `group`
//! is its `"group"` field, defaulting to the root it was found under
//! (`websites` / `plugins`). A bare name still resolves as an alias, but a
//! grouped id only matches its own group.
//!
//! Plugins are resolved in layers, highest priority first: an explicit `dir` →
//! `$LAYA_PLUGIN_DIR/<name>` → `plugins/<name>` / `websites/*/plugin/` walking up to
//! the git root → `~/.laya-workflow/{plugins,websites}` (the
//! `plugin install` default) → the copy compiled into this binary. That last
//! layer is why the bundled alphaXiv downloader still works after
//! `sudo install`-ing a single binary.

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

/// Name of the bundled 闲鱼 / goofish.com plugin (search, browse, watch).
pub const GOOFISH_PLUGIN: &str = "goofish";

/// A `kind: "plugin"` capability: which plugin to load and how to bound it.
#[derive(Clone, Debug, Default)]
pub struct PluginCap {
    /// Plugin id, `group/name` (a bare `name` is also accepted): matched against
    /// the manifest `name` + `group` (or the root class) under `plugins/` and
    /// `websites/*/plugin/`. A path is used instead when `dir` is set.
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
    /// Optional namespace segment: the plugin id is `group/name` when set.
    group: String,
    version: String,
    description: String,
    entry: String,
    entry_op: String,
    max_operations: u64,
}

impl Manifest {
    /// The plugin's canonical id: `group/name` when it declares a group, else
    /// the bare `name`.
    fn id(&self) -> String {
        if self.group.is_empty() {
            self.name.clone()
        } else {
            format!("{}/{}", self.group, self.name)
        }
    }

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
            group: v
                .get("group")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
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
/// Split a plugin id into `(group, name)`; a bare name has an empty group.
fn split_id(id: &str) -> (&str, &str) {
    match id.trim().rfind('/') {
        Some(i) => (&id.trim()[..i], &id.trim()[i + 1..]),
        None => ("", id.trim()),
    }
}

/// The plugin name inside an id (`websites/hackernews` → `hackernews`).
fn bare_name(id: &str) -> &str {
    split_id(id).1
}

/// The name a plugin id should be *installed* under: the bare name segment.
///
/// Discovery reports ids as `group/name` (`websites/hackernews`), but a
/// plugin lives in a single directory, so installing under the full id would
/// create a `plugins/websites/` tree and then fail name validation. Exposed for
/// `install`, which turns every discovered plugin into a directory.
pub fn install_name_for(id: &str) -> &str {
    bare_name(id)
}

/// The group a plugin root stands for: `websites` or `plugins` roots map to that
/// class, any other root name yields no default group.
fn root_class(root: &std::path::Path) -> Option<&str> {
    match root.file_name().and_then(|n| n.to_str()) {
        Some("websites") => Some("websites"),
        Some("plugins") => Some("plugins"),
        _ => None,
    }
}

/// Resolve a child's canonical id from its manifest, defaulting the group to the
/// root's class when the manifest omits one (e.g. `websites/<domain>/plugin/`
/// with no `"group"` still reads as `websites/<name>`).
fn child_id(manifest: &Manifest, root: &std::path::Path) -> String {
    if !manifest.group.is_empty() {
        return format!("{}/{}", manifest.group, manifest.name);
    }
    match root_class(root) {
        Some(class) => format!("{class}/{}", manifest.name),
        None => manifest.name.clone(),
    }
}

fn builtin(name: &str) -> Option<Sources> {
    let (manifest, entry, pages): (&str, &str, Vec<(&str, &str)>) = match name {
        ALPHAXIV_PLUGIN => (
            include_str!("../../websites/alphaxiv.org/plugin/plugin.json"),
            include_str!("../../websites/alphaxiv.org/plugin/main.rhai"),
            vec![
                (
                    "links.js",
                    include_str!("../../websites/alphaxiv.org/plugin/page/links.js"),
                ),
                (
                    "feed.js",
                    include_str!("../../websites/alphaxiv.org/plugin/page/feed.js"),
                ),
                (
                    "overview.js",
                    include_str!("../../websites/alphaxiv.org/plugin/page/overview.js"),
                ),
            ],
        ),
        "goofish" => (
            include_str!("../../websites/goofish.com/plugin/plugin.json"),
            include_str!("../../websites/goofish.com/plugin/main.rhai"),
            vec![
                (
                    "search.js",
                    include_str!("../../websites/goofish.com/plugin/page/search.js"),
                ),
                (
                    "item.js",
                    include_str!("../../websites/goofish.com/plugin/page/item.js"),
                ),
            ],
        ),
        "textdigest" => (
            include_str!("../../plugins/textdigest/plugin.json"),
            include_str!("../../plugins/textdigest/main.rhai"),
            Vec::new(),
        ),
        "browser_base" => (
            include_str!("../../plugins/browser_base/plugin.json"),
            include_str!("../../plugins/browser_base/main.rhai"),
            Vec::new(),
        ),
        // The BDD step vocabulary, bundled for the same reason browser_base is:
        // on-disk discovery finds plugins/<name> only by walking up from the
        // cwd, so without this an installed binary - run from anywhere but a
        // checkout - reports `plugin "bdd" not found` for a plugin the repo
        // documents as standard.
        "bdd" => (
            include_str!("../../plugins/bdd/plugin.json"),
            include_str!("../../plugins/bdd/main.rhai"),
            Vec::new(),
        ),
        "hf-trending" => (
            include_str!("../../websites/huggingface.co/plugin/plugin.json"),
            include_str!("../../websites/huggingface.co/plugin/main.rhai"),
            Vec::new(),
        ),
        "hackernews" => (
            include_str!("../../websites/news.ycombinator.com/plugin/plugin.json"),
            include_str!("../../websites/news.ycombinator.com/plugin/main.rhai"),
            vec![
                (
                    "front.js",
                    include_str!("../../websites/news.ycombinator.com/plugin/page/front.js"),
                ),
                (
                    "story.js",
                    include_str!("../../websites/news.ycombinator.com/plugin/page/story.js"),
                ),
            ],
        ),
        "arxiv" => (
            include_str!("../../websites/arxiv.org/plugin/plugin.json"),
            include_str!("../../websites/arxiv.org/plugin/main.rhai"),
            vec![
                (
                    "search.js",
                    include_str!("../../websites/arxiv.org/plugin/page/search.js"),
                ),
                (
                    "abs.js",
                    include_str!("../../websites/arxiv.org/plugin/page/abs.js"),
                ),
            ],
        ),
        "wikipedia" => (
            include_str!("../../websites/wikipedia.org/plugin/plugin.json"),
            include_str!("../../websites/wikipedia.org/plugin/main.rhai"),
            vec![
                (
                    "search.js",
                    include_str!("../../websites/wikipedia.org/plugin/page/search.js"),
                ),
                (
                    "clean.js",
                    include_str!("../../websites/wikipedia.org/plugin/page/clean.js"),
                ),
            ],
        ),
        "mdn" => (
            include_str!("../../websites/developer.mozilla.org/plugin/plugin.json"),
            include_str!("../../websites/developer.mozilla.org/plugin/main.rhai"),
            Vec::new(),
        ),
        "bing" => (
            include_str!("../../websites/bing.com/plugin/plugin.json"),
            include_str!("../../websites/bing.com/plugin/main.rhai"),
            vec![(
                "search.js",
                include_str!("../../websites/bing.com/plugin/page/search.js"),
            )],
        ),
        "v2ex" => (
            include_str!("../../websites/v2ex.com/plugin/plugin.json"),
            include_str!("../../websites/v2ex.com/plugin/main.rhai"),
            vec![
                (
                    "list.js",
                    include_str!("../../websites/v2ex.com/plugin/page/list.js"),
                ),
                (
                    "topic.js",
                    include_str!("../../websites/v2ex.com/plugin/page/topic.js"),
                ),
            ],
        ),
        "crates" => (
            include_str!("../../websites/crates.io/plugin/plugin.json"),
            include_str!("../../websites/crates.io/plugin/main.rhai"),
            vec![(
                "search.js",
                include_str!("../../websites/crates.io/plugin/page/search.js"),
            )],
        ),
        "pypi" => (
            include_str!("../../websites/pypi.org/plugin/plugin.json"),
            include_str!("../../websites/pypi.org/plugin/main.rhai"),
            vec![
                (
                    "search.js",
                    include_str!("../../websites/pypi.org/plugin/page/search.js"),
                ),
                (
                    "project.js",
                    include_str!("../../websites/pypi.org/plugin/page/project.js"),
                ),
            ],
        ),
        "docsrs" => (
            include_str!("../../websites/docs.rs/plugin/plugin.json"),
            include_str!("../../websites/docs.rs/plugin/main.rhai"),
            vec![(
                "search.js",
                include_str!("../../websites/docs.rs/plugin/page/search.js"),
            )],
        ),
        "github" => (
            include_str!("../../websites/github.com/plugin/plugin.json"),
            include_str!("../../websites/github.com/plugin/main.rhai"),
            vec![
                (
                    "trending.js",
                    include_str!("../../websites/github.com/plugin/page/trending.js"),
                ),
                (
                    "repo.js",
                    include_str!("../../websites/github.com/plugin/page/repo.js"),
                ),
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

/// Bare names of the plugins compiled into this binary (their grouped ids come
/// from each bundled `plugin.json` and are reported by [`discover_plugins`]).
pub fn builtin_names() -> &'static [&'static str] {
    &[
        ALPHAXIV_PLUGIN,
        "textdigest",
        "browser_base",
        "hf-trending",
        "hackernews",
        "arxiv",
        "wikipedia",
        "mdn",
        "bing",
        "v2ex",
        "crates",
        "pypi",
        "docsrs",
        "github",
    ]
}

/// Write a plugin compiled into the binary into `root`, returning the files written.
///
/// Builtin plugins are normally used straight out of the embedded copy, so this
/// is only needed to make them *editable* — `install` does it so a machine that
/// has the binary but no checkout still ends up with an inspectable, patchable
/// plugin tree rather than a set that can only ever be replaced by
/// reinstalling a different binary.
///
/// Returns `Ok(None)` when the plugin is not a builtin, so a caller can fall
/// through to a different source.
pub fn install_builtin_to(name: &str, root: &std::path::Path, force: bool) -> Result<Option<InstalledPlugin>> {
    let Some(src) = builtin(name) else {
        return Ok(None);
    };
    validate_plugin_name(name)?;
    let dest = root.join(name);
    if dest.exists() {
        if !force {
            return Ok(None);
        }
        std::fs::remove_dir_all(&dest)
            .map_err(|e| anyhow!("cannot clear {}: {e}", dest.display()))?;
    }
    let m = Manifest::parse(&src.manifest, name)?;
    std::fs::create_dir_all(&dest)?;
    let mut files = 0usize;
    let mut write = |rel: &str, body: &str| -> Result<()> {
        let path = dest.join(rel);
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(&path, body)?;
        files += 1;
        Ok(())
    };
    write("plugin.json", &src.manifest)?;
    // `Sources::entry_name` is empty for a builtin (it is only filled in when a
    // plugin is read off disk); the manifest is where the entry file is named.
    write(&m.entry, &src.entry)?;
    for (page, body) in &src.pages {
        write(&format!("page/{page}"), body)?;
    }
    Ok(Some(InstalledPlugin {
        name: name.to_string(),
        version: m.version,
        dest,
        files,
    }))
}

// ── installing plugins from a git repo ──────────────────────────────

/// Per-user plugin root: `$LAYA_USER_PLUGIN_DIR` → `~/.laya-workflow/plugins`.
/// Mirrors `spec::user_spec_dir()` — both live under the tool's single state
/// root; `plugin install` writes here unless `$LAYA_PLUGIN_DIR` (or `--root`)
/// overrides it.
pub fn user_plugin_dir() -> Option<PathBuf> {
    crate::state::user_plugin_dir()
}

/// Where `plugin install` writes: `$LAYA_PLUGIN_DIR` when set (so a checkout can
/// pin installs next to the repo), otherwise the per-user plugin root.
pub fn install_root() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("LAYA_PLUGIN_DIR") {
        if !dir.trim().is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    user_plugin_dir()
        .ok_or_else(|| anyhow!("cannot pick an install root: set LAYA_PLUGIN_DIR or LAYA_HOME/HOME"))
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
        bail!("--path must name a directory inside the repo (e.g. websites/alphaxiv.org)");
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
        // A child directory is a plugin root (the plugin may sit in it directly
        // or in its `plugin/` subdir); its *name* is what `plugin.json` declares
        // (so `websites/<domain>/` maps to a short plugin id).
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for child in dirs {
            let Some(dir) = plugin_dir_of(&child) else {
                continue;
            };
            let folder = child
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let manifest = std::fs::read_to_string(dir.join("plugin.json"))
                .ok()
                .and_then(|s| Manifest::parse(&s, &folder).ok());
            let name = manifest
                .as_ref()
                .map(|m| child_id(m, &root))
                .unwrap_or_else(|| folder.clone());
            let (version, description) = manifest
                .map(|m| (m.version, m.description))
                .unwrap_or_else(|| ("0.0.0".to_string(), String::new()));
            if !seen.insert(name.clone()) {
                continue;
            }
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
        let (name, version, description) = builtin(n)
            .and_then(|s| Manifest::parse(&s.manifest, n).ok())
            .map(|m| (m.id(), m.version, m.description))
            .unwrap_or_else(|| (n.to_string(), "0.0.0".to_string(), String::new()));
        if seen.insert(name.clone()) {
            out.push(PluginEntry {
                name,
                layer: PluginLayer::Builtin,
                version,
                description,
                path: None,
            });
        }
    }
    out
}

/// Default plugin name for an install: a bare `plugin/` subdir takes its parent
/// folder's name (`websites/alphaxiv.org/plugin` → `alphaxiv.org`), otherwise
/// the last segment of the checkout path. The manifest `name` normally wins.
fn plugin_name_fallback(plugin_dir: &std::path::Path, sub: &str) -> String {
    if plugin_dir.file_name().and_then(|n| n.to_str()) == Some("plugin") {
        if let Some(parent) = plugin_dir
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty())
        {
            return parent.to_string();
        }
    }
    sub.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string()
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
    if !name.trim().is_empty() {
        validate_plugin_name(name)?;
    }
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
        // The plugin may be the checked-out dir itself (`plugins/<name>/`,
        // `websites/<domain>/plugin/`) or a site folder's `plugin/` subdir
        // (`websites/<domain>/`).
        let plugin_dir = plugin_dir_of(&src).unwrap_or_else(|| src.clone());
        let name = if name.trim().is_empty() {
            let fallback = plugin_name_fallback(&plugin_dir, &sub);
            std::fs::read_to_string(plugin_dir.join("plugin.json"))
                .ok()
                .and_then(|m| Manifest::parse(&m, &fallback).ok())
                .map(|m| m.name)
                .filter(|n| !n.trim().is_empty() && validate_plugin_name(n).is_ok())
                .unwrap_or(fallback)
        } else {
            name.trim().to_string()
        };
        install_from_dir(&plugin_dir, root, &name, force)
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
    /// `plugins/<name>` or `websites/<domain>/plugin/` walking up from the cwd
    /// to the git root.
    Repo,
    /// `~/.laya-workflow/plugins/<name>` (where `plugin install` lands)
    /// or its `websites/` sibling.
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

/// Candidate on-disk plugin roots, highest priority first: `$LAYA_PLUGIN_DIR` →
/// `plugins/` and `websites/` up to the git root → the per-user install root
/// (with its `websites/` sibling).
///
/// A root is a directory whose children are plugin directories. Under `plugins/`
/// the child is named after the plugin; under `websites/<domain>/` the child is
/// named after the site and its plugin lives in a `plugin/` subdir (the plugin
/// name comes from `plugin/plugin.json`). Both are found by [`plugin_dir_in`].
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
        roots.push((PluginLayer::Repo, dir.join("websites")));
        if dir.join(".git").exists() {
            break;
        }
        cur = dir.parent().map(std::path::Path::to_path_buf);
    }
    if let Some(user) = user_plugin_dir() {
        roots.push((PluginLayer::User, user.clone()));
        if let Some(parent) = user.parent() {
            roots.push((PluginLayer::User, parent.join("websites")));
        }
    }
    roots
}

/// The directory a plugin root child actually stores the plugin in: the child
/// itself (`plugins/<name>/`), or its `plugin/` subdirectory — the layout of a
/// site folder (`websites/<domain>/plugin/`). `None` when the child holds no
/// `plugin.json` either way.
fn plugin_dir_of(child: &std::path::Path) -> Option<PathBuf> {
    if child.join("plugin.json").is_file() {
        return Some(child.to_path_buf());
    }
    let nested = child.join("plugin");
    if nested.join("plugin.json").is_file() {
        return Some(nested);
    }
    None
}

/// The plugin directory under `root` that provides `name`: a child named after
/// the plugin (`plugins/<name>/`), or a site folder whose plugin sits in a
/// `plugin/` subdir and whose `plugin.json` declares that name
/// (`websites/<domain>/plugin/`). `None` when the root has no such plugin.
fn plugin_dir_in(root: &std::path::Path, id: &str) -> Option<PathBuf> {
    let (want_group, want_name) = split_id(id);
    if want_name.is_empty() {
        return None;
    }
    // Fast path: a bare id is a child directory named after the plugin.
    if want_group.is_empty() {
        if let Some(dir) = plugin_dir_of(&root.join(want_name)) {
            return Some(dir);
        }
    }
    let mut children: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    children.sort();
    for child in children {
        let Some(dir) = plugin_dir_of(&child) else {
            continue;
        };
        let folder = child
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(want_name)
            .to_string();
        if want_group.is_empty() && folder == want_name {
            return Some(dir);
        }
        // The folder name is only a fallback: a bare id resolves by the
        // manifest name, a grouped id by the manifest group (or the root class).
        let Ok(src) = std::fs::read_to_string(dir.join("plugin.json")) else {
            continue;
        };
        let Ok(m) = Manifest::parse(&src, &folder) else {
            continue;
        };
        if m.name != want_name {
            continue;
        }
        if want_group.is_empty() || m.group == want_group || root_class(root) == Some(want_group) {
            return Some(dir);
        }
    }
    None
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
        if let Some(d) = plugin_dir_in(&root, &cap.plugin) {
            return read_dir_sources(&d, &cap.plugin, entry);
        }
    }
    builtin(bare_name(&cap.plugin)).ok_or_else(|| {
        anyhow!(
            "plugin {:?} not found (looked in $LAYA_PLUGIN_DIR, plugins/ and websites/ above the cwd, and the built-ins)",
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
        "plugin": manifest.id(),
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
            3 => engine.call_fn(
                &mut scope,
                &ast,
                name,
                (dyns[0].clone(), dyns[1].clone(), dyns[2].clone()),
            ),
            4 => engine.call_fn(
                &mut scope,
                &ast,
                name,
                (
                    dyns[0].clone(),
                    dyns[1].clone(),
                    dyns[2].clone(),
                    dyns[3].clone(),
                ),
            ),
            5 => engine.call_fn(
                &mut scope,
                &ast,
                name,
                (
                    dyns[0].clone(),
                    dyns[1].clone(),
                    dyns[2].clone(),
                    dyns[3].clone(),
                    dyns[4].clone(),
                ),
            ),
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

    /// Call a function whose first parameter is the host handle, with 4 more
    /// args (`observe(host, wdata, rec, now, hist_max)` and friends).
    fn call_host4(
        plugin: &str,
        name: &str,
        a: Value,
        b: Value,
        c: Value,
        d: Value,
    ) -> Result<Value> {
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
                    to_dyn(c).map_err(|e| anyhow!("{e}"))?,
                    to_dyn(d).map_err(|e| anyhow!("{e}"))?,
                ),
            )
            .map_err(|e| anyhow!("{name}(): {e}"))?;
        from_dyn(out).map_err(|e| anyhow!("{e}"))
    }

    fn call(name: &str, args: Vec<Value>) -> Result<Value> {
        call_of(ALPHAXIV_PLUGIN, name, args)
    }

    /// Call a bundled goofish function with JSON args and no host interaction.
    fn gs(name: &str, args: Vec<Value>) -> Result<Value> {
        call_of(GOOFISH_PLUGIN, name, args)
    }

    fn gs_plan(with: Value) -> Result<Value> {
        gs("plan", vec![with])
    }

    fn plan(with: Value) -> Result<Value> {
        call("plan", vec![with])
    }

    /// `opt_bool(args, key, default)`: the switch that decides whether the
    /// downloader asks alphaXiv to generate a missing AI Overview.
    fn opt_bool(with: Value, key: &str, dflt: bool) -> Result<Value> {
        call("opt_bool", vec![with, json!(key), json!(dflt)])
    }

    #[test]
    fn overview_generation_switch_is_opt_out_and_null_safe() {
        // Absent → the caller's default.
        assert_eq!(
            opt_bool(json!({}), "generate_overview", true).unwrap(),
            json!(true)
        );
        assert_eq!(
            opt_bool(json!({}), "generate_overview", false).unwrap(),
            json!(false)
        );
        // A `${state.x}` the workflow never resolved arrives as null and must not
        // be read as "false": it has to fall back to the default, or every url
        // run would silently stop generating overviews.
        for null in [json!(null), json!(""), json!("null")] {
            let args = json!({"generate_overview": null});
            assert_eq!(
                opt_bool(args.clone(), "generate_overview", true).unwrap(),
                json!(true),
                "state {null} should keep the default"
            );
            assert_eq!(
                opt_bool(args, "generate_overview", false).unwrap(),
                json!(false)
            );
        }
        // Real booleans and the string spellings a CLI flag can produce.
        for truthy in [
            json!(true),
            json!("true"),
            json!("1"),
            json!("yes"),
            json!("on"),
        ] {
            let args = json!({"generate_overview": truthy});
            assert_eq!(
                opt_bool(args.clone(), "generate_overview", false).unwrap(),
                json!(true),
                "{truthy} should be true"
            );
        }
        for falsy in [
            json!(false),
            json!("false"),
            json!("0"),
            json!("no"),
            json!("off"),
        ] {
            let args = json!({"generate_overview": falsy});
            assert_eq!(
                opt_bool(args.clone(), "generate_overview", true).unwrap(),
                json!(false),
                "{falsy} should be false"
            );
        }
    }

    /// The wait is charged on the wall clock, and its helpers read a page result
    /// without trusting its shape.
    #[test]
    fn overview_reads_page_results_defensively() {
        let ready =
            json!({"state": "ready", "len": 5017, "href": "https://www.alphaxiv.org/zh/abs/1"});
        assert_eq!(
            call("ov_str", vec![ready.clone(), json!("state")]).unwrap(),
            json!("ready")
        );
        assert_eq!(
            call("ov_int", vec![ready.clone(), json!("len"), json!(-1)]).unwrap(),
            json!(5017)
        );
        // Missing keys, a null field, and a non-map all degrade to the default
        // instead of throwing — a page that answers differently must not lose
        // the paper.
        assert_eq!(
            call("ov_str", vec![ready.clone(), json!("nope")]).unwrap(),
            json!("")
        );
        assert_eq!(
            call("ov_str", vec![json!({"state": null}), json!("state")]).unwrap(),
            json!("")
        );
        assert_eq!(
            call("ov_str", vec![json!("not a map"), json!("state")]).unwrap(),
            json!("")
        );
        assert_eq!(
            call("ov_int", vec![json!("not a map"), json!("len"), json!(-1)]).unwrap(),
            json!(-1)
        );
        assert_eq!(
            call("ov_int", vec![ready, json!("len"), json!(-1)]).unwrap(),
            json!(5017)
        );
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

    /// A `websites/<domain>/plugin/` folder is found by the plugin *name* its
    /// `plugin.json` declares, not by the domain folder name.
    #[test]
    fn resolves_a_website_folder_by_manifest_name() {
        let root = std::env::temp_dir().join(format!("laya-web-root-{}", std::process::id()));
        // A site folder keeps its plugin in a `plugin/` subdirectory.
        let site = root.join("example.com");
        let dir = site.join("plugin");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"example","entry":"main.rhai"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("main.rhai"), "fn run(host, ctx) { #{ ok: true } }").unwrap();

        // Found by the manifest name (`websites/<domain>/plugin/` → `example`).
        assert_eq!(
            plugin_dir_in(&root, "example").as_deref(),
            Some(dir.as_path())
        );
        // ...and by the domain folder name too, so both conventions work.
        assert_eq!(
            plugin_dir_in(&root, "example.com").as_deref(),
            Some(dir.as_path())
        );
        assert!(plugin_dir_in(&root, "nope").is_none());

        // A tool plugin stays flat (`plugins/<name>/`) and still resolves.
        let tool = root.join("mini");
        std::fs::create_dir_all(&tool).unwrap();
        std::fs::write(tool.join("plugin.json"), r#"{"name":"mini"}"#).unwrap();
        assert_eq!(
            plugin_dir_in(&root, "mini").as_deref(),
            Some(tool.as_path())
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A plugin root child resolves to itself (`plugins/<name>/`) or to its
    /// `plugin/` subdir (`websites/<domain>/plugin/`); nothing else counts.
    /// `install` must be able to lay down the plugins compiled into the binary,
    /// so a machine with the binary but no checkout still gets a usable,
    /// editable plugin tree. Getting the entry filename from the manifest (not
    /// from `Sources::entry_name`, which is empty for a builtin) is the part
    /// that silently produced a directory named `main.rhai/`.
    #[test]
    fn install_builtin_to_writes_a_runnable_tree() {
        let root = std::env::temp_dir().join(format!("laya-builtin-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        let out = install_builtin_to("browser_base", &root, false)
            .unwrap()
            .expect("browser_base is a builtin");
        assert_eq!(out.name, "browser_base");
        assert_eq!(out.version, "0.1.0");
        assert!(out.dest.join("plugin.json").is_file(), "manifest written");
        assert!(out.dest.join("main.rhai").is_file(), "entry written");
        assert!(out.files >= 2, "expected manifest + entry, got {}", out.files);

        // A page-carrying builtin, so the page/ subdir path is covered too.
        let out = install_builtin_to("hackernews", &root, false)
            .unwrap()
            .expect("hackernews is a builtin");
        assert!(out.dest.join("main.rhai").is_file(), "entry written");
        assert!(out.dest.join("page/front.js").is_file(), "page script written");
        assert!(out.dest.join("page/story.js").is_file(), "page script written");

        // Second call without --force keeps an existing install.
        std::fs::write(out.dest.join("marker"), "x").unwrap();
        assert!(
            install_builtin_to("hackernews", &root, false).unwrap().is_none(),
            "an existing plugin must be left alone without --force"
        );
        assert!(out.dest.join("marker").exists(), "kept without --force");
        // With --force the dir is replaced, marker and all.
        assert!(install_builtin_to("hackernews", &root, true).unwrap().is_some());
        assert!(!out.dest.join("marker").exists(), "--force replaces the tree");

        // Not a builtin: no silent empty install.
        assert!(install_builtin_to("definitely-not-a-plugin", &root, true).unwrap().is_none());

        // The written tree must actually load as a plugin — an install that
        // produces the right filenames but an unloadable script is still broken.
        let cap = PluginCap {
            plugin: "browser_base".to_string(),
            dir: root.join("browser_base").display().to_string(),
            entry: String::new(),
            op: String::new(),
            browser: String::new(),
            max_operations: 0,
            timeout_ms: 0,
        };
        let src = load_sources(&cap).expect("written plugin loads");
        assert_eq!(src.entry_name, "main.rhai");
        assert!(src.entry.contains("fn run"), "entry body preserved");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn plugin_dir_of_finds_flat_and_nested_layouts() {
        let root = std::env::temp_dir().join(format!("laya-pdo-{}", std::process::id()));
        let flat = root.join("mini");
        std::fs::create_dir_all(&flat).unwrap();
        std::fs::write(flat.join("plugin.json"), "{}").unwrap();
        assert_eq!(plugin_dir_of(&flat).as_deref(), Some(flat.as_path()));

        let site = root.join("example.com");
        let nested = site.join("plugin");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("plugin.json"), "{}").unwrap();
        assert_eq!(plugin_dir_of(&site).as_deref(), Some(nested.as_path()));
        assert_eq!(plugin_dir_of(&nested).as_deref(), Some(nested.as_path()));

        let bare = root.join("nothing");
        std::fs::create_dir_all(&bare).unwrap();
        assert!(plugin_dir_of(&bare).is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    /// The install name falls back to the site folder for a bare `plugin/`
    /// subdir, and to the path's last segment otherwise.
    #[test]
    fn install_name_falls_back_to_the_site_folder() {
        let site_plugin = std::path::Path::new("websites/alphaxiv.org/plugin");
        assert_eq!(
            plugin_name_fallback(site_plugin, "websites/alphaxiv.org/plugin"),
            "alphaxiv.org"
        );
        let plain = std::path::Path::new("/tmp/clone/plugins/v2ex");
        assert_eq!(plugin_name_fallback(plain, "plugins/v2ex"), "v2ex");
    }

    /// A plugin id is `group/name`; resolution accepts the grouped id, and the
    /// group may come from the manifest or from the root class (`websites`).
    #[test]
    fn resolves_grouped_plugin_ids() {
        let base = std::env::temp_dir().join(format!("laya-group-{}", std::process::id()));
        // A `websites/` root: its children are site folders, the plugin sits in
        // each `<domain>/plugin/`, and the group defaults to `websites`.
        let websites = base.join("websites");
        let site = websites.join("example.com").join("plugin");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::write(
            site.join("plugin.json"),
            r#"{"name":"example","group":"websites","entry":"main.rhai"}"#,
        )
        .unwrap();
        std::fs::write(
            site.join("main.rhai"),
            "fn run(host, ctx) { #{ ok: true } }",
        )
        .unwrap();

        assert_eq!(
            plugin_dir_in(&websites, "websites/example").as_deref(),
            Some(site.as_path())
        );
        assert_eq!(
            plugin_dir_in(&websites, "example").as_deref(),
            Some(site.as_path())
        );
        assert!(plugin_dir_in(&websites, "plugins/example").is_none());
        assert!(plugin_dir_in(&websites, "websites/nope").is_none());

        // No manifest group: the root class still names the group.
        let site2 = websites.join("other.org").join("plugin");
        std::fs::create_dir_all(&site2).unwrap();
        std::fs::write(site2.join("plugin.json"), r#"{"name":"other"}"#).unwrap();
        assert_eq!(
            plugin_dir_in(&websites, "websites/other").as_deref(),
            Some(site2.as_path())
        );

        // `child_id` / `Manifest::id` agree with what resolution accepts.
        let m = Manifest::parse(r#"{"name":"example","group":"websites"}"#, "example").unwrap();
        assert_eq!(m.id(), "websites/example");
        assert_eq!(child_id(&m, &websites), "websites/example");
        let bare = Manifest::parse(r#"{"name":"other"}"#, "other").unwrap();
        assert_eq!(bare.id(), "other");
        assert_eq!(child_id(&bare, &websites), "websites/other");
        std::fs::remove_dir_all(&base).ok();
    }

    /// From the crate root, a bundled site plugin resolves off
    /// `websites/<domain>/plugin/` on disk (not from the compiled-in copy)
    /// under its short id.
    #[test]
    fn finds_a_repo_plugin_under_websites_by_name() {
        let probe = std::path::Path::new("websites/news.ycombinator.com/plugin/plugin.json");
        if !probe.exists() {
            return; // not run from the repo root
        }
        let src = load_sources(&PluginCap {
            plugin: "hackernews".to_string(),
            ..Default::default()
        })
        .unwrap();
        let dir = src.dir.expect("resolved from disk, not builtin");
        assert!(
            dir.ends_with("websites/news.ycombinator.com/plugin"),
            "{}",
            dir.display()
        );
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
            normalize_subdir("websites/alphaxiv.org").unwrap(),
            "websites/alphaxiv.org"
        );
        assert_eq!(
            normalize_subdir("/websites/alphaxiv.org/").unwrap(),
            "websites/alphaxiv.org"
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
        // Names are grouped: `group/plugin`.
        assert!(names.iter().any(|n| n == "websites/alphaxiv"), "{names:?}");
        assert!(names.iter().any(|n| n == "plugins/textdigest"), "{names:?}");
        // A name is never listed twice (higher layers shadow lower ones).
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "{names:?}");
        // The description/version parsed out of plugin.json survive to the listing.
        let ax = found
            .iter()
            .find(|e| e.name == "websites/alphaxiv")
            .unwrap();
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

    #[test]
    fn bundled_browser_base_compiles_and_guards() {
        let src = builtin("browser_base").expect("bundled browser_base plugin");
        let m = Manifest::parse(&src.manifest, "browser_base").unwrap();
        assert_eq!(m.name, "browser_base");
        assert_eq!(m.group, "plugins");
        assert_eq!(m.entry_op, "run");
        // The whole script must compile on the same engine the host builds.
        assert!(build_engine(0).compile(&src.entry).is_ok());
        // All four ops are present; `run` is a guard that tells you to pick one.
        for op in ["open", "evaluate", "wait_htmx", "assert"] {
            assert!(
                build_engine(0)
                    .compile(&src.entry)
                    .unwrap()
                    .iter_functions()
                    .any(|f| f.name.to_string() == op),
                "browser_base must define op {op}"
            );
        }
        let guard = call_with_host("browser_base", "run", json!({ "with": {} }));
        assert!(guard.is_err(), "run() must reject direct invocation");
    }

    fn bundled_v2ex_compiles_and_plans() {
        let src = builtin("v2ex").expect("bundled v2ex plugin");
        let m = Manifest::parse(&src.manifest, "v2ex").unwrap();
        assert_eq!(m.name, "v2ex");
        assert!(build_engine(0).compile(&src.entry).is_ok());
        let mut names: Vec<_> = src.pages.iter().map(|(n, _)| n.as_str()).collect();
        names.sort();
        assert_eq!(names, ["list.js", "topic.js"]);

        let p = |v: Value| call_of("v2ex", "plan", vec![v]).unwrap();
        // No query -> the hot tab.
        assert_eq!(p(json!({}))["mode"], json!("hot"));
        for tab in [
            "latest", "tech", "creative", "play", "apple", "jobs", "deals", "city", "qna", "all",
        ] {
            assert_eq!(p(json!({ "query": tab }))["mode"], json!(tab), "{tab}");
        }
        assert_eq!(p(json!({ "query": "new" }))["mode"], json!("latest"));
        // A topic id / URL reads a topic; a node lists that node.
        let t = p(json!({ "query": "https://www.v2ex.com/t/1245140#reply140" }));
        assert_eq!(t["mode"], json!("topic"));
        assert_eq!(t["id"], json!("1245140"));
        assert_eq!(p(json!({ "query": "1245140" }))["mode"], json!("topic"));
        let n = p(json!({ "query": "go/rust" }));
        assert_eq!(n["mode"], json!("node"));
        assert_eq!(n["node"], json!("rust"));
        assert_eq!(p(json!({ "node": "python" }))["node"], json!("python"));
        // A bare unknown word cannot be planned.
        assert!(call_of("v2ex", "plan", vec![json!({ "query": "hello world" })]).is_err());
        // The limit is capped.
        assert_eq!(
            p(json!({ "query": "hot", "count": 999 }))["limit"],
            json!(100)
        );
    }

    #[test]
    fn bundled_crates_compiles_and_plans() {
        let src = builtin("crates").expect("bundled crates plugin");
        let m = Manifest::parse(&src.manifest, "crates").unwrap();
        assert_eq!(m.name, "crates");
        assert_eq!(m.entry_op, "run");
        assert!(build_engine(0).compile(&src.entry).is_ok());
        let names: Vec<_> = src.pages.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["search.js"]);

        let p = |v: Value| call_of("crates", "plan", vec![v]).unwrap();
        assert!(call_of("crates", "plan", vec![json!({})]).is_err());
        let s = p(json!({ "query": "serde" }));
        assert_eq!(s["mode"], json!("search"));
        assert_eq!(s["limit"], json!(10));
        // A crates.io URL (or an explicit crate/name) reads one crate page.
        let c = p(json!({ "query": "https://crates.io/crates/serde" }));
        assert_eq!(c["mode"], json!("crate"));
        assert_eq!(c["name"], json!("serde"));
        let c = p(json!({ "crate": "tokio", "mode": "read" }));
        assert_eq!(c["mode"], json!("crate"));
        assert_eq!(c["name"], json!("tokio"));
        // The search row count is bounded.
        assert_eq!(
            p(json!({ "query": "serde", "count": 999 }))["limit"],
            json!(50)
        );
        assert!(call_of(
            "crates",
            "plan",
            vec![json!({ "mode": "nope", "query": "x" })]
        )
        .is_err());
        assert!(call_of("crates", "plan", vec![json!({ "mode": "crate" })]).is_err());
    }

    #[test]
    fn bundled_pypi_compiles_and_plans() {
        let src = builtin("pypi").expect("bundled pypi plugin");
        let m = Manifest::parse(&src.manifest, "pypi").unwrap();
        assert_eq!(m.name, "pypi");
        assert_eq!(m.entry_op, "run");
        assert!(build_engine(0).compile(&src.entry).is_ok());
        let mut names: Vec<_> = src.pages.iter().map(|(n, _)| n.as_str()).collect();
        names.sort();
        assert_eq!(names, ["project.js", "search.js"]);

        let p = |v: Value| call_of("pypi", "plan", vec![v]).unwrap();
        assert!(call_of("pypi", "plan", vec![json!({})]).is_err());
        let s = p(json!({ "query": "requests" }));
        assert_eq!(s["mode"], json!("search"));
        assert_eq!(s["limit"], json!(10));
        let c = p(json!({ "query": "https://pypi.org/project/requests/" }));
        assert_eq!(c["mode"], json!("project"));
        assert_eq!(c["name"], json!("requests"));
        let c = p(json!({ "project": "flask", "mode": "read" }));
        assert_eq!(c["mode"], json!("project"));
        assert_eq!(c["name"], json!("flask"));
        assert!(call_of(
            "pypi",
            "plan",
            vec![json!({ "mode": "nope", "query": "x" })]
        )
        .is_err());
        assert!(call_of("pypi", "plan", vec![json!({ "mode": "project" })]).is_err());
    }

    #[test]
    fn bundled_docsrs_compiles_and_plans() {
        let src = builtin("docsrs").expect("bundled docsrs plugin");
        let m = Manifest::parse(&src.manifest, "docsrs").unwrap();
        assert_eq!(m.name, "docsrs");
        assert_eq!(m.entry_op, "run");
        assert!(build_engine(0).compile(&src.entry).is_ok());
        let names: Vec<_> = src.pages.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["search.js"]);

        let p = |v: Value| call_of("docsrs", "plan", vec![v]).unwrap();
        assert!(call_of("docsrs", "plan", vec![json!({})]).is_err());
        let s = p(json!({ "query": "serde" }));
        assert_eq!(s["mode"], json!("search"));
        assert_eq!(s["limit"], json!(10));
        // A docs.rs URL, or an explicit crate/name, reads one crate's docs.
        let c = p(json!({ "query": "https://docs.rs/serde/latest/serde/" }));
        assert_eq!(c["mode"], json!("crate"));
        assert_eq!(c["crate"], json!("serde"));
        let c = p(json!({ "crate": "tokio", "mode": "docs" }));
        assert_eq!(c["mode"], json!("crate"));
        assert_eq!(c["crate"], json!("tokio"));
        assert!(call_of(
            "docsrs",
            "plan",
            vec![json!({ "mode": "nope", "query": "x" })]
        )
        .is_err());
        assert!(call_of("docsrs", "plan", vec![json!({ "mode": "crate" })]).is_err());
    }

    #[test]
    fn bundled_github_compiles_and_plans() {
        let src = builtin("github").expect("bundled github plugin");
        let m = Manifest::parse(&src.manifest, "github").unwrap();
        assert_eq!(m.name, "github");
        assert_eq!(m.entry_op, "run");
        assert!(build_engine(0).compile(&src.entry).is_ok());
        let mut names: Vec<_> = src.pages.iter().map(|(n, _)| n.as_str()).collect();
        names.sort();
        assert_eq!(names, ["repo.js", "trending.js"]);

        let p = |v: Value| call_of("github", "plan", vec![v]).unwrap();
        // No query -> today's trending repositories.
        let t = p(json!({}));
        assert_eq!(t["mode"], json!("trending"));
        assert_eq!(t["since"], json!("daily"));
        assert_eq!(t["limit"], json!(25));
        // The trending window and language pass through.
        let t = p(json!({ "query": "trending", "since": "weekly", "language": "rust" }));
        assert_eq!(t["mode"], json!("trending"));
        assert_eq!(t["since"], json!("weekly"));
        assert_eq!(t["language"], json!("rust"));
        // A github.com URL (or owner/repo) reads one repository.
        let r = p(json!({ "query": "https://github.com/BurntSushi/ripgrep" }));
        assert_eq!(r["mode"], json!("repo"));
        assert_eq!(r["repo"], json!("BurntSushi/ripgrep"));
        let r = p(json!({ "repo": "tokio-rs/tokio", "mode": "read" }));
        assert_eq!(r["mode"], json!("repo"));
        assert_eq!(r["repo"], json!("tokio-rs/tokio"));
        // The trending row count is bounded.
        assert_eq!(
            p(json!({ "query": "trending", "count": 999 }))["limit"],
            json!(50)
        );
        assert!(call_of(
            "github",
            "plan",
            vec![json!({ "mode": "nope", "query": "x" })]
        )
        .is_err());
        assert!(call_of("github", "plan", vec![json!({ "mode": "repo" })]).is_err());
    }

    #[test]
    fn bundled_wikipedia_compiles_and_plans() {
        let src = builtin("wikipedia").expect("bundled wikipedia plugin");
        let m = Manifest::parse(&src.manifest, "wikipedia").unwrap();
        assert_eq!(m.name, "wikipedia");
        assert!(build_engine(0).compile(&src.entry).is_ok());
        let mut names: Vec<_> = src.pages.iter().map(|(n, _)| n.as_str()).collect();
        names.sort();
        assert_eq!(names, ["clean.js", "search.js"]);

        let p = |v: Value| call_of("wikipedia", "plan", vec![v]).unwrap();
        assert!(call_of("wikipedia", "plan", vec![json!({})]).is_err());
        let s = p(json!({ "query": "transformer neural network" }));
        assert_eq!(s["mode"], json!("search"));
        assert_eq!(s["limit"], json!(10));
        assert_eq!(s["lang"], json!("en"));
        // A URL both picks the page and its language.
        let a = p(json!({ "query": "https://zh.wikipedia.org/wiki/Transformer_(机器学习)" }));
        assert_eq!(a["mode"], json!("page"));
        assert_eq!(a["title"], json!("Transformer_(机器学习)"));
        assert_eq!(a["lang"], json!("zh"));
        // A bare title becomes an underscored article path.
        let a = p(json!({ "title": "Rust (programming language)" }));
        assert_eq!(a["mode"], json!("page"));
        assert_eq!(a["title"], json!("Rust_(programming_language)"));
        // An explicit search mode keeps its bounded row count.
        let s = p(json!({ "query": "rust", "mode": "search", "count": 3, "lang": "zh" }));
        assert_eq!(s["mode"], json!("search"));
        assert_eq!(s["limit"], json!(3));
        assert_eq!(s["lang"], json!("zh"));
        // An unsupported mode fails loudly.
        assert!(call_of("wikipedia", "plan", vec![json!({ "mode": "nope" })]).is_err());
    }

    #[test]
    fn bundled_mdn_compiles_and_plans() {
        let src = builtin("mdn").expect("bundled mdn plugin");
        let m = Manifest::parse(&src.manifest, "mdn").unwrap();
        assert_eq!(m.name, "mdn");
        assert!(build_engine(0).compile(&src.entry).is_ok());
        assert!(src.pages.is_empty());

        let p = |v: Value| call_of("mdn", "plan", vec![v]).unwrap();
        assert!(call_of("mdn", "plan", vec![json!({})]).is_err());
        let s = p(json!({ "query": "fetch api" }));
        assert_eq!(s["mode"], json!("search"));
        assert_eq!(s["limit"], json!(10));
        assert_eq!(s["locale"], json!("en-US"));
        // A doc path or an MDN URL reads that page.
        let a = p(json!({ "query": "/en-US/docs/Web/API/Fetch_API" }));
        assert_eq!(a["mode"], json!("page"));
        assert_eq!(a["path"], json!("/en-US/docs/Web/API/Fetch_API"));
        let a = p(json!({ "query": "https://developer.mozilla.org/en-US/docs/Web/API/Fetch_API" }));
        assert_eq!(a["mode"], json!("page"));
        assert_eq!(a["path"], json!("/en-US/docs/Web/API/Fetch_API"));
        let a = p(
            json!({ "query": "https://developer.mozilla.org/zh-CN/docs/Web/API/Fetch_API", "lang": "zh-CN" }),
        );
        assert_eq!(a["mode"], json!("page"));
        assert_eq!(a["path"], json!("/zh-CN/docs/Web/API/Fetch_API"));
        assert_eq!(a["locale"], json!("zh-CN"));
        // An explicit search mode keeps its bounded row count.
        let s = p(json!({ "query": "promise", "mode": "search", "count": 3 }));
        assert_eq!(s["mode"], json!("search"));
        assert_eq!(s["limit"], json!(3));
        // An unsupported mode fails loudly.
        assert!(call_of("mdn", "plan", vec![json!({ "mode": "nope" })]).is_err());
    }

    #[test]
    fn bundled_bing_compiles_and_plans() {
        let src = builtin("bing").expect("bundled bing plugin");
        let m = Manifest::parse(&src.manifest, "bing").unwrap();
        assert_eq!(m.name, "bing");
        assert!(build_engine(0).compile(&src.entry).is_ok());
        let names: Vec<_> = src.pages.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["search.js"]);

        let p = |v: Value| call_of("bing", "plan", vec![v]).unwrap();
        assert!(call_of("bing", "plan", vec![json!({})]).is_err());
        let s = p(json!({ "query": "typescript ai framework" }));
        assert_eq!(s["mode"], json!("search"));
        assert_eq!(s["limit"], json!(10));
        assert_eq!(s["mkt"], json!(""));
        let s = p(json!({ "query": "x", "count": 3, "mkt": "zh-CN" }));
        assert_eq!(s["limit"], json!(3));
        assert_eq!(s["mkt"], json!("zh-CN"));
        // The limit is capped and the mode is validated.
        assert_eq!(p(json!({ "query": "x", "count": 999 }))["limit"], json!(30));
        assert!(call_of(
            "bing",
            "plan",
            vec![json!({ "mode": "nope", "query": "x" })]
        )
        .is_err());
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

    // ── goofish / 闲鱼 ──────────────────────────────────────────────────────

    /// The goofish plugin resolves from the compiled-in copy and carries the two
    /// page scripts its op depends on.
    #[test]
    fn goofish_plugin_loads_with_both_page_scripts() {
        let src = builtin(GOOFISH_PLUGIN).expect("bundled goofish plugin");
        let m = Manifest::parse(&src.manifest, GOOFISH_PLUGIN).unwrap();
        assert_eq!(m.name, "goofish");
        assert_eq!(m.entry, "main.rhai");
        assert_eq!(m.entry_op, "run");
        assert_eq!(m.id(), "websites/goofish");
        assert!(src.entry.contains("fn run"));
        let pages: Vec<&str> = src.pages.iter().map(|(n, _)| n.as_str()).collect();
        assert!(pages.contains(&"search.js"), "{pages:?}");
        assert!(pages.contains(&"item.js"), "{pages:?}");
        // Both page scripts are the promise-wrapped IIFE the op evaluates, and
        // both read their options out of the injected `__LAYA_OPTS__`.
        for name in ["search.js", "item.js"] {
            let (_, js) = src.pages.iter().find(|(n, _)| n == name).expect(name);
            assert!(js.contains("__LAYA_OPTS__"), "{name} reads no opts");
            // Both are promise-wrapped IIFEs that resolve a plain object, which
            // is the shape browser_evaluate gets back.
            assert!(js.contains("new Promise"), "{name} is not promise-wrapped");
            assert!(js.contains("resolve(state)"), "{name} resolves nothing");
        }
    }

    /// One query string decides which of the four behaviours runs, so the
    /// classification is pinned here rather than only in a live run.
    #[test]
    fn goofish_plan_infers_the_mode_from_the_query() {
        let mode = |with: Value| gs_plan(with).unwrap()["mode"].as_str().unwrap().to_string();

        // A keyword is a search, and it stays the search phrase.
        let p = gs_plan(json!({"query": "索尼 A7M4"})).unwrap();
        assert_eq!(p["mode"], json!("search"));
        assert_eq!(p["query"], json!("索尼 A7M4"));

        // A full /search URL is unwrapped back to its `q=` term, still
        // percent-encoded: the engine has `urlencode` but no decoder, so the
        // term goes back into the URL verbatim rather than being decoded and
        // then encoded a second time (which would search for "%E7%B4%A2").
        let p =
            gs_plan(json!({"query": "https://www.goofish.com/search?q=%E7%B4%A2%E5%B0%BC%20A7M4"}))
                .unwrap();
        assert_eq!(p["mode"], json!("search"));
        assert_eq!(p["query"], json!("%E7%B4%A2%E5%B0%BC%20A7M4"));
        assert_eq!(p["preencoded"], json!(true));
        assert_eq!(
            gs_plan(json!({"query": "索尼 A7M4"})).unwrap()["preencoded"],
            json!(false),
            "a typed keyword still goes through urlencode"
        );

        // An item URL browses that one product…
        let p = gs_plan(json!({"query": "https://www.goofish.com/item?id=1085216610239"})).unwrap();
        assert_eq!(p["mode"], json!("item"));
        assert_eq!(p["item_id"], json!("1085216610239"));
        assert_eq!(
            p["item_url"],
            json!("https://www.goofish.com/item?id=1085216610239")
        );

        // …and so does a bare 12-13 digit id, normalized to the item URL.
        let p = gs_plan(json!({"query": "1085216610239"})).unwrap();
        assert_eq!(p["mode"], json!("item"));
        assert_eq!(
            p["item_url"],
            json!("https://www.goofish.com/item?id=1085216610239")
        );
        assert_eq!(p["query"], json!(""), "the id is not a search phrase");

        // A URL asked for as a search is still one item, unless `count` says
        // otherwise — the caller explicitly wants a listing then.
        let p = gs_plan(
            json!({"query": "https://www.goofish.com/item?id=1085216610239",
                               "count": 10}),
        )
        .unwrap();
        assert_eq!(p["mode"], json!("search"));

        // Price monitoring, and adding to it, are their own modes.
        for q in ["价格监控", "监控价格", "watch price", "降价提醒"] {
            assert_eq!(mode(json!({ "query": q })), "watch", "{q}");
        }
        let p = gs_plan(json!({"query": "添加监控 https://www.goofish.com/item?id=1071111102831"}))
            .unwrap();
        assert_eq!(p["mode"], json!("watch_add"));
        assert_eq!(
            p["item_url"],
            json!("https://www.goofish.com/item?id=1071111102831"),
            "the sentence around the URL must not leak into item_url"
        );
        // The same sentence with a bare id instead of a URL.
        let p = gs_plan(json!({"query": "添加监控 1087828137579"})).unwrap();
        assert_eq!(p["mode"], json!("watch_add"));
        assert_eq!(
            p["item_url"],
            json!("https://www.goofish.com/item?id=1087828137579")
        );
        // An explicit item mode reaches into the query the same way.
        let p = gs_plan(json!({"query": "帮我看看 1087828137579", "mode": "item"})).unwrap();
        assert_eq!(
            p["item_url"],
            json!("https://www.goofish.com/item?id=1087828137579")
        );
        // But a search that merely mentions a long number is still a search.
        assert_eq!(
            gs_plan(json!({"query": "1087828137579 的东西"})).unwrap()["mode"],
            json!("search")
        );

        // An explicit mode always wins over the inference.
        assert_eq!(
            mode(json!({"query": "价格监控", "mode": "search"})),
            "search"
        );
        assert_eq!(
            mode(json!({"query": "索尼", "mode": "Item"})),
            "item",
            "the mode is case-normalized"
        );

        // `item_url` / `id` are accepted as their own args, as the spec passes
        // them; a keyword still defaults to 20 rows and an item to 1.
        let p = gs_plan(json!({"item_url": "1071111102831"})).unwrap();
        assert_eq!(p["mode"], json!("item"));
        assert_eq!(p["count"], json!(1));
        assert_eq!(gs_plan(json!({"query": "x"})).unwrap()["count"], json!(20));
        assert_eq!(
            gs_plan(json!({"query": "x", "count": "45"})).unwrap()["count"],
            json!(45),
            "an interpolated count arrives as a string"
        );
    }

    /// Prices cross the boundary in three shapes: as JSON integers, as JSON
    /// floats, and as strings out of the hand-editable watch file. `onum`
    /// normalizes all three — and the integer case is the one that used to
    /// silently read as "no price at all".
    #[test]
    fn goofish_reads_prices_in_every_shape_they_arrive() {
        let n = |v: Value| gs("onum", vec![v]).unwrap();
        assert_eq!(n(json!(20500)), json!(20500.0), "an integer price");
        assert_eq!(n(json!(20500.0)), json!(20500.0));
        assert_eq!(n(json!("20500")), json!(20500.0), "watch.json string");
        assert_eq!(n(json!("20500.55")), json!(20500.55));
        assert_eq!(n(json!("948.90")), json!(948.9));
        assert_eq!(n(json!("  690  ")), json!(690.0), "padded by hand");
        assert_eq!(n(json!("-12.5")), json!(-12.5));
        // No number at all is "no price", never a guess.
        assert_eq!(n(json!(null)), json!(0.0));
        assert_eq!(n(json!("价格面议")), json!(0.0));
        assert_eq!(n(json!("")), json!(0.0));
        assert_eq!(n(json!("abc")), json!(0.0));

        // The option readers agree, which is what makes `--state '{"price_min":
        // 15000}'` filter instead of silently defaulting to 0.
        let f =
            |with: Value, key: &str| gs("opt_float", vec![with, json!(key), json!(0.0)]).unwrap();
        assert_eq!(f(json!({"price_min": 15000}), "price_min"), json!(15000.0));
        assert_eq!(
            f(json!({"price_min": 15000.5}), "price_min"),
            json!(15000.5)
        );
        assert_eq!(
            f(json!({"price_min": "15000"}), "price_min"),
            json!(15000.0)
        );
        assert_eq!(f(json!({}), "price_min"), json!(0.0));
        assert_eq!(f(json!({"price_min": null}), "price_min"), json!(0.0));
        assert_eq!(f(json!({"price_min": ""}), "price_min"), json!(0.0));
        assert_eq!(f(json!({"price_min": "面议"}), "price_min"), json!(0.0));
        let i = |with: Value, key: &str| gs("opt_int", vec![with, json!(key), json!(7)]).unwrap();
        assert_eq!(i(json!({"pages": 2}), "pages"), json!(2));
        assert_eq!(i(json!({"pages": "2"}), "pages"), json!(2));
        assert_eq!(i(json!({}), "pages"), json!(7));
        assert_eq!(i(json!({"pages": null}), "pages"), json!(7));

        // The price band itself: goofish ignores ?priceMin=/?priceMax= on
        // /search, so the band is applied to what came back.
        let ok = |row: Value, lo: f64, hi: f64| {
            gs("price_ok", vec![row, json!(lo), json!(hi)])
                .unwrap()
                .as_bool()
                .unwrap()
        };
        assert!(ok(json!({"price": 20500}), 15000.0, 22000.0));
        assert!(!ok(json!({"price": 9200}), 15000.0, 22000.0));
        assert!(!ok(json!({"price": 99999}), 15000.0, 22000.0));
        assert!(
            ok(json!({"price": 20500}), 0.0, 0.0),
            "no band keeps everything"
        );
        assert!(ok(json!({}), 15000.0, 22000.0), "a priceless card is kept");
        assert!(ok(json!({"price": 0}), 15000.0, 22000.0));
        assert!(ok(json!({"price": "¥13800"}), 15000.0, 0.0));
    }

    /// A whole-number price must print without float noise (`¥20500`, not
    /// `¥20500.0`) and a fraction must keep its two decimals.
    #[test]
    fn goofish_formats_prices_without_float_noise() {
        let f = |v: f64| {
            gs("fmt_num", vec![json!(v)])
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(f(20500.0), "20500");
        assert_eq!(f(0.0), "0");
        assert_eq!(f(1.98), "1.98");
        assert_eq!(f(97.0), "97");
        assert_eq!(f(-12.5), "-12.5");
        // 1.38 * 10000 is 13799.999999999998 in binary floating point.
        assert_eq!(f(13799.999999999998), "13800");
    }

    /// The same picture is spelled two ways on the two paths that meet: the
    /// carousel carries the site's rendition, save_article records the thumbnail
    /// it downloaded. Matching on the whole url left six of seven images
    /// pointing at the network instead of the copy on disk.
    #[test]
    fn goofish_matches_a_downloaded_image_across_both_url_spellings() {
        let key = |u: &str| {
            gs("alicdn_key", vec![json!(u)])
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        };
        let on_page = "https://img.alicdn.com/bao/uploaded/i2/2222253586975/O1CN01midsWE2HTiD2YcGq_790x10000Q90.jpg_.webp";
        let downloaded = "https://img.alicdn.com/bao/uploaded/i2/2222253586975/O1CN01midsWE2HTiD2YcGq_!!4611686018427386399-0-xy_item.jpg_Q90.jpg_.webp";
        assert_eq!(key(on_page), key(downloaded));
        assert_ne!(on_page, downloaded, "the two spellings really do differ");
        assert_eq!(key(on_page), "O1CN01midsWE2HTiD2YcGq");
        // A different photo on the same account is a different key.
        assert_ne!(
            key(downloaded),
            key("https://img.alicdn.com/bao/uploaded/i2/2222253586975/O1CN01ZLUTCv2LLqI2Yc4y_!!1-xy_item.jpg_.webp")
        );

        let capture = json!({"images": [
            {"name": "img-1.webp", "url": downloaded, "ok": true},
            {"name": "img-2.webp", "url": "https://img.alicdn.com/other.png", "ok": true}
        ]});
        let local = |u: &str| {
            gs("local_image", vec![capture.clone(), json!(u)])
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(local(on_page), "img-1.webp");
        assert_eq!(local(downloaded), "img-1.webp");
        // Not downloaded ⇒ no local name, and the sheet falls back to the CDN.
        assert_eq!(
            local("https://img.alicdn.com/bao/uploaded/i9/1/O1CN01missing_790x.jpg_.webp"),
            ""
        );
        assert_eq!(local("https://example.com/x.png"), "");
        // A capture without an image list must not throw.
        assert_eq!(
            gs("local_image", vec![json!({}), json!(on_page)]).unwrap(),
            json!("")
        );
    }

    /// `observe` has to hand the store back with the call. Rhai maps are value
    /// types: writing `wdata["items"]` inside the function updates a local copy
    /// that is dropped on return, and a price monitor that quietly forgets every
    /// sighting is worse than no monitor at all.
    #[test]
    fn goofish_observe_returns_the_store_it_just_updated() {
        let now = json!({"rfc3339": "2026-09-28T00:00:00.000Z", "unix_ms": 1790000000000i64});
        let later = json!({"rfc3339": "2026-09-28T01:00:00.000Z", "unix_ms": 1790003600000i64});
        let empty = json!({"version": 1, "items": []});
        let url = "https://www.goofish.com/item?id=1085216610239";
        let rec = |price: Value| {
            json!({"id": "1085216610239", "url": url, "title": "索尼A7M4",
                   "price": price, "status": "on_sale"})
        };
        let observe = |wdata: Value, rec: Value, at: Value| {
            call_host4(GOOFISH_PLUGIN, "observe", wdata, rec, at, json!(200))
                .unwrap_or_else(|e| panic!("observe(): {e}"))
        };
        let price_of = |wdata: &Value, id: &str| -> Value {
            wdata["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["id"] == json!(id))
                .unwrap()["price"]
                .clone()
        };

        // First sighting: "new", and the item really is in the returned store.
        let first = observe(empty.clone(), rec(json!(20500)), now.clone());
        assert_eq!(first["change"]["direction"], json!("new"));
        assert_eq!(first["change"]["new_price"], json!(20500.0));
        assert_eq!(first["change"]["old_price"], json!(null));
        assert_eq!(price_of(&first["data"], "1085216610239"), json!(20500.0));
        assert_eq!(first["data"]["items"].as_array().unwrap().len(), 1);

        // A drop is the headline case: same price type, new number.
        let dropped = observe(first["data"].clone(), rec(json!(19800)), later.clone());
        let ch = &dropped["change"];
        assert_eq!(ch["direction"], json!("down"));
        assert_eq!(ch["old_price"], json!(20500.0));
        assert_eq!(ch["new_price"], json!(19800.0));
        assert_eq!(ch["delta"], json!(-700.0));
        assert_eq!(ch["delta_pct"], json!(-3.41));
        assert_eq!(price_of(&dropped["data"], "1085216610239"), json!(19800.0));

        // Unchanged, and the sighting still lands in the history.
        let same = observe(dropped["data"].clone(), rec(json!(19800)), later.clone());
        assert_eq!(same["change"]["direction"], json!("same"));
        assert_eq!(same["change"]["delta"], json!(0.0));
        let entry = &same["data"]["items"][0];
        assert_eq!(entry["seen_count"], json!(3));
        assert_eq!(entry["history"].as_array().unwrap().len(), 3);
        assert_eq!(entry["last_seen"], later["rfc3339"]);

        // A first sighting without a number is "unknown", not a zero price.
        let priceless = observe(
            empty.clone(),
            json!({"id": "1", "url": "https://www.goofish.com/item?id=1",
                   "title": "面议", "status": "on_sale"}),
            now.clone(),
        );
        assert_eq!(priceless["change"]["direction"], json!("new"));
        assert_eq!(priceless["data"]["items"][0]["price"], json!(null));

        // Gone beats any price comparison, and the stale price is dropped: a
        // delisted page has none, and a monitor that keeps quoting one is
        // reporting fiction.
        let gone = observe(
            same["data"].clone(),
            json!({"id": "1085216610239", "url": url, "title": "索尼A7M4", "status": "gone"}),
            later.clone(),
        );
        assert_eq!(gone["change"]["direction"], json!("gone"));
        assert_eq!(gone["data"]["items"][0]["price"], json!(null));
        assert_eq!(gone["data"]["items"][0]["status"], json!("gone"));

        // A dead id seen for the first time is "gone", not "new".
        let never = observe(
            empty.clone(),
            json!({"id": "1000000000000",
                   "url": "https://www.goofish.com/item?id=1000000000000",
                   "status": "gone"}),
            now.clone(),
        );
        assert_eq!(never["change"]["direction"], json!("gone"));

        // history_max bounds the file: it keeps the newest points, drops the
        // oldest, and still counts every sighting.
        let mut store = first["data"].clone();
        for i in 2..=8 {
            let at = json!({"rfc3339": "2026-09-28T02:00:00.000Z",
                            "unix_ms": 1790007200000i64 + i});
            store = call_host4(
                GOOFISH_PLUGIN,
                "observe",
                store,
                rec(json!(20000 + i)),
                at,
                json!(3),
            )
            .unwrap()["data"]
                .clone();
        }
        assert_eq!(store["items"][0]["history"].as_array().unwrap().len(), 3);
        assert_eq!(store["items"][0]["seen_count"], json!(8));
        // The kept tail is the newest one: the last price is in it, the first
        // sighting is not.
        let tail = store["items"][0]["history"].as_array().unwrap();
        assert_eq!(tail[2]["price"], json!(20008.0));
        assert_eq!(tail[0]["price"], json!(20006.0));
    }
}
