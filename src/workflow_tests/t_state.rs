//! The state root and the `install` command: after `laya-workflow install`, a
//! checkout must not be needed for anything.
//!
//! These are end-to-end assertions through the real CLI binary, because the
//! bugs they guard are all "the layers disagree at runtime" — a unit test of
//! `state_dir()` would pass while `plugin list` still resolved from the repo.

use std::path::{Path, PathBuf};
use std::process::Command;

pub use crate::{Harness};

/// `Harness::check` takes only a condition, so a failing assertion that printed
/// nothing left no way to tell *which* value was wrong. These wrappers keep the
/// observed value next to the failure.
fn check_msg(h: &mut Harness, name: &str, cond: bool, detail: impl std::fmt::Debug) {
    if !cond {
        println!("       detail: {detail:?}");
    }
    h.check(name, cond);
}

fn cli() -> PathBuf {
    // The test binary lives beside the CLI in target/<profile>/.
    let mut p = std::env::current_exe().expect("test binary path");
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("laya-workflow")
}

/// Run the CLI with a private state root and cwd, returning (success, stdout+stderr).
fn run_in(root: &Path, cwd: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(cli())
        .args(args)
        .current_dir(cwd)
        .env("LAYA_HOME", root)
        // A stray override from the developer's shell would silently redirect
        // the very paths under test.
        .env_remove("LAYA_PLUGIN_DIR")
        .env_remove("LAYA_USER_PLUGIN_DIR")
        .env_remove("LAYA_USER_DSL_DIR")
        .env_remove("LAYA_MEM_SQLITE")
        .env_remove("LAYA_MEM_SPEC_DIR")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .expect("run laya-workflow");
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

fn fresh(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("laya-state-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A cwd with no `plugins/`, no `websites/` and no `.git` — i.e. not a checkout.
/// Plugin resolution from there can only be answered by the user layer, which is
/// exactly the claim `install` makes.
fn outside_a_checkout(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("laya-cwd-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn test_install(h: &mut Harness) {
    let root = fresh("install");
    let cwd = outside_a_checkout("install");

    let (ok, out) = run_in(&root, &cwd, &["install"]);
    h.eq("install: exits 0", ok, true);
    if !ok {
        eprintln!("{}", &out[..out.len().min(2000)]);
    }

    // The layout the whole tool now shares.
    for sub in ["dsl", "plugins", "websites", "laya-mem", "chrome"] {
        h.eq(
            &format!("install: creates {sub}/"),
            root.join(sub).is_dir(),
            true,
        );
    }
    h.eq(
        "install: laya-mem specs land under the state root",
        root.join("laya-mem/specs/admission.json").is_file(),
        true,
    );

    // Every bundled plugin, discoverable with no checkout in sight.
    let (ok, out) = run_in(&root, &cwd, &["plugin", "list"]);
    h.eq("plugin list outside a checkout: exits 0", ok, true);
    let listed = out.lines().filter(|l| l.contains(" user ")).count();
    check_msg(
        h,
        "install: installed plugins into the user layer",
        listed >= 14,
        listed,
    );
    h.check(
        "install: plugins resolve from the user layer, not a repo",
        out.contains(" user "),
    );

    // The specs must be complete: a server that starts and then answers
    // `spec not found` is the failure this check exists to catch.
    let (ok, out) = run_in(&root, &cwd, &["laya-mem", "info"]);
    h.eq("laya-mem info: exits 0", ok, true);
    // Built from EMBEDDED_SPECS so adding a 10th spec doesn't break this test.
    let all_specs = format!(
        "all {} present",
        laya_workflow::laya_mem::EMBEDDED_SPECS.len()
    );
    check_msg(
        h,
        &format!("laya-mem info: {all_specs}"),
        out.contains(&all_specs),
        &out,
    );
    check_msg(
        h,
        "laya-mem info: store under the state root",
        out.contains(root.join("laya-mem").to_str().unwrap()),
        out.lines()
            .find(|l| l.starts_with("store:"))
            .unwrap_or("<no store: line>"),
    );

    // Idempotence: a second run must not fail or duplicate, and without --force
    // it must not clobber an edited spec.
    let edited = root.join("laya-mem/specs/routing.json");
    std::fs::write(&edited, "{\"dsl_version\":2,\"start\":\"EDITED\"}").unwrap();
    let (ok, out2) = run_in(&root, &cwd, &["install"]);
    h.eq("install: re-run exits 0", ok, true);
    check_msg(
        h,
        "install: re-run keeps existing plugins",
        out2.contains("already present"),
        out2.lines()
            .find(|l| l.starts_with("plugins:"))
            .unwrap_or("<no plugins: line>"),
    );
    h.check(
        "install: re-run preserves a locally edited spec",
        std::fs::read_to_string(&edited).unwrap().contains("EDITED"),
    );

    // --force refreshes plugins and specs from the binary.
    let (ok, _) = run_in(&root, &cwd, &["install", "--force"]);
    h.eq("install --force: exits 0", ok, true);
    h.check(
        "install --force: restores the shipped spec",
        std::fs::read_to_string(&edited).unwrap().contains("\"nodes\""),
    );

    // A fresh root with nothing but --dirs-only must create layout and install
    // no plugins, so the two halves of `install` are separately usable.
    let bare = fresh("dirsonly");
    let (ok, _) = run_in(&bare, &cwd, &["install", "--dirs-only"]);
    h.eq("install --dirs-only: exits 0", ok, true);
    h.eq(
        "install --dirs-only: creates the root",
        bare.join("plugins").is_dir(),
        true,
    );
    h.eq(
        "install --dirs-only: installs no plugins",
        std::fs::read_dir(bare.join("plugins")).unwrap().count(),
        0,
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&bare);
    let _ = std::fs::remove_dir_all(&cwd);
}

/// `LAYA_HOME` relocates everything, so a checkout can be exercised against a
/// throwaway state root without touching the real one.
pub fn test_state_home_override(h: &mut Harness) {
    let a = fresh("home-a");
    let b = fresh("home-b");
    let cwd = outside_a_checkout("home");

    let (ok, out_a) = run_in(&a, &cwd, &["install", "--dirs-only"]);
    h.eq("LAYA_HOME=a: exits 0", ok, true);
    let (ok, out_b) = run_in(&b, &cwd, &["install", "--dirs-only"]);
    h.eq("LAYA_HOME=b: exits 0", ok, true);

    check_msg(
        h,
        "LAYA_HOME: a's root reports itself",
        out_a.contains(a.to_str().unwrap()),
        out_a.lines().next().unwrap_or("<empty>"),
    );
    check_msg(
        h,
        "LAYA_HOME: b's root reports itself",
        out_b.contains(b.to_str().unwrap()),
        out_b.lines().next().unwrap_or("<empty>"),
    );
    h.check(
        "LAYA_HOME: the two roots are independent",
        a.join("plugins").canonicalize().unwrap() != b.join("plugins").canonicalize().unwrap(),
    );

    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
    let _ = std::fs::remove_dir_all(&cwd);
}
