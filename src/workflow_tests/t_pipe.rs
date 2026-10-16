//! A reader that leaves early must end the process with `SIGPIPE`, not a panic.
//!
//! Rust's runtime ignores `SIGPIPE`, so `println!` after the reader closed panics
//! (`failed printing to stdout: Broken pipe`, rc=101). Both binaries now call
//! `sigpipe::restore_sigpipe()` first thing in `main`, and these assertions are what
//! keeps that call from being dropped: an agent (or a shell user) slicing a long
//! output with `head` is the normal case, and a 101 plus a stack trace is not an
//! answer to it.
//!
//! The child is given a write end whose read end is closed *before* it starts, so
//! there is no race with a reader draining a 64 KiB pipe buffer: the first write can
//! only fail with `EPIPE`.

use super::Harness;

#[cfg(unix)]
fn with_closed_stdout(bin: &std::path::Path, args: &[&str]) -> (Option<i32>, Option<i32>, String) {
    use std::io::Read;
    use std::os::unix::io::FromRawFd;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};

    let mut fds = [0i32; 2];
    // SAFETY: `pipe(2)` fills two fds on success; both are closed on every path below.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe(2)");
    let (read_end, write_end) = (fds[0], fds[1]);
    // SAFETY: `read_end` came from `pipe(2)` and is never used again.
    unsafe { libc::close(read_end) };
    // SAFETY: `write_end` came from `pipe(2)`; the `File` takes ownership, so the fd
    // is closed when the child's stdio is dropped.
    let out = unsafe { std::fs::File::from_raw_fd(write_end) };

    let mut child = Command::new(bin)
        .args(args)
        .stdout(Stdio::from(out))
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the CLI");
    // Drain stderr first: reading to EOF waits for exit, and it is where a panic
    // would have written. (Bounded output, so this cannot deadlock.)
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    let status = child.wait().expect("wait");
    (status.code(), status.signal(), err)
}

pub fn test_sigpipe_hygiene(h: &mut Harness) {
    if !cfg!(unix) {
        return;
    }
    #[cfg(unix)]
    {
        let cli = super::cli_bin();
        let tests = std::env::current_exe().expect("test binary path");
        for (label, bin, args) in [
            ("laya-workflow apps", cli.clone(), vec!["apps"]),
            ("laya-workflow skill --list", cli.clone(), vec!["skill", "--list"]),
            ("laya-workflow-tests --list", tests, vec!["--list"]),
        ] {
            let (code, signal, err) = with_closed_stdout(&bin, &args);
            h.check(
                &format!("{label} with a closed stdout: no Rust panic on stderr"),
                !err.contains("panicked") && err.is_empty(),
            );
            h.eq(
                &format!("{label} with a closed stdout: killed by SIGPIPE (13)"),
                signal,
                Some(13),
            );
            h.eq(
                &format!("{label} with a closed stdout: not exit code 101"),
                code,
                None::<i32>,
            );
        }
    }
}
