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
        // the box closes it before the command starts: the command finds no descriptor 7
        assert!(!o.status.success(), "the box let through {what}: {}", said(&o));
        assert!(!said(&o).contains("outside"), "the command reached the file outside by {what}: {}", said(&o));
    }
    assert_eq!(std::fs::read_to_string(p.outside.join("file")).expect("read"), "outside\n");
    // as a CI runner's shell hands it: two descriptors past 9, the box opens all the same and neither is inside
    let o = Command::new("bash")
        .arg("-c").arg(r#"exec 142<>"$1" 145<>"$1"; shift; exec "$@""#).arg("bash").arg(&file)
        .arg(BIN).arg("box").arg("--write").arg(&p.work).args(hide(&p)).arg("--").args(["sh", "-c", "ls /proc/self/fd"])
        .output().expect("box");
    assert!(o.status.success(), "the box opens though it was handed files open: {}", said(&o));
    let fds: Vec<String> = String::from_utf8_lossy(&o.stdout).split_whitespace().map(str::to_string).collect();
    assert!(fds.iter().all(|f| ["0", "1", "2", "3"].contains(&f.as_str())), "only 0, 1, 2 and ls's own listing: {fds:?}");
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

// --- the network --------------------------------------------------------------------------------------------

/// Which network a command that runs local servers gets on this machine: "own", "ports", or "refused". When the
/// one running the tests says which it expects (JAYNES_TEST_NET), the box must give that one, so a test never
/// passes in the wrong way.
fn local_network(p: &Place) -> String {
    opens();
    let o = boxed(p, "echo $JAYNES_BOX_NETWORK", &["--local"]);
    let got = if o.status.success() { String::from_utf8_lossy(&o.stdout).trim().to_string() } else { "refused".to_string() };
    if let Ok(want) = std::env::var("JAYNES_TEST_NET") {
        assert_eq!(got, want, "the box gave another network than this machine should: {}", said(&o));
    }
    eprintln!("local servers here: {got}");
    got
}

/// A Python one-liner that serves on `port` and reaches it: exit 0 when both worked.
fn serve_and_reach(port: u16) -> String {
    format!("python3 -c 'import socket,sys\ns=socket.socket(); s.bind((\"127.0.0.1\",{port})); s.listen(1)\nc=socket.create_connection((\"127.0.0.1\",{port}),timeout=3); print(\"served and reached\")'")
}

fn reach(host: &str, port: u16) -> String {
    format!("python3 -c 'import socket; socket.create_connection((\"{host}\",{port}),timeout=3); print(\"reached\")'")
}

#[test]
fn a_command_that_runs_local_servers_serves_them_and_reaches_nothing_outside() {
    let p = place("local");
    let net = local_network(&p);
    let port = 20000 + (std::process::id() % 20000) as u16;
    let o = boxed(&p, &serve_and_reach(port), &["--local", "--port", &port.to_string()]);
    if net == "refused" {
        assert!(!o.status.success() && said(&o).contains("refuses to open local ports"), "it says why it refuses: {}", said(&o));
        return;
    }
    assert!(o.status.success() && said(&o).contains("served and reached"), "a listed port serves and is reached: {}", said(&o));
    // the internet stays closed, whichever network the box has
    let o = boxed(&p, &reach("1.1.1.1", 443), &["--local", "--port", "443"]);
    assert!(!o.status.success(), "the box reached the internet: {}", said(&o));
}

#[test]
fn a_port_outside_the_list_is_closed_unless_the_box_has_a_network_of_its_own() {
    let p = place("unlisted");
    let net = local_network(&p);
    if net == "refused" {
        return;
    }
    let listed = 20000 + (std::process::id() % 20000) as u16;
    let other = listed + 1;
    let o = boxed(&p, &serve_and_reach(other), &["--local", "--port", &listed.to_string()]);
    if net == "own" {
        assert!(o.status.success(), "in its own network any port serves: {}", said(&o));
    } else {
        assert!(!o.status.success(), "a port not listed served: {}", said(&o));
    }
}

#[test]
fn a_service_of_this_machine_is_not_reached_and_the_page_never_is() {
    let p = place("service");
    let net = local_network(&p);
    if net == "refused" {
        return;
    }
    // a service listening on this machine's loopback, outside the box, as the Jaynes page or a database
    let service = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = service.local_addr().expect("addr").port();
    let o = boxed(&p, &reach("127.0.0.1", port), &["--local"]);
    assert!(!o.status.success(), "the box reached a service of this machine: {}", said(&o));
    // even with every port opened by the person, the page's port stays closed
    let o = boxed(&p, &reach("127.0.0.1", port), &["--local", "--any-port", "--never-port", &port.to_string()]);
    assert!(!o.status.success(), "the box reached the page's port: {}", said(&o));
    drop(service);
}

#[test]
fn any_port_is_only_for_a_command_that_runs_local_servers() {
    let p = place("any");
    opens();
    let o = boxed(&p, "echo ran", &["--any-port"]);
    assert!(!o.status.success() && !said(&o).contains("ran"), "{}", said(&o));
}

#[test]
fn a_command_that_asks_for_no_network_gets_none_on_any_machine() {
    let p = place("none");
    opens();
    let o = boxed(&p, "echo $JAYNES_BOX_NETWORK", &[]);
    assert!(o.status.success() && said(&o).contains("none"), "{}", said(&o));
}

#[test]
fn what_is_shown_inside_a_hidden_folder_is_read_and_nothing_else_of_it() {
    // the run's copy lives inside the hidden runs: the box reads the copy and no other run
    let p = place("show");
    opens();
    let runs = p.base.join("runs");
    for d in ["mine/project", "other"] {
        std::fs::create_dir_all(runs.join(d)).expect("dir");
    }
    std::fs::write(runs.join("mine/project/greeting.txt"), "hello\n").expect("write");
    std::fs::write(runs.join("other/output.json"), "secret\n").expect("write");
    std::fs::write(runs.join("mine/record"), "secret\n").expect("write");
    let copy = runs.join("mine/project");
    let args = ["--hide", runs.to_str().expect("path"), "--show", copy.to_str().expect("path")];
    let o = boxed(&p, &format!("cat {}/mine/project/greeting.txt", runs.display()), &args);
    assert!(o.status.success() && said(&o).contains("hello"), "the shown copy is read: {}", said(&o));
    for other in ["other/output.json", "mine/record"] {
        let o = boxed(&p, &format!("cat {}/{other}", runs.display()), &args);
        assert!(!o.status.success(), "the box read {other}, which is hidden: {}", said(&o));
    }
}

#[test]
fn a_folder_it_may_only_read_inside_its_own_is_not_written_and_the_rest_is() {
    // the run's copy, written by a role's script, and its .git, where Jaynes runs git later outside any box
    let p = place("readonly");
    opens();
    let copy = p.work.join("copy");
    std::fs::create_dir_all(copy.join(".git/hooks")).expect("dir");
    std::fs::create_dir_all(copy.join("src")).expect("dir");
    std::fs::write(copy.join(".git/config"), "[core]\n").expect("write");
    std::fs::write(copy.join("README.md"), "hello\n").expect("write");
    let git_dir = copy.join(".git");
    let args = ["--write", copy.to_str().expect("path"), "--readonly", git_dir.to_str().expect("path")];
    let in_copy = |script: &str| {
        Command::new(BIN).arg("box").args(args).args(hide(&p)).arg("--").args(["sh", "-c", &format!("cd {} && {script}", copy.display())]).output().expect("box")
    };
    for (what, script) in [
        ("a hook", "echo 'echo owned' > .git/hooks/post-commit"),
        ("git's settings", "echo x >> .git/config"),
        ("a folder in .git", "mkdir .git/info"),
        ("a rename of .git", "mv .git .old"),
        ("a link to .git", "ln -s .git g && echo x > g/config"),
    ] {
        let o = in_copy(script);
        assert!(!o.status.success(), "the box let {what} be written: {}", said(&o));
    }
    assert!(!copy.join(".git/hooks/post-commit").exists() && std::fs::read_to_string(copy.join(".git/config")).expect("read") == "[core]\n");
    // it reads .git, and writes everywhere else in the copy, its top as well
    for (what, script) in [
        ("reading .git", "cat .git/config"),
        ("a file at the top", "echo hi > README.md"),
        ("a new file in a folder", "echo a > src/a.rs && mkdir src/deep && echo b > src/deep/b.rs"),
        ("removing in a folder", "rm src/a.rs"),
        ("a new file at the top", "echo x > NEW && rm NEW"),
    ] {
        let o = in_copy(script);
        assert!(o.status.success(), "the box refused {what}: {}", said(&o));
    }
    assert_eq!(std::fs::read_to_string(copy.join("README.md")).expect("read"), "hi\n");
    // one not there yet cannot be made inside the box either
    std::fs::remove_dir_all(copy.join(".git")).expect("remove");
    let o = in_copy("mkdir -p .git/hooks; echo 'echo owned' > .git/hooks/post-commit");
    assert!(!o.status.success() && !copy.join(".git/hooks").exists(), "the box let .git be made: {}", said(&o));
}

#[test]
fn nothing_of_what_the_box_hides_shows_not_even_its_names_sizes_or_times() {
    // Landlock refuses a hidden file's contents but not its stat: the box's mounts hide that too
    let p = place("names");
    opens();
    let runs = p.base.join("runs");
    std::fs::create_dir_all(runs.join("mine/project")).expect("dir");
    std::fs::create_dir_all(runs.join("other")).expect("dir");
    std::fs::write(runs.join("other/secret.txt"), "a secret of some length\n").expect("write");
    std::fs::write(runs.join("mine/project/greeting.txt"), "hello\n").expect("write");
    let key = p.base.join("page.key");
    std::fs::write(&key, "k3y-of-a-length").expect("write");
    let copy = runs.join("mine/project");
    let args = ["--hide", runs.to_str().expect("path"), "--hide", key.to_str().expect("path"), "--show", copy.to_str().expect("path")];
    for (what, script) in [
        ("a hidden file's stat", format!("stat -c %s {}/other/secret.txt", runs.display())),
        ("another run's folder", format!("ls -d {}/other", runs.display())),
        ("a hidden file's size", format!("test -s {}", key.display())),
        ("a hidden file's contents", format!("cat {}", key.display())),
        ("a hidden file's name", format!("ls {}", key.display())),
    ] {
        let o = boxed(&p, &script, &args);
        assert!(!o.status.success(), "the box let through {what}: {}", said(&o));
        assert!(!said(&o).contains("24") && !said(&o).contains("k3y"), "{what} showed: {}", said(&o));
    }
    // the hidden folder lists only what was mounted back in it, and the folder of a hidden file lists it no more
    let o = boxed(&p, &format!("ls -a {} 2>&1; ls -a {}/mine 2>&1; ls -a {} 2>&1", runs.display(), runs.display(), p.base.display()), &args);
    assert!(!said(&o).contains("other") && !said(&o).contains("secret") && !said(&o).contains("page.key"), "a hidden name showed: {}", said(&o));
    // what it is shown inside the hidden is there whole
    let o = boxed(&p, &format!("cat {}/greeting.txt", copy.display()), &args);
    assert!(o.status.success() && said(&o).contains("hello"), "the shown copy is read: {}", said(&o));
    // and what it writes inside the hidden is written there, and nowhere else of it
    let o = Command::new(BIN).arg("box").args(["--write", copy.to_str().expect("path"), "--hide", runs.to_str().expect("path")]).args(hide(&p)).arg("--")
        .args(["sh", "-c", &format!("echo hi > {}/greeting.txt", copy.display())]).output().expect("box");
    assert!(o.status.success(), "{}", said(&o));
    assert_eq!(std::fs::read_to_string(copy.join("greeting.txt")).expect("read"), "hi\n");
}

#[test]
fn a_command_started_inside_what_the_box_holds_reaches_it_only_through_the_box() {
    // a process enters the box standing in the run's copy: a path from there passes through the box's mounts too
    let p = place("cwd");
    opens();
    let copy = p.work.join("copy");
    std::fs::create_dir_all(copy.join(".git/hooks")).expect("dir");
    let git_dir = copy.join(".git");
    let o = Command::new(BIN).current_dir(&copy).arg("box").args(["--write", copy.to_str().expect("path"), "--readonly", git_dir.to_str().expect("path")]).args(hide(&p))
        .arg("--").args(["sh", "-c", "echo owned > .git/hooks/post-commit"]).output().expect("box");
    assert!(!o.status.success() && !copy.join(".git/hooks/post-commit").exists(), "a relative path passed by the read-only .git: {}", said(&o));
    // started inside a folder it hides, it stands in the empty folder that took its place
    let secret = p.base.join("secret");
    std::fs::create_dir_all(&secret).expect("dir");
    std::fs::write(secret.join("key"), "k3y").expect("write");
    let o = Command::new(BIN).current_dir(&secret).arg("box").args(["--write", p.work.to_str().expect("path"), "--hide", secret.to_str().expect("path")]).args(hide(&p))
        .arg("--").args(["sh", "-c", "cat key; ls"]).output().expect("box");
    assert!(!said(&o).contains("k3y"), "started inside the hidden, the command read it: {}", said(&o));
}

#[test]
fn a_home_with_a_hidden_file_and_hidden_folders_shows_none_of_them_and_keeps_what_is_shown_inside_one() {
    // the arrangement the mounts' step 2 reads after step 1: the home holds a hidden file, so step 1 puts a tmpfs on
    // it, and two hidden folders beside it, one of which holds something the box is shown
    let p = place("home-arrangement");
    opens();
    let home = p.base.join("person");
    for d in [".ssh", ".aws", "notes"] {
        std::fs::create_dir_all(home.join(d)).expect("dir");
    }
    std::fs::write(home.join(".bash_history"), "a command of the person's\n").expect("write");
    std::fs::write(home.join(".ssh/id_ed25519"), "the key\n").expect("write");
    std::fs::write(home.join(".aws/credentials"), "the credentials\n").expect("write");
    std::fs::write(home.join(".aws/config"), "region = shown\n").expect("write");
    std::fs::write(home.join("notes/todo"), "readable\n").expect("write");
    let h = |s: &str| home.join(s).display().to_string();
    let hide: Vec<String> = [".bash_history", ".ssh", ".aws"].iter().flat_map(|s| ["--hide".to_string(), h(s)]).collect();
    let mut args: Vec<&str> = hide.iter().map(String::as_str).collect();
    // what it lists of the home, and what it reads of each: an error of cat names the path it was given, so names
    // are judged by the listings and contents by what was read
    let listed = said(&boxed(&p, &format!("ls -a {}", home.display()), &args));
    assert!(listed.contains("notes"), "what is not hidden is there: {listed}");
    for name in [".bash_history", ".ssh", ".aws"] {
        assert!(!listed.contains(name), "{name} showed in the home: {listed}");
    }
    let read = said(&boxed(&p, &format!("cat {home}/notes/todo; cat {home}/.bash_history {home}/.ssh/id_ed25519 {home}/.aws/credentials; true", home = home.display()), &args));
    assert!(read.contains("readable"), "what is not hidden is read: {read}");
    for secret in ["a command of", "the key", "the credentials"] {
        assert!(!read.contains(secret), "{secret} was read: {read}");
    }
    // shown inside a hidden folder: that alone of it is there, and the rest of it stays hidden
    let config = h(".aws/config");
    args.extend(["--show", config.as_str()]);
    let read = said(&boxed(&p, &format!("cat {config}; cat {}/credentials; true", h(".aws")), &args));
    assert!(read.contains("region = shown") && !read.contains("the credentials"), "{read}");
    let listed = said(&boxed(&p, &format!("ls -a {}; ls -a {}", h(".aws"), home.display()), &args));
    for name in ["credentials", ".bash_history", ".ssh"] {
        assert!(!listed.contains(name), "{name} showed: {listed}");
    }
}

// --- what a command leaves behind, and the person's terminal ---------------------------------------------------

/// A command line that starts, in the background, a process in a session of its own that adds a line to `f` every
/// 100 ms until something stops it: what a check's suite could leave running to change the copy after its checks.
fn left_running(f: &Path) -> String {
    format!("setsid sh -c 'while :; do echo . >> {f}; sleep 0.1; done' </dev/null >/dev/null 2>&1 &", f = f.display())
}

/// Whether anything still writes `f`: its size now and a while later.
fn still_written(f: &Path) -> bool {
    let size = || std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
    let before = size();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    size() != before
}

/// Wait until `f` exists, saying so past 10 s.
fn appears(f: &Path) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !f.exists() {
        assert!(std::time::Instant::now() < until, "{} never appeared", f.display());
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn nothing_a_command_starts_outlives_it_not_even_in_a_session_of_its_own() {
    let p = place("left-running");
    opens();
    let f = p.work.join("written");
    let o = boxed(&p, &format!("{} sleep 0.5", left_running(&f)), &[]);
    assert!(o.status.success(), "{}", said(&o));
    appears(&f);
    assert!(!still_written(&f), "a process the command started went on after the command ended");
}

#[test]
fn nothing_a_command_starts_outlives_it_when_it_is_stopped_from_outside() {
    // stopped at once, as a halt of the run stops it
    let p = place("left-killed");
    opens();
    let f = p.work.join("written");
    let mut c = Command::new(BIN);
    c.arg("box").arg("--write").arg(&p.work).args(hide(&p)).arg("--").args(["sh", "-c", &format!("{} sleep 30", left_running(&f))]);
    let mut running = c.current_dir(&p.work).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("box");
    appears(&f);
    let _killed = running.kill();
    let _reaped = running.wait();
    assert!(!still_written(&f), "a process the command started went on after the command was stopped");
}

#[test]
fn a_command_cannot_put_keys_into_a_terminal_nor_use_the_console_s_control_call() {
    // on a terminal of its own; the box's filter refuses both calls (EPERM), on any kernel, whatever its
    // dev.tty.legacy_tiocsti says (a kernel that disables the first answers EIO by itself, which is not the box)
    let p = place("terminal");
    opens();
    let script = r#"python3 -c '
import errno, fcntl, os, termios
_, term = os.openpty()
for name, request, arg in (("TIOCSTI", termios.TIOCSTI, b"x"), ("TIOCLINUX", 0x541C, b"\x06")):
    try:
        fcntl.ioctl(term, request, arg)
        print(name, "taken")
    except OSError as e:
        print(name, errno.errorcode.get(e.errno, str(e.errno)))
'"#;
    let o = boxed(&p, script, &[]);
    let out = said(&o);
    assert!(out.contains("TIOCSTI EPERM"), "the box let a command put keys into a terminal, or did not refuse it itself: {out}");
    assert!(out.contains("TIOCLINUX EPERM"), "the box let a command use the console's control call: {out}");
}

// --- the relay of the ports of this machine a command may reach ----------------------------------------------------

/// A service on this machine's loopback, outside the box, that says hello to whoever connects.
fn service() -> (std::net::TcpListener, u16) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = l.local_addr().expect("addr").port();
    let served = l.try_clone().expect("clone");
    std::thread::spawn(move || {
        for c in served.incoming().flatten() {
            use std::io::Write;
            let mut c = c;
            let _said = c.write_all(b"hello from outside\n");
        }
    });
    (l, port)
}

fn read_from(host: &str, port: u16) -> String {
    format!("python3 -c 'import socket; s=socket.create_connection((\"{host}\",{port}),timeout=3); print(s.recv(100).decode())'")
}

#[test]
fn a_listed_port_of_this_machine_is_reached_through_the_relay_and_nothing_else_is() {
    let p = place("relay");
    opens();
    let (_listed, port) = service();
    let (_other, unlisted) = service();
    let (_page, page) = service();
    let o = boxed(&p, &read_from("127.0.0.1", port), &["--port", &port.to_string()]);
    assert!(o.status.success() && said(&o).contains("hello from outside"), "a listed port of this machine is reached: {}", said(&o));
    assert!(said(&o).is_empty() || !said(&o).contains("refuses"), "{}", said(&o));
    for (what, script, args) in [
        ("a port not listed", read_from("127.0.0.1", unlisted), vec!["--port".to_string(), port.to_string()]),
        ("the page's port, listed and never", read_from("127.0.0.1", page), vec!["--port".to_string(), page.to_string(), "--never-port".to_string(), page.to_string()]),
        ("another address at a listed port", read_from("10.255.255.1", port), vec!["--port".to_string(), port.to_string()]),
        ("the internet", reach("1.1.1.1", 443), vec!["--port".to_string(), "443".to_string()]),
    ] {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let o = boxed(&p, &script, &args);
        assert!(!o.status.success() && !said(&o).contains("hello from outside"), "the box reached {what}: {}", said(&o));
    }
}

#[test]
fn a_command_that_runs_local_servers_reaches_the_services_of_this_machine_it_names_and_no_other() {
    // a test suite that serves on its own port and reaches a database of this machine its step names
    let p = place("reach");
    let net = local_network(&p);
    if net == "refused" {
        return;
    }
    let (_named, port) = service();
    let (_other, unnamed) = service();
    let (_page, page) = service();
    let own = 20000 + (std::process::id() % 20000) as u16;
    let both = format!("{} && {}", serve_and_reach(own), read_from("127.0.0.1", port));
    let o = boxed(&p, &both, &["--local", "--port", &own.to_string(), "--reach", &port.to_string()]);
    assert!(o.status.success() && said(&o).contains("served and reached") && said(&o).contains("hello from outside"),
            "its own server and the service it names, in one command: {}", said(&o));
    for (what, script, args) in [
        ("a service it does not name", read_from("127.0.0.1", unnamed), vec!["--reach".to_string(), port.to_string()]),
        ("the page's port, named and never", read_from("127.0.0.1", page), vec!["--reach".to_string(), page.to_string(), "--never-port".to_string(), page.to_string()]),
        ("another address at a named port", read_from("10.255.255.1", port), vec!["--reach".to_string(), port.to_string()]),
    ] {
        let mut args: Vec<&str> = args.iter().map(String::as_str).collect();
        args.insert(0, "--local");
        let o = boxed(&p, &script, &args);
        assert!(!o.status.success() && !said(&o).contains("hello from outside"), "the box reached {what}: {}", said(&o));
    }
}

#[test]
fn with_every_port_opened_a_command_reaches_the_services_of_this_machine_but_the_page() {
    // the person opened every local port for the project: a service of this machine is reached, the page never; and
    // where the machine's network has a way out, the box refuses, since its rules hold a port and not an address
    let p = place("any-reach");
    opens();
    let (_service, port) = service();
    let (_page, page) = service();
    let any = ["--local", "--any-port", "--never-port", &page.to_string()].map(str::to_string);
    let any: Vec<&str> = any.iter().map(String::as_str).collect();
    let o = boxed(&p, &read_from("127.0.0.1", port), &any);
    if let Some(out) = jaynes_shell::sandbox::way_out() {
        assert!(!o.status.success() && said(&o).contains("refuses to open local ports"), "{out}, and it opened every port: {}", said(&o));
        return;
    }
    assert!(o.status.success() && said(&o).contains("hello from outside"), "every port opened, and a service of this machine was not reached: {}", said(&o));
    let o = boxed(&p, &read_from("127.0.0.1", page), &any);
    assert!(!o.status.success() && !said(&o).contains("hello from outside"), "the box reached the page's port: {}", said(&o));
}

#[test]
fn the_relay_s_helper_connects_only_for_the_ports_it_was_started_with() {
    // the helper as the box starts it, asked from its channel as the forwarder asks it
    opens();
    let (_listed, port) = service();
    let (_other, unlisted) = service();
    let (mine, its) = rustix::net::socketpair(rustix::net::AddressFamily::UNIX, rustix::net::SocketType::SEQPACKET, rustix::net::SocketFlags::CLOEXEC, None).expect("pair");
    let mut helper = Command::new(BIN)
        .args(["box-relay", "out", "--parent", &std::process::id().to_string(), "--port", &port.to_string()])
        .stdin(Stdio::from(its))
        .spawn()
        .expect("helper");
    use std::os::fd::AsFd;
    assert!(jaynes_shell::relay::ask(mine.as_fd(), port).expect("asked").is_some(), "a listed port");
    for other in [unlisted, 8750, 0] {
        assert!(jaynes_shell::relay::ask(mine.as_fd(), other).expect("asked").is_none(), "the helper connected for port {other}");
    }
    // a request of another shape opens nothing either
    rustix::net::send(mine.as_fd(), b"\x00", rustix::net::SendFlags::empty()).expect("send");
    let mut b = [0u8; 1];
    let (n, _) = rustix::net::recv(mine.as_fd(), &mut b[..], rustix::net::RecvFlags::empty()).expect("recv");
    assert_eq!((n, b[0]), (1, 0), "a request of another shape got a socket");
    drop(mine);
    let _ended = helper.wait();
}

/// The processes whose parent is `pid`, that have not ended.
fn children_running(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").expect("proc").flatten() {
        let Ok(child) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{child}/stat")) else { continue };
        let after = stat.rsplit_once(')').map(|(_, a)| a.trim().to_string()).unwrap_or_default();
        let mut fields = after.split_whitespace();
        let (state, parent) = (fields.next().unwrap_or(""), fields.next().unwrap_or(""));
        if parent == pid.to_string() && state != "Z" && state != "X" {
            out.push(child);
        }
    }
    out
}

#[test]
fn the_relay_holds_only_its_channel_and_ends_with_the_command_even_when_it_is_killed() {
    let p = place("relay-end");
    opens();
    let (_listed, port) = service();
    let mut c = Command::new(BIN);
    c.arg("box").arg("--write").arg(&p.work).args(hide(&p)).args(["--port", &port.to_string()]).arg("--").args(["sh", "-c", "sleep 30"]);
    let mut boxed = c.current_dir(&p.work).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("box");
    let pid = boxed.id();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    // the box's children that are its relay (the first process of its process namespace is another one)
    let relay = |p: &u32| std::fs::read(format!("/proc/{p}/cmdline")).is_ok_and(|c| String::from_utf8_lossy(&c).contains("box-relay"));
    let relays = loop {
        let r: Vec<u32> = children_running(pid).into_iter().filter(relay).collect();
        if r.len() >= 2 || std::time::Instant::now() > until {
            break r;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(relays.len(), 2, "the helper and the forwarder run beside the command: {relays:?}");
    // each holds its channel, its standard streams and its sockets, and nothing of a folder on disk
    for r in &relays {
        for fd in std::fs::read_dir(format!("/proc/{r}/fd")).expect("fds").flatten() {
            let to = std::fs::read_link(fd.path()).map(|l| l.display().to_string()).unwrap_or_default();
            assert!(to.starts_with("socket:") || to.starts_with("pipe:") || to == "/dev/null" || to.starts_with("anon_inode:"), "relay {r} holds {to}");
        }
    }
    // killed at once, as a halt of the engine would kill it: neither outlives it
    let _killed = rustix::process::kill_process(rustix::process::Pid::from_raw(pid as i32).expect("pid"), rustix::process::Signal::KILL);
    let _reaped = boxed.wait();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while relays.iter().any(|r| std::fs::read_to_string(format!("/proc/{r}/stat")).is_ok_and(|s| !s.contains(") Z") && !s.contains(") X"))) && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    for r in &relays {
        let left = std::fs::read_to_string(format!("/proc/{r}/stat")).is_ok_and(|s| !s.contains(") Z") && !s.contains(") X"));
        assert!(!left, "relay {r} outlived the command it served");
    }
}
