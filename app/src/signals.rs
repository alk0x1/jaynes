//! How the app holds the signals that end a program, and how the programs it starts get them. A program keeps what the
//! thread that started it blocks: Rust's spawn does not reset the mask (std, sys/process/unix), and the threads macOS
//! runs the app's background work on (GPUI's dispatcher, on GCD) block the signals. So the page "Open live version"
//! started from one never ended with TERM, only with KILL (the engineer lead's finding, 21/11); started from the main
//! thread, or from a shell, it did. Every program the app starts from such a thread starts with nothing blocked:
//! through `open` around the work that starts it, and through spawn::command, which clears the mask before exec.

#[cfg(unix)]
/// What this process does on `sig` now: "default", "ignored" or "caught".
pub fn disposition(sig: libc::c_int) -> &'static str {
    // SAFETY: a query, with no new action: sigaction only fills `old`
    let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
    if unsafe { libc::sigaction(sig, std::ptr::null(), &mut old) } != 0 {
        return "unknown";
    }
    match old.sa_sigaction {
        libc::SIG_DFL => "default",
        libc::SIG_IGN => "ignored",
        _ => "caught",
    }
}

#[cfg(unix)]
/// The ending signals as this process holds them, in a line for the log.
pub fn said() -> String {
    [("TERM", libc::SIGTERM), ("INT", libc::SIGINT), ("HUP", libc::SIGHUP), ("QUIT", libc::SIGQUIT), ("PIPE", libc::SIGPIPE)]
        .iter()
        .map(|(n, s)| format!("{n} {}", disposition(*s)))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(unix)]
/// Whether this thread blocks `sig` now: a program started from it keeps the thread's mask (Rust's spawn does not
/// reset it), and a blocked TERM never ends it.
pub fn blocked_here(sig: libc::c_int) -> bool {
    // SAFETY: a query, with no new mask: pthread_sigmask only fills `old`
    let mut old: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &mut old) == 0 && libc::sigismember(&old, sig) == 1 }
}

#[cfg(unix)]
/// Runs `f` with no signal blocked on this thread, then blocks again what was: every program `f` starts, here or deep
/// in the engine (the page, `jaynes` to halt or continue), starts with the signals as a program expects them.
pub fn open<T>(f: impl FnOnce() -> T) -> T {
    // SAFETY: the mask of this thread only, put back as it was once `f` returns
    let mut was: libc::sigset_t = unsafe { std::mem::zeroed() };
    let mut none: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe {
        libc::sigemptyset(&mut none);
        libc::pthread_sigmask(libc::SIG_SETMASK, &none, &mut was);
    }
    let out = f();
    unsafe {
        libc::pthread_sigmask(libc::SIG_SETMASK, &was, std::ptr::null_mut());
    }
    out
}

/// Elsewhere, no signals to clear.
#[cfg(not(unix))]
pub fn open<T>(f: impl FnOnce() -> T) -> T {
    f()
}

#[cfg(unix)]
/// In a program just forked, before exec: nothing blocked (spawn::command).
pub fn clear_in_child() -> std::io::Result<()> {
    // SAFETY: sigemptyset and sigprocmask only touch this new process's own mask, as async-signal-safe calls
    let mut none: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe {
        libc::sigemptyset(&mut none);
        libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::{blocked_here, open};
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;

    /// A thread that blocks TERM, as a background thread of the app does, and what a program started from it does on
    /// TERM: a shell that sends TERM to itself and, if it lives, ends with 7.
    fn from_a_blocking_thread(start: impl FnOnce() -> std::process::ExitStatus + Send + 'static) -> (std::process::ExitStatus, bool) {
        std::thread::spawn(move || {
            let mut term: libc::sigset_t = unsafe { std::mem::zeroed() };
            unsafe {
                libc::sigemptyset(&mut term);
                libc::sigaddset(&mut term, libc::SIGTERM);
                libc::pthread_sigmask(libc::SIG_BLOCK, &term, std::ptr::null_mut());
            }
            let ended = start();
            (ended, blocked_here(libc::SIGTERM))
        })
        .join()
        .expect("the thread")
    }

    const ENDS_ON_TERM: [&str; 2] = ["-c", "kill -TERM $$; sleep 1; exit 7"];

    #[test]
    fn a_program_the_app_starts_from_a_background_thread_ends_on_term() {
        // the page, halt or continue, started in work wrapped by open: TERM ends it, and the thread blocks again after
        let (ended, still) = from_a_blocking_thread(|| open(|| Command::new("/bin/sh").args(ENDS_ON_TERM).status().expect("sh")));
        assert_eq!((ended.signal(), ended.code()), (Some(libc::SIGTERM), None), "it lived through TERM: {ended:?}");
        assert!(still, "the thread's own mask comes back");
        // git, hc, curl, through spawn::command, from such a thread with nothing around it
        let (ended, _) = from_a_blocking_thread(|| crate::spawn::command("/bin/sh").args(ENDS_ON_TERM).status().expect("sh"));
        assert_eq!((ended.signal(), ended.code()), (Some(libc::SIGTERM), None), "it lived through TERM: {ended:?}");
    }
}
