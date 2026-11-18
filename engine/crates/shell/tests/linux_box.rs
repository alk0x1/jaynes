//! The Linux box, tried from outside: each promise of the table in src/sandbox.rs, and each way out a command in the
//! box could try. A test fails when the box lets something through, and when the box cannot open on this machine:
//! a kernel without the Landlock it needs is a machine these tests are not for.
#![cfg(target_os = "linux")]

use std::os::linux::net::SocketAddrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

struct Place {
    base: PathBuf,
    /// The box's own folder.
    work: PathBuf,
    /// A home with secrets and a file anyone may read.
    home: PathBuf,
    /// A folder outside the box.
    outside: PathBuf,
}

fn place(name: &str) -> Place {
    let base = std::env::temp_dir().join(format!("jaynes-linux-box-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let (work, home, outside) = (base.join("work"), base.join("home"), base.join("outside"));
    for d in [&work, &outside, &home.join(".ssh"), &home.join(".config/gh")] {
        std::fs::create_dir_all(d).expect("dir");
    }
    std::fs::write(home.join(".ssh/id_ed25519"), "the key\n").expect("write");
    std::fs::write(home.join(".config/gh/hosts.yml"), "token: t\n").expect("write");
    std::fs::write(home.join("notes"), "readable\n").expect("write");
    std::fs::write(outside.join("file"), "outside\n").expect("write");
    let real = |p: &Path| std::fs::canonicalize(p).expect("real");
    Place { base: real(&base), work: real(&work), home: real(&home), outside: real(&outside) }
}

/// What the caller hides, as jaynes_box.hidden_paths gives it: here the secrets of the test's home.
fn hide(p: &Place) -> Vec<String> {
    [".ssh", ".config/gh"].iter().flat_map(|s| ["--hide".to_string(), p.home.join(s).display().to_string()]).collect()
}

/// Run `script` with sh in a box that writes only `p.work` and hides the secrets of `p.home`.
fn boxed(p: &Place, script: &str, extra: &[&str]) -> Output {
    let mut c = Command::new(BIN);
    c.arg("box").arg("--write").arg(&p.work).args(hide(p)).args(extra).arg("--").args(["sh", "-c", script]);
    c.current_dir(&p.work).stdin(Stdio::null());
    c.output().expect("box")
}

fn said(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

/// The box opens here; else every test says why, instead of passing over a machine with no box.
fn opens() {
    let o = Command::new(BIN).args(["box", "--check"]).output().expect("check");
    assert!(o.status.success(), "the box does not open on this machine, so these tests cannot hold: {}", said(&o));
    // which way of holding Unix sockets these tests ran in, for whoever reads the output
    eprintln!("{}", said(&o).trim());
}

/// `script` succeeds in the box only if it got through; the box must make it fail.
fn refused(p: &Place, what: &str, script: &str) {
    opens();
    let o = boxed(p, script, &[]);
    assert!(!o.status.success(), "the box let through {what}: {}", said(&o));
}

#[test]
fn a_command_runs_reads_the_system_and_writes_its_own_folder() {
    let p = place("use");
    opens();
    let o = boxed(&p, &format!("echo hi > {w}/made && cat {w}/made && cat /etc/hostname >/dev/null && cat {h}/notes && ls /usr/bin >/dev/null", w = p.work.display(), h = p.home.display()), &[]);
    assert!(o.status.success(), "{}", said(&o));
    assert!(said(&o).contains("hi") && said(&o).contains("readable"), "{}", said(&o));
}

#[test]
fn a_write_outside_its_folder_is_refused() {
    let p = place("write");
    refused(&p, "a write outside", &format!("echo x > {}/new", p.outside.display()));
    refused(&p, "a write over a file outside", &format!("echo x > {}/file", p.outside.display()));
    refused(&p, "a write in the home", &format!("echo x > {}/new", p.home.display()));
    refused(&p, "a write in /tmp", "echo x > /tmp/jaynes-escaped-the-box");
    assert_eq!(std::fs::read_to_string(p.outside.join("file")).expect("read"), "outside\n");
}

#[test]
fn a_write_cannot_leave_by_a_symbolic_link() {
    let p = place("symlink");
    // made inside the box, and made before it opened
    refused(&p, "a write through a link it made", &format!("ln -s {o}/file {w}/out && echo x > {w}/out", o = p.outside.display(), w = p.work.display()));
    std::os::unix::fs::symlink(p.outside.join("file"), p.work.join("there")).expect("link");
    refused(&p, "a write through a link it found", &format!("echo x > {}/there", p.work.display()));
    std::os::unix::fs::symlink(&p.outside, p.work.join("dir")).expect("link");
    refused(&p, "a new file through a linked folder", &format!("echo x > {}/dir/new", p.work.display()));
    assert_eq!(std::fs::read_to_string(p.outside.join("file")).expect("read"), "outside\n");
    assert!(!p.outside.join("new").exists());
}

#[test]
fn a_write_cannot_leave_by_a_hard_link_or_a_rename() {
    let p = place("links");
    refused(&p, "a hard link to a file outside", &format!("ln {o}/file {w}/hard && echo x > {w}/hard", o = p.outside.display(), w = p.work.display()));
    refused(&p, "a rename out of the box", &format!("echo x > {w}/mine && mv {w}/mine {o}/mine", o = p.outside.display(), w = p.work.display()));
    refused(&p, "a rename of a file outside into the box", &format!("mv {o}/file {w}/taken", o = p.outside.display(), w = p.work.display()));
    assert_eq!(std::fs::read_to_string(p.outside.join("file")).expect("read"), "outside\n");
    assert!(!p.outside.join("mine").exists());
}

#[test]
fn secrets_and_hidden_folders_are_not_read() {
    let p = place("read");
    refused(&p, "a key in ~/.ssh", &format!("cat {}/.ssh/id_ed25519", p.home.display()));
    refused(&p, "a listing of ~/.ssh", &format!("ls {}/.ssh", p.home.display()));
    refused(&p, "a token in ~/.config/gh", &format!("cat {}/.config/gh/hosts.yml", p.home.display()));
    refused(&p, "a key through a link to it", &format!("ln -s {h}/.ssh {w}/keys; cat {w}/keys/id_ed25519", h = p.home.display(), w = p.work.display()));
    opens();
    let hidden = p.base.join("hidden");
    std::fs::create_dir_all(&hidden).expect("dir");
    std::fs::write(hidden.join("original"), "the person's own\n").expect("write");
    let o = boxed(&p, &format!("cat {}/original", hidden.display()), &["--hide", hidden.to_str().expect("path")]);
    assert!(!o.status.success(), "the box read a hidden folder: {}", said(&o));
}

#[test]
fn proc_does_not_reach_outside() {
    let p = place("proc");
    let mut outside = Command::new("sleep").arg("30").env("JAYNES_TEST_SECRET", "s3cret").spawn().expect("sleep");
    let pid = outside.id();
    refused(&p, "the environment of a process outside", &format!("grep -a s3cret /proc/{pid}/environ"));
    refused(&p, "the memory of a process outside", &format!("head -c 1 /proc/{pid}/mem"));
    refused(&p, "a write through the root of a process outside", &format!("echo x > /proc/{pid}/root{}/new", p.outside.display()));
    refused(&p, "a secret through its own root", &format!("cat /proc/self/root{}/.ssh/id_ed25519", p.home.display()));
    let _killed = outside.kill();
    let _waited = outside.wait();
    assert!(!p.outside.join("new").exists());
}

#[test]
fn signals_and_ptrace_do_not_reach_outside() {
    let p = place("signal");
    let mut outside = Command::new("sleep").arg("30").spawn().expect("sleep");
    let pid = outside.id();
    refused(&p, "a signal to a process outside", &format!("kill -TERM {pid}"));
    refused(&p, "ptrace of a process outside", &format!(
        "python3 -c 'import ctypes,sys; l=ctypes.CDLL(None,use_errno=True); sys.exit(0 if l.ptrace(16,{pid},0,0)==0 else 1)'"
    ));
    assert!(outside.try_wait().expect("wait").is_none(), "the process outside is still running");
    let _killed = outside.kill();
    let _waited = outside.wait();
    // inside the box, its own processes take its signals
    opens();
    let o = boxed(&p, "sleep 30 & kill -TERM $! || exit 1; wait $!; [ $? -eq 143 ]", &[]);
    assert!(o.status.success(), "a signal inside the box: {}", said(&o));
}

#[test]
fn sockets_other_than_unix_and_tcp_are_not_made_and_tcp_reaches_nothing() {
    let p = place("net");
    let py = |body: &str| format!("python3 -c 'import socket,sys\ntry:\n    {body}\nexcept OSError as e:\n    print(e); sys.exit(1)'");
    refused(&p, "a UDP socket", &py("socket.socket(socket.AF_INET, socket.SOCK_DGRAM).sendto(b\"x\", (\"1.1.1.1\", 53))"));
    refused(&p, "a UDP socket over IPv6", &py("socket.socket(socket.AF_INET6, socket.SOCK_DGRAM)"));
    refused(&p, "a raw socket", &py("socket.socket(socket.AF_INET, socket.SOCK_RAW, socket.IPPROTO_ICMP)"));
    refused(&p, "a netlink socket", &py("socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 0)"));
    refused(&p, "a packet socket", &py("socket.socket(socket.AF_PACKET, socket.SOCK_RAW, 0)"));
    refused(&p, "an SCTP socket", &py("socket.socket(socket.AF_INET, socket.SOCK_STREAM, 132)"));
    let server = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = server.local_addr().expect("addr").port();
    refused(&p, "a TCP connection to a local port", &py(&format!("socket.create_connection((\"127.0.0.1\", {port}), timeout=2)")));
    refused(&p, "a TCP connection to the internet", &py("socket.create_connection((\"1.1.1.1\", 443), timeout=2)"));
    refused(&p, "a TCP server", &py("socket.socket().bind((\"127.0.0.1\", 0))"));
    // the same socket by its x32 number: on x86_64 the filter refuses it, elsewhere the number is no call at all
    refused(&p, "a UDP socket by the x32 number of socket", "python3 -c 'import ctypes,sys; l=ctypes.CDLL(None,use_errno=True); sys.exit(0 if l.syscall(0x40000000 | 41, 2, 2, 0) >= 0 else 1)'");
    refused(&p, "io_uring, which would pass the filter", "python3 -c 'import ctypes,sys; l=ctypes.CDLL(None,use_errno=True); p=ctypes.create_string_buffer(120); sys.exit(0 if l.syscall(425,1,p)>=0 else 1)'");
}

#[test]
fn unix_sockets_outside_the_box_are_not_reached() {
    let p = place("unix");
    let path = p.outside.join("agent.sock");
    let _server = std::os::unix::net::UnixListener::bind(&path).expect("listen");
    refused(&p, "a Unix socket outside, as the Docker or ssh-agent one", &format!(
        "python3 -c 'import socket; socket.socket(socket.AF_UNIX).connect(\"{}\")'", path.display()
    ));
    let name = format!("jaynes-test-{}", std::process::id());
    let abstract_server = std::os::unix::net::UnixListener::bind_addr(&std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).expect("name")).expect("listen");
    refused(&p, "an abstract Unix socket outside", &format!(
        "python3 -c 'import socket; socket.socket(socket.AF_UNIX).connect(\"\\0{name}\")'"
    ));
    drop(abstract_server);
}

#[test]
fn a_box_asked_for_local_ports_refuses_until_it_has_a_network_of_its_own() {
    let p = place("ports");
    opens();
    let o = boxed(&p, "echo ran", &["--local"]);
    assert!(!o.status.success() && !said(&o).contains("ran"), "{}", said(&o));
    assert!(said(&o).contains("refuses"), "{}", said(&o));
}

#[test]
fn a_file_handed_open_outside_the_box_is_not_reached() {
    let p = place("fd");
    opens();
    // a careless parent holds a file outside the box open across exec: a shell that opens it as descriptor 7 and
    // becomes the box, so no other test's command inherits it
    let file = p.outside.join("file");
    for (what, script) in [
        ("a read through /proc/self/fd", "cat /proc/self/fd/7"),
        ("a write through /proc/self/fd", "echo x > /proc/self/fd/7"),
        ("a read of the descriptor itself", "cat <&7"),
        ("a write to the descriptor itself", "echo x >&7"),
    ] {
        let o = Command::new("sh")
            .arg("-c").arg(r#"exec 7<>"$1"; shift; exec "$@""#).arg("sh").arg(&file)
            .arg(BIN).arg("box").arg("--write").arg(&p.work).args(hide(&p)).arg("--").args(["sh", "-c", script])
            .output().expect("box");
        assert!(!o.status.success(), "the box let through {what}: {}", said(&o));
        assert!(said(&o).contains("handed open files (7)"), "the box refuses to open, and says why: {}", said(&o));
    }
    assert_eq!(std::fs::read_to_string(p.outside.join("file")).expect("read"), "outside\n");
    // with only the standard three, the box opens, and nothing else is open inside it but what it opens itself
    let o = boxed(&p, "ls /proc/self/fd", &[]);
    assert!(o.status.success(), "{}", said(&o));
    let fds: Vec<String> = String::from_utf8_lossy(&o.stdout).split_whitespace().map(str::to_string).collect();
    assert!(fds.iter().all(|f| ["0", "1", "2", "3"].contains(&f.as_str())), "only 0, 1, 2 and ls's own listing: {fds:?}");
}

#[test]
fn nothing_in_the_box_gains_privilege() {
    let p = place("privilege");
    opens();
    let o = boxed(&p, "grep -E '^(NoNewPrivs|Seccomp):' /proc/self/status", &[]);
    let status = said(&o);
    assert!(status.contains("NoNewPrivs:\t1"), "no_new_privs is on: {status}");
    assert!(status.contains("Seccomp:\t2"), "the seccomp filter is on: {status}");
    // a setuid program of the system runs, if at all, as the box's own user: su cannot become root
    if Path::new("/usr/bin/su").exists() {
        refused(&p, "a setuid program becoming root", "su root -c 'touch /root/jaynes-escaped' </dev/null");
    }
}

#[test]
fn a_writable_folder_given_as_a_link_is_confined_where_the_link_points() {
    let p = place("linked-folder");
    opens();
    let target = p.base.join("real");
    std::fs::create_dir_all(&target).expect("dir");
    let link = p.base.join("link");
    std::os::unix::fs::symlink(&target, &link).expect("link");
    let o = Command::new(BIN)
        .arg("box").arg("--write").arg(&link).args(hide(&p)).arg("--")
        .args(["sh", "-c", &format!("echo in > {}/made && echo out > {}/new", link.display(), p.outside.display())])
        .output().expect("box");
    assert!(std::fs::read_to_string(target.join("made")).is_ok_and(|t| t == "in\n"), "the folder the link names is written: {}", said(&o));
    assert!(!p.outside.join("new").exists() && !o.status.success(), "nothing else is: {}", said(&o));
}

#[test]
fn temporary_files_and_a_real_build_work_with_tmpdir_in_its_folder() {
    let p = place("tmpdir");
    opens();
    let tmp = p.work.join("tmp");
    std::fs::create_dir_all(&tmp).expect("dir");
    // /tmp is closed, as on macOS: a temporary file needs TMPDIR inside the box
    let o = Command::new(BIN).arg("box").arg("--write").arg(&p.work).args(hide(&p)).arg("--")
        .args(["sh", "-c", "TMPDIR=/tmp mktemp"]).env_remove("TMPDIR").output().expect("box");
    assert!(!o.status.success(), "/tmp is closed: {}", said(&o));
    let o = Command::new(BIN).arg("box").arg("--write").arg(&p.work).args(hide(&p)).arg("--")
        .args(["sh", "-c", "f=$(mktemp) && echo ok > \"$f\" && cat \"$f\""]).env("TMPDIR", &tmp).output().expect("box");
    assert!(o.status.success() && said(&o).contains("ok"), "mktemp in the box's TMPDIR: {}", said(&o));
    // a real cargo build, its caches in the box
    let found = Command::new("sh").args(["-c", "command -v cargo"]).output().expect("which");
    if found.stdout.is_empty() {
        return;
    }
    let script = "cargo new --quiet --vcs none hello && cd hello && cargo build --quiet --offline && \"$CARGO_TARGET_DIR/debug/hello\"";
    let o = Command::new(BIN).arg("box").arg("--write").arg(&p.work).args(hide(&p)).arg("--")
        .args(["sh", "-c", script]).current_dir(&p.work)
        .env("TMPDIR", &tmp).env("CARGO_HOME", p.work.join("cargo")).env("CARGO_TARGET_DIR", p.work.join("target"))
        .output().expect("box");
    assert!(o.status.success() && said(&o).contains("Hello, world!"), "cargo builds in the box: {}", said(&o));
}
