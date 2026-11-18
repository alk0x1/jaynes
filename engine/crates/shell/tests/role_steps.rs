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
    for part in ["steps", "checks"] {
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
    assert_eq!(started(&e), ["survey", "stamp", "announce"], "{out}");
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

    // the state of the world is proved by a check that asks the world, not by what the script said of itself
    let judged = of(&e, "ChecksJudged", "announce");
    let announced = judged[0]["checks"].as_array().and_then(|c| c.iter().find(|c| c["check"] == "announced")).expect("the check ran");
    assert_eq!((announced["passed"].clone(), announced["blocked"].clone()), (json!(true), json!(false)), "{announced}");
    // each output follows the format its step declares
    for step in ["survey", "stamp", "announce"] {
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
fn a_script_reaches_only_the_local_ports_its_step_names() {
    let world = World::open();
    let s = scratch("ports", &world.address());
    let call = ASK.replace("REGISTRY", &world.address());
    let closed = one_script(&s, "closed", "", &call);
    let out = start(&s, "closed", &closed, "ask");
    assert!(out.contains("blocked (Jaynes)") && out.contains("the box refused"), "a port the step does not name is closed, and the person is told the box refused it: {out}");
    assert!(world.heard.lock().expect("lock").is_empty(), "the registry was reached without being named");
    let open = one_script(&s, "open", &format!("reach = {{ local_ports = [{}] }}", world.port), &call);
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
    // any other failure may have left its mark on the world
    for (name, script) in [("broke", "echo sent; exit 2\n"), ("silent", "exit 0\n")] {
        let role = one_script(&s, name, keys, script);
        let out = start(&s, name, &role, "act");
        assert!(out.contains("blocked (Role)") && out.contains("cannot be undone") && out.contains("may have acted"), "{name}: {out}");
        assert_eq!(started(&events(&s, name)), ["act"], "{name}: what may have acted is not run again by itself");
    }
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
    assert!(out.contains("may have acted"), "{out}");
    std::fs::write(&marker, "").expect("write");
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
    assert_eq!(started(&events(&s, "once")).iter().filter(|step| *step == "send").count(), 1, "{out}");
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
    assert!(out.contains("✔ announce-version 1.0.0 reads: survey → stamp → announce"), "{out}");
    assert!(out.lines().any(|l| l.starts_with("! ") && l.contains("announce cannot be undone") && l.contains("high risk")), "an author is told what the step costs every run: {out}");
    assert!(out.contains("✔ \"Nothing is announced before the changelog has the version's entry\": proved"), "{out}");
}

#[test]
fn jaynes_check_refuses_a_script_step_that_is_not_well_formed_with_its_line() {
    let s = scratch("check-bad", "http://127.0.0.1:1");
    let role = one_script(&s, "bad", "effect = \"permanent\"", "exit 0\n");
    let (ok, out) = engine(&s, &["check", role.to_str().expect("path")]);
    assert!(!ok && out.contains("✘ the role does not read") && out.contains("role.toml:4: step act has the effect \"permanent\": the one effect a step declares is \"irreversible\""), "{out}");
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
}

#[test]
fn a_role_of_scripts_conforms_to_the_protocol() {
    let s = scratch("conform", "http://127.0.0.1:1");
    let (ok, out) = engine(&s, &["conformance", roles().join("announce-version").to_str().expect("path")]);
    assert!(ok, "{out}");
}
