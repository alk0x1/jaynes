//! Before `jaynes update` installs a newer release, it says the open runs that release leaves behind: those an older
//! engine started whose program is not kept, which no answer, Halt or Abandon will reach after it (the design-lead's
//! words, 0.2.5). The releases, the installer and Claude Code are stand-ins in the test's folder: nothing is fetched,
//! nothing of the person's is installed, and no model is reached.

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

struct Scratch {
    base: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _gone = std::fs::remove_dir_all(&self.base);
    }
}

/// Claude Code that fails every turn at once, and takes the plugin's commands.
const STAND_IN: &str = "#!/bin/sh\n[ \"$1\" = plugin ] && exit 0\necho 'no model here' >&2\nexit 1\n";

/// The release's installer: it only says it ran, where the update's own words go.
const INSTALLER: &str = "echo 'the installer ran' >&2\n";

impl Scratch {
    fn new(name: &str) -> Scratch {
        let base = std::env::temp_dir().join(format!("jaynes-update-warns-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let project = base.join("project");
        for dir in [&project, &base.join("person"), &base.join("bin"), &base.join("releases"), &base.join("install")] {
            std::fs::create_dir_all(dir).expect("dir");
        }
        std::fs::write(project.join("greeting.txt"), "hello\n").expect("write");
        std::fs::write(project.join("Makefile"), "test:\n\tgrep -q hi greeting.txt\n").expect("write");
        for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
            assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success(), "git {args:?}");
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(base.join("bin/claude"), STAND_IN).expect("write");
        std::fs::set_permissions(base.join("bin/claude"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
        std::fs::write(base.join("install.sh"), INSTALLER).expect("write");
        let built = serde_json::json!({"url": "file:///nonexistent/jaynes.tar.gz"});
        let targets: serde_json::Map<String, serde_json::Value> =
            ["macos-aarch64", "macos-x86_64", "linux-x86_64", "linux-aarch64"].iter().map(|t| (t.to_string(), built.clone())).collect();
        std::fs::write(base.join("releases/latest.json"), serde_json::json!({"version": "99.0.0", "targets": targets}).to_string()).expect("write");
        Scratch { base }
    }

    fn jaynes(&self, args: &[&str]) -> (bool, String) {
        let b = &self.base;
        let out = Command::new(BIN)
            .args(args)
            .env("HOME", b.join("person"))
            .env("PATH", format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", b.join("bin").display()))
            .env("JAYNES_ENGINE_HOME", b.join("home")).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
            .env("JAYNES_HOME", b.join("settings")).env("JAYNES_INSTALL_ROOT", b.join("install"))
            .env("JAYNES_RELEASES", format!("file://{}", b.join("releases").display()))
            .env("JAYNES_INSTALLER", format!("file://{}", b.join("install.sh").display()))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("JAYNES_CLAUDE_MODEL").env_remove("JAYNES_CLAUDE_EFFORT").env_remove("JAYNES_AGENT").env_remove("JAYNES_CODEX")
            .output().expect("jaynes");
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    /// An open run of a small change, its record rewritten as if Jaynes `engine` had started it.
    fn open_run_of(&self, run: &str, engine: &str) {
        let role = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roles/small-change");
        let project = self.base.join("project");
        let _said = self.jaynes(&["start", "--run", run, "--role", role.to_str().expect("path"), "--project", project.to_str().expect("path"),
                                  "--request", "say hi", "--agent", "claude"]);
        let record = self.base.join("home/record.sqlite3");
        let sql = format!("update events set body = json_set(body, '$.engine', '{engine}') where run = '{run}' and type = 'RunStarted'");
        let ok = Command::new("python3").args(["-I", "-c", "import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute(sys.argv[2]); c.commit()"])
            .arg(&record).arg(&sql).status().expect("python3").success();
        assert!(ok, "the record of {run} is rewritten");
        let (_ok, shown) = self.jaynes(&["show", run]);
        assert!(!shown.contains("finished"), "{run} stays open: {shown}");
    }
}

/// This engine's release, as the update says it: without its fingerprint.
fn this_release() -> &'static str {
    jaynes_core::ENGINE.split('+').next().unwrap_or(jaynes_core::ENGINE)
}

#[test]
fn an_update_says_the_open_runs_it_leaves_behind_before_it_installs() {
    let s = Scratch::new("left");
    // a run of this engine that goes on today, whose program Jaynes did not keep: the update leaves it without one
    s.open_run_of("r1", jaynes_core::ENGINE);
    let kept = s.base.join("home/engines").join(jaynes_core::ENGINE).join("jaynes");
    std::fs::remove_file(&kept).unwrap_or_else(|e| panic!("{}: {e}", kept.display()));
    let (ok, said) = s.jaynes(&["update"]);
    assert!(ok, "{said}");
    let v = this_release();
    let warned = said.find(&format!("1 run started on Jaynes {v} can't go on after this update: no answer, Halt or Abandon will reach it."))
        .unwrap_or_else(|| panic!("the run left behind is said: {said}"));
    let ran = said.find("the installer ran").unwrap_or_else(|| panic!("the release's installer runs: {said}"));
    assert!(warned < ran, "said before the install, while the person can still finish it: {said}");
    assert!(said.contains(&format!("To finish it on {v}, answer or abandon it before you update.")), "{said}");
    assert!(said.contains("Jaynes 99.0.0 installed"), "{said}");
}

/// A run whose older engine already left the Mac cannot go on before the update either: it is listed under "From an
/// older Jaynes", and the update, which changes nothing for it, does not say it (nor tell the person to finish it on
/// a version they no longer have).
#[test]
fn an_update_does_not_say_a_run_that_already_could_not_go_on() {
    let s = Scratch::new("already");
    s.open_run_of("r1", "0.2.3+0ld");
    let (ok, said) = s.jaynes(&["update"]);
    assert!(ok && said.contains("the installer ran"), "{said}");
    assert!(!said.contains("can't go on after this update") && !said.contains("To finish"), "{said}");
}

#[test]
fn an_update_that_leaves_no_run_behind_says_nothing_of_them() {
    let s = Scratch::new("none");
    // a run of this very engine, whose program Jaynes kept as the run started: the update leaves it a program
    s.open_run_of("r1", jaynes_core::ENGINE);
    let (ok, said) = s.jaynes(&["update"]);
    assert!(ok, "{said}");
    assert!(said.contains("the installer ran"), "{said}");
    assert!(!said.contains("can't go on after this update"), "a run whose program is kept is not left behind: {said}");
}
