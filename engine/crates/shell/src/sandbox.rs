//! The box on Linux: `jaynes box` runs a command confined by the kernel, as `sandbox-exec` confines it on macOS
//! (engine/python/jaynes_box.py, `profile`). The program confines itself, then becomes the command: Landlock limits
//! what it may read, write, connect to and signal, and a seccomp filter refuses the kinds of sockets and calls
//! Landlock does not see. Neither needs root or a user namespace, so the box opens inside a container too.
//!
//! What the macOS box promises, and what holds it on Linux. Where Linux cannot hold a promise the same way, the box
//! refuses to open; it never opens a looser box. Each line has its test in tests/linux_box.rs.
//!
//! | Promise of the macOS box                                  | On Linux                                              |
//! |-----------------------------------------------------------|-------------------------------------------------------|
//! | writes only in its folders (and /dev/null, ttys)          | Landlock: write rights only beneath them              |
//! | a write cannot leave by a symbolic link, a hard link, a   | Landlock checks the path a write resolves to; a link  |
//! | rename                                                    | or rename across folders needs rights on both sides   |
//! | reads everything but where secrets live and the hidden    | Landlock: read rights on every path but those, found  |
//! | folders                                                   | by walking down to them (symbolic links not followed) |
//! | reads its own folders even inside hidden ones             | Landlock: full rights beneath its folders             |
//! | no network but local ports                                | Landlock: no TCP bind or connect at all; a box asked  |
//! |                                                           | for local ports refuses until the network part        |
//! | UDP, raw sockets and other kinds of socket                | seccomp: only Unix and TCP sockets can be made        |
//! | Unix sockets only in its own folders                      | Landlock ABI 9 (ResolveUnix); below it, seccomp       |
//! |                                                           | refuses every Unix socket, stricter                   |
//! | abstract Unix sockets of other processes                  | Landlock scope (ABI 6)                                |
//! | signals only to processes in the box                      | Landlock scope (ABI 6); ptrace and /proc of processes |
//! |                                                           | outside are refused by Landlock's domain              |
//! | (macOS) no preferences, Apple events, opening apps        | nothing on Linux to hold                              |
//! | io_uring, bpf, perf, keys, new namespaces, mounts         | seccomp refuses them (io_uring would pass the filter) |
//! | the same calls by another way in: x32 numbers, a 32-bit   | seccomp refuses the x32 numbers too; a call of another |
//! | process, socketcall                                       | architecture (socketcall is only 32-bit) ends it      |
//! | a file it was handed open outside its folders             | the box refuses to open while any file but 0, 1 and 2 |
//! |                                                           | would stay open into the command (Landlock does not   |
//! |                                                           | see a file already open)                              |
//! | no gain of privilege (a setuid program)                   | no_new_privs, which Landlock and seccomp require      |
//!
//! "web" and "packages", the network a step's agent may reach, are Claude Code's own sandbox, not this box.
//!
//! A writable folder given as a symbolic link is confined where the link points when the box opens: the folder the
//! caller names is the one written, and what the link later points at does not move the box. /tmp and the rest of
//! the machine are closed to writing, as on macOS: a command that makes temporary files needs TMPDIR inside one of
//! its folders, which the caller sets (jaynes_box.environment does).
//!
//! What the box does not hold, on Linux as on macOS: the machine's resources (a command may fill the disk of its
//! folders, take the memory or start processes until the system says no; the caller's deadline stops it), the time
//! and the processor it takes, and side channels such as the timing of other processes. The environment reaches the
//! command as the caller gives it.
//!
//! The kernel must have Landlock ABI 6 (Linux 6.12): below it, or with Landlock off, the box refuses.

use std::path::{Path, PathBuf};

/// What a box lets its command do, as the macOS box takes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Spec {
    /// Folders it may write, and read even inside hidden ones.
    pub writable: Vec<PathBuf>,
    /// Folders it may not read, apart from its writable folders inside them.
    pub hidden: Vec<PathBuf>,
    /// Local ports it may connect to.
    pub ports: Vec<u16>,
    /// Any local port: a test suite's own servers and databases; never the internet.
    pub local: bool,
}

/// The paths a box may not read, as they are on disk. What a box hides is one list, kept where both systems' boxes
/// take it from (`hidden_paths` in jaynes_box.py: where secrets live, what the engine keeps, other runs); the
/// caller passes each path.
pub fn denied(spec: &Spec) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = spec.hidden.iter().filter_map(|p| std::fs::canonicalize(p).ok()).collect();
    out.sort();
    out.dedup();
    out
}

/// The paths to grant reading to, so that everything is readable but `denied`: a path with nothing denied beneath
/// it is granted whole; one with something denied beneath is walked into, and its symbolic links are left out,
/// since what they point at is granted, or not, where it is.
pub fn readable(root: &Path, denied: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, denied, &mut out);
    out
}

fn walk(path: &Path, denied: &[PathBuf], out: &mut Vec<PathBuf>) {
    if denied.iter().any(|d| d == path) {
        return;
    }
    if !denied.iter().any(|d| d.starts_with(path)) {
        out.push(path.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if e.file_type().is_ok_and(|t| t.is_symlink()) {
            continue;
        }
        walk(&p, denied, out);
    }
}

/// Why the box cannot open on this machine, or None when it can.
pub fn unavailable() -> Option<String> {
    imp::unavailable()
}

/// How the box holds Unix sockets on this machine: by their path (Landlock ABI 9), or by making none at all.
pub fn unix_sockets() -> &'static str {
    imp::unix_sockets()
}

/// Confine this process as `spec` says, then become `argv`. Returns only when it could not: the box never runs
/// the command unconfined.
pub fn run(spec: &Spec, argv: &[String]) -> String {
    if spec.local || !spec.ports.is_empty() {
        return "the Linux box has no network for local ports yet: it refuses rather than open the network".into();
    }
    if argv.is_empty() {
        return "the box needs a command to run".into();
    }
    imp::run(spec, argv)
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::Spec;

    pub fn unavailable() -> Option<String> {
        Some("`jaynes box` is the box of Linux; on macOS the box is sandbox-exec".into())
    }

    pub fn run(_spec: &Spec, _argv: &[String]) -> String {
        unavailable().unwrap_or_default()
    }

    pub fn unix_sockets() -> &'static str {
        "no Linux box here"
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{denied, readable, Spec};
    use landlock::{
        Access, AccessFs, AccessNet, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
        RulesetStatus, Scope, ABI,
    };
    use seccompiler::{BpfProgram, SeccompAction, SeccompCmpArgLen as Len, SeccompCmpOp as Op, SeccompCondition as Cond, SeccompFilter, SeccompRule, TargetArch};
    use std::collections::BTreeMap;
    use std::os::unix::process::CommandExt;
    use std::path::Path;

    pub fn unavailable() -> Option<String> {
        // the box is tried for real: a child opens one around `true`
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => return Some(format!("the program cannot find itself to try the box: {e}")),
        };
        let tmp = std::env::temp_dir();
        match std::process::Command::new(exe).arg("box").arg("--write").arg(&tmp).args(["--", "/bin/sh", "-c", "true"]).output() {
            Ok(o) if o.status.success() => None,
            Ok(o) => Some(String::from_utf8_lossy(&o.stderr).trim().trim_start_matches("error: ").to_string()),
            Err(e) => Some(format!("the box could not be tried: {e}")),
        }
    }

    pub fn run(spec: &Spec, argv: &[String]) -> String {
        if let Some(open) = handed_open() {
            return open;
        }
        match confine(spec) {
            Ok(()) => format!("the command did not start in the box: {}", std::process::Command::new(&argv[0]).args(&argv[1..]).exec()),
            Err(e) => e,
        }
    }

    /// The files this process was handed open that would stay open into the command: anything but 0, 1 and 2 not
    /// marked to close on exec. Landlock does not see a file already open, so the box refuses rather than run with
    /// one: a caller passes only the standard three, as Python's subprocess does.
    fn handed_open() -> Option<String> {
        const CLOSE_ON_EXEC: u32 = 0o2_000_000;
        let Ok(entries) = std::fs::read_dir("/proc/self/fdinfo") else {
            return Some("the box cannot read /proc/self/fdinfo to see what files it was handed open".into());
        };
        let mut open = Vec::new();
        for e in entries.flatten() {
            let Ok(fd) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
            if fd <= 2 {
                continue;
            }
            let info = std::fs::read_to_string(e.path()).unwrap_or_default();
            let flags = info.lines().find_map(|l| l.strip_prefix("flags:")).and_then(|f| u32::from_str_radix(f.trim(), 8).ok());
            // a file gone between the listing and the reading, as the listing's own, is not open
            if flags.is_some_and(|f| f & CLOSE_ON_EXEC == 0) {
                open.push(fd.to_string());
            }
        }
        (!open.is_empty()).then(|| format!("the box was handed open files ({}) that would reach the command past its rules; it refuses", open.join(", ")))
    }

    pub fn unix_sockets() -> &'static str {
        if ruleset(ABI::V9).is_ok() { "Unix sockets only in its folders, by their path (Landlock ABI 9)" } else { "no Unix socket at all (Landlock below ABI 9)" }
    }

    /// The rights the box handles at `abi`, every one of them required: a kernel that lacks one refuses the
    /// whole ruleset instead of dropping it.
    fn ruleset(abi: ABI) -> Result<landlock::RulesetCreated, landlock::RulesetError> {
        Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(AccessFs::from_all(abi))?
            .handle_access(AccessNet::BindTcp | AccessNet::ConnectTcp)?
            .scope(Scope::AbstractUnixSocket | Scope::Signal)?
            .create()
    }

    fn confine(spec: &Spec) -> Result<(), String> {
        let fail = |e: &dyn std::fmt::Display| format!("the box could not be made: {e}");
        // Landlock ABI 9 checks a Unix socket's path; ABI 6 is the least the box opens with, since it keeps signals
        // and abstract sockets in the box (Linux 6.12)
        let (abi, unix_by_path, mut ruleset) = match ruleset(ABI::V9) {
            Ok(r) => (ABI::V9, true, r),
            Err(_) => match ruleset(ABI::V6) {
                Ok(r) => (ABI::V6, false, r),
                Err(e) => return Err(format!("this kernel's Landlock cannot hold the box, which needs Landlock ABI 6 (Linux 6.12) or later: {e}")),
            },
        };
        let all = AccessFs::from_all(abi);
        let reads = AccessFs::from_read(abi);
        let hidden = denied(spec);
        for p in readable(Path::new("/"), &hidden) {
            // a file is granted what a file can be granted; a folder all the reading rights
            let rights = if p.is_dir() { reads } else { reads & AccessFs::from_file(abi) };
            let Ok(fd) = PathFd::new(&p) else { continue };
            ruleset = ruleset.add_rule(PathBeneath::new(fd, rights)).map_err(|e| fail(&e))?;
        }
        // the terminal and the null devices a command writes to, as the macOS box lets it
        for dev in ["/dev/null", "/dev/zero", "/dev/tty", "/dev/ptmx", "/dev/pts"] {
            if let Ok(fd) = PathFd::new(dev) {
                let rights = if Path::new(dev).is_dir() { all } else { all & AccessFs::from_file(abi) };
                ruleset = ruleset.add_rule(PathBeneath::new(fd, rights)).map_err(|e| fail(&e))?;
            }
        }
        for w in &spec.writable {
            let fd = PathFd::new(w).map_err(|e| format!("the box's folder {} cannot be opened: {e}", w.display()))?;
            ruleset = ruleset.add_rule(PathBeneath::new(fd, all)).map_err(|e| fail(&e))?;
        }
        let status = ruleset.restrict_self().map_err(|e| fail(&e))?;
        if status.ruleset != RulesetStatus::FullyEnforced || !status.no_new_privs {
            return Err(format!("the kernel did not enforce the whole box ({:?})", status.ruleset));
        }
        filter(unix_by_path)
    }

    /// The old kind of packet socket, which libc names only as deprecated.
    const SOCK_PACKET: i32 = 10;

    /// The seccomp filter: the calls and the kinds of socket Landlock does not see are refused with EPERM.
    fn filter(unix_by_path: bool) -> Result<(), String> {
        let program = program(unix_by_path)?;
        seccompiler::apply_filter(&program).map_err(|e| format!("the box's filter could not be set: {e}"))
    }

    pub(super) fn program(unix_by_path: bool) -> Result<BpfProgram, String> {
        let fail = |e: &dyn std::fmt::Display| format!("the box's filter could not be made: {e}");
        let c = |arg: u8, op: Op, v: u64| Cond::new(arg, Len::Dword, op, v).map_err(|e| fail(&e));
        let r = |conds: Vec<Cond>| SeccompRule::new(conds).map_err(|e| fail(&e));
        let (unix, inet, inet6) = (libc::AF_UNIX as u64, libc::AF_INET as u64, libc::AF_INET6 as u64);
        let mut socket = vec![r(vec![c(0, Op::Ne, unix)?, c(0, Op::Ne, inet)?, c(0, Op::Ne, inet6)?])?];
        for domain in [inet, inet6] {
            for kind in [libc::SOCK_DGRAM, libc::SOCK_RAW, libc::SOCK_RDM, libc::SOCK_SEQPACKET, libc::SOCK_DCCP, SOCK_PACKET] {
                socket.push(r(vec![c(0, Op::Eq, domain)?, c(1, Op::MaskedEq(0xf), kind as u64)?])?);
            }
            // a stream that is not TCP, such as SCTP or MPTCP, which Landlock's TCP rules may not hold
            socket.push(r(vec![c(0, Op::Eq, domain)?, c(1, Op::MaskedEq(0xf), libc::SOCK_STREAM as u64)?, c(2, Op::Ne, 0)?, c(2, Op::Ne, libc::IPPROTO_TCP as u64)?])?);
        }
        if !unix_by_path {
            // below Landlock ABI 9 a Unix socket's path is not checked: none is made at all
            socket.push(r(vec![c(0, Op::Eq, unix)?])?);
        }
        let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
        rules.insert(libc::SYS_socket, socket);
        rules.insert(libc::SYS_socketpair, vec![r(vec![c(0, Op::Ne, unix)?])?]);
        for call in [
            libc::SYS_io_uring_setup, libc::SYS_io_uring_enter, libc::SYS_io_uring_register, libc::SYS_bpf, libc::SYS_perf_event_open,
            libc::SYS_keyctl, libc::SYS_add_key, libc::SYS_request_key, libc::SYS_unshare, libc::SYS_setns, libc::SYS_mount,
            libc::SYS_umount2, libc::SYS_pivot_root, libc::SYS_open_tree, libc::SYS_move_mount, libc::SYS_fsopen, libc::SYS_fsmount,
        ] {
            rules.insert(call, vec![]);
        }
        // On x86_64 the x32 calls pass the check of architecture with their number plus this bit: each refused
        // call is refused under that number too. A call of a 32-bit architecture, socketcall among them, fails
        // the check of architecture itself and ends the process.
        if cfg!(target_arch = "x86_64") {
            const X32: i64 = 0x4000_0000;
            let x32: Vec<(i64, Vec<SeccompRule>)> = rules.iter().map(|(n, r)| (n | X32, r.clone())).collect();
            rules.extend(x32);
        }
        let arch = TargetArch::try_from(std::env::consts::ARCH).map_err(|e| fail(&e))?;
        let filter = SeccompFilter::new(rules, SeccompAction::Allow, SeccompAction::Errno(libc::EPERM as u32), arch).map_err(|e| fail(&e))?;
        filter.try_into().map_err(|e: seccompiler::BackendError| fail(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn the_filter_ends_a_call_of_another_architecture_before_anything_else() {
        // the first three instructions: load the call's architecture, go on if it is this one, else end the
        // process; so a 32-bit call, socketcall among them, never reaches the rules
        let p = imp::program(false).expect("program");
        assert_eq!(p[0].k, 4, "the architecture is read first (its offset in seccomp_data)");
        assert_eq!(p[2].k, libc::SECCOMP_RET_KILL_PROCESS);
    }

    #[test]
    fn everything_is_readable_but_what_is_denied_and_links_are_not_followed_into_it() {
        let base = std::env::temp_dir().join(format!("jaynes-sandbox-walk-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        for d in ["home/.ssh", "home/.config/gh", "home/.config/other", "home/code"] {
            std::fs::create_dir_all(base.join(d)).expect("dir");
        }
        std::fs::write(base.join("home/notes"), "ok").expect("write");
        std::os::unix::fs::symlink(base.join("home/.ssh"), base.join("home/code/keys")).expect("link");
        let base = std::fs::canonicalize(&base).expect("real");
        let spec = Spec { hidden: vec![base.join("home/.ssh"), base.join("home/.config/gh"), base.join("home/.aws")], ..Default::default() };
        let denied = denied(&spec);
        assert_eq!(denied, vec![base.join("home/.config/gh"), base.join("home/.ssh")]);
        let mut got = readable(&base, &denied);
        got.sort();
        // the code folder is whole, with its link inside it: the link resolves to .ssh, which is not granted
        assert_eq!(got, vec![base.join("home/.config/other"), base.join("home/code"), base.join("home/notes")]);
    }
}
