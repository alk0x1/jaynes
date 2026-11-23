//! A step goes on from where it was, unless the person asks it to start over (the tech-lead's cut of a run's cost,
//! 22/11, line 32 of the board). After an attempt that failed, the next starts from the files it left, in its session,
//! told why it failed (the real-user's horta: a second attempt made again what the first had made). Sent back with the
//! person's words, a round starts from the files and the session of its last attempt that delivered (Alan, 21/11: a
//! small change, Go instead of Python, cost more tokens than the whole proposal). Sent back to be done again without a
//! word, it starts from nothing, in a new session. No hidden case ever reaches the files a maker starts from.
//! The agent here is a shell command that notes what each turn finds, so no model is reached.

use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

/// A command at a terminal, as when the person types it, as e2e.rs has it: the program's end is waited for, not the
/// terminal's, so the engine going on with the run after the answer never keeps the test waiting.
const TTY: &str = r#"
import fcntl, os, select, sys, termios, time
typed = os.environ.pop("TTY_TYPED", "")
for tried in range(10):
    try:
        master, slave = os.openpty()
        break
    except OSError as e:
        why = e
        time.sleep(0.2)
else:
    print(f"the test's terminal could not be opened: {why}")
    sys.exit(97)
pid = os.fork()
if pid == 0:
    os.close(master)
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    for fd in (0, 1, 2):
        os.dup2(slave, fd)
    if slave > 2:
        os.close(slave)
    os.execvp(sys.argv[1], sys.argv[1:])
# the slave stays open here too, so what the program wrote before it ended is never thrown away with the terminal
if typed:
    os.write(master, typed.encode())
status = None
while True:
    ready, _, _ = select.select([master], [], [], 0.05)
    if ready:
        data = os.read(master, 65536)
        if data:
            os.write(1, data)
            continue
    if status is not None:
        break
    done, st = os.waitpid(pid, os.WNOHANG)
    if done:
        status = st
sys.exit(os.waitstatus_to_exitcode(status))
"#;

struct Scratch {
    base: PathBuf,
    home: PathBuf,
    project: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _gone = std::fs::remove_dir_all(&self.base);
        }
    }
}

const ROLE: &str = r#"
role = "carry"
version = "1.0.0"

[[step]]
name = "runner"
by = "jaynes"
use = "find-suite"

[[step]]
name = "cases"
hidden = true
reach = { project = "read", commands = true }
about = "Writes the cases the change is judged by."
instructions = "Write the cases of: {{request}}"
checks = ["output-follows-schema", { use = "field-required", field = "cases" }]

[step.output]
cases = { type = "array", max_items = 8, items = { type = "object", fields = { says = "string", run = "string" } } }

[[step]]
name = "change"
needs = ["runner"]
reach = { project = "write", write = ["**"], commands = true }
about = "Makes the change."
instructions = "Make the change: {{request}}"
checks = ["suite-passes", { use = "hidden-cases-pass", name = "proof", reads = ["cases"] }]
stop = { ask = "Keep it?", verdicts = ["keep", "change", "redo"] }
show = { label = "{{summary}}" }

[step.output]
summary = { type = "string", about = "what changed" }

[[promise]]
says = "The agent that changes never sees the cases the change is judged by"
agent_of = "change"
never_sees = "cases"
"#;

/// The agent: the cases step writes a case with a word no maker may ever find; each turn of the change step notes
/// its number and the session it starts (as Claude Code does, on its first line), the session it goes on in, the note a turn before left and whether the case's word is anywhere in its
/// folder, then does what MODE says of that turn.
const AGENT: &str = r#"#!/bin/sh
p="$BASE/prompt-$$"
cat > "$p"
step=$(sed -n 's/^# Your step: //p' "$p" | head -1)
if [ "$step" = cases ]; then
  printf '{"cases":[{"says":"SECRETCASE says hi","run":"grep -q hi greeting.txt"}]}' > .jaynes/output.json
  echo '{"tokens":100,"session":"c1"}'
  exit 0
fi
n=$(( $(cat "$BASE/count" 2>/dev/null || echo 0) + 1 ))
echo $n > "$BASE/count"
echo "{\"type\":\"system\",\"session_id\":\"s$n\"}"
note=none; [ -e note.txt ] && note=$(cat note.txt)
leak=no; grep -rq SECRETCASE . && leak=yes
rejected=no; grep -q '^# Why the last attempt was rejected' "$p" && rejected=yes
left=no; grep -q '^# Your last attempt' "$p" && left=yes
echo "turn=$n session=${JAYNES_SESSION:-none} note=$note greeting=$(cat greeting.txt) leak=$leak rejected=$rejected left=$left" >> "$BASE/seen"
[ -e pyproject.toml ] && echo "turn=$n" >> "$BASE/py"
case "$(sed -n "${n}p" "$BASE/mode")" in
  fail) echo "from turn $n" > note.txt; perl -pi -e 's/hello/hi/' greeting.txt; exit 1 ;;
  deliver) [ -e note.txt ] || echo "from turn $n" > note.txt; perl -pi -e 's/hello/hi/' greeting.txt ;;
  py) printf '[tool.pytest.ini_options]\naddopts = "-q"\n' > pyproject.toml ;;
  unpy) rm -f pyproject.toml; [ -e note.txt ] || echo "from turn $n" > note.txt; perl -pi -e 's/hello/hi/' greeting.txt ;;
esac
printf '{"summary":"hello becomes hi"}' > .jaynes/output.json
echo "{\"tokens\":$(( n == 1 ? 30000 : 2000 )),\"session\":\"s$n\"}"
"#;

/// What each turn of the change step does, one line each: `fail` leaves a note and fails, `deliver` leaves a note
/// unless one is there and delivers, `py` writes a pyproject.toml and delivers with the greeting as it was (its checks
/// refuse it), `unpy` takes the pyproject.toml out and delivers as `deliver` does.
fn scratch(name: &str, mode: &[&str]) -> Scratch {
    let base = std::env::temp_dir().join(format!("jaynes-carry-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(&project).expect("dir");
    std::fs::write(project.join("greeting.txt"), "hello\n").expect("write");
    std::fs::write(project.join("Makefile"), "test:\n\tgrep -q hi greeting.txt\n").expect("write");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
        let ok = Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success();
        assert!(ok, "git {args:?}");
    }
    std::fs::create_dir_all(base.join("carry")).expect("dir");
    std::fs::write(base.join("carry/role.toml"), ROLE).expect("write");
    std::fs::write(base.join("agent.sh"), AGENT).expect("write");
    std::fs::write(base.join("mode"), mode.join("\n") + "\n").expect("write");
    Scratch { home: base.join("home"), project, base }
}

impl Scratch {
    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        c.args(args)
            .env("JAYNES_ENGINE_HOME", &self.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
            .env("JAYNES_CHROME", "/nonexistent/no-test-browser").env("BASE", &self.base);
        c
    }

    fn engine(&self, args: &[&str]) -> String {
        let out = self.command(args).output().expect("engine");
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    }

    /// The person, at a terminal.
    fn person(&self, args: &[&str]) -> String {
        let mut c = Command::new("python3");
        c.args(["-c", TTY, BIN]).args(args)
            .env("JAYNES_ENGINE_HOME", &self.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
            .env("JAYNES_CHROME", "/nonexistent/no-test-browser").env("BASE", &self.base);
        let out = c.output().expect("person");
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    }

    fn start(&self, run: &str) -> String {
        let agent = format!("command:sh {}", self.base.join("agent.sh").display());
        self.engine(&["start", "--run", run, "--role", self.base.join("carry").to_str().expect("path"), "--project",
                      self.project.to_str().expect("path"), "--request", "say hi instead of hello", "--agent", &agent])
    }

    fn events(&self, run: &str) -> Vec<Value> {
        self.engine(&["log", run]).lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    /// What each turn of the change step found, one line each.
    fn seen(&self) -> Vec<String> {
        std::fs::read_to_string(self.base.join("seen")).unwrap_or_default().lines().map(str::to_string).collect()
    }

    fn question(&self, run: &str, nth: usize) -> String {
        let e = self.events(run);
        let opened: Vec<&Value> = e.iter().filter(|e| e["type"] == "QuestionOpened" && e["question"]["step"] == "change").collect();
        opened.get(nth).unwrap_or_else(|| panic!("no question {nth}: {:?}", e.iter().map(|e| e["type"].clone()).collect::<Vec<_>>()))["question"]["id"]
            .as_str().expect("id").to_string()
    }

    /// The person sends the change back with `verdict`; once the next round asked again, what its turn found.
    fn send_back(&self, run: &str, verdict: &str) -> String {
        let q = self.question(run, 0);
        let said = self.person(&["answer", run, &q, &format!("change={verdict}"), "--send"]);
        for _ in 0..150 {
            if self.seen().len() >= 2 && self.events(run).iter().filter(|e| e["type"] == "QuestionOpened" && e["question"]["step"] == "change").count() > 1 {
                return self.seen()[1].clone();
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("the round sent back never asked again: {said}\n{:?}", self.seen());
    }

    fn started_in(&self, run: &str) -> Vec<Value> {
        self.events(run).iter().filter(|e| e["type"] == "AttemptStarted" && e["step"] == "change").map(|e| e["session"].clone()).collect()
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn an_attempt_after_one_that_failed_starts_from_its_files_and_its_session() {
    let s = scratch("failed", &["fail", "deliver"]);
    let out = s.start("failed");
    let seen = s.seen();
    assert_eq!(seen.len(), 2, "{seen:?}\n{out}");
    assert_eq!(seen[0], "turn=1 session=none note=none greeting=hello leak=no rejected=no left=no", "{out}");
    // the second finds the first's files and goes on in its session, told why it failed and that the files are its own
    assert_eq!(seen[1], "turn=2 session=s1 note=from turn 1 greeting=hi leak=no rejected=yes left=yes", "{out}");
    assert!(out.contains("waiting"), "its checks passed and the person is asked: {out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_a_small_change_with_words_goes_on_from_the_proposal_s_files_and_session_for_fewer_tokens() {
    let s = scratch("words", &["deliver", "keep"]);
    let out = s.start("words");
    assert!(out.contains("waiting"), "{out}");
    let next = s.send_back("words", "change:in Go instead of Python");
    assert_eq!(next, "turn=2 session=s1 note=from turn 1 greeting=hi leak=no rejected=no left=no", "{:?}", s.seen());
    let e = s.events("words");
    let spent: Vec<u64> = e.iter().filter(|e| e["type"] == "AttemptDelivered" && e["step"] == "change").map(|e| e["usage"]["tokens"].as_u64().unwrap_or(0)).collect();
    assert_eq!(spent.len(), 2, "{spent:?}");
    assert!(spent[1] < spent[0], "the small change costs less than the proposal it goes on from: {spent:?}");
    assert_eq!(s.started_in("words"), vec![Value::Null, Value::String("s1".into())]);
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_redo_without_words_starts_from_nothing_in_a_new_session() {
    let s = scratch("redo", &["deliver", "deliver"]);
    let out = s.start("redo");
    assert!(out.contains("waiting"), "{out}");
    let next = s.send_back("redo", "redo");
    assert_eq!(next, "turn=2 session=none note=none greeting=hello leak=no rejected=no left=no", "{:?}", s.seen());
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn an_attempt_that_failed_in_a_round_sent_back_with_words_leaves_its_files_to_the_next() {
    // the round sent back lays the proposal's change again, a new commit each time: what its failed attempt left is
    // kept over that state and laid again over the same files, whatever the commit's id
    let s = scratch("again", &["deliver", "fail", "deliver"]);
    let out = s.start("again");
    assert!(out.contains("waiting"), "{out}");
    let q = s.question("again", 0);
    let said = s.person(&["answer", "again", &q, "change=change:keep the note", "--send"]);
    for _ in 0..150 {
        if s.seen().len() >= 3 && s.events("again").iter().filter(|e| e["type"] == "QuestionOpened" && e["question"]["step"] == "change").count() > 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let seen = s.seen();
    assert_eq!(seen.len(), 3, "{said}\n{seen:?}");
    assert_eq!(seen[1], "turn=2 session=s1 note=from turn 1 greeting=hi leak=no rejected=no left=no", "{seen:?}");
    assert_eq!(seen[2], "turn=3 session=s2 note=from turn 2 greeting=hi leak=no rejected=yes left=yes", "{seen:?}");
}

/// What a change touches raises the run's risk only from an attempt its checks accepted (the roles-engineer's reading
/// of the contas, 23/11: the risk went up as the fix delivered, before its checks judged it). A refused attempt that
/// wrote pyproject.toml raises nothing; its files come back to the next attempt (the tech-lead, 22/11), which is
/// measured again: kept and accepted, the risk goes up after its checks; taken out, it stays low and the file is not
/// in what lands.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_refused_attempt_raises_no_risk_and_its_files_raise_it_only_from_a_next_attempt_the_checks_accept() {
    for (run, second, raised) in [("risk-kept", "deliver", true), ("risk-out", "unpy", false)] {
        let s = scratch(run, &["py", second]);
        let out = s.start(run);
        assert!(out.contains("waiting"), "{out}");
        // the second attempt started from the files the refused one left
        assert_eq!(std::fs::read_to_string(s.base.join("py")).unwrap_or_default(), "turn=2\n", "{run}: {:?}", s.seen());
        let e = s.events(run);
        let told: Vec<String> = e
            .iter()
            .filter(|e| ["AttemptDelivered", "ChecksJudged", "AttemptFailed", "StepDone"].contains(&e["type"].as_str().unwrap_or("")) && e["step"] == "change" || e["type"] == "RiskRaised")
            .map(|e| format!("{} {}", e["type"].as_str().unwrap_or(""), e["attempt"].as_u64().map(|a| a.to_string()).unwrap_or_default()).trim().to_string())
            .collect();
        let mut expected = vec!["AttemptDelivered 1", "ChecksJudged 1", "AttemptFailed 1", "AttemptDelivered 2", "ChecksJudged 2", "StepDone"];
        if raised {
            expected.insert(5, "RiskRaised");
        }
        assert_eq!(told, expected, "{run}");
        let done = e.iter().rfind(|e| e["type"] == "StepDone" && e["step"] == "change").expect("done");
        let changed: Vec<&str> = done["changed"].as_array().expect("changed").iter().filter_map(|c| c["what"].as_str()).collect();
        if raised {
            let up = e.iter().find(|e| e["type"] == "RiskRaised").expect("raised");
            assert_eq!(up["to"], "high", "{up}");
            assert!(up["reasons"].to_string().contains("pyproject.toml"), "{up}");
            assert!(changed.contains(&"pyproject.toml"), "{changed:?}");
            continue;
        }
        assert_eq!(changed, ["greeting.txt", "note.txt"], "the file taken out is not in what the step changed");
        // kept by the person, the run's branch carries the greeting and the note, and no pyproject.toml
        let q = s.question(run, 0);
        let said = s.person(&["answer", run, &q, "change=keep", "--send"]);
        for _ in 0..150 {
            if s.events(run).iter().any(|e| e["type"] == "Delivered") {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let lands = Command::new("git").arg("-C").arg(&s.project).args(["diff", "--name-only", "main", &format!("jaynes/{run}")]).output().expect("git");
        assert_eq!(String::from_utf8_lossy(&lands.stdout), "greeting.txt\nnote.txt\n", "{said}");
        assert!(!s.events(run).iter().any(|e| e["type"] == "RiskRaised"), "{run}");
    }
}
