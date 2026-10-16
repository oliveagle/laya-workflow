//! Offline tests for the plugin gate's environment handling.
//!
//! The plugin gate's fold sub-check drives a real browser, so it can only run
//! where a Chrome/Chromium is installed; everywhere else (CI) it is skipped
//! rather than failed. That decision rests on `browser::chrome_available()`,
//! which is `find_chrome_binary_in` over `$PATH`. The path resolution is pure,
//! so it is pinned here: name priority, regular files only, no panic on a
//! missing directory - the three properties the old `is_absolute() && is_file()`
//! probe got wrong (it accepted none of the bare names and always fell through
//! to the hard-coded "google-chrome").

use super::Harness;
use super::run_code_in;
use laya_workflow::capability::browser::{find_chrome_binary_in, CHROME_BINARY_NAMES};

pub fn test_plugin_chrome_discovery(h: &mut Harness) {
    let root = std::env::temp_dir().join(format!("laya-chrome-disc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let empty = root.join("empty");
    let plain = root.join("plain"); // only the distro name
    let branded = root.join("branded"); // a real google-chrome as well
    let dirshadow = root.join("dirshadow"); // a *directory* named like a binary
    for d in [&empty, &plain, &branded, &dirshadow] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(plain.join("chromium-browser"), b"#!/bin/sh\n").unwrap();
    std::fs::write(branded.join("chromium-browser"), b"#!/bin/sh\n").unwrap();
    std::fs::write(branded.join("google-chrome"), b"#!/bin/sh\n").unwrap();
    std::fs::create_dir_all(dirshadow.join("chromium")).unwrap();

    h.eq(
        "a PATH with no candidate resolves to None",
        find_chrome_binary_in(&[empty.clone()]),
        None,
    );
    h.eq(
        "the distro name chromium-browser is discovered",
        find_chrome_binary_in(&[plain.clone()]),
        Some(
            plain
                .join("chromium-browser")
                .to_string_lossy()
                .into_owned(),
        ),
    );
    h.eq(
        "a branded google-chrome is preferred over chromium-browser",
        find_chrome_binary_in(&[branded.clone()]),
        Some(branded.join("google-chrome").to_string_lossy().into_owned()),
    );
    h.eq(
        "name priority dominates PATH order (branded wins anywhere)",
        find_chrome_binary_in(&[plain.clone(), branded.clone()]),
        Some(branded.join("google-chrome").to_string_lossy().into_owned()),
    );
    h.eq(
        "a directory that merely shares the name is not a binary",
        find_chrome_binary_in(&[dirshadow.clone()]),
        None,
    );
    h.eq(
        "a missing PATH directory is skipped, not a panic",
        find_chrome_binary_in(&[root.join("does-not-exist")]),
        None,
    );
    h.check(
        "the candidate list starts with the branded names",
        CHROME_BINARY_NAMES[0] == "google-chrome"
            && CHROME_BINARY_NAMES[1] == "google-chrome-stable",
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// `plugin registry-check` / `plugin check` when there is no checkout to read.
///
/// Both read `env!("CARGO_MANIFEST_DIR")`: they ignored `LAYA_WORKFLOW_REPO`, and
/// with no checkout they compiled zero `.rhai`, skipped goofish and feishu with an
/// `eprintln!` that was never counted, and exited 0. The v0.11.0 release asset
/// printed exactly that (`goofish plugin missing, skipping routing` /
/// `0 .rhai compiled, 0 problems`, rc=0). A gate that examined nothing is not a
/// pass; these are the assertions that keep the fail-open green from coming back.
pub fn test_plugin_checkout_gate(h: &mut Harness) {
    use laya_workflow::capability::plugin as plg;

    // A tree with neither `plugins/` nor `websites/`: every sub-check has nothing
    // to look at, so every one of them must report a problem rather than a skip.
    let empty = std::env::temp_dir().join(format!("laya-plugin-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&empty);
    std::fs::create_dir_all(&empty).unwrap();

    let rc = plg::registry_check_at(&empty).unwrap_or(0);
    h.check(
        "registry-check over a tree with no plugins/ is a problem, not a green",
        rc > 0,
    );
    let gc = plg::gate_at(&empty).unwrap_or(0);
    h.check(
        "plugin check over a tree with no plugins/ or websites/ is a problem, not a green",
        gc > 0,
    );
    h.check(
        "plugin check counts the missing goofish routing + fold spec + feishu, not just skips them",
        gc >= 3,
    );

    // Through the CLI: an override naming a path that is not a checkout is rc=2
    // (the "could not run" code), and the reason says how to fix it.
    let bogus = format!("/nonexistent-laya-checkout-{}", std::process::id());
    for sub in [vec!["plugin", "registry-check"], vec!["plugin", "check"]] {
        let label = sub.join(" ");
        let (code, text) = run_code_in(&empty, &sub, &[("LAYA_WORKFLOW_REPO", &bogus)]);
        h.eq(&format!("{label}: no checkout -> rc=2"), code, Some(2));
        h.check(
            &format!("{label}: names the override that does not exist"),
            text.contains("does not exist"),
        );
        h.check(
            &format!("{label}: says how a release binary is pointed at a checkout"),
            text.contains("Point LAYA_WORKFLOW_REPO at a laya-workflow checkout"),
        );
    }

    // The dev binary, with no override, still reads the checkout it was built in
    // even from an unrelated cwd - the root is the repo, not `pwd`.
    let (code, text) = run_code_in(&empty, &["plugin", "registry-check"], &[]);
    h.eq(
        "registry-check from an unrelated cwd reads the checkout it was built in",
        code,
        Some(0),
    );
    h.check(
        "registry-check reports the registries it actually read",
        text.contains("registries, no stale rows"),
    );

    let _ = std::fs::remove_dir_all(&empty);
}
