//! A role's script check in a real run: the run keeps its own copy of the role under the engine's home, which the
//! box hides from a project's commands, and the check must still run from it. It reads its own copy of the role
//! and nothing else the engine keeps: not another run, not its own run's steps, and it writes none of it.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

struct Scratch {
    base: PathBuf,
    home: PathBuf,
    project: PathBuf,
    role: PathBuf,
}

fn scratch(name: &str) -> Scratch {
    let base = std::env::temp_dir().join(format!("jaynes-script-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let (home, project, role) = (base.join("home"), base.join("project"), base.join("judged"));
    std::fs::create_dir_all(&project).expect("dir");
    std::fs::write(project.join("greeting.txt"), "hello\n").expect("write");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
        assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success());
    }
    Scratch { base, home, project, role }
}

fn engine(s: &Scratch, args: &[&str]) -> (bool, String) {
    let out = Command::new(BIN)
        .args(args)
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
        .output()
        .expect("engine");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

const ROLE: &str = r#"
role = "judged"
version = "1.0.0"

[[step]]
name = "look"
reach = { project = "read" }
instructions = "Look at {{request}}."
checks = [{ script = "checks/judge.py", about = "the output says hello, judged by a module of the role's own, and the box holds" }]

[step.output]
said = "string"
"#;

/// Exit 0 when the output says hello, by a module of the role, and every wall of the box holds; 1 naming each
/// that does not.
const JUDGE: &str = r#"#!/usr/bin/env python3
import json, os, sys
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "lib"))
import words  # a module of the role's own, beside its checks

said = json.load(sys.stdin)["output"]["said"]
role = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
run = os.path.dirname(role)
runs = os.path.dirname(run)
holes = []
if not words.greets(said):
    holes.append(f"the output does not greet: {said}")
# its own run's copy of the role is read, and not written
open(os.path.join(role, "role.toml")).read()
try:
    open(os.path.join(role, "written.txt"), "w").write("x")
    holes.append("wrote into the run's copy of the role")
except OSError:
    pass
# another run, and its own run's steps, are not read
for path in [os.path.join(runs, "r-other", "secret.txt"), os.path.join(run, "steps")]:
    try:
        os.listdir(path) if os.path.isdir(path) else open(path).read()
        holes.append("read " + path)
    except OSError:
        pass
if not os.path.isfile("greeting.txt"):
    holes.append("not run on the project")
print("\n".join(holes))
sys.exit(1 if holes else 0)
"#;

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which is macOS's sandbox-exec")]
fn a_script_check_runs_from_the_run_s_own_copy_of_the_role_and_reads_nothing_else_the_engine_keeps() {
    let s = scratch("judged");
    std::fs::create_dir_all(s.role.join("checks")).expect("dir");
    std::fs::create_dir_all(s.role.join("lib")).expect("dir");
    std::fs::write(s.role.join("role.toml"), ROLE).expect("write");
    std::fs::write(s.role.join("checks/judge.py"), JUDGE).expect("write");
    let script = s.role.join("checks/judge.py");
    let mut mode = std::fs::metadata(&script).expect("meta").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(&script, mode).expect("chmod");
    std::fs::write(s.role.join("lib/words.py"), "def greets(said):\n    return 'hello' in said\n").expect("write");
    // another run's folder, with what no command of this run may read
    std::fs::create_dir_all(s.home.join("runs/r-other")).expect("dir");
    std::fs::write(s.home.join("runs/r-other/secret.txt"), "secret\n").expect("write");
    let agent = r#"printf '{"said":"hello there"}' > .jaynes/output.json && echo '{"tokens":1}'"#;
    let (ok, out) = engine(&s, &["start", "--run", "judged", "--role", s.role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                                 "--request", "the greeting", "--agent", &format!("command:{agent}")]);
    assert!(ok, "{out}");
    assert!(!out.contains("Operation not permitted"), "the box hid the run's own copy of the role from its check:\n{out}");
    assert!(out.contains("✓ look"), "the check ran from the run's copy of the role and passed:\n{out}");
    let (_, log) = engine(&s, &["log", "judged"]);
    let events: Vec<Value> = log.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let judged = events.iter().find(|e| e["type"] == "ChecksJudged").expect("judged");
    assert_eq!((judged["checks"][0]["check"].as_str(), judged["checks"][0]["passed"].as_bool()), (Some("judge"), Some(true)), "{judged}");
    // the run really kept the role under the engine's home, where the box hides the runs
    assert!(s.home.join("runs/judged/role/checks/judge.py").is_file());
    assert!(!s.home.join("runs/judged/role/written.txt").exists());
    let _gone = std::fs::remove_dir_all(Path::new(&s.base));
}
