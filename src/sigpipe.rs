//! Broken-pipe behaviour for the two binaries.
//!
//! Rust's runtime installs `SIG_IGN` for `SIGPIPE`, so the OS default disposition
//! is lost. When a reader leaves early — `laya-workflow apps | head -2`, or an
//! agent slicing a long output — the next `println!` fails with `EPIPE` and the
//! standard library panics:
//!
//!     $ laya-workflow apps | head -2
//!     thread 'main' panicked at library/std/src/io/stdio.rs:1166:9:
//!     failed printing to stdout: Broken pipe (os error 32)          rc=101
//!
//! That is the wrong answer to a normal shell idiom (measured on the v0.11.0
//! asset and on this binary before the fix, for `apps`, `skill --list` and
//! `laya-workflow-tests --list`). Restoring the OS default makes the process die
//! the way every Unix tool does — killed by `SIGPIPE`, so the shell reports 141 and
//! nothing is printed, and a pipeline's `echo $?` sees a signal rather than a
//! panic.
//!
//! Off Unix there is no `SIGPIPE`; a closed pipe is an ordinary write error there,
//! so the function is a no-op.

/// Restore the OS default `SIGPIPE` disposition. Call once, first thing in `main`.
#[cfg(unix)]
pub fn restore_sigpipe() {
    // SAFETY: `signal` with `SIG_DFL` only changes this process's disposition for
    // one signal, before any thread or fd of ours exists; it takes no pointers and
    // cannot violate a Rust invariant.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// No-op where there is no `SIGPIPE` to restore.
#[cfg(not(unix))]
pub fn restore_sigpipe() {}

#[cfg(all(test, unix))]
mod tests {
    /// `restore_sigpipe()` must leave `SIG_DFL` installed. Rust's runtime starts at
    /// `SIG_IGN`, so the previous handler is what distinguishes "called" from "not
    /// called": after the call, querying returns `SIG_DFL` again (idempotent).
    #[test]
    fn restore_sigpipe_installs_the_default_disposition() {
        super::restore_sigpipe();
        let prev = unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
        assert_eq!(
            prev,
            libc::SIG_DFL,
            "restore_sigpipe() should have installed SIG_DFL; got {prev:?}"
        );
    }
}
