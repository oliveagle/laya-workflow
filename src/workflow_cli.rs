//! `laya-workflow` — run Laya apps / workflows on the Rust engine.
//!
//! Subcommands:
//!   * `apps`     — run the 4 apps' reference cases against a live `laya-tch`
//!   * `describe` — print the workflow graph for an app
//!   * `demo`     — run a built-in composition demo (gate → triage chain)
//!
//! With `--base-url` the CLI drives a running `laya-tch` server (its
//! `/v1/systemone` endpoint); the graph logic is identical to the Python side.

use anyhow::{anyhow, bail, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use laya_workflow::apps;
use laya_workflow::backend::{HeuristicBackend, LayaBackend};
use laya_workflow::workflow::{Decide, ResilientWorkflow};

#[derive(Parser)]
#[command(
    name = "laya-workflow",
    about = "Run Laya workflows / apps on the Rust engine"
)]
struct Cli {
    /// Base URL of a running `laya-tch` server, e.g. http://127.0.0.1:8400.
    /// Omit to run the offline heuristic backend (graph/plumbing checks only).
    #[arg(long)]
    base_url: Option<String>,

    /// Pin the spec root, replacing the layered lookup (repo → user → builtin).
    /// Equivalent to setting `LAYA_DSL_DIR`; use it to point at a specific tree.
    #[arg(long, value_name = "PATH")]
    dsl_dir: Option<String>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the 4 apps' reference cases and report pass/fail
    Apps,
    /// Print the workflow graph description for an app
    Describe { app: String },
    /// Run the composition demo (gate → triage chain)
    Demo,
    /// Run a Laya-driven optimizer loop (propose → evaluate → Laya continue/strategy)
    Optimize {
        /// Max optimization steps
        #[arg(long, default_value_t = 12)]
        steps: usize,
        /// Comma-separated strategy names the Laya strategy node may switch to
        #[arg(
            long,
            default_value = "aggressive_step,conservative_step,random_restart"
        )]
        strategies: String,
    },
    /// Accuracy self-improvement: measure decisions, learn from misses under a
    /// hold-out gate, and persist the accepted policy.
    Improve {
        /// Directory holding policy.json / samples.jsonl / rounds.jsonl
        #[arg(long)]
        dir: String,
        /// How many improve rounds to run (each round: score → propose → gate)
        #[arg(long, default_value_t = 1)]
        rounds: usize,
        /// Just report the current accuracy; change nothing
        #[arg(long, default_value_t = false)]
        score_only: bool,
    },
    /// Export a built-in app workflow as a generic JSON spec
    Export { app: String },
    /// Run a workflow defined by a JSON spec (generic, no recompilation)
    Run {
        /// Path to the workflow spec JSON
        #[arg(long)]
        spec: String,
        /// State JSON to feed the workflow (default: {})
        #[arg(long, default_value = "{}")]
        state: String,
        /// Shorthand for --state '{"query":"..."}'; merged into --state when
        /// both are supplied (the --query value wins)
        #[arg(long)]
        query: Option<String>,
    },
    /// Print every stored node record (per-iteration tracking)
    State {
        #[arg(long)]
        dir: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Resume a run: continue at `from + 1` with the materialised state
    Resume {
        #[arg(long)]
        spec: String,
        #[arg(long)]
        dir: String,
        #[arg(long)]
        from: Option<u64>,
        #[arg(long, default_value = "{}")]
        state: String,
    },
    /// Replay a single iteration in place (atomic overwrite of one record)
    Replay {
        #[arg(long)]
        spec: String,
        #[arg(long)]
        dir: String,
        #[arg(long)]
        iter: u64,
    },
    /// Validate a workflow spec and print its graph
    Validate {
        #[arg(long)]
        spec: String,
    },
    /// Progressive disclosure of how to use this CLI. Run with no args for
    /// the top-level map; `--section <name>` for one subcommand; `--recipe <name>`
    /// for a concrete end-to-end walk-through; `--list` for the index.
    Skill {
        /// Section name to expand (use `skill --list` to see them).
        #[arg(long)]
        section: Option<String>,
        /// End-to-end recipe name (use `skill --list` to see them).
        #[arg(long)]
        recipe: Option<String>,
        /// Print just the section + recipe index.
        #[arg(long, default_value_t = false)]
        list: bool,
        /// Output format: text (default) or json (machine-readable index).
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// List every spec found across the layered DSL roots (repo → user → builtin)
    List,
    /// Manage Rhai plugins: install one from a git repo, list them, show where
    /// they are looked up. See `skill --section plugins`.
    Plugin {
        #[command(subcommand)]
        cmd: PluginCmd,
    },
}

#[derive(Subcommand)]
enum PluginCmd {
    /// Install a *single* plugin directory from a git repo (sparse clone — not
    /// the whole repo) into the plugin root.
    Install {
        /// Repo as `owner/repo` or a git URL (https://, ssh://, git@).
        repo: String,
        /// Directory inside the repo that holds the plugin (e.g. websites/alphaxiv.org).
        #[arg(long)]
        path: String,
        /// Installed name (default: the last segment of --path).
        #[arg(long)]
        name: Option<String>,
        /// Branch/tag/commit to check out (default: the repo's default branch).
        #[arg(long = "git-ref")]
        git_ref: Option<String>,
        /// Install root (default: $LAYA_PLUGIN_DIR, else ~/.config/laya-workflow/plugins).
        #[arg(long)]
        root: Option<String>,
        /// Overwrite an existing install of the same name.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// List every plugin the engine can see (installed layers + built-ins).
    List,
    /// Print where `plugin install` writes and the search path it lands in.
    Dir,
}

/// Live engine when `--base-url` is given, otherwise the offline heuristic.
fn make_backend(url: Option<&str>) -> (Box<dyn Decide>, String) {
    match url {
        Some(u) => (Box::new(LayaBackend::new(u)), format!("laya-tch @ {u}")),
        None => (Box::new(HeuristicBackend), "offline heuristic".to_string()),
    }
}

/// Build run state, supporting the human-friendly `--query TEXT` shorthand.
/// Any JSON remains valid for `--state`; when `--query` is present, that value
/// is inserted into the object's `query` field.
fn run_state(state: &str, query: Option<&str>) -> Result<Value> {
    let mut value: Value = serde_json::from_str(state)?;
    let Some(query) = query else {
        return Ok(value);
    };
    let query = query.trim();
    if query.is_empty() {
        bail!("--query must not be empty");
    }
    let Some(obj) = value.as_object_mut() else {
        bail!("--query can only be combined with a JSON object in --state");
    };
    obj.insert("query".to_string(), json!(query));
    Ok(value)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // Pin the spec root only when asked (`--dsl-dir` > `LAYA_DSL_DIR`); otherwise
    // leave it unpinned so the layered lookup (repo → user → builtin) applies.
    if let Some(dir) = explicit_dsl_dir(&cli) {
        laya_workflow::spec::set_dsl_dir(&dir);
    }
    // Load secrets (environment + .env files) before anything can reference them.
    let secrets_dir = laya_workflow::spec::primary_spec_dir();
    laya_workflow::capability::secret::init(Some(secrets_dir.as_path()))?;
    let (backend, label) = make_backend(cli.base_url.as_deref());
    match &cli.cmd {
        Cmd::Apps => run_apps(backend.as_ref(), &label),
        Cmd::Describe { app } => {
            let wf = app_workflow(app)?;
            println!("{}", serde_json::to_string_pretty(&wf.describe())?);
            Ok(())
        }
        Cmd::Demo => run_demo(backend.as_ref(), &label),
        Cmd::Optimize { steps, strategies } => {
            let strats: Vec<String> = strategies
                .split(',')
                .map(|s| s.trim().to_string())
                .collect();
            run_optimize(backend.as_ref(), &label, *steps, strats)
        }
        Cmd::Improve {
            dir,
            rounds,
            score_only,
        } => run_improve(dir, *rounds, *score_only),
        Cmd::Export { app } => {
            let wf = app_workflow(app)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&laya_workflow::spec::to_spec(&wf, app))?
            );
            Ok(())
        }
        Cmd::Validate { spec } => {
            let raw = std::fs::read_to_string(spec)?;
            let sv: Value = serde_json::from_str(&raw)?;
            match laya_workflow::spec::check_version(&sv)? {
                laya_workflow::spec::VersionCheck::Ok(v) => {
                    println!("# dsl_version: {v} (current)")
                }
                laya_workflow::spec::VersionCheck::Upgradable { from, to } => {
                    println!("# dsl_version: {from} (legacy; engine supports {to})")
                }
            }
            let wf = laya_workflow::spec::load_file(spec)?;
            // report external capabilities + the safety policy this spec declares
            let reg = laya_workflow::capability::Registry::from_spec(&sv)?;
            let names = reg.names();
            if names.is_empty() {
                println!("# capabilities: none");
            } else {
                println!("# capabilities: {}", names.join(", "));
            }
            let pol = reg.policy();
            println!(
                "# policy: allow_exec={} allow_hosts={:?} max_timeout_ms={} max_output={} retries={}",
                pol.allow_exec, pol.allow_hosts, pol.max_timeout_ms, pol.max_output, pol.retries
            );
            // secrets audit: names + readiness, never values
            let refs = laya_workflow::capability::secret::referenced_names(&sv);
            if refs.is_empty() {
                println!("# secrets: none required");
            } else {
                let mut ready = Vec::new();
                let mut missing = Vec::new();
                for r in &refs {
                    if laya_workflow::capability::secret::has(r) {
                        ready.push(r.clone());
                    } else {
                        missing.push(r.clone());
                    }
                }
                println!("# secrets: {} required, {} ready", refs.len(), ready.len());
                if !ready.is_empty() {
                    println!("# secrets ready   (names only): {}", ready.join(", "));
                }
                if !missing.is_empty() {
                    println!("# secrets MISSING (names only): {}", missing.join(", "));
                }
            }
            let envnames = laya_workflow::capability::secret::env_names(&sv);
            if !envnames.is_empty() {
                println!(
                    "# env vars referenced (names only): {}",
                    envnames.join(", ")
                );
            }
            let hard = laya_workflow::capability::secret::hardcoded_secret_fields(&sv);
            if !hard.is_empty() {
                println!(
                    "# WARNING: possible hard-coded secrets at: {}",
                    hard.join(", ")
                );
            }
            println!("{}", serde_json::to_string_pretty(&wf.describe())?);
            Ok(())
        }
        Cmd::Run { spec, state, query } => {
            let wf = laya_workflow::spec::load_file(spec)?;
            let st = run_state(state, query.as_deref())?;
            println!("backend: {label}");
            let out = wf.run(backend.as_ref(), &st)?;
            // redact before printing: a secret must never reach stdout
            let shown = laya_workflow::capability::secret::redact(&out.to_json());
            println!("{}", serde_json::to_string_pretty(&shown)?);
            Ok(())
        }
        Cmd::State { dir, json } => {
            let store = laya_workflow::persist::NodeStore::open(dir)?;
            let recs = store.read_all()?;
            if *json {
                let v: Vec<serde_json::Value> = recs.iter().map(|r| r.to_json()).collect();
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                println!("dir: {dir}");
                println!("records: {}", recs.len());
                if let Some(last) = store.last_iteration()? {
                    println!("last_iteration: {last}");
                }
                for r in &recs {
                    println!(
                        "  iter={:04} node={:>14} action={:8} conf={:.3} next={:?} detail={:?}",
                        r.iteration, r.node, r.action, r.confidence, r.next_node, r.detail
                    );
                }
            }
            Ok(())
        }
        Cmd::Resume {
            spec,
            dir,
            from,
            state,
        } => {
            let wf = laya_workflow::spec::load_file(spec)?;
            let mut store = laya_workflow::persist::NodeStore::open(dir)?;
            let initial: Option<serde_json::Value> = if state == "{}" {
                None
            } else {
                Some(serde_json::from_str(state)?)
            };
            let (backend, label) = make_backend(cli.base_url.as_deref());
            println!("backend: {label}");
            let out = wf.run_persistent(backend.as_ref(), &mut store, initial.as_ref(), *from)?;
            println!(
                "final_action={} iterations={} stored_iterations={}",
                out.final_action(),
                out.iterations,
                store.last_iteration()?.unwrap_or(0)
            );
            Ok(())
        }
        Cmd::Replay { spec, dir, iter } => {
            let wf = laya_workflow::spec::load_file(spec)?;
            let mut store = laya_workflow::persist::NodeStore::open(dir)?;
            let (backend, label) = make_backend(cli.base_url.as_deref());
            println!("backend: {label}");
            let rec = wf.replay(backend.as_ref(), &mut store, *iter)?;
            println!(
                "replayed iter={} node={} action={} conf={:.3}",
                rec.iteration, rec.node, rec.action, rec.confidence
            );
            Ok(())
        }
        Cmd::Skill {
            section,
            recipe,
            list,
            format,
        } => run_skill(section.as_deref(), recipe.as_deref(), *list, format),
        Cmd::Plugin { cmd } => run_plugin(cmd),
        Cmd::List => {
            let roots = laya_workflow::spec::spec_roots_low_to_high();
            println!("DSL search path (low → high; later overrides earlier):");
            for r in &roots {
                let mark = if r.path.is_dir() { "" } else { "  (missing)" };
                println!("  {:<8} {}{}", r.layer.as_str(), r.path.display(), mark);
            }
            let found = laya_workflow::spec::discover_layered()?;
            println!(
                "\n{} spec(s)  (engine dsl_version: {}):",
                found.len(),
                laya_workflow::spec::DSL_VERSION
            );
            for s in &found {
                let mut line = format!(
                    "  {:<24} {:<8} v{:<3} {}",
                    s.name,
                    s.layer.as_str(),
                    s.version,
                    s.path.display()
                );
                if let Some((layer, root)) = &s.shadowed_by {
                    line.push_str(&format!(
                        "  (shadowed by {} {})",
                        layer.as_str(),
                        root.display()
                    ));
                }
                println!("{line}");
            }
            Ok(())
        }
    }
}

/// `plugin install | list | dir`. Kept tiny: the real work (validation, sparse
/// clone, copy, discovery) lives in `capability::plugin` so it is unit-tested.
fn run_plugin(cmd: &PluginCmd) -> Result<()> {
    use laya_workflow::capability::plugin as plg;
    match cmd {
        PluginCmd::Install {
            repo,
            path,
            name,
            git_ref,
            root,
            force,
        } => {
            let root = match root {
                Some(r) => std::path::PathBuf::from(r),
                None => plg::install_root()?,
            };
            // The plugin name is resolved at install time: `--name` wins, else
            // the source `plugin.json` `name`, else the checkout path's segment.
            let name = name.clone().unwrap_or_default();
            let url = plg::repo_clone_url(repo)?;
            println!("repo:   {}", plg::redact_url(&url));
            println!("path:   {}", plg::normalize_subdir(path)?);
            println!("root:   {}", root.display());
            let out = plg::install_from_git(repo, path, &name, &root, git_ref.as_deref(), *force)?;
            println!("name:   {}", out.name);
            println!(
                "installed {} v{} ({} file(s)) -> {}",
                out.name,
                out.version,
                out.files,
                out.dest.display()
            );
            println!("next:   laya-workflow plugin list   |   skill --section plugins");
            Ok(())
        }
        PluginCmd::List => {
            let entries = plg::discover_plugins();
            if entries.is_empty() {
                println!("# no plugins found");
            }
            println!(
                "{:<16} {:<8} {:<8} {}",
                "NAME", "LAYER", "VERSION", "LOCATION"
            );
            for e in &entries {
                let loc = e
                    .path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(compiled into the binary)".to_string());
                let note = if e.description.is_empty() {
                    String::new()
                } else {
                    format!("  — {}", e.description)
                };
                println!(
                    "{:<16} {:<8} {:<8} {}{}",
                    e.name,
                    e.layer.as_str(),
                    e.version,
                    loc,
                    note
                );
            }
            Ok(())
        }
        PluginCmd::Dir => {
            let root = plg::install_root()?;
            println!("install root: {}", root.display());
            println!("plugin search path (high → low):");
            for (layer, p) in plg::plugin_search_path() {
                let mark = if p.is_dir() { "" } else { "  (missing)" };
                println!("  {:<8} {}{}", layer.as_str(), p.display(), mark);
            }
            println!("  {:<8} {}", "builtin", plg::builtin_names().join(", "));
            Ok(())
        }
    }
}

/// Skill content: progressive disclosure of how to use laya-workflow.
///
/// Layering:
///   * `skill`             → top-level map (what you can do, subcommand cheats)
///   * `skill --section S` → focused detail on one subcommand
///   * `skill --recipe R`  → concrete end-to-end walk-through
///   * `skill --list`      → section + recipe index, one line each
///
/// `format=json` on `--list` returns the index as JSON (machine-readable).
/// All content is self-contained so the binary ships without external files.
fn run_skill(
    section: Option<&str>,
    recipe: Option<&str>,
    only_list: bool,
    format: &str,
) -> Result<()> {
    let index = skill_index();
    if only_list || (section.is_none() && recipe.is_none() && format == "json") {
        if format == "json" {
            let v: Vec<serde_json::Value> = index
                .iter()
                .map(|(k, d, _b)| json!({"name": k, "kind": "section", "description": d}))
                .chain(
                    skill_recipes()
                        .iter()
                        .map(|(k, d, _b)| json!({"name": k, "kind": "recipe", "description": d})),
                )
                .collect();
            println!("{}", serde_json::to_string_pretty(&v)?);
            return Ok(());
        }
        print_index(&index, &skill_recipes());
        return Ok(());
    }
    if let Some(s) = section {
        return print_section(s);
    }
    if let Some(r) = recipe {
        return print_recipe(r);
    }
    // No flags → top-level map.
    print_overview();
    Ok(())
}

fn skill_index() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("overview",  "Top-level map of what `laya-workflow` can do and how subcommands relate.", "skill"),
        ("validate",  "Run `laya-workflow validate --spec <file>` to lint a workflow spec before any execution.", "validate"),
        ("run",       "Run a spec end-to-end (`run --spec S --state '{}'`) on the offline heuristic or a live server (`--base-url`).", "run"),
        ("list",      "List every spec discoverable across the layered roots (repo `.laya-workflow/dsl`/`dsl` → user → builtin), with the search path.", "list"),
        ("apps",      "Run the four built-in apps' reference cases against a live `laya-tch` server.", "apps"),
        ("describe",  "Print the graph description (`start`, `nodes`, edges, retries) for one built-in app.", "describe"),
        ("demo",      "Run the built-in composition demo (`gate → triage` chain) on whatever backend is selected.", "demo"),
        ("optimize",  "Run a Laya-driven optimizer loop: propose → evaluate → Laya continue/strategy for `steps` rounds.", "optimize"),
        ("improve",  "Accuracy self-improvement: measure decisions, learn from misses under a hold-out gate, persist the accepted policy.", "improve"),
        ("export",    "Export a built-in app workflow as a generic JSON spec (so it can be edited and re-loaded).", "export"),
        ("state",     "List every per-node record in a NodeStore (track what ran, what it returned, the full state snapshot).", "state"),
        ("resume",   "Continue a partially-completed run from `dir/`, optionally from `--from <iter>` with the materialised state.", "resume"),
        ("replay",   "Re-execute a single iteration in place from its `state_before` (atomic overwrite of one record).", "replay"),
        ("persist",   "Where the per-node store lives on disk (`<dir>/runs/0001.json` + `manifest.json`), how `run_persistent` / `rewind` / `replay` fit together.", "state"),
        ("dsl",       "Workflow JSON shape (`name`, `start`, `nodes[*]`, `actions`, `capabilities`), versioning (`dsl_version`), folder layout, and the kind catalogue.", "list"),
        ("plugins",   "Extension seam: write site logic as a sandboxed Rhai plugin, install one from a git repo (`plugin install`), and call it with `kind: \"plugin\"`.", "dsl"),
        ("tests",     "The offline test runner `laya-workflow-tests` is modular: each `[section]` is selectable via `./target/release/laya-workflow-tests <section>`.", "tests"),
        ("safety",    "Safety gates every spec goes through: `policy.allow_exec`, `policy.allow_paths`, `policy.allow_hosts`, `policy.max_timeout_ms`, `policy.max_output`, secret redaction.", "validate"),
    ]
}

fn skill_recipes() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("first-run", "Run a workflow spec end-to-end on the offline heuristic.", "run"),
        ("live-server", "Point the CLI at a running `laya-tch` HTTP server for real decisions.", "run"),
        ("debug-failure", "A workflow came back wrong — what to check, in order.", "state"),
        ("checkpoint-resume", "Run, kill, resume: prove the per-node store survives a process restart.", "resume"),
        ("rollback-bad-iter", "An iteration produced wrong state. Rewind to before it and replay from the materialised state.", "replay"),
        ("harden-spec", "Before shipping a spec, lint it (`validate`), confirm required secrets are present, and exercise the safety gates.", "validate"),
        ("agent-onboarding", "Quick path: what an agent should read first when handed the `laya-workflow` tool cold.", "overview"),
    ]
}

fn print_index(
    sections: &[(&'static str, &'static str, &str)],
    recipes: &[(&'static str, &'static str, &str)],
) {
    println!("# laya-workflow skill index");
    println!();
    println!("Run `laya-workflow skill` for the top-level map.");
    println!(
        "Run `laya-workflow skill --section <name>` or `--recipe <name>` to expand one entry."
    );
    println!("Run `laya-workflow skill --list --format json` for machine-readable index.");
    println!();
    println!("## Sections ({})", sections.len());
    let mut names: Vec<&str> = sections.iter().map(|s| s.0).collect();
    names.sort();
    for n in names {
        if let Some((_, d, _)) = sections.iter().find(|(k, _, _)| *k == n) {
            println!("  {:20} {}", n, d);
        }
    }
    println!();
    println!("## Recipes ({})", recipes.len());
    let mut names: Vec<&str> = recipes.iter().map(|s| s.0).collect();
    names.sort();
    for n in names {
        if let Some((_, d, _)) = recipes.iter().find(|(k, _, _)| *k == n) {
            println!("  {:20} {}", n, d);
        }
    }
}

fn print_overview() {
    println!("# laya-workflow — quick map for agents");
    println!();
    println!("What this CLI is:");
    println!("  Rust implementation of the Laya workflow engine. Runs a JSON spec");
    println!("  (a graph of decision nodes) against either the offline heuristic");
    println!("  backend (graph/plumbing checks only) or a live `laya-tch` HTTP server");
    println!("  (`--base-url http://127.0.0.1:8400`) for real model decisions.");
    println!();
    println!("Subcommand cheats (read top-down; pick the lowest that answers you):");
    println!();
    println!("  apps           run the 4 built-in apps' reference cases");
    println!("  describe <a>   print the graph of one built-in app");
    println!("  demo           run a built-in composition demo");
    println!("  optimize       Laya-driven optimizer loop");
    println!("  improve        accuracy self-improvement under a hold-out gate");
    println!("  export <a>     write a built-in app as a generic JSON spec");
    println!("  validate -s S  lint a spec (graph, capabilities, policy, secrets)");
    println!("  run -s S     run a spec end-to-end");
    println!("  list           enumerate specs across the layered roots (repo → user → builtin)");
    println!("  state -d D    list every per-node record in a NodeStore");
    println!("  resume -s S -d D [-from N] [-state JSON]");
    println!("                 continue a partial run from the last stored iter");
    println!("  replay -s S -d D -iter N");
    println!("                 re-run a single iter in place from its state_before");
    println!("  plugin i|l|d   install/list/inspect Rhai plugins (the extension seam)");
    println!("  skill          this help (progressive disclosure)");
    println!();
    println!("Pick the entry point that matches your intent:");
    println!("  * Just want to see whether a spec works?  → validate → run");
    println!("  * Want to know what just ran?             → state --json | jq");
    println!("  * Long-running workflow?                  → resume (with NodeStore)");
    println!("  * Something came back wrong?              → debug-failure recipe");
    println!("  * New to the tool?                        → skill --recipe agent-onboarding");
    println!();
    println!("Common env vars:");
    println!(
        "  LAYA_DSL_DIR     pin the spec root (overrides the layered repo/user/builtin lookup)"
    );
    println!("  LAYA_USER_DSL_DIR  per-user spec root (default: ~/.config/laya-workflow/dsl)");
    println!("  LAYA_PLUGIN_DIR  pin the plugin root (also the `plugin install` target)");
    println!(
        "  LAYA_USER_PLUGIN_DIR  per-user plugin root (default: ~/.config/laya-workflow/plugins)"
    );
    println!("  LAYA_TEST_PYTHON python3 binary for mock agent capability");
    println!("  LAYA_MOCK3       host:redis:nats:mqtt:smtp:s3:prom:kafka:udp");
    println!("  LAYA_AGENT_BIN_DIR  dir containing `cxgo` / `cmdgo` wrappers");
    println!("  LAYA_GOAL_DIR    allow-list root for goal_runner goal docs");
    println!();
    println!("Read on: `skill --section <name>` for the one you need.");
}

fn print_section(name: &str) -> Result<()> {
    let body = match name {
        "overview" => SKILL_OVERVIEW,
        "validate" => SKILL_VALIDATE,
        "run" => SKILL_RUN,
        "list" => SKILL_LIST,
        "apps" => SKILL_APPS,
        "describe" => SKILL_DESCRIBE,
        "demo" => SKILL_DEMO,
        "optimize" => SKILL_OPTIMIZE,
        "improve" => SKILL_IMPROVE,
        "export" => SKILL_EXPORT,
        "state" => SKILL_STATE,
        "resume" => SKILL_RESUME,
        "replay" => SKILL_REPLAY,
        "persist" => SKILL_PERSIST,
        "dsl" => SKILL_DSL,
        "tests" => SKILL_TESTS,
        "safety" => SKILL_SAFETY,
        "plugins" => SKILL_PLUGINS,
        other => {
            eprintln!("# no such section: {other}");
            eprintln!("run `laya-workflow skill --list` to see the names.");
            std::process::exit(2);
        }
    };
    print!("{body}");
    Ok(())
}

fn print_recipe(name: &str) -> Result<()> {
    let body = match name {
        "first-run" => RECIPE_FIRST_RUN,
        "live-server" => RECIPE_LIVE_SERVER,
        "debug-failure" => RECIPE_DEBUG_FAILURE,
        "checkpoint-resume" => RECIPE_CHECKPOINT_RESUME,
        "rollback-bad-iter" => RECIPE_ROLLBACK_BAD_ITER,
        "harden-spec" => RECIPE_HARDEN_SPEC,
        "agent-onboarding" => RECIPE_AGENT_ONBOARDING,
        other => {
            eprintln!("# no such recipe: {other}");
            eprintln!("run `laya-workflow skill --list` to see the names.");
            std::process::exit(2);
        }
    };
    print!("{body}");
    Ok(())
}

static SKILL_OVERVIEW: &str = include_str!("skill/sections/overview.md");
static SKILL_VALIDATE: &str = include_str!("skill/sections/validate.md");
static SKILL_RUN: &str = include_str!("skill/sections/run.md");
static SKILL_LIST: &str = include_str!("skill/sections/list.md");
static SKILL_APPS: &str = include_str!("skill/sections/apps.md");
static SKILL_DESCRIBE: &str = include_str!("skill/sections/describe.md");
static SKILL_DEMO: &str = include_str!("skill/sections/demo.md");
static SKILL_OPTIMIZE: &str = include_str!("skill/sections/optimize.md");
static SKILL_IMPROVE: &str = include_str!("skill/sections/improve.md");
static SKILL_EXPORT: &str = include_str!("skill/sections/export.md");
static SKILL_STATE: &str = include_str!("skill/sections/state.md");
static SKILL_RESUME: &str = include_str!("skill/sections/resume.md");
static SKILL_REPLAY: &str = include_str!("skill/sections/replay.md");
static SKILL_PERSIST: &str = include_str!("skill/sections/persist.md");
static SKILL_DSL: &str = include_str!("skill/sections/dsl.md");
static SKILL_TESTS: &str = include_str!("skill/sections/tests.md");
static SKILL_SAFETY: &str = include_str!("skill/sections/safety.md");
static SKILL_PLUGINS: &str = include_str!("skill/sections/plugins.md");

static RECIPE_FIRST_RUN: &str = include_str!("skill/recipes/first_run.md");
static RECIPE_LIVE_SERVER: &str = include_str!("skill/recipes/live_server.md");
static RECIPE_DEBUG_FAILURE: &str = include_str!("skill/recipes/debug_failure.md");
static RECIPE_CHECKPOINT_RESUME: &str = include_str!("skill/recipes/checkpoint_resume.md");
static RECIPE_ROLLBACK_BAD_ITER: &str = include_str!("skill/recipes/rollback_bad_iter.md");
static RECIPE_HARDEN_SPEC: &str = include_str!("skill/recipes/harden_spec.md");
static RECIPE_AGENT_ONBOARDING: &str = include_str!("skill/recipes/agent_onboarding.md");

/// The explicitly requested spec root, if any: `--dsl-dir` beats
/// `LAYA_DSL_DIR`. `None` means "no pin" — fall back to the layered lookup
/// (repo → user → builtin) resolved by `spec::spec_roots()`.
fn explicit_dsl_dir(cli: &Cli) -> Option<String> {
    cli.dsl_dir
        .clone()
        .or_else(|| std::env::var("LAYA_DSL_DIR").ok().filter(|d| !d.is_empty()))
}

fn app_workflow(app: &str) -> Result<ResilientWorkflow> {
    match app {
        "agent_gate" => Ok(apps::agent_gate::workflow()),
        "email_triage" => Ok(apps::email_triage::workflow()),
        "content_moderation" => Ok(apps::content_moderation::workflow()),
        "draft_scorer" => Ok(apps::draft_scorer::workflow()),
        "gateway" => Ok(apps::gateway_workflow()),
        other => Err(anyhow!(
            "unknown app {other:?}; expected agent_gate/email_triage/content_moderation/draft_scorer/gateway"
        )),
    }
}

fn run_apps(backend: &dyn Decide, label: &str) -> Result<()> {
    let mut total = 0usize;
    let mut pass = 0usize;
    println!("backend: {label}");

    println!("== App 1: agent_gate ==");
    for (cmd, intent, expected) in apps::agent_gate::cases() {
        let r = apps::agent_gate::run(backend, cmd, intent, "/")?;
        let action = r["action"].as_str().unwrap_or("");
        let ok = expected.split('|').any(|e| e == action);
        total += 1;
        pass += ok as usize;
        println!(
            "  {} {:8} {:>7.1}ms  {}",
            if ok { "OK " } else { "!! " },
            action,
            r["latency_ms"].as_f64().unwrap_or(0.0),
            cmd
        );
    }

    println!("== App 2: email_triage ==");
    for (subj, body, sender, expected) in apps::email_triage::cases() {
        let r = apps::email_triage::run(backend, subj, body, sender)?;
        let action = r["action"].as_str().unwrap_or("");
        let ok = action == expected;
        total += 1;
        pass += ok as usize;
        println!(
            "  {} {:14} cat={:9} {:>7.1}ms  {}",
            if ok { "OK " } else { "!! " },
            action,
            r["category"].as_str().unwrap_or(""),
            r["latency_ms"].as_f64().unwrap_or(0.0),
            subj
        );
    }

    println!("== App 3: content_moderation ==");
    for (text, expected) in apps::content_moderation::cases() {
        let r = apps::content_moderation::run(backend, text, "user")?;
        let action = r["action"].as_str().unwrap_or("");
        let ok = action == expected;
        total += 1;
        pass += ok as usize;
        println!(
            "  {} {:8} label={:11} {:>7.1}ms  {}",
            if ok { "OK " } else { "!! " },
            action,
            r["label"].as_str().unwrap_or(""),
            r["latency_ms"].as_f64().unwrap_or(0.0),
            &text[..text.len().min(44)]
        );
    }

    println!("== App 4: draft_scorer ==");
    for (text, audience, expected) in apps::draft_scorer::cases() {
        let r = apps::draft_scorer::run(backend, text, audience)?;
        let sug = r["suggestion"].as_str().unwrap_or("");
        let ok = expected.split('|').any(|e| e == sug);
        total += 1;
        pass += ok as usize;
        println!(
            "  {} {:8} {:>7.1}ms  {}",
            if ok { "OK " } else { "!! " },
            sug,
            r["latency_ms"].as_f64().unwrap_or(0.0),
            &text[..text.len().min(44)]
        );
    }

    println!("\ntotal: {pass}/{total} cases matched");
    Ok(())
}

fn run_optimize(
    backend: &dyn Decide,
    label: &str,
    steps: usize,
    strategies: Vec<String>,
) -> Result<()> {
    use laya_workflow::optimizer::LayaOptimizerLoop;

    println!("backend: {label}");
    let mut lp =
        LayaOptimizerLoop::new(strategies, /*maximize=*/ true, steps).with_task("demo_quadratic");

    // A deterministic "solver": proposes delta_params that move k toward 5.
    let propose = |prompt: &str| {
        let cur = parse_k(prompt);
        let target = 5.0;
        let next = cur + (target - cur) * 0.5;
        format!("{{\"delta_params\": {{\"k\": {next:.3}}}, \"rationale\": \"move k toward 5\"}}")
    };
    // Objective: -((k-5)^2), maximised at k = 5.
    let eval = |params: &Value| -> Result<f64> {
        let k = params.get("k").and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(-(k - 5.0) * (k - 5.0))
    };

    let out = lp.run(backend, &propose, &eval)?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

/// Accuracy self-improvement: score current decisions, derive an update from the
/// misses, and adopt it only if a hold-out split does not regress.
fn run_improve(dir: &str, rounds: usize, score_only: bool) -> Result<()> {
    use laya_workflow::accuracy::{self, AccuracyLoop};

    let lp = AccuracyLoop::open(dir)?;
    println!("policy dir: {}", lp.dir().display());
    println!("revision  : {}", lp.policy().revision);

    let all = lp.evaluate_reference();
    let score = lp.score(&all);
    println!(
        "accuracy  : {}/{} = {:.1}%",
        score.correct,
        score.total,
        score.accuracy * 100.0
    );
    for (app, s) in &score.per_app {
        println!("  {app:20} {}/{}", s.correct, s.total);
    }

    if score_only {
        println!(
            "{}",
            serde_json::to_string_pretty(&accuracy::summarize(&score))?
        );
        return Ok(());
    }

    if score.misses.is_empty() {
        println!("no misses; nothing to improve");
        return Ok(());
    }
    println!("misses    : {}", score.misses.len());

    for i in 1..=rounds {
        let rep = lp.step()?;
        println!(
            "round {i}: {} (train {:.1}% -> {:.1}%, holdout {:.1}% -> {:.1}%)",
            rep.reason,
            rep.train_before * 100.0,
            rep.train_after * 100.0,
            rep.holdout_before * 100.0,
            rep.holdout_after * 100.0
        );
        if !rep.accepted {
            break;
        }
        // Re-open so the next round sees the persisted policy.
        let lp = AccuracyLoop::open(dir)?;
        if lp.policy().revision != rep.revision_to {
            break;
        }
        let after = lp.score(&lp.evaluate_reference());
        println!(
            "  -> revision {} now {}/{} = {:.1}%",
            lp.policy().revision,
            after.correct,
            after.total,
            after.accuracy * 100.0
        );
    }
    Ok(())
}

/// Pull the current `k` back out of the prompt built by the optimizer loop.
fn parse_k(prompt: &str) -> f64 {
    // the prompt prints `params: {"k": 1.234}` (or empty at step 0)
    if let Some(idx) = prompt.find("\"k\":") {
        let rest = &prompt[idx + 4..];
        let num: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == 'e' || *c == '+')
            .collect();
        return num.parse::<f64>().unwrap_or(0.0);
    }
    0.0
}

fn run_demo(backend: &dyn Decide, label: &str) -> Result<()> {
    let wf = apps::gateway_workflow();
    println!("backend: {label}");
    println!("graph:\n{}", serde_json::to_string_pretty(&wf.describe())?);

    let states = [
        json!({"command": "ls -la /tmp", "intent": "list files",
               "subject": "Re: invoice duplicate charge",
               "body": "Hi, we were billed twice for March. Please refund today or we cancel.",
               "from": "user@acme.com"}),
        json!({"command": "rm -rf ./build", "intent": "clean build dir",
               "subject": "cleanup", "body": "remove build artifacts", "from": "ci@x.com"}),
    ];
    for st in states {
        let out = wf.run(backend, &st)?;
        println!(
            "\nstate command={} -> final_action={} iterations={}",
            st["command"],
            out.final_action(),
            out.iterations
        );
        println!("{}", serde_json::to_string_pretty(&out.to_json())?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_shorthand_merges_into_object_state() {
        let value = run_state(r#"{"result_count":5}"#, Some("  jev ai  ")).unwrap();
        assert_eq!(value, json!({"result_count":5, "query":"jev ai"}));
    }

    #[test]
    fn query_shorthand_requires_object_state() {
        let err = run_state("[]", Some("jev ai")).unwrap_err().to_string();
        assert!(err.contains("JSON object"), "unexpected error: {err}");
    }
}
