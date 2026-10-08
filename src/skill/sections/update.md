# update — self-update from GitHub Releases

`laya-workflow update` installs the newest release of this binary from the
repo's GitHub Releases page, replacing the running executable in place. The
whole path is pure Rust: `ureq` talks to the GitHub API and downloads the asset,
`flate2` + `tar` unpack the `.tar.gz`, and the swap is an atomic same-dir
`rename` with a timestamped backup of the previous binary left beside it.

## Why

This is the "distributed binary" upgrade path. The release workflow
(`.github/workflows/release.yml`) builds two platform tarballs on every tag and
attaches them to the release:

| asset | platform |
|---|---|
| `laya-workflow-aarch64-apple-darwin.tar.gz` | macOS (Apple Silicon) |
| `laya-workflow-x86_64-unknown-linux-gnu.tar.gz` | Linux x86_64 |

`update` picks the right asset for the host it runs on (override with
`--target`), so `~/.cargo/bin/laya-workflow` — or any other copy of the binary —
can upgrade itself without a checkout.

## Usage

```sh
laya-workflow update                 # install the latest release
laya-workflow update --check         # report current vs latest, install nothing
laya-workflow update --tag v0.8.0    # install a specific tag (downgrade works)
laya-workflow update --force         # reinstall even if versions match
laya-workflow update --repo owner/repo   # update from a fork/mirror
laya-workflow update --target <triple>   # override platform detection
```

## What it does

1. Resolves the platform target triple (`macos/aarch64` →
   `aarch64-apple-darwin`, `linux/x86_64` → `x86_64-unknown-linux-gnu`; anything
   else is an explicit error, not a guess).
2. Asks the GitHub API for the latest release (or the `--tag` you pinned) and
   finds the `laya-workflow-<target>.tar.gz` asset.
3. Compares versions. Same version → "up to date" (no body download);
   installed newer than release → tells you, and offers `--tag`/`--force` to
   move the other way.
4. Downloads the tarball, unpacks it, sanity-checks the extracted binary is
   executable, backs up the current executable to
   `laya-workflow.v<unix-ts>.bak` in the same directory, then atomically
   renames the new binary over the old one.

## Safety notes

* The old binary is never deleted: the `.bak` copy in the install directory is
  your rollback (`mv laya-workflow.v*.bak laya-workflow`).
* The swap is atomic on the same filesystem (stage a `.new.<pid>.tmp` beside
  the target, then `rename`). A crash mid-download cannot truncate the live
  binary.
* A non-executable tarball (mode lacking `+x`) is refused before install.
* `GITHUB_TOKEN` (when set) is sent as a Bearer token to lift the unauthenticated
  GitHub API rate limit (60 req/hr per IP) — useful on shared IPs.
* `--check` makes a single API call and installs nothing; safe to run on a
  schedule.

Next: `skill --section install`, `skill --section overview`.
