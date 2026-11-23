//! The engine end to end, as a person runs it: the real binary, the real small-change role over the protocol, a
//! real git project, and an agent that is a shell command, so no tokens are spent.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The system's Python: roles need nothing of the Python engine, only the library inside the program.
fn python() -> String {
    "python3".to_string()
}

/// A command run at a terminal, as when the person types it: what follows `tty()` is the program and its arguments.
/// Python's pty is the same on macOS and Linux, where script(1) takes its arguments differently. The terminal is opened
/// here and its program's side stays open here too, so what the program wrote before it ended is read whole and never
/// thrown away with the terminal (a test once read an empty `open-ports` on CI); the program's end is waited for, not
/// the terminal's, so a process it left holding the terminal never keeps the test waiting. When no terminal can be
/// had, the helper says why on its output, where the test's message shows it. What TTY_TYPED holds is typed at the
/// terminal, as the person answers a question.
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

fn tty() -> Command {
    let mut c = Command::new(python());
    c.args(["-c", TTY]).env("JAYNES_CHROME", test_browser().unwrap_or_else(|| PathBuf::from(NO_BROWSER)));
    c
}

/// The engine as the tests run it: it photographs only with a test browser, never the person's own Chrome. Without
/// a test browser it is named one that is not there, so it photographs with none.
fn jaynes() -> Command {
    let mut c = Command::new(BIN);
    c.env("JAYNES_CHROME", test_browser().unwrap_or_else(|| PathBuf::from(NO_BROWSER)));
    c
}

const NO_BROWSER: &str = "/nonexistent/jaynes-test-browser";

/// The test browser the photographs take: JAYNES_CHROME when the one running the tests names one, else Playwright's
/// headless shell, else its Chrome for Testing, the newest first. Never a browser where macOS keeps the person's
/// apps, whatever names it.
fn test_browser() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let mut roots: Vec<PathBuf> = std::env::var("PLAYWRIGHT_BROWSERS_PATH").map(PathBuf::from).into_iter().collect();
    roots.extend([home.join("Library/Caches/ms-playwright"), home.join(".cache/ms-playwright")]);
    test_browser_in(std::env::var("JAYNES_CHROME").ok().filter(|n| !n.is_empty()), &home, &roots)
}

fn test_browser_in(named: Option<String>, home: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let the_person_s = |p: &Path| {
        let real = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        [Path::new("/Applications"), &home.join("Applications")].iter().any(|apps| p.starts_with(apps) || real.starts_with(apps))
    };
    if let Some(named) = named {
        let p = PathBuf::from(named);
        return (p.is_file() && !the_person_s(&p)).then_some(p);
    }
    // (headless shell first, newest version first) among what Playwright installed
    let mut found: Vec<(u8, u32, PathBuf)> = Vec::new();
    for root in roots {
        for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let version = name.rsplit('-').next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
            let (kind, inside): (u8, &[&str]) = if name.starts_with("chromium_headless_shell-") {
                // the layouts Playwright has used: the newer by platform, and the older chrome-mac and chrome-linux
                (0, &["chrome-headless-shell-mac-arm64/chrome-headless-shell", "chrome-headless-shell-mac-x64/chrome-headless-shell",
                      "chrome-headless-shell-linux64/chrome-headless-shell", "chrome-mac/headless_shell", "chrome-linux/headless_shell"])
            } else if name.starts_with("chromium-") {
                (1, &["chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
                      "chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
                      "chrome-linux64/chrome", "chrome-linux/chrome"])
            } else {
                continue;
            };
            found.extend(inside.iter().map(|i| e.path().join(i)).filter(|p| p.is_file()).map(|p| (kind, u32::MAX - version, p)));
        }
    }
    found.sort();
    found.into_iter().map(|(_, _, p)| p).find(|p| !the_person_s(p))
}

#[test]
fn the_tests_photograph_with_a_test_browser_and_never_the_person_s_chrome() {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    // the person's Chrome is never taken, even named
    for apps in [PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"), home.join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome")] {
        assert_eq!(test_browser_in(Some(apps.display().to_string()), &home, &[]), None, "{}", apps.display());
    }
    // what these tests take, here, is a test browser or none
    let chosen = test_browser();
    if let Some(b) = &chosen {
        assert!(!b.starts_with("/Applications") && !b.starts_with(home.join("Applications")), "{}", b.display());
    }
    eprintln!("the test browser here: {}", chosen.map(|b| b.display().to_string()).unwrap_or_else(|| "none, so the photographs are skipped".into()));
}

/// Without a test browser, a test that photographs says it skipped, and is not run on the person's Chrome.
fn skipped_without_a_test_browser() -> bool {
    let none = test_browser().is_none();
    if none {
        eprintln!("skipped: no test browser (JAYNES_CHROME, or Playwright's headless shell or Chrome for Testing: npx playwright install chromium-headless-shell)");
    }
    none
}

/// A port free now, the system's choice, so two runs of these tests at once never ask for the same one.
/// How long a test waits on a page: past it, the test fails saying what answered, instead of holding the queue.
const PAGE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Wait until Jaynes's page answers on `port`: its /api/version, with the record's sequence, and nothing else's.
/// A program of someone else's listening there fails the test at once, saying what it answered.
fn page_up(port: u16) {
    let until = std::time::Instant::now() + PAGE_WAIT;
    let mut said = String::new();
    while std::time::Instant::now() < until {
        if let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            use std::io::{Read, Write};
            let _timed = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
            let _timed = s.set_write_timeout(Some(std::time::Duration::from_secs(2)));
            let _sent = s.write_all(format!("GET /api/version HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes());
            said.clear();
            let _read = s.read_to_string(&mut said);
            if said.starts_with("HTTP/1.1 200") && said.contains("\"seq\"") {
                return;
            }
            assert!(said.is_empty(), "something that is not Jaynes's page answers on port {port}: {}", said.chars().take(300).collect::<String>());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("Jaynes's page did not come up on port {port} within {} s", PAGE_WAIT.as_secs());
}

/// Starts the page on a port the system gives (`--port 0`), reads the port from the first line it says on stderr
/// ("the page: http://localhost:PORT/ ..."), and waits until it answers there. Its other lines go on being read,
/// so the page never blocks on a full pipe.
fn page_on_any_port(cmd: &mut Command) -> (Page, u16) {
    use std::io::BufRead;
    let mut child = cmd.args(["--port", "0"]).stderr(std::process::Stdio::piped()).spawn().expect("page");
    let out = child.stderr.take().expect("what the page says");
    let page = Page(child);
    let (said, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            let _told = said.send(line);
        }
    });
    let until = std::time::Instant::now() + PAGE_WAIT;
    let port = loop {
        let left = until.saturating_duration_since(std::time::Instant::now());
        let line = heard.recv_timeout(left).unwrap_or_else(|e| panic!("the page never said its port within {} s: {e}", PAGE_WAIT.as_secs()));
        if let Some(p) = line.split("the page: http://localhost:").nth(1).and_then(|r| r.split('/').next()).and_then(|p| p.parse::<u16>().ok()) {
            break p;
        }
    };
    page_up(port);
    (page, port)
}

struct Scratch {
    base: PathBuf,
    home: PathBuf,
    project: PathBuf,
}

/// A test's folders go when it passes; a failed one keeps them for whoever reads why, and JAYNES_KEEP_TEST_RUNS=1
/// keeps every one, for the step of the CI that proves the Lean model against the records the runs left.
impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() && std::env::var("JAYNES_KEEP_TEST_RUNS").as_deref() != Ok("1") {
            let _gone = std::fs::remove_dir_all(&self.base);
        }
    }
}

/// A run of these tests that was stopped before its folders went (killed, or kept for the Lean model) leaves them
/// behind: what an earlier run left more than an hour ago is swept here, so a machine that runs the tests often does
/// not fill up.
fn sweep() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
        let mine = format!("-{}", std::process::id());
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with("jaynes-e2e-") || name.ends_with(&mine) {
                continue;
            }
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 3600);
            if old {
                let _gone = std::fs::remove_dir_all(e.path());
            }
        }
    });
}

fn scratch(name: &str) -> Scratch {
    sweep();
    let base = std::env::temp_dir().join(format!("jaynes-e2e-{name}-{}", std::process::id()));
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

fn engine(s: &Scratch, args: &[&str]) -> (bool, String) {
    let out = jaynes()
        .args(args)
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .output()
        .expect("engine");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

fn start(s: &Scratch, run: &str, agent: &str) -> String {
    let role = root().join("crates/shell/tests/roles/small-change");
    let (ok, out) = engine(s, &[
        "start", "--run", run, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
        "--request", "say hi instead of hello", "--agent", &format!("command:{agent}"),
    ]);
    assert!(ok, "start failed:\n{out}");
    out
}

fn events(s: &Scratch, run: &str) -> Vec<Value> {
    let (ok, out) = engine(s, &["log", run]);
    assert!(ok, "{out}");
    out.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn types(e: &[Value]) -> Vec<String> {
    e.iter().map(|e| e["type"].as_str().unwrap_or("").to_string()).collect()
}

const OUTPUT: &str = r#"printf '{"summary":"hello becomes hi","files":["greeting.txt"]}' > .jaynes/output.json"#;

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_small_change_is_delivered_as_a_branch_with_no_stop() {
    let s = scratch("small");
    let prompt = s.home.parent().expect("base").join("prompt.txt");
    let agent = format!("cat > {} && perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT} && echo '{{\"tokens\":1234,\"session\":\"s1\"}}'", prompt.display());
    let out = start(&s, "small", &agent);
    // the role's instructions, with the data Jaynes filled in wrapped in tags
    let said = std::fs::read_to_string(&prompt).expect("prompt");
    assert!(said.contains("<output step=\"runner\" field=\"suite\">make test</output>"), "{said}");
    assert!(said.contains("field-required: the output has summary, not empty") && said.contains("suite-passes: the project's whole suite passes"), "{said}");
    assert!(out.contains("finished: delivered"), "{out}");
    let e = events(&s, "small");
    let t = types(&e);
    assert!(!t.contains(&"QuestionOpened".to_string()), "no stop on a small change: {t:?}");
    assert!(e[0]["agent"].as_str().unwrap_or("").starts_with("command:cat"), "the run records its agent: {}", e[0]);
    // the person's working tree is untouched, and the change is on the run's branch
    assert_eq!(std::fs::read_to_string(s.project.join("greeting.txt")).expect("read"), "hello\n");
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/small:greeting.txt"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "hi\n");
    // G5: the tokens the agent program reported are recorded
    let finished = e.iter().find(|e| e["type"] == "RunFinished").expect("finished");
    assert_eq!(finished["cost"]["usage"]["tokens"], 1234);
    // G15: the record holds
    let (ok, out) = engine(&s, &["verify"]);
    assert!(ok, "{out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_delivery_that_failed_is_made_again_only_by_the_person_s_continue_and_never_by_one_refused() {
    let s = scratch("refused");
    // while the step works, the person checks out the run's branch in a worktree of theirs: git will not fetch into it
    let aside = s.base.join("aside");
    let agent = format!(
        "perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT} && git -C {p} worktree add -q -b jaynes/refused {a} >/dev/null 2>&1",
        p = s.project.display(),
        a = aside.display()
    );
    let out = start(&s, "refused", &agent);
    let tip = || String::from_utf8_lossy(&Command::new("git").arg("-C").arg(&s.project).args(["rev-parse", "jaynes/refused"]).output().expect("git").stdout).trim().to_string();
    let before = tip();
    assert!(!before.is_empty() && aside.exists(), "the worktree is there: {out}");
    let delivered = |s: &Scratch| events(s, "refused").iter().filter(|e| e["type"] == "Delivered").count();
    assert_eq!(delivered(&s), 0, "{out}");
    assert!(events(&s, "refused").iter().any(|e| e["type"] == "Blocked" && e["fault"] == "jaynes"), "{out}");
    // a continue that is not the person's is refused, and the process it started delivers nothing
    let ok = Command::new("git").arg("-C").arg(&s.project).args(["worktree", "remove", "--force"]).arg(&aside).status().expect("git").success();
    assert!(ok);
    let (_, said) = engine(&s, &["continue", "refused"]);
    assert!(said.contains("refused"), "{said}");
    assert_eq!(delivered(&s), 0, "a refused continue delivered: {said}");
    assert_eq!(tip(), before, "a refused continue moved the run's branch");
    // the person's continue, at a terminal, delivers
    let mut c = tty();
    c.args([BIN, "continue", "refused"]).env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "");
    let out = c.output().expect("continue");
    assert_eq!(delivered(&s), 1, "{}", String::from_utf8_lossy(&out.stdout));
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/refused:greeting.txt"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "hi\n");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_failing_suite_is_fixed_in_the_same_session_without_spending_an_attempt() {
    let s = scratch("fix");
    // first the agent writes "hey" (the suite wants "hi"); asked to fix, in its session, it writes "hi"
    let agent = format!(
        "if [ -n \"$JAYNES_SESSION\" ]; then perl -pi -e 's/hey/hi/' greeting.txt; else perl -pi -e 's/hello/hey/' greeting.txt; fi && {OUTPUT} && echo '{{\"session\":\"s1\"}}'"
    );
    let out = start(&s, "fix", &agent);
    assert!(out.contains("finished: delivered"), "{out}");
    let e = events(&s, "fix");
    let t = types(&e);
    assert_eq!(t.iter().filter(|x| *x == "FixAsked").count(), 1, "{t:?}");
    assert_eq!(t.iter().filter(|x| *x == "AttemptStarted").count(), 2, "one attempt for runner, one for change: {t:?}");
    let finished = e.iter().find(|e| e["type"] == "RunFinished").expect("finished");
    assert_eq!(finished["cost"]["spent"], 0);
    assert_eq!(finished["cost"]["fixes"], 1);
}

/// Thread A (Alan, 21/11: "eu queria uma thread de conversa com a jaynes e nao que minha pergunta sumisse"): the agent
/// answers the person in .jaynes/reply.md, beside its output and never inside it, and the attempt records those words
/// as the person reads them; an attempt with no reply records none.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn an_agent_s_reply_beside_its_output_is_recorded_with_the_attempt() {
    let s = scratch("reply");
    let agent = format!(
        "perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT} && printf 'I changed hello to hi.\\n\\n\\n\\nGo would take longer here.\\n' > .jaynes/reply.md && echo '{{\"session\":\"s1\"}}'"
    );
    let out = start(&s, "reply", &agent);
    assert!(out.contains("finished: delivered"), "{out}");
    let e = events(&s, "reply");
    let delivered = e.iter().rfind(|e| e["type"] == "AttemptDelivered" && e["step"] == "change").expect("delivered");
    assert_eq!(delivered["reply"].as_str(), Some("I changed hello to hi.\n\nGo would take longer here."), "{delivered}");
    let s = scratch("no-reply");
    let out = start(&s, "no-reply", &format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT} && echo '{{\"session\":\"s1\"}}'"));
    assert!(out.contains("finished: delivered"), "{out}");
    let e = events(&s, "no-reply");
    let delivered = e.iter().rfind(|e| e["type"] == "AttemptDelivered" && e["step"] == "change").expect("delivered");
    assert!(delivered.get("reply").is_none(), "{delivered}");
}

/// A process the agent left behind, out of its process group (setsid after a double fork), that writes in the copy
/// as soon as Jaynes has measured the attempt: it waits for the engine's index beside the copy to change.
const LATE_WRITER: &str = r#"python3 -c '
import os, sys, time
if os.fork(): sys.exit(0)
os.setsid()
if os.fork(): sys.exit(0)
for fd in (0, 1, 2): os.close(fd)
index = sys.argv[1]
before = os.stat(index).st_mtime_ns
end = time.time() + 20
while time.time() < end and os.stat(index).st_mtime_ns == before:
    time.sleep(0.002)
open("greeting.txt", "w").write("hello\n")
' "../$(basename "$PWD").git/index""#;

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_checks_judge_the_tree_the_step_delivered_and_a_copy_changed_after_it_is_not_delivered() {
    let s = scratch("late");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT} && {LATE_WRITER} && echo '{{\"session\":\"s1\"}}'");
    let out = start(&s, "late", &agent);
    let e = events(&s, "late");
    // the suite judged what the attempt left, hi, and not what the process wrote after it: no fix was asked
    let t = types(&e);
    assert!(!t.contains(&"FixAsked".to_string()), "the check judged the copy as the process left it: {t:?}\n{out}");
    let delivered: Vec<&Value> = e.iter().filter(|e| e["type"] == "AttemptDelivered" && e["step"] == "change").collect();
    let tree = delivered.last().expect("delivered")["tree"].as_str().expect("the attempt's tree is recorded").to_string();
    let judged = e.iter().rfind(|e| e["type"] == "ChecksJudged" && e["step"] == "change").expect("judged");
    for c in judged["checks"].as_array().expect("checks") {
        assert_eq!(c["tree"].as_str(), Some(tree.as_str()), "each check records the tree it judged: {judged}");
    }
    // the copy is not the tree its checks judged: no delivery, Jaynes says which file, and puts the copy back
    assert!(!e.iter().any(|e| e["type"] == "Delivered"), "{t:?}");
    let blocked = e.iter().rfind(|e| e["type"] == "Blocked").expect("blocked");
    assert_eq!(blocked["fault"], "jaynes", "{blocked}");
    assert!(blocked["reason"].as_str().is_some_and(|r| r.contains("greeting.txt")), "{blocked}");
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["rev-parse", "-q", "--verify", "jaynes/late"]).output().expect("git");
    assert!(!shown.status.success(), "no branch was delivered");
    assert_eq!(std::fs::read_to_string(copy_of(&s, "late").join("greeting.txt")).expect("read"), "hi\n", "the copy is back as judged");
    // the person goes on at a terminal, and what is delivered is the tree the checks judged
    let mut c = tty();
    c.args([BIN, "continue", "late"]).env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "");
    let out = c.output().expect("continue");
    let e = events(&s, "late");
    assert!(e.iter().any(|e| e["type"] == "Delivered"), "{}\n{:?}", String::from_utf8_lossy(&out.stdout), types(&e));
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/late:greeting.txt"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "hi\n");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_check_is_not_given_the_files_git_ignores_and_its_failure_names_them() {
    let s = scratch("ignored");
    std::fs::write(s.project.join(".gitignore"), ".env\ndist/\n").expect("write");
    std::fs::write(s.project.join("Makefile"), "test:\n\tgrep -q hi greeting.txt\n\tgrep -q KEY .env\n").expect("write");
    let ok = Command::new("git").arg("-C").arg(&s.project).args(["add", "-A"]).status().expect("git").success()
        && Command::new("git").arg("-C").arg(&s.project).args(["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "reads .env"]).status().expect("git").success();
    assert!(ok);
    // the step writes a .env and a build, both ignored: the delivery carries neither, and the check is given neither
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && echo KEY=1 > .env && mkdir -p dist && echo x > dist/app.js && {OUTPUT} && echo '{{\"session\":\"s1\"}}'");
    let out = start(&s, "ignored", &agent);
    let e = events(&s, "ignored");
    assert!(!e.iter().any(|e| e["type"] == "Delivered"), "{:?}\n{out}", types(&e));
    let said = serde_json::to_string(&e).expect("json");
    assert!(
        said.contains("not given to this check, since git ignores them and the delivery does not carry them: .env, dist/"),
        "{:?}\n{out}",
        e.iter().filter(|e| e["type"] == "FixAsked" || e["type"] == "AttemptFailed").collect::<Vec<_>>()
    );
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_change_to_a_file_that_runs_later_raises_the_risk_and_asks_the_person() {
    let s = scratch("risky");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && printf 'test:\\n\\tgrep -q hi greeting.txt\\n\\n.PHONY: test\\n' > Makefile && {OUTPUT}");
    let out = start(&s, "risky", &agent);
    assert!(out.contains("risk raised to high"), "{out}");
    assert!(out.contains("waiting: delivered; your answers can still correct it"), "{out}");
    // a program without a terminal is an agent: it may not send the change back
    let state: Vec<Value> = events(&s, "risky");
    let q = state.iter().find(|e| e["type"] == "QuestionOpened").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    let (ok, out) = engine(&s, &["answer", "risky", &q, "change=redo:smaller", "--send"]);
    assert!(!ok && out.contains("sending it back is yours"), "{out}");
    // the person, at a terminal, keeps it
    let script = tty()
        .args([BIN, "answer", "risky", &q, "--send"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .output()
        .expect("script");
    let said = String::from_utf8_lossy(&script.stdout);
    assert!(said.contains("is kept") && said.contains("finished: delivered"), "{said}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_person_asks_the_agent_at_a_stop_and_its_answer_joins_the_stop_with_nothing_redone() {
    let s = scratch("ask");
    let prompt = s.base.join("ask-prompt.txt");
    // the stand-in agent answers a question in its reply, and otherwise makes the change
    let agent = format!(
        "cat > {p} && if grep -q 'what if it were Go' {p}; then printf 'Go would be warmer here.' > .jaynes/reply.md && echo '{{\"tokens\":40,\"session\":\"s2\"}}'; \
         else perl -pi -e 's/hello/hi/' greeting.txt && printf 'test:\\n\\tgrep -q hi greeting.txt\\n\\n.PHONY: test\\n' > Makefile && {OUTPUT} && echo '{{\"session\":\"s1\"}}'; fi",
        p = prompt.display()
    );
    let out = start(&s, "ask", &agent);
    assert!(out.contains("waiting: delivered"), "{out}");
    let e = events(&s, "ask");
    let q = e.iter().find(|e| e["type"] == "QuestionOpened").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    let attempts = e.iter().filter(|e| e["type"] == "AttemptStarted").count();
    // a program without a terminal is an agent: asking is the person's
    let (ok, said) = engine(&s, &["ask", "ask", &q, "--text", "what if it were Go?"]);
    assert!(!ok && said.contains("only you ask"), "{said}");
    let script = tty()
        .args([BIN, "ask", "ask", &q, "--text", "what if it were Go?"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .output()
        .expect("script");
    let said = String::from_utf8_lossy(&script.stdout);
    assert!(said.contains("Go would be warmer here."), "{said}{}", String::from_utf8_lossy(&script.stderr));
    let e = events(&s, "ask");
    let answered = e.iter().find(|e| e["type"] == "AgentAnswered").unwrap_or_else(|| panic!("answered: {:?}", types(&e)));
    assert_eq!(answered["text"].as_str(), Some("Go would be warmer here."), "{answered}");
    assert!(answered.get("late").is_none(), "an answer in time: {answered}");
    // nothing redone, nothing chosen: no attempt, no answer, the stop still open
    assert_eq!(e.iter().filter(|e| e["type"] == "AttemptStarted").count(), attempts, "{:?}", types(&e));
    assert!(!e.iter().any(|e| e["type"] == "AnswerRecorded"), "{:?}", types(&e));
    let (_, state) = engine(&s, &["show", "ask"]);
    assert!(state.contains(&q), "{state}");
    assert!(std::fs::read_to_string(&prompt).expect("prompt").contains("what if it were Go?"));

    // the app asks at the same stop as the person, with the paired key's signature over its words
    use ed25519_dalek::{Signer, SigningKey};
    use jaynes_shell::person;
    let me = SigningKey::from_bytes(&[7; 32]);
    let public = person::hex(me.verifying_key().as_bytes());
    person::pair(&s.home, &public).expect("pair");
    // the run's process the ask carries on in reads the test's own variables, as every call of the engine here does
    let exe = s.base.join("jaynes-for-the-app");
    std::fs::write(&exe, format!(
        "#!/bin/sh\nJAYNES_NO_PAGE=1 JAYNES_PYTHON={} PYTHONPATH= JAYNES_CHROME='{}' exec '{BIN}' \"$@\"\n",
        python(), test_browser().unwrap_or_else(|| PathBuf::from(NO_BROWSER)).display()
    )).expect("exe");
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("chmod");
    let store = jaynes_shell::store::Store::open(&s.home.join("record.sqlite3")).expect("store");
    let run = store.state("ask").expect("state").expect("run");
    let words = "and in Rust, what if it were Go?";
    let sig = person::hex(&me.sign(&person::ask_message("ask", &q, None, words)).to_bytes());
    // another key's signature, or one over other words, is refused, and nothing is recorded
    let other = SigningKey::from_bytes(&[9; 32]);
    let theirs = person::hex(&other.sign(&person::ask_message("ask", &q, None, words)).to_bytes());
    let before = events(&s, "ask").len();
    assert!(person::submit_ask(&s.home, &exe, &run, &q, None, words, &public, &theirs).is_err());
    assert!(person::submit_ask(&s.home, &exe, &run, &q, None, "drop the tests", &public, &sig).is_err());
    assert_eq!(events(&s, "ask").len(), before);
    assert_eq!(store.proof_keys("ask").expect("proofs"), vec![]);
    // the signed ask is the person's: the agent answers it, and its proof is kept under the ask, not the stop
    person::submit_ask(&s.home, &exe, &run, &q, None, words, &public, &sig).expect("the signed ask");
    let mut e = events(&s, "ask");
    for _ in 0..300 {
        if e.iter().filter(|e| e["type"] == "AgentAnswered" || e["type"] == "AskFailed").count() == 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        e = events(&s, "ask");
    }
    let asked = e.iter().filter(|e| e["type"] == "AgentAsked").nth(1).unwrap_or_else(|| panic!("asked: {:?}", types(&e)));
    let a2 = format!("{q}-a2");
    assert_eq!((asked["ask"].as_str(), asked["by"]["by"].as_str(), asked["text"].as_str()), (Some(a2.as_str()), Some("person"), Some(words)), "{asked}");
    let answered = e.iter().filter(|e| e["type"] == "AgentAnswered").nth(1).unwrap_or_else(|| panic!("answered: {:?}", types(&e)));
    assert_eq!(answered["text"].as_str(), Some("Go would be warmer here."), "{answered}");
    assert_eq!(store.proof_keys("ask").expect("proofs"), vec![(a2, public)]);
    // the run's story in the app keeps what was asked and what the agent answered, nothing redone, as the page says it
    let told: Vec<String> = jaynes_shell::listing::story(&s.home, "ask").into_iter().flat_map(|(_, said)| said).collect();
    let asked_at = told.iter().position(|l| l == "you asked: what if it were Go?").unwrap_or_else(|| panic!("the story: {told:?}"));
    assert_eq!(told.get(asked_at + 1).map(String::as_str), Some("agent answered, nothing redone: Go would be warmer here."), "{told:?}");
    assert!(told.contains(&format!("you asked: {words}")), "{told:?}");
    assert_eq!(e.iter().filter(|e| e["type"] == "AttemptStarted").count(), attempts, "{:?}", types(&e));
}

/// Alan, 21/11: "eu queria uma thread de conversa com a jaynes e nao que minha pergunta sumisse". He asks at the stop,
/// the agent answers with nothing redone, he sends it back to be redone, and the round that redoes it is told what he asked and
/// what it answered: his question does not vanish.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_question_at_the_stop_and_its_answer_reach_the_round_that_redoes_the_step() {
    let s = scratch("thread");
    let prompt = s.base.join("thread-prompt.txt");
    let asked = "eu queria uma thread de conversa com a jaynes e nao que minha pergunta sumisse";
    let answer = "A pergunta fica no fio, ao lado da parada.";
    // the stand-in answers when it is asked about what it made, and otherwise makes the change
    let agent = format!(
        "cat > {p} && if grep -q '^# A question from the person' {p}; then printf '{answer}' > .jaynes/reply.md && echo '{{\"tokens\":40,\"session\":\"s2\"}}'; \
         else perl -pi -e 's/hello/hi/' greeting.txt && printf 'test:\\n\\tgrep -q hi greeting.txt\\n\\n.PHONY: test\\n' > Makefile && {OUTPUT} && echo '{{\"session\":\"s1\"}}'; fi",
        p = prompt.display()
    );
    let out = start(&s, "thread", &agent);
    assert!(out.contains("waiting: delivered"), "{out}");
    let e = events(&s, "thread");
    let q = e.iter().find(|e| e["type"] == "QuestionOpened").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    let person = |args: &[&str]| {
        tty().arg(BIN).args(args).env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
            .output().expect("person")
    };
    let said = person(&["ask", "thread", &q, "--text", asked]);
    assert!(String::from_utf8_lossy(&said.stdout).contains(answer), "{}", String::from_utf8_lossy(&said.stdout));
    let changed = person(&["answer", "thread", &q, "change=redo:keep the greeting short", "--send"]);
    assert!(changed.status.success(), "{}", String::from_utf8_lossy(&changed.stdout));
    let mut next = String::new();
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let e = events(&s, "thread");
        if e.iter().filter(|e| e["type"] == "QuestionOpened").count() > 1 || e.iter().any(|e| e["type"] == "RunFinished") {
            next = std::fs::read_to_string(&prompt).expect("prompt");
            break;
        }
    }
    let at = next.find("# What the person asked at the stop, and what you answered").unwrap_or_else(|| panic!("the next round: {next}"));
    assert!(next[at..].contains(asked) && next[at..].contains(answer), "{next}");
    assert!(next.contains("keep the greeting short"), "{next}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn an_agent_lets_a_change_that_is_not_small_go_on_and_it_counts_as_the_agent_s() {
    let s = scratch("release");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && printf 'test:\\n\\tgrep -q hi greeting.txt\\n\\n.PHONY: test\\n' > Makefile && {OUTPUT}");
    start(&s, "release", &agent);
    let e = events(&s, "release");
    let q = e.iter().find(|e| e["type"] == "QuestionOpened").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    let (ok, out) = engine(&s, &["answer", "release", &q, "--send"]);
    assert!(ok && out.contains("counts as the agent's") && out.contains("finished: delivered"), "{out}");
    let e = events(&s, "release");
    let answered = e.iter().find(|e| e["type"] == "AnswerRecorded").expect("answered");
    assert_eq!(answered["answer"]["by"]["by"], "agent");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn an_agent_program_that_does_not_start_spends_no_attempt() {
    let s = scratch("fault");
    let out = start(&s, "fault", "echo 'no such program' >&2; exit 3");
    assert!(out.contains("blocked (Jaynes)"), "{out}");
    let e = events(&s, "fault");
    assert!(e.iter().all(|e| !(e["type"] == "AttemptFailed" && e["spent"] == true)), "{e:?}");
}

#[test]
fn the_protocol_schema_on_disk_is_the_one_the_engine_speaks() {
    let s = scratch("schema");
    let (ok, out) = engine(&s, &["schema"]);
    assert!(ok);
    let on_disk = std::fs::read_to_string(root().join("protocol/role-protocol-1.schema.json")).expect("schema file");
    assert_eq!(out.trim(), on_disk.trim(), "regenerate it: jaynes schema > engine/protocol/role-protocol-1.schema.json");
}

#[test]
fn the_small_change_role_conforms() {
    let s = scratch("conform");
    let (ok, out) = engine(&s, &["conformance", root().join("crates/shell/tests/roles/small-change").to_str().expect("path")]);
    assert!(ok, "{out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn an_input_sent_while_another_process_drives_waits_in_the_inbox() {
    let s = scratch("inbox");
    let role = root().join("crates/shell/tests/roles/small-change");
    let agent = format!("sleep 4 && perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let child = jaynes()
        .args(["start", "--run", "inbox", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start");
    std::thread::sleep(std::time::Duration::from_secs(2));
    let (ok, out) = engine(&s, &["halt", "inbox"]);
    assert!(ok && out.contains("recorded for the process that drives the run"), "{out}");
    let done = child.wait_with_output().expect("wait");
    let said = format!("{}{}", String::from_utf8_lossy(&done.stdout), String::from_utf8_lossy(&done.stderr));
    // the halt stopped the attempt under way; nothing was delivered
    assert!(said.contains("(halted)"), "{said}");
    let t = types(&events(&s, "inbox"));
    assert!(t.contains(&"Halted".to_string()), "{t:?}");
    assert!(!t.contains(&"Delivered".to_string()), "nothing is delivered while halted: {t:?}");
}

/// "não consigo saber fácil qual run é de qual projeto" (Alan, 21/11/2026): the title the start gave is what the run
/// is listed by, after its project's name, wherever runs are listed.
#[test]
fn a_run_is_listed_by_its_project_and_the_title_its_start_gave() {
    let s = scratch("titled");
    let role = root().join("crates/shell/tests/roles/small-change");
    let (_started, out) = engine(&s, &[
        "start", "--run", "titled", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
        "--request", "Agreed with Alan on 2026-11-21: say hi instead of hello", "--title", "  Say hi  ", "--agent", "command:true",
    ]);
    let (ok, listed) = engine(&s, &["runs"]);
    assert!(ok, "{listed}");
    let folder = s.project.file_name().expect("folder").to_string_lossy().into_owned();
    assert!(listed.lines().any(|l| l.starts_with("titled ") && l.ends_with(&format!("{folder} · Say hi"))), "{listed}\n{out}");
}

/// Item 10 of 0.2.5: `jaynes runs` said "running" of a run the person halted, where the app said "halted". It says
/// what the app and the page say: halted while its agent works, and "needs you" at a stop, halted or not.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_jaynes_runs_says_halted_of_a_halted_run_as_the_app_does() {
    let s = scratch("runs-halted");
    let role = root().join("crates/shell/tests/roles/small-change");
    let mark = s.base.join("agent-at-work");
    // the stand-in agent waits until the halt stops it
    let agent = format!("echo $$ > {} && exec sleep 20", mark.display());
    // the start this test made, and its stand-in agent, end with the test however it ends: only their own pids
    struct Ends(std::process::Child, PathBuf);
    impl Drop for Ends {
        fn drop(&mut self) {
            let _gone = self.0.kill();
            let _ended = self.0.wait();
            if let Some(pid) = std::fs::read_to_string(&self.1).ok().and_then(|p| p.trim().parse::<u32>().ok()) {
                let _gone = Command::new("kill").arg(pid.to_string()).status();
            }
        }
    }
    let started = jaynes()
        .args(["start", "--run", "slow", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
            "--request", "say hi instead of hello", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .spawn()
        .expect("start");
    let mut started = Ends(started, mark.clone());
    for _ in 0..300 {
        if std::fs::read_to_string(&mark).is_ok_and(|p| !p.trim().is_empty()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(mark.exists(), "the agent never started: {:?}", types(&events(&s, "slow")));
    let state_of = |s: &Scratch| -> String {
        let (ok, listed) = engine(s, &["runs"]);
        assert!(ok, "{listed}");
        listed.lines().find(|l| l.starts_with("slow ")).unwrap_or_else(|| panic!("no slow in {listed}")).to_string()
    };
    let working = state_of(&s);
    assert!(working.contains("  working · step "), "{working}");
    let halted = tty()
        .args([BIN, "halt", "slow"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python())
        .output()
        .expect("halt");
    assert!(halted.status.success(), "{}{}", String::from_utf8_lossy(&halted.stdout), String::from_utf8_lossy(&halted.stderr));
    let _ended = started.0.wait();
    let line = state_of(&s);
    assert!(line.contains("  halted · step ") && !line.contains("running"), "{line}");
    // the same row the app and the page draw
    let rows = jaynes_shell::listing::read(&s.home).expect("listing");
    let row = rows.needs.iter().chain(rows.projects.iter().flat_map(|(_, r)| r)).find(|r| r.id == "slow").expect("row");
    assert_eq!(row.mark, jaynes_shell::listing::Mark::Halted, "{row:?}");
    assert!(line.contains(&row.line), "{line} | {row:?}");
}

/// Where Jaynes has no box for a project's commands, a run never runs them open: it stops at once, says why, and
/// nothing of the project runs, the agent included. No variable opens it: only the person does, with their proof (G18).
#[test]
#[cfg(not(target_os = "macos"))]
fn without_a_box_a_run_stops_at_once_and_nothing_of_the_project_runs() {
    let s = scratch("no-box");
    let ran = s.home.parent().expect("base").join("agent-ran");
    let role = root().join("crates/shell/tests/roles/small-change");
    let started = std::time::Instant::now();
    let out = jaynes()
        .args(["start", "--run", "no-box", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:touch {} && sleep 60", ran.display())])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        // the variable that once ran them unboxed changes nothing
        .env("JAYNES_UNBOXED", "1")
        .output()
        .expect("start");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(started.elapsed() < std::time::Duration::from_secs(30), "the run waited: {:?}\n{said}", started.elapsed());
    assert!(said.contains("blocked") && said.contains("no box for project commands"), "the person is told why: {said}");
    assert!(!said.contains("JAYNES_UNBOXED"), "no variable is offered as a way out: {said}");
    assert!(said.contains("macOS"), "the ways out are said: {said}");
    assert!(!ran.exists(), "the agent ran without a box: {said}");
    let e = events(&s, "no-box");
    let t = types(&e);
    assert!(t.contains(&"Blocked".to_string()), "{t:?}");
    assert!(e.iter().any(|e| e["type"] == "Blocked" && e["fault"] == "jaynes"), "the machine's fault, not the role's: {e:?}");
    assert!(e.iter().filter(|e| e["type"] == "AttemptFailed").all(|e| e["spent"] == false), "a missing box spends no attempt: {e:?}");
    assert!(!t.iter().any(|e| e == "StepDone" || e == "Delivered" || e == "SessionStarted"), "nothing ran: {t:?}");
    // the missing box is the machine's to fix, as a missing Chrome is: not the role's, and no attempt is spent
    assert!(said.contains("No attempt was spent") && !said.contains("the role says"), "{said}");
    assert_eq!(std::fs::read_to_string(s.project.join("greeting.txt")).expect("read"), "hello\n", "the project is as it was");
}

/// The processes whose command line holds `mark`, as ps lists them: only listed, never signalled.
fn running_with(mark: &str) -> Vec<String> {
    let out = Command::new("ps").args(["-axo", "pid=,command="]).output().expect("ps");
    String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.contains(mark) && !l.contains("ps -axo")).map(str::to_string).collect()
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the box for project commands is macOS's sandbox-exec here")]
fn what_a_project_s_command_leaves_running_in_the_box_ends_with_it() {
    // the command runs in a session of its own, so nothing else stops what it leaves behind: its call does, and while
    // it runs its group is noted where the engine looks on a halt
    let base = std::env::temp_dir().join(format!("jaynes-box-left-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let (project, groups) = (base.join("project"), base.join("groups"));
    std::fs::create_dir_all(&project).expect("dir");
    std::fs::write(project.join("README"), "a project\n").expect("write");
    let mark = format!("LEFT-{}", std::process::id());
    let script = format!(r#"
import json, os, sys, threading, time
sys.path.insert(0, {lib:?})
import jaynes_box as b
from pathlib import Path
groups = Path({groups:?})
os.environ["JAYNES_BOX_GROUPS"] = str(groups)
seen, done = set(), threading.Event()
def watch():
    while not done.is_set():
        if groups.is_dir():
            seen.update(os.listdir(groups))
        time.sleep(0.05)
w = threading.Thread(target=watch)
w.start()
r = b.run(Path({project:?}), "perl -e 'sleep 30' {mark} </dev/null >/dev/null 2>&1 &\nsleep 1\necho started", timeout=60)
done.set()
w.join()
print(json.dumps({{"code": r.code, "printed": r.printed.strip(), "seen": sorted(seen), "left": sorted(os.listdir(groups)) if groups.is_dir() else []}}))
"#, lib = root().join("python"), groups = groups, project = project);
    let out = Command::new(python()).args(["-c", &script]).output().expect("python");
    let got: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&out.stderr)));
    assert_eq!((got["code"].as_i64(), got["printed"].as_str()), (Some(0), Some("started")), "{got}");
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert_eq!(running_with(&mark), Vec::<String>::new(), "what the command left running outlived its call");
    assert_eq!(got["seen"].as_array().map(Vec::len), Some(1), "the command's group was not noted while it ran: {got}");
    assert_eq!(got["left"], serde_json::json!([]), "the note outlived the command: {got}");
    let _gone = std::fs::remove_dir_all(&base);
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_halt_stops_a_check_s_command_in_the_box_and_the_check_runs_again_on_resume() {
    let s = scratch("halt-box");
    // the suite passes once the change is made, after 20 s: what a halt stops, if it reaches the box
    let mark = format!("MARK-halt-box-{}", std::process::id());
    std::fs::write(s.project.join("Makefile"), format!("test:\n\tgrep -q hi greeting.txt\n\tperl -e 'sleep 20' {mark}\n")).expect("write");
    for args in [vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "a slow suite"]] {
        assert!(Command::new("git").arg("-C").arg(&s.project).args(&args).status().expect("git").success());
    }
    let role = root().join("crates/shell/tests/roles/small-change");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let mut child = jaynes()
        .args(["start", "--run", "halt-box", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start");
    let waiting = std::time::Instant::now();
    while running_with(&mark).is_empty() {
        if let Ok(Some(_)) = child.try_wait() {
            let done = child.wait_with_output().expect("wait");
            panic!("the run ended before its check's suite ran:\n{}{}", String::from_utf8_lossy(&done.stdout), String::from_utf8_lossy(&done.stderr));
        }
        assert!(waiting.elapsed() < std::time::Duration::from_secs(90), "the check's suite never ran");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let asked = std::time::Instant::now();
    let (ok, out) = engine(&s, &["halt", "halt-box"]);
    assert!(ok, "{out}");
    while !running_with(&mark).is_empty() && asked.elapsed() < std::time::Duration::from_secs(12) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let stopped = asked.elapsed();
    let done = child.wait_with_output().expect("wait");
    let said = format!("{}{}", String::from_utf8_lossy(&done.stdout), String::from_utf8_lossy(&done.stderr));
    assert!(stopped < std::time::Duration::from_secs(10), "the check's command went on after the halt: {stopped:?}\n{said}");
    assert!(said.contains("(halted)"), "{said}");
    let e = events(&s, "halt-box");
    let failed = |e: &[Value]| e.iter().any(|x| x["type"] == "ChecksJudged"
        && x["checks"].as_array().is_some_and(|c| c.iter().any(|c| c["check"] == "suite-passes" && c["passed"] == false)));
    assert!(types(&e).contains(&"Halted".to_string()), "{:?}", types(&e));
    assert!(!failed(&e) && !types(&e).contains(&"AttemptFailed".to_string()), "a halt counted as a failed check: {:?}", types(&e));
    // the person resumes, and the check that the halt stopped runs again
    let out = tty()
        .args([BIN, "resume", "halt-box"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .output()
        .expect("resume");
    let out = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.contains("finished: delivered"), "{out}");
    let e = events(&s, "halt-box");
    assert!(types(&e).contains(&"Resumed".to_string()) && !failed(&e), "{:?}", types(&e));
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_halt_stops_the_agent_at_once_and_its_attempt_is_not_spent() {
    let s = scratch("halt-now");
    let role = root().join("crates/shell/tests/roles/small-change");
    // the first turn would work for a minute; the one after the resume is quick
    let once = s.home.parent().expect("base").join("once");
    let agent = format!("if [ -e {o} ]; then perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}; else touch {o}; sleep 60; fi", o = once.display());
    let child = jaynes()
        .args(["start", "--run", "halt-now", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start");
    // the agent starts within seconds, or the run ended before it could: then say why rather than wait for ever
    let mut child = child;
    let waiting = std::time::Instant::now();
    while !once.exists() {
        if let Ok(Some(_)) = child.try_wait() {
            let done = child.wait_with_output().expect("wait");
            panic!("the run ended before its agent started:\n{}{}", String::from_utf8_lossy(&done.stdout), String::from_utf8_lossy(&done.stderr));
        }
        assert!(waiting.elapsed() < std::time::Duration::from_secs(60), "the agent never started");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let asked = std::time::Instant::now();
    let (ok, out) = engine(&s, &["halt", "halt-now"]);
    assert!(ok, "{out}");
    let done = child.wait_with_output().expect("wait");
    let said = format!("{}{}", String::from_utf8_lossy(&done.stdout), String::from_utf8_lossy(&done.stderr));
    assert!(asked.elapsed() < std::time::Duration::from_secs(10), "the agent went on after the halt: {:?}\n{said}", asked.elapsed());
    assert!(said.contains("(halted)"), "{said}");
    let t = types(&events(&s, "halt-now"));
    assert!(t.contains(&"Halted".to_string()) && t.contains(&"AttemptInterrupted".to_string()), "{t:?}");
    assert!(!t.contains(&"AttemptFailed".to_string()), "a halt spends no attempt: {t:?}");
    // the person resumes, at a terminal (script gives the command one), and the step runs again from where it was
    let out = tty()
        .args([BIN, "resume", "halt-now"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .output()
        .expect("resume");
    let out = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.contains("finished: delivered"), "{out}");
    let t = types(&events(&s, "halt-now"));
    assert!(t.contains(&"Resumed".to_string()) && !t.contains(&"AttemptFailed".to_string()), "{t:?}");
}

/// The real-user's identity run, 22/11: a command outside Jaynes sent its `jaynes start` a SIGTERM, the start died
/// without a word and its agent went on working (and spending) for minutes, while the run said "running" with
/// nobody driving it. A start stopped by a signal does what a halt does: it stops its agent, records the halt as
/// Jaynes's own ("Jaynes stopped while it ran"), and the person resumes it.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_a_start_stopped_by_a_signal_stops_its_agent_and_says_jaynes_stopped_while_it_ran() {
    use std::io::Read;
    for sig in ["TERM", "INT"] {
        let s = scratch(&format!("signal-{sig}"));
        let role = root().join("crates/shell/tests/roles/small-change");
        let mark = s.base.join("agent-at-work");
        // the first turn works on as the real agent did, deaf to the pipe that broke; the one after the resume is quick
        let agent = format!(
            "if [ -s {m} ]; then perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}; else trap '' PIPE; echo $$ > {m}; exec sleep 60; fi",
            m = mark.display()
        );
        // the start this test made, and its stand-in agent, end with the test however it ends: only their own pids
        struct Ends(std::process::Child, PathBuf);
        impl Drop for Ends {
            fn drop(&mut self) {
                let _gone = self.0.kill();
                let _ended = self.0.wait();
                if let Some(pid) = std::fs::read_to_string(&self.1).ok().and_then(|p| p.trim().parse::<u32>().ok()) {
                    let _gone = Command::new("kill").args(["-KILL", &pid.to_string()]).stderr(std::process::Stdio::null()).status();
                }
            }
        }
        let started = jaynes()
            .args(["start", "--run", "stopped", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                "--request", "say hi instead of hello", "--agent", &format!("command:{agent}")])
            .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
            .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
            .spawn()
            .expect("start");
        let mut started = Ends(started, mark.clone());
        let waiting = std::time::Instant::now();
        let agent_pid = loop {
            if let Some(pid) = std::fs::read_to_string(&mark).ok().and_then(|p| p.trim().parse::<u32>().ok()) {
                break pid;
            }
            if let Ok(Some(_)) = started.0.try_wait() {
                panic!("{sig}: the run ended before its agent started: {:?}", types(&events(&s, "stopped")));
            }
            assert!(waiting.elapsed() < std::time::Duration::from_secs(60), "{sig}: the agent never started");
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        let sent = std::time::Instant::now();
        assert!(Command::new("kill").args([&format!("-{sig}"), &started.0.id().to_string()]).status().expect("kill").success());
        let status = started.0.wait().expect("wait");
        let mut said = String::new();
        for pipe in [started.0.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read>), started.0.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read>)].into_iter().flatten() {
            let mut pipe = pipe;
            let _read = pipe.read_to_string(&mut said);
        }
        assert!(sent.elapsed() < std::time::Duration::from_secs(10), "{sig}: the start went on after the signal: {:?}\n{said}", sent.elapsed());
        // its agent does not outlive it
        let alive = || Command::new("kill").args(["-0", &agent_pid.to_string()]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
        assert!(!alive(), "{sig}: the agent went on working after its start was stopped\n{said}");
        assert_ne!(status.code(), Some(0), "{sig}: a start stopped by a signal says so in how it ends\n{said}");
        // the record says Jaynes stopped it, as a halt of Jaynes's own: no attempt spent
        let e = events(&s, "stopped");
        let t = types(&e);
        assert!(t.contains(&"Halted".to_string()), "{sig}: {t:?}\n{said}");
        let halted = e.iter().find(|e| e["type"] == "Halted").expect("halted").to_string();
        assert!(halted.contains("jaynes-was-stopped"), "{sig}: {halted}");
        assert!(e.iter().any(|e| e["type"] == "AttemptInterrupted" && e["reason"] == "Jaynes stopped while it ran"), "{sig}: {t:?}");
        assert!(!t.contains(&"AttemptFailed".to_string()), "{sig}: a stop spends no attempt: {t:?}");
        // `jaynes runs` says it in those words, never "running" with nobody driving it
        let (ok, listed) = engine(&s, &["runs"]);
        assert!(ok, "{listed}");
        let line = listed.lines().find(|l| l.starts_with("stopped ")).unwrap_or_else(|| panic!("no run in {listed}")).to_string();
        assert!(line.contains("Jaynes stopped while it ran") && !line.contains("running"), "{sig}: {line}");
        // and the person resumes it, at a terminal, where it goes on to the end
        let out = tty()
            .args([BIN, "resume", "stopped"])
            .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
            .output()
            .expect("resume");
        let out = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.contains("finished: delivered"), "{sig}: {out}");
    }
}

/// A start killed outright (a SIGKILL, which no process can answer) leaves its run saying "running" with nobody
/// driving it: whoever reads the run says "Jaynes stopped while it ran", and the person resumes it from there, without
/// halting it first.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_a_run_whose_process_is_gone_says_jaynes_stopped_while_it_ran_and_resumes() {
    let s = scratch("killed");
    let role = root().join("crates/shell/tests/roles/small-change");
    let mark = s.base.join("agent-at-work");
    let agent = format!(
        "if [ -s {m} ]; then perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}; else trap '' PIPE; echo $$ > {m}; exec sleep 60; fi",
        m = mark.display()
    );
    struct Ends(std::process::Child, PathBuf);
    impl Drop for Ends {
        fn drop(&mut self) {
            let _gone = self.0.kill();
            let _ended = self.0.wait();
            if let Some(pid) = std::fs::read_to_string(&self.1).ok().and_then(|p| p.trim().parse::<u32>().ok()) {
                let _gone = Command::new("kill").args(["-KILL", &pid.to_string()]).stderr(std::process::Stdio::null()).status();
            }
        }
    }
    let started = jaynes()
        .args(["start", "--run", "killed", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
            "--request", "say hi instead of hello", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .spawn()
        .expect("start");
    let mut started = Ends(started, mark.clone());
    let waiting = std::time::Instant::now();
    while !std::fs::read_to_string(&mark).is_ok_and(|p| !p.trim().is_empty()) {
        assert!(waiting.elapsed() < std::time::Duration::from_secs(60), "the agent never started: {:?}", types(&events(&s, "killed")));
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let line = |s: &Scratch| -> String {
        let (ok, listed) = engine(s, &["runs"]);
        assert!(ok, "{listed}");
        listed.lines().find(|l| l.starts_with("killed ")).unwrap_or_else(|| panic!("no run in {listed}")).to_string()
    };
    // while its process lives, it works
    let working = line(&s);
    assert!(working.contains("  working · step ") && working.contains("running"), "{working}");
    let _killed = started.0.kill();
    let _ended = started.0.wait();
    let gone = line(&s);
    assert!(gone.contains("Jaynes stopped while it ran") && !gone.contains("running"), "{gone}");
    let rows = jaynes_shell::listing::read(&s.home).expect("listing");
    let row = rows.needs.iter().chain(rows.projects.iter().flat_map(|(_, r)| r)).find(|r| r.id == "killed").expect("row");
    assert_eq!(row.mark, jaynes_shell::listing::Mark::Halted, "{row:?}");
    // the person resumes it as it stands: the record says Jaynes stopped it, then that they resumed it
    let out = tty()
        .args([BIN, "resume", "killed"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .output()
        .expect("resume");
    let out = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.contains("finished: delivered"), "{out}");
    let e = events(&s, "killed");
    let halted = e.iter().find(|e| e["type"] == "Halted").map(|h| h.to_string()).unwrap_or_default();
    assert!(halted.contains("jaynes-was-stopped"), "{:?}", types(&e));
    assert!(types(&e).contains(&"Resumed".to_string()) && !types(&e).contains(&"AttemptFailed".to_string()), "{:?}", types(&e));
}

fn bug_project(name: &str) -> Scratch {
    let s = scratch(name);
    std::fs::write(s.project.join("greet.sh"), "echo \"hello $1\"\n").expect("write");
    std::fs::write(s.project.join("Makefile"), "test:\n\tfor t in tests/*.sh; do [ -e \"$$t\" ] || continue; sh \"$$t\" || exit 1; done\n").expect("write");
    for args in [vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "bug"]] {
        assert!(Command::new("git").arg("-C").arg(&s.project).args(&args).status().expect("git").success());
    }
    s
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn fix_bug_proves_the_fix_with_cases_the_maker_never_sees() {
    let s = bug_project("fixbug");
    let prompts = s.home.join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    let agent = root().join("crates/shell/tests/agents/fix-bug.sh");
    let role = root().join("crates/shell/tests/roles/fix-bug");
    let out = jaynes()
        .args(["start", "--run", "fixbug", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "greet should say hi with the name, not hello", "--agent", &format!("command:sh {}", agent.display())])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("PROMPTS", &prompts)
        .output()
        .expect("start");
    let started = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let (_, said) = merge_as_person(&s, "fixbug", fix_bug_env(&s));
    assert!(said.contains("delivered: jaynes/fixbug"), "{started}\n{said}");
    let e = events(&s, "fixbug");
    // the incomplete fix passed the open checks and was caught by the hidden one, spending one attempt
    let failed: Vec<&Value> = e.iter().filter(|e| e["type"] == "AttemptFailed" && e["step"] == "fix").collect();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0]["reasons"][0], "A check you cannot see found that this does not hold: greet says hi with the name");
    // G3: no prompt but the hidden step's own shows the case's command, and no agent was told the check's name
    let mut seen_sentence = false;
    for f in std::fs::read_dir(&prompts).expect("prompts").flatten() {
        let p = std::fs::read_to_string(f.path()).expect("read");
        if p.contains("# Your step: cases") {
            continue;
        }
        assert!(!p.contains("grep -q") && !p.contains("proof"), "a prompt leaked the hidden check:\n{p}");
        seen_sentence |= p.contains("greet says hi with the name");
    }
    assert!(seen_sentence, "the second fix was told the sentence of the failed case");
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/fixbug:greet.sh"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "echo \"hi $1\"\n");
}

// The real-user, 22/11 (r1795330821-771): "O cartão do bloqueio diz só "A check you cannot see found that this does
// not hold". A pessoa não vê o caso". A fix that never passes its hidden case blocks the run, and the card shows the
// person that case, as the check ran it, while no prompt of the agent that fixes ever held its command (G3).
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_a_run_blocked_by_a_hidden_case_shows_the_person_the_case_and_never_the_agent() {
    let s = bug_project("neverright");
    let prompts = s.home.join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    let agent = root().join("crates/shell/tests/agents/fix-bug.sh");
    let role = root().join("crates/shell/tests/roles/fix-bug");
    let out = jaynes()
        .args(["start", "--run", "neverright", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "greet should say hi with the name, not hello", "--agent", &format!("command:sh {}", agent.display())])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("PROMPTS", &prompts)
        .env("NEVER_RIGHT", "1")
        .output()
        .expect("start");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    // the run as the app reads it, and its card as the app lays it out
    let store = jaynes_shell::store::Store::open(&s.home.join("record.sqlite3")).expect("store");
    let run = store.state("neverright").expect("state").unwrap_or_else(|| panic!("no run:\n{said}"));
    let b = jaynes_shell::listing::blocked(&s.home, &run).unwrap_or_else(|| panic!("not blocked: {:?}\n{said}", run.phase));
    assert_eq!(b.why, "“fix” used its 3 attempts. Each time, a case the agent does not see failed.", "{said}");
    assert_eq!(b.cases.len(), 1, "{:?}", b.cases);
    let c = &b.cases[0];
    assert_eq!(c.says, "“greet says hi with the name”");
    assert_eq!(c.written, "Written at “cases”, by the agent there.");
    assert_eq!(c.ran, "Ran: `sh greet.sh ana | grep -q \"hi ana\"`");
    assert_eq!((c.expected.as_str(), c.got.as_str(), c.failed_in.as_str()), ("Expected: exit 0.", "Got: exit 1.", "Failed in 3 of 3 attempts."));
    assert!(b.text().contains("Ran: `sh greet.sh ana | grep -q \"hi ana\"`"), "{}", b.text());
    // kept in the run's folder, beside the folders an agent's step works in
    assert!(s.home.join("runs/neverright/cases/fix/attempt-3.json").is_file());
    // G3: the agent that fixes was told the case's sentence and never its command
    let mut told = 0;
    for f in std::fs::read_dir(&prompts).expect("prompts").flatten() {
        let p = std::fs::read_to_string(f.path()).expect("read");
        if p.contains("# Your step: cases") {
            continue;
        }
        assert!(!p.contains("grep -q") && !p.contains("Ran: `"), "a prompt leaked the hidden case:\n{p}");
        told += usize::from(p.contains("# Your step: fix") && p.contains("greet says hi with the name"));
    }
    assert!(told >= 2, "the fixes after the first were told the case's sentence");
}

// The tech-lead's T3 (29b, 23/11): on the card of a run blocked by a hidden case, the person takes that case out.
// Only the person, at a terminal, and only a case the card shows; the last attempt is judged again without it and
// without the agent, and the run goes on to the merge, which says it; the run's story keeps it.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_the_person_takes_out_a_hidden_case_they_saw_fail_and_the_run_goes_on_judged_without_it() {
    const CASE: &str = "greet says hi with the name";
    let s = bug_project("dropcase");
    let prompts = s.home.join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    let agent = root().join("crates/shell/tests/agents/fix-bug.sh");
    let role = root().join("crates/shell/tests/roles/fix-bug");
    let env = |c: &mut Command| {
        c.env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
            .env("PROMPTS", &prompts).env("NEVER_RIGHT", "1");
    };
    let mut start = jaynes();
    start.args(["start", "--run", "dropcase", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
        "--request", "greet should say hi with the name, not hello", "--agent", &format!("command:sh {}", agent.display())]);
    env(&mut start);
    let out = start.output().expect("start");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(said.contains("blocked"), "{said}");
    let attempts = |s: &Scratch| events(s, "dropcase").iter().filter(|e| e["type"] == "AttemptStarted").count();
    let before = attempts(&s);
    // an agent's shell: refused, nothing recorded
    let (_, refused) = engine(&s, &["drop-case", "dropcase", "--step", "fix", "--case", CASE]);
    assert!(refused.contains("only you take a hidden case out"), "{refused}");
    // a case the card does not show: refused by the shell, before the core
    let mut c = tty();
    c.args([BIN, "drop-case", "dropcase", "--step", "fix", "--case", "greet says nothing"]);
    env(&mut c);
    let other = c.output().expect("drop");
    let other = format!("{}{}", String::from_utf8_lossy(&other.stdout), String::from_utf8_lossy(&other.stderr));
    assert!(other.contains("the card of “fix” shows no case “greet says nothing”"), "{other}");
    assert!(!types(&events(&s, "dropcase")).contains(&"CaseDropped".to_string()), "{other}");
    // the person, at a terminal, with the case as the card shows it, quotes and all
    let mut c = tty();
    c.args([BIN, "drop-case", "dropcase", "--step", "fix", "--case", &format!("“{CASE}”")]);
    env(&mut c);
    let out = c.output().expect("drop");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let e = events(&s, "dropcase");
    let dropped: Vec<&Value> = e.iter().filter(|e| e["type"] == "CaseDropped").collect();
    assert_eq!(dropped.len(), 1, "{said}\n{:?}", types(&e));
    assert_eq!((dropped[0]["by"]["by"].as_str(), dropped[0]["case"].as_str()), (Some("person"), Some(CASE)));
    // judged again without the agent: no attempt started since
    assert_eq!(attempts(&s), before, "an agent worked again\n{said}");
    // the merge says it, before the person merges
    let q = open_merge(&s, "dropcase").unwrap_or_else(|| panic!("the run goes on to the merge: {:?}\n{said}", types(&e)));
    let asked = e.iter().find(|x| x["type"] == "QuestionOpened" && x["question"]["id"] == q.as_str()).expect("asked");
    let about = asked["question"]["about"].as_str().unwrap_or_default();
    assert!(about.contains(&format!("You took out a hidden case after seeing it fail, so the change was judged without it: “{CASE}” (fix).")), "{about}");
    // the run's story keeps it, at its step
    let story = jaynes_shell::listing::story(&s.home, "dropcase");
    let fix = story.iter().find(|(step, _)| step == "fix").map(|(_, said)| said.clone()).unwrap_or_default();
    assert!(fix.iter().any(|l| l == &format!("you took out the hidden case “{CASE}”, after seeing it fail: judged again without it")), "{fix:?}");
}

// The real-user, 22/11 (r1795330821-771): "o Continue devolve as tentativas e bate no mesmo caso de novo". The person
// continues a run blocked at a hidden case with their words: the attempts come back and the agent's next attempt
// reads them. Only the person, at a terminal: a program without one is refused, and the record keeps the words.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_continue_with_the_person_s_words_sends_them_to_the_agent_that_works_again() {
    const WORDS: &str = "The case needs git history the copy lacks: say so in your reply instead of changing greet again";
    let s = bug_project("sendback");
    let prompts = s.home.join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    let agent = root().join("crates/shell/tests/agents/fix-bug.sh");
    let role = root().join("crates/shell/tests/roles/fix-bug");
    let out = jaynes()
        .args(["start", "--run", "sendback", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "greet should say hi with the name, not hello", "--agent", &format!("command:sh {}", agent.display())])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("PROMPTS", &prompts)
        .env("NEVER_RIGHT", "1")
        .output()
        .expect("start");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(said.contains("blocked"), "{said}");
    let before = std::fs::read_dir(&prompts).expect("prompts").count();
    // a program without a terminal: refused, nothing recorded, no agent runs
    let (_, refused) = engine(&s, &["continue", "sendback", "--note", WORDS]);
    assert!(refused.contains("refused: only you"), "{refused}");
    assert_eq!(std::fs::read_dir(&prompts).expect("prompts").count(), before);
    // the person, at a terminal
    let out = tty()
        .args([BIN, "continue", "sendback", "--note", WORDS])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("PROMPTS", &prompts)
        .env("NEVER_RIGHT", "1")
        .output()
        .expect("continue");
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    let continued: Vec<Value> = events(&s, "sendback").into_iter().filter(|e| e["type"] == "Continued").collect();
    assert_eq!(continued.len(), 1, "{said}");
    assert_eq!((continued[0]["by"]["by"].as_str(), continued[0]["words"].as_str()), (Some("person"), Some(WORDS)), "{continued:?}");
    let mut after: Vec<String> = Vec::new();
    for n in before..std::fs::read_dir(&prompts).expect("prompts").count() {
        after.push(std::fs::read_to_string(prompts.join(format!("{n}.txt"))).expect("read"));
    }
    assert!(after.iter().any(|p| p.contains("# Your step: fix") && p.contains(WORDS)), "the next attempt reads the words:\n{said}\n{after:?}");
    // the hidden case still stays hidden (G3)
    assert!(after.iter().all(|p| !p.contains("grep -q")), "{after:?}");
}

fn http(port: u16, raw: &str) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(PAGE_WAIT)).expect("timeout");
    s.set_write_timeout(Some(PAGE_WAIT)).expect("timeout");
    s.write_all(raw.as_bytes()).expect("write");
    let mut out = Vec::new();
    if let Err(e) = s.read_to_end(&mut out) {
        panic!("the page on port {port} did not answer within {} s ({e}); what came: {}", PAGE_WAIT.as_secs(), String::from_utf8_lossy(&out).chars().take(300).collect::<String>());
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A POST from the page's own origin; `me` is "jaynes_key=KEY" for the person, sent as the page's script sends it
/// (the key in its header), or "" for a client without the key.
fn post(port: u16, path: &str, body: &str, me: &str) -> Value {
    let key = me.strip_prefix("jaynes_key=").map(|k| format!("X-Jaynes-Key: {k}\r\n")).unwrap_or_default();
    let raw = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\nContent-Type: application/json\r\n{key}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let out = http(port, &raw);
    serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap_or("")).unwrap_or(Value::Null)
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_page_answers_as_the_person_only_with_its_key() {
    let s = scratch("page");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && printf 'test:\\n\\tgrep -q hi greeting.txt\\n\\n.PHONY: test\\n' > Makefile && {OUTPUT}");
    start(&s, "page", &agent);
    let e = events(&s, "page");
    let q = e.iter().find(|e| e["type"] == "QuestionOpened").expect("asked")["question"].clone();
    let shown = jaynes_core::question::Question::shown(&serde_json::from_value(q.clone()).expect("question")).0;
    let (_page, port) = page_on_any_port(
        jaynes()
        .arg("page")
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("JAYNES_PAGE_KEY", "k3y"),
    );
    // another address is refused, an old link with the key goes on to the fragment, and the question is on the page
    assert!(http(port, &format!("GET / HTTP/1.1\r\nHost: evil.example:{port}\r\n\r\n")).starts_with("HTTP/1.1 403"));
    let keyed = http(port, &format!("GET /?key=k3y HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"));
    assert!(keyed.contains("Location: /#key=k3y\r\n"), "{keyed}");
    let home = http(port, &format!("GET / HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"));
    // the stop says what raised the risk: the Makefile, which runs later outside any box
    assert!(home.contains("This change touches <code>Makefile</code>, which runs later outside any box. Keep it, or have it done again?") && !home.contains("`Makefile`"), "{home}");
    // each run shows its steps as a strip, as the earlier page did
    assert!(home.contains("<div class=\"strip\"><i class=\"\" title=\"runner\"></i><i class=\"waiting\" title=\"change\"></i></div>"), "{home}");
    assert!(home.contains("step 2 of 2") || home.contains("step 1 of 2"), "{home}");
    let id = q["id"].as_str().expect("id");
    // without the key it is an agent: it may not send the change back
    let redo = format!(r#"{{"run":"page","question":"{id}","shown":"{shown}","verdicts":{{"change":"redo"}},"notes":{{"change":"smaller"}}}}"#);
    let r = post(port, "/api/effects", &redo, "");
    assert_eq!(r["ok"], false, "{r}");
    // with the key, the core writes what the answer will do, and the answer is the person's
    let keep = format!(r#"{{"run":"page","question":"{id}","shown":"{shown}","verdicts":{{}},"context":"for accountants"}}"#);
    let r = post(port, "/api/effects", &keep, "jaynes_key=k3y");
    assert!(r["ok"] == true && r["sentences"].to_string().contains("Your context goes to every step after change"), "{r}");
    let r = post(port, "/api/answer", &keep, "jaynes_key=k3y");
    assert_eq!(r["ok"], true, "{r}");
    // the run goes on in its own process and finishes
    let mut finished = false;
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if events(&s, "page").iter().any(|e| e["type"] == "RunFinished") {
            finished = true;
            break;
        }
    }
    assert!(finished, "{:?}", types(&events(&s, "page")));
    let answered = events(&s, "page").into_iter().find(|e| e["type"] == "AnswerRecorded").expect("answered");
    assert_eq!(answered["answer"]["by"]["by"], "person");
    // with nothing waiting, the page opens on home: what was delivered, newest first, each a link to its run
    let home = http(port, &format!("GET / HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"));
    assert!(home.contains("<h2>Delivered</h2>") && home.contains("<a class=\"hrow\" href=\"/run/page\">") && home.contains("Nothing waits for you"), "{home}");
    assert!(http(port, &format!("GET /home HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n")).contains("<a class=\"hrow\" href=\"/run/page\">"));
    assert_eq!(answered["answer"]["context"], "for accountants");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "only macOS's box keeps tests from serving")]
fn at_the_stack_stop_of_a_new_project_the_person_lets_its_tests_start_their_own_test_servers_and_only_with_the_page_s_key() {
    // the tech-lead's T5 (23/11): the first stop of a new project offers its tests their own test servers, marked
    // beforehand when the chosen stack serves HTTP; only the person's proof grants it, for this project, and never
    // the ports of the usual databases and caches
    let s = scratch("grant");
    std::fs::write(s.project.join("Makefile"), "test:\n\tif [ -f serve.py ]; then python3 serve.py; fi\n\tgrep -q hello greeting.txt\n").expect("write");
    for args in [vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "make"]] {
        assert!(Command::new("git").arg("-C").arg(&s.project).args(&args).status().expect("git").success());
    }
    let base = s.home.parent().expect("base").to_path_buf();
    let role = base.join("new-service");
    std::fs::create_dir_all(&role).expect("dir");
    std::fs::write(role.join("role.toml"), r#"
role = "new-service"
version = "1.0.0"

[[step]]
name = "runner"
by = "jaynes"
use = "find-suite"

[[step]]
name = "stack"
reach = { project = "read" }
instructions = "Pick the stack for {{request}}."
stop = { ask = "Which stack?", verdicts = ["keep", "redo"], holds = true, offers = ["test_servers"], offers_default = { test_servers = "serves_http" } }
show = { label = "{{stack}}" }

[step.output]
stack = "string"
serves_http = "boolean"

[[step]]
name = "build"
needs = ["runner", "stack"]
reach = { project = "write", write = ["**"], commands = true }
instructions = "Build {{stack.stack}}."
checks = ["suite-passes"]
"#).expect("write");
    // a role whose default names a field the output does not declare as a boolean is refused
    let wrong = base.join("wrong");
    std::fs::create_dir_all(&wrong).expect("dir");
    let text = std::fs::read_to_string(role.join("role.toml")).expect("read").replace("serves_http = \"boolean\"", "serves_http = \"string\"");
    std::fs::write(wrong.join("role.toml"), text).expect("write");
    let (ok, out) = engine(&s, &["check", wrong.to_str().expect("path")]);
    assert!(!ok && out.contains("offers_default names serves_http, which the step's output does not declare as a boolean"), "{out}");
    // the stack step picks one that serves HTTP; the build adds a test that starts its own server on a port the
    // system picks
    let agent = base.join("agent.sh");
    std::fs::write(&agent, r##"p=$(cat)
case "$p" in
  *"# Your step: stack"*) printf '{"stack":"a Python service","serves_http":true}' > .jaynes/output.json ;;
  *) printf 'import socket\ns = socket.socket()\ns.bind(("127.0.0.1", 0))\ns.listen(1)\n' > serve.py
     printf '{}' > .jaynes/output.json ;;
esac
"##).expect("write");
    let (ok, out) = engine(&s, &[
        "start", "--run", "grant", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
        "--request", "a small service", "--agent", &format!("command:sh {}", agent.display()),
    ]);
    assert!(ok, "{out}");
    let e = events(&s, "grant");
    let q = e.iter().find(|e| e["type"] == "QuestionOpened").expect("asked")["question"].clone();
    assert_eq!(q["step"], "stack", "{q}");
    let shown = jaynes_core::question::Question::shown(&serde_json::from_value(q.clone()).expect("question")).0;
    let id = q["id"].as_str().expect("id");
    let (_page, port) = page_on_any_port(
        jaynes()
            .arg("page")
            .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
            .env("JAYNES_PYTHON", python())
            .env("PYTHONPATH", "")
            .env("JAYNES_PAGE_KEY", "k3y"),
    );
    // the stop offers the box, marked beforehand: the stack serves HTTP
    let store = jaynes_shell::store::Store::open(&s.home.join("record.sqlite3")).expect("store");
    let stop = jaynes_shell::stop::waiting(&s.home, &store.state("grant").expect("state").expect("run")).expect("waits");
    assert!(stop.offers.len() == 1 && stop.offers[0].marked && stop.offers[0].marked_for.is_empty(), "{:?}", stop.offers);
    drop(store);
    let grant = format!(r#"{{"run":"grant","action":"grant","question":"{id}","offer":"test_servers"}}"#);
    // without the key it is an agent's, and refused
    let r = post(port, "/api/act", &grant, "");
    assert!(r["ok"] == false && r.to_string().contains("only you let this project's tests start their own test servers"), "{r}");
    assert!(events(&s, "grant").iter().all(|e| e["type"] != "Granted"));
    // with the key, the person's: then their answer
    let r = post(port, "/api/act", &grant, "jaynes_key=k3y");
    assert_eq!(r["ok"], true, "{r}");
    let keep = format!(r#"{{"run":"grant","question":"{id}","shown":"{shown}","verdicts":{{}}}}"#);
    let r = post(port, "/api/answer", &keep, "jaynes_key=k3y");
    assert_eq!(r["ok"], true, "{r}");
    let mut e = Vec::new();
    for _ in 0..150 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        e = events(&s, "grant");
        if e.iter().any(|e| e["type"] == "RunFinished") {
            break;
        }
    }
    let granted: Vec<&Value> = e.iter().filter(|e| e["type"] == "Granted").collect();
    assert_eq!(granted.len(), 1, "{:?}", types(&e));
    assert_eq!((granted[0]["offer"].as_str(), granted[0]["by"]["by"].as_str()), (Some("test_servers"), Some("person")), "{granted:?}");
    // the run records it as jaynes project test-servers does: test servers, not every port, no database port freed
    let chosen = e.iter().find(|e| e["type"] == "PortsChosen").unwrap_or_else(|| panic!("ports recorded: {:?}", types(&e)));
    assert!(chosen["servers"] == true && chosen["any"] == false && chosen.get("freed_ports").is_none(), "{chosen}");
    let finished = e.iter().find(|e| e["type"] == "RunFinished").unwrap_or_else(|| panic!("{:?}", types(&e)));
    assert_eq!(finished["outcome"], "delivered", "{finished}");
    // the build's suite served in the box on its first attempt
    assert_eq!(e.iter().filter(|e| e["type"] == "AttemptStarted" && e["step"] == "build").count(), 1, "{:?}", types(&e));
    // the project's choice stays, as the command's would
    let prefs = jaynes_shell::project_prefs::load(&s.home, &s.project).expect("prefs").ports.expect("ports");
    assert!(prefs.servers && !prefs.any && prefs.freed_ports.is_empty(), "{prefs:?}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "only macOS's box keeps tests from serving")]
fn at_a_stop_that_chooses_a_stack_the_test_servers_box_is_marked_as_the_stack_the_person_picks_says() {
    // the roles-engineer's scaffold (23/11): the stack stop chooses one of up to three, and only the chosen stack
    // says whether the project serves HTTP; the box follows the item the person picks
    let s = scratch("grant-each");
    let base = s.home.parent().expect("base").to_path_buf();
    let role = base.join("stacks");
    std::fs::create_dir_all(&role).expect("dir");
    let toml = r#"
role = "stacks"
version = "1.0.0"

[[step]]
name = "stack"
reach = { project = "read" }
instructions = "Propose stacks for {{request}}."
stop = { ask = "Which stack?", choose = true, holds = true, offers = ["test_servers"], offers_default = { test_servers = "stacks.serves_http" } }
show = { each = "stacks", id = "{{name}}", label = "{{name}}" }

[step.output]
stacks = { type = "array", max_items = 3, items = { type = "object", fields = { name = "string", serves_http = "boolean" } } }
"#;
    std::fs::write(role.join("role.toml"), toml).expect("write");
    // a field the items do not declare as a boolean, or of a list the stop does not show, is refused
    for (name, text, says) in [
        ("not-bool", toml.replace("serves_http = \"boolean\"", "serves_http = \"string\""), "offers_default names stacks.serves_http, which the items of stacks do not declare as a boolean"),
        ("not-shown", toml.replace("\"stacks.serves_http\"", "\"others.serves_http\""), "offers_default names others.serves_http, a field of others, which this stop does not show"),
    ] {
        let dir = base.join(name);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("role.toml"), text).expect("write");
        let (ok, out) = engine(&s, &["check", dir.to_str().expect("path")]);
        assert!(!ok && out.contains(says), "{name}: {out}");
    }
    let agent = r#"printf '{"stacks":[{"name":"Go with templates","serves_http":true},{"name":"A command line tool","serves_http":false}]}' > .jaynes/output.json"#;
    let (ok, out) = engine(&s, &[
        "start", "--run", "each", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
        "--request", "a small tool", "--agent", &format!("command:{agent}"),
    ]);
    assert!(ok, "{out}");
    let store = jaynes_shell::store::Store::open(&s.home.join("record.sqlite3")).expect("store");
    let run = store.state("each").expect("state").expect("run");
    let stop = jaynes_shell::stop::waiting(&s.home, &run).expect("waits at the stack stop");
    assert_eq!(stop.offers.len(), 1, "{stop:?}");
    let offered = &stop.offers[0];
    assert_eq!(offered.offer, jaynes_core::manifest::Offer::TestServers);
    assert!(offered.marked, "the first stack serves HTTP: {offered:?}");
    let each: Vec<(&str, bool)> = offered.marked_for.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    assert_eq!(each, [("A command line tool", false), ("Go with templates", true)], "{offered:?}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn sending_an_earlier_step_back_reruns_it_without_the_later_steps_changes() {
    let s = bug_project("redo");
    let prompts = s.home.join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    let agent = root().join("crates/shell/tests/agents/fix-bug.sh");
    let role = root().join("crates/shell/tests/roles/fix-bug");
    let env = |c: &mut Command| {
        c.env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "").env("PROMPTS", &prompts);
    };
    let mut c = jaynes();
    c.args(["start", "--run", "redo", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
            "--request", "greet should say hi with the name", "--agent", &format!("command:sh {}", agent.display())]);
    env(&mut c);
    assert!(c.output().expect("start").status.success());
    let q = events(&s, "redo").into_iter().find(|e| e["type"] == "QuestionOpened" && e["question"]["step"] == "reproduce").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    // the person, at a terminal, sends reproduce back after the fix was made
    let mut c = tty();
    c.args([BIN, "answer", "redo", &q, "reproduce=redo:also an empty name", "--send"]);
    env(&mut c);
    let mut said = String::from_utf8_lossy(&c.output().expect("answer").stdout).to_string();
    // the first delivery waited at its merge, and the person's answer to reproduce replaced it: the fix made again
    // lands only once the person merges it
    said.push_str(&merge_as_person(&s, "redo", env).1);
    let e = events(&s, "redo");
    let t = types(&e);
    // reproduce reran over the base, where the bug still is, and passed; fix ran again on top and was delivered
    let reproduce_failed = e.iter().filter(|e| e["type"] == "AttemptFailed" && e["step"] == "reproduce").count();
    assert_eq!(reproduce_failed, 0, "{said}\n{t:?}");
    assert!(e.iter().filter(|e| e["type"] == "StepDone" && e["step"] == "reproduce").count() == 2, "{t:?}");
    assert!(e.iter().filter(|e| e["type"] == "StepDone" && e["step"] == "fix").count() == 2, "{t:?}");
    // delivered once: the first delivery never went out, since it waited at its merge when reproduce was sent back
    let delivered = e.iter().filter(|e| e["type"] == "Delivered").count();
    assert_eq!(delivered, 1, "{t:?}");
    {
        let (file, want) = ("greet.sh", "echo \"hi $1\"\n");
        let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", &format!("jaynes/redo:{file}")]).output().expect("git");
        assert_eq!(String::from_utf8_lossy(&shown.stdout), want);
    }
    for test in ["tests/test_hi.sh", "tests/test_empty.sh"] {
        let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", &format!("jaynes/redo:{test}")]).output().expect("git");
        assert!(shown.status.success(), "{test} is in the delivery");
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_crash_in_the_middle_of_a_turn_stops_the_agent_left_running_and_spends_no_attempt() {
    let s = scratch("crash");
    let mark = s.home.join("mark");
    std::fs::create_dir_all(&mark).expect("dir");
    let agent = format!(
        "if [ ! -e {m}/started ]; then echo '{{\"type\":\"system\",\"session_id\":\"s9\",\"model\":\"m9\",\"permissionMode\":\"default\",\"tools\":[\"Read\",\"Bash\"]}}'; touch {m}/started; sleep 20; touch {m}/orphan-finished; exit 0; fi; echo \"$JAYNES_SESSION\" > {m}/resumed; perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}",
        m = mark.display()
    );
    let role = root().join("crates/shell/tests/roles/small-change");
    let mut engine_process = jaynes()
        .args(["start", "--run", "crash", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start");
    for _ in 0..100 {
        if mark.join("started").exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(mark.join("started").exists(), "the agent started");
    // the engine dies at once, as with kill -9: nothing of it runs any more, and its agent goes on
    let _killed = engine_process.kill();
    let _waited = engine_process.wait();
    let pidfile = s.home.join("runs/crash/agent.pid");
    let pgid = std::fs::read_to_string(&pidfile).expect("the agent's process group is noted");
    let alive = |g: &str| Command::new("kill").args(["-0", "--", &format!("-{}", g.trim())]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    assert!(alive(&pgid), "the agent outlived the engine");
    // the next process stops what was left, and the run goes on with an attempt that is not counted against the agent
    let (ok, out) = engine(&s, &["continue", "crash"]);
    assert!(ok && out.contains("stopped an agent program") && out.contains("finished: delivered"), "{out}");
    assert!(!alive(&pgid), "the agent left running was stopped");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!mark.join("orphan-finished").exists(), "the stopped agent never finished its work");
    // the session was recorded as the turn started, and the next attempt went on in it
    assert_eq!(std::fs::read_to_string(mark.join("resumed")).expect("resumed").trim(), "s9");
    // what the agent program said it runs with, in its first message, is in the record
    let started = events(&s, "crash").into_iter().find(|e| e["type"] == "SessionStarted").expect("session");
    assert_eq!((started["confirmed"]["model"].as_str(), started["confirmed"]["tools"][1].as_str()), (Some("m9"), Some("Bash")), "{started}");
    let e = events(&s, "crash");
    assert!(e.iter().any(|e| e["type"] == "AttemptInterrupted"), "{:?}", types(&e));
    let finished = e.iter().find(|e| e["type"] == "RunFinished").expect("finished");
    assert_eq!(finished["cost"]["spent"], 0);
}

#[test]
fn no_run_goes_on_over_a_record_that_was_changed() {
    let s = scratch("tamper");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    start(&s, "tamper", &agent);
    let db = s.home.join("record.sqlite3");
    let ok = Command::new("sqlite3")
        .arg(&db)
        .arg("update events set body = replace(body, 'say hi instead of hello', 'push to production') where type = 'RunStarted'")
        .status()
        .expect("sqlite3");
    assert!(ok.success());
    let (ok, out) = engine(&s, &["verify"]);
    assert!(!ok && out.contains("changed at entry 1"), "{out}");
    let (ok, out) = engine(&s, &["continue", "tamper"]);
    assert!(!ok && out.contains("drives no run over a record that does not hold"), "{out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_page_halts_resumes_and_records_a_correction() {
    let s = scratch("acts");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && printf 'test:\\n\\tgrep -q hi greeting.txt\\n\\n.PHONY: test\\n' > Makefile && {OUTPUT}");
    start(&s, "acts", &agent);
    let (_page, port) = page_on_any_port(
        jaynes()
        .arg("page")
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("JAYNES_PAGE_KEY", "k3y"),
    );
    let me = "jaynes_key=k3y";
    let view = http(port, &format!("GET /run/acts HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {me}\r\n\r\n"));
    assert!(view.contains("class=\"stages told\"") && view.contains("waiting for you"), "{view}");
    // what the run waits for is the first thing to do there: a way to the question, apart from the run's controls
    assert!(view.contains("<a class=\"answer\" href=\"/q/acts/"), "{view}");
    // a run that waits for the person has nothing at work to halt and is far from its ceiling: only abandoning is offered
    assert!(view.contains("data-act=\"abandon\""), "{view}");
    assert!(!view.contains("data-act=\"halt\"") && !view.contains("data-act=\"raise-ceiling\""), "{view}");
    // what each check said, and where its answer comes from
    assert!(view.contains("suite-passes (open, read runner) passed"), "{view}");
    // what the step changed, line by line, as git shows it
    assert!(view.contains("Changes made at “change” · 2 files") && view.contains("<span class=\"del\"><i>-</i>hello") && view.contains("<span class=\"file\">greeting.txt</span>"), "{view}");
    // an agent may halt, only the person resumes
    assert_eq!(post(port, "/api/act", r#"{"run":"acts","action":"halt"}"#, "")["ok"], true);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(post(port, "/api/act", r#"{"run":"acts","action":"resume"}"#, "")["ok"], false);
    let halted = http(port, &format!("GET /run/acts HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {me}\r\n\r\n"));
    assert!(halted.contains("data-act=\"resume\""), "{halted}");
    assert_eq!(post(port, "/api/act", r#"{"run":"acts","action":"resume"}"#, me)["ok"], true);
    std::thread::sleep(std::time::Duration::from_millis(300));
    // a correction of the delivery, on the step that erred
    let r = post(port, "/api/act", r#"{"run":"acts","action":"correct","note":"the Makefile lost its tests","step":"change"}"#, me);
    assert_eq!(r["ok"], true, "{r}");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let version = http(port, &format!("GET /api/version HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"));
    assert!(version.contains("\"seq\""), "{version}");
    let t = types(&events(&s, "acts"));
    for want in ["Halted", "Resumed", "DeliveryCorrected"] {
        assert!(t.contains(&want.to_string()), "{want} missing: {t:?}");
    }
}

struct Mcp {
    child: std::process::Child,
    out: std::io::BufReader<std::process::ChildStdout>,
    n: u64,
}

impl Mcp {
    fn call(&mut self, method: &str, params: Value) -> Value {
        use std::io::{BufRead, Write};
        self.n += 1;
        let msg = serde_json::json!({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params});
        let stdin = self.child.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{msg}").expect("write");
        stdin.flush().expect("flush");
        let mut line = String::new();
        self.out.read_line(&mut line).expect("read");
        serde_json::from_str(&line).expect("json")
    }
    fn permit(&mut self, tool: &str, input: Value) -> Value {
        let r = self.call("tools/call", serde_json::json!({"name": "permit", "arguments": {"tool_name": tool, "input": input}}));
        serde_json::from_str(r["result"]["content"][0]["text"].as_str().expect("text")).expect("decision")
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_leave_the_agent_asks_mid_turn_is_granted_by_a_rule_or_by_the_person() {
    let s = scratch("leave");
    let mark = s.home.join("mark");
    std::fs::create_dir_all(&mark).expect("dir");
    // the agent works for a while: the leave server is asked while its turn runs
    let agent = format!("touch {}/started; sleep 8; perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}", mark.display());
    let role = root().join("crates/shell/tests/roles/small-change");
    let run = jaynes()
        .args(["start", "--run", "leave", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .spawn().expect("start");
    for _ in 0..100 {
        if mark.join("started").exists() { break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let attempt = events(&s, "leave").iter().filter(|e| e["type"] == "AttemptStarted" && e["step"] == "change").count();
    let cwd = s.home.join("runs/leave/project");
    let mut child = jaynes()
        .args(["permit", "--run", "leave", "--step", "change", "--attempt", &attempt.to_string(), "--cwd", cwd.to_str().expect("path")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null())
        .spawn().expect("permit");
    let out = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let mut mcp = Mcp { child, out, n: 0 };
    let init = mcp.call("initialize", serde_json::json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "jaynes");
    assert_eq!(mcp.call("tools/list", serde_json::json!({}))["result"]["tools"][0]["name"], "permit");
    // reading inside the project is routine: a rule grants it at once
    let inside = mcp.permit("Read", serde_json::json!({"file_path": cwd.join("greeting.txt").display().to_string()}));
    assert_eq!(inside["behavior"], "allow", "{inside}");
    // reading outside is the person's: the server waits, the person refuses on the command line
    let asking = std::thread::spawn(move || {
        let d = mcp.permit("Read", serde_json::json!({"file_path": "/etc/hosts"}));
        (d, mcp)
    });
    let mut q = String::new();
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Some(e) = events(&s, "leave").into_iter().rfind(|e| e["type"] == "QuestionOpened")
            && e["question"]["kind"] == "permission" && e["question"]["about"].as_str().unwrap_or("").contains("/etc/hosts") {
                q = e["question"]["id"].as_str().expect("id").to_string();
                break;
            }
    }
    assert!(!q.is_empty(), "the leave was asked of the person");
    // while it waits for the person, the turn's clock stands still: the server marks the wait
    let waiting = s.home.join("runs/leave/leave-waiting");
    assert!(waiting.exists(), "the wait is marked");
    let answered = tty()
        .args([BIN, "answer", "leave", &q, "leave=drop", "--context", "not outside the project", "--send"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .output().expect("answer");
    assert!(String::from_utf8_lossy(&answered.stdout).contains("recorded"), "{}", String::from_utf8_lossy(&answered.stdout));
    let (decided, mut mcp) = asking.join().expect("asked");
    assert_eq!(decided["behavior"], "deny", "{decided}");
    assert!(decided["message"].as_str().unwrap_or("").contains("not outside the project"), "{decided}");
    assert!(!waiting.exists(), "the wait is over");
    // the same leave asked again gets the person's answer at once, without asking them again
    let again = mcp.permit("Read", serde_json::json!({"file_path": "/etc/hosts"}));
    assert_eq!(again["behavior"], "deny", "{again}");
    assert!(again["message"].as_str().unwrap_or("").contains("not outside the project"), "{again}");
    let _killed = mcp.child.kill();
    let _done = run.wait_with_output();
    let e = events(&s, "leave");
    let rules = e.iter().filter(|e| e["type"] == "AnswerRecorded" && e["answer"]["by"]["by"] == "rule").count();
    let people = e.iter().filter(|e| e["type"] == "AnswerRecorded" && e["answer"]["by"]["by"] == "person").count();
    assert_eq!((rules, people), (2, 1));
    let repeated = e.iter().filter(|e| e["type"] == "AnswerRecorded" && e["answer"]["by"]["rule"] == "answered-before").count();
    assert_eq!(repeated, 1, "the repeated leave names the rule that answered it");
    let finished = e.iter().find(|e| e["type"] == "RunFinished").unwrap_or_else(|| panic!("the run went on and finished: {:?}", types(&e)));
    assert_eq!(finished["cost"]["sent_back"], 0, "a leave refused is not work sent back");
}

fn fix_bug_run(s: &Scratch, run: &str) -> String {
    let prompts = s.home.join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    let agent = root().join("crates/shell/tests/agents/fix-bug.sh");
    let out = jaynes()
        .args(["start", "--run", run, "--role", root().join("crates/shell/tests/roles/fix-bug").to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "greet should say hi with the name", "--agent", &format!("command:sh {}", agent.display())])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "").env("PROMPTS", &prompts)
        .output().expect("start");
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

/// The merge question a run waits on, if it waits on one: every merge waits for the person.
fn open_merge(s: &Scratch, run: &str) -> Option<String> {
    let e = events(s, run);
    let closed: Vec<&str> = e
        .iter()
        .filter_map(|e| if e["type"] == "AnswerRecorded" { e["answer"]["question"].as_str() } else if e["type"] == "QuestionSuperseded" { e["question"].as_str() } else { None })
        .collect();
    e.iter()
        .filter(|e| e["type"] == "QuestionOpened" && e["question"]["kind"] == "merge")
        .filter_map(|e| e["question"]["id"].as_str())
        .rfind(|id| !closed.contains(id))
        .map(str::to_string)
}

/// The person, at a terminal, merges what the stop before the merge shows: its exit code and what it said.
fn merge_as_person(s: &Scratch, run: &str, envs: impl Fn(&mut Command)) -> (Option<i32>, String) {
    let q = open_merge(s, run).unwrap_or_else(|| panic!("the merge waits for the person: {:?}", types(&events(s, run))));
    let mut c = tty();
    c.args([BIN, "answer", run, &q, "--send"]);
    envs(&mut c);
    let out = c.output().expect("answer");
    (out.status.code(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// The environment of a fix-bug run, for what the person types after it starts.
fn fix_bug_env(s: &Scratch) -> impl Fn(&mut Command) + '_ {
    move |c: &mut Command| {
        c.env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "").env("PROMPTS", s.home.join("prompts"));
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_role_that_delivers_by_merge_merges_into_the_person_s_branch_and_undo_reverts_it() {
    let s = bug_project("merge");
    let started = fix_bug_run(&s, "merge");
    // nothing lands before the person looked at what lands and merged it
    assert!(!started.contains("merged as"), "{started}");
    assert_eq!(std::fs::read_to_string(s.project.join("greet.sh")).expect("read"), "echo \"hello $1\"\n", "nothing landed unasked");
    let (_, said) = merge_as_person(&s, "merge", fix_bug_env(&s));
    assert!(said.contains("merged as"), "{started}\n{said}");
    // the person's checkout moved forward: their working tree has the fix, and nothing was pushed
    assert_eq!(std::fs::read_to_string(s.project.join("greet.sh")).expect("read"), "echo \"hi $1\"\n");
    let log = Command::new("git").arg("-C").arg(&s.project).args(["log", "--format=%s", "-1", "main"]).output().expect("git");
    assert!(String::from_utf8_lossy(&log.stdout).contains("Jaynes: run merge, merged"));
    // an agent may not undo; the person at a terminal does
    let (ok, out) = engine(&s, &["undo", "merge", "--note", "not now"]);
    assert!(!ok && out.contains("only you undo"), "{out}");
    let undo = tty()
        .args([BIN, "undo", "merge", "--note", "not now"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .output().expect("undo");
    assert!(String::from_utf8_lossy(&undo.stdout).contains("undone"), "{}", String::from_utf8_lossy(&undo.stdout));
    assert_eq!(std::fs::read_to_string(s.project.join("greet.sh")).expect("read"), "echo \"hello $1\"\n");
    assert!(types(&events(&s, "merge")).contains(&"MergeUndone".to_string()));
}

#[test]
fn asked_jaynes_starts_in_a_project_with_no_commit_and_the_merge_makes_its_first_commit() {
    // the real-user's start of 23/11 in a folder fresh from `git init`: "git rev-parse HEAD failed: fatal: ambiguous
    // argument 'HEAD'", and an empty commit made by hand; Jaynes never asks for one (the tech-lead's T4)
    let s = scratch("unborn");
    let project = s.base.join("fresh");
    std::fs::create_dir_all(&project).expect("dir");
    assert!(Command::new("git").arg("-C").arg(&project).args(["init", "-q", "-b", "main"]).status().expect("git").success());
    let role = s.base.join("first-role");
    std::fs::create_dir_all(&role).expect("dir");
    std::fs::write(role.join("role.toml"), r#"
role = "first-role"
version = "1.0.0"
delivery = "merge"

[[step]]
name = "change"
reach = { project = "write", write = ["**"] }
instructions = "Make the change: {{request}}"
checks = [{ use = "field-required", field = "summary" }]

[step.output]
summary = { type = "string", about = "what changed" }
"#).expect("write");
    let agent = r#"printf 'hello\n' > greeting.txt && printf '{"summary":"a greeting"}' > .jaynes/output.json"#;
    let (ok, out) = engine(&s, &["start", "--run", "unborn", "--role", role.to_str().expect("path"), "--project", project.to_str().expect("path"),
        "--request", "add a greeting", "--agent", &format!("command:{agent}")]);
    assert!(ok, "{out}");
    let (_, said) = merge_as_person(&s, "unborn", |c: &mut Command| { c.env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1"); });
    assert!(said.contains("merged as"), "{out}\n{said}");
    let git = |args: &[&str]| String::from_utf8_lossy(&Command::new("git").arg("-C").arg(&project).args(args).output().expect("git").stdout).trim().to_string();
    assert_eq!(git(&["log", "--format=%s", "main"]), "Jaynes: run unborn, merged", "the project's first commit is the merge");
    assert_eq!(std::fs::read_to_string(project.join("greeting.txt")).expect("the checkout has it"), "hello\n");
    let undo = tty().args([BIN, "undo", "unborn", "--note", "not now"]).env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .output().expect("undo");
    assert!(String::from_utf8_lossy(&undo.stdout).contains("undone"), "{}{}", String::from_utf8_lossy(&undo.stdout), String::from_utf8_lossy(&undo.stderr));
    assert!(!project.join("greeting.txt").exists(), "undone");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_merge_waits_for_a_clean_checkout_and_says_why() {
    let s = bug_project("dirty");
    std::fs::write(s.project.join("greet.sh"), "echo \"hello there $1\"\n").expect("write");
    fix_bug_run(&s, "dirty");
    let (_, said) = merge_as_person(&s, "dirty", fix_bug_env(&s));
    assert!(said.contains("not merged: your checkout of main has uncommitted changes"), "{said}");
    assert_eq!(std::fs::read_to_string(s.project.join("greet.sh")).expect("read"), "echo \"hello there $1\"\n", "the person's work is untouched");
    let branch = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/dirty:greet.sh"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&branch.stdout), "echo \"hi $1\"\n", "the branch is delivered all the same");
}

// The real-user and the tech-lead, 23/11 (T7, the horta): "Finished: delivered", the folder unchanged, the change only
// on jaynes/r…, and no way to merge it. A run delivered on a branch always says how to reach the project: the branch
// with Copy, and Merge into the branch it started from, one click from its page; the merge waits at the stop every
// merge waits at (G19), the person keeps it, the change lands in their folder, nothing is pushed, and Undo takes it
// back. Only the person merges.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn asked_a_run_delivered_on_a_branch_shows_the_branch_and_merges_into_the_folder_from_its_page() {
    let s = scratch("horta");
    let out = start(&s, "horta", &format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}"));
    assert!(out.contains("finished: delivered"), "{out}");
    // a second run makes the same change, on a branch of its own
    let twin = start(&s, "horta-2", &format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}"));
    assert!(twin.contains("finished: delivered"), "{twin}");
    assert_eq!(std::fs::read_to_string(s.project.join("greeting.txt")).expect("read"), "hello\n", "on its branch, not in the folder");
    let (_page, port) = page_on_any_port(
        jaynes()
        .arg("page")
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("JAYNES_PAGE_KEY", "k3y"),
    );
    let me = "jaynes_key=k3y";
    let view = |port: u16| http(port, &format!("GET /run/horta HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {me}\r\n\r\n"));
    let before = view(port);
    assert!(before.contains("The change is on the branch “<code>jaynes/horta</code>”, not in your folder yet.") && before.contains("data-copy=\"jaynes/horta\""), "{before}");
    assert!(before.contains("data-act=\"merge\">Merge into “main”</button>") && before.contains("It lands on “main” in project. Nothing is pushed, and Undo takes it back."), "{before}");
    // an agent may not merge
    let refused = post(port, "/api/act", r#"{"run":"horta","action":"merge"}"#, "");
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(open_merge(&s, "horta").is_none(), "an agent's merge opens nothing");
    // the person's opens the merge's stop, and says which, so the page goes to its card
    let asked = post(port, "/api/act", r#"{"run":"horta","action":"merge"}"#, me);
    assert_eq!(asked["ok"], true, "{asked}");
    let q = (0..50).find_map(|_| open_merge(&s, "horta").or_else(|| { std::thread::sleep(std::time::Duration::from_millis(100)); None })).expect("the merge's stop");
    assert_eq!(asked["question"].as_str(), Some(q.as_str()), "{asked}");
    // a second Merge while it waits opens no other
    assert_eq!(post(port, "/api/act", r#"{"run":"horta","action":"merge"}"#, me)["ok"], false);
    let (_, said) = merge_as_person(&s, "horta", |c| {
        c.env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "");
    });
    assert!(said.contains("merged as"), "{said}");
    assert_eq!(std::fs::read_to_string(s.project.join("greeting.txt")).expect("read"), "hi\n", "the change is in the folder");
    let after = view(port);
    assert!(after.contains("Merged into “main” in project. Nothing was pushed.") && after.contains("data-act=\"undo\""), "{after}");
    assert!(!after.contains("not in your folder yet") && !after.contains("data-act=\"merge\""), "{after}");
    // the summary says where it went: merged, not only on its branch
    let (ok, told) = engine(&s, &["summary", "horta"]);
    assert!(ok && told.contains("merged as"), "{told}");
    // the twin brings what main has now: nothing to merge, said, no button, and a merge asked of it refused
    let twin = http(port, &format!("GET /run/horta-2 HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {me}\r\n\r\n"));
    assert!(twin.contains("Nothing to merge: “main” in project already has this change.") && twin.contains("data-copy=\"jaynes/horta-2\""), "{twin}");
    assert!(!twin.contains("data-act=\"merge\"") && !twin.contains("data-act=\"undo\"") && !twin.contains("not merged yet"), "{twin}");
    let nothing = post(port, "/api/act", r#"{"run":"horta-2","action":"merge"}"#, me);
    assert_eq!((nothing["ok"].as_bool(), nothing["refused"].as_str()), (Some(false), Some("Nothing to merge: “main” in project already has this change.")), "{nothing}");
    assert!(open_merge(&s, "horta-2").is_none(), "no stop opened for it");
    // Undo takes it back
    assert_eq!(post(port, "/api/act", r#"{"run":"horta","action":"undo","note":"not now"}"#, me)["ok"], true);
    assert_eq!(std::fs::read_to_string(s.project.join("greeting.txt")).expect("read"), "hello\n", "undone");
    let t = types(&events(&s, "horta"));
    for want in ["MergeAsked", "Merged", "MergeUndone"] {
        assert!(t.contains(&want.to_string()), "{want} missing: {t:?}");
    }
    let (ok, out) = engine(&s, &["verify"]);
    assert!(ok, "{out}");
}

#[test]
fn the_page_shows_each_item_with_its_sentence_and_picture() {
    let s = scratch("looks");
    let svg = r#"<svg xmlns='http://www.w3.org/2000/svg' width='40' height='40'><rect width='40' height='40' fill='#2b59c3'/></svg>"#;
    let agent = format!(
        "printf \"%s\" \"{svg}\" > .jaynes/blue.svg && printf '{{\"looks\":[{{\"id\":\"blue\",\"name\":\"Blue\",\"why\":\"ink on newsprint\",\"picture\":\".jaynes/blue.svg\"}}]}}' > .jaynes/output.json"
    );
    let role = root().join("crates/shell/tests/roles/looks");
    let (ok, out) = engine(&s, &["start", "--run", "looks", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                                 "--request", "make looks", "--agent", &format!("command:{agent}")]);
    assert!(ok && out.contains("Which looks go on?"), "{out}");
    let (_page, port) = page_on_any_port(
        jaynes()
        .arg("page")
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PAGE_KEY", "k3y"),
    );
    let html = http(port, &format!("GET / HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"));
    assert!(html.contains("ink on newsprint"), "the item's sentence is shown");
    assert!(html.contains("<img src=\"data:image/svg+xml;base64,"), "the item's picture is shown");
    // the picture was kept by its fingerprint, apart from the run's copy
    let kept: Vec<_> = std::fs::read_dir(s.home.join("pictures")).expect("kept").flatten().collect();
    assert_eq!(kept.len(), 1);
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_role_is_installed_by_name_and_only_when_it_conforms() {
    let s = scratch("roles");
    let (ok, out) = engine(&s, &["add", root().join("crates/shell/tests/roles/small-change").to_str().expect("path")]);
    assert!(ok && out.contains("role small-change installed"), "{out}");
    let (ok, out) = engine(&s, &["roles"]);
    assert!(ok && out.contains("small-change 4.0.0"), "{out}");
    // a folder that is not a role is refused
    let bad = s.home.join("bad");
    std::fs::create_dir_all(&bad).expect("dir");
    std::fs::write(bad.join("role.py"), "print('not a role')\n").expect("write");
    let (ok, out) = engine(&s, &["add", bad.to_str().expect("path")]);
    assert!(!ok && out.contains("does not conform"), "{out}");
    // a run starts with the installed role by its name
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let (ok, out) = engine(&s, &["start", "--run", "byname", "--role", "small-change", "--project", s.project.to_str().expect("path"),
                                 "--request", "say hi", "--agent", &format!("command:{agent}")]);
    assert!(ok && out.contains("finished: delivered"), "{out}");
    let (ok, out) = engine(&s, &["guide"]);
    assert!(ok && out.contains("jaynes start --role"), "{out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn agents_in_claude_code_use_the_mcp_server_and_may_only_release() {
    let s = scratch("mcp");
    assert!(engine(&s, &["add", root().join("crates/shell/tests/roles/small-change").to_str().expect("path")]).0);
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && printf 'test:\\n\\tgrep -q hi greeting.txt\\n\\n.PHONY: test\\n' > Makefile && {OUTPUT}");
    // the agent program of the run is a command; the MCP start uses Claude by default, so start here and read through MCP
    start(&s, "viamcp", &agent);
    let child = jaynes()
        .arg("mcp")
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python())
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null())
        .spawn().expect("mcp");
    let mut child = child;
    let out = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let mut mcp = Mcp { child, out, n: 0 };
    mcp.call("initialize", serde_json::json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}));
    let names: Vec<String> = mcp.call("tools/list", serde_json::json!({}))["result"]["tools"].as_array().expect("tools").iter().map(|t| t["name"].as_str().unwrap_or("").to_string()).collect();
    assert_eq!(names, ["home", "roles", "start", "run", "answer", "halt", "continue", "abandon", "correct"]);
    let tool = |mcp: &mut Mcp, name: &str, args: Value| -> (bool, String) {
        let r = mcp.call("tools/call", serde_json::json!({"name": name, "arguments": args}));
        (r["result"]["isError"] == false, r["result"]["content"][0]["text"].as_str().unwrap_or("").to_string())
    };
    let (_, home) = tool(&mut mcp, "home", serde_json::json!({}));
    // the stop says what raised the risk: the Makefile the agent wrote
    assert!(home.contains("run viamcp") && home.contains("This change touches `Makefile`, which runs later outside any box."), "{home}");
    let (_, roles) = tool(&mut mcp, "roles", serde_json::json!({}));
    assert!(roles.contains("small-change"), "{roles}");
    let q = events(&s, "viamcp").into_iter().find(|e| e["type"] == "QuestionOpened").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    // small-change's stop is a release: an agent may let it go on
    let (ok, said) = tool(&mut mcp, "answer", serde_json::json!({"run": "viamcp", "question": q}));
    assert!(ok && said.contains("counts as the agent's"), "{said}");
    let (ok, said) = tool(&mut mcp, "run", serde_json::json!({"run": "viamcp"}));
    assert!(ok && said.contains("run viamcp"), "{said}");
    let (ok, said) = tool(&mut mcp, "halt", serde_json::json!({"run": "nope"}));
    assert!(!ok && said.contains("no run nope"), "{said}");
    let _killed = mcp.child.kill();
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_machine_that_cannot_run_the_tests_stops_the_run_before_any_agent_works() {
    if Command::new("python3").args(["-c", "import pytest"]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipped: this machine's python3 has pytest");
        return;
    }
    let s = scratch("nopytest");
    std::fs::write(s.project.join("pytest.ini"), "[pytest]\n").expect("write");
    std::fs::write(s.project.join("test_greet.py"), "def test_it():\n    assert True\n").expect("write");
    std::fs::remove_file(s.project.join("Makefile")).expect("rm");
    for args in [vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "pytest"]] {
        assert!(Command::new("git").arg("-C").arg(&s.project).args(&args).status().expect("git").success());
    }
    let out = start(&s, "nopytest", "touch agent-ran");
    assert!(out.contains("cannot run on this machine") && out.contains("install pytest"), "{out}");
    let t = types(&events(&s, "nopytest"));
    assert_eq!(t.iter().filter(|x| *x == "AttemptStarted").count(), 1, "only the role's own step started: {t:?}");
    assert!(!s.home.join("runs/nopytest/project/agent-ran").exists(), "no agent worked");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn abandoning_a_run_stops_its_agent_at_once() {
    let s = scratch("abandon");
    let mark = s.home.join("mark");
    std::fs::create_dir_all(&mark).expect("dir");
    let agent = format!("touch {m}/started; sleep 30; touch {m}/finished", m = mark.display());
    let role = root().join("crates/shell/tests/roles/small-change");
    let mut run = jaynes()
        .args(["start", "--run", "abandon", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .spawn().expect("start");
    for _ in 0..100 {
        if mark.join("started").exists() { break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let pgid = std::fs::read_to_string(s.home.join("runs/abandon/agent.pid")).expect("noted");
    let abandoned = tty()
        .args([BIN, "abandon", "abandon"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .output().expect("abandon");
    let said = String::from_utf8_lossy(&abandoned.stdout).to_string();
    let alive = Command::new("kill").args(["-0", "--", &format!("-{}", pgid.trim())]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    assert!(!alive, "the agent was stopped at once: {said}");
    let _done = run.wait();
    assert!(!mark.join("finished").exists());
    assert!(types(&events(&s, "abandon")).contains(&"RunFinished".to_string()));
}

/// A stand-in for an agent program: a program the process table names `claude` that runs one shell line and waits
/// for it. A script would not do: the table names it by its interpreter.
fn agent_stand_in(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).expect("dir");
    let src = dir.join("stand-in.c");
    std::fs::write(&src, "#include <stdlib.h>\nint main(int c, char **v) { return c > 1 ? system(v[1]) != 0 : 2; }\n").expect("source");
    let bin = dir.join("claude");
    let ok = Command::new("cc").arg("-o").arg(&bin).arg(&src).status().expect("cc").success();
    assert!(ok, "the stand-in compiled");
    bin
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_agent_that_started_a_run_sets_it_aside_and_another_agent_cannot() {
    let s = scratch("own");
    let mark = s.home.join("mark");
    std::fs::create_dir_all(&mark).expect("dir");
    let agent = format!("touch {m}/started; sleep 30; touch {m}/finished", m = mark.display());
    let role = root().join("crates/shell/tests/roles/small-change");
    let (a, b) = (agent_stand_in(&s.base.join("a")), agent_stand_in(&s.base.join("b")));
    let env = |c: &mut Command| {
        c.env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
            .env("JAYNES_CHROME", test_browser().unwrap_or_else(|| PathBuf::from(NO_BROWSER))).env("BIN", BIN)
            .stdin(std::process::Stdio::null());
    };
    // agent A starts the run, then abandons it once told to; agent B, beside it, tries first
    let line = format!(
        "\"$BIN\" start --run own --role '{role}' --project '{project}' --request 'say hi' --agent 'command:{agent}' >/dev/null 2>&1 & \
         while [ ! -e '{m}/go' ]; do sleep 0.1; done; \"$BIN\" abandon own > '{m}/a.out' 2>&1; wait",
        role = role.display(), project = s.project.display(), m = mark.display()
    );
    let mut starter = Command::new(&a);
    starter.arg(line);
    env(&mut starter);
    let mut starter = starter.spawn().expect("agent A");
    for _ in 0..100 {
        if mark.join("started").exists() { break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(mark.join("started").exists(), "the run's agent started");
    let mut other = Command::new(&b);
    other.arg(format!("\"$BIN\" abandon own > '{m}/b.out' 2>&1; true", m = mark.display()));
    env(&mut other);
    assert!(other.status().expect("agent B").success());
    let said_b = std::fs::read_to_string(mark.join("b.out")).unwrap_or_default();
    assert!(said_b.contains("only you abandon a run, or the agent that started it"), "{said_b}");
    assert!(!types(&events(&s, "own")).contains(&"SetAside".to_string()), "another agent set it aside");
    // A abandons: set aside, its agent stopped at once as at a halt, nothing finished
    let pgid = std::fs::read_to_string(s.home.join("runs/own/agent.pid")).expect("noted");
    std::fs::write(mark.join("go"), "").expect("go");
    let _done = starter.wait();
    let said_a = std::fs::read_to_string(mark.join("a.out")).unwrap_or_default();
    let alive = Command::new("kill").args(["-0", "--", &format!("-{}", pgid.trim())]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    assert!(!alive, "the agent was stopped at once: {said_a}");
    assert!(!mark.join("finished").exists());
    let log = events(&s, "own");
    let started = log.iter().find(|e| e["type"] == "RunStarted").expect("started");
    assert!(started["starter"].as_str().is_some_and(|w| w.starts_with(&format!("claude:{}@", starter.id()))), "{started}");
    let t = types(&log);
    assert!(t.contains(&"SetAside".to_string()) && !t.contains(&"RunFinished".to_string()), "{t:?} {said_a}");
    assert!(t.contains(&"AttemptInterrupted".to_string()) && !t.contains(&"AttemptFailed".to_string()), "came back interrupted, as at a halt: {t:?}");
    // only the person ends it for good
    let out = tty().args([BIN, "abandon", "own"]).env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").output().expect("abandon");
    assert!(types(&events(&s, "own")).contains(&"RunFinished".to_string()), "{}", String::from_utf8_lossy(&out.stdout));
}

/// A page process that dies with its test, even when the test panics, so it never holds the test's output open.
struct Page(std::process::Child);

impl Drop for Page {
    fn drop(&mut self) {
        let _killed = self.0.kill();
        let _reaped = self.0.wait();
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_person_starts_a_run_from_the_page() {
    let s = scratch("new");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let (_page, port) = page_on_any_port(
        jaynes()
        .arg("page")
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("JAYNES_PAGE_KEY", "k3y")
        .env("JAYNES_AGENT", format!("command:{agent}")),
    );
    let me = "jaynes_key=k3y";
    let form = http(port, &format!("GET /new HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {me}\r\n\r\n"));
    assert!(form.contains("<form id=\"start\"") && form.contains("name=\"request\""), "{form}");
    let role = root().join("crates/shell/tests/roles/small-change");
    let body = serde_json::json!({"role": role, "project": s.project, "request": "say hi instead of hello", "risk": "low"}).to_string();
    // only the person starts a run from the page
    assert_eq!(post(port, "/api/start", &body, "")["ok"], false);
    let r = post(port, "/api/start", &body, me);
    assert_eq!(r["ok"], true, "{r}");
    let run = r["run"].as_str().expect("run").to_string();
    let mut finished = false;
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if s.home.join("record.sqlite3").exists() && events(&s, &run).iter().any(|e| e["type"] == "RunFinished") {
            finished = true;
            break;
        }
    }
    assert!(finished, "{}", std::fs::read_to_string(s.home.join(format!("start-{run}.log"))).unwrap_or_default());
    let view = http(port, &format!("GET /run/{run} HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {me}\r\n\r\n"));
    assert!(view.contains("say hi instead of hello") && view.contains("Finished: delivered"), "{view}");
}

/// A made-up toolchain in a folder of its own, as mise, asdf or Homebrew put one: on the PATH of the person's terminal,
/// not on the one of a page or an app opened from the Dock.
fn made_up_tool(s: &Scratch) -> PathBuf {
    let tools = s.base.join("tools/bin");
    std::fs::create_dir_all(&tools).expect("dir");
    std::fs::write(tools.join("quitanda-cc"), "#!/bin/sh\nexit 0\n").expect("write");
    let mut mode = std::fs::metadata(tools.join("quitanda-cc")).expect("meta").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(tools.join("quitanda-cc"), mode).expect("chmod");
    tools
}

/// The engine lead's direction of 21/11 for the run the page or the app starts: every `jaynes` the person runs at a
/// terminal keeps that terminal's environment, and a run started without one uses it, so the agent finds the
/// toolchains the person has. Until the person ran `jaynes` in a terminal, the start form says so.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_run_started_from_a_page_with_a_short_path_finds_the_toolchain_of_the_person_s_terminal() {
    let s = scratch("page-env");
    let tools = made_up_tool(&s);
    let agent = format!("quitanda-cc && perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let short = "/usr/bin:/bin:/usr/sbin:/sbin";
    let page = |s: &Scratch| page_on_any_port(
        jaynes()
        .arg("page")
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("JAYNES_PAGE_KEY", "k3y")
        .env("PATH", short)
        .env("JAYNES_AGENT", format!("command:{agent}")),
    );
    let me = "jaynes_key=k3y";
    {
        let (_page, port) = page(&s);
        let form = http(port, &format!("GET /new HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {me}\r\n\r\n"));
        assert!(form.contains("run <code>jaynes</code> once in a terminal"), "the form does not say where the tools come from: {form}");
    }
    // the person runs jaynes in their terminal, whose PATH has their tools
    let long = format!("{}:{}", tools.display(), std::env::var("PATH").unwrap_or_default());
    let out = tty().arg(BIN).arg("--version").env("JAYNES_ENGINE_HOME", &s.home).env("PATH", &long).output().expect("terminal");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let (_page, port) = page(&s);
    let form = http(port, &format!("GET /new HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {me}\r\n\r\n"));
    assert!(!form.contains("once in a terminal"), "{form}");
    let role = root().join("crates/shell/tests/roles/small-change");
    let body = serde_json::json!({"role": role, "project": s.project, "request": "say hi instead of hello", "risk": "low"}).to_string();
    let r = post(port, "/api/start", &body, me);
    assert_eq!(r["ok"], true, "{r}");
    let run = r["run"].as_str().expect("run").to_string();
    let mut finished = false;
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if s.home.join("record.sqlite3").exists() && events(&s, &run).iter().any(|e| e["type"] == "RunFinished") {
            finished = true;
            break;
        }
    }
    let log = std::fs::read_to_string(s.home.join(format!("start-{run}.log"))).unwrap_or_default();
    assert!(finished, "{log}");
    assert!(types(&events(&s, &run)).iter().any(|t| t == "Delivered"), "the agent did not find the toolchain: {log}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_start_without_a_terminal_keeps_its_own_environment_and_only_the_app_s_mark_gives_it_the_person_s_terminal() {
    let s = scratch("start-env");
    let tools = made_up_tool(&s);
    let long = format!("{}:{}", tools.display(), std::env::var("PATH").unwrap_or_default());
    let out = tty().arg(BIN).arg("--version").env("JAYNES_ENGINE_HOME", &s.home).env("PATH", &long).output().expect("terminal");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let role = root().join("crates/shell/tests/roles/small-change");
    let agent = format!("quitanda-cc && perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    // an agent's start, with no terminal: it may be in a direnv or a PATH of the project that the terminal does not know
    let start = |run: &str, mark: Option<&str>| {
        let mut c = jaynes();
        c.args(["start", "--run", run, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
            "--request", "say hi instead of hello", "--agent", &format!("command:{agent}")])
            .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin").env_remove("JAYNES_START_ENV")
            .stdin(std::process::Stdio::null());
        if let Some(m) = mark {
            c.env("JAYNES_START_ENV", m);
        }
        let out = c.output().expect("start");
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    };
    let said = start("agent-start", None);
    assert!(said.contains("quitanda-cc: command not found"), "a start without a terminal took the person's terminal's PATH: {said}");
    assert!(!types(&events(&s, "agent-start")).iter().any(|t| t == "Delivered"), "{said}");
    // the app's start says it is the app's, and finds the person's tools
    let said = start("app-start", Some("terminal"));
    assert!(types(&events(&s, "app-start")).iter().any(|t| t == "Delivered"), "the app's start did not find the toolchain: {said}");
}

#[test]
fn the_page_sets_the_guard_and_says_when_jaynes_was_updated() {
    let s = scratch("setup");
    let settings = s.home.parent().expect("base").join("settings");
    let install = s.home.parent().expect("base").join("install");
    std::fs::create_dir_all(&install).expect("dir");
    let installed = |commit: &str| {
        let line = format!("[v1]\n\"jaynes-shell 0.1.0 (git+https://github.com/alk0x1/jaynes#{commit})\" = [\"jaynes\"]\n");
        std::fs::write(install.join(".crates.toml"), line).expect("write");
    };
    installed(&"a".repeat(40));
    let (_page, port) = page_on_any_port(
        jaynes()
        .arg("page")
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_HOME", &settings)
        .env("JAYNES_INSTALL_ROOT", &install)
        .env("JAYNES_NO_UPDATE_CHECK", "1")
        .env("JAYNES_PAGE_KEY", "k3y"),
    );
    let me = "jaynes_key=k3y";
    let get = |path: &str| http(port, &format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {me}\r\n\r\n"));
    let setup = get("/setup");
    assert!(setup.contains("<h2>Guard</h2>") && setup.contains("chip green\">on") && setup.contains("aaaaaaa"), "{setup}");
    // only the person changes the guard
    assert_eq!(post(port, "/api/setup", r#"{"action":"guard-off"}"#, "")["ok"], false);
    assert_eq!(post(port, "/api/setup", r#"{"action":"guard-off"}"#, me)["ok"], true);
    assert_eq!(post(port, "/api/setup", r#"{"action":"owner-add","owner":"someone"}"#, me)["ok"], true);
    let g: Value = serde_json::from_str(&std::fs::read_to_string(settings.join("guard.json")).expect("guard")).expect("json");
    assert_eq!(g["on"], false);
    assert_eq!(g["owners"], serde_json::json!(["someone"]));
    assert_eq!(post(port, "/api/setup", r#"{"action":"owner-remove","owner":"someone"}"#, me)["ok"], true);
    // a newer Jaynes installed on disk: the page asks to be restarted
    assert!(!get("/home").contains("Restart the page"));
    installed(&"b".repeat(40));
    assert!(get("/home").contains("Restart the page"));
}

/// "A newer Jaynes is out." and its Update show only when a release newer than the program installed is published:
/// main ahead of the last release is no newer Jaynes, since Update installs the release (found by the real-user agent
/// on 0.2.4, the latest release, with main ahead of it: the notice showed, and Update changed nothing). Nothing
/// reaches the network: git finds Jaynes's source in a repository of the test's, and the releases in a folder of it.
#[test]
fn the_newer_jaynes_notice_waits_for_a_release_never_for_main() {
    let s = scratch("newer");
    let base = s.home.parent().expect("base").to_path_buf();
    // main, ahead of the release and of the program installed
    let source = base.join("source.git");
    let git = |args: &[&str]| assert!(Command::new("git").args(args).status().expect("git").success(), "git {args:?}");
    git(&["init", "-q", "--bare", &source.display().to_string()]);
    git(&["-C", &s.project.display().to_string(), "push", "-q", &source.display().to_string(), "HEAD:refs/heads/main"]);
    let gitconfig = base.join("gitconfig");
    std::fs::write(&gitconfig, format!("[url \"file://{}\"]\n\tinsteadOf = https://github.com/alk0x1/jaynes\n", source.display())).expect("write");
    let install = base.join("install");
    std::fs::create_dir_all(&install).expect("dir");
    let commit = "a".repeat(40);
    std::fs::write(install.join(".crates.toml"), format!("[v1]\n\"jaynes-shell 0.1.0 (git+https://github.com/alk0x1/jaynes#{commit})\" = [\"jaynes\"]\n")).expect("write");
    let me = "jaynes_key=k3y";
    // a page whose latest release is `version`, built for every machine
    let page = |name: &str, version: &str| {
        let releases = base.join(format!("releases-{name}"));
        std::fs::create_dir_all(&releases).expect("dir");
        let built = serde_json::json!({"url": "file:///nonexistent/jaynes.tar.gz"});
        let targets: serde_json::Map<String, Value> =
            ["macos-aarch64", "macos-x86_64", "linux-x86_64", "linux-aarch64"].iter().map(|t| (t.to_string(), built.clone())).collect();
        std::fs::write(releases.join("latest.json"), serde_json::json!({"version": version, "targets": targets}).to_string()).expect("write");
        page_on_any_port(
            jaynes()
                .arg("page")
                .env("JAYNES_ENGINE_HOME", base.join(format!("home-{name}")))
                .env("JAYNES_NO_PAGE", "1")
                .env("JAYNES_HOME", base.join(format!("settings-{name}")))
                .env("JAYNES_INSTALL_ROOT", &install)
                .env("JAYNES_RELEASES", format!("file://{}", releases.display()))
                .env("GIT_CONFIG_GLOBAL", &gitconfig)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("JAYNES_PAGE_KEY", "k3y"),
        )
    };
    let (_latest, latest) = page("latest", env!("CARGO_PKG_VERSION"));
    let (_newer, newer) = page("newer", "99.0.0");
    let home = |port: u16| http(port, &format!("GET /home HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {me}\r\n\r\n"));
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !home(newer).contains("A newer Jaynes is out.") {
        assert!(std::time::Instant::now() < until, "a release newer than the program installed is told: {}", home(newer));
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert!(home(newer).contains("data-setup=\"update\">Update"), "{}", home(newer));
    // the page started with it has asked too: the latest release is the program installed, and main is no release
    for _ in 0..10 {
        let said = home(latest);
        assert!(!said.contains("A newer Jaynes is out.") && !said.contains("data-setup=\"update\""), "main ahead of the latest release is no newer Jaynes: {said}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
fn the_page_shows_what_the_watching_guard_would_have_asked_and_frees_an_owner_from_it() {
    let s = scratch("watch");
    let settings = s.home.parent().expect("base").join("settings");
    let install = s.home.parent().expect("base").join("install");
    std::fs::create_dir_all(&settings).expect("dir");
    std::fs::create_dir_all(&install).expect("dir");
    std::fs::write(settings.join("guard.json"), r#"{"on": true, "owners": ["me"]}"#).expect("write");
    let (_page, port) = page_on_any_port(
        jaynes()
        .arg("page")
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_HOME", &settings)
        .env("JAYNES_INSTALL_ROOT", &install)
        .env("JAYNES_NO_UPDATE_CHECK", "1")
        .env("JAYNES_PAGE_KEY", "k3y"),
    );
    let me = "jaynes_key=k3y";
    let get = |path: &str| http(port, &format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {me}\r\n\r\n"));
    let settings_now = || -> Value { serde_json::from_str(&std::fs::read_to_string(settings.join("guard.json")).expect("guard")).expect("json") };
    assert!(get("/setup").contains("data-setup=\"guard-watch\""), "the guard on offers to only watch");
    // only the person makes it watch, and it stays on
    assert_eq!(post(port, "/api/setup", r#"{"action":"guard-watch"}"#, "")["ok"], false);
    assert_eq!(post(port, "/api/setup", r#"{"action":"guard-watch"}"#, me)["ok"], true);
    assert_eq!((settings_now()["on"].clone(), settings_now()["watch"].clone()), (Value::Bool(true), Value::Bool(true)));
    assert_eq!(settings_now()["owners"], serde_json::json!(["me"]), "the owners stay");
    // what it noted, as the guard writes it; an owner is read from a remote, so it is text from outside
    let note = |owner: Value, action: &str, at: &str| format!("{}\n", serde_json::json!({"at": at, "session": "s", "cwd": "/p", "held": [{"action": action, "where": "github.com/x/y", "owner": owner}]}));
    let notes = [
        note("acme".into(), "git push", "2026-11-10T09:00:00-03:00"),
        note("acme".into(), "git push", "2026-11-11T09:00:00-03:00"),
        note("acme".into(), "gh pr merge", "2026-11-12T09:00:00-03:00"),
        note("ev\"il<b>x".into(), "git push", "2026-11-12T10:00:00-03:00"),
        note(Value::Null, "git push", "2026-11-12T11:00:00-03:00"),
        "not a note\n".to_string(),
    ]
    .concat();
    std::fs::write(settings.join("guard-watch.jsonl"), notes).expect("write");
    let setup = get("/setup");
    assert!(setup.contains("chip\">watching") && setup.contains("data-setup=\"guard-on\">Ask again"), "{setup}");
    assert!(setup.contains("5 since 2026-11-10"), "five notes, from the first one's day: {setup}");
    assert!(setup.contains("<code>acme</code> 3: gh pr merge 1, git push 2") && setup.contains("data-owner=\"acme\">Add to free owners"), "{setup}");
    assert!(setup.contains("ev&quot;il&lt;b&gt;x") && !setup.contains("ev\"il<b>") && !setup.contains("data-owner=\"ev"), "an owner from outside is escaped and never offered: {setup}");
    assert!(setup.contains("<code>github.com/x/y</code> 1: git push"), "where it could not tell an owner, by where: {setup}");
    // freeing an owner from it goes through the same check as the Add of Free owners
    assert_eq!(post(port, "/api/setup", r#"{"action":"owner-add","owner":"ev\"il<b>x"}"#, me)["ok"], false);
    assert_eq!(post(port, "/api/setup", r#"{"action":"owner-add","owner":"acme"}"#, me)["ok"], true);
    assert_eq!(settings_now()["owners"], serde_json::json!(["me", "acme"]));
    assert_eq!(settings_now()["watch"], true, "freeing an owner leaves it watching");
    let setup = get("/setup");
    assert!(setup.contains("<code>acme</code> 3: gh pr merge 1, git push 2 <span class=\"none\">free now") && !setup.contains("data-owner=\"acme\">Add to free owners"), "{setup}");
    // only the person clears the notes, and asking again turns watching off
    assert_eq!(post(port, "/api/setup", r#"{"action":"watch-clear"}"#, "")["ok"], false);
    assert!(settings.join("guard-watch.jsonl").exists());
    assert_eq!(post(port, "/api/setup", r#"{"action":"watch-clear"}"#, me)["ok"], true);
    assert!(!settings.join("guard-watch.jsonl").exists() && !get("/setup").contains("would have asked"));
    assert_eq!(post(port, "/api/setup", r#"{"action":"guard-on"}"#, me)["ok"], true);
    assert_eq!((settings_now()["on"].clone(), settings_now()["watch"].clone()), (Value::Bool(true), Value::Bool(false)));
    assert!(get("/setup").contains("chip green\">on"));
}

#[test]
fn the_page_turns_the_guard_off_in_a_project_and_the_guard_reads_it_so() {
    let s = scratch("off-in");
    let base = s.home.parent().expect("base").to_path_buf();
    let settings = base.join("settings");
    let install = base.join("install");
    let project = base.join("pro<j>ect");
    for d in [&settings, &install, &project] {
        std::fs::create_dir_all(d).expect("dir");
    }
    for args in [vec!["init", "-q"], vec!["remote", "add", "origin", "https://github.com/someone-else/thing.git"]] {
        assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success());
    }
    std::fs::write(settings.join("guard.json"), r#"{"on": true, "owners": ["me"]}"#).expect("write");
    let (_page, port) = page_on_any_port(
        jaynes()
        .arg("page")
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_HOME", &settings)
        .env("JAYNES_INSTALL_ROOT", &install)
        .env("JAYNES_NO_UPDATE_CHECK", "1")
        .env("JAYNES_PAGE_KEY", "k3y"),
    );
    let me = "jaynes_key=k3y";
    let get = |path: &str| http(port, &format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {me}\r\n\r\n"));
    // the guard as Claude Code runs it, on a push from inside the project: whether it asks
    let guard_asks = || {
        let mut child = Command::new("python3")
            .arg(root().join("../hooks/guard.py"))
            .env("JAYNES_HOME", &settings).env("GH_CONFIG_DIR", base.join("gh")).env("CLAUDE_PLUGIN_ROOT", root().join(".."))
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
            .spawn().expect("guard");
        let event = serde_json::json!({"session_id": "s", "hook_event_name": "PreToolUse", "tool_name": "Bash", "cwd": project.join("src"), "tool_input": {"command": "git push origin main"}});
        std::io::Write::write_all(&mut child.stdin.take().expect("stdin"), event.to_string().as_bytes()).expect("event");
        let done = child.wait_with_output().expect("guard");
        assert!(done.stderr.is_empty(), "{}", String::from_utf8_lossy(&done.stderr));
        String::from_utf8_lossy(&done.stdout).contains("\"ask\"")
    };
    std::fs::create_dir_all(project.join("src")).expect("dir");
    assert!(guard_asks(), "the guard asks before a push to someone else's repository");
    let folder = project.display().to_string();
    let add = serde_json::json!({"action": "off-in-add", "folder": folder}).to_string();
    // only the person turns the guard off in a project
    assert_eq!(post(port, "/api/setup", &add, "")["ok"], false);
    assert!(guard_asks());
    // a folder that is not one, or one that holds the person's home (almost everything) or Jaynes's own folder, is
    // refused: Turn off is for everywhere, and says what it is
    let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
    let above_home = home.parent().expect("a home has a parent").display().to_string();
    let engine = s.home.display().to_string();
    let above_engine = base.display().to_string();
    for bad in ["relative/path", "/no/such/folder/anywhere", "/", "~", "~/", &above_home, &engine, &above_engine] {
        let body = serde_json::json!({"action": "off-in-add", "folder": bad}).to_string();
        let said = post(port, "/api/setup", &body, me);
        assert_eq!(said["ok"], false, "{bad}: {said}");
    }
    let g: Value = serde_json::from_str(&std::fs::read_to_string(settings.join("guard.json")).expect("guard")).expect("json");
    assert!(g["off_in"].is_null(), "nothing was written: {g}");
    assert_eq!(post(port, "/api/setup", &add, me)["ok"], true);
    let g: Value = serde_json::from_str(&std::fs::read_to_string(settings.join("guard.json")).expect("guard")).expect("json");
    let canonical = project.canonicalize().expect("canonical").display().to_string();
    assert_eq!(g["off_in"], serde_json::json!([canonical]), "kept as the folder itself, once");
    assert_eq!((g["on"].clone(), g["owners"].clone()), (Value::Bool(true), serde_json::json!(["me"])), "the rest stays");
    assert_eq!(post(port, "/api/setup", &add, me)["ok"], true);
    let g: Value = serde_json::from_str(&std::fs::read_to_string(settings.join("guard.json")).expect("guard")).expect("json");
    assert_eq!(g["off_in"].as_array().map(Vec::len), Some(1), "added twice, kept once");
    assert!(!guard_asks(), "the guard reads what the page wrote: off in this project, inside its folders too");
    let setup = get("/setup");
    let shown = canonical.replace('<', "&lt;").replace('>', "&gt;");
    assert!(setup.contains(&shown) && !setup.contains("pro<j>ect"), "the folder is shown, escaped: {setup}");
    assert!(setup.contains("data-setup=\"off-in-remove\"") && setup.contains("data-setup=\"off-in-add\""), "{setup}");
    // and on again
    let remove = serde_json::json!({"action": "off-in-remove", "folder": canonical}).to_string();
    assert_eq!(post(port, "/api/setup", &remove, "")["ok"], false);
    assert_eq!(post(port, "/api/setup", &remove, me)["ok"], true);
    assert!(guard_asks(), "turned on again in the project, the guard asks again");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn an_agent_that_says_it_cannot_be_done_waits_for_the_person_and_goes_on_from_their_words() {
    let s = scratch("impossible");
    let prompts = s.home.parent().expect("base").join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    // first turn: it cannot be done as asked; second turn, in the same session: it does what the person said
    let agent = format!(
        "cat > {p}/turn-${{JAYNES_SESSION:-1}}; if [ -n \"$JAYNES_SESSION\" ]; then perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}; else echo 'the request asks for hi and for hello at once' > .jaynes/impossible.txt; echo '{{\"session\":\"s1\"}}'; fi",
        p = prompts.display()
    );
    let out = start(&s, "impossible", &agent);
    assert!(out.contains("cannot be done as asked"), "{out}");
    let first = std::fs::read_to_string(prompts.join("turn-1")).expect("prompt");
    assert!(first.contains(".jaynes/impossible.txt"), "the agent is told the way out: {first}");
    let e = events(&s, "impossible");
    assert!(e.iter().any(|e| e["type"] == "AttemptImpossible"), "{:?}", types(&e));
    let q = e.iter().rfind(|e| e["type"] == "InputNeeded").expect("asked")["question"]["id"].as_str().expect("id").to_string();
    let answered = tty()
        .args([BIN, "answer", "impossible", &q, "impossible=change:only hi", "--send"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .output().expect("answer");
    assert!(answered.status.success(), "{}", String::from_utf8_lossy(&answered.stdout));
    let mut finished = false;
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if events(&s, "impossible").iter().any(|e| e["type"] == "RunFinished") {
            finished = true;
            break;
        }
    }
    let e = events(&s, "impossible");
    assert!(finished, "{:?}", types(&e));
    let second = std::fs::read_to_string(prompts.join("turn-s1")).expect("the second turn resumed the session");
    assert!(second.contains("only hi"), "{second}");
    let spent: u64 = e.iter().filter(|e| e["type"] == "AttemptFailed" && e["spent"] == true).count() as u64;
    assert_eq!(spent, 0, "saying so spent no attempt");
}

/// The engine killed at many moments of a run that merges: each time the next process recovers, the run delivers,
/// the fix lands exactly once, nothing is left running and the record holds.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_run_killed_at_any_moment_recovers_and_lands_once() {
    let agent = root().join("crates/shell/tests/agents/fix-bug.sh");
    let role = root().join("crates/shell/tests/roles/fix-bug");
    let mut outcomes = Vec::new();
    for trial in 0..16u64 {
        let s = bug_project(&format!("kill{trial}"));
        let prompts = s.home.join("prompts");
        std::fs::create_dir_all(&prompts).expect("dir");
        let run = format!("kill{trial}");
        let envs = |c: &mut Command| {
            c.env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "").env("PROMPTS", &prompts);
        };
        let mut first = jaynes();
        first.args(["start", "--run", &run, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                    "--request", "greet should say hi with the name", "--agent", &format!("command:sh {}", agent.display())]);
        envs(&mut first);
        let mut p = first.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().expect("start");
        // the first six in the start, where the run is being set up; the rest spread over the run's length
        let at = if trial < 6 { trial * 50 } else { 40 + (trial * 389) % 2600 };
        std::thread::sleep(std::time::Duration::from_millis(at));
        let _killed = p.kill();
        let _waited = p.wait();
        if events(&s, &run).is_empty() {
            // killed before the run was recorded: it never started, and starting it again works
            let mut again = jaynes();
            again.args(["start", "--run", &run, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                        "--request", "greet should say hi with the name", "--agent", &format!("command:sh {}", agent.display())]);
            envs(&mut again);
            let out = again.output().expect("start again");
            assert!(out.status.success(), "trial {trial}: {}", String::from_utf8_lossy(&out.stderr));
        }
        let mut said = String::new();
        for _ in 0..6 {
            if events(&s, &run).iter().any(|e| e["type"] == "Delivered") {
                break;
            }
            // the merge waits for the person; anything else, the run goes on
            if open_merge(&s, &run).is_some() {
                said.push_str(&merge_as_person(&s, &run, envs).1);
                continue;
            }
            let mut c = tty();
            c.args([BIN, "continue", &run]);
            envs(&mut c);
            let out = c.output().expect("continue");
            said.push_str(&String::from_utf8_lossy(&out.stdout));
        }
        let e = events(&s, &run);
        // the run delivers, and waits only for answers that can still correct it
        assert!(e.iter().any(|e| e["type"] == "Delivered"), "trial {trial}: {said}\n{:?}", types(&e));
        let (ok, out) = engine(&s, &["verify"]);
        assert!(ok, "trial {trial}: {out}");
        let log = Command::new("git").arg("-C").arg(&s.project).args(["log", "--format=%s", "main"]).output().expect("git");
        let merges = String::from_utf8_lossy(&log.stdout).lines().filter(|l| l.contains("merged")).count();
        assert_eq!(merges, 1, "trial {trial}: the fix landed {merges} times");
        assert_eq!(std::fs::read_to_string(s.project.join("greet.sh")).expect("read"), "echo \"hi $1\"\n", "trial {trial}");
        let pid = s.home.join(format!("runs/{run}/agent.pid"));
        assert!(!pid.exists(), "trial {trial}: an agent was left noted as running");
        outcomes.push(e.iter().any(|e| e["type"] == "AttemptInterrupted"));
    }
    assert!(outcomes.iter().any(|&x| x), "some kill fell inside a turn: {outcomes:?}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_finished_run_leaves_no_toolchain_cache_behind() {
    let s = scratch("caches");
    // what Go leaves: a build cache, and modules it makes read-only
    let fill = r#"mkdir -p "$GOCACHE/ab" "$GOPATH/pkg/mod/m@v1" "$npm_config_cache" && echo x > "$GOCACHE/ab/o" && echo x > "$GOPATH/pkg/mod/m@v1/m.go" && echo x > "$npm_config_cache/p" && chmod -R a-w "$GOPATH/pkg/mod""#;
    let agent = format!("{fill} && perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let out = start(&s, "caches", &agent);
    assert!(out.contains("finished: delivered"), "{out}");
    let exchange = s.home.join("runs/caches/project/.jaynes");
    assert!(exchange.exists(), "the exchange stays");
    assert!(!exchange.join("tmp").exists(), "the toolchains' caches are gone once the run is finished");
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/caches:greeting.txt"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "hi\n");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_finished_run_s_copy_is_cleared_after_a_while_and_its_changes_still_show() {
    let s = scratch("sweep");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    start(&s, "old", &agent);
    assert!(s.home.join("runs/old/project/.git").exists());
    // the next start clears the copies of runs finished longer ago than the time kept (none kept here)
    let role = root().join("crates/shell/tests/roles/small-change");
    let out = jaynes()
        .args(["start", "--run", "new", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .env("JAYNES_KEEP_COPIES_SECONDS", "0")
        .output().expect("start");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!s.home.join("runs/old/project").exists(), "the finished run's copy is cleared");
    assert!(s.home.join("runs/new/project/.git").exists(), "the run just started keeps its copy");
    // the page still shows what the old run changed
    let (_page, port) = page_on_any_port(
        jaynes()
        .arg("page")
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PAGE_KEY", "k3y").env("JAYNES_NO_UPDATE_CHECK", "1"),
    );
    let view = http(port, &format!("GET /run/old HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"));
    assert!(view.contains("Changes made at “change”") && view.contains("<span class=\"add\"><i>+</i>hi</span>"), "{view}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_role_s_own_check_is_a_script_run_in_the_box_judging_by_its_exit() {
    let s = scratch("script");
    let role = s.home.parent().expect("base").join("odd");
    std::fs::create_dir_all(role.join("checks")).expect("dir");
    std::fs::write(role.join("role.toml"), r#"
role = "odd"
version = "1.0.0"

[[step]]
name = "say"
reach = { project = "read" }
instructions = "Say something about {{request}}."
checks = [{ script = "checks/odd.sh", about = "the output says nothing bad" }]
"#).expect("write");
    // reads the call on standard input; tries to write outside the box, which must fail
    std::fs::write(role.join("checks/odd.sh"), r#"
input=$(cat)
if touch "$HOME/.jaynes-escaped-the-box" 2>/dev/null; then echo "wrote outside the box"; exit 1; fi
[ -f greeting.txt ] || { echo "not run on the project"; exit 1; }
case "$input" in
  *'"bad"'*) echo "it says bad"; echo "and that is all"; exit 1 ;;
  *'"unsure"'*) echo "cannot tell"; exit 3 ;;
esac
exit 0
"#).expect("write");
    let (ok, out) = engine(&s, &["check", role.to_str().expect("path")]);
    assert!(ok, "{out}");
    let verdict = |said: &str| -> Value {
        let call = serde_json::json!({"jaynes": 1, "id": "x", "call": "check", "step": "say", "check": "odd", "request": "r",
            "output": {"word": said}, "given": [], "project": s.project, "source": s.project});
        let mut child = jaynes()
            .args(["role-serve", role.to_str().expect("path")])
            .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
            .spawn().expect("serve");
        use std::io::Write;
        child.stdin.take().expect("stdin").write_all(format!("{call}\n").as_bytes()).expect("write");
        let out = child.wait_with_output().expect("out");
        serde_json::from_slice::<Value>(&out.stdout).expect("reply")["verdict"].clone()
    };
    assert_eq!(verdict("good")["verdict"], "pass", "{}", verdict("good"));
    let bad = verdict("bad");
    assert_eq!(bad["verdict"], "fail");
    assert_eq!(bad["reasons"], serde_json::json!(["it says bad", "and that is all"]));
    let unsure = verdict("unsure");
    assert_eq!((unsure["verdict"].as_str(), unsure["reason"].as_str()), (Some("blocked"), Some("cannot tell")));
    assert!(!Path::new(&std::env::var("HOME").expect("home")).join(".jaynes-escaped-the-box").exists());
}

#[test]
fn a_role_whose_promise_does_not_hold_is_refused_with_the_path_that_breaks_it() {
    let s = scratch("promise");
    let role = s.home.parent().expect("base").join("asks-late");
    std::fs::create_dir_all(&role).expect("dir");
    let text = std::fs::read_to_string(root().join("crates/shell/tests/roles/small-change/role.toml")).expect("read")
        .replace("role = \"small-change\"", "role = \"asks-late\"")
        + "\n[[promise]]\nsays = \"Nothing is delivered unless you answered at change\"\nnever = \"deliver\"\nunless = \"person answered at change\"\n";
    std::fs::write(role.join("role.toml"), text).expect("write");
    let (ok, out) = engine(&s, &["check", role.to_str().expect("path")]);
    assert!(!ok, "{out}");
    assert!(out.contains("✔ \"Nothing is delivered before the project's suite passed on the change\": proved"), "{out}");
    assert!(out.contains("✘ \"Nothing is delivered unless you answered at change\" (role.toml:"), "{out}");
    assert!(out.contains("low risk → runner's reach is tested → runner done → change's reach is tested → change delivers a small change → change's checks pass → change's stop asks only from high risk → deliver, with no answer of yours at change"), "{out}");
    // the stop it rests on asks only from high risk: the line under the path says how a role tells what holds then
    assert!(out.contains("   a run at low risk does not take the stop of change: if the role means that, say in not_taken what holds on such a run"), "{out}");
    // the real-user, 22/11: "Instalar o fix-bug levou uns 10 min em silêncio": checking and installing say what they
    // prove and how far the search is
    let proving = "Proving the 2 promises of asks-late over every way a run of it can go, up to 2,000,000 states:";
    assert!(out.contains(proving) && out.contains("  searched ") && out.contains(" states so far, of at most 2,000,000"), "{out}");
    let (ok, out) = engine(&s, &["add", role.to_str().expect("path")]);
    assert!(!ok && out.contains("a promise of the role does not hold, so it was not installed"), "{out}");
    assert!(out.contains(proving), "{out}");
    let (_, roles) = engine(&s, &["roles"]);
    assert!(!roles.contains("asks-late"), "{roles}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_step_done_in_the_record_keeps_its_change_when_the_process_dies_before_committing_it() {
    let s = scratch("diedone");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let role = root().join("crates/shell/tests/roles/small-change");
    let out = jaynes()
        .args(["start", "--run", "diedone", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi instead of hello", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .env("JAYNES_DIE_AFTER_DONE", "change")
        .output().expect("start");
    assert_eq!(out.status.code(), Some(9), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(types(&events(&s, "diedone")).contains(&"StepDone".to_string()));
    let (ok, said) = engine(&s, &["continue", "diedone"]);
    assert!(ok && said.contains("finished: delivered"), "{said}");
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/diedone:greeting.txt"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "hi\n", "the delivery lost the change of a step the record says is done");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_merge_that_landed_before_the_process_died_lands_once_and_can_be_undone() {
    let s = bug_project("diemerge");
    let prompts = s.home.join("prompts");
    std::fs::create_dir_all(&prompts).expect("dir");
    let agent = root().join("crates/shell/tests/agents/fix-bug.sh");
    let out = jaynes()
        .args(["start", "--run", "diemerge", "--role", root().join("crates/shell/tests/roles/fix-bug").to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "greet should say hi with the name", "--agent", &format!("command:sh {}", agent.display())])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "").env("PROMPTS", &prompts)
        .output().expect("start");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // the person merges, and the process that merges dies once the merge landed
    let (code, said) = merge_as_person(&s, "diemerge", |c: &mut Command| {
        fix_bug_env(&s)(c);
        c.env("JAYNES_DIE_AFTER_MERGE", "1");
    });
    assert_eq!(code, Some(9), "{said}");
    // the delivery made again a second later is made of new commits, with the same change
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let mut c = jaynes();
    c.args(["continue", "diemerge"]).env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "").env("PROMPTS", &prompts);
    let said = String::from_utf8_lossy(&c.output().expect("continue").stdout).to_string();
    assert!(said.contains("delivered"), "{said}");
    let log = Command::new("git").arg("-C").arg(&s.project).args(["log", "--format=%s", "main"]).output().expect("git");
    let merges = String::from_utf8_lossy(&log.stdout).lines().filter(|l| l.contains("merged")).count();
    assert_eq!(merges, 1, "the fix landed {merges} times: {said}");
    // the merge made before the process died is the run's, so Undo reverts it
    let undo = tty()
        .args([BIN, "undo", "diemerge", "--note", "not now"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .output().expect("undo");
    assert!(String::from_utf8_lossy(&undo.stdout).contains("undone"), "{}", String::from_utf8_lossy(&undo.stdout));
    assert_eq!(std::fs::read_to_string(s.project.join("greet.sh")).expect("read"), "echo \"hello $1\"\n");
}

/// An index in a git repository, with small-change in it under Apache-2.0, its entry written by `jaynes index check --write`.
fn an_index(s: &Scratch) -> PathBuf {
    let index = s.home.parent().expect("base").join("index");
    std::fs::create_dir_all(index.join("roles")).expect("dir");
    let from = root().join("crates/shell/tests/roles/small-change");
    let ok = Command::new("cp").arg("-R").arg(&from).arg(index.join("roles/small-change")).status().expect("cp").success();
    assert!(ok);
    std::fs::write(index.join("index.toml"), "[[role]]\nname = \"small-change\"\nversion = \"4.0.0\"\nsource = \"roles/small-change\"\nfingerprint = \"\"\n").expect("write");
    std::fs::write(index.join("licenses.toml"), "small-change = \"Apache-2.0\"\n").expect("write");
    let (ok, out) = engine(s, &["index", "check", index.to_str().expect("path"), "--write"]);
    assert!(ok, "{out}");
    let (ok, out) = engine(s, &["index", "check", index.to_str().expect("path")]);
    assert!(ok && out.contains("✔ small-change 4.0.0") && out.contains("proved"), "{out}");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "index"]] {
        assert!(Command::new("git").arg("-C").arg(&index).args(&args).status().expect("git").success());
    }
    index
}

fn engine_with_index(s: &Scratch, index: &str, args: &[&str]) -> (bool, String) {
    let out = jaynes()
        .args(args)
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .env("JAYNES_INDEX", index)
        .output()
        .expect("engine");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn a_role_is_installed_by_name_from_the_index_once_its_entry_holds() {
    let s = scratch("index");
    let index = an_index(&s);
    let url = format!("file://{}", index.display());
    let (ok, out) = engine_with_index(&s, &url, &["add", "small-change"]);
    assert!(ok && out.contains("role small-change installed"), "{out}");
    let origin = std::fs::read_to_string(s.home.join("roles/small-change/.jaynes-origin")).expect("origin");
    assert!(origin.contains("\"index:small-change\""), "{origin}");
    let (ok, out) = engine_with_index(&s, &url, &["add", "no-such-role"]);
    assert!(!ok && out.contains("the index has no role no-such-role"), "{out}");
}

#[test]
fn an_index_entry_that_does_not_hold_installs_nothing() {
    let s = scratch("badindex");
    let index = an_index(&s);
    let entry = index.join("index.toml");
    let listed = std::fs::read_to_string(&entry).expect("read");
    // a promise the index lists and the role does not make
    std::fs::write(&entry, listed.replace("promises = [", "promises = [\"Nothing is ever delivered\", ")).expect("write");
    let (ok, out) = engine_with_index(&s, index.to_str().expect("path"), &["add", "small-change"]);
    assert!(!ok && out.contains("does not hold, so it was not installed") && out.contains("Nothing is ever delivered"), "{out}");
    // the role's content changed after it was listed
    std::fs::write(&entry, &listed).expect("write");
    let role = index.join("roles/small-change/role.toml");
    std::fs::write(&role, std::fs::read_to_string(&role).expect("read") + "\n# changed\n").expect("write");
    let (ok, out) = engine_with_index(&s, index.to_str().expect("path"), &["add", "small-change"]);
    assert!(!ok && out.contains("content is not what the index lists"), "{out}");
    let (ok, out) = engine(&s, &["index", "check", index.to_str().expect("path")]);
    assert!(!ok && out.contains("✘ small-change"), "{out}");
    assert!(!s.home.join("roles/small-change").exists());
}

#[test]
fn add_says_a_role_s_license_before_installing_it_and_a_role_without_one_is_refused() {
    let s = scratch("license");
    let index = an_index(&s);
    let (ok, out) = engine_with_index(&s, index.to_str().expect("path"), &["add", "small-change"]);
    assert!(ok, "{out}");
    let said = out.find("license: Apache-2.0").unwrap_or_else(|| panic!("add does not say the license: {out}"));
    assert!(said < out.find("role small-change installed").expect("installed"), "the license comes after the install: {out}");
    let _gone = std::fs::remove_dir_all(s.home.join("roles/small-change"));
    // a role the index lists with no license: the index check refuses it, and add installs nothing
    std::fs::write(index.join("licenses.toml"), "").expect("write");
    let (ok, out) = engine(&s, &["index", "check", index.to_str().expect("path")]);
    assert!(!ok && out.contains("✘ small-change") && out.contains("no license"), "{out}");
    let (ok, out) = engine_with_index(&s, index.to_str().expect("path"), &["add", "small-change"]);
    assert!(!ok && out.contains("no license"), "{out}");
    assert!(!s.home.join("roles/small-change").exists());
}

#[test]
fn a_role_from_a_folder_of_this_machine_installs_without_a_license_and_add_says_none_is_named() {
    // the index's rule is the index's: a role of one's own, from a folder, is installed as before
    let s = scratch("ownrole");
    let own = s.home.parent().expect("base").join("own-role");
    let ok = Command::new("cp").arg("-R").arg(root().join("crates/shell/tests/roles/small-change")).arg(&own).status().expect("cp").success();
    assert!(ok);
    let (ok, out) = engine(&s, &["add", own.to_str().expect("path")]);
    assert!(ok && out.contains("role small-change installed"), "{out}");
    let said = out.find("no license named").unwrap_or_else(|| panic!("add does not say no license is named: {out}"));
    assert!(said < out.find("role small-change installed").expect("installed"), "{out}");
}

#[test]
fn a_field_of_the_index_this_jaynes_does_not_know_breaks_nothing_and_is_kept() {
    // so a field added to the index later does not break the engines already installed
    let s = scratch("newfield");
    let index = an_index(&s);
    let file = index.join("index.toml");
    let listed = std::fs::read_to_string(&file).expect("read");
    std::fs::write(&file, format!("format = 2\n{}", listed.replace("[[role]]\n", "[[role]]\nmaintainer = \"someone\"\n"))).expect("write");
    let (ok, out) = engine(&s, &["index", "check", index.to_str().expect("path")]);
    assert!(ok && out.contains("✔ small-change"), "{out}");
    assert!(out.contains("does not know") && out.contains("format") && out.contains("maintainer"), "it does not say so: {out}");
    let (ok, out) = engine_with_index(&s, index.to_str().expect("path"), &["add", "small-change"]);
    assert!(ok && out.contains("role small-change installed"), "{out}");
    // written again, the fields it does not know stay
    let (ok, out) = engine(&s, &["index", "check", index.to_str().expect("path"), "--write"]);
    assert!(ok, "{out}");
    let written = std::fs::read_to_string(&file).expect("read");
    assert!(written.contains("format = 2") && written.contains("maintainer = \"someone\""), "{written}");
}

/// The agent of the directions test role: two pages and an output naming them.
const DRAW: &str = "mkdir -p design && printf '<html><body style=\"background:#1d4d3a\"><h1>Ink</h1></body></html>' > design/ink.html \
    && printf '<html><body style=\"background:#f2c14e\"><h1>Sun</h1></body></html>' > design/sun.html \
    && printf '{\"directions\":[{\"name\":\"Ink\",\"why\":\"dark green\",\"page\":\"design/ink.html\"},{\"name\":\"Sun\",\"why\":\"warm yellow\",\"page\":\"design/sun.html\"}]}' > .jaynes/output.json";

fn copy_of(s: &Scratch, run: &str) -> PathBuf {
    let runs = s.home.join("runs").join(run);
    let found = walk(&runs).into_iter().find(|p| p.ends_with(".git") && p.is_dir()).expect("the run's copy");
    found.parent().expect("copy").to_path_buf()
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        out.push(p.clone());
        if p.is_dir() && !p.ends_with(".git") && !p.is_symlink() {
            out.extend(walk(&p));
        }
    }
    out
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands and a Chrome in it, which Linux does not have yet (slice 2)")]
fn a_page_shown_as_a_picture_is_photographed_by_jaynes() {
    if skipped_without_a_test_browser() {
        return;
    }
    let s = scratch("photo");
    let role = root().join("crates/shell/tests/roles/directions");
    let (ok, out) = engine(&s, &["start", "--run", "photo", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                                 "--request", "two looks", "--agent", &format!("command:{DRAW}")]);
    assert!(ok && out.contains("Which direction goes on?") && out.lines().any(|l| l.trim() == "- Ink"), "{out}");
    // each page was photographed, kept by its fingerprint as a picture, and left in the copy for later agents
    let kept: Vec<String> = std::fs::read_dir(s.home.join("pictures")).expect("kept").flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    assert_eq!(kept.iter().filter(|k| k.ends_with(".png")).count(), 2, "{kept:?}");
    // the page itself is kept beside its photograph, so the person can open it
    let pages: Vec<&String> = kept.iter().filter(|k| k.ends_with(".html")).collect();
    assert_eq!(pages.len(), 2, "{kept:?}");
    let copy = copy_of(&s, "photo");
    let photo = std::fs::read(copy.join(".jaynes/pictures/design/ink.png")).expect("the photograph is in the copy");
    assert_eq!(&photo[1..4], b"PNG");
    // on the page the photograph opens the page, served apart: sandboxed, so what an agent wrote cannot answer
    let (_page, port) = page_on_any_port(
        jaynes()
        .arg("page")
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PAGE_KEY", "k3y"),
    );
    let html = http(port, &format!("GET / HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"));
    let link = html.split("<a class=\"open\" href=\"/page/").nth(1).and_then(|r| r.split('"').next()).expect("a link to the page").to_string();
    assert!(pages.contains(&&link), "{link} {pages:?}");
    let served = http(port, &format!("GET /page/{link} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"));
    assert!(served.starts_with("HTTP/1.1 200") && served.contains("<h1>"), "{served}");
    assert!(served.contains("Content-Security-Policy: sandbox allow-scripts;"), "{served}");
    // only a kept page by its fingerprint is served, never a path
    assert!(http(port, &format!("GET /page/..%2Fpage.key HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n")).starts_with("HTTP/1.1 404"));
}

/// The width and height a PNG says in its header.
fn png_size(bytes: &[u8]) -> (u32, u32) {
    let at = |i: usize| u32::from_be_bytes(bytes[i..i + 4].try_into().expect("four bytes"));
    (at(16), at(20))
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands and a Chrome in it, which Linux does not have yet (slice 2)")]
fn asked_a_screen_drawn_for_a_phone_is_photographed_whole_at_a_phone_s_window() {
    // the real-user, 22/11: a phone's layouts photographed at a computer's window; the design-lead, 22/11: the
    // identity board is taller than the window, and the stop's photograph left its fonts out
    if skipped_without_a_test_browser() {
        return;
    }
    let s = scratch("phone");
    let draw = "mkdir -p design && printf '<html><body style=\"margin:0\"><h1>Top</h1><div style=\"height:2000px\"></div><p style=\"background:#b3261e;color:#fff\">Bottom</p></body></html>' > design/tall.html \
        && printf '<html><body><h1>Short</h1></body></html>' > design/short.html \
        && printf '{\"device\":\"phone\",\"layouts\":[{\"name\":\"Tall\",\"page\":\"design/tall.html\",\"device\":\"phone\"},{\"name\":\"Short\",\"page\":\"design/short.html\",\"device\":\"computer\"}]}' > .jaynes/output.json";
    let role = root().join("crates/shell/tests/roles/phone-screens");
    let (ok, out) = engine(&s, &["start", "--run", "phone", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                                 "--request", "the home screen, on the phone", "--agent", &format!("command:{draw}")]);
    assert!(ok && out.contains("Which layout goes on?"), "{out}");
    let copy = copy_of(&s, "phone");
    // at a phone's width, and the whole page down to its last line, not the window's 844
    let (w, h) = png_size(&std::fs::read(copy.join(".jaynes/pictures/design/tall.png")).expect("photographed"));
    assert_eq!(w, 390);
    assert!(h > 2000, "{h}");
    // a page shorter than the window keeps the window's height; each item's own device names its size
    let (w, h) = png_size(&std::fs::read(copy.join(".jaynes/pictures/design/short.png")).expect("photographed"));
    assert_eq!((w, h), (1440, 900));
    // the role's line under the question is on the stop's page, dim, before the options
    let (_page, port) = page_on_any_port(
        jaynes().arg("page").env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "").env("JAYNES_PAGE_KEY", "k3y"),
    );
    let get = |path: &str| http(port, &format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: jaynes_key=k3y\r\n\r\n"));
    let view = get("/run/phone");
    let at = view.find("href=\"/q/phone/").expect("a way to the stop") + 6;
    let stop = get(&view[at..at + view[at..].find('"').expect("its end")]);
    let note = stop.find("<p class=\"stop-note\">Each is drawn for the device it names.</p>").expect("the note");
    assert!(note < stop.find("data-item=\"Tall\"").expect("the options"), "{stop}");
}

/// Answer as the person, at a terminal.
fn as_person(s: &Scratch, args: &[&str]) -> String {
    let mut all = vec![BIN];
    all.extend_from_slice(args);
    let out = tty()
        .args(&all)
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .output()
        .expect("script");
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn question_of(s: &Scratch, run: &str, n: usize) -> Value {
    events(s, run).into_iter().filter(|e| e["type"] == "QuestionOpened").nth(n).expect("asked")["question"].clone()
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands and a Chrome in it, which Linux does not have yet (slice 2)")]
fn choosing_a_direction_keeps_it_alone_without_the_agent_and_the_run_goes_on() {
    if skipped_without_a_test_browser() {
        return;
    }
    let s = scratch("choose");
    let calls = s.home.parent().expect("base").join("calls.txt");
    let agent = format!("echo x >> {} && {DRAW}", calls.display());
    let role = root().join("crates/shell/tests/roles/directions");
    let (ok, out) = engine(&s, &["start", "--run", "choose", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                                 "--request", "two looks", "--agent", &format!("command:{agent}")]);
    assert!(ok && out.contains("Which direction goes on?"), "{out}");
    let q = question_of(&s, "choose", 0);
    assert_eq!(q["choose"], true, "{q}");
    let said = as_person(&s, &["answer", "choose", q["id"].as_str().expect("id"), "--choose", "Sun", "--send"]);
    assert!(said.contains("“Sun” is chosen") && said.contains("keeps only what you chose"), "{said}");
    assert!(said.contains("finished: delivered"), "{said}");
    // the agent worked twice: directions and apply; keeping Sun alone was Jaynes's
    assert_eq!(std::fs::read_to_string(&calls).expect("calls").lines().count(), 2);
    assert_eq!(events(&s, "choose").iter().filter(|e| e["type"] == "QuestionOpened").count(), 1, "not asked again");
    // apply read Sun alone
    let given = std::fs::read_to_string(copy_of(&s, "choose").join(".jaynes/inputs/directions.json")).expect("given");
    assert!(given.contains("Sun") && !given.contains("Ink"), "{given}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands and a Chrome in it, which Linux does not have yet (slice 2)")]
fn choosing_with_a_note_has_the_agent_combine_and_asks_about_the_result_alone() {
    if skipped_without_a_test_browser() {
        return;
    }
    let s = scratch("adjust");
    // the second time it draws, the agent gives Sun alone with Ink's colour
    let agent = format!(
        "if [ -f .jaynes/previous.json ] && grep -q Ink .jaynes/previous.json && ! [ -f .jaynes/inputs/directions.json ]; then \
         printf '{{\"directions\":[{{\"name\":\"Sun\",\"why\":\"warm with dark green\",\"page\":\"design/sun.html\"}}]}}' > .jaynes/output.json; \
         printf '<html><body style=\"background:#1d4d3a\"><h1>Sun</h1></body></html>' > design/sun.html; else {DRAW}; fi"
    );
    let role = root().join("crates/shell/tests/roles/directions");
    let (ok, out) = engine(&s, &["start", "--run", "adjust", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                                 "--request", "two looks", "--agent", &format!("command:{agent}")]);
    assert!(ok, "{out}");
    let q = question_of(&s, "adjust", 0);
    let said = as_person(&s, &["answer", "adjust", q["id"].as_str().expect("id"), "--choose", "Sun", "--adjust", "with the green of Ink", "--send"]);
    assert!(said.contains("adjusted as you wrote"), "{said}");
    let q2 = question_of(&s, "adjust", 1);
    let ids: Vec<&str> = q2["items"].as_array().expect("items").iter().filter_map(|x| x["id"].as_str()).collect();
    assert_eq!(ids, ["Sun"], "{q2}");
    let said = as_person(&s, &["answer", "adjust", q2["id"].as_str().expect("id"), "--choose", "Sun", "--send"]);
    assert!(said.contains("finished: delivered"), "{said}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands and a Chrome in it, which Linux does not have yet (slice 2)")]
fn an_answer_from_the_page_that_sends_a_step_back_starts_it_once() {
    if skipped_without_a_test_browser() {
        return;
    }
    let s = scratch("again");
    // sent back, the agent gives Ink alone, changed
    let agent = format!(
        "if [ -f .jaynes/previous.json ]; then \
         printf '{{\"directions\":[{{\"name\":\"Ink\",\"why\":\"greener\",\"page\":\"design/ink.html\"}}]}}' > .jaynes/output.json; \
         printf '<html><body style=\"background:#0d6d3a\"><h1>Ink</h1></body></html>' > design/ink.html; else {DRAW}; fi"
    );
    let role = root().join("crates/shell/tests/roles/directions");
    let (ok, out) = engine(&s, &["start", "--run", "again", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                                 "--request", "two looks", "--agent", &format!("command:{agent}")]);
    assert!(ok && out.contains("Which direction goes on?"), "{out}");
    let q = question_of(&s, "again", 0);
    let shown = jaynes_core::question::Question::shown(&serde_json::from_value(q.clone()).expect("question")).0;
    let (_page, port) = page_on_any_port(
        jaynes()
        .arg("page")
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1")
        .env("JAYNES_PYTHON", python())
        .env("PYTHONPATH", "")
        .env("JAYNES_PAGE_KEY", "k3y"),
    );
    // the person keeps Ink with a change: the step that drew it works again
    let id = q["id"].as_str().expect("id");
    let body = format!(r#"{{"run":"again","question":"{id}","shown":"{shown}","verdicts":{{"Ink":"change","Sun":"drop"}},"notes":{{"Ink":"greener"}}}}"#);
    let r = post(port, "/api/answer", &body, "jaynes_key=k3y");
    assert_eq!(r["ok"], true, "{r}");
    let mut asked = false;
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if events(&s, "again").iter().filter(|e| e["type"] == "QuestionOpened").count() == 2 {
            asked = true;
            break;
        }
    }
    let t = types(&events(&s, "again"));
    assert!(asked, "{t:?}");
    // the page leaves the answer to the run's process, so no attempt is begun where no agent runs it
    assert!(!t.contains(&"AttemptInterrupted".to_string()), "{t:?}");
    let started = events(&s, "again").iter().filter(|e| e["type"] == "AttemptStarted" && e["step"] == "directions").count();
    assert_eq!(started, 2, "{t:?}");
}

/// Run a role's script check through `role-serve`, as the engine does, with `call` and extra environment.
fn serve_check(s: &Scratch, role: &Path, call: Value, env: &[(&str, &str)]) -> Value {
    let mut c = jaynes();
    c.args(["role-serve", role.to_str().expect("path")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    let mut child = c.spawn().expect("serve");
    use std::io::Write;
    child.stdin.take().expect("stdin").write_all(format!("{call}\n").as_bytes()).expect("write");
    let out = child.wait_with_output().expect("out");
    serde_json::from_slice::<Value>(&out.stdout).expect("reply")["verdict"].clone()
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which is macOS's sandbox-exec")]
fn a_project_s_command_reads_nothing_the_engine_keeps_and_reaches_only_its_reserved_and_declared_local_ports() {
    let s = scratch("wall");
    std::fs::create_dir_all(&s.home).expect("home");
    // what the engine keeps that no project command may read: the page's key and log, the record, other runs, and
    // the run's own folder but for its copy of the project (its steps, a hidden step's cases among them)
    for f in ["page.key", "page.log", "app-key.pub", "record.sqlite3", "record.sqlite3-wal", "record.sqlite3-shm"] {
        std::fs::write(s.home.join(f), "secret\n").expect("write");
    }
    let own = s.home.join("runs/r-own");
    for d in ["runs/r-other", "runs/r-own/steps/cases", "runs/r-own/role"] {
        std::fs::create_dir_all(s.home.join(d)).expect("dir");
    }
    std::fs::write(s.home.join("runs/r-other/output.json"), "secret\n").expect("write");
    std::fs::write(own.join("steps/cases/output.json"), "secret\n").expect("write");
    std::fs::write(own.join("role/tool.txt"), "the run's own role\n").expect("write");
    let copy = own.join("project");
    std::fs::create_dir_all(&copy).expect("dir");
    std::fs::write(copy.join("greeting.txt"), "hello\n").expect("write");

    let role = s.home.parent().expect("base").join("wall");
    std::fs::create_dir_all(role.join("checks")).expect("dir");
    std::fs::write(role.join("role.toml"), r#"
role = "wall"
version = "1.0.0"

[[step]]
name = "look"
reach = { project = "read" }
instructions = "Look at {{request}}."
checks = [{ script = "checks/wall.py", about = "the box holds" }]
"#).expect("write");
    // Exit 0 when every wall holds, 1 naming each one that did not.
    std::fs::write(role.join("checks/wall.py"), r#"#!/usr/bin/env python3
import json, os, socket, sys
said = json.load(sys.stdin)["output"]
home, own = said["home"], said["own"]
holes = []
for f in ["page.key", "page.log", "app-key.pub", "record.sqlite3", "record.sqlite3-wal", "record.sqlite3-shm",
          "runs/r-other/output.json", "runs/r-own/steps/cases/output.json", "runs/r-own/role/tool.txt"]:
    try:
        open(os.path.join(home, f)).read()
        holes.append("read " + f)
    except OSError:
        pass
try:
    open(os.path.join(own, "project/greeting.txt")).read()
except OSError as e:
    holes.append(f"could not read the run's own copy of the project: {e}")
# a suite's own server on a port the run reserved answers it, on 127.0.0.1 and ::1
reserved = [int(p) for p in os.environ.get("JAYNES_PORTS", "").split(",") if p]
if len(reserved) < 2:
    holes.append(f"too few reserved ports: {os.environ.get('JAYNES_PORTS')!r}")
for (host, family), port in zip([("127.0.0.1", socket.AF_INET), ("::1", socket.AF_INET6)], reserved):
    mine = socket.socket(family)
    try:
        mine.bind((host, port))
        mine.listen()
        socket.create_connection((host, port), timeout=3).close()
    except OSError as e:
        holes.append(f"its own server on {host} port {port}, reserved, was not reached: {e}")
    finally:
        mine.close()
# a port of its own choosing is refused: the bind of port 0
try:
    socket.socket().bind(("127.0.0.1", 0))
    holes.append("bound port 0")
except PermissionError:
    pass
def reach(port):
    try:
        socket.create_connection(("127.0.0.1", port), timeout=3).close()
        return "reached"
    except PermissionError:
        return "refused"
    except OSError as e:
        return type(e).__name__
# what already listened when the box opened, and the page's port: refused; what the step declared: reached
for port in said["listening"] + [8750]:
    if reach(port) != "refused":
        holes.append(f"port {port}: {reach(port)}")
# no UDP, not even to its own machine
u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
try:
    u.sendto(b"", ("127.0.0.1", 9))
    holes.append("sent UDP to 127.0.0.1")
except PermissionError:
    pass
# a terminal that is not the box's: neither read nor written
for mode in (os.O_RDONLY, os.O_WRONLY):
    try:
        os.close(os.open(said["tty"], mode | os.O_NOCTTY))
        holes.append(f"opened the terminal {said['tty']}")
    except PermissionError:
        pass
if not os.path.isfile("greeting.txt"):
    holes.append("not run on the project")
print("\n".join(holes))
sys.exit(1 if holes else 0)
"#).expect("write");
    let script = role.join("checks/wall.py");
    let mut mode = std::fs::metadata(&script).expect("meta").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(&script, mode).expect("chmod");
    let (ok, out) = engine(&s, &["check", role.to_str().expect("path")]);
    assert!(ok, "{out}");

    // a terminal of the person's: a pty held open outside the box for the length of the test
    let mut holder = Command::new(python())
        .args(["-c", "import os, pty, sys, time\nm, s = pty.openpty()\nprint(os.ttyname(s), flush=True)\ntime.sleep(60)"])
        .stdout(std::process::Stdio::piped()).spawn().expect("pty");
    let mut tty = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(holder.stdout.take().expect("out")), &mut tty).expect("tty");
    // services that already listen when the box opens, on every address localhost answers on
    let listeners: Vec<std::net::TcpListener> = ["127.0.0.1:0", "[::1]:0", "0.0.0.0:0", "[::]:0"]
        .iter().map(|a| std::net::TcpListener::bind(a).expect("listen")).collect();
    let ports: Vec<u16> = listeners.iter().map(|l| l.local_addr().expect("addr").port()).collect();
    let call = serde_json::json!({"jaynes": 1, "id": "x", "call": "check", "step": "look", "check": "wall", "request": "r",
        "output": {"home": s.home, "own": own, "tty": tty.trim(), "listening": ports}, "given": [], "project": copy,
        "source": s.project});
    let held = serve_check(&s, &role, call, &[]);
    let _gone = holder.kill();
    let _reaped = holder.wait();
    assert_eq!(held["verdict"], "pass", "{held}");
    drop(listeners);
    // the planted files are not a record: CI compares the Lean model with every jaynes-e2e-*/home/record.sqlite3
    // the tests leave, so this test leaves none
    let _gone = std::fs::remove_dir_all(s.home.parent().expect("base"));
}

/// Inside another program's sandbox (Claude Code's Bash with its sandbox on: measured) macOS opens no box: the
/// project's commands are refused, saying how to start the run outside it, never run under that sandbox.
#[test]
fn inside_another_program_s_sandbox_the_project_s_commands_are_refused() {
    let script = r#"
import jaynes_box as b
from pathlib import Path
b.inside = lambda: True
b.available = lambda: True
try:
    b.command_in_box("true", b.Box((Path("/tmp"),)))
    print("ran under the other sandbox")
except b.Unavailable as e:
    print("refused:", e)
"#;
    // the variable that once ran them unboxed changes nothing here either
    let out = Command::new(python()).args(["-c", script]).env("PYTHONPATH", root().join("python")).env("JAYNES_UNBOXED", "1").output().expect("python");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!said.contains("ran under the other sandbox"), "{said}");
    assert!(said.contains("refused: Jaynes was started inside another program's sandbox and cannot put its own box around your project's commands"), "{said}");
    assert!(said.contains("mcp__plugin_jaynes_jaynes__start") && said.contains("in an ordinary terminal"), "both ways out, each named: {said}");

}

/// A role that is a program runs with the person's rights, outside any box: jaynes add says so, and says nothing of
/// the kind for a role in role.toml, whose commands stay in the box.
#[test]
fn adding_a_role_that_is_a_program_says_it_runs_with_your_rights() {
    let s = scratch("add-program");
    let add = |role: &str| engine(&s, &["add", root().join("crates/shell/tests/roles").join(role).to_str().expect("path")]);
    let (ok, out) = add("looks");
    assert!(ok, "{out}");
    assert!(out.contains("looks is a program, not a role.toml") && out.contains("runs with your rights, outside any box"), "{out}");
    let (ok, out) = add("small-change");
    assert!(ok, "{out}");
    assert!(!out.contains("runs with your rights"), "a role in role.toml is not told so: {out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the box for project commands with local ports is macOS's sandbox-exec")]
fn a_port_the_step_declares_is_reached_and_a_refused_server_on_port_0_is_named() {
    // a service of the person's that the step declares, and the page's port, declared too, which stays closed
    let service = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = service.local_addr().expect("addr").port();
    let base = std::env::temp_dir().join(format!("jaynes-declared-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("project")).expect("dir");
    let script = format!(r#"
import json, sys
sys.path.insert(0, {lib:?})
import jaynes_box as box, jaynes_menu as menu
reach = "import socket, sys\nfor p in sys.argv[1:]:\n    try:\n        socket.create_connection(('127.0.0.1', int(p)), timeout=3).close(); print(p, 'reached')\n    except OSError as e:\n        print(p, type(e).__name__)"
open({dir:?} + "/project/reach.py", "w").write(reach)
open({dir:?} + "/project/serve.py", "w").write("import http.server\nhttp.server.HTTPServer(('127.0.0.1', 0), http.server.BaseHTTPRequestHandler)")
ran = box.run(__import__("pathlib").Path({dir:?} + "/project"), "python3 reach.py {port} 8750", local_ports=[{port}, 8750])
served = menu._box({{"project": {dir:?} + "/project", "reach": {{"local_ports": [{port}]}}}}, "python3 serve.py")
print(json.dumps({{"reach": ran.printed, "local_ports": ran.local_ports, "served": getattr(served, "reason", "ran")}}))
"#, lib = root().join("python"), dir = base.display().to_string(), port = port);
    let out = Command::new(python()).args(["-c", &script]).output().expect("python");
    let said: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&out.stderr)));
    let reach = said["reach"].as_str().unwrap_or("");
    assert!(reach.contains(&format!("{port} reached")), "{said}");
    assert!(reach.contains("8750 PermissionError"), "the page's port opens for no declaration: {said}");
    assert_eq!(said["local_ports"], serde_json::json!([port]), "{said}");
    // a test server on port 0 is refused the bind: blocked, the machine's fault, saying what to do instead
    let served = said["served"].as_str().unwrap_or("");
    assert!(served.contains("JAYNES_PORTS") && served.contains("local_ports") && served.contains("local port 0"), "{said}");
    drop(service);
    let _gone = std::fs::remove_dir_all(&base);
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the box for project commands with local ports is macOS's sandbox-exec")]
fn in_25_profiles_the_box_writes_a_command_reaches_exactly_the_local_ports_it_lists() {
    // ten listeners outside the box; each profile, as profile() writes it, lists some of them as reserved and some as
    // declared, in a random order: the command inside reaches those and no other
    let script = format!(r#"
import json, random, socket, subprocess, sys, tempfile, threading
from pathlib import Path
sys.path.insert(0, {lib:?})
import jaynes_box as box
listeners = []
for _ in range(10):
    s = socket.socket(); s.bind(("127.0.0.1", 0)); s.listen(64); listeners.append(s)
    threading.Thread(target=lambda s=s: [s.accept()[0].close() for _ in iter(int, 1)], daemon=True).start()
ports = [s.getsockname()[1] for s in listeners]
client = "import socket, sys\nout = []\nfor p in sys.argv[1:]:\n    s = socket.socket(); s.settimeout(2)\n    try:\n        s.connect(('127.0.0.1', int(p))); out.append(int(p))\n    except OSError: pass\n    s.close()\nprint(out)"
rng = random.Random(18)
wrong = []
for trial in range(25):
    listed = rng.sample(ports, rng.randint(0, 6))
    cut = rng.randint(0, len(listed))
    reserved, declared = tuple(listed[:cut]), tuple(listed[cut:])
    prof = box.profile(box.Box((Path(tempfile.mkdtemp()),), local=True, reserved=reserved, local_ports=declared))
    r = subprocess.run(["/usr/bin/sandbox-exec", "-p", prof, sys.executable, "-c", client, *map(str, ports)], capture_output=True, text=True)
    reached = set(eval(r.stdout.strip() or "None") or [])
    if reached != set(listed):
        wrong.append({{"listed": sorted(listed), "reached": sorted(reached), "err": r.stderr[-200:]}})
print(json.dumps(wrong))
"#, lib = root().join("python"));
    let out = Command::new(python()).args(["-c", &script]).output().expect("python");
    let wrong: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&out.stderr)));
    assert_eq!(wrong, serde_json::json!([]), "profiles where what was reached is not what was listed: {wrong}");
}

/// An agent that writes in the copy's .git what git runs: every hook git knows, and the configuration keys that run a
/// program. Each one that runs leaves a mark.
fn planting_agent(marks: &Path) -> String {
    let hooks = [
        "applypatch-msg", "pre-applypatch", "post-applypatch", "pre-commit", "pre-merge-commit", "prepare-commit-msg", "commit-msg",
        "post-commit", "pre-rebase", "post-checkout", "post-merge", "pre-push", "pre-receive", "update", "proc-receive", "post-receive",
        "post-update", "reference-transaction", "push-to-checkout", "pre-auto-gc", "post-rewrite", "sendemail-validate",
        "fsmonitor-watchman", "post-index-change",
    ];
    let m = marks.display();
    let mut s = String::new();
    s.push_str(&format!("mkdir -p .git/hooks .git/info {m} && "));
    for h in hooks {
        s.push_str(&format!("printf '#!/bin/sh\\ntouch {m}/hook-{h}\\n' > .git/hooks/{h} && chmod +x .git/hooks/{h} && "));
    }
    let run = |name: &str| format!("{m}/run-{name}.sh");
    for key in ["fsmonitor", "clean", "smudge", "diff", "pager", "ssh", "askpass"] {
        s.push_str(&format!("printf '#!/bin/sh\\ntouch {m}/config-{key}\\ncat\\n' > {} && chmod +x {} && ", run(key), run(key)));
    }
    s.push_str(&format!(
        "git config core.fsmonitor {} && git config filter.evil.clean {} && git config filter.evil.smudge {} && git config diff.external {} && \
         git config core.pager {} && git config core.sshCommand {} && git config core.askPass {} && git config core.hooksPath .git/hooks && ",
        run("fsmonitor"), run("clean"), run("smudge"), run("diff"), run("pager"), run("ssh"), run("askpass")
    ));
    s.push_str("printf '* filter=evil diff=evil\\n' > .git/info/attributes && printf 'greeting.txt filter=evil\\n' > .gitattributes && ");
    s.push_str(&format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}"));
    s
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn nothing_an_agent_writes_in_the_copy_s_git_runs_outside_its_box() {
    let s = scratch("git-hooks");
    let marks = s.home.parent().expect("base").join("marks");
    let out = start(&s, "hooks", &planting_agent(&marks));
    // the attributes file in the tree is on the floor, so the run is high risk and delivers with its stop open
    assert!(out.contains("delivered"), "{out}");
    let ran: Vec<String> = std::fs::read_dir(&marks).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with("run-")).collect()).unwrap_or_default();
    assert!(ran.is_empty(), "the engine's git ran what the agent wrote in the copy's .git: {ran:?}");
    // the change is measured as it is: the greeting and the attributes file the agent wrote in the tree
    let e = events(&s, "hooks");
    let done = e.iter().find(|e| e["type"] == "StepDone" && e["step"] == "change").expect("done");
    let mut paths: Vec<String> = done["changed"].as_array().expect("changed").iter().map(|c| c["what"].as_str().unwrap_or("").to_string()).collect();
    paths.sort();
    assert_eq!(paths, [".gitattributes", "greeting.txt"]);
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/hooks:greeting.txt"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "hi\n");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn what_an_agent_writes_in_the_copy_s_git_is_not_what_is_delivered() {
    let s = scratch("git-refs");
    // the agent commits a file of its own in the copy's .git and moves every branch there to it
    let agent = format!(
        "perl -pi -e 's/hello/hi/' greeting.txt && echo planted > planted.txt && git add planted.txt && \
         git -c user.name=a -c user.email=a@a commit -q -m planted && for r in $(git for-each-ref --format='%(refname)' refs/heads); do git update-ref $r HEAD; done && \
         git rm -q --cached planted.txt && rm planted.txt && {OUTPUT}"
    );
    let out = start(&s, "refs", &agent);
    assert!(out.contains("finished: delivered"), "{out}");
    let files = Command::new("git").arg("-C").arg(&s.project).args(["ls-tree", "-r", "--name-only", "jaynes/refs"]).output().expect("git");
    let files = String::from_utf8_lossy(&files.stdout);
    assert!(!files.contains("planted.txt"), "the delivery carries what the agent committed in the copy's .git:\n{files}");
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/refs:greeting.txt"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "hi\n");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn an_output_that_is_a_link_out_of_the_copy_is_not_read() {
    let s = scratch("output-link");
    // what the agent points at: a file of this account outside the copy, which Jaynes must never take in
    let secret = s.home.parent().expect("base").join("secret.json");
    std::fs::write(&secret, r#"{"summary":"a secret of the account","files":["greeting.txt"]}"#).expect("write");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && ln -sf {} .jaynes/output.json", secret.display());
    let (_, out) = engine(&s, &[
        "start", "--run", "link", "--role", root().join("crates/shell/tests/roles/small-change").to_str().expect("path"),
        "--project", s.project.to_str().expect("path"), "--request", "say hi", "--agent", &format!("command:{agent}"),
    ]);
    assert!(out.contains("is not a plain file"), "{out}");
    let (_, log) = engine(&s, &["log", "link"]);
    assert!(!log.contains("a secret of the account"), "the record holds what the link pointed at");
}

/// A marker program: it writes its name in `marks` and passes its input through, as a filter would.
fn marker(marks: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(marks).expect("marks");
    let p = marks.join(format!("run-{name}.sh"));
    std::fs::write(&p, format!("#!/bin/sh\ntouch {}/ran-{name}\ncat\n", marks.display())).expect("write");
    std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("chmod");
    p
}

fn ran(marks: &Path) -> Vec<String> {
    std::fs::read_dir(marks).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("ran-")).collect()).unwrap_or_default()
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_repository_an_agent_nests_in_the_copy_is_refused_and_nothing_of_it_runs() {
    let s = scratch("git-nested");
    let marks = s.home.parent().expect("base").join("marks");
    let (fsmon, clean) = (marker(&marks, "fsmonitor"), marker(&marks, "clean"));
    let hook = marker(&marks, "post-index-change");
    let agent = format!(
        // the agent makes the nested repository first and sets up what runs after, so nothing it runs itself leaves a mark
        "perl -pi -e 's/hello/hi/' greeting.txt && mkdir -p sub && cd sub && git init -q && echo x > f.txt && printf 'f.txt filter=mine\\n' > .gitattributes && \
         git add -A && git -c user.name=a -c user.email=a@a commit -q -m nested && git config core.fsmonitor {fsmon} && \
         git config filter.mine.clean {clean} && cp {hook} .git/hooks/post-index-change && echo y > f.txt && cd .. && {OUTPUT}",
        fsmon = fsmon.display(), clean = clean.display(), hook = hook.display()
    );
    let out = start(&s, "nested", &agent);
    assert!(out.contains("sub/.git"), "the nested repository is not named as refused: {out}");
    assert!(ran(&marks).is_empty(), "the engine's git ran what the nested repository set up: {:?}", ran(&marks));
    let e = events(&s, "nested");
    assert!(!e.iter().any(|e| e["type"] == "StepDone" && e["step"] == "change"), "a change with a nested repository was taken");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_engine_s_git_reads_none_of_the_person_s_own_configuration_on_the_copy() {
    let s = scratch("git-global");
    let marks = s.home.parent().expect("base").join("marks");
    // the person's own configuration defines a filter, as git-lfs does; the agent's attributes call it on every file
    let home = s.home.parent().expect("base").join("person-home");
    std::fs::create_dir_all(&home).expect("home");
    let clean = marker(&marks, "global-clean");
    std::fs::write(home.join(".gitconfig"), format!("[filter \"x\"]\n\tclean = {}\n\tsmudge = {}\n", clean.display(), clean.display())).expect("config");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && printf '* filter=x\\n' > .gitattributes && {OUTPUT}");
    let role = root().join("crates/shell/tests/roles/small-change");
    let out = Command::new(BIN)
        .args(["start", "--run", "global", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
               "--request", "say hi", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .env("HOME", &home)
        .output()
        .expect("engine");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(said.contains("delivered"), "{said}");
    assert!(ran(&marks).is_empty(), "the engine's git ran a filter of the person's own configuration on the agent's files: {:?}", ran(&marks));
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_git_file_pointing_elsewhere_and_a_bare_repository_in_the_tree_change_nothing_the_engine_does() {
    let s = scratch("git-pointer");
    let marks = s.home.parent().expect("base").join("marks");
    let hook = marker(&marks, "elsewhere-hook");
    let elsewhere = s.home.parent().expect("base").join("elsewhere.git");
    let ok = Command::new("git").args(["init", "-q", "--bare"]).arg(&elsewhere).status().expect("git").success();
    assert!(ok);
    std::fs::copy(&hook, elsewhere.join("hooks/post-index-change")).expect("hook");
    std::fs::copy(&hook, elsewhere.join("hooks/reference-transaction")).expect("hook");
    // the agent swaps its .git for a file pointing at another repository, and leaves a bare repository in the tree
    let agent = format!(
        "perl -pi -e 's/hello/hi/' greeting.txt && rm -rf .git && printf 'gitdir: {}\\n' > .git && git init -q --bare vendor/inner.git && \
         cp {} vendor/inner.git/hooks/post-index-change && {OUTPUT}",
        elsewhere.display(), hook.display()
    );
    let out = start(&s, "pointer", &agent);
    assert!(out.contains("delivered"), "{out}");
    assert!(ran(&marks).is_empty(), "the engine's git ran a hook of a repository the agent pointed at: {:?}", ran(&marks));
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/pointer:greeting.txt"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "hi\n");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_start_says_when_the_copy_lacks_git_lfs_content_or_submodules() {
    let s = scratch("git-notes");
    std::fs::write(s.project.join(".gitattributes"), "*.bin filter=lfs diff=lfs merge=lfs -text\n").expect("write");
    std::fs::write(s.project.join(".gitmodules"), "[submodule \"lib\"]\n\tpath = lib\n\turl = ../lib\n").expect("write");
    let out = start(&s, "notes", &format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}"));
    assert!(out.contains("note: the project keeps files in git-lfs"), "{out}");
    assert!(out.contains("note: the project has submodules"), "{out}");
    assert!(out.contains("delivered"), "the run is not refused: {out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_agent_s_git_reads_none_of_the_person_s_configuration_and_its_commits_are_not_the_delivery() {
    let s = scratch("agent-git");
    let said = s.home.parent().expect("base").join("said.txt");
    // the agent reads its history, asks for the person's identity, and commits in its own repository
    let agent = format!(
        "perl -pi -e 's/hello/hi/' greeting.txt && (git log -1 --format=%s; git config --global user.email || echo no-global; \
         git commit -qam mine && git log -1 --format='%an <%ae>') > {} 2>&1 && {OUTPUT}",
        said.display()
    );
    let out = start(&s, "agentgit", &agent);
    assert!(out.contains("delivered"), "{out}");
    let said = std::fs::read_to_string(&said).expect("said");
    let lines: Vec<&str> = said.lines().collect();
    // its global configuration is the one Jaynes writes, never the person's
    assert_eq!(lines, ["start", "agent@jaynes.invalid", "Jaynes agent <agent@jaynes.invalid>"], "{said}");
    // what the agent committed is not what is delivered: the delivery is the change Jaynes measured
    let log = Command::new("git").arg("-C").arg(&s.project).args(["log", "--format=%an %s", "jaynes/agentgit"]).output().expect("git");
    assert!(!String::from_utf8_lossy(&log.stdout).contains("Jaynes agent mine"), "{}", String::from_utf8_lossy(&log.stdout));
    let shown = Command::new("git").arg("-C").arg(&s.project).args(["show", "jaynes/agentgit:greeting.txt"]).output().expect("git");
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "hi\n");
}

/// A project's tests that make a repository of their own and give it an identity read their own names back, as when the
/// person runs them; a repository that gives none commits as Jaynes's agent.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_repository_the_agent_s_commands_make_commits_with_the_identity_it_sets_and_one_without_as_jaynes_s_agent() {
    let s = scratch("own-identity");
    let base = s.home.parent().expect("base").to_path_buf();
    let said = base.join("said.txt");
    let repos = base.join("repos");
    std::fs::create_dir_all(&repos).expect("dir");
    let commit = "echo a > a && git add a && git commit -qm one && git log -1 --format='%an <%ae>, %cn <%ce>'";
    let agent = format!(
        "perl -pi -e 's/hello/hi/' greeting.txt && (cd {r} && git init -q own && cd own && git config user.name 'quitanda tests' && \
         git config user.email tests@quitanda.test && {commit}; cd {r} && git init -q plain && cd plain && {commit}) > {} 2>&1 && {OUTPUT}",
        said.display(),
        r = repos.display()
    );
    let out = start(&s, "ownid", &agent);
    assert!(out.contains("delivered"), "{out}");
    let said = std::fs::read_to_string(&said).expect("said");
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(
        lines,
        ["quitanda tests <tests@quitanda.test>, quitanda tests <tests@quitanda.test>", "Jaynes agent <agent@jaynes.invalid>, Jaynes agent <agent@jaynes.invalid>"],
        "{said}"
    );
}

/// A role that draws a board only for the person to choose from, then changes the greeting, judged by the project's
/// tests (tests.sh, when the agent wrote one), and merges.
fn boards_role(s: &Scratch) -> (PathBuf, PathBuf) {
    let base = s.home.parent().expect("base");
    let role = base.join("boards-role");
    std::fs::create_dir_all(role.join("checks")).expect("dir");
    std::fs::write(role.join("role.toml"), r#"
role = "boards-role"
version = "1.0.0"
delivery = "merge"

[[step]]
name = "boards"
delivered = false
reach = { project = "write", write = ["design/**"] }
instructions = "Draw a board for: {{request}}"

[step.output]
summary = "string"

[[step]]
name = "change"
needs = ["boards"]
reach = { project = "write", write = ["**"], commands = true }
instructions = "Make the change: {{request}}"
checks = [{ script = "checks/tests.sh", about = "the project's tests pass", project_tests = true }]

[step.output]
summary = "string"
"#).expect("write");
    std::fs::write(role.join("checks/tests.sh"), "[ -f tests.sh ] || exit 0\nsh tests.sh || { echo \"the project's tests fail\"; exit 1; }\n").expect("write");
    // the agent draws the board; at the change, its tests read the board when the request says so
    let agent = base.join("boards-agent.sh");
    std::fs::write(&agent, r##"prompt=$(cat)
case "$prompt" in
  *"# Your step: boards"*)
    mkdir -p design && echo board > design/board.html
    printf '{"summary":"a board"}' > .jaynes/output.json ;;
  *"# Your step: change"*)
    perl -pi -e 's/hello/hi/' greeting.txt
    case "$prompt" in
      *"read the board"*) printf 'grep -q hi greeting.txt && test -f design/board.html\n' > tests.sh ;;
      *) printf 'grep -q hi greeting.txt\n' > tests.sh ;;
    esac
    printf '{"summary":"hello becomes hi"}' > .jaynes/output.json ;;
esac
echo '{"tokens":100}'
"##).expect("write");
    (role, agent)
}

/// The tech-lead's item 2 (23/11): what a role draws only for the person to choose from is never merged, and the
/// checks that ran the project's tests run once more on what is delivered, before the merge.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn a_board_drawn_only_to_choose_from_is_never_merged_and_the_tests_judge_what_is_merged() {
    for (run, request, merges) in [("boards-ok", "say hi", true), ("boards-read", "say hi; the tests read the board", false)] {
        let s = scratch(run);
        let (role, agent) = boards_role(&s);
        let out = jaynes()
            .args(["start", "--run", run, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                "--request", request, "--agent", &format!("command:sh {}", agent.display())])
            .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
            .output().expect("start");
        let started = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let (_, said) = merge_as_person(&s, run, |c: &mut Command| {
            c.env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "");
        });
        let at = |rev: &str, path: &str| {
            let o = Command::new("git").arg("-C").arg(&s.project).args(["show", &format!("{rev}:{path}")]).output().expect("git");
            o.status.success().then(|| String::from_utf8_lossy(&o.stdout).to_string())
        };
        // the run's branch has the change and its tests, never the board
        let branch = format!("jaynes/{run}");
        assert_eq!(at(&branch, "greeting.txt").as_deref(), Some("hi\n"), "{started}\n{said}");
        assert!(at(&branch, "tests.sh").is_some(), "{said}");
        assert_eq!(at(&branch, "design/board.html"), None, "the board went into the delivery\n{said}");
        assert!(said.contains("checked again on what is delivered in "), "{started}\n{said}");
        let took = said.split("checked again on what is delivered in ").nth(1).and_then(|r| r.split('s').next()).unwrap_or("?");
        println!("{run}: the checks ran again on what is delivered in {took}s");
        if merges {
            assert!(said.contains("merged as"), "{said}");
            assert_eq!(std::fs::read_to_string(s.project.join("greeting.txt")).expect("read"), "hi\n");
            assert!(!s.project.join("design").exists(), "the merge brought the board in");
        } else {
            assert!(
                said.contains("not merged: the check tests of change fails on what is delivered, without what the role only shows: the project's tests fail"),
                "{started}\n{said}"
            );
            assert_eq!(std::fs::read_to_string(s.project.join("greeting.txt")).expect("read"), "hello\n", "merged though its tests fail without the board");
        }
    }
}

/// A role whose one step changes the greeting and whose check reads the history where it runs, failing when git
/// reads another repository's.
fn history_role(s: &Scratch) -> PathBuf {
    let role = s.home.parent().expect("base").join("history-role");
    std::fs::create_dir_all(role.join("checks")).expect("dir");
    std::fs::write(role.join("role.toml"), r#"
role = "history-role"
version = "1.0.0"

[[step]]
name = "change"
reach = { project = "write", write = ["**"], commands = true }
instructions = "Make the change: {{request}}"
checks = [{ use = "field-required", field = "summary" }, { script = "checks/history.py", about = "git reads this project's history" }]

[step.output]
summary = { type = "string", about = "what changed" }
files = { type = "array", items = "string" }
"#).expect("write");
    std::fs::write(role.join("checks/history.py"), r#"#!/usr/bin/env python3
import subprocess, sys
said = subprocess.run(["git", "log", "-1", "--format=%s"], capture_output=True, text=True)
if "elsewhere" in said.stdout:
    print("git read another repository: " + said.stdout.strip())
    sys.exit(1)
"#).expect("write");
    let script = role.join("checks/history.py");
    let mut mode = std::fs::metadata(&script).expect("meta").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(&script, mode).expect("chmod");
    role
}

/// A run started from inside another repository's git (a hook, an editor, a shell that set GIT_DIR) does not have the
/// agent's git nor the checks' read and write that repository in place of the project's copy.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn git_s_variables_of_the_process_that_starts_a_run_point_neither_the_agent_nor_the_checks_at_another_repository() {
    let s = scratch("git-vars");
    let base = s.home.parent().expect("base").to_path_buf();
    let elsewhere = base.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("dir");
    for args in [&["init", "-q", "-b", "main"][..], &["-c", "user.name=t", "-c", "user.email=t@t.test", "commit", "-q", "--allow-empty", "-m", "elsewhere"]] {
        assert!(Command::new("git").arg("-C").arg(&elsewhere).args(args).status().expect("git").success());
    }
    let said = base.join("said.txt");
    let role = history_role(&s);
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && git log -1 --format=%s > {} 2>&1 && {OUTPUT}", said.display());
    let git = elsewhere.join(".git");
    let out = jaynes()
        .args(["start", "--run", "gitvars", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
            "--request", "say hi instead of hello", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .env("GIT_DIR", &git).env("GIT_WORK_TREE", &elsewhere).env("GIT_INDEX_FILE", git.join("index"))
        .output().expect("start");
    let out = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let said = std::fs::read_to_string(&said).unwrap_or_default();
    assert_eq!(said.trim(), "start", "the agent's git read another repository\n{out}");
    let e = events(&s, "gitvars");
    let judged = e.iter().rev().find(|x| x["type"] == "ChecksJudged").unwrap_or_else(|| panic!("{:?}\n{out}", types(&e)));
    let history = judged["checks"].as_array().into_iter().flatten().find(|c| c["check"] == "history").expect("history");
    assert_eq!(history["passed"], true, "the check's git read another repository: {judged}\n{out}");
    assert!(out.contains("delivered"), "{out}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the box for project commands is macOS's sandbox-exec here")]
fn a_project_s_command_in_the_box_hands_no_job_to_launchd() {
    // launchctl submit and bootstrap work outside any box (measured on 18/11): a job they hand to launchd runs
    // outside the box, with the person's rights, and outlives the run. In the box both must be refused; a later
    // macOS that lets either through shows here. The jobs are taken away by the labels this test made, and only those.
    let base = std::env::temp_dir().join(format!("jaynes-box-launchd-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let (project, out) = (base.join("project"), base.join("out"));
    for d in [&project, &out] {
        std::fs::create_dir_all(d).expect("dir");
    }
    std::fs::write(project.join("README"), "a project\n").expect("write");
    let nonce = format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("time").as_nanos());
    let (submitted, booted) = (format!("dev.jaynes.test.{nonce}-submit"), format!("dev.jaynes.test.{nonce}-boot"));
    let uid = String::from_utf8_lossy(&Command::new("id").arg("-u").output().expect("id").stdout).trim().to_string();
    let plist = out.join("job.plist");
    std::fs::write(&plist, format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>Label</key><string>{booted}</string><key>RunAtLoad</key><true/>
<key>ProgramArguments</key><array><string>/bin/sh</string><string>-c</string><string>echo ran &gt; {o}/booted</string></array></dict></plist>
"#, o = out.display())).expect("plist");
    let command = format!("launchctl submit -l {submitted} -- /bin/sh -c 'echo ran > {o}/submitted'; echo submit=$?\nlaunchctl bootstrap gui/{uid} {plist}; echo bootstrap=$?",
                          o = out.display(), plist = plist.display());
    let script = format!(r#"
import sys
sys.path.insert(0, {lib:?})
import jaynes_box as b
from pathlib import Path
print(b.run(Path({project:?}), {command:?}, timeout=60).printed)
"#, lib = root().join("python"), project = project);
    let ran = Command::new(python()).args(["-c", &script]).output().expect("python");
    let said = String::from_utf8_lossy(&ran.stdout).to_string();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let ran_outside = [out.join("submitted").exists(), out.join("booted").exists()];
    let known = [
        Command::new("launchctl").args(["list", &submitted]).output().is_ok_and(|o| o.status.success()),
        Command::new("launchctl").args(["print", &format!("gui/{uid}/{booted}")]).output().is_ok_and(|o| o.status.success()),
    ];
    // what this test may have handed to launchd goes, by its own labels, before anything is asserted
    let _removed = Command::new("launchctl").args(["remove", &submitted]).output();
    let _out = Command::new("launchctl").args(["bootout", &format!("gui/{uid}/{booted}")]).output();
    let _gone = std::fs::remove_dir_all(&base);
    assert!(said.contains("submit=") && said.contains("bootstrap="), "the command did not run in the box: {said}{}", String::from_utf8_lossy(&ran.stderr));
    assert_eq!(ran_outside, [false, false], "a job handed to launchd from the box ran outside it (submit, bootstrap): {said}");
    assert_eq!(known, [false, false], "launchd took a job from the box (submit, bootstrap): {said}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which is macOS's sandbox-exec here")]
fn in_a_real_run_the_project_s_suite_neither_reads_nor_writes_the_pasteboard_nor_reaches_the_keychain() {
    // the path a real run takes: small-change runs the project's suite in the box on the run's copy under runs/. The
    // suite is a probe: it fails when it reads what was copied, writes the pasteboard, or searches the keychain.
    let pb = |args: &[&str], input: Option<&[u8]>| -> Option<Vec<u8>> {
        let mut c = Command::new(args[0]);
        c.args(&args[1..]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
        let mut child = c.spawn().ok()?;
        if let Some(i) = input {
            use std::io::Write;
            child.stdin.take()?.write_all(i).ok()?;
        }
        drop(child.stdin.take());
        let out = child.wait_with_output().ok()?;
        out.status.success().then_some(out.stdout)
    };
    // the find pasteboard, not the one the person copies with; what it held is put back at the end
    let saved = pb(&["/usr/bin/pbpaste", "-pboard", "find"], None);
    // the keychain's service answers outside the box: it lists the keychains (main's box let it answer, too)
    let keychain = Command::new("/usr/bin/security").arg("list-keychains").output().is_ok_and(|o| o.status.success());
    const WRITTEN: &str = "written-from-the-box";
    // one real run with the probe; what the find pasteboard held after it, and whether it answered at all
    let attempt = |name: &str| -> (bool, Option<Vec<u8>>, String) {
        let s = scratch(name);
        let token = format!("jaynes-mach-{name}-{}", std::process::id());
        let pasteboard = pb(&["/usr/bin/pbcopy", "-pboard", "find"], Some(token.as_bytes())).is_some()
            && pb(&["/usr/bin/pbpaste", "-pboard", "find"], None).as_deref() == Some(token.as_bytes());
        std::fs::write(s.project.join("probe.sh"), format!(r#"
if [ "$(/usr/bin/pbpaste -pboard find 2>/dev/null)" = "{token}" ]; then echo "read the pasteboard"; exit 1; fi
printf {WRITTEN} | /usr/bin/pbcopy -pboard find 2>/dev/null
if /usr/bin/security list-keychains >/dev/null 2>&1; then echo "reached the keychain"; exit 1; fi
grep -q hi greeting.txt
"#)).expect("write");
        std::fs::write(s.project.join("Makefile"), "test:\n\tsh probe.sh\n").expect("write");
        for args in [&["add", "-A"][..], &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "probe"][..]] {
            assert!(Command::new("git").arg("-C").arg(&s.project).args(args).status().expect("git").success());
        }
        let out = start(&s, name, &format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}"));
        let after = pb(&["/usr/bin/pbpaste", "-pboard", "find"], None);
        let e = events(&s, name);
        // the suite reading the token fails it, and so the run: what the box let through, never another app
        assert!(types(&e).iter().any(|t| t == "Delivered"), "the suite failed in the box:\n{out}\n{:?}", types(&e));
        let kept = pasteboard.then(|| after.clone().unwrap_or_default());
        (pasteboard, kept.map(|k| if k == token.as_bytes() { b"token".to_vec() } else { k }), out)
    };
    // the find pasteboard is the whole system's: another app's search may change it while the run goes on, and then
    // what it holds after says nothing of the box; once more, and said
    let (mut pasteboard, mut after, _) = attempt("mach");
    let other = |a: &Option<Vec<u8>>| a.as_deref().is_some_and(|v| v != b"token" && v != WRITTEN.as_bytes());
    if pasteboard && other(&after) {
        eprintln!("the find pasteboard changed to something else during the run (another app's search, not the box): once more");
        (pasteboard, after, _) = attempt("mach-again");
    }
    if let Some(v) = &saved {
        let _back = pb(&["/usr/bin/pbcopy", "-pboard", "find"], Some(v));
    }
    if pasteboard {
        assert_ne!(after.as_deref(), Some(WRITTEN.as_bytes()), "the suite wrote the pasteboard");
        assert!(!other(&after), "the find pasteboard changed under both runs (another app's searches): this proves nothing of the box");
    }
    assert!(pasteboard || keychain, "neither the pasteboard nor the keychain answers here, so this test proves nothing");
}

/// A test that finds someone else's program on its page's port (as one that had listened on 18082 for three days,
/// and held a whole queue of tests for 55 minutes) fails within its wait, saying what answered, and never hangs.
#[test]
fn a_page_test_that_reaches_another_program_fails_within_its_wait() {
    use std::io::{Read, Write};
    // one that takes the connection and says nothing
    let silent = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = silent.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        let held: Vec<_> = silent.incoming().take(64).flatten().collect();
        std::thread::sleep(std::time::Duration::from_secs(60));
        drop(held);
    });
    let started = std::time::Instant::now();
    assert!(std::panic::catch_unwind(|| page_up(port)).is_err(), "a silent program is not the page");
    assert!(started.elapsed() < PAGE_WAIT + std::time::Duration::from_secs(5), "it waited {:?}", started.elapsed());
    // one that answers something else is told at once
    let other = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = other.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for mut c in other.incoming().take(8).flatten() {
            let mut buf = [0u8; 512];
            let _read = c.read(&mut buf);
            let _sent = c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nnot jaynes");
        }
    });
    let started = std::time::Instant::now();
    let said = std::panic::catch_unwind(|| page_up(port)).expect_err("another program is not the page");
    let said = said.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(said.contains("not Jaynes's page") && said.contains("not jaynes"), "{said}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
}

#[test]
fn an_agent_jaynes_does_not_know_is_refused_before_anything_starts_and_never_becomes_claude_code() {
    // No model is reached however this goes: a home and a path of the test's, where the only `claude` is a stand-in
    // that fails and leaves a mark. Today an unknown agent fell back to Claude Code and spent the person's quota.
    let s = scratch("unknown-agent");
    let guard = s.base.join("guard");
    std::fs::create_dir_all(&guard).expect("dir");
    std::fs::create_dir_all(s.base.join("person")).expect("dir");
    let mark = s.base.join("claude-was-called");
    std::fs::write(guard.join("claude"), format!("#!/bin/sh\ntouch '{}'\nexit 3\n", mark.display())).expect("write");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(guard.join("claude"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let role = root().join("crates/shell/tests/roles/small-change");
    for (run, agent) in [("gemini", "gemini:x"), ("gpt", "gpt:x"), ("typo", "clade:x")] {
        let out = jaynes()
            .args(["start", "--run", run, "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                   "--request", "say hi", "--agent", agent])
            .env("HOME", s.base.join("person")).env("PATH", format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", guard.display()))
            .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
            .env_remove("JAYNES_AGENT")
            .output().expect("start");
        let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(!mark.exists(), "{agent} became Claude Code: {said}");
        assert!(!out.status.success(), "{agent} started: {said}");
        assert!(said.contains(agent) && said.contains("claude:MODEL") && said.contains("command:CMD"), "{agent}: it does not say which agents there are: {said}");
        assert!(!s.home.join("runs").join(run).exists(), "{agent}: a run was recorded");
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the box for project commands with local ports is macOS's sandbox-exec")]
fn only_the_person_opens_a_project_s_local_ports_and_only_the_engine_s_call_opens_the_box() {
    let s = scratch("open-ports");
    let project = s.project.to_str().expect("path");
    // a program without a terminal is an agent: it may not open them, nor close them
    let (ok, out) = engine(&s, &["project", "open-ports", project]);
    assert!(!ok && out.contains("only the person opens or closes"), "{out}");
    let (_, said) = engine(&s, &["project", "ports", project]);
    assert!(said.contains("reach only the ports a run reserves"), "{said}");
    // the person, at a terminal, opens them
    let opened = tty()
        .args([BIN, "project", "open-ports", project])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python())
        .output().expect("script");
    let opened = String::from_utf8_lossy(&opened.stdout).to_string();
    assert!(opened.contains("opened by you on") && opened.contains("the Jaynes page's port among them"), "{opened}");
    let (_, said) = engine(&s, &["project", "ports", project]);
    assert!(said.contains("this project's checks reach any local port of this machine: opened by you on"), "{said}");
    // a run says it before it starts, records it in its setup, and shows it
    let out = start(&s, "open", OUTPUT);
    assert!(out.contains("this project's checks reach any local port of this machine"), "{out}");
    let (_, shown) = engine(&s, &["show", "open"]);
    assert!(shown.contains("this project's checks reach any local port"), "{shown}");

    // the box opens on the engine's word in the call, and on nothing else: not a variable, not a file
    let role = s.home.parent().expect("base").join("open");
    std::fs::create_dir_all(role.join("checks")).expect("dir");
    std::fs::write(role.join("role.toml"), r#"
role = "open"
version = "1.0.0"

[[step]]
name = "look"
reach = { project = "read" }
instructions = "Look at {{request}}."
checks = [{ script = "checks/open.py", about = "a server on port 0 is reached" }]
"#).expect("write");
    std::fs::write(role.join("checks/open.py"), r#"#!/usr/bin/env python3
import json, os, socket, sys
said = json.load(sys.stdin)["output"]
# what the person chose for each project: no command reads or writes it
for mode in ("r", "w"):
    try:
        open(os.path.join(said["home"], "projects", "x.toml"), mode).close()
        print(f"opened the projects' folder to {mode}")
        sys.exit(1)
    except OSError:
        pass
mine = socket.socket()
mine.bind(("127.0.0.1", 0))
mine.listen()
socket.create_connection(("127.0.0.1", mine.getsockname()[1]), timeout=3).close()
"#).expect("write");
    let script = role.join("checks/open.py");
    let mut mode = std::fs::metadata(&script).expect("meta").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(&script, mode).expect("chmod");
    // a file in the project that says the ports are open is just a file
    std::fs::write(s.project.join("jaynes-ports.toml"), "[ports]\nany = true\n").expect("write");
    let call = |any: Option<bool>| {
        let mut c = serde_json::json!({"jaynes": 1, "id": "x", "call": "check", "step": "look", "check": "open", "request": "r",
            "output": {"home": s.home}, "given": [], "project": s.project, "source": s.project});
        if let Some(a) = any {
            c["any_port"] = serde_json::json!(a);
        }
        c
    };
    let open = serve_check(&s, &role, call(Some(true)), &[]);
    assert_eq!(open["verdict"], "pass", "{open}");
    for (said, env) in [(call(None), vec![]), (call(Some(false)), vec![]), (call(None), vec![("JAYNES_ANY_LOCAL_PORT", "1")])] {
        let shut = serve_check(&s, &role, said, &env);
        assert_eq!(shut["verdict"], "blocked", "{shut}");
        let why = shut["reason"].as_str().unwrap_or("");
        assert!(why.contains("Two ways out") && why.contains("JAYNES_PORTS") && why.contains("jaynes project open-ports"), "{why}");
    }
    // and the person closes them again
    let closed = tty()
        .args([BIN, "project", "close-ports", project])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python())
        .output().expect("script");
    assert!(String::from_utf8_lossy(&closed.stdout).contains("closed"), "{closed:?}");
    let (_, said) = engine(&s, &["project", "ports", project]);
    assert!(said.contains("reach only the ports a run reserves"), "{said}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the box for project commands with local ports is macOS's sandbox-exec")]
fn in_a_real_run_a_role_s_script_check_runs_from_the_run_s_copy_of_the_role_and_serves_on_a_reserved_port() {
    // the path a real run takes: the role pinned under runs/<run>/role, its script run in the box on the run's
    // copy of the project, a test server of its own on a port Jaynes reserved
    let s = scratch("real-run-script");
    let role = s.home.parent().expect("base").join("served");
    std::fs::create_dir_all(role.join("checks")).expect("dir");
    std::fs::write(role.join("role.toml"), r#"
role = "served"
version = "1.0.0"

[[step]]
name = "change"
reach = { project = "write", write = ["**"], commands = true }
instructions = "Make the change: {{request}}"
checks = [{ use = "field-required", field = "summary" }, { script = "checks/served.py", about = "the project serves its greeting" }]

[step.output]
summary = { type = "string", about = "what changed" }
files = { type = "array", items = "string" }
"#).expect("write");
    std::fs::write(role.join("checks/served.py"), r#"#!/usr/bin/env python3
import http.server, os, sys, threading, urllib.request
port = int(os.environ["JAYNES_PORTS"].split(",")[0])
text = open("greeting.txt").read().encode()
class Say(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.end_headers(); self.wfile.write(text)
    def log_message(self, *a):
        pass
server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Say)
threading.Thread(target=server.serve_forever, daemon=True).start()
got = urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=5).read()
server.shutdown()
if b"hi" not in got:
    print(f"the project serves {got!r}")
    sys.exit(1)
"#).expect("write");
    let script = role.join("checks/served.py");
    let mut mode = std::fs::metadata(&script).expect("meta").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(&script, mode).expect("chmod");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let (ok, out) = engine(&s, &[
        "start", "--run", "served", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
        "--request", "say hi instead of hello", "--agent", &format!("command:{agent}"),
    ]);
    assert!(ok, "{out}");
    let e = events(&s, "served");
    let judged = e.iter().find(|x| x["type"] == "ChecksJudged").unwrap_or_else(|| panic!("no check was judged: {:?}", types(&e)));
    let served = judged["checks"].as_array().into_iter().flatten().find(|c| c["check"] == "served")
        .unwrap_or_else(|| panic!("the script check was not judged: {judged}"));
    assert_eq!((served["passed"].as_bool(), served["blocked"].as_bool()), (Some(true), Some(false)), "the script check did not pass in the box: {judged}");
    assert!(types(&e).iter().any(|t| t == "Delivered"), "{:?}\n{judged}", types(&e));
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the box of a role's program is macOS's sandbox-exec here")]
fn a_role_s_program_in_its_box_closed_by_default_does_its_work_and_none_of_the_escapes() {
    // the box as program_profile() writes it, from what program_needs() works out of the real interpreter; a
    // pretend engine's home with secrets planted in it; the program does what a role's program does, then tries out
    let base = std::env::temp_dir().join(format!("jaynes-program-closed-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let engine = base.join("engine");
    let (role_dir, library, project) = (engine.join("runs/r1/role"), engine.join("runs/r1/library"), engine.join("runs/r1/project"));
    let (tmp, declared) = (base.join("call-tmp"), base.join("declared"));
    for d in [&role_dir, &library, &tmp, &declared, &project, &engine.join("runs/r2")] {
        std::fs::create_dir_all(d).expect("dir");
    }
    for (f, text) in [(engine.join("page.key"), "secret"), (engine.join("runs/r2/output.json"), "secret"), (role_dir.join("role.toml"), "role"),
        (library.join("jaynes_role.py"), "lib"), (declared.join("data.txt"), "data"), (project.join("greeting.txt"), "hello")] {
        std::fs::write(f, text).expect("write");
    }
    let broker = tmp.join("broker.sock");
    let _broker = std::os::unix::net::UnixListener::bind(&broker).expect("broker");
    let other_sock = base.join("other.sock");
    let _other = std::os::unix::net::UnixListener::bind(&other_sock).expect("other");
    let service = std::net::TcpListener::bind("127.0.0.1:0").expect("service");
    let undeclared = std::net::TcpListener::bind("127.0.0.1:0").expect("undeclared");
    let (port, other_port) = (service.local_addr().expect("a").port(), undeclared.local_addr().expect("b").port());
    // the real interpreter, as a role declares it: /usr/bin/python3 is a shim the box refuses
    let real = Command::new(python()).args(["-c", "import sys; print(sys.executable)"]).output().expect("python");
    let interpreter = String::from_utf8_lossy(&real.stdout).trim().to_string();
    let library_run = |what: &str, given: Value| -> (bool, String) {
        let mut make = Command::new(&interpreter)
            .args([root().join("python/jaynes_box.py").to_str().expect("path"), what])
            .env("JAYNES_ENGINE_HOME", &engine)
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().expect("profile");
        {
            use std::io::Write;
            make.stdin.take().expect("stdin").write_all(given.to_string().as_bytes()).expect("write");
        }
        let out = make.wait_with_output().expect("out");
        (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string())
    };
    // what the program needs, worked out once (at pinning), then each call's box from it; it declares pbpaste and
    // security as commands, to try the pasteboard and the keychain the system's own rules might leave open
    let commands = ["/usr/bin/pbpaste", "/usr/bin/security"];
    let (ok, needs) = library_run("program-needs", serde_json::json!({"interpreter": interpreter, "commands": commands}));
    assert!(ok, "{needs}");
    let needs: Value = serde_json::from_str(&needs).expect("needs");
    let given = serde_json::json!({"needs": needs, "role_dir": role_dir, "library": library, "tmp": tmp, "broker": broker,
        "reads": [declared], "local_ports": [port, 8750], "project": project, "project_write": false});
    let (ok, profile) = library_run("program-profile", given.clone());
    assert!(ok && profile.contains("(deny default)"), "{profile}");
    let (ok, said) = library_run("program-profile", serde_json::json!({"role_dir": role_dir, "library": library, "tmp": tmp}));
    assert!(!ok && said.contains("program-needs"), "a call's box never works the needs out itself: {said}");
    // what it never opens is refused before the program starts, never opened with less
    let (ok, said) = library_run("program-needs", serde_json::json!({"interpreter": "/usr/bin/python3"}));
    assert!(!ok && said.contains("xcrun"), "{said}");
    for bad in [serde_json::json!({"reads": [std::env::var("HOME").expect("home")]}), serde_json::json!({"reads": [engine.clone()]})] {
        let mut g = given.clone();
        for (k, v) in bad.as_object().expect("obj") {
            g[k] = v.clone();
        }
        let (ok, said) = library_run("program-profile", g);
        assert!(!ok && !said.contains("(version 1)"), "{bad}: {said}");
    }
    // a value on the find pasteboard, put back as it was at the end; never printed
    let pb = |args: &[&str], input: Option<&[u8]>| -> Option<Vec<u8>> {
        let mut child = Command::new(args[0]).args(&args[1..]).stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok()?;
        if let Some(i) = input {
            use std::io::Write;
            child.stdin.take()?.write_all(i).ok()?;
        }
        drop(child.stdin.take());
        let out = child.wait_with_output().ok()?;
        out.status.success().then_some(out.stdout)
    };
    let saved = pb(&["/usr/bin/pbpaste", "-pboard", "find"], None);
    let token = format!("jaynes-program-{}", std::process::id());
    let _set = pb(&["/usr/bin/pbcopy", "-pboard", "find"], Some(token.as_bytes()));
    std::fs::write(role_dir.join("prog.py"), r#"
import hashlib, json, os, socket, sqlite3, ssl, subprocess, sys, tempfile, threading
a = json.loads(sys.argv[1])
got = {}
def try_(name, f):
    try: got[name] = f() or "ok"
    except Exception as e: got[name] = type(e).__name__
# what a role's program does
try_("work: sqlite, ssl, hashlib, a thread", lambda: (sqlite3.connect(":memory:").execute("select 1").fetchone(), ssl.create_default_context(),
     hashlib.sha256(b"x").hexdigest(), threading.Thread(target=lambda: None).start()) and "ok")
try_("work: a temporary file", lambda: tempfile.NamedTemporaryFile(dir=os.environ["TMPDIR"]).close())
try_("work: its own folder", lambda: open(os.path.join(a["role_dir"], "role.toml")).read() and "ok")
try_("work: Jaynes's library", lambda: open(os.path.join(a["library"], "jaynes_role.py")).read() and "ok")
try_("work: the declared folder", lambda: open(os.path.join(a["declared"], "data.txt")).read() and "ok")
try_("work: the project", lambda: open(os.path.join(a["project"], "greeting.txt")).read() and "ok")
try_("work: a child of itself", lambda: "ok" if subprocess.run([sys.executable, "-c", "print(1)"], capture_output=True).returncode == 0 else "failed")
s = socket.socket(socket.AF_UNIX)
try_("work: the broker", lambda: s.connect(a["broker"]))
c = socket.socket(); c.settimeout(3)
try_("work: the declared port", lambda: c.connect(("127.0.0.1", a["port"])))
# what it must not do
home = a["real_home"]
try_("escape: list the person's home", lambda: os.listdir(home) and "listed")
try_("escape: know whether ~/.ssh exists", lambda: "knows" if os.path.exists(os.path.join(home, ".ssh")) else "does not know")
try_("escape: read the page's key", lambda: open(os.path.join(a["engine"], "page.key")).read() and "read")
try_("escape: read another run", lambda: open(os.path.join(a["engine"], "runs", "r2", "output.json")).read() and "read")
try_("escape: write its own folder", lambda: open(os.path.join(a["role_dir"], "x"), "w").write("x") and "wrote")
try_("escape: write the project", lambda: open(os.path.join(a["project"], "greeting.txt"), "w").write("x") and "wrote")
try_("escape: run a program it did not declare", lambda: subprocess.run(["/bin/ls", "/"], capture_output=True) and "ran")
o = socket.socket(socket.AF_UNIX)
try_("escape: another socket", lambda: o.connect(a["other_sock"]) or "reached")
for name, p in (("escape: an undeclared port", a["other_port"]), ("escape: the page's port, declared", 8750)):
    t = socket.socket(); t.settimeout(3)
    try_(name, lambda: t.connect(("127.0.0.1", p)) or "reached")
u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
try_("escape: UDP", lambda: u.sendto(b"", ("127.0.0.1", 9)) and "sent")
b = socket.socket()
try_("escape: bind", lambda: b.bind(("127.0.0.1", 0)) or "bound")
try_("escape: a name on the network", lambda: socket.getaddrinfo("example.com", 80) and "resolved")
def pasteboard():
    r = subprocess.run(["/usr/bin/pbpaste", "-pboard", "find"], capture_output=True)
    if r.stdout.decode(errors="replace") == a["token"]:
        return "read"
    raise PermissionError("not read")
try_("escape: read the pasteboard", pasteboard)
def keychain():
    if subprocess.run(["/usr/bin/security", "list-keychains"], capture_output=True).returncode == 0:
        return "listed"
    raise PermissionError("not listed")
try_("escape: the keychain", keychain)
print(json.dumps(got))
"#).expect("write");
    let home = std::env::var("HOME").expect("home");
    let args = serde_json::json!({"role_dir": role_dir, "library": library, "declared": declared, "project": project, "engine": engine,
        "broker": broker, "other_sock": other_sock, "port": port, "other_port": other_port, "real_home": home, "token": token});
    let out = Command::new("/usr/bin/sandbox-exec")
        .args(["-p", &profile, &interpreter, role_dir.join("prog.py").to_str().expect("path"), &args.to_string()])
        .current_dir(&role_dir).env_clear().env("PATH", "/usr/bin:/bin").env("TMPDIR", &tmp).env("HOME", &tmp)
        .env("PYTHONDONTWRITEBYTECODE", "1").env("LANG", "en_US.UTF-8")
        .output().expect("sandbox-exec");
    if let Some(v) = &saved {
        let _back = pb(&["/usr/bin/pbcopy", "-pboard", "find"], Some(v));
    }
    let got: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)));
    for (k, v) in got.as_object().expect("got") {
        let v = v.as_str().unwrap_or("");
        if k.starts_with("work:") {
            assert_eq!(v, "ok", "{k}: {got}");
        } else {
            assert!(v == "PermissionError" || v == "does not know" || v == "gaierror", "{k} got through: {v}: {got}");
        }
    }
    assert_eq!(got.as_object().expect("got").len(), 24, "{got}");
    // opening an app or sending Apple events is never tried for real, which would open it on the person's screen if
    // the box failed: the profile as written is read for both, as the program's proof does
    for rule in ["(deny lsopen)", "(deny appleevent-send)"] {
        assert!(profile.lines().any(|l| l.trim() == rule), "the profile lacks {rule}:\n{profile}");
    }
    drop((service, undeclared));
    let _gone = std::fs::remove_dir_all(&base);
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the box of a role's program is macOS's sandbox-exec here")]
fn a_program_s_box_holds_with_each_python_of_this_mac() {
    // a Python of Homebrew's loads its libraries by a path through links (/opt/homebrew/opt/sqlite -> ../Cellar/...):
    // the box opens where the links lead, and the links themselves too, or its sqlite3 does not load
    let mut pythons = vec![python()];
    for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut found: Vec<String> = entries.flatten().map(|e| e.path()).filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            n.strip_prefix("python3.").is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        }).map(|p| p.display().to_string()).collect();
        found.sort();
        pythons.extend(found);
    }
    for py in pythons {
        let engine = std::env::temp_dir().join(format!("jaynes-program-pythons-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&engine);
        std::fs::create_dir_all(&engine).expect("dir");
        let out = Command::new(&py).arg("-I").arg(root().join("python/jaynes_box.py")).arg("program-proof")
            .env("JAYNES_ENGINE_HOME", &engine).output().expect("run");
        let _gone = std::fs::remove_dir_all(&engine);
        assert!(out.status.success(), "{py}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the box of a role's program is macOS's sandbox-exec here")]
fn a_program_s_box_is_proved_on_this_macos_before_use_and_stays_shut_when_the_proof_fails() {
    let base = std::env::temp_dir().join(format!("jaynes-program-proof-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let engine = base.join("engine");
    let (role_dir, tmp) = (base.join("role"), base.join("tmp"));
    for d in [&engine, &role_dir, &tmp] {
        std::fs::create_dir_all(d).expect("dir");
    }
    let real = Command::new(python()).args(["-c", "import sys; print(sys.executable)"]).output().expect("python");
    let interpreter = String::from_utf8_lossy(&real.stdout).trim().to_string();
    let lib = |what: &str, input: &str| -> (i32, String) {
        let mut c = Command::new(&interpreter)
            .args([root().join("python/jaynes_box.py").to_str().expect("path"), what]).env("JAYNES_ENGINE_HOME", &engine)
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().expect("run");
        {
            use std::io::Write;
            c.stdin.take().expect("stdin").write_all(input.as_bytes()).expect("write");
        }
        let out = c.wait_with_output().expect("out");
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).to_string())
    };
    let sw = |flag: &str| String::from_utf8_lossy(&Command::new("sw_vers").arg(flag).output().expect("sw_vers").stdout).trim().to_string();
    let this_macos = format!("{} {}", sw("-productVersion"), sw("-buildVersion"));
    let proof_file = engine.join("program-box.json");
    let proof = || -> Value { serde_json::from_str(&std::fs::read_to_string(&proof_file).expect("proof")).expect("json") };
    // at install: the proof is made and kept, with the macOS it held on
    let (code, said) = lib("program-proof", "");
    assert_eq!(code, 0, "{said}");
    assert!(said.contains(&this_macos), "{said}");
    assert_eq!((proof()["holds"].as_bool(), proof()["macos"].as_str()), (Some(true), Some(this_macos.as_str())));
    let (code, needs) = lib("program-needs", &serde_json::json!({"interpreter": interpreter}).to_string());
    assert_eq!(code, 0, "{needs}");
    let call = serde_json::json!({"needs": serde_json::from_str::<Value>(&needs).expect("needs"), "role_dir": role_dir, "library": role_dir, "tmp": tmp}).to_string();
    // another macOS: proved again before the first use
    let mut p = proof();
    p["macos"] = serde_json::json!("1.0 another");
    std::fs::write(&proof_file, p.to_string()).expect("write");
    let (code, profile) = lib("program-profile", &call);
    assert!(code == 0 && profile.contains("(deny default)"), "{profile}");
    assert_eq!(proof()["macos"].as_str(), Some(this_macos.as_str()), "the proof was not made again on this macOS");
    // another box (a jaynes update that changed the profile), or another interpreter: proved again before the first use
    use sha2::Digest as _;
    let this_box = format!("{:x}", sha2::Sha256::digest(std::fs::read(root().join("python/jaynes_box.py")).expect("box")));
    let this_interpreter = std::fs::canonicalize(&interpreter).expect("interpreter").display().to_string();
    assert_eq!((proof()["box"].as_str(), proof()["interpreter"].as_str()), (Some(this_box.as_str()), Some(this_interpreter.as_str())));
    for (key, other) in [("box", "an older profile"), ("interpreter", "/another/python3")] {
        let mut p = proof();
        p[key] = serde_json::json!(other);
        p["at"] = serde_json::json!(0);
        std::fs::write(&proof_file, p.to_string()).expect("write");
        let (code, profile) = lib("program-profile", &call);
        assert!(code == 0 && profile.contains("(deny default)"), "{profile}");
        assert!(proof()["at"].as_i64() != Some(0), "a proof kept for {other} was used: {}", proof());
    }
    // a proof that did not hold here: the box stays shut and says what got through
    let mut p = proof();
    p["holds"] = serde_json::json!(false);
    p["wrong"] = serde_json::json!(["escape: list the person's home: listed"]);
    std::fs::write(&proof_file, p.to_string()).expect("write");
    let (code, said) = lib("program-profile", &call);
    assert!(code == 3 && said.contains("did not hold") && said.contains("list the person's home"), "{said}");
    assert!(!said.contains("(version 1)"), "{said}");
    let _gone = std::fs::remove_dir_all(&base);
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the box of a role's program is macOS's sandbox-exec here")]
fn what_a_program_s_box_works_out_from_its_commands_never_opens_the_person_s_home() {
    // a command's folder, the interpreter's and the libraries' are opened as what is declared is: never the person's
    // home or what holds it, a place secrets live, or the engine's folder (the engineer lead read a command at the root
    // of the home open the whole home, but for the secrets)
    let base = std::env::temp_dir().join(format!("jaynes-program-derived-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let (home, engine) = (base.join("home"), base.join("engine"));
    for d in [home.join("x/bin"), home.join(".ssh/bin"), engine.join("bin"), base.join("role"), base.join("tmp")] {
        std::fs::create_dir_all(d).expect("dir");
    }
    let tool = |at: &Path| {
        std::fs::write(at, "#!/bin/sh\necho hi\n").expect("tool");
        std::fs::set_permissions(at, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("chmod");
        at.display().to_string()
    };
    let real = Command::new(python()).args(["-c", "import sys; print(sys.executable)"]).output().expect("python");
    let interpreter = String::from_utf8_lossy(&real.stdout).trim().to_string();
    let lib = |what: &str, input: &Value| -> (i32, String) {
        let mut c = Command::new(&interpreter)
            .args([root().join("python/jaynes_box.py").to_str().expect("path"), what]).env("JAYNES_ENGINE_HOME", &engine)
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().expect("run");
        {
            use std::io::Write;
            c.stdin.take().expect("stdin").write_all(input.to_string().as_bytes()).expect("write");
        }
        let out = c.wait_with_output().expect("out");
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).to_string())
    };
    let needs = |commands: &[String]| lib("program-needs", &serde_json::json!({"interpreter": interpreter, "commands": commands, "home": home}));
    // a command at the root of the home, in a place secrets live, or in the engine's folder: refused, naming it
    for at in [home.join("tool"), home.join(".ssh/bin/tool"), engine.join("bin/tool")] {
        let t = tool(&at);
        let (code, said) = needs(std::slice::from_ref(&t));
        assert!(code == 3 && said.contains(&format!("the command {t}")) && said.contains("never reaches"), "{t}: {code} {said}");
    }
    // a command in ~/x/bin opens ~/x/bin and nothing else of the home
    let t = tool(&home.join("x/bin/tool"));
    let (code, said) = needs(&[t]);
    assert_eq!(code, 0, "{said}");
    let got: Value = serde_json::from_str(&said).expect("needs");
    let real_home = std::fs::canonicalize(&home).expect("home");
    let of_home: Vec<&str> = got["read"].as_array().expect("read").iter().filter_map(Value::as_str).filter(|p| Path::new(p).starts_with(&real_home)).collect();
    assert_eq!(of_home, vec![real_home.join("x/bin").display().to_string()], "{got}");
    // needs kept from pinning that name the home are refused when the box is made, as a declaration of it is
    let mut kept = got.clone();
    kept["read"].as_array_mut().expect("read").push(Value::from(home.display().to_string()));
    let call = serde_json::json!({"needs": kept, "role_dir": base.join("role"), "library": base.join("role"), "tmp": base.join("tmp"), "home": home});
    let (code, said) = lib("program-profile", &call);
    assert!(code == 3 && said.contains("never reaches") && !said.contains("(version 1)"), "{code} {said}");
    let _gone = std::fs::remove_dir_all(&base);
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the box of a role's program is macOS's sandbox-exec here")]
fn the_proof_of_a_program_s_box_fails_when_any_one_rule_of_the_profile_is_taken_away() {
    // the proof holds on the profile as written, and fails on each profile with one rule taken away or one door
    // opened: nothing it guards can be lost without the proof saying so
    let script = format!(r#"
import json, sys
sys.path.insert(0, {lib:?})
import jaynes_box as b
from pathlib import Path
def without(rule):
    return lambda box: "\n".join(l for l in b.program_profile(box).splitlines() if l != rule) + "\n"
def without_secrets(box):
    lines = b.program_profile(box).splitlines()
    last = max(i for i, l in enumerate(lines) if l.startswith("(deny file-read*"))
    return "\n".join(lines[:last] + lines[last + 1:]) + "\n"
def adding(rule):
    return lambda box: b.program_profile(box) + rule(box) + "\n"
cases = {{
    "as written": b.program_profile,
    "without (deny default)": lambda box: b.program_profile(box).replace("(deny default)", "(allow default)"),
    "without (deny lsopen)": without("(deny lsopen)"),
    "without (deny appleevent-send)": without("(deny appleevent-send)"),
    "without the last denial of secrets": without_secrets,
    "with ~/.aws opened": adding(lambda box: f"(allow file-read* (subpath \"{{box.home / '.aws'}}\"))"),
    "with every local port opened": adding(lambda box: '(allow network-outbound (remote tcp "localhost:*"))'),
    "with any program allowed to run": adding(lambda box: "(allow process-exec)"),
    "without stopping its own children": without("(allow signal (target same-sandbox))"),
    "with any process signalled": adding(lambda box: "(allow signal)"),
}}
print(json.dumps({{name: b.program_selftest(Path(sys.executable), make=make) for name, make in cases.items()}}))
"#, lib = root().join("python"));
    let out = Command::new(python()).args(["-c", &script]).output().expect("python");
    let got: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&out.stderr)));
    assert_eq!(got["as written"], serde_json::json!([]), "{got}");
    for (case, wrong) in got.as_object().expect("cases") {
        if case != "as written" {
            assert!(wrong.as_array().is_some_and(|w| !w.is_empty()), "the proof did not notice the profile {case}: {got}");
        }
    }
}

/// A role whose one step changes the greeting and whose check serves on a port of its own choosing (port 0, as
/// Go's httptest does), after running `tool` from the PATH when one is named: with the project's ports closed, the
/// box refuses the port and the check cannot judge.
fn port_zero_role(s: &Scratch, name: &str, tool: Option<&str>) -> PathBuf {
    let role = s.home.parent().expect("base").join(name);
    std::fs::create_dir_all(role.join("checks")).expect("dir");
    std::fs::write(role.join("role.toml"), format!(r#"
role = "{name}"
version = "1.0.0"

[[step]]
name = "change"
reach = {{ project = "write", write = ["**"], commands = true }}
instructions = "Make the change: {{{{request}}}}"
checks = [{{ use = "field-required", field = "summary" }}, {{ script = "checks/serves.py", about = "the project serves on a port of its own" }}]

[step.output]
summary = {{ type = "string", about = "what changed" }}
files = {{ type = "array", items = "string" }}
"#)).expect("write");
    let needs = tool.map(|t| format!("if subprocess.run([{t:?}]).returncode != 0:\n    print(\"{t}: command not found\")\n    sys.exit(1)\n")).unwrap_or_default();
    std::fs::write(role.join("checks/serves.py"), format!(r#"#!/usr/bin/env python3
import socket, subprocess, sys
{needs}# as Go's httptest.NewServer, [::1]:0 first, then 127.0.0.1:0
for family, host in ((socket.AF_INET6, "::1"), (socket.AF_INET, "127.0.0.1")):
    mine = socket.socket(family)
    mine.bind((host, 0))
    mine.listen()
    socket.create_connection((host, mine.getsockname()[1]), timeout=3).close()
if b"hi" not in open("greeting.txt", "rb").read():
    print("the greeting is not hi")
    sys.exit(1)
"#)).expect("write");
    let script = role.join("checks/serves.py");
    let mut mode = std::fs::metadata(&script).expect("meta").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(&script, mode).expect("chmod");
    role
}

/// `jaynes ARGS` at a terminal, as the person types it, with `path` as the PATH when one is given.
fn at_terminal(s: &Scratch, args: &[&str], path: Option<&str>) -> String {
    typed_at_terminal(s, args, path, "")
}

/// `jaynes ARGS` at a terminal, the person typing `typed` (an answer to its question).
fn typed_at_terminal(s: &Scratch, args: &[&str], path: Option<&str>, typed: &str) -> String {
    let mut c = tty();
    c.arg(BIN).args(args).env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .env("TTY_TYPED", typed);
    if let Some(p) = path {
        c.env("PATH", p);
    }
    let out = c.output().expect("terminal");
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the window for test servers is macOS's box (Linux's box serves on port 0 itself)")]
fn a_suite_that_opens_thousands_of_connections_still_reaches_the_test_server_it_starts_after_them() {
    let _one = holding_alone();
    let s = scratch("many-conns");
    let project = s.project.to_str().expect("path");
    let said = typed_at_terminal(&s, &["project", "test-servers", project], None, "y\n");
    assert!(said.contains("Done."), "{said}");
    let role = s.home.parent().expect("base").join("many");
    std::fs::create_dir_all(role.join("checks")).expect("dir");
    std::fs::write(role.join("role.toml"), r#"
role = "many"
version = "1.0.0"

[[step]]
name = "look"
reach = { project = "read" }
instructions = "Look at {{request}}."
checks = [{ script = "checks/many.py", about = "a suite's many requests, then a server of its own" }]
"#).expect("write");
    // as a suite of many HTTP tests: each request a connection on a port the system picks, more than the box
    // used to allow, and a test server started after them all; and as the machine's other programs, which take the
    // same ports meanwhile
    std::fs::write(role.join("checks/many.py"), r#"#!/usr/bin/env python3
import socket, struct, sys
app = socket.socket()
app.bind(("127.0.0.1", 0))
app.listen(128)
for n in range(5000):
    try:
        c = socket.create_connection(("127.0.0.1", app.getsockname()[1]), timeout=3)
    except OSError as e:
        print(f"request {n} refused: {e}")
        sys.exit(1)
    # closed at once with a reset, so its port waits for nobody: 5,000 ports in TIME_WAIT for 30 s would leave
    # the Mac's other programs short of ports to connect from
    c.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
    c.close()
    app.accept()[0].close()
for family, host in ((socket.AF_INET6, "::1"), (socket.AF_INET, "127.0.0.1")):
    mine = socket.socket(family)
    mine.bind((host, 0))
    mine.listen()
    try:
        socket.create_connection((host, mine.getsockname()[1]), timeout=3).close()
    except OSError as e:
        print(f"the test server on {host}:{mine.getsockname()[1]} out of reach: {e}")
        sys.exit(1)
"#).expect("write");
    let script = role.join("checks/many.py");
    let mut mode = std::fs::metadata(&script).expect("meta").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(&script, mode).expect("chmod");
    let call = serde_json::json!({"jaynes": 1, "id": "x", "call": "check", "step": "look", "check": "many", "request": "r",
        "output": {}, "given": [], "project": s.project, "source": s.project, "test_servers": true});
    // the box lets the tests reach the 4,000 ports macOS gives next, as a list: the server's comes after them, and the
    // check says so, never that the tests went for a port of the person's
    let verdict = serve_check(&s, &role, call, &[]);
    assert_eq!(verdict["verdict"], "blocked", "{verdict}");
    let reason = verdict["reason"].as_str().unwrap_or("");
    let tried = reason.split("out of reach").next().and_then(|t| t.rsplit(':').next()).unwrap_or("").trim().to_string();
    assert!(reason.contains(&format!(" It tried port {tried}, which macOS gave after the 4,000 the box let the tests reach (")), "{reason}");
    assert!(!reason.contains("JAYNES_PORTS"), "{reason}");
    assert!(reason.contains("`jaynes project open-ports "), "{reason}");

    // in a run: the block, then the person's Continue checks the same change again, and the same block comes back
    // saying so, the agent working no more
    std::fs::write(role.join("role.toml"), r#"
role = "many"
version = "1.0.0"

[[step]]
name = "change"
reach = { project = "write", write = ["**"], commands = true }
instructions = "Make the change: {{request}}"
checks = [{ script = "checks/many.py", about = "a suite's many requests, then a server of its own" }]

[step.output]
summary = { type = "string", about = "what changed" }
files = { type = "array", items = "string" }
"#).expect("write");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let out = jaynes()
        .args(["start", "--run", "many", "--role", role.to_str().expect("path"), "--project", project,
            "--request", "say hi instead of hello", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .output().expect("start");
    let blocks = |e: &[Value]| e.iter().filter(|x| x["type"] == "Blocked").map(|x| x["reason"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>();
    let first = blocks(&events(&s, "many"));
    assert_eq!(first.len(), 1, "{}\n{first:?}", String::from_utf8_lossy(&out.stdout));
    assert!(first[0].contains(", which macOS gave after the 4,000 the box let the tests reach (") && !first[0].contains("It happened again."), "{}", first[0]);
    let said = at_terminal(&s, &["continue", "many"], None);
    let e = events(&s, "many");
    let both = blocks(&e);
    assert_eq!(both.len(), 2, "{said}\n{both:?}");
    assert!(both[1].contains(", which macOS gave after the 4,000 the box let the tests reach (") && both[1].contains("It happened again."), "{}", both[1]);
    // the card, from what the record keeps: Continue first the first time, gone the second, the ports first then
    let card = |reason: &str| jaynes_shell::listing::blocked_said(Some("change"), jaynes_core::event::Fault::Jaynes, reason);
    let (once, again) = (card(&first[0]), card(&both[1]));
    assert!(once.why.contains("so macOS gave it a port past the ones the box lets the tests reach.") && !once.why.contains("It happened again"), "{once:?}");
    assert!(once.ways[0].says.starts_with("Continue to check again") && once.ways[1].says.starts_with("If it happens again, open this project's ports"), "{once:?}");
    assert!(again.why.ends_with("It happened again, so the tests themselves likely make that many connections."), "{again:?}");
    assert_eq!(again.ways[0].says, "Open this project's ports, then Continue.", "{again:?}");
    assert!(again.ways[0].command.as_deref().is_some_and(|c| c.starts_with("jaynes project open-ports ")), "{again:?}");
    assert_eq!(e.iter().filter(|x| x["type"] == "AttemptStarted").count(), 1, "{:?}", types(&e));
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the window for test servers is macOS's box (Linux's box serves on port 0 itself)")]
fn the_person_s_test_servers_let_a_check_serve_on_a_port_the_system_picks_and_reach_nothing_else() {
    let _one = holding_alone();
    let s = scratch("test-servers");
    let project = s.project.to_str().expect("path");
    // a program without a terminal is an agent: it may not allow them
    let (ok, out) = engine(&s, &["project", "test-servers", project]);
    assert!(!ok && out.contains("only the person"), "{out}");
    // the person is told what stays open before it holds, and a no keeps it closed
    let said = typed_at_terminal(&s, &["project", "test-servers", project], None, "n\n");
    for words in ["may start their own test servers, as when you run them yourself.", "They still cannot reach your services or the Jaynes page.",
        "Still open while the tests run:", "A test server listens on every network of this Mac, so another machine on your network could reach it.",
        "A service that starts on this Mac while the tests run may get a port they can reach.", "? [y/N]"] {
        assert!(said.contains(words), "{words}: {said}");
    }
    assert!(!said.contains("Done."), "{said}");
    let (_, ports) = engine(&s, &["project", "ports", project]);
    assert!(ports.contains("reach only the ports a run reserves"), "{ports}");
    let said = typed_at_terminal(&s, &["project", "test-servers", project], None, "y\n");
    assert!(said.contains("Done. Go back to the run and press Continue."), "{said}");
    let (_, ports) = engine(&s, &["project", "ports", project]);
    assert!(ports.contains("this project's tests may start their own test servers: allowed by you on"), "{ports}");

    // a service of the person's, on a port of its own outside the system's (as a database's), and the page's port
    let service = (20_000..30_000).find_map(|p| std::net::TcpListener::bind(("127.0.0.1", p)).ok()).expect("a free port");
    let service_port = service.local_addr().expect("addr").port();
    let role = s.home.parent().expect("base").join("servers");
    std::fs::create_dir_all(role.join("checks")).expect("dir");
    std::fs::write(role.join("role.toml"), r#"
role = "servers"
version = "1.0.0"

[[step]]
name = "look"
reach = { project = "read" }
instructions = "Look at {{request}}."
checks = [{ script = "checks/servers.py", about = "its own test servers, and nothing else" }]
"#).expect("write");
    std::fs::write(role.join("checks/servers.py"), r#"#!/usr/bin/env python3
import json, socket, sys
said = json.load(sys.stdin)["output"]
# as Go's httptest.NewServer: [::1]:0 first, then 127.0.0.1:0, each served and reached
for family, host in ((socket.AF_INET6, "::1"), (socket.AF_INET, "127.0.0.1")):
    mine = socket.socket(family)
    mine.bind((host, 0))
    mine.listen()
    socket.create_connection((host, mine.getsockname()[1]), timeout=3).close()
# the person's service stays out of reach
try:
    socket.create_connection(("127.0.0.1", said["service"]), timeout=3).close()
    print("reached the person's service")
    sys.exit(1)
except OSError:
    pass
# and the page's port is never taken, the page up or not
for family, host in ((socket.AF_INET, "127.0.0.1"), (socket.AF_INET6, "::1")):
    page = socket.socket(family)
    page.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        page.bind((host, 8750))
        page.listen()
        print(f"took the page's port on {host}")
        sys.exit(1)
    except OSError:
        pass
"#).expect("write");
    let script = role.join("checks/servers.py");
    let mut mode = std::fs::metadata(&script).expect("meta").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(&script, mode).expect("chmod");
    let call = |servers: Option<bool>| {
        let mut c = serde_json::json!({"jaynes": 1, "id": "x", "call": "check", "step": "look", "check": "servers", "request": "r",
            "output": {"home": s.home, "service": service_port}, "given": [], "project": s.project, "source": s.project});
        if let Some(on) = servers {
            c["test_servers"] = serde_json::json!(on);
        }
        c
    };
    // the box opens the window on the engine's word in the call, and on nothing else
    let open = serve_check(&s, &role, call(Some(true)), &[]);
    assert_eq!(open["verdict"], "pass", "{open}");
    for said in [call(None), call(Some(false))] {
        let shut = serve_check(&s, &role, said, &[]);
        assert_eq!(shut["verdict"], "blocked", "{shut}");
    }
    drop(service);
    // the person closes both choices with one command
    let closed = at_terminal(&s, &["project", "close-ports", project], None);
    assert!(closed.contains("closed"), "{closed}");
    let (_, ports) = engine(&s, &["project", "ports", project]);
    assert!(ports.contains("reach only the ports a run reserves"), "{ports}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the window for test servers is macOS's box (Linux's box serves on port 0 itself)")]
fn continue_after_the_person_allows_test_servers_checks_the_same_change_again_and_delivers() {
    let _one = holding_alone();
    let s = scratch("servers-continue");
    let role = port_zero_role(&s, "servers-role", None);
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let out = jaynes()
        .args(["start", "--run", "servers", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
            "--request", "say hi instead of hello", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .output().expect("start");
    let e = events(&s, "servers");
    assert!(e.iter().any(|x| x["type"] == "Blocked" && x["fault"] == "jaynes"), "{}\n{:?}", String::from_utf8_lossy(&out.stdout), types(&e));
    // what the card reads to offer the test servers, the way out that serves a port the system picks
    let blocked = e.iter().rev().find(|x| x["type"] == "Blocked").expect("blocked");
    assert!(blocked["reason"].as_str().unwrap_or("").contains("It tried local port 0, a port of its own choosing."), "{blocked}");
    let said = typed_at_terminal(&s, &["project", "test-servers", s.project.to_str().expect("path")], None, "y\n");
    assert!(said.contains("Done."), "{said}");
    let said = at_terminal(&s, &["continue", "servers"], None);
    let e = events(&s, "servers");
    assert!(types(&e).iter().any(|t| t == "Delivered"), "{:?}\n{said}", types(&e));
    // the agent did not work again: one Execute of change
    assert_eq!(e.iter().filter(|x| x["type"] == "AttemptStarted").count(), 1, "{:?}", types(&e));
}

/// A test whose checks run with the project's test servers holds the ports of the usual databases and caches while
/// they run, as Jaynes does: one such test at a time, so no other one's hold reads as its own.
fn holding_alone() -> std::sync::MutexGuard<'static, ()> {
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A port free on 127.0.0.1, ::1, 0.0.0.0 and :: now, the first of `ports`.
fn free_everywhere(ports: &[u16]) -> u16 {
    use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};
    *ports.iter().find(|&&p| {
        [Ipv4Addr::LOCALHOST.into(), Ipv6Addr::LOCALHOST.into(), Ipv4Addr::UNSPECIFIED.into(), Ipv6Addr::UNSPECIFIED.into()]
            .iter().all(|a: &std::net::IpAddr| TcpListener::bind((*a, p)).is_ok())
    }).unwrap_or_else(|| {
        // a Jaynes box elsewhere on this machine (another run's agent or check with its test servers) holds them all
        let who: String = ports.iter().map(|p| Command::new("lsof").args(["-nP", &format!("-iTCP:{p}"), "-sTCP:LISTEN"]).output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()).collect();
        panic!("none of the ports {ports:?} is free on this machine now; who holds them:\n{who}")
    })
}

/// Databases and caches a test machine may run, whose ports Jaynes holds: the first free one is the test's.
const DATA_PORTS: [u16; 7] = [5432, 3306, 6379, 27017, 11211, 9200, 5672];

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the window for test servers is macOS's box (Linux's box has a network of its own)")]
fn with_the_person_s_test_servers_a_test_server_cannot_take_a_free_database_port_and_a_web_port_stays_its_own() {
    let _one = holding_alone();
    let s = scratch("held");
    let held = free_everywhere(&DATA_PORTS);
    let web = free_everywhere(&[3000, 5173, 8000, 8080, 4200]);
    let role = s.home.parent().expect("base").join("held");
    std::fs::create_dir_all(role.join("checks")).expect("dir");
    std::fs::write(role.join("role.toml"), format!(r#"
role = "held"
version = "1.0.0"

[[step]]
name = "look"
reach = {{ project = "read" }}
instructions = "Look at {{{{request}}}}."
checks = [{{ script = "checks/held.py", about = "a database's port held, a web port its own" }}]

[[step]]
name = "declared"
reach = {{ project = "read" }}
instructions = "Look at {{{{request}}}}."
checks = [{{ script = "checks/held.py", about = "a port the check names is never held", local_ports = [{held}] }}]
"#)).expect("write");
    std::fs::write(role.join("checks/held.py"), r#"#!/usr/bin/env python3
import errno, json, socket, sys
said = json.load(sys.stdin)["output"]
held, web, expect = said["held"], said["web"], said["expect"]
# as Go's net.Listen: SO_REUSEADDR, on each address a test server may take
for family, host in ((socket.AF_INET, "127.0.0.1"), (socket.AF_INET6, "::1"), (socket.AF_INET, "0.0.0.0"), (socket.AF_INET6, "::")):
    mine = socket.socket(family)
    mine.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        mine.bind((host, held))
        took = True
    except OSError as e:
        took = False
        if e.errno != errno.EADDRINUSE:
            print(f"{host}:{held} refused otherwise: {e}")
            sys.exit(1)
    finally:
        mine.close()
    if took != (expect == "free"):
        print(f"{host}:{held} {'taken' if took else 'held'}, expected {expect}")
        sys.exit(1)
# a web port a test brings its own app up on stays its own
app = socket.socket()
app.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
app.bind(("127.0.0.1", web))
app.listen()
"#).expect("write");
    let script = role.join("checks/held.py");
    let mut mode = std::fs::metadata(&script).expect("meta").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(&script, mode).expect("chmod");
    let call = |step: &str, expect: &str, more: Value| {
        let mut c = serde_json::json!({"jaynes": 1, "id": "x", "call": "check", "step": step, "check": "held", "request": "r",
            "output": {"held": held, "web": web, "expect": expect}, "given": [], "project": s.project, "source": s.project, "test_servers": true});
        for (k, v) in more.as_object().into_iter().flatten() {
            c[k] = v.clone();
        }
        c
    };
    // with the person's yes to test servers: the database's port is held, the web port is the test's
    assert_eq!(serve_check(&s, &role, call("look", "held", serde_json::json!({})), &[])["verdict"], "pass", "held");
    // never a port the step names, nor one the person let the project's tests use, nor any with its open ports
    assert_eq!(serve_check(&s, &role, call("declared", "free", serde_json::json!({})), &[])["verdict"], "pass", "declared");
    assert_eq!(serve_check(&s, &role, call("look", "free", serde_json::json!({"freed_ports": [held]})), &[])["verdict"], "pass", "freed");
    assert_eq!(serve_check(&s, &role, call("look", "free", serde_json::json!({"any_port": true})), &[])["verdict"], "pass", "open ports");
    // and the hold ends with the check
    assert_eq!(free_everywhere(&[held]), held);
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the window for test servers is macOS's box (Linux's box has a network of its own)")]
fn a_test_server_on_a_port_jaynes_holds_blocks_saying_so_and_the_person_lets_it_and_continues() {
    let _one = holding_alone();
    let s = scratch("held-continue");
    let project = s.project.to_str().expect("path").to_string();
    let held = free_everywhere(&DATA_PORTS);
    let role = s.home.parent().expect("base").join("held-role");
    std::fs::create_dir_all(role.join("checks")).expect("dir");
    std::fs::write(role.join("role.toml"), r#"
role = "held-role"
version = "1.0.0"

[[step]]
name = "change"
reach = { project = "write", write = ["**"], commands = true }
instructions = "Make the change: {{request}}"
checks = [{ use = "field-required", field = "summary" }, { script = "checks/serves.py", about = "the project serves on a database's port" }]

[step.output]
summary = { type = "string", about = "what changed" }
files = { type = "array", items = "string" }
"#).expect("write");
    // a suite whose test server takes a fixed port, as one standing in for the project's database
    std::fs::write(role.join("checks/serves.py"), format!(r#"#!/usr/bin/env python3
import socket, sys
mine = socket.socket()
mine.bind(("127.0.0.1", {held}))
mine.listen()
socket.create_connection(("127.0.0.1", {held}), timeout=3).close()
if b"hi" not in open("greeting.txt", "rb").read():
    print("the greeting is not hi")
    sys.exit(1)
"#)).expect("write");
    let script = role.join("checks/serves.py");
    let mut mode = std::fs::metadata(&script).expect("meta").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(&script, mode).expect("chmod");
    assert!(typed_at_terminal(&s, &["project", "test-servers", &project], None, "y\n").contains("Done."));
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let (_, out) = engine(&s, &["start", "--run", "held", "--role", role.to_str().expect("path"), "--project", &project,
        "--request", "say hi instead of hello", "--agent", &format!("command:{agent}")]);
    let e = events(&s, "held");
    let blocked = e.iter().rev().find(|x| x["type"] == "Blocked").unwrap_or_else(|| panic!("{out}\n{:?}", types(&e)));
    assert_eq!(blocked["fault"], "jaynes", "not the code's fault: {blocked}");
    let reason = blocked["reason"].as_str().unwrap_or("");
    for words in ["Jaynes held a port the command tried to serve on, so its failure says nothing about the project: ",
        &format!(" It tried held port {held}."), &format!("`jaynes project test-servers {}`", std::fs::canonicalize(&project).expect("real path").display())] {
        assert!(reason.contains(words), "{words}: {reason}");
    }
    // an agent may not let it; the person does, at a terminal, for that port alone
    let (ok, said) = engine(&s, &["project", "test-servers", &project, &held.to_string()]);
    assert!(!ok && said.contains("only the person"), "{said}");
    let said = typed_at_terminal(&s, &["project", "test-servers", &project, &held.to_string()], None, "y\n");
    assert!(said.contains(&format!("Only if nothing of yours runs on {held} while the tests run")) && said.contains("Done."), "{said}");
    let (_, ports) = engine(&s, &["project", "ports", &project]);
    assert!(ports.contains(&format!("its tests may also use port {held}")), "{ports}");
    // when each port was let, apart from when the test servers were (T10, 23/11: the one date of the test servers
    // was read as the port's, and a hold that was right looked like a fault), in the file and in what it says
    let file = std::fs::read_to_string(jaynes_shell::project_prefs::path(&s.home, std::path::Path::new(&project))).expect("the project's file");
    assert!(file.contains("[ports.freed_at]") && file.contains(&format!("{held} = ")), "{file}");
    assert!(ports.contains(&format!("port {held}: let by you on 20")) && ports.contains(" UTC"), "{ports}");
    let said = at_terminal(&s, &["continue", "held"], None);
    let e = events(&s, "held");
    let chosen = e.iter().rev().find(|x| x["type"] == "PortsChosen").unwrap_or_else(|| panic!("{:?}\n{said}", types(&e)));
    assert_eq!(chosen["freed_ports"], serde_json::json!([held]), "{chosen}");
    // it failed once under load (fcf92301) with no Delivered and no cause: say what the checks after the continue
    // said, and who holds the port now, so the next failure names it
    if !types(&e).iter().any(|t| t == "Delivered") {
        let after = e.iter().rposition(|x| x["type"] == "Continued").unwrap_or(0);
        let said_then: Vec<String> = e[after..]
            .iter()
            .filter_map(|x| match x["type"].as_str()? {
                "ChecksJudged" => Some(format!("ChecksJudged: {}", x["checks"].as_array()?.iter()
                    .filter(|c| c["passed"] != true).map(|c| c.to_string()).collect::<Vec<_>>().join("; "))),
                "ChecksUnjudged" | "Blocked" | "AttemptFailed" | "FixAsked" => Some(format!("{}: {}", x["type"], x)),
                _ => None,
            })
            .collect();
        let holder = Command::new("lsof").args(["-nP", &format!("-iTCP:{held}")]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        panic!("no Delivered after the continue: {:?}\nafter the continue:\n{}\nwho has port {held} now:\n{holder}\nthe continue said:\n{said}", types(&e), said_then.join("\n"));
    }
    assert_eq!(e.iter().filter(|x| x["type"] == "AttemptStarted").count(), 1, "the agent did not work again: {:?}", types(&e));
    // closing the ports closes this too
    at_terminal(&s, &["project", "close-ports", &project], None);
    let (_, ports) = engine(&s, &["project", "ports", &project]);
    assert!(!ports.contains("also use"), "{ports}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the window for test servers is macOS's box (Linux's box has a network of its own)")]
fn tests_that_serve_on_a_port_jaynes_holds_stop_the_run_at_its_start_before_any_agent_works() {
    // the tech-lead's, 22/11 (the real-user's estoque): a suite whose fake database serves on a port Jaynes holds,
    // the port only in the project's own file, never in what the suite prints. Never 5432, a port of the person's
    let _one = holding_alone();
    let s = scratch("held-start");
    let project = s.project.to_str().expect("path").to_string();
    let held = free_everywhere(&DATA_PORTS[1..]);
    std::fs::write(s.project.join("config.py"), format!("HOST = \"127.0.0.1\"\nPORT = {held}\n")).expect("write");
    std::fs::write(s.project.join("serve.py"), "import socket\nimport config\nfake = socket.socket()\nfake.bind((config.HOST, config.PORT))\nfake.listen()\n").expect("write");
    std::fs::write(s.project.join("Makefile"), "test:\n\tpython3 serve.py\n\tgrep -q hi greeting.txt\n").expect("write");
    for args in [vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "a fake database"]] {
        assert!(Command::new("git").arg("-C").arg(&s.project).args(&args).status().expect("git").success(), "git {args:?}");
    }
    // known beforehand: letting the test servers says the project names a port Jaynes holds, and the line that lets it
    let said = typed_at_terminal(&s, &["project", "test-servers", &project], None, "y\n");
    let real = std::fs::canonicalize(&project).expect("real path").display().to_string();
    assert!(said.contains(&format!("The files of {real} mention port {held}, which Jaynes holds while the checks run. If its tests start a server on it, let them:")), "{said}");
    // the design-lead's of 22/11: Continue only after the command still to run
    assert!(said.contains(&format!("Done.\r\nThe files of {real} mention port {held}")) || said.contains(&format!("Done.\nThe files of {real} mention port {held}")), "{said}");
    assert!(said.contains(&format!("  jaynes project test-servers {real} {held}\r\nThen go back to the run and press Continue.")) || said.contains(&format!("  jaynes project test-servers {real} {held}\nThen go back to the run and press Continue.")), "{said}");
    assert!(!said.contains("Done. Go back"), "{said}");
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    start(&s, "held-start", &agent);
    let e = events(&s, "held-start");
    let blocked = e.iter().rev().find(|x| x["type"] == "Blocked").unwrap_or_else(|| panic!("{:?}", types(&e)));
    assert_eq!((&blocked["step"], &blocked["fault"]), (&serde_json::json!("runner"), &serde_json::json!("jaynes")), "{blocked}");
    let reason = blocked["reason"].as_str().unwrap_or("");
    for words in ["Jaynes held a port the command tried to serve on, so its failure says nothing about the project: ",
        "Address already in use", &format!(" It tried held port {held}."), &format!("`jaynes project test-servers {real}`")] {
        assert!(reason.contains(words), "{words}: {reason}");
    }
    let agent_worked = |e: &[Value]| e.iter().filter(|x| x["type"] == "AttemptStarted" && x["step"] == "change").count();
    assert_eq!(agent_worked(&e), 0, "no agent worked: {:?}", types(&e));
    // the card as the app lays it out, from the run's own reason (the role program's words before it)
    let store = jaynes_shell::store::Store::open(&s.home.join("record.sqlite3")).expect("store");
    let run = store.state("held-start").expect("state").expect("run");
    let b = jaynes_shell::listing::blocked(&s.home, &run).unwrap_or_else(|| panic!("not blocked: {:?}", run.phase));
    assert_eq!(b.what, "The run stopped before any agent worked.", "{reason}");
    assert_eq!(b.holds, None, "what already says no agent worked");
    assert!(b.why.starts_with(&format!("This project's tests start a test server on port {held}, which Jaynes holds")), "{}", b.why);
    assert_eq!(b.continue_does, "Continue: Jaynes runs the project's tests again. If they start, the run goes on to “change”.");
    let way = b.ways.first().expect("a way");
    assert_eq!(way.command.as_deref(), Some(format!("jaynes project test-servers {real} {held}").as_str()));
    drop(store);
    // the one line the card gives, then Continue: the tests run again, and the run goes on to its delivery
    let said = typed_at_terminal(&s, &["project", "test-servers", &project, &held.to_string()], None, "y\n");
    assert!(said.contains("Done."), "{said}");
    let said = at_terminal(&s, &["continue", "held-start"], None);
    let e = events(&s, "held-start");
    assert!(types(&e).iter().any(|t| t == "Delivered"), "{:?}\n{said}", types(&e));
    assert_eq!(agent_worked(&e), 1, "{:?}", types(&e));
    assert_eq!(free_everywhere(&[held]), held, "the hold ends with the check, and the fake database with the suite");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the window for test servers is macOS's box (Linux's box has a network of its own)")]
fn tests_that_start_their_own_test_server_without_the_person_s_yes_stop_the_run_at_its_start_with_the_line_that_lets_them() {
    // the tech-lead's T5, 23/11 (the real-user's vazio-x): a suite whose test server takes a port the system picks, as
    // Go's httptest does, in a project whose test servers the person never let. Every check would fail for it, so the
    // run stops before any agent works, with the one line that lets them, and never in the middle
    let _one = holding_alone();
    let s = scratch("serves-start");
    let project = s.project.to_str().expect("path").to_string();
    std::fs::write(s.project.join("serve.py"), "import socket\nserver = socket.socket()\nserver.bind((\"127.0.0.1\", 0))\nserver.listen()\n").expect("write");
    std::fs::write(s.project.join("Makefile"), "test:\n\tpython3 serve.py\n\tgrep -q hi greeting.txt\n").expect("write");
    for args in [vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "a test server"]] {
        assert!(Command::new("git").arg("-C").arg(&s.project).args(&args).status().expect("git").success(), "git {args:?}");
    }
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    start(&s, "serves-start", &agent);
    let e = events(&s, "serves-start");
    let blocked = e.iter().rev().find(|x| x["type"] == "Blocked").unwrap_or_else(|| panic!("{:?}", types(&e)));
    assert_eq!((&blocked["step"], &blocked["fault"]), (&serde_json::json!("runner"), &serde_json::json!("jaynes")), "{blocked}");
    let agent_worked = |e: &[Value]| e.iter().filter(|x| x["type"] == "AttemptStarted" && x["step"] == "change").count();
    assert_eq!(agent_worked(&e), 0, "no agent worked: {:?}", types(&e));
    let real = std::fs::canonicalize(&project).expect("real path").display().to_string();
    let store = jaynes_shell::store::Store::open(&s.home.join("record.sqlite3")).expect("store");
    let run = store.state("serves-start").expect("state").expect("run");
    let b = jaynes_shell::listing::blocked(&s.home, &run).unwrap_or_else(|| panic!("not blocked: {:?}", run.phase));
    assert_eq!(b.what, "The run stopped before any agent worked.", "{blocked}");
    assert!(b.why.starts_with("This project's tests start a test server on a port the system picks"), "{}", b.why);
    assert_eq!(b.continue_does, "Continue: Jaynes runs the project's tests again. If they start, the run goes on to “change”.");
    let way = b.ways.first().expect("a way");
    assert_eq!(way.command.as_deref(), Some(format!("jaynes project test-servers {real}").as_str()));
    drop(store);
    // the one line the card gives, then Continue: the tests run again, and the run goes on to its delivery
    let said = typed_at_terminal(&s, &["project", "test-servers", &project], None, "y\n");
    assert!(said.contains("Done."), "{said}");
    let said = at_terminal(&s, &["continue", "serves-start"], None);
    let e = events(&s, "serves-start");
    assert!(types(&e).iter().any(|t| t == "Delivered"), "{:?}\n{said}", types(&e));
    assert_eq!(agent_worked(&e), 1, "{:?}", types(&e));
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the box for project commands with local ports is macOS's sandbox-exec")]
fn continue_after_the_person_opens_the_ports_checks_the_same_change_again_and_the_agent_does_not_work_again() {
    let s = scratch("reopen");
    let role = port_zero_role(&s, "reopen-role", None);
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let (_, out) = engine(&s, &[
        "start", "--run", "reopen", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
        "--request", "say hi instead of hello", "--agent", &format!("command:{agent}"),
    ]);
    let e = events(&s, "reopen");
    let started = |e: &[Value]| e.iter().filter(|x| x["type"] == "AttemptStarted").count();
    assert!(e.iter().any(|x| x["type"] == "Blocked" && x["fault"] == "jaynes"), "the closed box stops the check: {out}\n{:?}", types(&e));
    assert_eq!(started(&e), 1, "{:?}", types(&e));
    // the person opens the project's ports at a terminal, after the run started, and continues
    let opened = at_terminal(&s, &["project", "open-ports", s.project.to_str().expect("path")], None);
    assert!(opened.contains("opened by you on"), "{opened}");
    let said = at_terminal(&s, &["continue", "reopen"], None);
    let e = events(&s, "reopen");
    let chosen = e.iter().find(|x| x["type"] == "PortsChosen").unwrap_or_else(|| panic!("the record does not say the ports were opened: {:?}\n{said}", types(&e)));
    assert_eq!(chosen["any"], true, "{chosen}");
    assert!(chosen["opened"].as_u64().is_some_and(|t| t > 0), "with when: {chosen}");
    assert_eq!(started(&e), 1, "the agent worked again: {:?}\n{said}", types(&e));
    let judged: Vec<&Value> = e.iter().filter(|x| x["type"] == "ChecksJudged").collect();
    assert_eq!(judged.len(), 2, "{:?}", types(&e));
    assert_eq!(judged[0]["attempt"], judged[1]["attempt"], "the same attempt's change is checked again");
    let served = judged[1]["checks"].as_array().into_iter().flatten().find(|c| c["check"] == "serves").expect("judged");
    assert_eq!(served["passed"], true, "{}\n{said}", judged[1]);
    assert_eq!(judged[0]["checks"][1]["tree"], judged[1]["checks"][1]["tree"], "the same tree");
    assert!(types(&e).iter().any(|t| t == "Delivered"), "{:?}\n{said}", types(&e));
}

/// The real-user's find of 21/11: "o Continue da página relançou o agente sem o Go no PATH", because the page's
/// process had a short PATH. Every continue carries the run on with the environment it started with.
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "the box for project commands with local ports is macOS's sandbox-exec")]
fn a_continue_from_a_process_with_a_short_path_still_finds_the_toolchain_the_run_started_with() {
    let s = scratch("shortpath");
    let tools = s.base.join("tools/bin");
    std::fs::create_dir_all(&tools).expect("dir");
    std::fs::write(tools.join("quitanda-cc"), "#!/bin/sh\nexit 0\n").expect("write");
    let mut mode = std::fs::metadata(tools.join("quitanda-cc")).expect("meta").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(tools.join("quitanda-cc"), mode).expect("chmod");
    let role = port_zero_role(&s, "shortpath-role", Some("quitanda-cc"));
    let agent = format!("perl -pi -e 's/hello/hi/' greeting.txt && {OUTPUT}");
    let long = format!("{}:{}", tools.display(), std::env::var("PATH").unwrap_or_default());
    let out = jaynes()
        .args(["start", "--run", "shortpath", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
            "--request", "say hi instead of hello", "--agent", &format!("command:{agent}")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", python()).env("PYTHONPATH", "")
        .env("PATH", &long)
        .output().expect("start");
    let e = events(&s, "shortpath");
    assert!(e.iter().any(|x| x["type"] == "Blocked" && x["fault"] == "jaynes"), "{}\n{:?}", String::from_utf8_lossy(&out.stdout), types(&e));
    at_terminal(&s, &["project", "open-ports", s.project.to_str().expect("path")], None);
    // the continue comes from a process whose PATH has none of the person's tools, as the app opened from the Dock
    let said = at_terminal(&s, &["continue", "shortpath"], Some("/usr/bin:/bin"));
    let e = events(&s, "shortpath");
    let last = e.iter().rev().find(|x| x["type"] == "ChecksJudged").expect("judged");
    let serves = last["checks"].as_array().into_iter().flatten().find(|c| c["check"] == "serves").expect("serves");
    assert_eq!(serves["passed"], true, "the check did not find the tool the run started with: {last}\n{said}");
    assert!(!said.contains("command not found"), "{said}");
    assert!(types(&e).iter().any(|t| t == "Delivered"), "{:?}\n{said}", types(&e));
}
