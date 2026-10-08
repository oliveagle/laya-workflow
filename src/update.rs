//! `update` — self-update from GitHub Releases.
//!
//! Resolves the right release asset for the current platform, downloads the
//! `laya-workflow-<target>.tar.gz` tarball, and atomically replaces the running
//! binary (backing up the old one first). Pure Rust: `ureq` for the GitHub API
//! and the asset download, `flate2` + `tar` for the `.tar.gz`.
//!
//! No network calls are made for `--check`-style decisions beyond the GitHub
//! release lookup itself, and the platform/version/asset logic is split into
//! pure functions so the offline test suite can cover it without a network.

use std::cmp::Ordering;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

/// Default upstream. Overridable via [`UpdateOptions::repo`].
pub const DEFAULT_REPO: &str = "oliveagle/laya-workflow";

/// What the user asked for.
#[derive(Debug, Clone)]
pub struct UpdateOptions {
    /// `owner/repo`, e.g. `oliveagle/laya-workflow`.
    pub repo: String,
    /// Pin a specific tag (e.g. `v0.9.0`); `None` → latest release.
    pub tag: Option<String>,
    /// Only report the latest version / whether an update exists; install nothing.
    pub check_only: bool,
    /// Reinstall even when the resolved version equals the running one.
    pub force: bool,
    /// Override platform detection (e.g. `x86_64-unknown-linux-gnu`).
    pub target: Option<String>,
    /// GitHub API base (tests can point this at a local fixture).
    pub api_base: Option<String>,
}

/// Map `(os, arch)` to the release asset target triple.
///
/// Only the two triples the release workflow actually builds are supported;
/// anything else is an explicit error rather than a silent wrong download.
pub fn detect_target(os: &str, arch: &str) -> Result<String> {
    match (os, arch) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin".to_string()),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu".to_string()),
        (os, arch) => Err(anyhow!(
            "no release asset for platform {os}/{arch}; supported: \
             macos/aarch64 (aarch64-apple-darwin), linux/x86_64 (x86_64-unknown-linux-gnu). \
             Use `--target <triple>` to force one."
        )),
    }
}

/// Parse `v1.2.3` / `1.2.3` (optionally `-rc1`-style suffix, ignored for order)
/// into a comparable `(major, minor, patch)`.
pub fn parse_version(raw: &str) -> Result<(u32, u32, u32)> {
    let s = raw.trim().trim_start_matches('v');
    let core = s.split(['-', '+']).next().unwrap_or(s);
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() < 2 || parts.len() > 3 {
        bail!("cannot parse version from {raw:?}");
    }
    let mut out = [0u32; 3];
    for (i, p) in parts.iter().enumerate() {
        let n: u32 = p
            .parse()
            .with_context(|| format!("invalid version component {p:?} in {raw:?}"))?;
        out[i] = n;
    }
    Ok((out[0], out[1], out[2]))
}

/// Compare two version strings (`v0.8.0` < `v0.9.0` < `v0.10.0`).
pub fn cmp_versions(a: &str, b: &str) -> Result<Ordering> {
    let (a, b) = (parse_version(a)?, parse_version(b)?);
    Ok(a.cmp(&b))
}

/// Asset file name for a target triple.
pub fn asset_name(target: &str) -> String {
    format!("laya-workflow-{target}.tar.gz")
}

fn api_url(api_base: &str, repo: &str, tag: Option<&str>) -> String {
    let base = api_base.trim_end_matches('/');
    match tag {
        Some(t) => format!("{base}/repos/{repo}/releases/tags/{t}"),
        None => format!("{base}/repos/{repo}/releases/latest"),
    }
}

/// One release entry as returned by the GitHub API (only the fields we read).
#[derive(serde::Deserialize)]
struct Release {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(serde::Deserialize)]
struct Asset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    browser_download_url: String,
}

/// Fetch the latest (or a pinned) release from the GitHub API.
fn fetch_release(
    agent: &ureq::Agent,
    api_base: &str,
    repo: &str,
    tag: Option<&str>,
    ua: &str,
) -> Result<Release> {
    let url = api_url(api_base, repo, tag);
    let mut req = agent.get(&url).set("Accept", "application/vnd.github+json");
    let token = std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty());
    if let Some(token) = token {
        req = req.set("Authorization", &format!("Bearer {token}"));
    }
    let resp = req
        .set("User-Agent", ua)
        .call()
        .map_err(|e| anyhow!("GitHub API request to {url} failed: {e}"))?;
    let body = resp
        .into_string()
        .context("reading GitHub API response")?;
    serde_json::from_str(&body)
        .with_context(|| format!("parsing GitHub API response from {url}"))
}

/// Download the release asset into memory.
fn download(agent: &ureq::Agent, url: &str, ua: &str) -> Result<Vec<u8>> {
    let resp = agent
        .get(url)
        .set("User-Agent", ua)
        .call()
        .map_err(|e| anyhow!("downloading asset {url} failed: {e}"))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(200 * 1024 * 1024) // sanity cap: 200 MB
        .read_to_end(&mut buf)
        .context("reading asset body")?;
    Ok(buf)
}

/// Extract the single `laya-workflow` binary from a `.tar.gz` blob into `dest_dir`,
/// returning the extracted file's path.
pub fn extract_binary(gz: &[u8], dest_dir: &Path) -> Result<PathBuf> {
    let decoder = flate2::read::GzDecoder::new(gz);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(dest_dir)
        .with_context(|| format!("unpacking tarball into {}", dest_dir.display()))?;

    // The release workflow packs the binary at the archive root. Accept a
    // nested `laya-workflow` under one directory level too, but refuse to guess
    // when there are several candidates.
    let root = dest_dir.join("laya-workflow");
    if root.is_file() {
        return Ok(root);
    }
    let mut nested: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dest_dir) {
        for e in rd.flatten() {
            let p = e.path().join("laya-workflow");
            if p.is_file() {
                nested.push(p);
            }
        }
    }
    match nested.len() {
        1 => Ok(nested.remove(0)),
        _ => bail!(
            "tarball did not contain a single `laya-workflow` binary (found {} candidates)",
            nested.len()
        ),
    }
}

/// Replace `current` with `new_bin`, keeping a timestamped backup of `current`
/// beside it. Uses a same-dir temp file + atomic rename so a crash mid-copy
/// cannot leave a truncated binary at the live path.
pub fn install_binary(current: &Path, new_bin: &Path) -> Result<PathBuf> {
    let dir = current
        .parent()
        .ok_or_else(|| anyhow!("cannot resolve parent dir of {}", current.display()))?;
    let exe_name = current
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("laya-workflow");

    // 1. Backup the running binary.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = dir.join(format!("{exe_name}.v{stamp}.bak"));
    std::fs::copy(current, &backup).with_context(|| {
        format!(
            "backing up {} -> {}",
            current.display(),
            backup.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o755));
    }

    // 2. Stage the new binary in the same dir (atomic rename needs same fs).
    let tmp = dir.join(format!(".{exe_name}.new.{}.tmp", std::process::id()));
    std::fs::copy(new_bin, &tmp)
        .with_context(|| format!("staging new binary at {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }

    // 3. Atomically replace.
    if let Err(e) = std::fs::rename(&tmp, current) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| {
            format!(
                "replacing {} with {} (is the install dir writable?)",
                current.display(),
                tmp.display()
            )
        });
    }
    Ok(backup)
}

fn user_agent() -> String {
    format!("laya-workflow-update/{}", env!("CARGO_PKG_VERSION"))
}

/// Run the self-update. Returns the resolved tag name (for `--check` callers to
/// inspect) or an error when nothing matched.
pub fn run(opts: &UpdateOptions) -> Result<String> {
    let target = match &opts.target {
        Some(t) => t.clone(),
        None => detect_target(std::env::consts::OS, std::env::consts::ARCH)?,
    };
    let asset = asset_name(&target);
    let api_base = opts
        .api_base
        .clone()
        .unwrap_or_else(|| "https://api.github.com".to_string());
    let repo = if opts.repo.is_empty() {
        DEFAULT_REPO.to_string()
    } else {
        opts.repo.clone()
    };

    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(60))
        .build();
    let ua = user_agent();

    let current = env!("CARGO_PKG_VERSION");
    let rel = fetch_release(&agent, &api_base, &repo, opts.tag.as_deref(), &ua)?;
    let tag = rel.tag_name.clone();

    let Some(asset_url) = rel
        .assets
        .iter()
        .find(|a| a.name == asset)
        .map(|a| a.browser_download_url.clone())
    else {
        bail!(
            "release {tag} has no asset {asset:?} for target {target}; \
             available: {}",
            rel.assets
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    };

    println!("current : laya-workflow v{current} ({target})");
    println!("latest  : {tag}");
    println!("asset   : {asset}");

    if !opts.force {
        let tag_cmp = tag.trim_start_matches('v');
        if tag_cmp == current {
            if opts.check_only {
                println!("up to date.");
            } else {
                println!("already at {tag}; nothing to install (use --force to reinstall).");
            }
            return Ok(tag);
        }
        match cmp_versions(&tag, current) {
            Ok(Ordering::Greater) => {}
            Ok(Ordering::Less) | Ok(Ordering::Equal) => {
                if opts.check_only {
                    println!("installed v{current} is newer than release {tag}.");
                } else {
                    println!(
                        "installed v{current} is newer than release {tag}; \
                         use --tag <version> to downgrade or --force to reinstall."
                    );
                }
                return Ok(tag);
            }
            Err(_) => {}
        }
    }

    if opts.check_only {
        println!("update available: {tag} (installed v{current}). Use `laya-workflow update` to install.");
        return Ok(tag);
    }

    // Download + install.
    println!("downloading {asset_url} ...");
    let gz = download(&agent, &asset_url, &ua)?;
    println!("  {} bytes", gz.len());

    let exe = std::env::current_exe().context("cannot resolve current executable path")?;
    let tmp_root = std::env::temp_dir().join(format!("laya-update-{}", std::process::id()));
    if tmp_root.exists() {
        let _ = std::fs::remove_dir_all(&tmp_root);
    }
    std::fs::create_dir_all(&tmp_root)?;
    let new_bin = extract_binary(&gz, &tmp_root)?;

    // Sanity: refuse to install a non-executable-looking file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&new_bin)?.permissions().mode();
        if mode & 0o111 == 0 {
            let _ = std::fs::remove_dir_all(&tmp_root);
            bail!("downloaded binary {asset} is not executable (mode {mode:o}); aborting");
        }
    }

    let backup = install_binary(&exe, &new_bin)?;
    let _ = std::fs::remove_dir_all(&tmp_root);

    println!("installed {tag} -> {}", exe.display());
    println!("previous binary kept at {}", backup.display());
    println!("run `laya-workflow update --check` to confirm.");
    Ok(tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_cmp() {
        assert_eq!(parse_version("v0.8.0").unwrap(), (0, 8, 0));
        assert_eq!(parse_version("0.9.0").unwrap(), (0, 9, 0));
        assert_eq!(parse_version("v0.10.0").unwrap(), (0, 10, 0));
        assert_eq!(parse_version("v1.2.3-rc1").unwrap(), (1, 2, 3));
        assert!(parse_version("nope").is_err());

        assert_eq!(cmp_versions("v0.8.0", "v0.9.0").unwrap(), Ordering::Less);
        assert_eq!(cmp_versions("v0.9.0", "v0.9.0").unwrap(), Ordering::Equal);
        assert_eq!(cmp_versions("v0.10.0", "v0.9.0").unwrap(), Ordering::Greater);
    }

    #[test]
    fn targets_and_assets() {
        assert_eq!(
            detect_target("macos", "aarch64").unwrap(),
            "aarch64-apple-darwin"
        );
        assert_eq!(
            detect_target("linux", "x86_64").unwrap(),
            "x86_64-unknown-linux-gnu"
        );
        assert!(detect_target("windows", "x86_64").is_err());
        assert!(detect_target("linux", "aarch64").is_err());
        assert_eq!(asset_name("x86_64-unknown-linux-gnu"), "laya-workflow-x86_64-unknown-linux-gnu.tar.gz");
    }
}
