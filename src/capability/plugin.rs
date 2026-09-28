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

#[derive(Clone, Debug)]
struct Manifest {
    name: String,
    entry: String,
    entry_op: String,
    max_operations: u64,
}

impl Manifest {
    fn parse(src: &str, fallback: &str) -> Result<Manifest> {
        let v: Value = serde_json::from_str(src)
            .map_err(|e| anyhow!("plugin {fallback:?} has an invalid plugin.json: {e}"))?;
        Ok(Manifest {
            name: v
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(fallback)
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

/// The raw text of a plugin: manifest, entry script, and its page scripts.
#[derive(Debug)]
struct Sources {
    manifest: String,
    entry: String,
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
        _ => return None,
    };
    Some(Sources {
        manifest: manifest.to_string(),
        entry: entry.to_string(),
        pages: pages
            .into_iter()
            .map(|(n, src)| (n.to_string(), src.to_string()))
            .collect(),
        dir: None,
    })
}

/// Candidate `plugins/` roots, highest priority first.
fn plugin_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(dir) = std::env::var("LAYA_PLUGIN_DIR") {
        if !dir.trim().is_empty() {
            roots.push(PathBuf::from(dir));
        }
    }
    let mut cur = std::env::current_dir().ok();
    while let Some(dir) = cur {
        roots.push(dir.join("plugins"));
        if dir.join(".git").exists() {
            break;
        }
        cur = dir.parent().map(std::path::Path::to_path_buf);
    }
    roots
}

fn read_dir_sources(dir: &std::path::Path, fallback: &str) -> Result<Sources> {
    let manifest = std::fs::read_to_string(dir.join("plugin.json")).map_err(|e| {
        anyhow!(
            "plugin {fallback:?}: cannot read {}: {e}",
            dir.join("plugin.json").display()
        )
    })?;
    let m = Manifest::parse(&manifest, fallback)?;
    if m.entry.is_empty() {
        bail!("plugin {fallback:?}: manifest has an empty 'entry'");
    }
    let entry = std::fs::read_to_string(dir.join(&m.entry))
        .map_err(|e| anyhow!("plugin {fallback:?}: cannot read entry {:?}: {e}", m.entry))?;
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
    Ok(Sources {
        manifest,
        entry,
        pages,
        dir: Some(dir.to_path_buf()),
    })
}

fn load_sources(cap: &PluginCap) -> Result<Sources> {
    if !cap.dir.trim().is_empty() {
        // An explicit directory is enough; the name is only used for messages.
        let dir = std::path::Path::new(&cap.dir);
        let fallback = if cap.plugin.trim().is_empty() {
            dir.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("plugin")
                .to_string()
        } else {
            cap.plugin.clone()
        };
        return read_dir_sources(dir, &fallback);
    }
    if cap.plugin.trim().is_empty() {
        bail!("plugin capability needs 'plugin' (a name) or 'dir'");
    }
    for root in plugin_roots() {
        let dir = root.join(&cap.plugin);
        if dir.join("plugin.json").is_file() {
            return read_dir_sources(&dir, &cap.plugin);
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
            let ms = util::now_unix_ms();
            to_dyn(json!({
                "unix_ms": ms as u64,
                "rfc3339": util::rfc3339_utc_from_unix_ms(ms),
            }))
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
        map.insert("plugin".to_string(), json!(cap.plugin));
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
    let entry = if cap.entry.trim().is_empty() {
        manifest.entry.clone()
    } else {
        cap.entry.clone()
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
    let ast = engine
        .compile(&sources.entry)
        .map_err(|e| anyhow!("plugin {:?} ({entry}) did not compile: {e}", cap.plugin))?;

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

    /// Evaluate a bundled-plugin function directly (the same engine the host
    /// builds, minus the browser calls) so the *script's* behaviour is what gets
    /// asserted — not a Rust re-implementation of it.
    fn call(name: &str, args: Vec<Value>) -> Result<Value> {
        let sources = builtin(ALPHAXIV_PLUGIN).expect("bundled alphaxiv plugin");
        let engine = build_engine(0);
        let ast = engine
            .compile(&sources.entry)
            .map_err(|e| anyhow!("compile: {e}"))?;
        let host = test_host();
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
}
