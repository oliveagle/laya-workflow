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
