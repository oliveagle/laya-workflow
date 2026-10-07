//! `laya-workflow-tests` — offline unit tests for the Laya workflow engine and
//! apps, mirroring the coverage of `code/laya/tests/*.py` but with a scripted
//! backend so they run without a model or server.
//!
//! Run: `cargo run --release --bin laya-workflow-tests`
//!
//! The sections live in sibling modules (`engine`, `apps`, `spec`,
//! `capabilities*`, `secrets`, `timeouts`, `heuristics`); this file owns the
//! harness, the shared helpers and the section registry.

mod capabilities;
mod capabilities2;
mod capabilities3;
mod codegraph;
mod engine;
mod heuristics;
mod persist;
mod secrets;
mod t_apps;
mod t_spec;
mod t_state;
mod timeouts;

pub use laya_workflow::apps;
pub use laya_workflow::backend::{choice, noul, score, verdict, ScriptedBackend};
pub use laya_workflow::capability;
pub use laya_workflow::optimizer;
pub use laya_workflow::spec;
pub use laya_workflow::workflow::{
    consecutive_same, Checkpoint, Decide, Edge, FanOut, NodeAction, ResilientWorkflow, SubWorkflow,
    WorkflowNode,
};
pub use serde_json::{json, Map, Value};

pub struct Harness {
    pub passed: usize,
    pub failed: usize,
}

impl Harness {
    pub fn check(&mut self, name: &str, cond: bool) {
        if cond {
            self.passed += 1;
            println!("  ok   {name}");
        } else {
            self.failed += 1;
            println!("  FAIL {name}");
        }
    }
    pub fn eq<T: std::fmt::Debug + PartialEq>(&mut self, name: &str, got: T, want: T) {
        let ok = got == want;
        if !ok {
            println!("       got={got:?} want={want:?}");
        }
        self.check(name, ok);
    }
}

pub fn q(v: &[(&str, &str)]) -> Value {
    let mut m = Map::new();
    for (k, s) in v {
        m.insert(k.to_string(), json!(s));
    }
    Value::Object(m)
}

fn sections() -> &'static [(&'static str, fn(&mut Harness))] {
    &[
        ("edge", engine::test_edge),
        ("node", engine::test_node),
        ("workflow", engine::test_workflow),
        ("composition", engine::test_composition),
        (
            "apps: rule logic with scripted verdicts",
            t_apps::test_apps_rule_logic_with_scripted_verdicts,
        ),
        ("app workflows", t_apps::test_app_workflows),
        ("optimizer", t_apps::test_optimizer),
        ("spec", t_spec::test_spec),
        ("spec-nesting", t_spec::test_spec_nesting),
        ("spec-folders", t_spec::test_spec_folders),
        ("spec-version", t_spec::test_spec_version),
        ("spec-layers", t_spec::test_spec_layers),
        ("state", t_state::test_state_home_override),
        ("install", t_state::test_install),
        ("capabilities", capabilities::test_capabilities),
        ("span-selection", capabilities::test_span_selection),
        ("capabilities-extra", capabilities::test_capabilities_extra),
        // On-device Needle 3 integration (extract / embed / complete); skipped
        // when libneedle.so or needle3.cact is not installed.
        ("needle", capabilities::test_needle),
        (
            "capabilities-batch2",
            capabilities2::test_capabilities_batch2,
        ),
        ("codegraph", codegraph::test_codegraph),
        (
            "capabilities-batch3",
            capabilities3::test_capabilities_batch3,
        ),
        ("web-research", capabilities3::test_web_research),
        ("secrets", secrets::test_secrets),
        ("capability-timeouts", timeouts::test_capability_timeouts),
        ("heuristic-fixes", heuristics::test_heuristic_fixes),
        ("persist", persist::test_persistence),
        ("accuracy", heuristics::test_accuracy),
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!("laya-workflow-tests [--list] [SECTION ...]");
        eprintln!("  no args       run every section (full suite)");
        eprintln!("  --list        print section names, run nothing");
        eprintln!("  SECTION ...   run only the named sections (prefix match)");
        eprintln!("  env LAYA_TEST_SECTIONS=comma,list  same selection");
        eprintln!();
        eprintln!("sections:");
        for (name, _) in sections() {
            eprintln!("  {name}");
        }
        std::process::exit(if args.len() > 1 { 0 } else { 2 });
    }
    if args.iter().any(|a| a == "--list" || a == "-l") {
        for (name, _) in sections() {
            println!("{name}");
        }
        return;
    }
    let mut h = Harness {
        passed: 0,
        failed: 0,
    };
    let mut filters: Vec<String> = args;
    if let Ok(env) = std::env::var("LAYA_TEST_SECTIONS") {
        for s in env.split(',') {
            let s = s.trim();
            if !s.is_empty() {
                filters.push(s.to_string());
            }
        }
    }
    for (name, run) in sections() {
        let selected = filters.is_empty()
            || filters
                .iter()
                .any(|f| f == "all" || name.starts_with(f.as_str()));
        if !selected {
            continue;
        }
        println!("[{name}]");
        run(&mut h);
    }
    println!("\n{} passed, {} failed", h.passed, h.failed);
    if h.failed > 0 {
        std::process::exit(1);
    }
}

fn _assert_trait_object() {
    let _: Option<&dyn Decide> = None;
}
