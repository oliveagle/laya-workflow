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
use laya_workflow::orchestrate::BrowserBackend;
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
    /// Evaluate a workflow against a labeled JSONL dataset (dev/holdout,
    /// slice breakdowns, confusion matrix, review queue). Port of the
    /// awesome-jev `evaluations/run.py` pattern.
    Evaluate {
        /// Path to the workflow spec JSON (must declare an `evaluation` block)
        #[arg(long)]
        spec: String,
        /// Path to the labeled JSONL dataset
        #[arg(long)]
        dataset: String,
        /// Which split to run: development | holdout | all
        #[arg(long, default_value = "all")]
        split: String,
        /// Comma-separated list of valid department keys (dataset validation)
        #[arg(long, default_value = "billing,technical,account,other")]
        departments: String,
        /// Write the summary JSON to this path (parent dirs created)
        #[arg(long)]
        output: Option<String>,
        /// Refuse to evaluate holdout if the spec hash differs from the
        /// recorded one in <output>.fingerprint (freeze config first).
        #[arg(long, default_value_t = false)]
        freeze: bool,
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
        /// Stream live per-node progress to stderr while running (one line per
        /// node as it completes). Stdout stays the final JSON verdict only.
        #[arg(long, default_value_t = false)]
        progress: bool,
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
    /// DSL smoke: validate every spec under dsl/ and run the labelled sample
    /// states against the expected labels. Replaces laya-workflow dsl smoke.
    Dsl {
        #[command(subcommand)]
        cmd: DslCmd,
    },
    /// Manage Rhai plugins: install one from a git repo, list them, show where
    /// they are looked up. See `skill --section plugins`.
    Plugin {
        #[command(subcommand)]
        cmd: PluginCmd,
    },
    /// Ensure a local browser backend is up on 127.0.0.1:<port> (idempotent).
    /// `--backend chrome` drives a CDP Chrome today; the selector keeps the call
    /// shape stable if a better backend arrives. Pair it with `server` and a
    /// `chrome_cdp` capability to drive a real local page.
    Browser {
        #[command(subcommand)]
        cmd: BrowserCmd,
    },
    /// Hidden back-compat alias for `browser ensure --backend chrome`.
    #[command(hide = true)]
    Chrome {
        #[command(subcommand)]
        cmd: ChromeAliasCmd,
    },
    /// Long-running local HTTP server lifecycle: `ensure` (idempotent) /
    /// `start` (foreground, or `--daemon` to detach) / `stop` / `status`.
    /// `ensure` and `start` print `BASE=<url>` so a workflow can capture it.
    Server {
        #[command(subcommand)]
        cmd: ServerCmd,
    },
    /// Generic MCP stdio server + registered tool sets (laya_mem today, more
    /// later). Reads JSON-RPC 2.0 from stdin, writes JSON-RPC to stdout.
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },
    /// HTAP database lifecycle: `serve` a local SQLite + DuckDB daemon (the
    /// *server* mode of the `db` capability), plus `ensure`/`stop`/`status`.
    /// Without it, `kind: "db"` runs *embedded* — the same ops, local CLIs,
    /// per call.
    Db {
        #[command(subcommand)]
        cmd: DbCmd,
    },
    /// Create the state root and install everything this tool bundles: every
    /// plugin, the laya-mem specs, and the SQLite store's parent directory.
    /// Idempotent — re-running it is safe and keeps what is already there.
    /// Pass `--force` to overwrite existing plugins and specs with the
    /// shipped versions (the refresh after a `git pull`).
    Install {
        /// Overwrite existing installs instead of keeping them.
        #[arg(long, default_value_t = false)]
        force: bool,
        /// Only create the directories; install nothing.
        #[arg(long, default_value_t = false)]
        dirs_only: bool,
    },
    /// Self-update: install the newest `laya-workflow` from this repo's GitHub
    /// Releases (or a pinned `--tag`), replacing the running binary in place.
    /// `--check` only reports whether an update exists. See
    /// `skill --section update`.
    Update {
        /// Only report the latest release and whether an update exists; change nothing.
        #[arg(long, default_value_t = false)]
        check: bool,
        /// Install this release tag instead of the latest (e.g. v0.9.0).
        #[arg(long, value_name = "TAG")]
        tag: Option<String>,
        /// Reinstall even when the resolved version equals the running one.
        #[arg(long, default_value_t = false)]
        force: bool,
        /// Repo to update from (default: oliveagle/laya-workflow).
        #[arg(long, default_value = "oliveagle/laya-workflow")]
        repo: String,
        /// Force the platform target triple instead of detecting it.
        #[arg(long, value_name = "TRIPLE")]
        target: Option<String>,
        /// Override the GitHub API base URL (tests / mirrors).
        #[arg(long, hide = true)]
        api_base: Option<String>,
    },

    /// The laya-mem memory gate: where its specs and store live, and how to
    /// restore the shipped specs.
    LayaMem {
        #[command(subcommand)]
        cmd: LayaMemCmd,
    },
    /// BDD: compile a `.feature` into a workflow spec, with accuracy as the
    /// hard gate. `bdd build` replaces laya-workflow bdd build; the sub-checks
    /// replace their scripts/bdd/*_check.py counterparts.
    Bdd {
        #[command(subcommand)]
        cmd: BddCmd,
    },
    /// Rule enforcement: compile AGENTS.md rules into a rubric, check a diff
    /// against it, and audit the checks. Port of coldteadotai/abide.
    Rules {
        #[command(subcommand)]
        cmd: RulesCmd,
    },
    /// Offline mock services: serve the protocol/HTTP mocks the integration
    /// tests need (`mock serve`), or the line-delimited JSON stdio agent
    /// (`mock stdio`). Replaces laya-workflow mock serve and laya-workflow mock serve.
    Mock {
        #[command(subcommand)]
        cmd: MockCmd,
    },
    /// Bench / diagnostic tools (Rust ports of bench/*.py).
    Bench {
        #[command(subcommand)]
        cmd: BenchCmd,
    },
    /// `feishu-collect [chats_limit] [page_size] [--print-knobs]` — feed the
    /// JSON from `lark-cli im +chat-list` on stdin, get the unread digest on
    /// stdout. Replaces the inline `python3 -c` in the Feishu plugin collect.sh.
    FeishuCollect {
        /// Up to N chats to fetch.
        #[arg(name = "chats_limit", default_value = "15")]
        chats_limit: String,
        /// Up to N messages per chat to consider for unread detection.
        #[arg(name = "page_size", default_value = "20")]
        page_size: String,
        /// Print resolved knobs and exit (plugin check contract).
        #[arg(long, default_value_t = false)]
        print_knobs: bool,
    },
    /// Run the singleton-Chrome demo against a temporary localhost page
    /// served by the binary itself. Replaces bench/browser_demo.py.
    BrowserDemo {
        /// Optional demo site root (default <manifest>/bench/demo-site).
        #[arg(long)]
        site: Option<String>,
        /// Optional port (default: first free of 18777..18787).
        #[arg(long)]
        port: Option<u16>,
        /// Optional spec override (default dsl/browser/browser_singleton.json).
        #[arg(long)]
        spec: Option<String>,
        /// Text to type into the demo page.
        #[arg(long)]
        text: Option<String>,
    },
    /// Send a local notification. On macOS this posts a real Notification Center
    /// banner (via `osascript`); it can also append to a log file and/or bell.
    Notify {
        /// Notification body text.
        #[arg(long)]
        message: String,
        /// Banner title (macOS).
        #[arg(long, default_value = "laya-workflow")]
        title: String,
        /// Banner subtitle (macOS).
        #[arg(long, default_value = "")]
        subtitle: String,
        /// Channel: auto | macos | log | both (default auto -> macos on macOS).
        #[arg(long, default_value = "auto")]
        channel: String,
        /// macOS banner sound name (e.g. Glass); empty = silent.
        #[arg(long, default_value = "")]
        sound: String,
        /// Log file for the log/both channels.
        #[arg(long)]
        path: Option<String>,
        /// Also emit a terminal bell.
        #[arg(long, default_value_t = false)]
        bell: bool,
    },
}

#[derive(Subcommand)]
enum BddCmd {
    /// Build every artifact a BDD document drives, with accuracy as the hard
    /// gate: compile + validate + 100% coverage + empty-plan error.
    Build {
        /// Feature files (default: all of bdd/features/*.feature).
        #[arg(name = "features", num_args = 0..)]
        features: Vec<String>,
        /// Output dir for generated artifacts.
        #[arg(long, default_value = "target/bdd-build")]
        out: String,
        /// local = hermetic fixtures; production = real target (--base-url).
        #[arg(long, default_value = "local")]
        profile: String,
        /// Keep only scenarios with this tag (e.g. @production).
        #[arg(long)]
        filter: Option<String>,
        /// (production) run every scenario, not just @production-tagged ones.
        #[arg(long, default_value_t = false)]
        all: bool,
        /// Minimum per-feature step coverage % (default 100 = accuracy-first).
        #[arg(long, default_value_t = 100.0)]
        coverage_min: f64,
        /// Skip the `validate` gate (for docs/demo only).
        #[arg(long, default_value_t = false)]
        no_validate: bool,
        /// Production base URL (default $BDD_BASE_URL).
        #[arg(long)]
        base_url: Option<String>,
        /// What to emit: all | spec | manifest | coverage.
        #[arg(long, default_value = "all")]
        emit: String,
        /// For out-of-vocabulary steps, print a needle suggestion + confidence.
        #[arg(long, default_value_t = false)]
        assist: bool,
    },
    /// Compile a `.feature` into a laya-workflow spec (no gate).
    Transpile {
        /// Feature files.
        #[arg(name = "features", num_args = 1..)]
        features: Vec<String>,
        /// Write generated specs into this directory.
        #[arg(long)]
        out: Option<String>,
        /// List scenarios, generate nothing.
        #[arg(long, default_value_t = false)]
        list: bool,
        /// Generate, verify each spec is well-formed, write nothing.
        #[arg(long, default_value_t = false)]
        check: bool,
        /// Chrome wrapper that forces headless.
        #[arg(long)]
        chrome_bin: Option<String>,
    },
    /// Open a greenfield decision workflow: write a minimal valid `@decision`
    /// `.feature` skeleton from an intent, self-verified to pass `bdd build`.
    Scaffold {
        /// Workflow name (also the file stem).
        #[arg(long)]
        name: String,
        /// Category subdir under bdd/workflows/ (default: routing).
        #[arg(long, default_value = "routing")]
        cat: String,
        /// Natural-language intent; becomes the Feature description.
        #[arg(long)]
        intent: Option<String>,
        /// Print the skeleton to stdout instead of writing a file.
        #[arg(long, default_value_t = false)]
        stdout: bool,
        /// Overwrite an existing feature file.
        #[arg(long, default_value_t = false)]
        force: bool,
        /// Primary question name for the starter node (default: option).
        #[arg(long)]
        primary_q: Option<String>,
        /// Emit a 1-node start instead of the 2-node canonical graph.
        #[arg(long, default_value_t = false)]
        single: bool,
        /// Emit a nested-pipeline starter (prefilter -> sub-workflow -> audit)
        /// instead of the canonical 2-node graph.
        #[arg(long, default_value_t = false)]
        nested: bool,
        /// Starter shape: two_node | single | nested | gate | loop
        /// (overrides --single / --nested).
        #[arg(long)]
        pattern: Option<String>,
    },
    /// Print the decision authoring vocabulary - the step templates the
    /// `@decision` compiler is built around (the list behind `did you mean`).
    /// `--json` emits the same list as machine-readable JSON.
    Vocabulary {
        /// Emit machine-readable JSON.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Prove the step vocabulary maps each step to the op it is supposed to run.
    VocabularyCheck,
    /// Prove the decision authoring vocabulary has not drifted from the compiler:
    /// every template `bdd vocabulary` prints still compiles, and every step shape
    /// the `@decision` compiler accepts is advertised (no silent gap).
    DecisionVocabCheck,
    /// Check the hand-written BDD probe specs without Chrome.
    ProbeCheck,
    /// Run the bdd plugin's argument-validation errors and hold the list complete.
    ArgsProbeCheck,
    /// Keep the numbers bdd/README.md quotes honest.
    DocCheck,
    /// Execute compiled scenarios against a real headless Chrome over CDP.
    /// Local profile = hermetic fixture server; production = external target.
    Run {
        /// Feature files (default: all of bdd/features/*.feature).
        #[arg(name = "features", num_args = 0..)]
        features: Vec<String>,
        /// Only features whose path contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// Only scenarios carrying this tag (e.g. @production).
        #[arg(long)]
        tags: Option<String>,
        /// local = hermetic fixtures; production = real target (--base-url).
        #[arg(long, default_value = "local")]
        profile: String,
        /// Production base URL (default $BDD_BASE_URL).
        #[arg(long)]
        base_url: Option<String>,
        /// Pin the CDP port (serial only).
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Maximum concurrent scenarios. 0 = auto (one worker per core,
        /// capped at 8); 1 = serial, the byte-identical default. A reduction
        /// (auto cap, plan size, `--port`) is named on the run's header line.
        #[arg(long, default_value_t = 1)]
        jobs: usize,
        /// Per-scenario timeout in seconds.
        #[arg(long, default_value_t = 180)]
        timeout_secs: u64,
        /// Keep the generated specs in --out.
        #[arg(long, default_value_t = false)]
        keep: bool,
        /// Output dir for generated specs.
        #[arg(long, default_value = "target/bdd-specs")]
        out: String,
        /// Live-fire against real external sites (no fixtures, no probes).
        #[arg(long, default_value_t = false)]
        live: bool,
        /// Write a per-scenario JSON report here.
        #[arg(long)]
        report: Option<String>,
        /// Fail (exit 1) instead of skipping cleanly when no browser is found.
        /// Without it, a browserless box prints a "skipped" line and exits 0.
        #[arg(long, default_value_t = false)]
        require_browser: bool,
        /// Attach to an existing CDP endpoint (e.g. http://127.0.0.1:9222)
        /// instead of launching Chrome. The endpoint must already be running;
        /// attach never spawns. Implies serial execution.
        #[arg(long)]
        cdp: Option<String>,
    },
    /// Score the authoring corpus: expressiveness, vocabulary recall/precision,
    /// the @outputs contract, and (optionally) an end-to-end run. Answers "how
    /// fast and how accurately can an agent author a workflow from an intent".
    Score {
        /// Turn the hard targets (compile, coverage, recall, precision,
        /// contract) into a gate: any shortfall exits non-zero.
        #[arg(long, default_value_t = false)]
        check: bool,
        /// Also run every task's reference workflow against real Chrome.
        #[arg(long, default_value_t = false)]
        run: bool,
        /// Write the metrics as JSON here.
        #[arg(long)]
        json: Option<String>,
        /// Score only these task ids (default: every task in the corpus).
        #[arg(name = "tasks", num_args = 0..)]
        tasks: Vec<String>,
    },
    /// Run the live-fire corpus (bdd/workflows/browser/*.feature) against real sites.
    Live {
        /// Feature files (default: every bdd/workflows/browser/*.feature).
        #[arg(name = "features", num_args = 0..)]
        features: Vec<String>,
        /// Only features whose path contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// Output dir for generated specs.
        #[arg(long, default_value = "target/bdd-live")]
        out: String,
        /// Per-scenario timeout in seconds.
        #[arg(long, default_value_t = 180)]
        timeout_secs: u64,
        /// Write a per-scenario JSON report here.
        #[arg(long)]
        json: Option<String>,
    },
    /// Decision-scoped live gate: drive every `@decision` workflow's registered
    /// sample states through a live System-1 backend and require the whole
    /// corpus to walk **structurally clean** (terminates, in-contract, no
    /// backend error). Label agreement with the offline expectation is printed
    /// as information, never gated - a live model's labels are *meant* to
    /// differ from the heuristic-shaped offline expects.
    LiveDecisions {
        /// Live backend base URL (default $BDD_BASE_URL, else http://127.0.0.1:18405).
        #[arg(long)]
        base_url: Option<String>,
        /// Only walk features whose path or stem contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// Write the per-spec verdicts as JSON here.
        #[arg(long)]
        json: Option<String>,
    },
    /// Compare System-1 backends over the decision corpus and rank them: drive
    /// every `@decision` spec's sample states through each backend, then rank the
    /// usable ones by label agreement (then latency). A backend that fails a
    /// single-option `choice` preflight probe is marked unusable, never ranked —
    /// that probe is exactly the P200 stale-code shape. Default backends are the
    /// keep-list (soft-decider-421m :18405, Laya FT :18400); pass `--backend` to
    /// add challengers (repeatable).
    CompareSystems {
        /// A System-1 backend base URL to compare. Repeatable.
        #[arg(long)]
        backend: Vec<String>,
        /// Only walk features whose path or stem contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// Write the comparison as JSON here.
        #[arg(long)]
        json: Option<String>,
        /// Walk every backend even if its preflight probe fails.
        #[arg(long)]
        no_preflight: bool,
    },
    /// Round-trip gate: every @decision feature must compile back to the same
    /// decision-graph spec as its checked-in dsl/<cat>/<name>.json counterpart.
    RoundTrip {
        /// Write the per-feature verdicts as JSON here.
        #[arg(long)]
        json: Option<String>,
    },
    /// App-parity gate: replay every built-in app's reference case (`laya-workflow
    /// apps`) end to end through the app's checked-in BDD port spec and
    /// require the engine's label to match the app's expectation. The port is the
    /// app's routing as *data*; this holds the two together, offline.
    AppParity {
        /// Write the per-case verdicts as JSON here.
        #[arg(long)]
        json: Option<String>,
    },
    /// Prove a cross-repo conversion is closed: every upstream scenario has a
    /// BDD port (a `@decision` feature and a checked-in spec) and every port has
    /// an upstream source. The checked-in seal under `bench/bdd/port_pins/`
    /// proves it without a second checkout; `--sources` adds a live checkout and
    /// puts the seal on trial.
    PortCheck {
        /// Directory of upstream scenarios, one subdirectory each (for example
        /// an ole-eval `code/qwen_vm/app_scenarios` checkout). Optional when
        /// `--pin` supplies the sealed source set.
        #[arg(long, default_value = "")]
        sources: String,
        /// Port category: `bdd/workflows/<cat>/` and `dsl/<cat>/`.
        #[arg(long, default_value = "ole_eval")]
        cat: String,
        /// Write the diff as JSON here.
        #[arg(long)]
        json: Option<String>,
        /// The sealed source set, `bench/bdd/port_pins/<cat>.json`. Proves the
        /// port is closed without a second checkout; with `--sources` it also
        /// re-verifies the seal (same names, same source fingerprints).
        #[arg(long)]
        pin: Option<String>,
        /// Seal the live `--sources` directory into this manifest (a deliberate
        /// act after a review, never a gate).
        #[arg(long)]
        write_pin: Option<String>,
    },
    /// Audit the scope census: every known upstream workflow is accounted for -
    /// ported (as a `@decision` graph), named as an optimizer search-space with a
    /// reason, named as `not_a_workflow` (engine / model / harness, one reason per
    /// group), or named under `gate_pending` (gate-shaped but not yet ported, one
    /// reason naming the gate). Reads `bench/bdd/scope_census.json`; `--sources
    /// <ole-eval root>` makes the census match the live `.py` files, so a new
    /// upstream workflow cannot be silently skipped.
    ScopeCheck {
        /// An ole-eval (or similar) checkout root; the census `upstream` paths
        /// are resolved under it. Omit to judge the census on its own.
        #[arg(long)]
        sources: Option<String>,
        /// Write the report as JSON here.
        #[arg(long)]
        json: Option<String>,
    },
    /// Import: turn an existing workflow spec (a `.json`) back into a
    /// `@decision` `.feature` that compiles to the very same graph — the
    /// inverse of `transpile`; the tool that lifts any known workflow into BDD.
    /// Pass one `<SPEC>`, or `--dir <DIR>` for a whole tree at once.
    Import {
        /// Path to the spec `.json` to import (omit with `--dir`).
        #[arg(required_unless_present = "dir")]
        spec: Option<String>,
        /// Import every decision spec under this directory.
        #[arg(long, conflicts_with = "spec")]
        dir: Option<String>,
        /// Write the `.feature` here — a file for one spec, a directory for `--dir`.
        #[arg(long)]
        out: Option<String>,
        /// File the port under `bdd/workflows/<cat>/` and copy its spec to `dsl/<cat>/`.
        #[arg(long)]
        into: Option<String>,
        /// Write the report as JSON here (`--dir` only).
        #[arg(long)]
        json: Option<String>,
        /// Overwrite an existing target instead of reporting a collision.
        #[arg(long)]
        force: bool,
    },
    /// Import gate: prove every checked-in spec is recoverable from a feature
    /// that `import` emits (the reverse of `round-trip`).
    ImportCheck {
        /// Write the per-spec verdicts as JSON here.
        #[arg(long)]
        json: Option<String>,
    },
    /// Corpus-completeness gate: every checked-in `.feature` compiles, and every
    /// one is covered by a named gate bucket - so no document (including the
    /// quickstart examples) ships unverified.
    FeaturesCheck {
        /// Write the per-feature verdicts as JSON here.
        #[arg(long)]
        json: Option<String>,
    },
    /// Quantify BDD authoring vs hand-written JSON DSL on the live corpus.
    Compare {
        /// Write the comparison as JSON here.
        #[arg(long)]
        json: Option<String>,
        /// Compare only these case ids (default: every case in the corpus).
        #[arg(name = "cases", num_args = 0..)]
        cases: Vec<String>,
    },
}

#[derive(Subcommand)]
enum MockCmd {
    /// Serve the offline mock services (Redis/NATS/MQTT/SMTP/S3/Prom/Kafka/
    /// HTTP score+agent/web/UDP/RPC/GraphQL/chat/MCP/vector/webhook) on the
    /// ports requested. Prints `{"ready": true, "listeners": {...}}` once every
    /// listener answers, so a launcher can wait for the line instead of
    /// sleeping. Replaces laya-workflow mock serve and laya-workflow mock serve.
    Serve {
        /// Redis protocol mock port.
        #[arg(long, default_value_t = 0)]
        redis: u16,
        /// NATS protocol mock port.
        #[arg(long, default_value_t = 0)]
        nats: u16,
        /// MQTT protocol mock port.
        #[arg(long, default_value_t = 0)]
        mqtt: u16,
        /// SMTP protocol mock port.
        #[arg(long, default_value_t = 0)]
        smtp: u16,
        /// UDP echo mock port.
        #[arg(long, default_value_t = 0)]
        udp: u16,
        /// S3-style HTTP mock port.
        #[arg(long, default_value_t = 0)]
        s3: u16,
        /// Prometheus-style HTTP mock port.
        #[arg(long, default_value_t = 0)]
        prom: u16,
        /// Kafka-style mock port.
        #[arg(long, default_value_t = 0)]
        kafka: u16,
        /// Generic web HTTP mock port.
        #[arg(long, default_value_t = 0)]
        web: u16,
        /// Score HTTP endpoint port.
        #[arg(long, default_value_t = 0)]
        score: u16,
        /// Agent HTTP endpoint port.
        #[arg(long, default_value_t = 0)]
        agent: u16,
        /// RPC HTTP mock port.
        #[arg(long, default_value_t = 0)]
        rpc: u16,
        /// GraphQL HTTP mock port.
        #[arg(long, default_value_t = 0)]
        graphql: u16,
        /// Chat HTTP mock port.
        #[arg(long, default_value_t = 0)]
        chat: u16,
        /// MCP HTTP mock port.
        #[arg(long, default_value_t = 0)]
        mcp: u16,
        /// Vector HTTP mock port.
        #[arg(long, default_value_t = 0)]
        vector: u16,
        /// Webhook HTTP mock port.
        #[arg(long, default_value_t = 0)]
        webhook: u16,
    },
    /// Run the line-delimited JSON agent on stdin/stdout (the stdio transport
    /// mock used by integration tests). Replaces `mock_server.py --stdio`.
    Stdio,
}

#[derive(Subcommand)]
enum BenchCmd {
    /// Side-by-side: offline heuristic backend vs live laya-tch @ /v1/systemone
    /// on the same laya_mem specs. Replaces bench/backend_comparison.py.
    BackendComparison {
        /// laya-tch base URL; empty string skips the live run.
        #[arg(long)]
        base_url: Option<String>,
        /// Skip the offline heuristic run.
        #[arg(long, default_value_t = false)]
        skip_heuristic: bool,
        /// Skip the live laya-tch run.
        #[arg(long, default_value_t = false)]
        skip_live: bool,
    },
    /// jev semantic gate vs GBNF structural gate on candidate memory records.
    /// Replaces bench/jev_vs_gbnf.py (pure logic, no model needed).
    JevVsGbnf {
        /// Write the markdown report to this path (in addition to stdout).
        #[arg(long)]
        md_out: Option<String>,
    },
    /// Probe: can the on-device Needle 3 compile Gherkin BDD steps into spec
    /// JSON? Replaces bench/bdd_to_needle.py (requires ~/.laya-workflow/models/
    /// needle3.cact).
    BddToNeedle,
    /// A/B bench: Needle 3 vs offline heuristics on routing / extraction /
    /// embedding / end-to-end. Replaces bench/needle_vs_heuristic.py.
    NeedleVsHeuristic,
    /// GBNF-strict stress: mutate a corpus of /system_one payloads and probe
    /// the rejection + invariant surface of a live laya-tch. Replaces
    /// bench/gbnf_strict_stress.py.
    GbnfStress {
        /// laya-tch base URL.
        #[arg(long, default_value = "http://127.0.0.1:8400")]
        base_url: String,
        /// Requests per mutation class.
        #[arg(long, default_value_t = 10)]
        n_per_class: usize,
        /// PRNG seed (deterministic corpus).
        #[arg(long, default_value_t = 42)]
        seed: u64,
        /// Write the markdown report to this path (in addition to stdout).
        #[arg(long)]
        out: Option<String>,
    },
}

#[derive(Subcommand)]
enum DslCmd {
    /// Validate every spec under dsl/ (or --dsl-dir), then run the sample
    /// states recorded in bench/dsl_smoke_states.json against the labels in
    /// bench/dsl_smoke_expect.json. Exit 1 on any validate or label failure.
    Smoke {
        /// Pin the dsl root instead of <manifest>/dsl.
        #[arg(long)]
        dsl_dir: Option<String>,
        /// Live laya-tch base-url; omit for the offline heuristic backend.
        #[arg(long)]
        base_url: Option<String>,
        /// Only run specs whose file stem contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// Quiet: only failures + the final summary line.
        #[arg(long, default_value_t = false)]
        quiet: bool,
    },
}

#[derive(Subcommand)]
enum RulesCmd {
    /// Create `.rules/` with a valid empty rubric + `.rulesignore`.
    Init,
    /// Validate a rubric file's structure (default `.rules/rubric.json`).
    Validate {
        /// Rubric file to validate (default: `<root>/.rules/rubric.json`).
        #[arg(long)]
        file: Option<String>,
    },
    /// Judge a diff against the rubric and print the hook-output JSON
    /// (`{kind: silent|notice|block}`). Offline by default; `--base-url`
    /// judges with a live Jev model instead of the offline heuristics.
    Check {
        /// Rubric file (default: `<root>/.rules/rubric.json`).
        #[arg(long)]
        file: Option<String>,
        /// Phase to judge: edit (per hunk) or turn (whole change).
        #[arg(long, default_value = "edit")]
        phase: String,
        /// Single-file hunk text read from this file. Requires --file-path.
        #[arg(long)]
        diff_file: Option<String>,
        /// Repo-relative path of the file when using --diff-file.
        #[arg(long)]
        file_path: Option<String>,
        /// Multi-file diff as JSON: a `{file,text}` object or array.
        #[arg(long)]
        diff_json: Option<String>,
        /// Multi-file diff JSON read from this file (`-` = stdin). Use instead
        /// of --diff-json when the diff is too large for an argv.
        #[arg(long)]
        diff_json_file: Option<String>,
        /// The user's task, given to the model as state.
        #[arg(long)]
        task: Option<String>,
        /// Live Jev model endpoint (e.g. http://127.0.0.1:8400). Omit for offline.
        #[arg(long)]
        base_url: Option<String>,
        /// Session id recorded in events.jsonl.
        #[arg(long)]
        session_id: Option<String>,
        /// Prompt/turn id recorded in events.jsonl.
        #[arg(long)]
        prompt_id: Option<String>,
        /// Repo root to look up .rules/ from (default: walk up from cwd).
        #[arg(long)]
        root: Option<String>,
    },
    /// Summarise the events log (~/.laya-workflow/rules/events.jsonl): checks, blocks, per-rule bands.
    Report {
        /// Repo root holding `.rules/` (default: walk up from cwd).
        #[arg(long)]
        root: Option<String>,
    },
    /// Pretty-print every entry in the events log (~/.laya-workflow/rules/events.jsonl).
    Audit {
        /// Repo root holding `.rules/` (default: walk up from cwd).
        #[arg(long)]
        root: Option<String>,
    },
    /// Emit the prompt that turns AGENTS.md / CLAUDE.md into a rubric. Run the
    /// printed prompt in an agent session inside the repo, then `rules check`.
    Compile {
        /// Repo root to read AGENTS.md from (default: walk up from cwd).
        #[arg(long)]
        root: Option<String>,
        /// Also write `.rules/rubric.json` scaffold first (via `init`).
        #[arg(long, default_value_t = false)]
        init: bool,
    },
}

#[derive(Subcommand)]
enum McpCmd {
    /// Start the generic MCP stdio server. Reads JSON-RPC 2.0 from stdin,
    /// writes one JSON-RPC object per line to stdout. Registers every
    /// built-in tool set (laya_mem today; more added via the same pattern).
    Serve {
        /// Directory holding the laya_mem DSL specs. Overrides $LAYA_MEM_SPEC_DIR.
        #[arg(long)]
        spec_dir: Option<String>,
        /// SQLite database file for the laya_mem persist/recall tools.
        /// Overrides $LAYA_MEM_SQLITE.
        #[arg(long)]
        db_path: Option<String>,
    },
}

#[derive(Subcommand)]
enum DbCmd {
    /// Run the HTAP daemon: one process owns a SQLite file (+ optional DuckDB
    /// warehouse) and answers `db` ops over HTTP on 127.0.0.1:<port>.
    Serve {
        /// SQLite database file — the ACID system of record.
        #[arg(long)]
        sqlite: String,
        /// Optional DuckDB warehouse file (default: analytics run in memory).
        #[arg(long)]
        duckdb: Option<String>,
        /// Schema alias the SQLite file gets inside DuckDB.
        #[arg(long, default_value = "sqlite")]
        alias: String,
        /// Bind host (localhost only by default).
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Bind port (default $LAYA_DB_PORT, else 18767).
        #[arg(long)]
        port: Option<u16>,
        /// Detach (own session, log redirected) instead of running in the foreground.
        #[arg(long, default_value_t = false)]
        daemon: bool,
    },
    /// Start the daemon if nothing healthy answers on the port, then print BASE.
    Ensure {
        #[arg(long)]
        sqlite: String,
        #[arg(long)]
        duckdb: Option<String>,
        #[arg(long, default_value = "sqlite")]
        alias: String,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long)]
        port: Option<u16>,
    },
    /// Stop a daemon-started HTAP server (SIGTERM + clean the pid/base files).
    Stop {
        #[arg(long)]
        port: Option<u16>,
    },
    /// Report RUNNING/STOPPED for the port; exits non-zero when stopped.
    Status {
        #[arg(long)]
        port: Option<u16>,
    },
}

#[derive(Subcommand)]
enum BrowserCmd {
    /// Launch (or confirm) the backend on an isolated profile.
    Ensure {
        /// Browser backend (default: chrome).
        #[arg(long, value_enum, default_value = "chrome")]
        backend: BrowserBackend,
        /// Endpoint port (default: the backend's — chrome reads $LAYA_CDP_PORT,
        /// else 9222).
        #[arg(long)]
        port: Option<u16>,
        /// Chrome executable (chrome backend; default $CHROME_BIN, else the
        /// platform path).
        #[arg(long = "chrome-bin")]
        chrome_bin: Option<String>,
        /// Isolated profile dir (chrome backend; default $LAYA_CDP_PROFILE, else
        /// /tmp/...). The headless kind reads $LAYA_CDP_HEADLESS_PROFILE.
        #[arg(long)]
        profile: Option<String>,
        /// Launch windowless (`--headless=new`) — the headless singleton, on its
        /// own default port 9232 + profile, so it can run alongside the GUI one.
        /// For servers / CI where no window can exist; the GUI kind is preferred
        /// for driving real pages. Also enabled by LAYA_CDP_HEADLESS=1.
        #[arg(long)]
        headless: bool,
    },
}

/// The deprecated `chrome ensure` spellings, forwarded to the chrome backend.
#[derive(Subcommand)]
enum ChromeAliasCmd {
    /// Equivalent to `browser ensure --backend chrome`.
    Ensure {
        #[arg(long)]
        port: Option<u16>,
        #[arg(long = "chrome-bin")]
        chrome_bin: Option<String>,
        #[arg(long)]
        profile: Option<String>,
        /// Same as `browser ensure --headless` (windowless instance).
        #[arg(long)]
        headless: bool,
    },
}

#[derive(Subcommand)]
enum ServerCmd {
    /// Start the server if nothing healthy answers on the port, then print BASE.
    Ensure {
        #[arg(long, default_value_t = 18766)]
        port: u16,
        /// Shell command that starts the server (required when nothing is up).
        #[arg(long)]
        command: Option<String>,
        /// Liveness path; any HTTP response counts (default /healthz).
        #[arg(long = "health-path", default_value = "/healthz")]
        health_path: String,
    },
    /// Start the server and keep this process alive (Ctrl-C / SIGTERM stops).
    Start {
        #[arg(long, default_value_t = 18766)]
        port: u16,
        /// Shell command that starts the server.
        #[arg(long)]
        command: String,
        #[arg(long = "health-path", default_value = "/healthz")]
        health_path: String,
        /// Detach (own session, log redirected) instead of running in the foreground.
        #[arg(long, default_value_t = false)]
        daemon: bool,
    },
    /// Stop a daemon-started server (SIGTERM + clean the pid/base files).
    Stop {
        #[arg(long, default_value_t = 18766)]
        port: u16,
    },
    /// Report RUNNING/STOPPED for the port; exits non-zero when stopped.
    Status {
        #[arg(long, default_value_t = 18766)]
        port: u16,
        #[arg(long = "health-path", default_value = "/healthz")]
        health_path: String,
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
        /// Install root (default: $LAYA_PLUGIN_DIR, else ~/.laya-workflow/plugins).
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
    /// Every bundled plugin must appear in both registries that document it,
    /// and every row must point at a plugin that ships. Exit 1 on any problem,
    /// 2 when the binary has no checkout to read.
    RegistryCheck,
    /// Compile a single `.rhai` file through the engine, exit 1 on a parse
    /// error. Replaces laya-workflow plugin compile.
    Compile {
        /// The .rhai file to compile-check.
        #[arg(name = "file")]
        file: std::path::PathBuf,
    },
    /// The full plugin gate: compile every shipped .rhai, assert the goofish
    /// intent routing, and run the fold + feishu integration checks. Exit 1 on
    /// any problem, 2 when the binary has no checkout to read.
    Check,
}

#[derive(Subcommand)]
enum LayaMemCmd {
    /// Print the resolved spec dir, SQLite path and backend, and check that the
    /// specs are actually present (the failure that matters is a server that
    /// starts and then answers every call with `spec not found`).
    Info,
    /// Write the embedded System-One specs into the spec dir, without touching
    /// local edits.
    Install,
    /// Overwrite the specs in the spec dir with the embedded copies, discarding
    /// local edits.
    Restore,
    /// Phase 7: migrate the SQLite store to the current schema version.
    /// Backfills the vector index for every memory that does not yet have one
    /// (i.e. rows written before the Phase-4 upgrade), bumps the
    /// `PRAGMA user_version` stamp, and writes a `migrate_backfill` audit row.
    Migrate,
    /// Wall-clock periodic consolidation watcher (Phase 8). When called with no
    /// arguments, blocks and runs sweep_once every LAYA_MEM_CONSOLIDATE_PERIOD_SECONDS
    /// (default 60s); with `--once`, runs a single sweep and exits (cron-friendly).
    ConsolidateWatch {
        /// Run a single sweep and exit (do not loop). Use from cron or a oneshot probe.
        #[arg(long)]
        once: bool,
        /// Override the period (seconds). Defaults to LAYA_MEM_CONSOLIDATE_PERIOD_SECONDS.
        #[arg(long)]
        period: Option<u64>,
    },
}

/// Live engine when `--base-url` is given, otherwise the offline heuristic.
fn make_backend(url: Option<&str>) -> (Box<dyn Decide>, String) {
    match url {
        Some(u) => (Box::new(LayaBackend::new(u)), format!("laya-tch @ {u}")),
        None => (Box::new(HeuristicBackend), "offline heuristic".to_string()),
    }
}

/// The message for a `--state` that is not valid JSON, or `None` when it is.
/// `--state` takes *inline* JSON (`--state '{"text":"..."}'`), not a filename --
/// the most common way to get it "wrong" is to pass a path, which then surfaces
/// as a bare serde position (`expected value at line 1 column 1`) naming neither
/// the flag nor the fix. The same guard covers `resume --state`.
fn state_arg_problem(state: &str) -> Option<String> {
    let err = match serde_json::from_str::<Value>(state) {
        Ok(_) => return None,
        Err(e) => e,
    };
    let t = state.trim();
    let path_shaped = !t.starts_with(['{', '[', '"'])
        && (t.ends_with(".json")
            || t.ends_with(".jsonl")
            || t.contains('/')
            || std::path::Path::new(t).exists());
    if path_shaped {
        return Some(format!(
            "`--state` takes inline JSON, not a path (`{t}`); read the file in with \
             --state \"$(cat {t})\""
        ));
    }
    Some(format!("`--state` is not valid JSON: {err}"))
}

/// Build run state, supporting the human-friendly `--query TEXT` shorthand.
/// Any JSON remains valid for `--state`; when `--query` is present, that value
/// is inserted into the object's `query` field.
fn run_state(state: &str, query: Option<&str>) -> Result<Value> {
    if let Some(msg) = state_arg_problem(state) {
        bail!("{msg}");
    }
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

/// Resolve a `--spec` argument to a `.json` workflow spec, answering the three
/// ways something else gets passed where a spec was meant: a directory, a BDD
/// `.feature`, or a path that is not there. Each branch names the next step, so
/// the failure reads as a fix and not as an errno. Used by every `--spec` entry
/// point (`run`, `validate`, `resume`, `replay`, `evaluate`).
fn resolve_spec_arg(spec: &str) -> Result<String> {
    let p = std::path::Path::new(spec);
    if spec.ends_with(".feature") {
        if p.exists() {
            bail!(
                "`{spec}` is a BDD `.feature`, not a workflow spec; compile it to JSON first \
                 with `laya-workflow bdd transpile {spec} --out <spec.json>`, or run the \
                 feature itself with `laya-workflow bdd run {spec}`"
            );
        }
        bail!(
            "`{spec}` is a `.feature` and does not exist; `--spec` wants a `.json` workflow \
             spec — features are run by `laya-workflow bdd run` and compiled by \
             `laya-workflow bdd transpile`"
        );
    }
    if p.is_dir() {
        bail!(
            "`{spec}` is a directory, not a spec; pass the spec `.json` itself \
             (list the specs on the layered roots with `laya-workflow list`)"
        );
    }
    if !p.exists() {
        bail!(
            "spec `{spec}` not found; check the path, or list the specs on the layered roots \
             with `laya-workflow list`"
        );
    }
    Ok(spec.to_string())
}

fn main() -> Result<()> {
    // Before anything can print: a reader that leaves early must end this process
    // with SIGPIPE, not with a stdout-write panic (see `laya_workflow::sigpipe`).
    laya_workflow::sigpipe::restore_sigpipe();
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
        Cmd::Evaluate {
            spec,
            dataset,
            split,
            departments,
            output,
            freeze,
        } => run_evaluate(cli.base_url.as_deref(), spec, dataset, split, departments, output.as_deref(), *freeze),
        Cmd::Improve {
            dir,
            rounds,
            score_only,
        } => run_improve(dir, *rounds, *score_only),
        Cmd::Export { app } => {
            let wf = app_workflow(app)?;
            let spec = laya_workflow::spec::to_spec(&wf, app);
            // Every built-in app routes in a Rust closure, so the exported
            // graph carries `"kind": "rust"` nodes that `from_spec` rejects.
            // Say so on stderr (stdout stays parseable JSON) instead of
            // emitting a spec that silently fails to reload.
            let native = laya_workflow::spec::native_action_nodes(&spec);
            if !native.is_empty() {
                eprintln!(
                    "note: {app} routes in a native Rust action (nodes: {}); `kind: \"rust\"` is \
                     not portable, so this reference spec is rejected by `validate` / `run`. \
                     To own the decision logic, author an equivalent @decision workflow \
                     (`laya-workflow bdd scaffold --name <wf> --cat <cat> --intent \"...\"`).",
                    native.join(", ")
                );
            }
            println!("{}", serde_json::to_string_pretty(&spec)?);
            Ok(())
        }
        Cmd::Validate { spec } => {
            let spec = resolve_spec_arg(spec)?;
            let raw = std::fs::read_to_string(&spec)?;
            let sv: Value = serde_json::from_str(&raw).map_err(|e| {
                anyhow!(
                    "`{spec}` is not a workflow spec: it does not parse as JSON ({e}); \
                     `validate` reads a `.json` spec, not a `.feature` or a directory"
                )
            })?;
            match laya_workflow::spec::check_version(&sv)? {
                laya_workflow::spec::VersionCheck::Ok(v) => {
                    println!("# dsl_version: {v} (current)")
                }
                laya_workflow::spec::VersionCheck::Upgradable { from, to } => {
                    println!("# dsl_version: {from} (legacy; engine supports {to})")
                }
            }
            let wf = laya_workflow::spec::load_file(&spec)?;
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
            // Static regex lint for `heuristic.match_regex` so a typo in a spec
            // is caught at validate-time, not on the first matching input.
            let mut bad_regex = Vec::new();
            if let Some(Value::Array(nodes)) = sv.get("nodes") {
                for n in nodes {
                    let nname = n.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    if let Some(qs) = n.get("questions").and_then(|v| v.as_object()) {
                        for (qid, qdef) in qs {
                            if let Some(rxs) = qdef.get("heuristic")
                                .and_then(|h| h.get("match_regex"))
                                .and_then(|a| a.as_array())
                            {
                                for rx in rxs.iter().filter_map(|v| v.as_str()) {
                                    if let Err(e) = regex_lite::Regex::new(rx) {
                                        bad_regex.push(format!(
                                            "node={nname} question={qid} pattern={rx:?} error={e}"
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !bad_regex.is_empty() {
                println!("# WARNING: heuristic.match_regex has invalid patterns:");
                for b in &bad_regex { println!("#   {b}"); }
            }
            println!("{}", serde_json::to_string_pretty(&wf.describe())?);
            Ok(())
        }
        Cmd::Run {
            spec,
            state,
            query,
            progress,
        } => {
            let spec = resolve_spec_arg(spec)?;
            let wf = laya_workflow::spec::load_file(&spec)?;
            let st = run_state(state, query.as_deref())?;
            println!("backend: {label}");
            let out = if *progress {
                let run_t0 = std::time::Instant::now();
                wf.run_with_progress(backend.as_ref(), &st, &mut |r| {
                    let next = r.next_node.as_deref().unwrap_or("STOP");
                    eprintln!(
                        "[laya-progress] {:>14}  {:8}  {:5.1}s  → {}",
                        r.node_name,
                        r.action.as_str(),
                        run_t0.elapsed().as_secs_f64(),
                        next
                    );
                })?
            } else {
                wf.run(backend.as_ref(), &st)?
            };
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
            let spec = resolve_spec_arg(spec)?;
            let wf = laya_workflow::spec::load_file(&spec)?;
            let mut store = laya_workflow::persist::NodeStore::open(dir)?;
            let initial: Option<serde_json::Value> = if state == "{}" {
                None
            } else {
                if let Some(msg) = state_arg_problem(state) {
                    bail!("{msg}");
                }
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
            let spec = resolve_spec_arg(spec)?;
            let wf = laya_workflow::spec::load_file(&spec)?;
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
        Cmd::Install { force, dirs_only } => run_install(*force, *dirs_only),
        Cmd::Update {
            check,
            tag,
            force,
            repo,
            target,
            api_base,
        } => laya_workflow::update::run(&laya_workflow::update::UpdateOptions {
            repo: repo.clone(),
            tag: tag.clone(),
            check_only: *check,
            force: *force,
            target: target.clone(),
            api_base: api_base.clone(),
        })
        .map(|_| ()),
        Cmd::Bdd { cmd } => run_bdd(cmd),
        Cmd::Mock { cmd } => run_mock(cmd),
        Cmd::Dsl { cmd } => run_dsl(cmd),
        Cmd::Bench { cmd } => run_bench(cmd),
        Cmd::BrowserDemo {
            site,
            port,
            spec,
            text,
        } => {
            // The demo page and the singleton spec are both repo files, so with
            // no checkout this used to fail with the build machine's path in the
            // message. rc=2 is the "could not run" code the other corpus readers
            // answer with; an explicit --site / --spec is the caller's input and
            // needs no checkout.
            if site.is_none() || spec.is_none() {
                if let Err(e) = laya_workflow::bdd::require_checkout("browser-demo") {
                    eprintln!("{e}");
                    std::process::exit(2);
                }
            }
            let _ = laya_workflow::browser_demo::run(
                &laya_workflow::browser_demo::BrowserDemoOptions {
                    site: site.clone(),
                    port: *port,
                    spec: spec.clone(),
                    text: text.clone(),
                },
            )?;
            Ok(())
        }
        Cmd::FeishuCollect {
            chats_limit,
            page_size,
            print_knobs,
        } => {
            let mut a = Vec::new();
            if *print_knobs {
                a.push("--print-knobs".to_string());
                a.push(chats_limit.clone());
                a.push(page_size.clone());
            } else {
                a.push(chats_limit.clone());
                a.push(page_size.clone());
            }
            laya_workflow::feishu_collect::run(&a)?;
            Ok(())
        }
        Cmd::Rules { cmd } => run_rules(cmd),
        Cmd::LayaMem { cmd } => run_laya_mem(cmd),
        Cmd::Browser { cmd } => run_browser(cmd),
        Cmd::Chrome { cmd } => run_chrome_alias(cmd),
        Cmd::Server { cmd } => run_server(cmd),
        Cmd::Mcp { cmd } => run_mcp(cmd),
        Cmd::Db { cmd } => run_db(cmd),
        Cmd::Notify {
            message,
            title,
            subtitle,
            channel,
            sound,
            path,
            bell,
        } => run_notify(
            message,
            title,
            subtitle,
            channel,
            sound,
            path.as_deref(),
            *bell,
        ),
        Cmd::List => {
            let roots = laya_workflow::spec::spec_roots_low_to_high();
            println!("DSL search path (low → high; later overrides earlier):");
            for r in &roots {
                // `spec_root_display` keeps a *missing* builtin root from printing
                // the build machine's path (see its doc); a dev build is unchanged.
                println!(
                    "  {:<8} {}",
                    r.layer.as_str(),
                    laya_workflow::spec::spec_root_display(r.layer, &r.path)
                );
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

/// `browser ensure --backend <b>`: idempotent; exits 1 (message already
/// printed) when the backend could not be brought up.
fn run_browser(cmd: &BrowserCmd) -> Result<()> {
    use laya_workflow::orchestrate as orch;
    match cmd {
        BrowserCmd::Ensure {
            backend,
            port,
            chrome_bin,
            profile,
            headless,
        } => {
            let req = orch::BrowserEnsureRequest::from_env(
                *backend,
                *port,
                chrome_bin.clone(),
                profile.clone(),
                *headless,
            );
            if !orch::ensure_browser(&req)? {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

/// Hidden alias: `chrome ensure` == `browser ensure --backend chrome`.
fn run_chrome_alias(cmd: &ChromeAliasCmd) -> Result<()> {
    use laya_workflow::orchestrate as orch;
    match cmd {
        ChromeAliasCmd::Ensure {
            port,
            chrome_bin,
            profile,
            headless,
        } => {
            let req = orch::BrowserEnsureRequest::from_env(
                orch::BrowserBackend::Chrome,
                *port,
                chrome_bin.clone(),
                profile.clone(),
                *headless,
            );
            if !orch::ensure_browser(&req)? {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

/// `server ensure | start | stop | status`: local HTTP server lifecycle.
fn run_server(cmd: &ServerCmd) -> Result<()> {
    use laya_workflow::orchestrate as orch;
    match cmd {
        ServerCmd::Status { port, health_path } => {
            if !orch::server_status(*port, health_path) {
                std::process::exit(1);
            }
            Ok(())
        }
        ServerCmd::Stop { port } => {
            orch::server_stop(*port)?;
            Ok(())
        }
        ServerCmd::Ensure {
            port,
            command,
            health_path,
        } => {
            if !orch::server_ensure(*port, command.as_deref(), health_path)? {
                std::process::exit(1);
            }
            Ok(())
        }
        ServerCmd::Start {
            port,
            command,
            health_path,
            daemon,
        } => {
            let ok = if *daemon {
                orch::server_start_daemon(*port, command, health_path)?
            } else {
                orch::server_start_foreground(*port, command, health_path)?
            };
            if !ok {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

/// `db serve | ensure | stop | status`: the *server* mode of the HTAP `db`
/// capability. `serve` is a dependency-free HTTP daemon (see `laya_workflow::db`);
/// the other three reuse the generic local-server lifecycle so pid/base/log live
/// in the same `/tmp/laya-ensure-server-<port>.*` files as `server ensure`.
fn run_db(cmd: &DbCmd) -> Result<()> {
    use laya_workflow::db::{self, DbServerConfig, DEFAULT_DB_HEALTH_PATH};
    use laya_workflow::orchestrate as orch;

    match cmd {
        DbCmd::Serve {
            sqlite,
            duckdb,
            alias,
            host,
            port,
            daemon,
        } => {
            let cfg = DbServerConfig {
                sqlite: sqlite.clone(),
                duckdb: duckdb.clone().unwrap_or_default(),
                alias: alias.clone(),
                host: host.clone(),
                port: port.unwrap_or_else(db_port),
            };
            if *daemon {
                let command = db_serve_command(&cfg);
                if !orch::server_start_daemon(cfg.port, &command, DEFAULT_DB_HEALTH_PATH)? {
                    std::process::exit(1);
                }
                Ok(())
            } else {
                db::serve(&cfg)
            }
        }
        DbCmd::Ensure {
            sqlite,
            duckdb,
            alias,
            host,
            port,
        } => {
            let cfg = DbServerConfig {
                sqlite: sqlite.clone(),
                duckdb: duckdb.clone().unwrap_or_default(),
                alias: alias.clone(),
                host: host.clone(),
                port: port.unwrap_or_else(db_port),
            };
            // Refuse to attach to a *different* database that already owns the
            // port — "ensure" must mean "this db is up", not "something is up".
            if let Some(h) = db::health(&cfg.base()) {
                let served = h.get("sqlite").and_then(|v| v.as_str()).unwrap_or("");
                if !served.is_empty() && served != cfg.sqlite {
                    bail!(
                        "port {} already serves sqlite={served:?}, not {sqlite:?}; \
                         stop it or pick another --port",
                        cfg.port
                    );
                }
            }
            let command = db_serve_command(&cfg);
            if !orch::server_ensure(cfg.port, Some(&command), DEFAULT_DB_HEALTH_PATH)? {
                std::process::exit(1);
            }
            Ok(())
        }
        DbCmd::Stop { port } => {
            orch::server_stop(port.unwrap_or_else(db_port))?;
            Ok(())
        }
        DbCmd::Status { port } => {
            if !orch::server_status(port.unwrap_or_else(db_port), DEFAULT_DB_HEALTH_PATH) {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

/// The default HTAP daemon port: `$LAYA_DB_PORT`, else 18767.
fn db_port() -> u16 {
    std::env::var("LAYA_DB_PORT")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(laya_workflow::db::DEFAULT_DB_PORT)
}

/// The shell command `db ensure` / `db serve --daemon` uses to (re)spawn itself
/// as a foreground daemon. Single-quoted so paths with spaces survive `sh -c`.
fn db_serve_command(cfg: &laya_workflow::db::DbServerConfig) -> String {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "laya-workflow".to_string());
    let mut parts = vec![
        shell_quote(&exe),
        "db".to_string(),
        "serve".to_string(),
        "--sqlite".to_string(),
        shell_quote(&cfg.sqlite),
        "--alias".to_string(),
        shell_quote(&cfg.alias),
        "--host".to_string(),
        shell_quote(&cfg.host),
        "--port".to_string(),
        cfg.port.to_string(),
    ];
    if !cfg.duckdb.is_empty() {
        parts.push("--duckdb".to_string());
        parts.push(shell_quote(&cfg.duckdb));
    }
    parts.join(" ")
}

/// POSIX single-quote a string for `sh -c` (embedded `'` → `'\''`).
fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// `notify` — deliver a local notification with no spec in the way. On macOS this
/// is a real Notification Center banner (osascript); it can also log/bell. The
/// CLI is the user's own action, so it enables `allow_exec` for the banner and
/// allow-lists only the log file's directory.
fn run_notify(
    message: &str,
    title: &str,
    subtitle: &str,
    channel: &str,
    sound: &str,
    path: Option<&str>,
    bell: bool,
) -> Result<()> {
    use laya_workflow::capability::sys::{call_notify_local, NotifyLocalCap};
    use laya_workflow::capability::Policy;

    let mut allow_paths = Vec::new();
    if let Some(p) = path {
        if let Some(dir) = std::path::Path::new(p).parent() {
            allow_paths.push(dir.to_string_lossy().into_owned());
        }
    }
    let cap = NotifyLocalCap {
        path: path.unwrap_or_default().to_string(),
        bell,
        timestamp: true,
        channel: channel.to_string(),
        title: title.to_string(),
        subtitle: subtitle.to_string(),
        sound: sound.to_string(),
        timeout_ms: 10_000,
    };
    let policy = Policy {
        allow_exec: true,
        allow_paths,
        ..Policy::default()
    };
    let out = call_notify_local(&cap, &json!({ "message": message }), &json!({}), &policy)?;
    let chan = out["channel"].as_str().unwrap_or("");
    let delivered = out["macos"]["delivered"].as_bool().unwrap_or(false);
    let log = out["path"]
        .as_str()
        .filter(|p| !p.is_empty())
        .map(|p| format!(" path={p}"))
        .unwrap_or_default();
    println!("notify: channel={chan} delivered={delivered}{log}");
    Ok(())
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
                "{:<22} {:<8} {:<8} {}",
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
                    "{:<22} {:<8} {:<8} {}{}",
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
        PluginCmd::RegistryCheck => {
            // The registries and the `plugins/` tree are repo-relative, so with no
            // checkout this answered `no plugins/ ... - skipped`, rc=0 - a green
            // over an empty tree. rc=2 is the "could not run" code the bdd gates
            // use.
            if let Err(e) = laya_workflow::bdd::require_checkout("plugin registry-check") {
                eprintln!("{e}");
                std::process::exit(2);
            }
            let problems = plg::registry_check()?;
            if problems > 0 {
                std::process::exit(1);
            }
            Ok(())
        }
        PluginCmd::Compile { file } => {
            match plg::compile_check(file) {
                Ok(None) => {
                    eprintln!("ok: {} compiles", file.display());
                    Ok(())
                }
                Ok(Some(err)) => {
                    eprintln!("error: {}: {err}", file.display());
                    std::process::exit(1)
                }
                Err(e) => Err(e),
            }
        }
        PluginCmd::Check => {
            // Same reason as registry-check: everything the gate compiles and
            // routes lives in the checkout, so a missing one must not read as a
            // pass.
            if let Err(e) = laya_workflow::bdd::require_checkout("plugin check") {
                eprintln!("{e}");
                std::process::exit(2);
            }
            let problems = plg::gate()?;
            if problems > 0 {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

/// `install` — lay out the state root and install everything the binary bundles.
///
/// This is the one command that makes a fresh machine ready. It exists because
/// the previous arrangement had three roots (`~/.config/laya-workflow` for
/// plugins, `~/tmp/laya_mem` for the memory store, `~/.laya-workflow/chrome`
/// for the browser), so a working setup was the result of three unrelated
/// manual steps, and a broken one was three separate things to diagnose.
fn run_install(force: bool, dirs_only: bool) -> Result<()> {
    use laya_workflow::capability::plugin as plg;
    use laya_workflow::state as st;

    let root = st::state_dir()
        .ok_or_else(|| anyhow!("cannot pick a state root: set LAYA_HOME or HOME"))?;
    st::ensure_dir(&root)?;
    println!("state root: {}", root.display());

    let dsl = st::user_dsl_dir().unwrap_or_else(|| root.join("dsl"));
    let plugins = st::user_plugin_dir().unwrap_or_else(|| root.join("plugins"));
    let websites = st::user_websites_dir().unwrap_or_else(|| root.join("websites"));
    let mem_dir = root.join("laya-mem");
    let chrome = st::browser_profile_dir().unwrap_or_else(|| root.join("chrome"));
    for d in [&dsl, &plugins, &websites, &mem_dir, &chrome] {
        st::ensure_dir(d)?;
    }
    println!("  dsl/          {}", dsl.display());
    println!("  plugins/      {}", plugins.display());
    println!("  websites/     {}", websites.display());
    println!("  laya-mem/     {}", mem_dir.display());
    println!("  chrome/       {}", chrome.display());
    if dirs_only {
        println!("\n(dirs only — nothing installed; drop --dirs-only to install plugins + specs)");
        return Ok(());
    }

    // laya-mem's specs. Materialized from the binary, so this is what makes an
    // installed `mcp serve` able to answer calls at all.
    let spec_dir = laya_workflow::laya_mem::LayaMemTools::default_spec_dir();
    if force {
        // `--force` means "make this match the shipped version", which for the
        // specs means overwriting local edits. Without it, ensure_specs leaves
        // them alone — that distinction is the only reason to have both paths.
        let n = laya_workflow::laya_mem::restore_specs(&spec_dir)?;
        println!("\nlaya-mem specs: {} ({} file(s), restored)", spec_dir.display(), n);
    } else {
        laya_workflow::laya_mem::ensure_specs(&spec_dir)?;
        println!("\nlaya-mem specs: {} ({} file(s))", spec_dir.display(), laya_workflow::laya_mem::EMBEDDED_SPECS.len());
    }
    println!("laya-mem store: {}", laya_workflow::laya_mem::LayaMemTools::default_db_path().display());

    // Every plugin, installed into the user layer so a checkout is not required.
    //
    // Two sources, and both are needed: `discover_plugins` only sees what is on
    // disk, so run outside a checkout it finds nothing, while the builtins are
    // compiled in and so are invisible to it. Installing only the first source
    // made `install` a no-op on a machine with just the binary — the case it
    // most needs to work. On-disk copies win where both exist, because a repo
    // checkout can carry plugins newer than the binary that reads them.
    let entries = plg::discover_plugins();
    let mut installed = 0usize;
    let mut kept = 0usize;
    let mut failed: Vec<String> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = Default::default();

    for e in &entries {
        let Some(src) = e.path.as_ref() else {
            continue; // no on-disk copy
        };
        // Never "install" a plugin from the install root onto itself. Discovery
        // walks the user layer too, so run outside a checkout every discovered
        // source *is* the destination; `install_from_dir` would `remove_dir_all`
        // the destination and then copy from the directory it just deleted,
        // leaving the plugin empty — and with `--force`, wiping the whole tree.
        // Such an entry is not a source: leave it out of `seen` so the bundled-
        // builtin loop below owns the name and can refresh it from the binary.
        if src.starts_with(plugins.as_path()) {
            continue;
        }
        // Discovery reports `group/name`; a plugin is one directory, so install
        // under the bare name (`websites/hackernews` → `plugins/hackernews`).
        let name = plg::install_name_for(&e.name).to_string();
        if !seen.insert(name.clone()) {
            continue;
        }
        if plugins.join(&name).exists() && !force {
            kept += 1;
            continue;
        }
        match plg::install_from_dir(src, &plugins, &name, force) {
            Ok(_) => installed += 1,
            Err(err) => failed.push(format!("{name}: {err}")),
        }
    }
    let from_repo = seen.len();
    for name in plg::builtin_names() {
        if seen.contains(*name) {
            continue;
        }
        match plg::install_builtin_to(name, &plugins, force) {
            Ok(Some(_)) => installed += 1,
            Ok(None) => kept += 1, // already present, or not a builtin
            Err(err) => failed.push(format!("{name}: {err}")),
        }
    }
    let total = seen.len() + plg::builtin_names()
        .iter()
        .filter(|n| !seen.contains(**n))
        .count();
    println!("\nplugins: {installed} installed, {kept} already present, {total} bundled ({from_repo} on disk, {} compiled in)", plg::builtin_names().len());
    if !failed.is_empty() {
        for f in &failed {
            eprintln!("  FAILED {f}");
        }
        bail!("{} plugin(s) failed to install", failed.len());
    }
    println!("\nnext: laya-workflow plugin list   |   laya-workflow laya-mem info");
    Ok(())
}

/// `laya-mem info | install | restore` — the memory gate's own state.
///
/// `info` deliberately reports per-spec presence: the failure mode this guards
/// against is a server that starts fine and then answers every tool call with
/// `spec not found`, which reads like a broken install rather than a missing
/// directory.
fn run_laya_mem(cmd: &LayaMemCmd) -> Result<()> {
    use laya_workflow::laya_mem::{self, LayaMemTools};
    let tools = LayaMemTools::from_env();
    match cmd {
        LayaMemCmd::Info => {
            let backend = match &tools.base_url {
                Some(u) => format!("Laya model @ {u}"),
                None => "offline heuristic (set LAYA_BASE_URL for the real model)".to_string(),
            };
            println!("spec dir: {}", tools.spec_dir.display());
            println!("store:    {}", tools.db_path.display());
            println!("backend:  {backend}");
            let missing: Vec<&str> = laya_mem::EMBEDDED_SPECS
                .iter()
                .filter(|(name, _)| !tools.spec_dir.join(name).is_file())
                .map(|(name, _)| *name)
                .collect();
            if missing.is_empty() {
                println!("specs:    all {} present", laya_mem::EMBEDDED_SPECS.len());
            } else {
                println!("specs:    MISSING {}", missing.join(", "));
                println!("          fix: laya-workflow laya-mem install");
                bail!("{} spec(s) missing from {}", missing.len(), tools.spec_dir.display());
            }
            if let Some(p) = tools.db_path.parent() {
                println!("note:     the store's directory is created on first persist ({})", p.display());
            }
            Ok(())
        }
        LayaMemCmd::Install => {
            laya_mem::ensure_specs(&tools.spec_dir)?;
            println!("specs ready: {} ({} file(s))", tools.spec_dir.display(), laya_mem::EMBEDDED_SPECS.len());
            Ok(())
        }
        LayaMemCmd::Restore => {
            let n = laya_mem::restore_specs(&tools.spec_dir)?;
            println!("restored {n} spec(s) into {}", tools.spec_dir.display());
            Ok(())
        }
        LayaMemCmd::ConsolidateWatch { once, period } => {
            use laya_workflow::laya_mem_periodic as periodic;
            if let Some(p) = period {
                std::env::set_var("LAYA_MEM_CONSOLIDATE_PERIOD_SECONDS", p.to_string());
            }
            let period_secs = std::env::var("LAYA_MEM_CONSOLIDATE_PERIOD_SECONDS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(60);
            if *once {
                let out = periodic::sweep_once(
                    &tools.db_path,
                    &tools.spec_dir,
                    tools.base_url.as_deref(),
                    periodic::min_age_secs(),
                    periodic::max_per_sweep(),
                    periodic::threshold(),
                )?;
                println!("{}", serde_json::to_string_pretty(&out)?);
                return Ok(());
            }
            eprintln!(
                "[laya-mem periodic] watching {} every {}s (ctrl-c to stop)",
                tools.db_path.display(),
                period_secs
            );
            loop {
                std::thread::sleep(std::time::Duration::from_secs(period_secs));
                match periodic::sweep_once(
                    &tools.db_path,
                    &tools.spec_dir,
                    tools.base_url.as_deref(),
                    periodic::min_age_secs(),
                    periodic::max_per_sweep(),
                    periodic::threshold(),
                ) {
                    Ok(out) => {
                        let due = out.get("due_count").and_then(|v| v.as_i64()).unwrap_or(0);
                        let acted = out.get("actions").and_then(|v| v.as_array())
                            .map(|a| a.len()).unwrap_or(0);
                        eprintln!(
                            "[laya-mem periodic] sweep: due={due} actions={acted} store={}",
                            tools.db_path.display()
                        );
                    }
                    Err(e) => eprintln!("[laya-mem periodic] sweep error: {e:#}"),
                }
            }
        }
        LayaMemCmd::Migrate => {
            let before = laya_workflow::laya_mem_migrate::schema_version(&tools.db_path);
            println!("migrating: {}", tools.db_path.display());
            println!("  from schema_version={before} → {}",
                laya_workflow::laya_mem_migrate::CURRENT_SCHEMA_VERSION);
            let (total, backfilled, skipped) =
                laya_workflow::laya_mem_migrate::backfill_vectors(&tools.db_path)?;
            let after = laya_workflow::laya_mem_migrate::schema_version(&tools.db_path);
            println!("  total_memories={total}");
            println!("  vectors_backfilled={backfilled}");
            if skipped > 0 {
                println!("  already_indexed={skipped}");
            }
            println!("  schema_version now {after}");
            println!("done");
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
        ("evaluate", "Evaluate a workflow against a labeled JSONL dataset (dev/holdout + slice + confusion + review queue).", "evaluate"),
        ("computer-use", "Bounded UI decision cycle (awesome-jev): fingerprint before inference, stale-guarded single fill, independent oracle.", "computer-use"),
        ("export",    "Export a built-in app workflow as a generic JSON spec (a reference graph — apps with native Rust actions warn on stderr and are not portable to validate/run).", "export"),
        ("state",     "List every per-node record in a NodeStore (track what ran, what it returned, the full state snapshot).", "state"),
        ("resume",   "Continue a partially-completed run from `dir/`, optionally from `--from <iter>` with the materialised state.", "resume"),
        ("replay",   "Re-execute a single iteration in place from its `state_before` (atomic overwrite of one record).", "replay"),
        ("persist",   "Where the per-node store lives on disk (`<dir>/runs/0001.json` + `manifest.json`), how `run_persistent` / `rewind` / `replay` fit together.", "state"),
        ("dsl",       "Workflow JSON shape (`name`, `start`, `nodes[*]`, `actions`, `capabilities`), versioning (`dsl_version`), folder layout, and the kind catalogue.", "list"),
        ("plugins",   "Extension seam: write site logic as a sandboxed Rhai plugin, install one from a git repo (`plugin install`), and call it with `kind: \"plugin\"`.", "dsl"),
        ("install",   "The state root (~/.laya-workflow), `install` (layout + every bundled plugin + the laya-mem specs), and the laya-mem memory gate (`mcp serve`, `laya-mem info`).", "overview"),
        ("rules",     "Rule enforcement: compile AGENTS.md rules into `.rules/rubric.json`, judge diffs against it, audit events.", "rules"),
        ("update",    "Self-update from GitHub Releases: resolve the host platform, download the right tarball, atomically replace the running binary with a `.bak` fallback.", "install"),
        ("tests",     "The offline test runner `laya-workflow-tests` is modular: each `[section]` is selectable via `./target/release/laya-workflow-tests <section>`.", "tests"),
        ("orchestrate", "Bring up the local resources a browser workflow needs first: `browser ensure --backend <b>` (a browser backend, idempotent) and `server ensure|start|stop|status` (a local HTTP server).", "plugins"),
        ("db",        "HTAP store in two modes: `kind: \"db\"` pairs SQLite (ACID) with DuckDB (analytics) over one file — `embed` (local CLIs) or `server` (`db serve` daemon, shared writer).", "orchestrate"),
        ("notify",    "Desktop notifications: `kind: \"notify\"` or `laya-workflow notify` posts a macOS Notification Center banner (via `osascript`, so `policy.allow_exec`) with a `log`-file fallback (`auto` picks per host).", "orchestrate"),
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
    println!("  evaluate       score decisions on a labeled dataset (dev/holdout + slices + review queue)");
    println!("  computer-use   bounded UI decision cycle (fingerprint + stale guard + oracle)");
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
    println!("  rules i|v|c|r|a  enforce AGENTS.md rules: init / validate / check / report / audit");
    println!("  browser ensure  bring up a browser backend on 127.0.0.1:<port> (--backend chrome, idempotent)");
    println!("  server e|s|p|st  ensure/start/stop/status a local HTTP server");
    println!("  db s|e|st|p    serve/ensure/status/stop the HTAP SQLite+DuckDB daemon");
    println!("  notify         post a macOS Notification Center banner (osascript) with a log-file fallback");
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
    println!("  LAYA_USER_DSL_DIR  per-user spec root (default: ~/.laya-workflow/dsl)");
    println!("  LAYA_PLUGIN_DIR  pin the plugin root (also the `plugin install` target)");
    println!(
        "  LAYA_USER_PLUGIN_DIR  per-user plugin root (default: ~/.laya-workflow/plugins)"
    );
    println!("  LAYA_HOME      the state root (default: ~/.laya-workflow)");
    println!("  LAYA_MEM_SQLITE   laya-mem store (default: <state>/laya-mem/codex.sqlite)");
    println!("  LAYA_MEM_SPEC_DIR  laya-mem specs (default: <state>/laya-mem/specs)");
    println!("  LAYA_BASE_URL      laya-mem backend (unset = offline heuristic)");
    println!("  LAYA_MOCK3       host:redis:nats:mqtt:smtp:s3:prom:kafka:udp");
    println!("  LAYA_AGENT_BIN_DIR  dir containing `cxgo` / `cmdgo` wrappers");
    println!("  LAYA_GOAL_DIR    allow-list root for goal_runner goal docs");
    println!();
    println!("Read on: `skill --section <name>` for the one you need.");
}

fn print_section(name: &str) -> Result<()> {
    let body = match name {
        "rules" => SKILL_RULES,
        "overview" => SKILL_OVERVIEW,
        "validate" => SKILL_VALIDATE,
        "run" => SKILL_RUN,
        "list" => SKILL_LIST,
        "apps" => SKILL_APPS,
        "describe" => SKILL_DESCRIBE,
        "demo" => SKILL_DEMO,
        "optimize" => SKILL_OPTIMIZE,
        "improve" => SKILL_IMPROVE,
        "evaluate" => SKILL_EVALUATE,
        "computer-use" => SKILL_COMPUTER_USE,
        "export" => SKILL_EXPORT,
        "state" => SKILL_STATE,
        "resume" => SKILL_RESUME,
        "replay" => SKILL_REPLAY,
        "persist" => SKILL_PERSIST,
        "dsl" => SKILL_DSL,
        "tests" => SKILL_TESTS,
        "safety" => SKILL_SAFETY,
        "plugins" => SKILL_PLUGINS,
        "install" => SKILL_INSTALL,
        "update" => SKILL_UPDATE,
        "orchestrate" => SKILL_ORCHESTRATE,
        "db" => SKILL_DB,
        "notify" => SKILL_NOTIFY,
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

static SKILL_RULES: &str = include_str!("skill/sections/rules.md");
static SKILL_OVERVIEW: &str = include_str!("skill/sections/overview.md");
static SKILL_VALIDATE: &str = include_str!("skill/sections/validate.md");
static SKILL_RUN: &str = include_str!("skill/sections/run.md");
static SKILL_LIST: &str = include_str!("skill/sections/list.md");
static SKILL_APPS: &str = include_str!("skill/sections/apps.md");
static SKILL_DESCRIBE: &str = include_str!("skill/sections/describe.md");
static SKILL_DEMO: &str = include_str!("skill/sections/demo.md");
static SKILL_OPTIMIZE: &str = include_str!("skill/sections/optimize.md");
static SKILL_IMPROVE: &str = include_str!("skill/sections/improve.md");
static SKILL_EVALUATE: &str = include_str!("skill/sections/evaluate.md");
static SKILL_COMPUTER_USE: &str = include_str!("skill/sections/computer_use.md");
static SKILL_EXPORT: &str = include_str!("skill/sections/export.md");
static SKILL_STATE: &str = include_str!("skill/sections/state.md");
static SKILL_RESUME: &str = include_str!("skill/sections/resume.md");
static SKILL_REPLAY: &str = include_str!("skill/sections/replay.md");
static SKILL_PERSIST: &str = include_str!("skill/sections/persist.md");
static SKILL_DSL: &str = include_str!("skill/sections/dsl.md");
static SKILL_TESTS: &str = include_str!("skill/sections/tests.md");
static SKILL_SAFETY: &str = include_str!("skill/sections/safety.md");
static SKILL_PLUGINS: &str = include_str!("skill/sections/plugins.md");
static SKILL_INSTALL: &str = include_str!("skill/sections/install.md");
static SKILL_UPDATE: &str = include_str!("skill/sections/update.md");
static SKILL_ORCHESTRATE: &str = include_str!("skill/sections/orchestrate.md");
static SKILL_DB: &str = include_str!("skill/sections/db.md");
static SKILL_NOTIFY: &str = include_str!("skill/sections/notify.md");

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

/// MCP subcommand dispatch. Every built-in tool group is registered here;
/// adding a new group is a two-line edit: `let mem = laya_workflow::laya_mem::LayaMemTools…`
/// and push another `Box::new(…)` onto the vector passed to `serve_stdio`.
fn run_mcp(cmd: &McpCmd) -> Result<()> {
    match cmd {
        McpCmd::Serve { spec_dir, db_path } => {
            let mut mem = laya_workflow::laya_mem::LayaMemTools::from_env();
            if let Some(d) = spec_dir.as_ref().filter(|d| !d.is_empty()) {
                mem.spec_dir = std::path::PathBuf::from(d);
            }
            if let Some(d) = db_path.as_ref().filter(|d| !d.is_empty()) {
                mem.db_path = std::path::PathBuf::from(d);
            }
            if !mem.spec_dir.is_dir() {
                bail!(
                    "spec dir does not exist: {} (set --spec-dir or LAYA_MEM_SPEC_DIR)",
                    mem.spec_dir.display()
                );
            }
            // Wall-clock periodic consolidation (Jev-Mem slow path): when
            // LAYA_MEM_CONSOLIDATE_PERIOD_SECONDS > 0, spawn a detached thread
            // that sweeps the store on a timer. Cloned before `mem` is moved
            // into the tool-set vector below.
            if std::env::var("LAYA_MEM_CONSOLIDATE_PERIOD_SECONDS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
                > 0
            {
                laya_workflow::laya_mem_periodic::spawn_background(
                    mem.db_path.clone(),
                    mem.spec_dir.clone(),
                    mem.base_url.clone(),
                );
            }
            let info = laya_workflow::mcp::ServerInfo {
                name: "laya-workflow",
                version: env!("CARGO_PKG_VERSION"),
            };
            let cg = laya_workflow::codegraph::CodegraphTools::from_env();
            let nd = laya_workflow::capability::needle::NeedleTools;
            let bddt = laya_workflow::bdd_mcp::BddTools;
            let sets: Vec<Box<dyn laya_workflow::mcp::McpToolSet>> =
                vec![Box::new(mem), Box::new(cg), Box::new(nd), Box::new(bddt)];
            laya_workflow::mcp::serve_stdio(info, sets)
        }
    }
}

fn run_mock(cmd: &MockCmd) -> Result<()> {
    match cmd {
        MockCmd::Serve {
            redis,
            nats,
            mqtt,
            smtp,
            udp,
            s3,
            prom,
            kafka,
            web,
            score,
            agent,
            rpc,
            graphql,
            chat,
            mcp,
            vector,
            webhook,
        } => laya_workflow::mock::run_serve(&laya_workflow::mock::MockOptions {
            redis: *redis,
            nats: *nats,
            mqtt: *mqtt,
            smtp: *smtp,
            udp: *udp,
            s3: *s3,
            prom: *prom,
            kafka: *kafka,
            web: *web,
            score: *score,
            agent: *agent,
            rpc: *rpc,
            graphql: *graphql,
            chat: *chat,
            mcp: *mcp,
            vector: *vector,
            webhook: *webhook,
        }),
        MockCmd::Stdio => laya_workflow::mock::run_stdio(),
    }
}

fn run_bench(cmd: &BenchCmd) -> Result<()> {
    match cmd {
        BenchCmd::BackendComparison {
            base_url,
            skip_heuristic,
            skip_live,
        } => {
            // Every case loads a laya_mem spec from the checkout, so a missing one
            // must not read as a clean benchmark (rc=2, like the bdd gates).
            if let Err(e) = laya_workflow::bdd::require_checkout("bench backend-comparison") {
                eprintln!("{e}");
                std::process::exit(2);
            }
            laya_workflow::bench::backend_comparison(
                &laya_workflow::bench::BackendComparisonOptions {
                    base_url: base_url.clone(),
                    skip_heuristic: *skip_heuristic,
                    skip_live: *skip_live,
                },
            )?
        }
        BenchCmd::JevVsGbnf { md_out } => {
            laya_workflow::bench_probe::jev_vs_gbnf(
                &laya_workflow::bench_probe::JevVsGbnfOptions { md_out: md_out.clone() },
            )?;
        }
        BenchCmd::BddToNeedle => {
            laya_workflow::bench_probe::bdd_to_needle(
                &laya_workflow::bench_probe::BddToNeedleOptions { quiet: false },
            )?;
        }
        BenchCmd::NeedleVsHeuristic => {
            laya_workflow::bench_probe::needle_vs_heuristic(
                &laya_workflow::bench_probe::NeedleVsHeuristicOptions { quiet: false },
            )?;
        }
        BenchCmd::GbnfStress {
            base_url,
            n_per_class,
            seed,
            out,
        } => {
            laya_workflow::bench::gbnf_stress(&laya_workflow::bench::GbnfStressOptions {
                base_url: base_url.clone(),
                n_per_class: *n_per_class,
                seed: *seed,
                out: out.clone(),
            })?;
        }
    }
    Ok(())
}

fn run_dsl(cmd: &DslCmd) -> Result<()> {
    match cmd {
        DslCmd::Smoke {
            dsl_dir,
            base_url,
            filter,
            quiet,
        } => {
            let code = laya_workflow::dsl_smoke::run(&laya_workflow::dsl_smoke::DslSmokeOptions {
                dsl_dir: dsl_dir.clone(),
                base_url: base_url.clone(),
                filter: filter.clone(),
                quiet: *quiet,
            })?;
            // `dsl smoke` needs this repo's tree (specs + sample tables); a binary
            // with no checkout answers rc=2 (the "could not run" code the bdd gates
            // use) rather than a red for the wrong reason.
            if code != 0 {
                std::process::exit(code);
            }
        }
    }
    Ok(())
}

fn run_bdd(cmd: &BddCmd) -> Result<()> {
    use laya_workflow::bdd;
    let code = match cmd {
        BddCmd::Build {
            features,
            out,
            profile,
            filter,
            all,
            coverage_min,
            no_validate,
            base_url,
            emit,
            assist,
        } => bdd::build(&bdd::BuildOptions {
            features: features.clone(),
            out: out.clone(),
            profile: profile.clone(),
            filter: filter.clone(),
            all: *all,
            coverage_min: *coverage_min,
            no_validate: *no_validate,
            base_url: base_url.clone(),
            emit: Some(emit.clone()),
            assist: *assist,
        })?,
        BddCmd::Transpile {
            features,
            out,
            list,
            check,
            chrome_bin,
        } => bdd::transpile_cli(
            features,
            out.as_deref(),
            *list,
            *check,
            chrome_bin.as_deref(),
        )?,
        BddCmd::Scaffold {
            name,
            cat,
            intent,
            stdout,
            force,
            primary_q,
            single,
            nested,
            pattern,
        } => bdd::scaffold(&bdd::ScaffoldOptions {
            name: name.clone(),
            cat: cat.clone(),
            intent: intent.clone(),
            stdout: *stdout,
            force: *force,
            primary_q: primary_q.clone().unwrap_or_default(),
            single: *single,
            nested: *nested,
            pattern: pattern.clone(),
        })?,
        BddCmd::Vocabulary { json } => {
            bdd::vocabulary_cli(&bdd::VocabularyOptions { json: *json })?
        }
        BddCmd::VocabularyCheck => bdd::check_vocabulary()?,
        BddCmd::DecisionVocabCheck => bdd::decision_vocab_check()?,
        BddCmd::ProbeCheck => bdd::check_probes()?,
        BddCmd::ArgsProbeCheck => bdd::check_args_probes()?,
        BddCmd::DocCheck => bdd::check_doc()?,
        BddCmd::Run {
            features,
            filter,
            tags,
            profile,
            base_url,
            port,
            jobs,
            timeout_secs,
            keep,
            out,
            live,
            report,
            require_browser,
            cdp,
        } => bdd::run(&bdd::RunOptions {
            features: features.clone(),
            filter: filter.clone(),
            tags: tags.clone(),
            profile: profile.clone(),
            base_url: base_url.clone(),
            port: *port,
            jobs: *jobs,
            timeout_secs: *timeout_secs,
            keep: *keep,
            out: out.clone(),
            live: *live,
            report: report.clone(),
            require_browser: *require_browser,
            cdp: cdp.clone(),
        })?,
        BddCmd::Score { check, run, json, tasks } => bdd::score(&bdd::ScoreOptions {
            check: *check,
            run: *run,
            json: json.clone(),
            tasks: tasks.clone(),
        })?,
        BddCmd::Live {
            features,
            filter,
            out,
            timeout_secs,
            json,
        } => bdd::live(&bdd::LiveOptions {
            features: features.clone(),
            filter: filter.clone(),
            out: out.clone(),
            timeout_secs: *timeout_secs,
            json: json.clone(),
        })?,
        BddCmd::LiveDecisions {
            base_url,
            filter,
            json,
        } => bdd::live_decisions(&bdd::LiveDecisionsOptions {
            base_url: base_url.clone(),
            filter: filter.clone(),
            json: json.clone(),
        })?,
        BddCmd::CompareSystems {
            backend,
            filter,
            json,
            no_preflight,
        } => bdd::compare_systems(&bdd::CompareSystemsOptions {
            backends: backend.clone(),
            filter: filter.clone(),
            json: json.clone(),
            no_preflight: *no_preflight,
        })?,
        BddCmd::RoundTrip { json } => {
            bdd::round_trip(&bdd::RoundTripOptions { json: json.clone() })?
        }
        BddCmd::AppParity { json } => {
            bdd::app_parity_check(&bdd::AppParityOptions { json: json.clone() })?
        }
        BddCmd::PortCheck {
            sources,
            cat,
            json,
            pin,
            write_pin,
        } => bdd::port_check(&bdd::PortCheckOptions {
            sources: std::path::PathBuf::from(sources),
            cat: cat.clone(),
            json: json.clone(),
            pin: pin.clone().map(std::path::PathBuf::from),
            write_pin: write_pin.clone().map(std::path::PathBuf::from),
        })?,
        BddCmd::ScopeCheck { sources, json } => bdd::scope_check(&bdd::ScopeCheckOptions {
            sources: sources.clone(),
            json: json.clone(),
        })?,
        BddCmd::Import {
            spec,
            dir,
            out,
            into,
            json,
            force,
        } => bdd::import(&bdd::ImportOptions {
            spec: spec.clone(),
            dir: dir.clone(),
            out: out.clone(),
            into: into.clone(),
            json: json.clone(),
            force: *force,
        })?,
        BddCmd::ImportCheck { json } => bdd::import_check(&bdd::ImportCheckOptions {
            json: json.clone(),
        })?,
        BddCmd::FeaturesCheck { json } => bdd::features_check(&bdd::FeaturesCheckOptions {
            json: json.clone(),
        })?,
        BddCmd::Compare { json, cases } => bdd::compare(&bdd::CompareOptions {
            json: json.clone(),
            only: cases.clone(),
        })?,
    };
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
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
            &text[..text.len().min(44)]);
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

/// The message for a `--dataset` that is not a readable JSONL corpus, or `None`.
/// `evaluate --dataset` takes a labeled `.jsonl` file (one JSON case per line) --
/// not a `.json` workflow spec and not a directory. A missing or wrong-shaped
/// path otherwise surfaces as a bare `No such file or directory (os error 2)` /
/// `Is a directory (os error 21)`, the same raw-errno shape `--spec` used to give.
fn dataset_arg_problem(path: &str) -> Option<String> {
    let p = std::path::Path::new(path);
    if p.is_dir() {
        return Some(format!(
            "{path} is a directory; pass the labeled `.jsonl` dataset file itself"
        ));
    }
    if !p.exists() {
        return Some(format!(
            "{path}: no such file; `--dataset` reads a labeled `.jsonl` corpus \
             (one JSON case per line)"
        ));
    }
    None
}

/// Evaluate a workflow against a labeled JSONL dataset (dev/holdout + slice
/// breakdowns + confusion matrix + review queue). Port of awesome-jev's
/// `evaluations/run.py`: only the ticket message reaches the workflow; the
/// split and expected labels stay out of the decision path.
fn run_evaluate(
    base_url: Option<&str>,
    spec_path: &str,
    dataset_path: &str,
    split: &str,
    departments_csv: &str,
    output: Option<&str>,
    freeze: bool,
) -> Result<()> {
    use laya_workflow::evaluate;

    let spec_path = resolve_spec_arg(spec_path)?;
    let spec_text = std::fs::read_to_string(&spec_path)?;
    let spec_json: Value = serde_json::from_str(&spec_text)?;
    let fingerprint = evaluate::config_sha256(&spec_json)?;
    let departments: Vec<&str> =
        departments_csv.split(',').map(str::trim).collect();

    // holdout freeze: refuse an evaluation against a *changed* spec
    if freeze && split == "holdout" {
        if let Some(out_path) = output {
            let fp_path = format!("{out_path}.fingerprint");
            if std::path::Path::new(&fp_path).exists() {
                let recorded = std::fs::read_to_string(&fp_path)?.trim().to_string();
                if recorded != fingerprint {
                    bail!(
                        "holdout evaluation refused: spec hash changed (recorded {recorded} vs now {fingerprint}); \
                         tune only on development cases, then author a fresh holdout"
                    );
                }
            }
        }
    }

    if let Some(msg) = dataset_arg_problem(dataset_path) {
        bail!("evaluate --dataset: {msg}");
    }
    let cases = evaluate::load_dataset(dataset_path, &departments)?;
    let selected: Vec<_> = cases
        .iter()
        .filter(|c| split == "all" || c.split == split)
        .cloned()
        .collect();
    if selected.is_empty() {
        bail!("no cases match split {split:?}");
    }

    let wf = laya_workflow::spec::load_file(&spec_path)?;
    let (backend, _label) = make_backend(base_url);
    let summary = evaluate::evaluate(&selected, &departments, |message| {
        let state = serde_json::json!({ "message": message });
        let out = wf.run(backend.as_ref(), &state)?;
        // redact before the summary sees it (same invariant as `run`)
        let redacted = out.to_json();
        let redacted = laya_workflow::capability::secret::redact(&redacted);
        Ok(redacted)
    })?;

    let text = serde_json::to_string_pretty(&summary)?;
    if let Some(out_path) = output {
        if let Some(parent) = std::path::Path::new(out_path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        // never overwrite: refuse if the file already exists
        if std::path::Path::new(out_path).exists() {
            bail!("output {out_path} already exists; use a new directory per run");
        }
        std::fs::write(out_path, format!("{text}\n"))?;
        // record the config fingerprint for the freeze rule
        let fp_path = format!("{out_path}.fingerprint");
        std::fs::write(&fp_path, format!("{fingerprint}\n"))?;
        println!("wrote {out_path}");
        println!("fingerprint {fingerprint} -> {fp_path}");
    } else {
        println!("{text}");
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

/// Walk up from cwd to find the repo root (matching abide's findRepoRoot).
fn rules_repo_root() -> std::path::PathBuf {
    laya_workflow::rules::find_repo_root(&std::env::current_dir().unwrap_or_default())
}

fn run_rules(cmd: &RulesCmd) -> Result<()> {
    use laya_workflow::rules as ab;
    match cmd {
        RulesCmd::Init => {
            let dir = ab::cmd_init(&rules_repo_root())?;
            println!("created {}", dir.display());
            println!("next: `laya-workflow rules compile` to build a rubric from AGENTS.md");
            Ok(())
        }
        RulesCmd::Validate { file } => {
            let path = file
                .as_deref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| ab::rubric_path(&rules_repo_root()));
            match ab::read_rubric(&path) {
                ab::RubricRead::Ok { rubric, .. } => {
                    println!(
                        "ok: {} rules across {} source{}",
                        rubric.rules.len(),
                        rubric.sources.len(),
                        if rubric.sources.len() == 1 { "" } else { "s" }
                    );
                    Ok(())
                }
                ab::RubricRead::Missing { path } => anyhow::bail!("no rubric at {}", path.display()),
                ab::RubricRead::Invalid { path, issues } => {
                    eprintln!("invalid rubric at {}:", path.display());
                    for issue in &issues {
                        eprintln!("  - {issue}");
                    }
                    std::process::exit(1);
                }
            }
        }
        RulesCmd::Check {
            file,
            phase,
            diff_file,
            file_path,
            diff_json,
            diff_json_file,
            task,
            base_url,
            session_id,
            prompt_id,
            root,
        } => {
            let repo = root
                .as_deref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(rules_repo_root);
            let rubric_file = file
                .as_deref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| ab::rubric_path(&repo));
            let phase = match phase.as_str() {
                "edit" => ab::RuleWhen::Edit,
                "turn" => ab::RuleWhen::Turn,
                other => anyhow::bail!("--phase must be `edit` or `turn`, got {other:?}"),
            };
            let diffs = match (diff_file, file_path, diff_json, diff_json_file) {
                (Some(path), Some(relative), None, None) => {
                    let text = std::fs::read_to_string(path)?;
                    vec![ab::FileDiff {
                        file: relative.clone(),
                        text,
                    }]
                }
                (None, None, Some(raw), None) => ab::parse_diff_json(&raw)?,
                (None, None, None, Some(path)) => {
                    let raw = if path == "-" {
                        let mut buf = String::new();
                        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
                        buf
                    } else {
                        std::fs::read_to_string(&path)?
                    };
                    ab::parse_diff_json(&raw)?
                }
                (None, None, None, None) => anyhow::bail!(
                    "no diff given: pass --diff-json <JSON> / --diff-json-file <PATH>, or --diff-file <PATH> --file-path <REL>"
                ),
                _ => anyhow::bail!(
                    "--diff-file requires --file-path; --diff-json / --diff-json-file take the whole diff and cannot be combined with them"
                ),
            };
            let input = ab::CheckCliInput {
                root: repo.clone(),
                rubric_file,
                phase,
                file_diffs: diffs,
                task: task.clone(),
                base_url: base_url.clone(),
                session_id: session_id.clone(),
                prompt_id: prompt_id.clone(),
            };
            let output = ab::cmd_check(&input)?;
            println!("{}", serde_json::to_string_pretty(&output)?);
            // Hook callers want exit 0 even on block: stdout carries the signal.
            Ok(())
        }
        RulesCmd::Report { root } => {
            let repo = root
                .as_deref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(rules_repo_root);
            let summary = ab::cmd_report(&repo)?;
            println!("{}", serde_json::to_string_pretty(&summary)?);
            Ok(())
        }
        RulesCmd::Audit { root } => {
            let repo = root
                .as_deref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(rules_repo_root);
            ab::cmd_audit(&repo)
        }
        RulesCmd::Compile { root, init } => {
            let repo = root
                .as_deref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(rules_repo_root);
            if *init {
                ab::cmd_init(&repo)?;
                eprintln!("scaffold: {}", ab::rubric_path(&repo).display());
            }
            print!("{}", ab::compile_prompt(&repo));
            Ok(())
        }
    }
}
