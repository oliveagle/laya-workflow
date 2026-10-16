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

use super::run_code_in;
use super::Harness;

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
