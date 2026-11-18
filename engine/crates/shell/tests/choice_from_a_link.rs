//! A stop that chooses, opened from the link the page printed, in a browser that has never kept the key: the real
//! page process, a run made by the core, and a test browser (Playwright's headless shell, never the person's own
//! Chrome). What each option will do is asked of the page as soon as it opens, and it must be asked as the person,
//! with the key the link brought: asked before the key is known, the engine refuses an agent's answer and the page
//! turns off the person's buttons.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");
const WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// A test browser: JAYNES_CHROME, or the newest headless shell Playwright installed; never a browser of the
/// person's Applications.
fn test_browser() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let theirs = |p: &Path| [Path::new("/Applications"), &home.join("Applications")].iter().any(|a| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).starts_with(a));
    if let Some(named) = std::env::var("JAYNES_CHROME").ok().filter(|n| !n.is_empty()) {
        let p = PathBuf::from(named);
        return (p.is_file() && !theirs(&p)).then_some(p);
    }
    let mut found: Vec<PathBuf> = [home.join("Library/Caches/ms-playwright"), home.join(".cache/ms-playwright")]
        .iter()
        .flat_map(|root| std::fs::read_dir(root).into_iter().flatten().flatten())
        .filter(|e| e.file_name().to_string_lossy().starts_with("chromium_headless_shell-"))
        .flat_map(|e| ["chrome-headless-shell-mac-arm64", "chrome-headless-shell-mac-x64", "chrome-headless-shell-linux64"].map(|d| e.path().join(d).join("chrome-headless-shell")))
        .filter(|p| p.is_file() && !theirs(p))
        .collect();
    found.sort();
    found.pop()
}

struct Scratch(PathBuf);

/// A test's folders go when it passes; a failed one keeps them for whoever reads why.
impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _gone = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// A page process that dies with its test, even when the test panics.
struct Page(std::process::Child);

impl Drop for Page {
    fn drop(&mut self) {
        let _killed = self.0.kill();
        let _reaped = self.0.wait();
    }
}

/// A run whose agent made three directions and whose stop asks the person which goes on; the question's id.
fn choosing(home: &Path) -> String {
    use jaynes_core::{AttemptOutcome, CheckResult, CheckVerdict, Command as Do, Digest, Input, Manifest};
    use jaynes_core::question::Item;
    let mut store = jaynes_shell::store::Store::open(&home.join("record.sqlite3")).expect("store");
    std::fs::create_dir_all(home.join("runs/pick")).expect("dir");
    let manifest: Manifest = serde_json::from_value(serde_json::json!({
        "role": "r", "version": "1", "protocol": 1,
        "steps": [{"name": "directions", "executor": "agent", "stop": {"verdicts": ["keep", "change", "drop"], "holds": true, "choose": true, "about": "Which direction?"}}],
    }))
    .expect("manifest");
    let start = Input::Start { run: "pick".into(), request: "a look".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false };
    store.put_setup("pick", &serde_json::json!({})).expect("setup");
    let mut todo = store.decide("pick", &start).expect("start").2;
    // what the run asks for, answered as its shell would, until its stop asks
    while let Some(c) = todo.pop() {
        let input = match c {
            Do::Probe { step, .. } => Input::ProbeEnded { step, ok: true, reason: String::new() },
            Do::RunAgent(t) => {
                let items = ["Ink", "Sun", "Moss"].map(|n| Item { id: n.to_lowercase(), label: n.into(), digest: Digest(n.into()) }).to_vec();
                Input::AttemptEnded { step: t.step, attempt: t.attempt, fix: 0, outcome: AttemptOutcome::Delivered { output: Digest("o".into()), items, changed: vec![], usage: Default::default(), session: None, tree: None } }
            }
            Do::RunChecks { step, attempt, fix, checks, .. } => {
                Input::ChecksEnded { step, attempt, fix, results: checks.into_iter().map(|check| CheckResult { check, verdict: CheckVerdict::Pass, tree: None }).collect() }
            }
            _ => continue,
        };
        todo.extend(store.decide("pick", &input).expect("fed").2);
    }
    let run = store.state("pick").expect("state").expect("run");
    let q = run.questions.values().find(|q| q.open() && q.question.choose).expect("the stop asks which goes on");
    q.question.id.clone()
}

/// Starts the page on a port the system gives and waits until it answers there.
fn page(home: &Path) -> (Page, u16) {
    use std::io::BufRead;
    let mut child = Command::new(BIN)
        .args(["page", "--port", "0"])
        .env("JAYNES_ENGINE_HOME", home).env("JAYNES_NO_PAGE", "1").env("JAYNES_NO_UPDATE_CHECK", "1").env("JAYNES_PAGE_KEY", "k3y")
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("page");
    let out = child.stderr.take().expect("what the page says");
    let page = Page(child);
    let (said, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            let _told = said.send(line);
        }
    });
    let until = std::time::Instant::now() + WAIT;
    loop {
        let left = until.saturating_duration_since(std::time::Instant::now());
        let line = heard.recv_timeout(left).expect("the page says its port");
        if let Some(port) = line.split("the page: http://localhost:").nth(1).and_then(|r| r.split('/').next()).and_then(|p| p.parse::<u16>().ok()) {
            return (page, port);
        }
    }
}

/// The page as the test browser leaves it once its scripts ran, in a profile of its own: what it shows, in HTML.
fn open(browser: &Path, profile: &Path, url: &str) -> String {
    let out = Command::new(browser)
        .args(["--headless", "--disable-gpu", "--no-first-run", "--virtual-time-budget=5000", "--dump-dom"])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(url)
        .output()
        .expect("the test browser");
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The Choose button of each option, as the page left it.
fn buttons(html: &str) -> Vec<String> {
    html.split("<button ").filter(|b| b.contains("data-choose=")).map(|b| b.split('>').next().unwrap_or("").to_string()).collect()
}

#[test]
fn a_stop_that_chooses_opened_from_a_new_link_keeps_the_person_s_buttons_on() {
    let Some(browser) = test_browser() else {
        assert!(std::env::var("JAYNES_REQUIRE_BROWSER").is_err(), "a test browser is required here and there is none");
        eprintln!("skipped: no test browser (Playwright's headless shell, or JAYNES_CHROME)");
        return;
    };
    let base = std::env::temp_dir().join(format!("jaynes-choice-link-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let _scratch = Scratch(base.clone());
    let home = base.join("home");
    let q = choosing(&home);
    let (_page, port) = page(&home);
    // a browser that never kept the key, opened from the link the page printed
    let shown = open(&browser, &base.join("profile"), &format!("http://localhost:{port}/q/pick/{q}#key=k3y"));
    let choose = buttons(&shown);
    assert_eq!(choose.len(), 3, "the three options are shown: {shown}");
    assert!(!shown.contains("You opened the page without its key"), "the link's key is the page's: {shown}");
    assert!(choose.iter().all(|b| !b.contains("disabled")), "every Choose stays on: {choose:?}");
    // and what each option will do is told, not refused
    assert!(!shown.contains("class=\"bad\""), "nothing was refused: {shown}");
}
