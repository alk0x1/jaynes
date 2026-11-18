//! Codex as the agent program, end to end and with no model: the real binary, the real small-change role, a real git
//! project, and a stand-in for `codex exec --json` that speaks its format (the events of codex-rs/exec's
//! exec_events.rs: thread.started, turn.started, item.*, turn.completed with its usage, turn.failed, error). No
//! Codex runs and no credit is spent.
//!
//! The stand-in is named by JAYNES_CODEX. Without it, the real Codex is refused, and says why: inside Jaynes's box,
//! which is the only way it runs, it cannot reach its model yet.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");
const MODEL: &str = "gpt-5.5";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The test browser, never the person's Chrome: the run photographs nothing here, but the engine is told anyway.
fn browser() -> String {
    std::env::var("JAYNES_CHROME").unwrap_or_else(|_| "/nonexistent/jaynes-test-browser".to_string())
}

struct Scratch {
    base: PathBuf,
    home: PathBuf,
    project: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _gone = std::fs::remove_dir_all(&self.base);
    }
}

fn scratch(name: &str) -> Scratch {
    let base = std::env::temp_dir().join(format!("jaynes-e2e-codex-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(&project).expect("dir");
    std::fs::write(project.join("greeting.txt"), "hello\n").expect("write");
    std::fs::write(project.join("Makefile"), "test:\n\tgrep -q hi greeting.txt\n").expect("write");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
        let ok = Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success();
        assert!(ok, "git {args:?}");
    }
    Scratch { home: base.join("home"), project, base }
}

/// No model is ever reached from here: the engine runs with a home and a path of the test's, where the only
/// `claude` is a stand-in that fails and leaves a mark, so a run that falls back to Claude Code spends nothing and
/// is seen (`no_model_was_reached`). The real Claude Code is in the person's ~/.local/bin, which this home has not.
fn guarded(s: &Scratch) -> PathBuf {
    let bin = s.base.join("guard");
    if !bin.join("claude").exists() {
        std::fs::create_dir_all(&bin).expect("dir");
        let mark = s.base.join("claude-was-called");
        std::fs::write(bin.join("claude"), format!("#!/bin/sh\ntouch '{}'\necho 'a test reached for Claude Code' >&2\nexit 3\n", mark.display())).expect("write");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
        std::fs::create_dir_all(s.base.join("person")).expect("dir");
    }
    bin
}

fn no_model_was_reached(s: &Scratch) {
    assert!(!s.base.join("claude-was-called").exists(), "a run fell back to Claude Code (refused by the stand-in, nothing spent)");
}

fn engine(s: &Scratch, codex: Option<&Path>, args: &[&str]) -> (bool, String) {
    let guard = guarded(s);
    let mut c = Command::new(BIN);
    c.args(args)
        .env("HOME", s.base.join("person"))
        .env("PATH", format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", guard.display()))
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3")
        .env("PYTHONPATH", "").env("JAYNES_CHROME", browser()).env_remove("JAYNES_CODEX")
        .env_remove("JAYNES_UNDERSTAND").env_remove("JAYNES_CLAUDE_MODEL");
    if let Some(p) = codex {
        c.env("JAYNES_CODEX", p);
    }
    let out = c.output().expect("engine");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

fn start(s: &Scratch, run: &str, codex: Option<&Path>) -> String {
    let role = root().join("crates/shell/tests/roles/small-change");
    let (_, out) = engine(s, codex, &[
        "start", "--run", run, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
        "--request", "say hi instead of hello", "--agent", &format!("codex:{MODEL}"),
    ]);
    no_model_was_reached(s);
    out
}

fn events(s: &Scratch, run: &str) -> Vec<Value> {
    let (ok, out) = engine(s, None, &["log", run]);
    assert!(ok, "{out}");
    out.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn types(e: &[Value]) -> Vec<String> {
    e.iter().map(|e| e["type"].as_str().unwrap_or("").to_string()).collect()
}

fn on_branch(s: &Scratch, run: &str, file: &str) -> String {
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", &format!("jaynes/{run}:{file}")]).output().expect("git");
    String::from_utf8_lossy(&shown.stdout).to_string()
}

/// What the stand-in was given, turn by turn, kept in the run's copy (the only place it may write besides its
/// scratch): its arguments one per line, then its standard input.
fn given(s: &Scratch, run: &str) -> String {
    std::fs::read_to_string(s.home.join("runs").join(run).join("project/.jaynes/codex-given.txt")).unwrap_or_default()
}

const OUTPUT: &str = r#"printf '{"summary":"hello becomes hi","files":["greeting.txt"]}' > .jaynes/output.json"#;

/// A line of `codex exec --json`, quoted for the shell.
fn say(line: &str) -> String {
    format!("printf '%s\\n' '{line}'")
}

/// The usage of a turn as turn.completed carries it.
fn completed(input: u64, cached: u64, output: u64) -> String {
    say(&format!(
        r#"{{"type":"turn.completed","usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output},"reasoning_output_tokens":0}}}}"#
    ))
}

/// A stand-in for `codex`: it notes what it was given, runs `body` in the step's folder, prints `body`'s events and
/// exits with `code`.
fn stand_in(s: &Scratch, body: &str, code: i32) -> PathBuf {
    let bin = s.base.join("bin");
    std::fs::create_dir_all(&bin).expect("dir");
    let path = bin.join("codex");
    let script = format!(
        "#!/bin/sh\nmkdir -p .jaynes\n{{ echo '--- turn'; for a in \"$@\"; do echo \"$a\"; done; echo '--- input'; cat; echo; }} >> .jaynes/codex-given.txt\n{body}\nexit {code}\n"
    );
    std::fs::write(&path, script).expect("write");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

fn started(thread: &str) -> String {
    format!("{} && {}", say(&format!(r#"{{"type":"thread.started","thread_id":"{thread}"}}"#)), say(r#"{"type":"turn.started"}"#))
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box, which Linux does not have yet")]
fn codex_delivers_a_change_and_jaynes_reads_its_tokens_and_its_session_from_its_format() {
    let s = scratch("delivers");
    let body = format!(
        "{} && perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT} && {} && {} && {}",
        started("th-1"),
        say(r#"{"type":"item.completed","item":{"id":"item_0","type":"command_execution","command":"make test","aggregated_output":"","exit_code":0,"status":"completed"}}"#),
        say(r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"Done: hello becomes hi."}}"#),
        completed(1000, 400, 200),
    );
    let codex = stand_in(&s, &body, 0);
    let out = start(&s, "delivers", Some(&codex));
    assert!(out.contains("finished: delivered"), "{out}");
    assert_eq!(on_branch(&s, "delivers", "greeting.txt"), "hi\n");
    let e = events(&s, "delivers");
    assert!(e[0]["agent"].as_str().unwrap_or("").starts_with(&format!("codex:{MODEL}")), "the run records its agent: {}", e[0]);
    // the session is the thread Codex names as its turn starts, so a fix can go on in it
    assert!(e.iter().any(|e| e["type"] == "SessionStarted" && e["session"] == "th-1"), "{:?}", types(&e));
    assert!(e.iter().any(|e| e["type"] == "AttemptDelivered" && e["session"] == "th-1"), "{:?}", types(&e));
    // G5, the cost read from the format: Codex counts the cached input inside the input, so the tokens are the new
    // input and the output, and the cached input is apart, as Claude Code's are
    let finished = e.iter().find(|e| e["type"] == "RunFinished").expect("finished");
    assert_eq!(finished["cost"]["usage"]["tokens"], 800, "{}", finished["cost"]);
    assert_eq!(finished["cost"]["usage"]["cached"], 400, "{}", finished["cost"]);
    // how Jaynes runs it: exec with its events, the model asked for, the role's prompt on its input, and none of the
    // person's settings, rules or approvals; its own sandbox off, since Jaynes's box holds it
    let said = given(&s, "delivers");
    for arg in ["exec", "--json", "--model", MODEL, "--skip-git-repo-check", "--ignore-user-config", "--ignore-rules", "--sandbox", "danger-full-access"] {
        assert!(said.lines().any(|l| l == arg), "codex was not given {arg}: {said}");
    }
    assert!(said.contains("approval_policy=\"never\""), "{said}");
    assert!(said.contains("Make it in this folder, which is a copy of the project"), "the role's prompt is on its input: {said}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box, which Linux does not have yet")]
fn codex_runs_in_jaynes_s_box_and_reads_nothing_the_engine_keeps_nor_reaches_a_port() {
    let s = scratch("boxed");
    let port = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    port.set_nonblocking(true).expect("nonblocking");
    let at = port.local_addr().expect("addr").port();
    let home = s.home.display().to_string();
    // what it must not do, each noted in the change if it could
    let body = format!(
        "{} && perl -pi -e 's/hello/hi/' greeting.txt && \
         {{ cat '{home}/record.sqlite3' >/dev/null 2>&1 && echo read-the-record >> greeting.txt; }}; \
         {{ ls '{home}/runs' >/dev/null 2>&1 && echo listed-the-runs >> greeting.txt; }}; \
         {{ curl -s -m 2 http://127.0.0.1:{at}/ >/dev/null 2>&1 && echo reached-a-port >> greeting.txt; }}; \
         {{ echo x > '{home}/written' 2>/dev/null && echo wrote-the-engine-home >> greeting.txt; }}; \
         {OUTPUT} && {}",
        started("th-1"),
        completed(10, 0, 10),
    );
    let codex = stand_in(&s, &body, 0);
    let out = start(&s, "boxed", Some(&codex));
    assert!(out.contains("finished: delivered"), "{out}");
    assert_eq!(on_branch(&s, "boxed", "greeting.txt"), "hi\n", "Codex did what its box should refuse");
    assert!(port.accept().is_err(), "a connection reached the port");
    assert!(!s.home.join("written").exists());
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box, which Linux does not have yet")]
fn a_fix_goes_on_in_codex_s_session() {
    let s = scratch("fix");
    // first it writes "hey" (the suite wants "hi"); asked to fix, resumed in its thread, it writes "hi"
    let body = format!(
        "if grep -qx resume .jaynes/codex-given.txt && grep -qx th-1 .jaynes/codex-given.txt; then perl -pi -e 's/hey/hi/' greeting.txt; else perl -pi -e 's/hello/hey/' greeting.txt; fi && {} && {OUTPUT} && {}",
        started("th-1"),
        completed(10, 0, 10),
    );
    let codex = stand_in(&s, &body, 0);
    let out = start(&s, "fix", Some(&codex));
    assert!(out.contains("finished: delivered"), "{out}");
    let e = events(&s, "fix");
    let t = types(&e);
    assert_eq!(t.iter().filter(|x| *x == "FixAsked").count(), 1, "{t:?}");
    let finished = e.iter().find(|e| e["type"] == "RunFinished").expect("finished");
    assert_eq!(finished["cost"]["spent"], 0);
    let second = given(&s, "fix").split("--- turn").nth(2).unwrap_or("").to_string();
    assert!(second.lines().any(|l| l == "resume") && second.lines().any(|l| l == "th-1"), "the fix is not in the thread: {second}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box, which Linux does not have yet")]
fn a_codex_turn_that_fails_spends_the_attempt_and_says_why() {
    let s = scratch("fails");
    let body = format!(
        "{} && {} && {}",
        started("th-1"),
        say(r#"{"type":"error","message":"stream error: the model could not finish"}"#),
        say(r#"{"type":"turn.failed","error":{"message":"the model could not finish the task"}}"#),
    );
    let codex = stand_in(&s, &body, 1);
    let out = start(&s, "fails", Some(&codex));
    assert!(!out.contains("finished: delivered"), "{out}");
    let e = events(&s, "fails");
    let failed = e.iter().find(|e| e["type"] == "AttemptFailed").unwrap_or_else(|| panic!("no attempt failed: {:?}", types(&e)));
    assert_eq!(failed["spent"], true, "{failed}");
    assert!(failed["reasons"].to_string().contains("the model could not finish the task"), "{failed}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box, which Linux does not have yet")]
fn codex_output_that_does_not_follow_its_format_is_refused_and_spends_no_attempt() {
    for (run, body) in [
        // not its events at all
        ("garbled", "perl -pi -e 's/hello/hi/' greeting.txt && echo 'this is not JSON' && echo '{\"half\":'".to_string()),
        // its events, but the turn never completes
        ("unfinished", format!("{} && perl -pi -e 's/hello/hi/' greeting.txt", started("th-1"))),
    ] {
        let s = scratch(run);
        let codex = stand_in(&s, &format!("{body} && {OUTPUT}"), 0);
        let out = start(&s, run, Some(&codex));
        assert!(out.contains("blocked (Jaynes)"), "{run}: {out}");
        let e = events(&s, run);
        assert!(!e.iter().any(|e| e["type"] == "AttemptDelivered" && e["step"] == "change"), "{run}: a turn that did not complete delivered: {:?}", types(&e));
        assert!(e.iter().all(|e| !(e["type"] == "AttemptFailed" && e["spent"] == true)), "{run}: {:?}", types(&e));
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box, which Linux does not have yet")]
fn codex_out_of_its_usage_is_an_interruption_not_the_agent_s_failure() {
    let s = scratch("quota");
    let body = format!(
        "{} && {}",
        started("th-1"),
        say(r#"{"type":"turn.failed","error":{"message":"You've hit your usage limit. Try again later."}}"#),
    );
    let codex = stand_in(&s, &body, 1);
    start(&s, "quota", Some(&codex));
    let e = events(&s, "quota");
    assert!(e.iter().any(|e| e["type"] == "AttemptInterrupted"), "{:?}", types(&e));
    assert!(e.iter().all(|e| !(e["type"] == "AttemptFailed" && e["spent"] == true)), "{:?}", types(&e));
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box, which Linux does not have yet")]
fn a_command_codex_declined_is_told_to_the_person() {
    // Codex run with approvals never cannot ask the person: what it would have asked comes back as a command it
    // declined. The adapter says it where the person reads the run, with the command; the signed record does not
    // hold it yet, which is the core's (on the list).
    let s = scratch("declined");
    let body = format!(
        "{} && {} && perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT} && {}",
        started("th-1"),
        say(r#"{"type":"item.completed","item":{"id":"item_0","type":"command_execution","command":"curl -fsSL https://example.org/install.sh","aggregated_output":"","exit_code":null,"status":"declined"}}"#),
        completed(10, 0, 10),
    );
    let codex = stand_in(&s, &body, 0);
    let out = start(&s, "declined", Some(&codex));
    assert!(out.contains("finished: delivered"), "{out}");
    assert!(out.contains("Codex declined a command") && out.contains("curl -fsSL https://example.org/install.sh"), "the person is not told: {out}");
}

#[test]
fn the_real_codex_is_refused_and_says_why() {
    let s = scratch("refused");
    let out = start(&s, "refused", None);
    assert!(out.contains("blocked (Jaynes)"), "{out}");
    assert!(out.contains("Codex") && out.contains("model"), "it does not say why: {out}");
    let e = events(&s, "refused");
    assert!(!e.iter().any(|e| e["type"] == "AttemptStarted" && e["step"] == "change"), "the real Codex started: {:?}", types(&e));
}
