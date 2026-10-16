//! Tests for the runner's own argv handling: how the `SECTION ...` selection is
//! resolved before any section runs. These spawn **this very binary** as a child
//! so they exercise the real command line, not a re-implementation of it.

use std::process::Command;

use super::Harness;

fn self_bin() -> std::path::PathBuf {
    std::env::current_exe().expect("the test binary's own path")
}

/// Run this binary with `args`, with the section env var cleared so the child's
/// selection is exactly what we passed and nothing else.
fn run_self(args: &[&str]) -> std::process::Output {
    Command::new(self_bin())
        .args(args)
        .env_remove("LAYA_TEST_SECTIONS")
        .output()
        .expect("spawn this test binary")
}

pub fn test_unknown_section_is_an_error(h: &mut Harness) {
    // A name that no section starts with used to read as a green run: the binary
    // printed "0 passed, 0 failed" and exited 0, so a typo - or a stale binary
    // that simply lacks the section - looked like a pass. Measured on the build
    // before this fix: `laya-workflow-tests bdd-compare-systems` answered exactly
    // "0 passed, 0 failed", rc=0, while the current source has that section.
    // It now names the offender and exits non-zero, before running anything.
    let bogus = "zz-no-section-starts-with-this";
    let out = run_self(&[bogus]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    h.check("an unknown section exits non-zero", !out.status.success());
    h.check("the unknown section is named on stderr", stderr.contains(bogus));
    h.check(
        "no fail-open '0 passed, 0 failed' line",
        !stdout.contains("0 passed, 0 failed"),
    );

    // Positive control: a name that really matches still runs, and runs green.
    // `rules-glob` is a small, hermetic section.
    let ok = run_self(&["rules-glob"]);
    let ok_out = String::from_utf8_lossy(&ok.stdout);
    h.check("a real section still runs (exit 0)", ok.status.success());
    h.check(
        "a real section really ran",
        ok_out.contains("[rules-glob]") && ok_out.contains("passed,"),
    );

    // A prefix that matches more than one section is still allowed: the selector
    // is documented as a prefix match, so `rules-p` (-> rules-probability) is a
    // valid selection as long as *something* matches.
    let prefix = run_self(&["rules-p"]);
    let prefix_out = String::from_utf8_lossy(&prefix.stdout);
    h.check("a matching prefix is not an error", prefix.status.success());
    h.check(
        "a matching prefix selected its section",
        prefix_out.contains("[rules-probability]"),
    );
}
