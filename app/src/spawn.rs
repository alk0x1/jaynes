//! Starting programs. The app has no console on Windows, so each console
//! program it starts (git, hc, curl) would open a window of its own. On macOS and Linux each starts with no signal
//! blocked, whatever thread starts it (signals.rs).

use std::ffi::OsStr;
use std::process::Command;

pub fn command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    // what the starting thread blocks never reaches the program: a background thread of the app blocks TERM (signals.rs)
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: clear_in_child only calls async-signal-safe functions, between fork and exec
        unsafe {
            cmd.pre_exec(crate::signals::clear_in_child);
        }
    }
    cmd
}
