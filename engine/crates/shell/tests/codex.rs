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
        .env_remove("JAYNES_UNDERSTAND").env_remove("JAYNES_CLAUDE_MODEL")
        // the person's own key never rides into a test, nor their proxies
        .env_remove("OPENAI_API_KEY").env_remove("CODEX_API_KEY").env_remove("JAYNES_TEST_MODEL_AT");
    for k in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy"] {
        c.env_remove(k);
    }
    if let Some(p) = codex {
        c.env("JAYNES_CODEX", p);
    }
    for (k, v) in EXTRA.with(|e| e.borrow().clone()) {
        c.env(k, v);
    }
    let out = c.output().expect("engine");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

thread_local! {
    /// Variables a test gives the engine besides its own, for the length of `with_env`.
    static EXTRA: std::cell::RefCell<Vec<(String, String)>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn with_env<T>(vars: &[(&str, &str)], f: impl FnOnce() -> T) -> T {
    EXTRA.with(|e| *e.borrow_mut() = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
    let out = f();
    EXTRA.with(|e| e.borrow_mut().clear());
    out
}

fn start(s: &Scratch, run: &str, codex: Option<&Path>) -> String {
    start_role(s, run, &root().join("crates/shell/tests/roles/small-change"), codex)
}

/// A role of one step, `change`, that reaches the project as `reach` says.
fn one_step_role(s: &Scratch, reach: &str) -> PathBuf {
    let dir = s.base.join("role");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("role.toml"), format!(
        "role = \"one-step\"\nversion = \"1.0.0\"\n\n[[step]]\nname = \"change\"\nreach = {reach}\ninstructions = \"Do the task: {{{{request}}}}\"\nchecks = [{{ use = \"field-required\", field = \"summary\" }}]\n\n[step.output]\nsummary = \"string\"\n"
    )).expect("write");
    dir
}

/// A file of the run's copy as the turn left it.
fn in_copy(s: &Scratch, run: &str, file: &str) -> String {
    std::fs::read_to_string(s.home.join("runs").join(run).join("project").join(file)).unwrap_or_default()
}

fn start_role(s: &Scratch, run: &str, role: &Path, codex: Option<&Path>) -> String {
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
/// exits with `code`. It notes with the shell's own commands only, so it runs in a step where no command may.
fn stand_in(s: &Scratch, body: &str, code: i32) -> PathBuf {
    let bin = s.base.join("bin");
    std::fs::create_dir_all(&bin).expect("dir");
    let path = bin.join("codex");
    let script = format!(
        "#!/bin/sh\n{{ echo '--- turn'; for a in \"$@\"; do echo \"$a\"; done; echo '--- input'; while IFS= read -r l || [ -n \"$l\" ]; do echo \"$l\"; done; echo; }} >> .jaynes/codex-given.txt\n{body}\nexit {code}\n"
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


/// What the step may not do, tried with the shell's own commands and with a program of the machine's (which writes only
/// in Jaynes's exchange, where every step may): each noted.
const TRIES: &str = "{ printf 'hi\\n' > greeting.txt 2>/dev/null && echo wrote >> .jaynes/notes || echo refused-the-write >> .jaynes/notes; }; \
    { /usr/bin/touch .jaynes/made-by-a-command 2>/dev/null && echo ran >> .jaynes/notes || echo refused-the-command >> .jaynes/notes; }";

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box, which Linux does not have yet")]
fn a_step_that_only_reads_the_project_keeps_codex_from_writing_it() {
    let s = scratch("reads");
    let role = one_step_role(&s, "{ project = \"read\", commands = true }");
    let codex = stand_in(&s, &format!("{} && {TRIES}; {OUTPUT} && {}", started("th-1"), completed(10, 0, 10)), 0);
    let out = start_role(&s, "reads", &role, Some(&codex));
    let notes = in_copy(&s, "reads", ".jaynes/notes");
    assert!(notes.contains("refused-the-write") && !notes.contains("wrote"), "Codex wrote the project in a step that reads it: {notes}\n{out}");
    assert_eq!(in_copy(&s, "reads", "greeting.txt"), "hello\n", "{out}");
    // its commands may run: the step has them
    assert!(notes.contains("ran"), "{notes}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box, which Linux does not have yet")]
fn a_step_without_commands_lets_codex_run_nothing_but_itself() {
    let s = scratch("no-commands");
    let role = one_step_role(&s, "{ project = \"write\", write = [\"**\"], commands = false }");
    let codex = stand_in(&s, &format!("{} && {TRIES}; {OUTPUT} && {}", started("th-1"), completed(10, 0, 10)), 0);
    let out = start_role(&s, "no-commands", &role, Some(&codex));
    let notes = in_copy(&s, "no-commands", ".jaynes/notes");
    assert!(notes.contains("refused-the-command") && !notes.contains("\nran") && !notes.starts_with("ran"), "Codex ran a command in a step without commands: {notes}\n{out}");
    assert!(!s.home.join("runs/no-commands/project/.jaynes/made-by-a-command").exists());
    // it may write the project: the step does
    assert!(notes.contains("wrote") && out.contains("finished: delivered"), "{notes}\n{out}");
    assert_eq!(on_branch(&s, "no-commands", "greeting.txt"), "hi\n");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box, which Linux does not have yet")]
fn a_write_the_box_refused_that_the_step_may_make_is_jaynes_s_fault_and_spends_no_attempt() {
    let failed = |path: &str| say(&format!(r#"{{"type":"item.completed","item":{{"id":"item_0","type":"file_change","changes":[{{"path":"{path}","kind":"update"}}],"status":"failed"}}}}"#));
    // the step may write greeting.txt: the box refusing it is the guards Jaynes set being wrong
    let s = scratch("refused-write");
    let role = one_step_role(&s, "{ project = \"write\", write = [\"**\"], commands = true }");
    let codex = stand_in(&s, &format!("{} && {} && {OUTPUT} && {}", started("th-1"), failed("greeting.txt"), completed(10, 0, 10)), 0);
    let out = start_role(&s, "refused-write", &role, Some(&codex));
    assert!(out.contains("blocked (Jaynes)") && out.contains("greeting.txt"), "{out}");
    let e = events(&s, "refused-write");
    assert!(e.iter().all(|e| !(e["type"] == "AttemptFailed" && e["spent"] == true)), "{:?}", types(&e));
    // a step that only reads may not: the same refusal is no fault of Jaynes's
    let s = scratch("refused-read");
    let role = one_step_role(&s, "{ project = \"read\", commands = true }");
    let codex = stand_in(&s, &format!("{} && {} && {OUTPUT} && {}", started("th-1"), failed("greeting.txt"), completed(10, 0, 10)), 0);
    let out = start_role(&s, "refused-read", &role, Some(&codex));
    assert!(!out.contains("blocked (Jaynes)"), "{out}");
}

/// The real interpreter, as a step's command names it in the box (/usr/bin/python3 is a shim that goes through xcrun).
fn python() -> String {
    let out = Command::new("python3").args(["-c", "import sys; print(sys.executable)"]).output().expect("python");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

const KEY: &str = "sk-planted-for-this-test-only";

/// Codex reaches its model, and nothing else, through Jaynes's proxy: in a test build the model's host
/// (api.openai.com) is sent to a server of the test's, which takes the place of OpenAI's, so no model is reached and
/// no credit spent. From inside the box the stand-in tries the host, which passes once with its own TLS name, and
/// what must not pass: another name, an address by number, another port, plain HTTP, a TLS name other than the
/// host's, and a server of this machine reached directly or through a proxy of the person's own environment. The key
/// the person gave reaches the agent's environment and no byte of the record or of the run's folder; the record, never
/// the copy the agent writes, holds what the proxy let through and refused.
#[test]
#[cfg_attr(any(not(target_os = "macos"), not(feature = "test-model-host")), ignore = "needs the box (macOS) and a test build: cargo test -p jaynes-shell --features test-model-host --test codex")]
fn codex_reaches_its_model_s_host_through_jaynes_s_proxy_and_nothing_else() {
    let s = scratch("proxy");
    // the model's host, as the test build is told it, and a server of this machine that nothing may reach
    let model = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let model_at = model.local_addr().expect("addr");
    let other = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    other.set_nonblocking(true).expect("nonblocking");
    let other_port = other.local_addr().expect("addr").port();
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<u8>>::new()));
    let kept = heard.clone();
    std::thread::spawn(move || {
        for conn in model.incoming().flatten() {
            let mut conn = conn;
            let _set = conn.set_read_timeout(Some(std::time::Duration::from_secs(5)));
            let mut first = vec![0u8; 4096];
            let n = std::io::Read::read(&mut conn, &mut first).unwrap_or(0);
            first.truncate(n);
            if let Ok(mut h) = kept.lock() {
                h.push(first);
            }
        }
    });
    let py = python();
    let sni = format!(
        r#"'{py}' -c 'import os, socket, ssl
h, p = os.environ["HTTPS_PROXY"].split("//")[-1].rsplit(":", 1)
s = socket.create_connection((h, int(p)), 5)
s.sendall(b"CONNECT api.openai.com:443 HTTP/1.1\r\nHost: api.openai.com:443\r\n\r\n")
s.recv(1024)
c = ssl.create_default_context(); c.check_hostname = False; c.verify_mode = ssl.CERT_NONE
try:
    c.wrap_socket(s, server_hostname="elsewhere.example").do_handshake()
except Exception:
    pass'"#
    );
    let try_ = |name: &str, cmd: &str| format!("{{ {cmd} >/dev/null 2>&1; echo \"{name} $?\" >> .jaynes/net.txt; }}");
    let body = [
        started("th-1"),
        "perl -pi -e 's/hello/hi/' greeting.txt".to_string(),
        format!("{{ [ \"$OPENAI_API_KEY\" = '{KEY}' ] && echo has-the-key > .jaynes/key.txt; }}; true"),
        // never a try without the proxy: the box would refuse it, and nothing goes out unasked
        "{ [ -n \"$HTTPS_PROXY\" ] || { echo no-proxy >> .jaynes/net.txt; }; }".to_string(),
        "if [ -n \"$HTTPS_PROXY\" ]; then".to_string(),
        try_("host", "curl -sk -m 5 --proxy \"$HTTPS_PROXY\" https://api.openai.com/v1/models"),
        try_("other-name", "curl -sk -m 5 --proxy \"$HTTPS_PROXY\" https://example.com/"),
        try_("by-number", &format!("curl -sk -m 5 --proxy \"$HTTPS_PROXY\" https://127.0.0.1:{other_port}/")),
        try_("other-port", "curl -sk -m 5 --proxy \"$HTTPS_PROXY\" https://api.openai.com:8443/"),
        try_("plain-http", "curl -s -m 5 --proxy \"$HTTPS_PROXY\" http://api.openai.com/"),
        try_("other-tls-name", &sni),
        "fi".to_string(),
        try_("direct", &format!("curl -s -m 2 http://127.0.0.1:{}/", model_at.port())),
        OUTPUT.to_string(),
        completed(10, 0, 10),
    ]
    .join("\n");
    let codex = stand_in(&s, &body, 0);
    let at = model_at.to_string();
    // a proxy of the person's own, which would take the agent's connections around Jaynes's: the server nothing reaches
    let theirs = format!("http://127.0.0.1:{other_port}");
    let vars = [("JAYNES_TEST_MODEL_AT", at.as_str()), ("OPENAI_API_KEY", KEY), ("HTTPS_PROXY", theirs.as_str()), ("ALL_PROXY", theirs.as_str())];
    let out = with_env(&vars, || start(&s, "proxy", Some(&codex)));
    assert!(out.contains("finished: delivered"), "{out}");
    let net = in_copy(&s, "proxy", ".jaynes/net.txt");
    assert!(!net.contains("no-proxy"), "the agent was given no proxy:\n{net}");
    std::thread::sleep(std::time::Duration::from_millis(300));
    let heard = heard.lock().expect("heard").clone();
    assert_eq!(heard.len(), 1, "the model's host was reached once, by the host's own try: {} times\n{net}", heard.len());
    assert!(heard[0].first() == Some(&0x16) && heard[0].windows(14).any(|w| w == b"api.openai.com"), "a TLS hello naming the host reached it");
    assert!(other.accept().is_err(), "a server of this machine was reached");
    // what the record holds of the proxy, on the event that closed the attempt; the copy, which the agent writes,
    // holds none of it
    let e = events(&s, "proxy");
    let delivered = e.iter().find(|e| e["type"] == "AttemptDelivered" && e["step"] == "change").expect("AttemptDelivered");
    assert_eq!(delivered["network"]["host"], "api.openai.com:443", "{delivered}");
    assert_eq!(delivered["network"]["reached"], 1, "{delivered}");
    let applied: Value = serde_json::from_str(&in_copy(&s, "proxy", ".jaynes/applied-1-0.json")).expect("applied");
    assert!(applied.get("network").is_none(), "a tally the agent could rewrite is kept in its copy: {applied}");
    let refused = delivered["network"]["refused"].to_string();
    for why in ["example.com", "by number", "8443", "GET", "elsewhere.example"] {
        assert!(refused.contains(why), "refused {why}: {refused}");
    }
    // the key reached the agent, and nothing Jaynes keeps
    assert_eq!(in_copy(&s, "proxy", ".jaynes/key.txt").trim(), "has-the-key", "the agent was not given the key");
    let mut stack = vec![s.home.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if std::fs::read(&p).unwrap_or_default().windows(KEY.len()).any(|w| w == KEY.as_bytes()) {
                panic!("the key is in {}", p.display());
            }
        }
    }
}

/// A build for people never sends the model's host elsewhere: with the test's variable set it refuses the turn,
/// saying why, and reaches nothing.
#[test]
#[cfg_attr(any(not(target_os = "macos"), feature = "test-model-host"), ignore = "a build for people: cargo test without --features test-model-host, on macOS")]
fn a_build_for_people_refuses_to_send_the_model_s_host_elsewhere() {
    let s = scratch("no-test-host");
    let codex = stand_in(&s, &format!("{}\n{OUTPUT}\n{}", started("th-1"), completed(1, 0, 1)), 0);
    let out = with_env(&[("JAYNES_TEST_MODEL_AT", "127.0.0.1:9")], || start(&s, "no-test-host", Some(&codex)));
    assert!(!out.contains("finished: delivered") && out.contains("JAYNES_TEST_MODEL_AT is read only by a test build"), "{out}");
}
