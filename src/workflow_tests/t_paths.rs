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

/// P238: the same "name the confusion" pass over the remaining `bdd`
/// sub-commands. `transpile` / `build` take `.feature` documents, `import` takes
/// a spec `.json`, `port-check --pin` takes a sealed manifest, `evaluate
/// --dataset` takes a labeled `.jsonl`, and `bdd score` takes task *ids*. Each
/// used to answer a raw errno (`No such file or directory (os error 2)`, `Is a
/// directory (os error 21)`, `Not a directory (os error 20)`), a context-only io
/// error (`<path>: reading <path>`), or a bare serde error (`expected value at
/// line 1 column 1`) - none of which names the fix. Each now names the next step,
/// and the happy paths stay unregressed.
pub fn test_bdd_arg_hints(h: &mut Harness) {
    let cwd = std::env::temp_dir().join(format!("laya-bddargv-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(cwd.join("sub")).unwrap();
    std::fs::write(cwd.join("doc.feature"), "Feature: x\n").unwrap();
    std::fs::write(cwd.join("out.txt"), "x\n").unwrap();
    // A spec that round-trips (`import` -> `build`), so both happy paths really run.
    let spec = json!({
        "dsl_version": 2, "name": "hint_ok", "start": "a",
        "max_iterations": 3, "convergence_window": 2, "convergence_eps": 0.001,
        "nodes": [{
            "name": "a", "primary_q": "q",
            "questions": {"q": {"type": "choice", "instructions": "?", "criteria": {"A": "no", "B": "yes"}}},
            "edge": {"condition": {"A": "STOP", "B": "STOP"}, "default": "STOP"},
            "min_confidence": 0.0, "max_retries": 1,
            "action": {"kind": "copy_keys", "keys": ["q"]}
        }]
    });
    std::fs::write(cwd.join("ok.json"), serde_json::to_string_pretty(&spec).unwrap()).unwrap();

    // Happy paths: the guards must not over-trigger.
    let (code, text) = run_code_in(&cwd, &["bdd", "import", "ok.json"], &[]);
    h.eq("bdd-arg: `bdd import <spec.json>` still imports (rc=0)", code, Some(0));
    h.check(
        "bdd-arg: import emits a @decision feature",
        text.contains("@decision") && text.contains("Feature: hint_ok"),
    );
    std::fs::write(cwd.join("roundtrip.feature"), &text).unwrap();
    let (code, _) = run_code_in(
        &cwd,
        &["bdd", "build", "roundtrip.feature", "--out", "build_out"],
        &[],
    );
    h.eq("bdd-arg: `bdd build <feature>` still builds (rc=0)", code, Some(0));

    // transpile: a spec (was a Feature-header parse error / the wrong path).
    let (code, text) = run_code_in(&cwd, &["bdd", "transpile", "ok.json"], &[]);
    h.eq("bdd-arg: `bdd transpile <spec.json>` rc=1", code, Some(1));
    h.check(
        "bdd-arg: transpile on a spec points at `run --spec` / `bdd import`",
        text.contains("compiled workflow spec")
            && text.contains("run --spec")
            && !text.contains("os error"),
    );
    // transpile: a directory (was `Is a directory (os error 21)`).
    let (code, text) = run_code_in(&cwd, &["bdd", "transpile", "sub"], &[]);
    h.eq("bdd-arg: `bdd transpile <dir>` rc=1", code, Some(1));
    h.check(
        "bdd-arg: transpile on a directory says so",
        text.contains("is a directory") && !text.contains("os error"),
    );
    // transpile: a missing feature.
    let (code, text) = run_code_in(&cwd, &["bdd", "transpile", "ghost.feature"], &[]);
    h.eq("bdd-arg: `bdd transpile <missing>` rc=1", code, Some(1));
    h.check(
        "bdd-arg: transpile on a missing feature says `no such file`",
        text.contains("no such file") && !text.contains("os error"),
    );

    // build: a missing feature (was the context-only `<path>: reading <path>`).
    let (code, text) = run_code_in(&cwd, &["bdd", "build", "ghost.feature"], &[]);
    h.eq("bdd-arg: `bdd build <missing.feature>` rc=1", code, Some(1));
    h.check(
        "bdd-arg: build on a missing feature says `no such file`",
        text.contains("no such file") && !text.contains("os error"),
    );
    // build: a spec handed to build (was `text before any Feature: header: "{"`).
    let (code, text) = run_code_in(&cwd, &["bdd", "build", "ok.json"], &[]);
    h.eq("bdd-arg: `bdd build <spec.json>` rc=1", code, Some(1));
    h.check(
        "bdd-arg: build on a spec points at `run --spec`",
        text.contains("compiled workflow spec")
            && text.contains("run --spec")
            && !text.contains("Feature: header"),
    );
    // build: `--out` that is a file (was `Not a directory (os error 20)`).
    let (code, text) = run_code_in(
        &cwd,
        &["bdd", "build", "roundtrip.feature", "--out", "out.txt"],
        &[],
    );
    h.eq("bdd-arg: `bdd build --out <file>` rc=1", code, Some(1));
    h.check(
        "bdd-arg: build `--out <file>` says not a directory",
        text.contains("is not a directory") && !text.contains("os error"),
    );

    // import: a missing spec / a feature handed to import (was a bare serde error).
    let (code, text) = run_code_in(&cwd, &["bdd", "import", "ghost.json"], &[]);
    h.eq("bdd-arg: `bdd import <missing.json>` rc=1", code, Some(1));
    h.check(
        "bdd-arg: import on a missing spec says `no such file`",
        text.contains("no such file") && !text.contains("os error"),
    );
    let (code, text) = run_code_in(&cwd, &["bdd", "import", "doc.feature"], &[]);
    h.eq("bdd-arg: `bdd import <feature>` rc=1", code, Some(1));
    h.check(
        "bdd-arg: import on a feature points at `bdd transpile`",
        text.contains("is a BDD `.feature`")
            && text.contains("bdd transpile")
            && !text.contains("expected value"),
    );

    // port-check: a missing / directory seal (was `reading the seal <path>` + errno).
    let (code, text) = run_code_in(&cwd, &["bdd", "port-check", "--pin", "ghost.json"], &[]);
    h.eq("bdd-arg: `bdd port-check --pin <missing>` rc=1", code, Some(1));
    h.check(
        "bdd-arg: port-check on a missing seal says `does not exist`",
        text.contains("does not exist")
            && text.contains("--write-pin")
            && !text.contains("os error"),
    );
    let (code, text) = run_code_in(&cwd, &["bdd", "port-check", "--pin", "sub"], &[]);
    h.eq("bdd-arg: `bdd port-check --pin <dir>` rc=1", code, Some(1));
    h.check(
        "bdd-arg: port-check on a directory seal says so",
        text.contains("is a directory") && !text.contains("os error"),
    );

    // evaluate: a missing / directory dataset (was a bare `os error 2` / `21`).
    for ds in ["ghost.jsonl", "sub"] {
        let (code, text) = run_code_in(
            &cwd,
            &["evaluate", "--spec", "ok.json", "--dataset", ds],
            &[],
        );
        h.eq(&format!("bdd-arg: `evaluate --dataset {ds}` rc=1"), code, Some(1));
        h.check(
            &format!("bdd-arg: `evaluate --dataset {ds}` names the flag, not an errno"),
            text.contains("evaluate --dataset") && !text.contains("os error"),
        );
    }

    // score: an unknown task id (ids, not a file / a path).
    let (code, text) = run_code_in(&cwd, &["bdd", "score", "no-such-task-id"], &[]);
    h.eq("bdd-arg: `bdd score <unknown-id>` rc=1", code, Some(1));
    h.check(
        "bdd-arg: score on an unknown id names the tasks corpus",
        text.contains("no authoring task matched")
            && text.contains("bench/bdd/tasks")
            && !text.contains("os error"),
    );

    let _ = std::fs::remove_dir_all(&cwd);
}
