//! The last of the build-time crate-dir readers (P218).
//!
//! `repo_root()` is `env!("CARGO_MANIFEST_DIR")` — a *compile-time* constant. In a
//! dev tree it is the checkout, so everything resolves and nothing looks wrong; a
//! release or installed binary carries the path of the machine it was built on
//! instead. P212-P217 closed the corpus gates, the remaining `bdd` sub-commands,
//! `dsl smoke`, the `builtin` spec layer, the injected browser extension and the
//! plugin gate. These are the two that were still left, and both were visible on
//! the v0.11.0 release asset run from a directory with no checkout:
//!
//!   browser-demo                `Error: demo site root not found:
//!                                <build-machine-path>/bench/demo-site`    rc=1
//!   bench backend-comparison    every row `ERR`, no reason given             rc=0
//!
//! The second is the worse one: a benchmark that measured nothing read as a clean
//! run (`heuristic` column all `ERR`, exit 0) — the fail-open green again. Both now
//! resolve through `repo_root()` and answer rc=2 with one line of reason when there
//! is no checkout, and `bench::backend_comparison` refuses to print a table it
//! could not fill.

use super::{json, run_code_in, Harness};

pub fn test_build_time_paths(h: &mut Harness) {
    let cwd = std::env::temp_dir().join(format!("laya-paths-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(&cwd).unwrap();
    let bogus = format!("/nonexistent-laya-checkout-{}", std::process::id());

    // `browser-demo` defaults to a repo file for both the site and the spec, so
    // with no checkout it must say so (rc=2) rather than fail on a path that only
    // existed on the build machine.
    let (code, text) = run_code_in(&cwd, &["browser-demo"], &[("LAYA_WORKFLOW_REPO", &bogus)]);
    h.eq("browser-demo with no checkout: rc=2", code, Some(2));
    h.check(
        "browser-demo: the reason names the override given and how a release binary is pointed at a checkout",
        text.contains("does not exist")
            && text.contains("Point LAYA_WORKFLOW_REPO at a laya-workflow checkout"),
    );

    // `bench backend-comparison` loads a laya_mem spec per case. Refusing to run is
    // the whole point: an all-ERR table at rc=0 is a measured-nothing green.
    let (code, text) = run_code_in(
        &cwd,
        &["bench", "backend-comparison"],
        &[("LAYA_WORKFLOW_REPO", &bogus)],
    );
    h.eq("bench backend-comparison with no checkout: rc=2", code, Some(2));
    h.check(
        "bench backend-comparison with no checkout: prints no table",
        !text.contains("ALLOW") && !text.contains("expected"),
    );

    // The dev binary, with a real checkout beside it, still measures: real
    // verdicts, not `ERR`.
    let (code, text) = run_code_in(&cwd, &["bench", "backend-comparison"], &[]);
    h.eq("bench backend-comparison in a checkout: rc=0", code, Some(0));
    h.check(
        "bench backend-comparison in a checkout: verdicts, not ERR",
        text.contains("ALLOW") && text.contains("BLOCK") && !text.contains("ERR"),
    );

    let _ = std::fs::remove_dir_all(&cwd);
}

/// P237: the `--spec` / feature confusion. `run` / `validate` (and the
/// `resume` / `replay` / `evaluate` siblings) take a `.json` workflow spec;
/// `bdd run` takes `.feature` documents. Passing one where the other belongs
/// used to surface a raw errno (`No such file or directory (os error 2)`,
/// `Is a directory (os error 21)`), a serde error (`expected value at line 1
/// column 1`), or a context-only io error (`<path>: reading <path>`) - none of
/// which names the fix. Each now names the next step, and the happy path is
/// unregressed. The resolver (`workflow_cli::resolve_spec_arg`) checks these
/// before any file is read, so the message is the same on every entry point.
pub fn test_spec_arg_hints(h: &mut Harness) {
    let cwd = std::env::temp_dir().join(format!("laya-argv-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(cwd.join("sub")).unwrap();

    // A minimal valid spec, so the happy path exercises the resolver's one Ok
    // branch rather than being exempt from it.
    let spec = json!({
        "dsl_version": 2, "name": "hint_ok", "start": "a",
        "max_iterations": 3, "convergence_window": 2, "convergence_eps": 0.001,
        "nodes": [{
            "name": "a",
            "questions": {"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}},
            "edge": {"condition": {"A": "STOP", "B": "STOP"}, "default": "STOP"},
            "min_confidence": 0.0, "max_retries": 1,
            "action": {"kind": "copy_keys", "keys": ["q"]}
        }]
    });
    std::fs::write(cwd.join("ok.json"), serde_json::to_string_pretty(&spec).unwrap()).unwrap();
    std::fs::write(cwd.join("doc.feature"), "Feature: x\n").unwrap();

    // The one Ok branch: a real spec still runs.
    let (code, _) = run_code_in(&cwd, &["run", "--spec", "ok.json", "--state", "{}"], &[]);
    h.eq("spec-arg: a real spec still runs (rc=0)", code, Some(0));

    // (1) a spec that is not there (was `No such file or directory (os error 2)`).
    let (code, text) = run_code_in(&cwd, &["run", "--spec", "ghost.json"], &[]);
    h.eq("spec-arg: missing spec rc=1", code, Some(1));
    h.check(
        "spec-arg: a missing spec says so and points at `laya-workflow list`",
        text.contains("not found") && text.contains("laya-workflow list") && !text.contains("os error"),
    );

    // (2) a directory (was `Is a directory (os error 21)`).
    let (code, text) = run_code_in(&cwd, &["run", "--spec", "sub"], &[]);
    h.eq("spec-arg: directory rc=1", code, Some(1));
    h.check(
        "spec-arg: a directory names the mistake, not `Is a directory (os error 21)`",
        text.contains("is a directory, not a spec") && !text.contains("os error"),
    );

    // (3) a `.feature`, on both spec-taking commands (was `expected value at line 1 column 1`).
    for cmd in ["run", "validate"] {
        let (code, text) = run_code_in(&cwd, &[cmd, "--spec", "doc.feature"], &[]);
        h.eq(&format!("spec-arg: `{cmd} --spec` on a .feature rc=1"), code, Some(1));
        h.check(
            &format!("spec-arg: `{cmd} --spec` on a .feature points at `bdd transpile`/`bdd run`"),
            text.contains("is a BDD `.feature`")
                && text.contains("bdd transpile")
                && text.contains("bdd run"),
        );
    }

    // (4) the mirror confusion: a `.json` handed to `bdd run`.
    let (code, text) = run_code_in(&cwd, &["bdd", "run", "ok.json"], &[]);
    h.eq("spec-arg: `bdd run <spec.json>` rc=1", code, Some(1));
    h.check(
        "spec-arg: `bdd run` on a spec points at `run --spec`",
        text.contains("compiled workflow spec") && text.contains("run --spec"),
    );

    // (5) a missing `.feature` (was the context-only `ghost.feature: reading ghost.feature`).
    let (code, text) = run_code_in(&cwd, &["bdd", "run", "ghost.feature"], &[]);
    h.eq("spec-arg: `bdd run <missing.feature>` rc=1", code, Some(1));
    h.check(
        "spec-arg: `bdd run` on a missing feature says `no such file`",
        text.contains("no such file"),
    );

    let _ = std::fs::remove_dir_all(&cwd);
}
