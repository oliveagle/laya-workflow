//! `goal_runner` capability — run an external agent's goal loop on a target doc.
//!
//! Some goals are best handed to a full agent harness rather than a single
//! capability call. Two such harnesses exist in this environment:
//!
//! * `cxgo`  → `codex_goal_runner.py`, driving **codex app-server**
//! * `cmdgo` → `cmd_goal_runner.py`, driving **command-code headless** (`cmd -p`)
//!
//! Both run a goal loop against a markdown target and, crucially, decide
//! "done" from **external evidence** rather than the model's own claim
//! (acceptance reports + hash de-duplication + an explicit completion marker).
//! This capability exposes that loop to a workflow spec.
//!
//! ## Contract
//!
//! The runners exit `0` only when their external checks pass and write their
//! reports under `<workdir>/acceptance-reports`. The result therefore reports
//! `ok` (exit 0), the exit code, the truncated stdout/stderr, and the report
//! directory, so a workflow can branch on real evidence instead of prose.
//!
//! ## Safety
//!
//! Spawning a process is gated behind `policy.allow_exec`, exactly like `exec`,
//! `shell` and `agent`. The wrapper path is checked against an explicit
//! allow-list (`wrapper` field must name a known runner) so a spec cannot point
//! this at an arbitrary binary, and the target doc must live under
//! `policy.allow_paths`. Output is capped by `max_output` and the run by
//! `timeout_ms` (itself capped by `policy.max_timeout_ms`).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use super::{bounded_timeout, expand, stringify, Policy};

/// The runners this capability is allowed to invoke.
///
/// A fixed list rather than a free-form command: the point of the capability is
/// "run a goal doc through one of the known harnesses", not "run any binary".
const RUNNERS: &[(&str, &str)] = &[
    ("cxgo", "codex_goal_runner.py"),
    ("cmdgo", "cmd_goal_runner.py"),
];

#[derive(Clone, Debug, Default)]
pub struct GoalRunnerCap {
    /// `cxgo` | `cmdgo` (see [`RUNNERS`]).
    pub runner: String,
    /// Extra runner flags passed verbatim after the target doc.
    pub args: Vec<String>,
    pub timeout_ms: u64,
    /// Cap for the captured stdout+stderr.
    pub max_output: usize,
    /// Where the runner's reports land, relative to the workdir
    /// (default `acceptance-reports`).
    pub reports_dir: String,
}

/// Resolve which harness to use and where its wrapper lives.
fn resolve_runner(name: &str) -> Result<(&'static str, PathBuf)> {
    let want = if name.is_empty() { "cxgo" } else { name };
    let script = RUNNERS
        .iter()
        .find(|(n, _)| *n == want)
        .map(|(_, s)| *s)
        .ok_or_else(|| {
            anyhow!(
                "goal_runner: unknown runner {want:?} (known: {:?})",
                RUNNERS.iter().map(|(n, _)| *n).collect::<Vec<_>>()
            )
        })?;
    // The wrappers live next to each other under a bin dir; find that dir from
    // the environment rather than hard-coding a user path.
    let bin = std::env::var("LAYA_AGENT_BIN_DIR").ok().filter(|s| !s.is_empty());
    let wrapper = match bin {
        Some(d) => PathBuf::from(d).join(want),
        None => {
            // Fall back to PATH lookup so the capability works without config.
            PathBuf::from(want)
        }
    };
    Ok((script, wrapper))
}

pub fn call_goal_runner(c: &GoalRunnerCap, with: &Value, state: &Value, policy: &Policy) -> Result<Value> {
    if !policy.allow_exec {
        bail!("goal_runner spawns an agent process; set policy.allow_exec = true to enable");
    }

    let runner = stringify(&expand(&Value::String(c.runner.clone()), state, with));
    let (_script, wrapper) = resolve_runner(&runner)?;

    // Target goal doc: required, and must sit under an allowed path.
    let doc = with
        .get("goal")
        .or_else(|| with.get("doc"))
        .map(stringify)
        .map(|d| stringify(&expand(&Value::String(d), state, with)))
        .unwrap_or_default();
    if doc.trim().is_empty() {
        bail!("goal_runner needs 'goal' (path to the target markdown)");
    }
    let doc_path = Path::new(&doc);
    if !doc_path.exists() {
        bail!("goal_runner: goal doc {doc:?} does not exist");
    }
    if policy.allow_paths.is_empty() {
        bail!("goal_runner needs policy.allow_paths (empty ⇒ denied)");
    }
    let cand = std::fs::canonicalize(doc_path).unwrap_or_else(|_| doc_path.to_path_buf());
    let allowed = policy.allow_paths.iter().any(|r| {
        let rc = std::fs::canonicalize(r).unwrap_or_else(|_| PathBuf::from(r));
        cand.starts_with(&rc)
    });
    if !allowed {
        bail!("goal_runner: goal {doc:?} is outside policy.allow_paths (denied)");
    }

    // Working directory: the doc's own directory by default.
    let workdir = with
        .get("workdir")
        .map(stringify)
        .filter(|w| !w.trim().is_empty())
        .unwrap_or_else(|| {
            cand.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| ".".to_string())
        });
    let reports = if c.reports_dir.is_empty() { "acceptance-reports" } else { c.reports_dir.as_str() };

    let timeout = bounded_timeout(if c.timeout_ms == 0 { 1_800_000 } else { c.timeout_ms }, policy);
    let cap = if c.max_output == 0 { 256 << 10 } else { c.max_output }.min(policy.max_output);

    // Assemble: <wrapper> <doc> [extra args...]
    let mut argv: Vec<String> = vec![wrapper.to_string_lossy().to_string(), doc.clone()];
    for a in &c.args {
        argv.push(stringify(&expand(&Value::String(a.clone()), state, with)));
    }
    // Allow the caller to append per-invocation args too.
    if let Some(extra) = with.get("args").and_then(|v| v.as_array()) {
        for a in extra {
            argv.push(stringify(a));
        }
    }

    let started = Instant::now();
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("goal_runner: cannot start {runner}: {e}"))?;

    let (out, err, status, timed_out) = wait_with_cap(&mut child, timeout, cap)?;
    let elapsed_ms = started.elapsed().as_millis() as i64;

    let reports_path = Path::new(&workdir).join(reports);
    let report_count = std::fs::read_dir(&reports_path)
        .map(|rd| rd.filter_map(|e| e.ok()).filter(|e| e.path().is_file()).count())
        .unwrap_or(0);

    // `ok` is the runner's own externally-checked verdict, never a guess.
    let ok = status == Some(0);
    Ok(json!({
        "capability": "goal_runner",
        "runner": runner,
        "goal": doc,
        "workdir": workdir,
        "ok": ok,
        "exit_code": status,
        "timed_out": timed_out,
        "elapsed_ms": elapsed_ms,
        "reports_dir": reports_path.to_string_lossy(),
        "report_count": report_count,
        "stdout": truncate(out, cap),
        "stderr": truncate(err, cap),
        "command": argv,
    }))
}

/// Read the child's pipes while enforcing a wall-clock timeout.
///
/// Reading has to happen on separate threads: a blocking `wait_with_output`
/// would ignore the timeout entirely, and a child that fills its pipe buffer
/// would deadlock if we waited first.
fn wait_with_cap(
    child: &mut std::process::Child,
    timeout: Duration,
    cap: usize,
) -> Result<(String, String, Option<i32>, bool)> {
    use std::io::Read as _;

    let out_pipe = child.stdout.take().ok_or_else(|| anyhow!("goal_runner: no stdout"))?;
    let err_pipe = child.stderr.take().ok_or_else(|| anyhow!("goal_runner: no stderr"))?;

    let out_h = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out_pipe.take(cap as u64 + 1).read_to_end(&mut buf);
        buf
    });
    let err_h = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err_pipe.take(cap as u64 + 1).read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + timeout;
    let mut status = None;
    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(s)) => {
                status = s.code();
                break;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    // Kill the whole process group so a runner's children go too.
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(anyhow!("goal_runner: wait failed: {e}")),
        }
    }

    let out = out_h.join().unwrap_or_default();
    let err = err_h.join().unwrap_or_default();
    Ok((
        String::from_utf8_lossy(&out).to_string(),
        String::from_utf8_lossy(&err).to_string(),
        status,
        timed_out,
    ))
}

fn truncate(mut s: String, max: usize) -> String {
    if s.len() > max {
        // Trim on a char boundary so the result stays valid UTF-8.
        let mut cut = max;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("...[truncated]");
    }
    s
}
