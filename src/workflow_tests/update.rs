//! Offline unit tests for the `update` self-update subcommand: version parsing
//! and comparison, platform-target detection, asset naming, and a local
//! extract+install round-trip. No network.

use std::cmp::Ordering;
use std::path::Path;

use laya_workflow::update::{asset_name, cmp_versions, detect_target, parse_version};
use super::Harness;

pub fn test_update(h: &mut Harness) {
    // Version parsing.
    let cases = [
        ("v0.8.0", (0, 8, 0)),
        ("0.9.0", (0, 9, 0)),
        ("v0.10.0", (0, 10, 0)),
        ("v1.2.3-rc1", (1, 2, 3)),
        ("v2.0.0+meta", (2, 0, 0)),
    ];
    for (raw, want) in cases {
        h.eq(&format!("parse_version({raw})"), parse_version(raw).unwrap(), want);
    }

    // Comparison.
    let ords = [
        ("v0.8.0", "v0.9.0", Ordering::Less),
        ("v0.9.0", "v0.9.0", Ordering::Equal),
        ("v0.10.0", "v0.9.0", Ordering::Greater),
        ("v0.9.0", "v0.8.0", Ordering::Greater),
    ];
    for (a, b, want) in ords {
        h.eq(&format!("cmp_versions({a},{b})"), cmp_versions(a, b).unwrap(), want);
    }

    // Target detection.
    h.eq("macos/aarch64", detect_target("macos", "aarch64").unwrap(), "aarch64-apple-darwin".to_string());
    h.eq("linux/x86_64", detect_target("linux", "x86_64").unwrap(), "x86_64-unknown-linux-gnu".to_string());
    h.check("windows rejected", detect_target("windows", "x86_64").is_err());
    h.check("linux/aarch64 rejected", detect_target("linux", "aarch64").is_err());

    // Asset naming.
    h.eq("asset linux", asset_name("x86_64-unknown-linux-gnu"), "laya-workflow-x86_64-unknown-linux-gnu.tar.gz".to_string());
    h.eq("asset macos", asset_name("aarch64-apple-darwin"), "laya-workflow-aarch64-apple-darwin.tar.gz".to_string());
}

pub fn test_extract_and_install(h: &mut Harness) {
    // Round-trip a fake tarball through extract_binary, then install_binary
    // onto a scratch copy and confirm the swap + backup. Uses only the pure
    // helpers with a local tmp dir — no network.
    let dir = std::env::temp_dir().join(format!("laya-update-test-{}", std::process::id()));
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::fs::create_dir_all(&dir).unwrap();

    // Build a .tar.gz in memory with one `laya-workflow` entry.
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(encoder);
    let fake_bin: &[u8] = b"#!/bin/sh\necho fake-laya\n";
    let mut header = tar::Header::new_gnu();
    header.set_size(fake_bin.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    tar.append_data(&mut header, "laya-workflow", fake_bin).unwrap();
    let encoder = tar.into_inner().unwrap();
    let gz = encoder.finish().unwrap();

    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let new_bin = laya_workflow::update::extract_binary(&gz, &out).unwrap();
    h.check("extracted name is laya-workflow", new_bin.file_name().map(|s| s.to_string_lossy().into_owned()) == Some("laya-workflow".to_string()));
    h.check("extracted file exists", new_bin.is_file());

    // Simulate the current executable and install over it.
    let current = dir.join("laya-workflow");
    std::fs::write(&current, b"old-binary-content").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let backup = laya_workflow::update::install_binary(&current, &new_bin).unwrap();
    h.check("backup exists", backup.is_file());
    h.check("current replaced with new", std::fs::read(&current).unwrap() == fake_bin);
    h.check("backup holds old", std::fs::read(&backup).unwrap() == b"old-binary-content");

    let _ = std::fs::remove_dir_all(&dir);
}
