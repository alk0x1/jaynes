//! `jaynes uninstall` takes away what the installer put on the machine and says what stays in each repository. It
//! shows first and removes only on a yes; it never removes what still runs (the app, a run's agent, a herdr-core that
//! holds terminals) nor what Jaynes did not put there; it asks apart before the data folder; and it never touches a
//! repository. Everything here is a stand-in: a home folder, an apps folder, and a `claude` and a `cargo` on the PATH
//! that note their calls, so nothing of the machine running the tests is touched.

use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct World {
    base: PathBuf,
    kids: Vec<Child>,
}

impl Drop for World {
    fn drop(&mut self) {
        for k in &mut self.kids {
            let _killed = k.kill();
            let _reaped = k.wait();
        }
        let _gone = std::fs::remove_dir_all(&self.base);
    }
}

fn executable(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    std::fs::write(path, text).expect("write");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn sha256(path: &Path) -> String {
    Sha256::digest(std::fs::read(path).expect("read")).iter().map(|b| format!("{b:02x}")).collect()
}

/// A port nothing answers on, below the range outgoing connections take.
fn port() -> u16 {
    for p in (21000..39000).step_by(37).skip((std::process::id() % 400) as usize) {
        let free = std::net::TcpListener::bind(("127.0.0.1", p)).is_ok() && std::net::TcpStream::connect(("127.0.0.1", p)).is_err();
        if free {
            return p;
        }
    }
    panic!("no free port")
}

impl World {
    /// What a release install leaves: the program and a half-written copy, the app with its permissions' bundle id,
    /// herdr-core with the installer's marker, the plugin in Claude Code, and the engine's folder.
    fn new(name: &str) -> World {
        let base = std::env::temp_dir().join(format!("jaynes-uninstall-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let w = World { base, kids: Vec::new() };
        executable(&w.program(), "#!/bin/sh\necho jaynes\n");
        executable(&w.bin().join("jaynes.new"), "half\n");
        let app = w.app();
        executable(&app.join("Contents/MacOS/Jaynes"), "#!/bin/sh\n");
        std::fs::write(app.join("Contents/Info.plist"), PLIST).expect("plist");
        executable(&w.herdr(), "#!/bin/sh\necho herdr-core\n");
        std::fs::create_dir_all(w.engine().join("runs")).expect("dir");
        std::fs::create_dir_all(w.engine().join("installed")).expect("dir");
        std::fs::write(w.note(), serde_json::json!({"path": w.herdr(), "sha256": sha256(&w.herdr())}).to_string()).expect("note");
        std::fs::write(w.data().join("guard.json"), "{}").expect("write");
        std::fs::write(w.data().join("jaynes.sqlite3"), "an old engine's record").expect("write");
        let calls = w.base.join("calls.txt");
        let tools = w.base.join("tools");
        for tool in ["claude", "cargo"] {
            executable(&tools.join(tool), &format!("#!/bin/sh\necho \"{tool} $* app=$([ -d '{}' ] && echo here || echo gone)\" >> '{}'\n", app.display(), calls.display()));
        }
        w
    }

    fn home(&self) -> PathBuf {
        self.base.join("home")
    }
    fn data(&self) -> PathBuf {
        self.home().join(".jaynes")
    }
    fn engine(&self) -> PathBuf {
        self.data().join("engine")
    }
    fn bin(&self) -> PathBuf {
        self.home().join(".local/bin")
    }
    fn program(&self) -> PathBuf {
        self.bin().join("jaynes")
    }
    fn herdr(&self) -> PathBuf {
        self.bin().join("herdr-core")
    }
    /// The installer's note that it put herdr-core there.
    fn note(&self) -> PathBuf {
        self.engine().join("installed/herdr-core.json")
    }
    fn apps(&self) -> PathBuf {
        self.base.join("Applications")
    }
    /// The app's stand-in: a folder of an app's shape whose name macOS never takes for an app (no .app), from which
    /// nothing is ever run.
    fn app(&self) -> PathBuf {
        self.apps().join("Jaynes-stand-in")
    }
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.base.join("calls.txt")).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default()
    }

    fn env(&self, cmd: &mut Command, page_port: u16) {
        let path = format!("{}:{}", self.base.join("tools").display(), std::env::var("PATH").unwrap_or_default());
        cmd.env("HOME", self.home()).env("JAYNES_ENGINE_HOME", self.engine()).env_remove("JAYNES_HOME")
            .env("JAYNES_INSTALL_ROOT", self.home().join(".local")).env("JAYNES_APP_DIRS", self.apps()).env("JAYNES_APP_NAME", "Jaynes-stand-in")
            .env("JAYNES_PAGE_PORT", page_port.to_string()).env("JAYNES_NO_PAGE", "1").env("PATH", path);
    }

    /// The environment, with no lsof on the PATH: what a machine without lsof gives, as a Linux image without it.
    fn env_without_lsof(&self, cmd: &mut Command, page_port: u16) {
        self.env_without(cmd, page_port, "lsof");
    }

    /// The environment with no program `name` on the PATH, every other program kept: a folder that holds it is
    /// replaced by one with links to all it holds but that one.
    fn env_without(&self, cmd: &mut Command, page_port: u16, name: &str) {
        self.env(cmd, page_port);
        let shadow = self.base.join(format!("no-{name}"));
        std::fs::create_dir_all(&shadow).expect("dir");
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![self.base.join("tools")];
        for d in std::env::split_paths(&path) {
            if !d.join(name).exists() {
                dirs.push(d);
                continue;
            }
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                if e.file_name() != name && !shadow.join(e.file_name()).exists() {
                    std::os::unix::fs::symlink(e.path(), shadow.join(e.file_name())).expect("link");
                }
            }
        }
        dirs.push(shadow);
        cmd.env("PATH", std::env::join_paths(dirs).expect("path"));
    }

    /// `jaynes uninstall` with `args`, standard input not a terminal: whether it succeeded, and what it said.
    fn uninstall(&self, args: &[&str]) -> (bool, String) {
        let mut cmd = Command::new(BIN);
        cmd.arg("uninstall").args(args).stdin(Stdio::null());
        self.env(&mut cmd, port());
        let out = cmd.output().expect("uninstall");
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    /// A program that stays running from `path`, as an app or herdr-core runs from its file: a copy of this build's
    /// jaynes serving MCP, which waits on its input. (A copy of a system program such as sleep is killed by macOS.)
    fn running(&mut self, path: &Path) {
        std::fs::copy(BIN, path).expect("copy");
        let mut child = Command::new(path).arg("mcp").env("JAYNES_ENGINE_HOME", self.base.join("elsewhere"))
            .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("run");
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(child.try_wait().expect("wait").is_none(), "{} did not stay running", path.display());
        self.kids.push(child);
    }

    /// A delivered run of small-change on a project of its own, which leaves its branch there.
    fn delivered(&self) -> PathBuf {
        let project = self.base.join("project");
        std::fs::create_dir_all(&project).expect("dir");
        std::fs::write(project.join("greeting.txt"), "hello\n").expect("write");
        std::fs::write(project.join("Makefile"), "test:\n\tgrep -q hi greeting.txt\n").expect("write");
        for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
            assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success());
        }
        let agent = r#"command:perl -pi -e 's/hello/hi/' greeting.txt && printf '{"summary":"hello becomes hi","files":["greeting.txt"]}' > .jaynes/output.json"#;
        let out = Command::new(BIN)
            .args(["start", "--run", "kept", "--role", root().join("crates/shell/tests/roles/small-change").to_str().expect("path")])
            .args(["--project", project.to_str().expect("path"), "--request", "say hi", "--agent", agent])
            .env("JAYNES_ENGINE_HOME", self.engine()).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
            .output().expect("start");
        let said = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success() && said.contains("delivered"), "{said}");
        project
    }
}

const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>dev.jaynes.test-app</string><key>CFBundleExecutable</key><string>Jaynes</string></dict></plist>
"#;

fn branches(project: &Path) -> String {
    let out = Command::new("git").arg("-C").arg(project).args(["for-each-ref", "--format=%(refname)", "refs/heads/jaynes/", "refs/jaynes/"]).output().expect("git");
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn without_a_yes_it_shows_everything_and_removes_nothing() {
    let w = World::new("show");
    let project = w.delivered();
    let (ok, said) = w.uninstall(&[]);
    assert!(!ok, "it went on without a yes: {said}");
    for thing in [w.app().display().to_string(), w.program().display().to_string(), w.herdr().display().to_string(), w.data().display().to_string(),
                  "jaynes@jaynes".into(), "jaynes/kept".into(), "--yes".into()] {
        assert!(said.contains(&thing), "it does not name {thing}:\n{said}");
    }
    let real = project.canonicalize().expect("project");
    assert!(said.contains(&format!("git -C {} branch -D jaynes/kept", real.display())), "the command to clean the repository:\n{said}");
    assert!(w.app().exists() && w.program().exists() && w.herdr().exists() && w.engine().exists(), "it removed something");
    assert_eq!(w.calls(), Vec::<String>::new(), "it ran claude or cargo");
}

#[test]
fn with_a_yes_it_removes_the_plugin_first_then_the_app_and_the_program_and_keeps_the_data_and_the_repository() {
    let w = World::new("yes");
    let project = w.delivered();
    let before = branches(&project);
    assert!(before.contains("jaynes/kept"), "{before}");
    let (ok, said) = w.uninstall(&["--yes", "--keep-data"]);
    assert!(ok, "{said}");
    assert_eq!(w.calls(), ["claude plugin uninstall jaynes@jaynes app=here", "claude plugin marketplace remove jaynes app=here"], "{said}");
    assert!(!w.app().exists() && !w.program().exists() && !w.bin().join("jaynes.new").exists(), "{said}");
    assert!(!w.herdr().exists(), "the herdr-core Jaynes installed, unchanged and not running, goes:\n{said}");
    assert!(w.engine().join("record.sqlite3").exists() && w.data().join("guard.json").exists(), "the data was kept");
    assert_eq!(branches(&project), before, "the repository is the person's");
    // what macOS keeps of the app's permissions; Linux keeps none
    assert_eq!(said.contains("tccutil reset Accessibility dev.jaynes.test-app"), cfg!(target_os = "macos"), "the permissions line:\n{said}");
    assert!(said.contains("PATH"), "says the shell's PATH was never changed:\n{said}");
}

#[test]
fn the_data_folder_goes_only_when_asked_for_and_its_size_is_told_in_parts() {
    let w = World::new("data");
    std::fs::create_dir_all(w.engine().join("runs/r1/project")).expect("dir");
    std::fs::write(w.engine().join("runs/r1/project/big.bin"), vec![0u8; 300_000]).expect("write");
    let (ok, said) = w.uninstall(&["--yes"]);
    assert!(ok && w.data().exists(), "with no word on the data, it stays:\n{said}");
    assert!(said.contains("runs") && said.contains("jaynes.sqlite3"), "the copies and the old engine's files are counted apart:\n{said}");
    let w = World::new("data-gone");
    let (ok, said) = w.uninstall(&["--yes", "--delete-data"]);
    assert!(ok && !w.data().exists(), "{said}");
}

#[test]
fn a_herdr_core_jaynes_did_not_put_there_or_that_changed_stays() {
    let w = World::new("herdr-changed");
    std::fs::write(w.herdr(), "updated by Herdr itself\n").expect("write");
    let (ok, said) = w.uninstall(&["--yes", "--keep-data"]);
    assert!(ok && w.herdr().exists(), "{said}");
    assert!(said.contains("changed since Jaynes installed it"), "{said}");
    let w = World::new("herdr-unmarked");
    std::fs::remove_file(w.note()).expect("rm");
    let (ok, said) = w.uninstall(&["--yes", "--keep-data"]);
    assert!(ok && w.herdr().exists(), "{said}");
    assert!(said.contains("Jaynes did not install"), "{said}");
}

#[test]
fn a_herdr_core_that_runs_stays_and_the_person_is_told_how_to_stop_and_remove_it() {
    let mut w = World::new("herdr-running");
    let herdr = w.herdr();
    w.running(&herdr);
    std::fs::write(w.note(), serde_json::json!({"path": herdr, "sha256": sha256(&herdr)}).to_string()).expect("note");
    let (ok, said) = w.uninstall(&["--yes", "--keep-data"]);
    assert!(ok, "the rest goes: {said}");
    assert!(w.herdr().exists() && !w.app().exists(), "{said}");
    assert!(said.contains("hc server stop") && said.contains(&format!("rm {}", herdr.display())), "{said}");
}

#[test]
fn nothing_is_removed_while_a_run_s_agent_still_works() {
    let mut w = World::new("run-alive");
    let dir = w.engine().join("runs/busy");
    std::fs::create_dir_all(&dir).expect("dir");
    let agent = Command::new("sh").args(["-c", "sleep 60"]).process_group(0).spawn().expect("agent");
    std::fs::write(dir.join("agent.pid"), agent.id().to_string()).expect("pid");
    w.kids.push(agent);
    let (ok, said) = w.uninstall(&["--yes", "--keep-data"]);
    assert!(!ok && said.contains("busy") && said.contains("jaynes halt busy"), "{said}");
    assert!(w.app().exists() && w.program().exists() && w.calls().is_empty(), "{said}");
}

#[test]
fn the_page_of_this_engine_is_stopped_first_and_no_other() {
    let mut w = World::new("page");
    let p = port();
    let mut page = Command::new(BIN);
    page.args(["page", "--port", &p.to_string()]).env("JAYNES_NO_UPDATE_CHECK", "1").stdout(Stdio::null()).stderr(Stdio::null());
    w.env(&mut page, p);
    let child = page.spawn().expect("page");
    let pid = child.id();
    w.kids.push(child);
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", p)).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let mut cmd = Command::new(BIN);
    cmd.args(["uninstall", "--yes", "--keep-data"]).stdin(Stdio::null());
    w.env(&mut cmd, p);
    let out = cmd.output().expect("uninstall");
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success() && said.contains("page"), "{said}");
    let alive = Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().expect("kill").success();
    let reaped = w.kids.last_mut().and_then(|k| k.try_wait().ok().flatten()).is_some();
    assert!(!alive || reaped, "the page still runs: {said}");
}

#[test]
fn a_program_cargo_installed_is_removed_by_cargo_so_its_list_stays_true() {
    let w = World::new("cargo");
    std::fs::write(w.home().join(".local/.crates.toml"), "[v1]\n\"jaynes-shell 0.2.2 (git+https://github.com/alk0x1/jaynes#abc)\" = [\"jaynes\"]\n").expect("write");
    let (ok, said) = w.uninstall(&["--yes", "--keep-data"]);
    assert!(ok, "{said}");
    let root = w.home().join(".local");
    assert!(w.calls().iter().any(|c| c.starts_with(&format!("cargo uninstall jaynes-shell --root {}", root.display()))), "{:?}", w.calls());
}

#[test]
fn the_installer_s_note_is_the_one_uninstall_reads() {
    let w = World::new("note");
    std::fs::remove_file(w.note()).expect("rm");
    // only the installer's note-writing part runs here: the whole installer would download and place an app
    let script = std::fs::read_to_string(root().join("../site/install.sh")).expect("install.sh");
    let sha = script.lines().find(|l| l.starts_with("sha256()")).expect("sha256 helper");
    let note = script.split("# --- installed note: begin").nth(1).and_then(|s| s.split("# --- installed note: end").next()).expect("the note's part");
    let note_it = |path: &Path| {
        Command::new("sh").arg("-c").arg(format!("set -eu\nsay() {{ printf '%s\\n' \"$*\"; }}\n{sha}\n{note}\nnote_herdr \"$1\"")).arg("sh").arg(path)
            .env("JAYNES_ENGINE_HOME", w.engine()).status().expect("sh").success()
    };
    // a path with a line break would make a note no one reads: none is kept
    let odd = w.bin().join("herdr\ncore");
    std::fs::write(&odd, "x").expect("write");
    assert!(note_it(&odd) && !w.note().exists(), "a note was kept for a path with a line break");
    assert!(note_it(&w.herdr()) && w.note().exists());
    let (ok, said) = w.uninstall(&["--yes", "--keep-data"]);
    assert!(ok && !w.herdr().exists(), "the herdr-core the installer noted goes:\n{said}");
}

/// What a machine without lsof gives: on Linux, /proc says what runs; where nothing can say it (macOS without lsof),
/// nothing is removed, and the person reads why and what to install. Never "nothing runs" by not knowing.
#[test]
fn a_herdr_core_that_runs_is_never_taken_for_stopped_without_lsof() {
    let mut w = World::new("herdr-no-lsof");
    let herdr = w.herdr();
    w.running(&herdr);
    std::fs::write(w.note(), serde_json::json!({"path": herdr, "sha256": sha256(&herdr)}).to_string()).expect("note");
    let mut cmd = Command::new(BIN);
    cmd.args(["uninstall", "--yes", "--keep-data"]).stdin(Stdio::null());
    w.env_without_lsof(&mut cmd, port());
    let out = cmd.output().expect("uninstall");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(w.herdr().exists(), "a running herdr-core was removed without lsof:\n{said}");
    if cfg!(target_os = "linux") {
        assert!(out.status.success() && said.contains("hc server stop") && !w.app().exists(), "/proc tells it runs:\n{said}");
    } else {
        assert!(!out.status.success() && said.contains("cannot tell") && said.contains("lsof"), "says why:\n{said}");
        assert!(w.app().exists() && w.program().exists() && w.calls().is_empty(), "removed something:\n{said}");
    }
}

#[test]
fn the_page_is_never_said_stopped_without_lsof() {
    let mut w = World::new("page-no-lsof");
    let p = port();
    let mut page = Command::new(BIN);
    page.args(["page", "--port", &p.to_string()]).env("JAYNES_NO_UPDATE_CHECK", "1").stdout(Stdio::null()).stderr(Stdio::null());
    w.env(&mut page, p);
    let child = page.spawn().expect("page");
    let pid = child.id();
    w.kids.push(child);
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", p)).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let mut cmd = Command::new(BIN);
    cmd.args(["uninstall", "--yes", "--keep-data"]).stdin(Stdio::null());
    w.env_without_lsof(&mut cmd, p);
    let out = cmd.output().expect("uninstall");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let alive = Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().expect("kill").success();
    let reaped = w.kids.last_mut().and_then(|k| k.try_wait().ok().flatten()).is_some();
    if cfg!(target_os = "linux") {
        assert!(out.status.success() && (!alive || reaped), "/proc finds the page's process and it stops:\n{said}");
    } else {
        assert!(alive && !reaped, "the page was stopped without its process known:\n{said}");
        assert!(!out.status.success() && said.contains("cannot tell") && !said.contains("the page stopped"), "{said}");
        assert!(w.program().exists() && w.calls().is_empty(), "removed something:\n{said}");
    }
}

/// A machine where the kill program cannot run (a container without procps): whether a run's agent lives cannot be
/// told, so nothing is removed, and the person reads why. Never "nothing works" by not knowing.
#[test]
fn nothing_is_removed_when_kill_cannot_run_and_a_run_s_agent_may_still_work() {
    let mut w = World::new("no-kill");
    let dir = w.engine().join("runs/busy");
    std::fs::create_dir_all(&dir).expect("dir");
    let agent = Command::new("sh").args(["-c", "sleep 60"]).process_group(0).spawn().expect("agent");
    std::fs::write(dir.join("agent.pid"), agent.id().to_string()).expect("pid");
    w.kids.push(agent);
    let mut cmd = Command::new(BIN);
    cmd.args(["uninstall", "--yes", "--keep-data"]).stdin(Stdio::null());
    w.env_without(&mut cmd, port(), "kill");
    let out = cmd.output().expect("uninstall");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success() && said.contains("cannot tell") && said.contains("kill could not run"), "says why:\n{said}");
    assert!(w.app().exists() && w.program().exists() && w.calls().is_empty(), "removed something:\n{said}");
}
