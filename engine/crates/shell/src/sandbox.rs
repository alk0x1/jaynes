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
//! | reads what it is shown inside the hidden (the run's copy) | Landlock: read rights beneath each, besides the rest  |
//! | no network but local ports                                | where the machine allows, a network of the box's own: |
//! |                                                           | a user and network namespace with only its loopback,  |
//! |                                                           | any port there; elsewhere Landlock's port rules for   |
//! |                                                           | the listed ports only, and only where this machine's  |
//! |                                                           | network has no way out (they hold a port, not an      |
//! |                                                           | address); else the box refuses                        |
//! | never the Jaynes page's port                              | not in the box's own network; elsewhere never listed, |
//! |                                                           | and closed even when the person opened any port       |
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
//! Where there is no network of the box's own and the person opened any local port for the project, the loopback
//! the command reaches is the one of the whole machine or container it runs in: every service listening on it there,
//! but the Jaynes page's port, is within reach of the command.
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
    /// Folders it may read even inside hidden ones, as the run's own copy inside the hidden runs.
    pub show: Vec<PathBuf>,
    /// Local ports it may bind and connect to.
    pub ports: Vec<u16>,
    /// It runs local servers, as a test suite does: it gets a network of its own where the machine allows one,
    /// with any local port; elsewhere only `ports`, and only where this machine's network has no way out.
    pub local: bool,
    /// The person opened any local port for this project: where there is no network of the box's own, every
    /// port but `never`.
    pub any_port: bool,
    /// Ports never opened, as the Jaynes page's.
    pub never: Vec<u16>,
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

/// How this machine's network could reach past it, or None when it has no way out: an interface that is not the
/// loopback. Read in /sys/class/net, where a container with no network (--network none) shows only lo.
pub fn way_out() -> Option<String> {
    way_out_in(Path::new("/sys/class/net"))
}

fn way_out_in(interfaces: &Path) -> Option<String> {
    let Ok(entries) = std::fs::read_dir(interfaces) else {
        return Some(format!("its network interfaces cannot be read in {}", interfaces.display()));
    };
    let mut out: Vec<String> = entries.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n != "lo").collect();
    out.sort();
    (!out.is_empty()).then(|| format!("this machine's network has a way out ({})", out.join(", ")))
}

/// How the box holds Unix sockets on this machine: by their path (Landlock ABI 9), or by making none at all.
pub fn unix_sockets() -> &'static str {
    imp::unix_sockets()
}

/// What a command that runs local servers gets on this machine: a network of its own, its listed ports only, or a
/// refusal, and why.
pub fn local_network() -> String {
    imp::local_network()
}

/// Confine this process as `spec` says, then become `argv`. Returns only when it could not: the box never runs
/// the command unconfined.
pub fn run(spec: &Spec, argv: &[String]) -> String {
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

    pub fn local_network() -> String {
        "no Linux box here".into()
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
            // the command is told which network it got, for itself and for tests; it opens nothing
            Ok(net) => format!(
                "the command did not start in the box: {}",
                std::process::Command::new(&argv[0]).args(&argv[1..]).env("JAYNES_BOX_NETWORK", net.name()).exec()
            ),
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

    /// Which network a command that runs local servers gets here: tried for real in a child box.
    pub fn local_network() -> String {
        let Ok(exe) = std::env::current_exe() else { return "unknown".into() };
        let tmp = std::env::temp_dir();
        match std::process::Command::new(exe).arg("box").arg("--write").arg(&tmp).args(["--local", "--", "/bin/sh", "-c", "echo $JAYNES_BOX_NETWORK"]).output() {
            Ok(o) if o.status.success() => match String::from_utf8_lossy(&o.stdout).trim() {
                "own" => "local servers get a network of the box's own".into(),
                "ports" => "local servers get only their listed ports: no network of its own here, and no way out".into(),
                other => format!("local servers get {other}"),
            },
            Ok(o) => format!("local servers are refused here: {}", String::from_utf8_lossy(&o.stderr).trim().trim_start_matches("error: ")),
            Err(e) => format!("unknown: {e}"),
        }
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

    /// What the box lets its command do over TCP.
    #[derive(Debug, PartialEq)]
    enum Net {
        /// No TCP at all.
        Nothing,
        /// A network of its own, with only its loopback: any port.
        Own,
        /// Only these ports, bound and reached.
        Ports(Vec<u16>),
        /// Every port but these.
        AllBut(Vec<u16>),
    }

    impl Net {
        fn name(&self) -> &'static str {
            match self {
                Net::Nothing => "none",
                Net::Own => "own",
                Net::Ports(_) => "ports",
                Net::AllBut(_) => "all-but",
            }
        }
    }

    /// The network the box gives `spec`, or why it refuses: it never opens a port where this machine's network has
    /// a way out, since Landlock holds a port and not an address.
    fn network(spec: &Spec) -> Result<Net, String> {
        if !spec.local && !spec.any_port && spec.ports.is_empty() {
            return Ok(Net::Nothing);
        }
        if spec.any_port && !spec.local {
            return Err("any local port is for a command that runs local servers (--local)".into());
        }
        let own = if spec.local {
            match own_network() {
                Ok(()) => return Ok(Net::Own),
                Err(e) => Some(e),
            }
        } else {
            None
        };
        if let Some(out) = super::way_out() {
            let why = own.map(|e| format!(" A network of its own could not be made: {e}.")).unwrap_or_default();
            return Err(format!(
                "the box refuses to open local ports here: {out}, and its rules hold a port, not an address, so a port \
                 opened here would reach other machines too.{why} Run Jaynes where a process may make a network of its \
                 own (unprivileged user namespaces allowed), or in a container with no network (--network none)"
            ));
        }
        let never = spec.never.clone();
        if spec.any_port {
            return Ok(Net::AllBut(never));
        }
        let mut ports: Vec<u16> = spec.ports.iter().copied().filter(|p| !never.contains(p)).collect();
        ports.sort_unstable();
        ports.dedup();
        Ok(Net::Ports(ports))
    }

    /// A network of the box's own: a new user namespace, which a process may make without privilege where the
    /// machine allows it, holding a new network namespace whose only interface is its loopback, brought up here.
    /// The box's user and group stay what they are, so the command does not see itself as root.
    pub(super) fn own_network() -> Result<(), String> {
        let threads = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| s.lines().find_map(|l| l.strip_prefix("Threads:").and_then(|n| n.trim().parse::<u32>().ok())));
        if threads != Some(1) {
            let n = threads.map(|n| n.to_string()).unwrap_or_else(|| "an unknown number of".into());
            return Err(format!("the box makes a network of its own only from a process with one thread, and this one has {n} threads"));
        }
        let (uid, gid) = (rustix::process::getuid().as_raw(), rustix::process::getgid().as_raw());
        new_user_and_network()?;
        let write = |f: &str, t: String| std::fs::write(f, t).map_err(|e| format!("its user could not be kept ({f}): {e}"));
        write("/proc/self/setgroups", "deny".into())?;
        write("/proc/self/uid_map", format!("{uid} {uid} 1"))?;
        write("/proc/self/gid_map", format!("{gid} {gid} 1"))?;
        loopback_up()
    }

    /// unshare(CLONE_NEWUSER | CLONE_NEWNET), and nothing else. rustix marks its safe unshare deprecated because
    /// with CLONE_FILES a thread may lose descriptors another one opened; new user and network namespaces share no
    /// descriptors with anyone, the flags are fixed here so no caller passes FILES, and own_network calls it only
    /// from a process of one thread.
    #[allow(deprecated)]
    fn new_user_and_network() -> Result<(), String> {
        use rustix::thread::UnshareFlags;
        rustix::thread::unshare(UnshareFlags::NEWUSER | UnshareFlags::NEWNET)
            .map_err(|e| format!("this machine does not let a process make a network of its own: {e}"))
    }

    /// Bring up the loopback of the new network namespace, by a netlink message to the kernel: RTM_NEWLINK on
    /// interface 1, the loopback in a namespace just made, setting IFF_UP. No program is needed for it.
    fn loopback_up() -> Result<(), String> {
        use rustix::net::{recv, sendto, AddressFamily, RecvFlags, SendFlags, SocketType, netlink::SocketAddrNetlink};
        let fail = |e: &dyn std::fmt::Display| format!("its loopback could not be brought up: {e}");
        let fd = rustix::net::socket(AddressFamily::NETLINK, SocketType::RAW, None).map_err(|e| fail(&e))?;
        const RTM_NEWLINK: u16 = 16;
        const NLM_F_REQUEST_ACK: u16 = 1 | 4;
        const IFF_UP: u32 = 1;
        let mut m: Vec<u8> = Vec::with_capacity(32);
        m.extend(32u32.to_ne_bytes());
        m.extend(RTM_NEWLINK.to_ne_bytes());
        m.extend(NLM_F_REQUEST_ACK.to_ne_bytes());
        m.extend(1u32.to_ne_bytes()); // sequence
        m.extend(0u32.to_ne_bytes()); // to the kernel
        m.extend([0u8, 0u8]); // any family, padding
        m.extend(0u16.to_ne_bytes()); // device type
        m.extend(1i32.to_ne_bytes()); // interface 1: the loopback
        m.extend(IFF_UP.to_ne_bytes());
        m.extend(IFF_UP.to_ne_bytes()); // what changes
        sendto(&fd, &m, SendFlags::empty(), &SocketAddrNetlink::new(0, 0)).map_err(|e| fail(&e))?;
        let mut buf = [0u8; 512];
        let (n, _) = recv(&fd, &mut buf[..], RecvFlags::empty()).map_err(|e| fail(&e))?;
        let said = &buf[..n.min(buf.len())];
        // the kernel's answer: an NLMSG_ERROR whose error is 0 for success
        const NLMSG_ERROR: u16 = 2;
        let kind = said.get(4..6).map(|b| u16::from_ne_bytes([b[0], b[1]]));
        let error = said.get(16..20).map(|b| i32::from_ne_bytes([b[0], b[1], b[2], b[3]]));
        match (kind, error) {
            (Some(NLMSG_ERROR), Some(0)) => Ok(()),
            (Some(NLMSG_ERROR), Some(e)) => Err(fail(&format!("the kernel said error {}", -e))),
            _ => Err(fail(&"the kernel's answer did not read")),
        }
    }

    fn confine(spec: &Spec) -> Result<Net, String> {
        // the network first: a network of its own must be made before Landlock and the filter close the calls
        let net = network(spec)?;
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
        // what is read again inside the hidden: Landlock only adds, so these are granted besides
        for p in &spec.show {
            let Ok(fd) = PathFd::new(p) else { continue };
            let rights = if p.is_dir() { reads } else { reads & AccessFs::from_file(abi) };
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
        let tcp = AccessNet::BindTcp | AccessNet::ConnectTcp;
        let open: Vec<u16> = match &net {
            Net::Nothing => Vec::new(),
            // only its own loopback is there: every port of it
            Net::Own => (1..=u16::MAX).collect(),
            Net::Ports(ports) => ports.clone(),
            Net::AllBut(never) => (1..=u16::MAX).filter(|p| !never.contains(p)).collect(),
        };
        for port in open {
            ruleset = ruleset.add_rule(landlock::NetPort::new(port, tcp)).map_err(|e| fail(&e))?;
        }
        let status = ruleset.restrict_self().map_err(|e| fail(&e))?;
        if status.ruleset != RulesetStatus::FullyEnforced || !status.no_new_privs {
            return Err(format!("the kernel did not enforce the whole box ({:?})", status.ruleset));
        }
        filter(unix_by_path)?;
        Ok(net)
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

    #[test]
    fn a_network_has_a_way_out_when_it_has_an_interface_but_the_loopback() {
        let base = std::env::temp_dir().join(format!("jaynes-net-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("lo")).expect("dir");
        assert_eq!(way_out_in(&base), None, "only the loopback: no way out");
        std::fs::create_dir_all(base.join("eth0")).expect("dir");
        assert!(way_out_in(&base).is_some_and(|w| w.contains("eth0")));
        assert!(way_out_in(&base.join("missing")).is_some(), "what cannot be read is a way out, never an opening");
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_network_of_its_own_is_made_only_from_a_process_of_one_thread() {
        // the test harness runs more than one thread: the box says so in its own words, and makes nothing
        let said = imp::own_network().expect_err("refused");
        assert!(said.contains("only from a process with one thread"), "{said}");
    }

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
