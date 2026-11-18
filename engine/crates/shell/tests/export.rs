//! The export of numbers end to end, as a partner of an evaluation makes it: the real binary, the real small-change
//! role, a real git project, an agent that is a shell command. Every text a run can carry is planted (its id, its
//! request, the project's path, the agent's command, what the agent said, a correction's note), and the export
//! carries none of it: only the task's id given at the start.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

const PLANTED: &str = "PLANTEDsecret";

/// A command run at a terminal, as when the person types it (the same as e2e.rs's).
const TTY: &str = r#"
import os, pty, sys
pid, fd = pty.fork()
if pid == 0:
    os.execvp(sys.argv[1], sys.argv[1:])
while True:
    try:
        data = os.read(fd, 65536)
    except OSError:
        break
    if not data:
        break
    os.write(1, data)
sys.exit(os.waitstatus_to_exitcode(os.waitpid(pid, 0)[1]))
"#;

struct Scratch {
    base: PathBuf,
    home: PathBuf,
    project: PathBuf,
}

/// A test's folders go when it passes; a failed one keeps them for whoever reads why.
impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() && std::env::var("JAYNES_KEEP_TEST_RUNS").as_deref() != Ok("1") {
            let _gone = std::fs::remove_dir_all(&self.base);
        }
    }
}

/// A git project whose path carries the planted text.
fn scratch(name: &str) -> Scratch {
    let base = std::env::temp_dir().join(format!("jaynes-export-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let project = base.join(format!("{PLANTED}-folder")).join("project");
    std::fs::create_dir_all(&project).expect("dir");
    std::fs::write(project.join("greeting.txt"), "hello\n").expect("write");
    std::fs::write(project.join("Makefile"), "test:\n\tgrep -q hi greeting.txt\n").expect("write");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
        assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success(), "git {args:?}");
    }
    Scratch { home: base.join("home"), project, base }
}

/// The engine's answer: whether it succeeded, what it printed, and what it said on stderr.
fn engine(s: &Scratch, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(BIN).args(args).env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "").output().expect("engine");
    (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

/// What the agent does: says hi, and tells what it did and what it spent with the planted text in it.
fn agent() -> String {
    format!(
        r#"command:printf 'hi\n' > greeting.txt && printf '{{"summary":"{PLANTED} hello becomes hi","files":["greeting.txt"]}}' > .jaynes/output.json && echo '{{"tokens":1234,"session":"{PLANTED}-session"}}'"#
    )
}

fn start(s: &Scratch, run: &str, labels: &[&str]) {
    let role = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roles/small-change");
    let request = format!("{PLANTED} say hi instead of hello, in /Users/someone/{PLANTED}");
    let agent = agent();
    let mut args = vec!["start", "--run", run, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"), "--request", &request, "--agent", &agent];
    args.extend_from_slice(labels);
    let (ok, out, err) = engine(s, &args);
    assert!(ok, "start failed:\n{out}{err}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_export_of_numbers_carries_no_word_of_the_record() {
    let s = scratch("planted");
    let (one, two) = (format!("{PLANTED}-run-1"), format!("{PLANTED}-run-2"));
    start(&s, &one, &["--task", "T-9", "--arm", "with-jaynes"]);
    start(&s, &two, &[]);
    // the person records that the first delivery was wrong, at a terminal
    let note = format!("{PLANTED} the greeting was wrong");
    let corrected = Command::new("python3")
        .args(["-c", TTY, BIN, "correct", &one, "--note", &note])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .output().expect("correct");
    assert!(corrected.status.success(), "{}", String::from_utf8_lossy(&corrected.stdout));
    // the record holds the planted text, everywhere a run keeps one
    let (ok, log, _) = engine(&s, &["log", &one]);
    assert!(ok && log.matches(PLANTED).count() >= 4, "{log}");
    let (ok, out, err) = engine(&s, &["export", "--numbers"]);
    assert!(ok, "{out}{err}");
    assert!(!out.contains(PLANTED) && !err.contains(PLANTED) && !out.to_lowercase().contains("planted"), "{out}{err}");
    let lines: Vec<serde_json::Value> = out.lines().map(|l| serde_json::from_str(l).expect("a line of JSON")).collect();
    assert_eq!(lines.len(), 2, "{out}");
    // these fields and nothing else (their order is in the line itself, below)
    let mut keys: Vec<&str> = lines[0].as_object().expect("object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["agent_minutes", "arm", "presence_minutes", "reason", "result", "stops", "stops_unmeasured", "task", "tokens"], "{out}");
    assert!(out.lines().next().is_some_and(|l| l.starts_with(r#"{"task":"T-9","arm":"with-jaynes","presence_minutes":null,"stops":0,"stops_unmeasured":0,"result":"corrected","reason":null,"agent_minutes":"#)), "{out}");
    assert_eq!(lines[0]["tokens"], 1234, "{out}");
    // a run with no task's id is named by a fingerprint of a salt kept here, owner only
    let task = lines[1]["task"].as_str().expect("task");
    assert!(task.len() == 12 && task.bytes().all(|b| b.is_ascii_hexdigit()), "{out}");
    assert_eq!((lines[1]["arm"].as_str(), lines[1]["result"].as_str()), (Some("none"), Some("accepted")), "{out}");
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(s.home.join("export.salt")).expect("salt").permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    // the same each time
    let (_, again, _) = engine(&s, &["export", "--numbers"]);
    assert_eq!(again, out);
    // a day: the runs started on it or after
    let (ok, all, _) = engine(&s, &["export", "--numbers", "--since", "1970-01-02"]);
    assert!(ok && all == out, "{all}");
    let (ok, none, _) = engine(&s, &["export", "--numbers", "--since", "2999-01-01"]);
    assert!(ok && none.is_empty(), "{none}");
    let (ok, _, said) = engine(&s, &["export", "--numbers", "--since", "last week"]);
    assert!(!ok && said.contains("--since takes a day as YYYY-MM-DD (UTC)"), "{said}");
    let (ok, _, said) = engine(&s, &["export"]);
    assert!(!ok && said.contains("export what? --numbers"), "{said}");
}

#[test]
fn a_task_or_an_arm_that_is_not_a_label_is_refused_before_a_run_exists() {
    let s = scratch("label");
    for labels in [["--task", "fix the login page"], ["--arm", "a/b"], ["--task", "--arm"]] {
        let role = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roles/small-change");
        let mut args = vec!["start", "--run", "refused", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"), "--request", "x", "--agent", "command:true"];
        args.extend_from_slice(&labels);
        let (ok, out, err) = engine(&s, &args);
        assert!(!ok && err.contains("is a label: 1 to 64 letters, digits and hyphens"), "{labels:?}: {out}{err}");
        assert!(!s.home.join("runs/refused").exists(), "{labels:?}");
    }
    // nothing finished, nothing exported
    let (ok, out, _) = engine(&s, &["export", "--numbers"]);
    assert!(ok && out.is_empty(), "{out}");
}
