//! Reading the person's words on the page: only the person starts it, at most two readings run at once, and a
//! reading that never ends is stopped at its deadline. The model is a shell command here (`JAYNES_UNDERSTAND`), so no
//! tokens are spent: it marks that it started, then sleeps.

use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");
const KEY: &str = "k3y";

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

struct Page(std::process::Child);

impl Drop for Page {
    fn drop(&mut self) {
        let _killed = self.0.kill();
        let _reaped = self.0.wait();
    }
}

fn scratch(name: &str) -> Scratch {
    let base = std::env::temp_dir().join(format!("jaynes-understand-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let (home, project) = (base.join("home"), base.join("project"));
    std::fs::create_dir_all(&project).expect("dir");
    std::fs::write(project.join("greeting.txt"), "hello\n").expect("write");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
        assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success());
    }
    Scratch { base, home, project }
}

fn engine(s: &Scratch, args: &[&str]) -> (bool, String) {
    let out = Command::new(BIN)
        .args(args)
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
        .output()
        .expect("engine");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// A run waiting at a question, and the question's id.
fn asking(s: &Scratch) -> String {
    let role = s.base.join("say");
    std::fs::create_dir_all(&role).expect("dir");
    std::fs::write(role.join("role.toml"), r#"
role = "say"
version = "1.0.0"

[[step]]
name = "say"
reach = { project = "read" }
instructions = "Say {{request}}."
stop = { ask = "Keep it?", verdicts = ["keep", "change"] }

[step.output]
said = "string"
"#).expect("write");
    let agent = r#"command:printf '{"said":"hi"}' > .jaynes/output.json && echo '{"tokens":1}'"#;
    let (ok, out) = engine(s, &["start", "--run", "u", "--role", role.to_str().expect("path"), "--project", s.project.to_str().expect("path"),
                                "--request", "hi", "--agent", agent]);
    assert!(ok && out.contains("Keep it?"), "{out}");
    let (_, log) = engine(s, &["log", "u"]);
    log.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).find(|e| e["type"] == "QuestionOpened").expect("asked")["question"]["id"]
        .as_str().expect("id").to_string()
}

/// A port for a page, as page_fixes.rs draws one: at random among 20000, below the range outgoing connections take,
/// free to bind on both of the page's addresses and with nothing answering there, since a bind alone does not tell it
/// free on macOS (SO_REUSEADDR lets a bind on 127.0.0.1 through where a program listens on every address).
fn port() -> u16 {
    use std::hash::{BuildHasher, Hasher};
    let answers = |address: &str, p: u16| {
        let at: std::net::SocketAddr = format!("{address}:{p}").parse().expect("address");
        TcpStream::connect_timeout(&at, Duration::from_millis(200)).is_ok()
    };
    for _ in 0..200 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        let p = 20000 + (h.finish() % 20000) as u16;
        let four = std::net::TcpListener::bind(("127.0.0.1", p)).is_ok();
        let six = !std::net::TcpListener::bind(("::1", p)).is_err_and(|e| e.kind() == std::io::ErrorKind::AddrInUse);
        if four && six && !answers("127.0.0.1", p) && !answers("[::1]", p) {
            return p;
        }
    }
    panic!("no free port between 20000 and 40000")
}

/// The page, with a model that is `model` (a shell command) and readings that may last `seconds`.
fn page(s: &Scratch, port: u16, model: &str, seconds: &str) -> Page {
    let child = Command::new(BIN)
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
        .env("JAYNES_PAGE_KEY", KEY).env("JAYNES_NO_UPDATE_CHECK", "1")
        .env("JAYNES_UNDERSTAND", model).env("JAYNES_UNDERSTAND_SECONDS", seconds)
        .stderr(std::process::Stdio::null()).stdout(std::process::Stdio::null())
        .spawn()
        .expect("page");
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Page(child)
}

/// Ask the page to read the person's words: the status line and the body, or None when no answer came in `wait`.
fn understand(port: u16, question: &str, key: bool, wait: Duration) -> Option<(String, Value)> {
    let body = format!(r#"{{"run":"u","question":"{question}","text":"keep it as it is"}}"#);
    let key = if key { format!("X-Jaynes-Key: {KEY}\r\n") } else { String::new() };
    let raw = format!(
        "POST /api/understand HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\nContent-Type: application/json\r\n{key}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(wait)).expect("timeout");
    stream.write_all(raw.as_bytes()).expect("write");
    let mut out = String::new();
    match stream.read_to_string(&mut out) {
        Ok(_) if !out.is_empty() => {}
        _ => return None,
    }
    let status = out.lines().next().unwrap_or("").to_string();
    let json = serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap_or("")).unwrap_or(Value::Null);
    Some((status, json))
}

fn lines(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path).map(|t| t.lines().count()).unwrap_or(0)
}

#[test]
fn without_the_person_s_key_no_model_is_started() {
    let s = scratch("nokey");
    let q = asking(&s);
    let marks = s.base.join("started.txt");
    let port = port();
    let _page = page(&s, port, &format!("echo started >> {}; sleep 5; echo '{{}}'", marks.display()), "30");
    let said = understand(port, &q, false, Duration::from_secs(3));
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(lines(&marks), 0, "a client without the key started the model: {said:?}");
    let (status, body) = said.expect("refused at once, not left waiting");
    assert!(status.contains("403"), "{status} {body}");
    assert_eq!(body["ok"], false, "{body}");
}

#[test]
fn a_third_reading_at_once_is_told_busy_and_starts_nothing() {
    let s = scratch("busy");
    let q = asking(&s);
    let marks = s.base.join("started.txt");
    let port = port();
    let _page = page(&s, port, &format!("echo started >> {}; sleep 4; echo '{{}}'", marks.display()), "30");
    let first: Vec<_> = (0..2)
        .map(|_| {
            let q = q.clone();
            std::thread::spawn(move || understand(port, &q, true, Duration::from_secs(20)))
        })
        .collect();
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(lines(&marks), 2, "two readings run");
    let started = Instant::now();
    let third = understand(port, &q, true, Duration::from_secs(3));
    let took = started.elapsed();
    let (status, body) = third.expect("the third is answered at once");
    assert!(took < Duration::from_secs(2), "the third waited {took:?}");
    assert!(status.contains("429"), "{status} {body}");
    assert_eq!(body["busy"], true, "{body}");
    assert_eq!(lines(&marks), 2, "the third started no model");
    for t in first {
        assert!(t.join().expect("reading").is_some(), "the first two are answered");
    }
    // their turns are given back: a reading now is not busy
    let after = understand(port, &q, true, Duration::from_secs(20)).expect("answered");
    assert_ne!(after.1["busy"], true, "{:?}", after);
}

#[test]
fn a_reading_is_not_closed_to_make_way_for_connections_that_come_after_it() {
    let s = scratch("flood");
    let q = asking(&s);
    let marks = s.base.join("started.txt");
    let port = port();
    let _page = page(&s, port, &format!("echo started >> {}; sleep 3; echo '{{}}'", marks.display()), "30");
    let reading = {
        let q = q.clone();
        std::thread::spawn(move || understand(port, &q, true, Duration::from_secs(20)))
    };
    let begun = Instant::now();
    while lines(&marks) == 0 && begun.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(lines(&marks), 1, "the reading started");
    // more connections than the page serves at once come after it, and hold their places saying nothing
    let flood: Vec<TcpStream> = (0..70).filter_map(|_| TcpStream::connect(("127.0.0.1", port)).ok()).collect();
    assert!(flood.len() >= 65, "only {} connected", flood.len());
    let (status, body) = reading.join().expect("reading").expect("the reading is answered, not closed to make way");
    assert!(status.contains("200"), "{status} {body}");
    drop(flood);
}

#[test]
fn a_reading_that_never_ends_is_stopped_at_its_deadline() {
    let s = scratch("deadline");
    let q = asking(&s);
    let pids = s.base.join("pids.txt");
    let port = port();
    let _page = page(&s, port, &format!("echo $$ >> {}; exec sleep 600", pids.display()), "2");
    let started = Instant::now();
    let said = understand(port, &q, true, Duration::from_secs(15));
    let took = started.elapsed();
    let (_, body) = said.expect("answered after the deadline, not left waiting");
    assert!(took < Duration::from_secs(8), "it took {took:?}");
    assert_eq!(body["ok"], false, "{body}");
    assert!(body["refused"].as_str().unwrap_or("").contains("longer than 2 s"), "the person reads that it did not work: {body}");
    let pid = std::fs::read_to_string(&pids).expect("the model started").trim().to_string();
    let alive = Command::new("kill").args(["-0", &pid]).stderr(std::process::Stdio::null()).status().expect("kill").success();
    assert!(!alive, "the model's process {pid} still runs after the deadline");
}

/// The first item of the run's question.
fn first_item(s: &Scratch) -> String {
    let (_, log) = engine(s, &["log", "u"]);
    log.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).find(|e| e["type"] == "QuestionOpened").expect("asked")["question"]["items"][0]["id"]
        .as_str().expect("item").to_string()
}

/// Ask the page to read `text`, with the key.
fn read_words(port: u16, question: &str, text: &str) -> Value {
    let body = serde_json::json!({"run": "u", "question": question, "text": text}).to_string();
    let raw = format!(
        "POST /api/understand HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\nContent-Type: application/json\r\nX-Jaynes-Key: {KEY}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(20))).expect("timeout");
    stream.write_all(raw.as_bytes()).expect("write");
    let mut out = Vec::new();
    let _read = stream.read_to_end(&mut out);
    let out = String::from_utf8(out).expect("utf-8");
    serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap_or("")).unwrap_or(Value::Null)
}

// The real-user, 22/11 (estoque): "pode deixar o banco falso dos testes numa porta livre, do jeito que você fez.
// entrega assim." came back as "you wrote: … A 5432 continua ocupada …", which the person never said.
#[test]
fn the_person_s_words_are_kept_as_written_and_the_model_only_reads_the_verdict() {
    let s = scratch("words");
    let q = asking(&s);
    let id = first_item(&s);
    let words = "pode deixar o banco falso dos testes numa porta livre, do jeito que você fez. entrega assim.";
    let invented = "A 5432 continua ocupada";
    // the model reads a change, and writes its own note with a fact the person did not say
    let said = s.base.join("model.json");
    std::fs::write(&said, serde_json::json!({
        "verdicts": {&id: "change"},
        "notes": {&id: format!("Aceita a opção 1: o banco falso fica numa porta livre. {invented}. Entregar assim.")},
        "back": {}, "unsure": "", "repeats": []
    }).to_string()).expect("write");
    let port = port();
    let _page = page(&s, port, &format!("cat > /dev/null; cat {}", said.display()), "30");
    let r = read_words(port, &q, words);
    assert_eq!(r["body"]["verdicts"][&id], "change", "the model's verdict is read: {r}");
    assert_eq!(r["body"]["notes"][&id].as_str(), Some(words), "the change carries the person's words, byte for byte: {r}");
    assert_eq!(r["body"]["context"].as_str(), Some(words), "{r}");
    assert!(!r.to_string().contains(invented), "a fact the person did not say reached the reading: {r}");
    let sentences: Vec<&str> = r["sentences"].as_array().expect("sentences").iter().filter_map(Value::as_str).collect();
    assert!(sentences.iter().any(|t| t.contains(words)), "what Jaynes says it does quotes the person's words: {sentences:?}");
}
