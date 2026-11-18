//! A reminder on the page end to end: the real page process, a record whose two runs have waited on the person for
//! two days, and a test browser (Playwright's headless shell, never the person's own Chrome) that opens the page as
//! the person does. The page shows the reminders at its first screen; only the person's opening, with the key,
//! puts them off a day.

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

/// A test's folders go when it passes; a failed one keeps them for whoever reads why, and JAYNES_KEEP_TEST_RUNS=1
/// keeps every one.
impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() && std::env::var("JAYNES_KEEP_TEST_RUNS").as_deref() != Ok("1") {
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

/// Two runs, each waiting on its person's step, whose question the person was told of two days ago: made by the
/// core, as a run is, Jaynes's own step done first.
fn told_two_days_ago(home: &Path) {
    use jaynes_core::{AttemptOutcome, Digest, Input, Manifest};
    let mut store = jaynes_shell::store::Store::open(&home.join("record.sqlite3")).expect("store");
    for run in ["w1", "w2"] {
        std::fs::create_dir_all(home.join("runs").join(run)).expect("dir");
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "role": "r", "version": "1", "protocol": 1,
            "steps": [
                {"name": "make", "executor": "role"},
                {"name": "look", "executor": "person", "needs": ["make"], "stop": {"verdicts": ["keep"], "holds": true, "about": "Is it right?"}},
            ],
        }))
        .expect("manifest");
        let start = Input::Start { run: run.into(), request: format!("the request of {run}"), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None };
        store.put_setup(run, &serde_json::json!({})).expect("setup");
        store.decide(run, &start).expect("start");
        store.decide(run, &Input::ProbeEnded { step: "make".into(), ok: true, reason: String::new() }).expect("probe");
        let attempt = store.state(run).expect("state").expect("run").steps[0].attempt;
        let done = AttemptOutcome::Delivered { output: Digest("o".into()), items: vec![], changed: vec![], usage: Default::default(), session: None };
        store.decide(run, &Input::AttemptEnded { step: "make".into(), attempt, fix: 0, outcome: done }).expect("done");
        let waits = store.state(run).expect("state").expect("run");
        assert!(!waits.told.is_empty(), "the run waits on the person: {:?}", waits.phase);
    }
    // two days back: the record's clock is the time each entry was written
    let db = rusqlite::Connection::open(home.join("record.sqlite3")).expect("record");
    db.execute("update events set ts = ts - 172800", []).expect("back-dated");
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
            while std::time::Instant::now() < until {
                if get(port, "/api/version").starts_with("HTTP/1.1 200") {
                    return (page, port);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            panic!("the page did not answer on {port}");
        }
    }
}

fn get(port: u16, path: &str) -> String {
    use std::io::{Read, Write};
    let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) else { return String::new() };
    let _timed = s.set_read_timeout(Some(WAIT));
    let _sent = s.write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes());
    let mut out = String::new();
    let _read = s.read_to_string(&mut out);
    out
}

/// The page as the test browser leaves it once its scripts ran: what it shows, in HTML.
fn open(browser: &Path, profile: &Path, url: &str) -> String {
    let out = Command::new(browser)
        .args(["--headless", "--disable-gpu", "--no-first-run", "--virtual-time-budget=5000", "--dump-dom"])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(url)
        .output()
        .expect("the test browser");
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn the_page_reminds_the_person_at_its_first_screen_and_their_opening_puts_it_off_a_day() {
    let Some(browser) = test_browser() else {
        assert!(std::env::var("JAYNES_REQUIRE_BROWSER").is_err(), "a test browser is required here and there is none");
        eprintln!("skipped: no test browser (Playwright's headless shell, or JAYNES_CHROME)");
        return;
    };
    let base = std::env::temp_dir().join(format!("jaynes-remind-e2e-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let _scratch = Scratch(base.clone());
    let home = base.join("home");
    told_two_days_ago(&home);
    let (_page, port) = page(&home);
    let first = get(port, "/");
    let q2 = jaynes_shell::store::Store::open(&home.join("record.sqlite3")).expect("store").state("w2").expect("state").expect("run").told.into_iter().next().expect("told");
    assert!(first.contains("<h2>Still waiting for you</h2>") && first.contains(&format!("href=\"/q/w2/{q2}\"")), "{first}");
    let marked = |run: &str| home.join("runs").join(run).join("reminded.json").exists();
    // opened without the key: the page runs its scripts, says so, and nobody was reminded
    let shown = open(&browser, &base.join("profile-without"), &format!("http://localhost:{port}/"));
    assert!(shown.contains("You opened the page without its key"), "the page's scripts ran: {shown}");
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(!marked("w1") && !marked("w2"));
    assert!(get(port, "/").contains("<h2>Still waiting for you</h2>"));
    // opened with the key, as from the link the page printed: both shown are put off a day
    let shown = open(&browser, &base.join("profile-with"), &format!("http://localhost:{port}/#key=k3y"));
    assert!(shown.contains("Still waiting for you") && !shown.contains("You opened the page without its key"), "{shown}");
    let until = std::time::Instant::now() + WAIT;
    while !(marked("w1") && marked("w2")) && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(marked("w1") && marked("w2"), "the person's opening marks what it showed");
    let after = get(port, "/");
    assert!(after.contains("HTTP/1.1 200") && !after.contains("id=\"remind\""), "{after}");
}
