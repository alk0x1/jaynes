//! The page: the questions waiting for the person, one at a time, answered in rows with a line of context.
//!
//! A local web server whose one origin is http://localhost:PORT: it listens on 127.0.0.1 and on ::1, so no other
//! program answers for that name. Whoever opened the link with the page's key (kept by the browser for that origin)
//! answers as the person; anyone else is an agent, and the core refuses what an agent may not answer. What an answer will do is written by the core (`jaynes_core::effects`) and fetched as the person marks
//! the rows; the page never writes it.
//! Requests must name this server as their host and origin, and posts must be JSON, so another site open in the
//! same browser cannot answer.

use crate::drive::{self, say};
use crate::listing::{short, title, where_in};
use crate::stop::label_for;
use crate::store::Store;
use jaynes_core::question::{Answer, By, Verdict};
use jaynes_core::run::{Phase, QuestionState, Run};
use jaynes_core::{decide, Input};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

#[derive(Clone)]
pub struct Page {
    pub home: PathBuf,
    pub port: u16,
    pub key: String,
    pub exe: PathBuf,
}

struct Request {
    method: String,
    /// The path and query as they came, for a request sent on to the page's name.
    target: String,
    path: String,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

/// A new key for the page, from the system's randomness. Without it there is no key: one that could be guessed would
/// be kept for good.
pub fn key() -> Result<String, String> {
    key_from(std::path::Path::new("/dev/urandom"))
}

fn key_from(randomness: &std::path::Path) -> Result<String, String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open(randomness)
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| format!("cannot make the page's key, since the system's randomness does not read: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// What the page answers to prove it holds `key` without saying it: a fingerprint of the key and a nonce the asker
/// chose. Only the page can make it, and it tells another program nothing of the key.
pub fn proof(key: &str, nonce: &str) -> String {
    jaynes_core::Digest::of_bytes(format!("jaynes page proof\n{key}\n{nonce}").as_bytes()).0
}

/// The commit of Jaynes installed when the page started: what it runs.
static RUNNING: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
/// The newest commit where Jaynes is published, asked once an hour.
static LATEST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
/// Connections the page serves at once, on both of its addresses.
const MOST_AT_ONCE: usize = 64;

/// A connection being served: when it came, a handle to close it, and whether it is kept from being closed to make
/// way, as a reading of the person's words is.
struct Slot {
    id: u64,
    since: std::time::Instant,
    stream: TcpStream,
    kept: bool,
}

static SERVED: std::sync::Mutex<(u64, Vec<Slot>)> = std::sync::Mutex::new((0, Vec::new()));

/// Readings of the person's words by a model at once, page-wide: each costs the person's tokens and a process.
const MOST_READINGS: usize = 2;
// a reading is kept from being closed when the page is full, so a place that is not kept must always be left to close
const _: () = assert!(MOST_READINGS < MOST_AT_ONCE);
static READINGS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A reading's place among those that run at once, given back when it ends, however it ends.
struct Reading;

impl Reading {
    fn take() -> Option<Reading> {
        use std::sync::atomic::Ordering;
        let mut n = READINGS.load(Ordering::SeqCst);
        loop {
            if n >= MOST_READINGS {
                return None;
            }
            match READINGS.compare_exchange(n, n + 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return Some(Reading),
                Err(now) => n = now,
            }
        }
    }
}

impl Drop for Reading {
    fn drop(&mut self) {
        READINGS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A connection's place among those served at once, given back when it ends, however it ends.
struct Turn(u64);

impl Turn {
    /// A place for a new connection. When every place is taken, the oldest connection is closed to make way,
    /// whether it is still sending its request or still taking its reply: one that only holds its place, sending or
    /// taking a byte at a time, cannot keep the person out, and a new connection is never refused. A kept one, a
    /// reading the person waits for, is passed over: there are at most [`MOST_READINGS`], far fewer than the places.
    fn take(stream: &TcpStream) -> Option<Turn> {
        let handle = stream.try_clone().ok()?;
        let mut served = SERVED.lock().ok()?;
        if served.1.len() >= MOST_AT_ONCE
            && let Some(oldest) = served.1.iter().enumerate().min_by_key(|(_, s)| (s.kept, s.since)).map(|(i, _)| i)
        {
            let gone = served.1.remove(oldest);
            let _closed = gone.stream.shutdown(std::net::Shutdown::Both);
        }
        served.0 += 1;
        let id = served.0;
        served.1.push(Slot { id, since: std::time::Instant::now(), stream: handle, kept: false });
        Some(Turn(id))
    }

    /// Keep this connection from being closed to make way for others. Only a reading is kept, so at most
    /// [`MOST_READINGS`] are, and [`Turn::take`] always finds one not kept to close while they are fewer than the
    /// places, which the build holds. Were the limit of readings ever set at run time, that would need holding there.
    fn keep(&self) {
        if let Ok(mut served) = SERVED.lock()
            && let Some(s) = served.1.iter_mut().find(|s| s.id == self.0)
        {
            s.kept = true;
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        if let Ok(mut served) = SERVED.lock() {
            served.1.retain(|s| s.id != self.0);
        }
    }
}

/// How long a connection has to send its whole request, from when it came.
const HEAD_DEADLINE: std::time::Duration = std::time::Duration::from_secs(3);
/// How long a connection has for all of it, from when it came: its request, and taking the whole of its reply. The
/// largest reply is a page an agent wrote, kept with no limit on its size; at even 1 MB a second, a thousand times
/// slower than a browser takes it from this machine, 30 seconds carry more than 25 MB after the request's 3.
const WHOLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
/// The most a request's head may hold, and its body: the page's own are far smaller.
const MOST_HEAD: u64 = 64 * 1024;
const MOST_BODY: u64 = 1 << 20;

/// Whether an update the page started is still building.
static UPDATING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Times a page asked for any port (0) asks again when the port the system gave it on 127.0.0.1 is held on ::1.
const PORT_TRIES: u32 = 5;

/// The page's two addresses on one port: 127.0.0.1, and ::1 where the machine has it, since localhost may reach
/// either and another program on ::1 would answer for the page. With `port` 0 the system gives the port, and a port
/// another program holds on ::1 is let go and another asked for, [`PORT_TRIES`] times; with a port given, that
/// program is an error at once. `bind` is how an address is taken: the system's, or a test's.
fn hold(port: u16, mut bind: impl FnMut(&str, u16) -> std::io::Result<TcpListener>) -> Result<(TcpListener, Option<TcpListener>), String> {
    let tries = if port == 0 { PORT_TRIES } else { 1 };
    for attempt in 1..=tries {
        let four = bind("127.0.0.1", port).map_err(|e| format!("cannot listen on {port}: {e}"))?;
        let given = four.local_addr().map_err(|e| e.to_string())?.port();
        match bind("::1", given) {
            Ok(six) => return Ok((four, Some(six))),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && attempt < tries => drop(four),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                return Err(if port == 0 {
                    format!("another program answered on ::1 at each of {tries} ports the system gave on 127.0.0.1, last [::1]:{given}, and it would answer for localhost, the page's origin: the page does not start")
                } else {
                    format!("another program answers on [::1]:{given}, and it would answer for localhost, the page's origin: the page does not start. Stop that program, or give the page another port")
                });
            }
            Err(e) => {
                say(&format!("no IPv6 here ({e}): the page listens on 127.0.0.1 alone, which is all localhost reaches"));
                return Ok((four, None));
            }
        }
    }
    Err("the page found no port".into())
}

impl Page {
    /// Serve the page on its port, or with port 0 on the one the system gives, which it takes as its own (the origin
    /// it answers for, what it restarts on) and says in its first line.
    pub fn serve(&self) -> Result<(), String> {
        let (listener, six) = hold(self.port, |address, port| TcpListener::bind((address, port)))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let page = Page { port, ..self.clone() };
        RUNNING.get_or_init(crate::install::installed_commit);
        if std::env::var("JAYNES_NO_UPDATE_CHECK").is_err() {
            std::thread::spawn(|| loop {
                if let Some(c) = crate::install::latest_commit()
                    && let Ok(mut l) = LATEST.lock()
                {
                    *l = Some(c);
                }
                std::thread::sleep(std::time::Duration::from_secs(3600));
            });
        }
        // the key never goes to what the page says, which Jaynes keeps in page.log: whoever needs it reads its file
        say(&format!(
            "the page: http://localhost:{port}/ . Open it with its key: http://localhost:{port}/#key= followed by what {} holds",
            page.home.join("page.key").display(),
            port = page.port
        ));
        if let Some(six) = six {
            let other = page.clone();
            let _started = std::thread::Builder::new().name("page-six".into()).spawn(move || other.accept(&six));
        }
        page.accept(&listener);
        Ok(())
    }

    /// Answer what comes to one address. Each connection on its own, with [`HEAD_DEADLINE`] from when it came to send
    /// its whole request and [`WHOLE_DEADLINE`] to have taken its reply: one that says nothing, or speaks or listens a
    /// byte at a time, holds no other, and one whose request breaks its handler ends alone, never the page. Past
    /// [`MOST_AT_ONCE`] in service, the oldest is closed to make way.
    fn accept(&self, listener: &TcpListener) {
        for stream in listener.incoming().flatten() {
            let came = std::time::Instant::now();
            let Some(turn) = Turn::take(&stream) else {
                drop(stream);
                continue;
            };
            let page = self.clone();
            let _started = std::thread::Builder::new().name("page-connection".into()).spawn(move || {
                let _done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| page.handle(stream, turn, came)));
            });
        }
    }

    /// Answer one connection, holding its `turn` among those served at once until its reply is sent.
    fn handle(&self, stream: TcpStream, turn: Turn, came: std::time::Instant) -> Result<(), String> {
        let req = read(&stream, came + HEAD_DEADLINE)?;
        // a font is bytes, and does not change: served here, kept by the browser
        if req.method == "GET" && self.host(&req) == Host::Name
            && let Some(name) = req.path.strip_prefix("/fonts/")
            && let Some((_, kind, bytes)) = FONTS.iter().find(|(n, _, _)| *n == name)
        {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: public, max-age=31536000, immutable\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            return send(&stream, came + WHOLE_DEADLINE, &[head.as_bytes(), bytes]);
        }
        // Reading the person's words takes a model's time, and the person's tokens: only the person starts it, at most
        // MOST_READINGS run at once, and each is answered on a thread of its own that keeps this connection's turn
        // until the reply is sent, so the page goes on and its limit of connections still counts it.
        if req.method == "POST" && req.path == "/api/understand" && self.host(&req) != Host::Other {
            let json_body = req.headers.get("content-type").is_some_and(|c| c.starts_with("application/json"));
            let origin_ok = req.headers.get("origin").is_none_or(|o| self.local(Some(o), true));
            if json_body && origin_ok {
                if !self.person(&req) {
                    let said = json!({"ok": false, "refused": "only the person has their words read here: open the page with its key"});
                    return reply_json(&stream, came + WHOLE_DEADLINE, "403 Forbidden", &said);
                }
                let Some(reading) = Reading::take() else {
                    let said = json!({"ok": false, "busy": true, "refused": "Jaynes is reading two answers already: try again in a moment, or answer with the buttons"});
                    return reply_json(&stream, came + WHOLE_DEADLINE, "429 Too Many Requests", &said);
                };
                // a reading the person waits for is not closed to make way for others: at most two are kept so
                turn.keep();
                let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                let home = self.home.clone();
                std::thread::spawn(move || {
                    let said = understand_in(&home, &body, By::Person).unwrap_or_else(|e| json!({"ok": false, "refused": e}));
                    // the reading took a model's time, which is not the connection's: its reply has its own deadline
                    let _sent = reply_json(&stream, std::time::Instant::now() + WHOLE_DEADLINE, "200 OK", &said);
                    drop(reading);
                    drop(turn);
                });
                return Ok(());
            }
        }
        let (status, kind, body, extra) = self.route(&req);
        let mut head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Frame-Options: DENY\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n",
            body.len()
        );
        for (k, v) in extra {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        send(&stream, came + WHOLE_DEADLINE, &[head.as_bytes(), body.as_bytes()])
    }

    /// Whether an Origin is this page's, exactly: http://localhost:PORT, its one origin.
    fn local(&self, value: Option<&String>, scheme: bool) -> bool {
        let p = self.port;
        let ours = if scheme { format!("http://localhost:{p}") } else { format!("localhost:{p}") };
        value.is_some_and(|v| *v == ours)
    }

    /// How a request names the page: by its name, by one of its numbers (which a browser is sent on from, and a
    /// program may still use, as an agent), or not at all.
    fn host(&self, req: &Request) -> Host {
        let p = self.port;
        match req.headers.get("host").map(String::as_str) {
            Some(h) if h == format!("localhost:{p}") => Host::Name,
            Some(h) if h == format!("127.0.0.1:{p}") || h == format!("[::1]:{p}") => Host::Number,
            _ => Host::Other,
        }
    }

    /// Whether a request is the person's: it carries the page's key in its header and comes from the page's own
    /// origin, as the page's script sends it. A program that sends no origin is never the person, key or not. This
    /// keeps other sites and apps out; it does not prove the person to a program that can read the key's file.
    fn person(&self, req: &Request) -> bool {
        let key = req.headers.get("x-jaynes-key").is_some_and(|k| !self.key.is_empty() && *k == self.key);
        key && self.local(req.headers.get("origin"), true)
    }

    /// Whether a request still carries the cookie earlier versions kept the key in.
    fn old_cookie(req: &Request) -> bool {
        req.headers.get("cookie").is_some_and(|c| c.split(';').any(|c| c.trim().starts_with("jaynes_key=")))
    }

    fn route(&self, req: &Request) -> (&'static str, &'static str, String, Vec<(String, String)>) {
        match self.host(req) {
            Host::Name => {}
            // an old link or bookmark by number goes on to the page's name, path and query kept, and the browser takes
            // the fragment along; the cookie earlier versions kept for that name is cleared there
            Host::Number if req.method == "GET" && !req.target.starts_with('/') => {
                return ("400 Bad Request", "text/plain", "a request names a path".into(), vec![]);
            }
            Host::Number if req.method == "GET" => {
                let mut headers = vec![("Location".to_string(), format!("http://localhost:{}{}", self.port, req.target))];
                if Self::old_cookie(req) {
                    headers.push(("Set-Cookie".into(), CLEAR.into()));
                }
                return ("308 Permanent Redirect", "text/plain", String::new(), headers);
            }
            // a program, such as an agent, may still speak to the page by number, and never as the person
            Host::Number if !req.headers.contains_key("origin") => {}
            _ => return ("403 Forbidden", "text/plain", "this page answers only on its own local address".into(), vec![]),
        }
        if req.method == "GET" {
            // a link of earlier versions, with the key in its query: it goes on to the fragment, and the old cookie goes
            if let Some(k) = req.query.get("key")
                && *k == self.key {
                    // always to the page's root: a path that starts with // would leave for another host
                    let to = format!("/#key={}", self.key);
                    return ("303 See Other", "text/plain", String::new(), vec![("Set-Cookie".into(), CLEAR.into()), ("Location".into(), to)]);
                }
            if req.path.starts_with("/fonts/") {
                return ("404 Not Found", "text/plain", "no such font".into(), vec![]);
            }
            if req.path == "/api/knows" {
                // the nonce is the asker's: hex, short, so the answer says nothing but the proof
                let nonce = req.query.get("nonce").filter(|n| !n.is_empty() && n.len() <= 64 && n.bytes().all(|b| b.is_ascii_hexdigit()));
                return match nonce {
                    Some(n) => ("200 OK", "application/json", json!({"proof": proof(&self.key, n)}).to_string(), vec![]),
                    None => ("400 Bad Request", "text/plain", "a nonce, in hex".into(), vec![]),
                };
            }
            if req.path == "/key.js" {
                return ("200 OK", "text/javascript; charset=utf-8", KEY_SCRIPT.to_string(), vec![]);
            }
            if req.path == "/page.js" {
                return ("200 OK", "text/javascript; charset=utf-8", SCRIPT.to_string(), vec![]);
            }
            if let Some(name) = req.path.strip_prefix("/page/") {
                // a page an agent wrote, kept by its fingerprint: sandboxed, with no origin, so it cannot answer here,
                // and reaching nothing beyond itself
                let read = if kept_page(name) { std::fs::read_to_string(self.home.join("pictures").join(name)).ok() } else { None };
                return match read {
                    Some(body) => {
                        let policy = "sandbox allow-scripts; default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src data:; font-src data:";
                        ("200 OK", "text/html; charset=utf-8", body, vec![("Content-Security-Policy".into(), policy.into())])
                    }
                    _ => ("404 Not Found", "text/plain", "no such page".into(), vec![]),
                };
            }
            if req.path == "/api/version" {
                let seq = self.store().and_then(|s| s.last_seq()).unwrap_or(0);
                return ("200 OK", "application/json", json!({"seq": seq}).to_string(), vec![]);
            }
            let parts: Vec<&str> = req.path.trim_matches('/').split('/').collect();
            let view = match parts.as_slice() {
                ["q", run, q] => View::Question(run.to_string(), q.to_string()),
                ["run", run] => View::Run(run.to_string()),
                ["home"] => View::Overview,
                ["new"] => View::New,
                ["setup"] => View::Setup,
                _ => View::Home,
            };
            let mut headers = vec![("Content-Security-Policy".to_string(), POLICY.to_string())];
            if Self::old_cookie(req) {
                headers.push(("Set-Cookie".into(), CLEAR.into()));
            }
            return match self.render(view, crate::store::now()) {
                Ok(html) => ("200 OK", "text/html; charset=utf-8", html, headers),
                Err(e) => ("500 Internal Server Error", "text/plain", e, vec![]),
            };
        }
        if req.method == "POST" {
            let json_body = req.headers.get("content-type").is_some_and(|c| c.starts_with("application/json"));
            let origin_ok = req.headers.get("origin").is_none_or(|o| self.local(Some(o), true));
            if !json_body || !origin_ok {
                // an answer or an action sent in the person's place from elsewhere leaves a trace on its run
                if matches!(req.path.as_str(), "/api/answer" | "/api/act") {
                    let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                    let from = crate::refused::from_origin(req.headers.get("origin").map(String::as_str));
                    let what = if req.path == "/api/act" { crate::refused::action(body["action"].as_str().unwrap_or("")) } else { "answer".into() };
                    crate::refused::record(&self.home, &crate::refused::Refusal {
                        at: crate::store::now(),
                        run: body["run"].as_str().unwrap_or("").to_string(),
                        what,
                        who: from,
                        reason: "it did not come from this page".into(),
                    });
                }
                return ("403 Forbidden", "text/plain", "only JSON from this page".into(), vec![]);
            }
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let by = if self.person(req) { By::Person } else { By::Agent { who: "a client without the page's key".into() } };
            let asked_by = by.clone();
            let out = match req.path.as_str() {
                "/api/effects" => self.effects(&body, by),
                "/api/answer" => self.answer(&body, by),
                "/api/act" => self.act(&body, by),
                "/api/start" => self.start(&body, by),
                "/api/setup" => self.setup(&body, by),
                // the person is on a stop's screen: what the people's minutes of the run's summary are measured from
                "/api/look" => crate::presence::look(&self.home, body["run"].as_str().unwrap_or(""), body["question"].as_str().unwrap_or(""), &by, crate::store::now())
                    .map(|recorded| json!({"ok": true, "recorded": recorded})),
                // whether this page counts what comes from here as the person's: the page says so when it does not
                "/api/whoami" => Ok(json!({"ok": true, "person": by.person()})),
                // the person saw the reminders the page showed them, at a pause of theirs: the next come a day later
                "/api/reminded" => reminded_in(&self.home, &body, &by, crate::store::now()),
                _ => Err("no such action".to_string()),
            };
            let reply = match out {
                Ok(v) => v,
                Err(e) => {
                    if matches!(req.path.as_str(), "/api/answer" | "/api/act") {
                        crate::refused::record(&self.home, &self.refusal(&req.path, &body, &asked_by, &e));
                    }
                    json!({"ok": false, "refused": e})
                }
            };
            return ("200 OK", "application/json", reply.to_string(), vec![]);
        }
        ("405 Method Not Allowed", "text/plain", String::new(), vec![])
    }

    /// The trace of an answer or an action the page refused: on which step's question or which action, by whom.
    fn refusal(&self, path: &str, body: &Value, by: &By, reason: &str) -> crate::refused::Refusal {
        let run = body["run"].as_str().unwrap_or("").to_string();
        let what = if path == "/api/act" {
            crate::refused::action(body["action"].as_str().unwrap_or(""))
        } else {
            let step = self.store().ok().and_then(|s| s.state(&run).ok().flatten()).and_then(|r| {
                body["question"].as_str().and_then(|q| r.questions.get(q)).map(|q| q.question.step.clone())
            });
            step.map(|s| format!("answer to {s}")).unwrap_or_else(|| "answer".into())
        };
        // the engine's words, without the request's: its errors may name what they were given
        crate::refused::Refusal { at: crate::store::now(), run, what, who: crate::refused::who(by), reason: crate::refused::scrub(reason, body) }
    }

    fn store(&self) -> Result<Store, String> {
        Store::open(&self.home.join("record.sqlite3"))
    }

    fn answer_from(&self, body: &Value, by: By) -> Result<(Run, QuestionState, Answer), String> {
        answer_in(&self.home, body, by)
    }

    fn effects(&self, body: &Value, by: By) -> Result<Value, String> {
        let (run, q, answer) = self.answer_from(body, by)?;
        let fx = drive::effects(&self.home, &run, &q.question, &answer)?;
        let refused = decide(Some(&run), &Input::Answer { answer }).err().map(|r| r.0);
        Ok(json!({"ok": refused.is_none(), "sentences": fx.sentences, "refused": refused}))
    }
}

/// The answer a body from the page (or the app, which sends the same) means, with the run and the question it is about.
pub fn answer_in(home: &std::path::Path, body: &Value, by: By) -> Result<(Run, QuestionState, Answer), String> {
        let run_id = body["run"].as_str().ok_or("which run?")?;
        let qid = body["question"].as_str().ok_or("which question?")?;
        let run = Store::open(&home.join("record.sqlite3"))?.state(run_id)?.ok_or("no such run")?;
        let q = run.questions.get(qid).cloned().ok_or("no such question")?;
        let mut verdicts = BTreeMap::new();
        if let Some(m) = body["verdicts"].as_object() {
            for (item, v) in m {
                let note = body["notes"][item].as_str().unwrap_or("").to_string();
                let verdict = match v.as_str().unwrap_or("keep") {
                    "keep" => Verdict::Keep,
                    "skip" => Verdict::Skip,
                    "drop" => Verdict::Drop,
                    "change" => Verdict::Change { text: note },
                    "redo" => Verdict::Redo { note },
                    other => return Err(format!("{other} is not a verdict")),
                };
                verdicts.insert(item.clone(), verdict);
            }
        }
        let shown = jaynes_core::Digest(body["shown"].as_str().unwrap_or("").to_string());
        let context = body["context"].as_str().unwrap_or("").to_string();
        let back: BTreeMap<String, String> = body["back"]
            .as_object()
            .map(|m| m.iter().map(|(id, w)| (id.clone(), w.as_str().unwrap_or("").to_string())).collect())
            .unwrap_or_default();
        let answer = Answer { back, question: q.question.id.clone(), shown, verdicts, by, context };
        Ok((run, q, answer))
}

/// Send a JSON reply, all of it by `until`, and close.
fn reply_json(stream: &TcpStream, until: std::time::Instant, status: &str, said: &Value) -> Result<(), String> {
    let body = said.to_string();
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Frame-Options: DENY\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    );
    send(stream, until, &[head.as_bytes(), body.as_bytes()])
}

/// The person's words read into the answer they mean, with what Jaynes says it will do: a proposal, never an answer.
pub fn understand_in(home: &std::path::Path, body: &Value, by: By) -> Result<Value, String> {
    let run_id = body["run"].as_str().ok_or("which run?")?;
    let qid = body["question"].as_str().ok_or("which question?")?;
    let store = Store::open(&home.join("record.sqlite3"))?;
    let run = store.state(run_id)?.ok_or("no such run")?;
    let q = run.questions.get(qid).cloned().ok_or("no such question")?;
    let text = body["text"].as_str().unwrap_or("").trim().to_string();
    if text.is_empty() {
        return Err("write what you think first".into());
    }
    let detail = |it: &jaynes_core::question::Item| {
        let d = store.display(&it.digest);
        let mut parts: Vec<String> = d["detail"].as_str().filter(|t| !t.is_empty()).map(str::to_string).into_iter().collect();
        parts.extend(d["aspects"].as_array().into_iter().flatten().map(|a| format!("{}: {}", a["label"].as_str().unwrap_or(""), a["text"].as_str().unwrap_or(""))));
        parts.join("; ")
    };
    let prompt = crate::understand::prompt(&run, &q.question, &detail, &text, body["pressed"].as_str());
    let read = crate::understand::read(&run.agent, &prompt)?;
    let proposal = crate::understand::proposal(&q.question, &read, &text);
    let mut sent = proposal.clone();
    for k in ["run", "question", "shown"] {
        sent[k] = body[k].clone();
    }
    let (run, q, answer) = answer_in(home, &sent, by)?;
    let fx = drive::effects(home, &run, &q.question, &answer)?;
    let refused = decide(Some(&run), &Input::Answer { answer }).err().map(|r| r.0);
    Ok(json!({"ok": refused.is_none(), "body": sent, "sentences": fx.sentences, "refused": refused, "unsure": proposal["unsure"], "repeats": proposal["repeats"]}))
}

impl Page {
    fn answer(&self, body: &Value, by: By) -> Result<Value, String> {
        let (run, _, answer) = self.answer_from(body, by)?;
        self.submit(&run, Input::Answer { answer })
    }

    /// What the person does to a run from its page: halt, resume, continue, raise, abandon, correct.
    /// Change the guard, update Jaynes, or restart the page; only the person does these.
    fn setup(&self, body: &Value, by: By) -> Result<Value, String> {
        use crate::install::{guard, guard_notes_path, set_guard, set_guard_watch};
        if by != By::Person {
            return Err("only you change the setup: open the link the page printed".into());
        }
        let (on, mut owners) = guard();
        let owner = body["owner"].as_str().unwrap_or("").trim().to_string();
        match body["action"].as_str().unwrap_or("") {
            "guard-on" => set_guard_watch(false)?,
            "guard-watch" => set_guard_watch(true)?,
            "guard-off" => set_guard(false, &owners)?,
            "watch-clear" => match std::fs::remove_file(guard_notes_path()?) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("the notes stay: {e}")),
                _ => {}
            },
            "owner-add" => {
                if !owner_shape(&owner) {
                    return Err("a GitHub owner is letters, digits and dashes".into());
                }
                if !owners.iter().any(|o| o.eq_ignore_ascii_case(&owner)) {
                    owners.push(owner);
                }
                set_guard(on, &owners)?
            }
            "owner-remove" => {
                owners.retain(|o| !o.eq_ignore_ascii_case(&owner));
                set_guard(on, &owners)?
            }
            "update" => {
                if UPDATING.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    return Err("an update is already building".into());
                }
                let log = std::fs::File::create(self.home.join("update.log")).map_err(|e| e.to_string())?;
                let err = log.try_clone().map_err(|e| e.to_string())?;
                let mut child = std::process::Command::new(&self.exe)
                    .arg("update")
                    .stdin(std::process::Stdio::null())
                    .stdout(log)
                    .stderr(err)
                    .spawn()
                    .map_err(|e| format!("the update could not start: {e}"))?;
                std::thread::spawn(move || {
                    let _done = child.wait();
                    UPDATING.store(false, std::sync::atomic::Ordering::SeqCst);
                });
            }
            "restart" => {
                use std::os::unix::process::CommandExt;
                let (exe, port) = (self.exe.clone(), self.port);
                // after this answer reaches the page: the new program takes this process's place
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    let failed = std::process::Command::new(exe).args(["page", "--port", &port.to_string()]).exec();
                    say(&format!("the page could not restart: {failed}"));
                });
            }
            other => return Err(format!("no setup action {other}")),
        }
        Ok(json!({"ok": true}))
    }

    /// Start a run of a role on a project; only the person does it here. An agent starts one with the start tool.
    fn start(&self, body: &Value, by: By) -> Result<Value, String> {
        if by != By::Person {
            return Err("only you start a run from the page: open the link the page printed".into());
        }
        let text = |k: &str| body[k].as_str().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| format!("{k} is needed"));
        let (role, project, request) = (text("role")?, text("project")?, text("request")?);
        let project = project.strip_prefix("~/").map(|rest| std::env::var("HOME").map(|h| format!("{h}/{rest}")).unwrap_or_else(|_| project.to_string())).unwrap_or_else(|| project.to_string());
        if !std::path::Path::new(&project).join(".git").exists() {
            return Err(format!("{project} is not a git repository"));
        }
        let run = drive::launch(&self.home, &self.exe, role, &project, request, body["risk"].as_str() == Some("high"))?;
        Ok(json!({"ok": true, "run": run}))
    }

    fn act(&self, body: &Value, by: By) -> Result<Value, String> {
        act_in(&self.home, &self.exe, body, by)
    }

    /// Record an input now and carry the run on in a process of its own, or leave it in the inbox of the process
    /// that drives the run.
    fn submit(&self, run: &Run, input: Input) -> Result<Value, String> {
        submit_in(&self.home, &self.exe, run, input)
    }
}

fn submit_in(home: &std::path::Path, exe: &std::path::Path, run: &Run, input: Input) -> Result<Value, String> {
    let recorded = drive::submit(home, exe, run, input)?;
    Ok(json!({"ok": true, "recorded": recorded}))
}

/// What the person does to a run from its page, from a body as the page's /api/act takes it (the app sends the
/// same): halt, resume, continue, raise the risk or the ceiling, abandon, undo, correct. `exe` is the program that
/// carries the run on; what `by` may do the engine decides, as for the page.
pub fn act_in(home: &std::path::Path, exe: &std::path::Path, body: &Value, by: By) -> Result<Value, String> {
        let run_id = body["run"].as_str().ok_or("which run?")?;
        let run = Store::open(&home.join("record.sqlite3"))?.state(run_id)?.ok_or("no such run")?;
        let input = match body["action"].as_str().unwrap_or("") {
            "halt" => Input::Halt { by },
            "resume" => Input::Resume { by },
            "continue" => Input::Continue { by },
            "abandon" => Input::Abandon { by },
            "raise-risk" => Input::RaiseRisk { to: jaynes_core::risk::Risk::High, by },
            "raise-ceiling" => {
                let c = run.ceiling;
                Input::RaiseCeiling { ceiling: jaynes_core::risk::Ceiling { seconds: c.seconds + 1800, tokens: c.tokens + 2_000_000 }, by }
            }
            "undo" => {
                let mut store = Store::open(&home.join("record.sqlite3"))?;
                let reverted = drive::undo(&mut store, home, run_id, body["note"].as_str().unwrap_or("").to_string(), by)?;
                return Ok(json!({"ok": true, "recorded": reverted}));
            }
            "correct" => Input::Correct {
                note: body["note"].as_str().unwrap_or("").to_string(),
                by,
                step: body["step"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
            },
            other => return Err(format!("no action {other}")),
        };
        submit_in(home, exe, &run, input)
}

impl Page {
    fn render(&self, view: View, now: u64) -> Result<String, String> {
        let store = self.store()?;
        let mut waiting: Vec<(Run, QuestionState)> = Vec::new();
        let mut released: Vec<(Run, QuestionState)> = Vec::new();
        let mut runs: Vec<Run> = Vec::new();
        for id in store.runs()? {
            let Some(run) = store.state(&id)? else { continue };
            runs.push(run.clone());
            if run.finished() {
                continue;
            }
            for q in run.questions.values() {
                if q.open() {
                    waiting.push((run.clone(), q.clone()));
                } else if !q.superseded && q.answer.as_ref().is_some_and(|a| matches!(a.by, By::Agent { .. })) {
                    let i = run.manifest.index(&q.question.step).unwrap_or(0);
                    if run.steps[i].question.as_deref() == Some(q.question.id.as_str()) {
                        released.push((run.clone(), q.clone()));
                    }
                }
            }
        }
        let chosen = match &view {
            View::Question(r, q) => Some((r.clone(), q.clone())),
            _ => None,
        };
        let current = if matches!(view, View::Run(_) | View::Overview | View::New | View::Setup) {
            None
        } else {
            chosen
                .and_then(|(r, q)| waiting.iter().chain(released.iter()).find(|(run, s)| run.id == r && s.question.id == q).cloned())
                .or_else(|| waiting.first().cloned())
                .or_else(|| released.first().cloned())
        };
        let side = |list: &[(Run, QuestionState)], released: bool| -> String {
            list.iter()
                .map(|(run, q)| {
                    let on = current.as_ref().is_some_and(|(r, c)| r.id == run.id && c.question.id == q.question.id);
                    let status = if released {
                        "an agent let it go on · you can change it".to_string()
                    } else if q.question.holds {
                        "<span class=\"holds\">the run waits here</span>".to_string()
                    } else {
                        "<span class=\"goes\">the run goes on</span>".to_string()
                    };
                    format!(
                        "<a class=\"q{}\" href=\"/q/{}/{}\">{}<small>{} · {} · {}</small>{}</a>",
                        if on { " on" } else { "" },
                        esc(&run.id),
                        esc(&q.question.id),
                        esc(&title(q)),
                        esc(&run.manifest.role),
                        where_in(run),
                        status,
                        strip(run)
                    )
                })
                .collect()
        };
        let mut aside = format!("<div class=\"qh\">Waiting for you · {}</div>{}", waiting.len(), side(&waiting, false));
        if !released.is_empty() {
            aside.push_str(&format!("<div class=\"qh\" style=\"margin-top:18px\">Let through by an agent · {}</div>{}", released.len(), side(&released, true)));
        }
        let open: Vec<&Run> = runs.iter().rev().filter(|r| !r.finished()).collect();
        let done: Vec<&Run> = runs.iter().rev().filter(|r| r.finished()).take(8).collect();
        let shown_run = if let View::Run(id) = &view { Some(id.clone()) } else { None };
        let run_link = |r: &&Run| {
            let on = shown_run.as_deref() == Some(r.id.as_str());
            format!("<a class=\"q{}\" href=\"/run/{}\">{}<small>{} · {} · {}</small>{}</a>", if on { " on" } else { "" }, esc(&r.id), esc(&short(&r.request)), esc(&r.manifest.role), where_in(r), phase_word(r), strip(r))
        };
        aside.push_str(&format!("<div class=\"qh\" style=\"margin-top:18px\">Runs · {}</div>{}", open.len(), open.iter().map(run_link).collect::<String>()));
        if !done.is_empty() {
            aside.push_str(&format!("<div class=\"qh\" style=\"margin-top:18px\">Finished</div>{}", done.iter().map(run_link).collect::<String>()));
        }
        // at a pause of the person's (the page opened, or the page they land on after an answer), the questions they
        // were told of a day ago or more and that nobody answered
        let remind = if matches!(view, View::Home | View::Overview) {
            let on = current.as_ref().map(|(r, q)| (r.id.as_str(), q.question.id.as_str()));
            reminders_html(&drive::reminders_due(&self.home, &store, now).unwrap_or_default(), &runs, on)
        } else {
            String::new()
        };
        let main = match (&view, &current) {
            (View::Run(id), _) => match runs.iter().find(|r| &r.id == id) {
                Some(r) => run_html(r, &changes_html(&self.home, r, None), &format!("{}{}", refused_html(&self.home, r), summary_html(&self.home, r))),
                None => match std::fs::read_to_string(self.home.join(format!("start-{id}.log"))) {
                    Ok(said) if said.trim().is_empty() => "<div class=\"empty\">The run is starting: its role is read and the project copied.</div>".to_string(),
                    Ok(said) => format!("<div class=\"empty\">The run is not recorded yet. What starting it said:</div><pre class=\"said\">{}</pre>", esc(said.trim())),
                    Err(_) => "<div class=\"empty\">There is no such run.</div>".to_string(),
                },
            },
            (View::New, _) => {
                let last = runs.iter().rev().find_map(|r| store.setup(&r.id).ok().and_then(|s| s["project"].as_str().map(|p| (r.manifest.role.clone(), p.to_string()))));
                new_html(&crate::install::list(&self.home).into_iter().map(|(n, _, a, _)| (n, a)).collect::<Vec<_>>(), last)
            }
            (View::Setup, _) => setup_html(&self.home),
            (_, None) => home_html(&runs, &store.times()?, waiting.len(), &self.home, &remind),
            (_, Some((run, q))) => {
                let shown = |it: &jaynes_core::question::Item| -> String {
                    let d = store.display(&it.digest);
                    let mut html = String::new();
                    if let Some(t) = d["detail"].as_str().filter(|t| !t.is_empty()) {
                        html.push_str(&format!("<p class=\"detail\">{}</p>", esc(t)));
                    }
                    // the parts the person judges apart, each with its label
                    let parts: String = d["aspects"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|a| format!("<dt>{}</dt><dd>{}</dd>", esc(a["label"].as_str().unwrap_or("")), esc(a["text"].as_str().unwrap_or(""))))
                        .collect();
                    if !parts.is_empty() {
                        html.push_str(&format!("<dl class=\"aspects\">{parts}</dl>"));
                    }
                    let pics: Vec<String> = d["pictures"].as_array().into_iter().flatten().filter_map(|p| p.as_str())
                        .filter_map(|name| {
                            let img = picture(&self.home, name)?;
                            // a photographed page opens itself, in a tab of its own
                            Some(match d["pages"][name].as_str().filter(|p| kept_page(p)) {
                                Some(page) => format!("<a class=\"open\" href=\"/page/{page}\" target=\"_blank\" rel=\"noopener\" title=\"Open the page\">{img}</a>"),
                                None => img,
                            })
                        })
                        .collect();
                    if !pics.is_empty() {
                        html.push_str(&format!("<div class=\"pics\">{}</div>", pics.join("")));
                    }
                    html
                };
                let i = run.manifest.index(&q.question.step);
                remind.clone() + &question_html(run, q, &shown, &changes_html(&self.home, run, i))
            }
        };
        // whether the person holds the key, the page's script asks, and says so here when they do not
        Ok(PAGE.replace("{{aside}}", &aside).replace("{{main}}", &main).replace("{{who}}", &format!("{}<div id=\"nokey\"></div>", banner())))
    }
}

/// The run's steps as a strip, one mark per step of its path, coloured by where each is, as the earlier page drew
/// it: done, waiting for you, blocked, running, halted (under way in a run the person halted), not yet, or asked only
/// a sample and skipped.
fn strip(r: &Run) -> String {
    use jaynes_core::run::Status;
    let blocked = match &r.phase {
        Phase::Blocked { step: Some(s), .. } => Some(s.as_str()),
        _ => None,
    };
    let marks: String = r
        .path()
        .into_iter()
        .map(|i| {
            let d = &r.manifest.steps[i];
            let s = &r.steps[i];
            let state = if blocked == Some(d.name.as_str()) {
                "blocked"
            } else {
                match &s.status {
                    Status::Running { .. } | Status::Checking { .. } | Status::Probing if !r.finished() && r.halted => "halted",
                    Status::Running { .. } | Status::Checking { .. } | Status::Probing if !r.finished() => "running",
                    Status::AwaitingInput { .. } => "waiting",
                    Status::Done if r.fresh(i) && r.open_question(i).is_some() => "waiting",
                    Status::Done if r.fresh(i) && s.skipped == Some(s.round) => "sampled",
                    Status::Done if r.fresh(i) => "",
                    _ if matches!(&r.phase, Phase::Waiting { step: Some(w), .. } if *w == d.name) => "waiting",
                    _ => "todo",
                }
            };
            format!("<i class=\"{state}\" title=\"{}\"></i>", esc(&d.name))
        })
        .collect();
    format!("<div class=\"strip\">{marks}</div>")
}

/// Who does a step and whether it asks the person, in a few words, as the earlier page said it.
fn who(d: &jaynes_core::manifest::StepDecl, r: &Run) -> String {
    use jaynes_core::manifest::Executor;
    let by = match d.executor {
        Executor::Agent => "agent",
        Executor::Role => "Jaynes",
        Executor::Person => "",
    };
    let asks = match &d.stop {
        Some(s) if s.from_risk > r.risk => "",
        Some(s) if s.release => "you or an agent",
        Some(s) if s.budget.is_some() => "sometimes you",
        Some(_) => "you",
        None => "",
    };
    [by, asks].iter().filter(|x| !x.is_empty()).copied().collect::<Vec<_>>().join(", then ")
}

/// The run's steps top to bottom: who does each, what the role says it does, and where it is, as the earlier
/// page showed them.
fn stage_list(r: &Run) -> String {
    let rows: String = crate::listing::stages(r)
        .into_iter()
        .map(|stage| {
            let (i, class, detail) = (stage.index, stage.class, stage.detail);
            let (d, s) = (&r.manifest.steps[i], &r.steps[i]);
            let mark = who(d, r);
            let mark = if mark.is_empty() { String::new() } else { format!("<span class=\"who\">{}</span>", esc(&mark)) };
            let about = if d.about.is_empty() { String::new() } else { format!("<p class=\"about\">{}</p>", esc(&d.about)) };
            let detail = if detail.is_empty() { String::new() } else { format!("<small>{}</small>", esc(&detail)) };
            // what the checks said, with where their answer comes from: the person's eyes only
            let checks: Vec<String> = s
                .checks
                .iter()
                .map(|c| {
                    let kind = match c.kind {
                        jaynes_core::manifest::CheckKind::Open => "open",
                        jaynes_core::manifest::CheckKind::Hidden => "hidden",
                    };
                    let said = if c.blocked { "could not judge" } else if c.passed { "passed" } else { "failed" };
                    let read = if c.read.is_empty() { String::new() } else { format!(", read {}", c.read.join(", ")) };
                    format!("{} ({kind}{read}) {said}", c.check)
                })
                .collect();
            let detail = if checks.is_empty() { detail } else { format!("{detail}<small class=\"checks\">checks: {}</small>", esc(&checks.join(" · "))) };
            format!("<li class=\"{class}\"><span class=\"dot\"></span><div><div class=\"r\"><b>{}{mark}</b></div>{about}{detail}</div></li>", esc(&d.name))
        })
        .collect();
    format!("<ul class=\"stages told\">{rows}</ul>")
}

/// Where a run is, in words: the step it is on, of how many.
enum View {
    Home,
    Overview,
    New,
    Setup,
    Question(String, String),
    Run(String),
}

fn phase_word(r: &Run) -> String {
    if r.halted && !r.finished() {
        return "<span class=\"holds\">halted</span>".into();
    }
    match &r.phase {
        Phase::Running => "<span class=\"goes\">running</span>".into(),
        Phase::Waiting { step: None, .. } => "<span class=\"goes\">delivered, answers open</span>".into(),
        Phase::Waiting { .. } => "<span class=\"holds\">waits for you</span>".into(),
        Phase::Blocked { .. } => "<span class=\"bad\">blocked</span>".into(),
        Phase::Finished { outcome } => format!("{outcome:?}").to_lowercase(),
    }
}

/// What the page refused on a run, newest last: a trace beside the record, shown so the person sees who tried.
fn refused_html(home: &std::path::Path, r: &Run) -> String {
    let all = crate::refused::of_run(home, &r.id);
    if all.is_empty() {
        return String::new();
    }
    let shown: String = all.iter().rev().take(10).rev().map(|t| format!("<li>{}</li>", esc(&t.line()))).collect();
    let more = if all.len() > 10 { format!("<li>and {} earlier</li>", all.len() - 10) } else { String::new() };
    format!(
        "<div class=\"notice bad\">{} attempt{} on this run {} refused. Kept beside the record, outside its chain: a trace, not evidence.<ul>{shown}{more}</ul></div>",
        all.len(),
        if all.len() == 1 { "" } else { "s" },
        if all.len() == 1 { "was" } else { "were" }
    )
}

/// What a finished run did, as a text to copy into a merge request or a note. A run not finished has none.
fn summary_html(home: &std::path::Path, r: &Run) -> String {
    if !r.finished() {
        return String::new();
    }
    match crate::listing::summary(home, &r.id) {
        Ok(told) => format!(
            "<section class=\"summary\"><div class=\"sumhead\"><h2>Summary</h2><button class=\"copy\" id=\"copysum\">Copy as Markdown</button></div>\
             <p class=\"sub\">What this run did, for someone who was not here: paste it in a merge request or a note.</p><pre class=\"said\" id=\"sumtext\">{}</pre></section>",
            esc(&told.markdown())
        ),
        Err(e) => format!("<div class=\"notice bad\">The summary could not be written: {}</div>", esc(&e)),
    }
}

/// A run: where it is, what it cost, its steps, what the person may do to it, and, once finished, its summary.
fn run_html(r: &Run, changes: &str, summary: &str) -> String {
    let path = r.path();
    let steps = stage_list(r);
    let phase = match &r.phase {
        Phase::Blocked { reason, fault, .. } => format!("<div class=\"notice bad\">Blocked ({}): {}</div>", format!("{fault:?}").to_lowercase(), esc(reason)),
        Phase::Waiting { why, .. } => format!("<div class=\"notice\">{}</div>", esc(why)),
        Phase::Finished { outcome } => format!("<div class=\"notice ok\">Finished: {}</div>", format!("{outcome:?}").to_lowercase()),
        Phase::Running => String::new(),
    };
    // what waits for the person comes first, as a way to its question, so the run's controls are not taken for it
    let answers: String = r
        .questions
        .values()
        .filter(|q| q.open())
        .map(|q| format!("<a class=\"answer\" href=\"/q/{}/{}\">Answer: {}</a>", esc(&r.id), esc(&q.question.id), esc(&q.question.about)))
        .collect();
    let finished = r.finished();
    let mut buttons = String::new();
    let mut button = |act: &str, text: &str, cls: &str| buttons.push_str(&format!("<button class=\"act {cls}\" data-act=\"{act}\">{text}</button>"));
    if !finished {
        // each control only when it can do something: halting needs something at work, raising the ceiling a run
        // stopped by it or near it (four fifths of its time or tokens)
        if r.halted {
            button("resume", "Resume", "");
        } else if matches!(r.phase, Phase::Running) {
            button("halt", "Halt", "");
        }
        if r.blocked() {
            button("continue", "Continue", "");
        }
        if r.risk == jaynes_core::risk::Risk::Low {
            button("raise-risk", "Raise the risk", "");
        }
        let near = r.spent.seconds * 5 >= r.ceiling.seconds * 4 || r.spent.tokens * 5 >= r.ceiling.tokens * 4;
        if near || matches!(r.phase, Phase::Blocked { fault: jaynes_core::event::Fault::Ceiling, .. }) {
            button("raise-ceiling", "Raise the ceiling by 30 min", "");
        }
        button("abandon", "Abandon", "danger");
    }
    if r.manifest.delivery == jaynes_core::manifest::DeliveryKind::Merge && r.delivered.is_some() && !r.undone {
        button("undo", "Undo the merge", "danger");
    }
    let correct = if r.delivered.is_some() {
        let options: String = std::iter::once("<option value=\"\">the step that erred, if you know it</option>".to_string())
            .chain(path.iter().map(|&i| format!("<option>{}</option>", esc(&r.manifest.steps[i].name))))
            .collect();
        format!(
            "<div class=\"say\" style=\"margin-top:18px\"><label>Correct</label><input id=\"cnote\" placeholder=\"What was wrong in the delivery\"><select id=\"cstep\">{options}</select><button class=\"act\" data-act=\"correct\">Record</button></div>"
        )
    } else {
        String::new()
    };
    let data = json!({"run": r.id}).to_string();
    format!(
        "<div class=\"where\">{role} {version} · run {id} · {risk} risk</div><h1>{request}</h1>{strip}{phase}{answers}\
         <p class=\"sub\">Spent {sm} of {cm} min, {st} of {ct} tokens ({cached} read from the cache) · {att} attempts, {spent} spent by the agent, {fixes} fixes, {back} sent back{corr}</p>\
         {steps}\
         <div class=\"acts\">{buttons}</div>{correct}<div class=\"refused\" id=\"actmsg\"></div>{summary}{changes}<script id=\"rundata\" type=\"application/json\">{data}</script>",
        role = esc(&r.manifest.role),
        version = esc(&r.manifest.version),
        id = esc(&r.id),
        risk = r.risk.word(),
        request = esc(&short(&r.request)),
        strip = strip(r),
        sm = r.spent.seconds / 60,
        cm = r.ceiling.seconds / 60,
        st = r.spent.tokens,
        ct = r.ceiling.tokens,
        cached = r.spent.cached,
        att = r.counts.attempts,
        spent = r.counts.spent,
        fixes = r.counts.fixes,
        back = r.counts.sent_back,
        corr = if r.counts.corrections > 0 { format!(", {} corrected after delivery", r.counts.corrections) } else { String::new() },
        data = data.replace('<', "\\u003c"),
    )
}

/// The name of a page Jaynes kept beside its photograph: its fingerprint, and nothing that could reach another file.
fn kept_page(name: &str) -> bool {
    // by bytes: a name from outside need not cut at a letter's edge
    let b = name.as_bytes();
    b.len() == 69 && b[..64].iter().all(u8::is_ascii_hexdigit) && &b[64..] == b".html"
}

/// A picture Jaynes kept, by its fingerprint, as an image inside the page: no address reaches a file.
fn picture(home: &std::path::Path, name: &str) -> Option<String> {
    // the name comes from what a role kept: its first 64 bytes are checked as bytes, and only then is it cut there
    let ok = name.len() > 65
        && name.as_bytes()[..64].iter().all(u8::is_ascii_hexdigit)
        && matches!(&name[64..], ".png" | ".jpg" | ".jpeg" | ".gif" | ".webp" | ".svg");
    if !ok {
        return None;
    }
    let bytes = std::fs::read(home.join("pictures").join(name)).ok()?;
    let kind = match &name[65..] {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "image/jpeg",
    };
    Some(format!("<img src=\"data:{kind};base64,{}\" alt=\"\">", base64(&bytes)))
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn question_html(run: &Run, q: &QuestionState, shown: &dyn Fn(&jaynes_core::question::Item) -> String, changes: &str) -> String {
    let question = &q.question;
    let i = run.manifest.index(&question.step).unwrap_or(0);
    let path = run.path();
    let at = path.iter().position(|&j| j == i).map(|n| n + 1).unwrap_or(0);
    // rounds the person opens have no ceiling: the round and what the run spent so far, for the person to decide
    // when to stop
    let rounds = if question.pass > 0 {
        format!(
            "<span class=\"badge\">● Round {} · {} min and {} tokens spent so far</span> ",
            question.pass,
            run.spent.seconds / 60,
            tokens(run.spent.tokens)
        )
    } else {
        String::new()
    };
    let badge = if question.holds {
        "<span class=\"badge wait\">● The run waits for your answer</span>"
    } else {
        "<span class=\"badge\">● The run goes on while you look</span>"
    };
    let phase = match &run.phase {
        Phase::Blocked { reason, .. } => format!("<div class=\"notice\">The run is blocked: {}</div>", esc(reason)),
        _ => String::new(),
    };
    // a stop that chooses: one click on an option sends the choice; a note beside it adjusts the chosen one
    let choose_rows = || -> String {
        let rows = question
            .items
            .iter()
            .map(|it| {
                let more_ways = format!("<input class=\"adjust\" data-i=\"{id}\" placeholder=\"Adjust it (optional): what to change, what to take from the others\"><button class=\"fin\" data-fin=\"{id}\" title=\"Keep it among two or three finalists that the agent develops, then choose again\">Finalist</button>", id = esc(&it.id));
                format!(
                    "<div class=\"row choose\" data-item=\"{id}\"><h3>{label}</h3>{more}<div class=\"pick\">{more_ways}<button class=\"choose\" data-choose=\"{id}\">Choose</button></div><ul class=\"fx-row\" data-i=\"{id}\"></ul></div>",
                    id = esc(&it.id),
                    label = esc(&it.label),
                    more = shown(it)
                )
            })
            .collect::<String>();
        rows + "<div class=\"row choose\" id=\"finals\" hidden><div class=\"pick\"><span class=\"finsay\"></span><button class=\"choose\" id=\"develop\">Develop the finalists</button></div><ul class=\"fx-row\" id=\"fx-finals\"></ul></div>\
               <div class=\"row choose\"><h3>None of these</h3><div class=\"pick\"><input class=\"adjust\" id=\"why\" placeholder=\"What was missing? The agent makes new options; these do not come back\"><button class=\"choose\" id=\"none\" disabled>None of these</button></div><ul class=\"fx-row\" id=\"fx-none\"></ul></div>"
    };
    let rows: String = if question.choose { choose_rows() } else { question
        .items
        .iter()
        .map(|it| {
            let buttons: String = question
                .offered
                .iter()
                .map(|v| {
                    let key = format!("{v:?}").to_lowercase();
                    format!("<button class=\"{key}\" data-i=\"{}\" data-v=\"{key}\">{}</button>", esc(&it.id), label_for(*v, question.kind))
                })
                .collect();
            format!(
                "<div class=\"row\" data-item=\"{id}\"><h3>{label}</h3>{more}<div class=\"seg\">{buttons}</div><input class=\"note hide\" data-i=\"{id}\"></div>",
                id = esc(&it.id),
                label = esc(&it.label),
                more = shown(it)
            )
        })
        .collect() };
    // what the person dropped in earlier rounds stays in view, folded: only they bring it back
    let rows = if question.back.is_empty() { rows } else {
        let earlier: String = question
            .back
            .iter()
            .map(|it| {
                let id = esc(&it.id);
                let words = format!("<input class=\"adjust bring\" data-b=\"{id}\" placeholder=\"How it comes back (optional): as it was, or what to change\">");
                let control = if question.choose {
                    format!("<div class=\"pick\">{words}<button class=\"choose\" id=\"bring-{id}\" data-back=\"{id}\">Bring back</button></div><ul class=\"fx-row\" id=\"fx-b-{id}\"></ul>")
                } else {
                    format!("<div class=\"seg\"><button class=\"bringt\" data-back=\"{id}\">Bring back</button></div>{words}")
                };
                format!("<div class=\"row{c}\" data-back-item=\"{id}\"><h3>{label}</h3>{more}{control}</div>", c = if question.choose { " choose" } else { "" }, label = esc(&it.label), more = shown(it))
            })
            .collect();
        format!(
            "{rows}<details class=\"earlier\"><summary>Dropped in earlier rounds · {}: you can bring one back</summary>{earlier}</details>",
            question.back.len()
        )
    };
    let items: Vec<&str> = question.items.iter().map(|x| x.id.as_str()).collect();
    let earlier: Vec<&str> = question.back.iter().map(|x| x.id.as_str()).collect();
    let data = json!({"run": run.id, "question": question.id, "shown": question.shown().0, "choose": question.choose, "items": items, "earlier": earlier}).to_string();
    format!(
        "<div class=\"where\"><a href=\"/run/{run_id}\">{role} · run {run_id}</a> · step {step} · step {at} of {n}</div><h1>{title}</h1>{strip}<p class=\"sub\">{rounds}{badge}</p>{phase}\
         <div class=\"say\"><label>Your words</label><input id=\"ctx\" placeholder=\"Say what you think, in any language: what stays, what changes (or answer with the buttons)\"><button class=\"read\" id=\"read\">Read my words</button></div><div class=\"understood\" id=\"understood\" hidden></div>\
         {rows}{changes}<details class=\"way\"><summary>Steps</summary>{stages}</details><script id=\"data\" type=\"application/json\">{data}</script>",
        role = esc(&run.manifest.role),
        run_id = esc(&run.id),
        step = esc(&question.step),
        n = path.len(),
        title = esc(&title(q)),
        strip = strip(run),
        stages = stage_list(run),
        data = data.replace('<', "\\u003c"),
    )
}

/// What a step changed in the project, as git shows it between where the step started and the commit its output
/// became: one step's, or every step's of the run when `only` is None. Nothing when the run has no project.
fn changes_html(home: &std::path::Path, run: &Run, only: Option<usize>) -> String {
    run.manifest
        .steps
        .iter()
        .enumerate()
        .filter(|(i, _)| only.is_none_or(|o| o == *i))
        .filter_map(|(_, d)| {
            let mut text = drive::step_diff(home, &run.id, &d.name)?;
            if text.trim().is_empty() {
                return None;
            }
            let n = text.lines().filter(|l| l.starts_with("diff --git ")).count();
            // never cut silently: what is not shown here must still be read
            if text.len() > 40_000 {
                let cut = (0..=40_000).rev().find(|&n| text.is_char_boundary(n)).unwrap_or(0);
                let more = text.len() - cut;
                text.truncate(cut);
                text.push_str(&format!("\n[{more} more bytes not shown: read the branch before it lands]"));
            }
            Some(format!(
                "<div class=\"changes\"><div class=\"qh\">What {} changed · {n} file{}</div><pre class=\"diff\">{}</pre></div>",
                esc(&d.name),
                if n == 1 { "" } else { "s" },
                diff_html(&text)
            ))
        })
        .collect()
}

/// A unified diff, escaped: each file's header set apart, added lines green, removed lines red.
fn diff_html(text: &str) -> String {
    text.lines()
        .map(|l| {
            let (class, mark, rest) = if l.starts_with("diff --git ") {
                ("file", "", l.trim_start_matches("diff --git ").split(" b/").last().unwrap_or(l))
            } else if l.starts_with("@@") {
                ("hunk", "", l)
            } else if l.starts_with("+++") || l.starts_with("---") || l.starts_with("index ") || l.starts_with("new file") || l.starts_with("deleted file") || l.starts_with("similarity") || l.starts_with("rename ") || l.starts_with('\\') {
                ("meta", "", l)
            } else if let Some(r) = l.strip_prefix('+') {
                ("add", "+", r)
            } else if let Some(r) = l.strip_prefix('-') {
                ("del", "-", r)
            } else {
                ("ctx", " ", l.strip_prefix(' ').unwrap_or(l))
            };
            if class == "meta" {
                return String::new();
            }
            let mark = if mark.is_empty() { String::new() } else { format!("<i>{mark}</i>") };
            format!("<span class=\"{class}\">{mark}{}</span>", if rest.is_empty() { " ".to_string() } else { esc(rest) })
        })
        .collect()
}

/// A newer Jaynes: installed since the page started, so the page should restart; or published, so it can be updated.
fn banner() -> String {
    let running = RUNNING.get().cloned().flatten();
    let installed = crate::install::installed_commit();
    let latest = LATEST.lock().ok().and_then(|l| l.clone());
    let (text, act, label) = if UPDATING.load(std::sync::atomic::Ordering::SeqCst) {
        return "<div class=\"notice new\"><span>Jaynes is updating: cargo builds it, which takes a few minutes.</span></div>".to_string();
    } else if let (Some(r), Some(i)) = (&running, &installed)
        && r != i
    {
        (format!("Jaynes was updated to {}.", &i[..7]), "restart", "Restart the page")
    } else if let (Some(l), Some(i)) = (&latest, &installed)
        && l != i
    {
        ("A newer Jaynes is out.".to_string(), "update", "Update")
    } else {
        return String::new();
    };
    format!("<div class=\"notice new\"><span>{text}</span><button class=\"act\" data-setup=\"{act}\">{label}</button></div>")
}

/// Whether a name has the shape of a GitHub owner, as Free owners takes one: letters, digits and dashes.
fn owner_shape(owner: &str) -> bool {
    !owner.is_empty() && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// What the watching guard noted it would have asked, by owner, or by where when it could not tell one, the most
/// first, each owner with a way to free it. The notes are text from outside, since an owner is read from a remote:
/// all of it is escaped, and an owner is offered only in the shape Free owners takes.
fn guard_notes_html(owners: &[String]) -> String {
    let Some(text) = crate::install::guard_notes_path().ok().and_then(|p| std::fs::read_to_string(p).ok()) else {
        return String::new();
    };
    // (owner or where, whether it is an owner, how many of each action, how many in all)
    let mut groups: Vec<(String, bool, std::collections::BTreeMap<String, usize>, usize)> = Vec::new();
    let (mut first, mut notes) = (None::<String>, 0usize);
    for note in text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
        let Some(held) = note["held"].as_array().filter(|h| !h.is_empty()) else { continue };
        notes += 1;
        if first.is_none() {
            first = note["at"].as_str().map(|t| t.chars().take(10).collect());
        }
        for h in held {
            let (key, is_owner) = match h["owner"].as_str() {
                Some(o) => (o.to_string(), true),
                None => (h["where"].as_str().unwrap_or("where it could not tell").to_string(), false),
            };
            let action = h["action"].as_str().unwrap_or("an action").to_string();
            let at = match groups.iter().position(|g| g.1 == is_owner && g.0.eq_ignore_ascii_case(&key)) {
                Some(at) => at,
                None => {
                    groups.push((key, is_owner, std::collections::BTreeMap::new(), 0));
                    groups.len() - 1
                }
            };
            if let Some(g) = groups.get_mut(at) {
                *g.2.entry(action).or_default() += 1;
                g.3 += 1;
            }
        }
    }
    if notes == 0 {
        return String::new();
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.3));
    let rows: String = groups
        .iter()
        .map(|(key, is_owner, actions, n)| {
            let what = actions.iter().map(|(a, k)| format!("{} {k}", esc(a))).collect::<Vec<_>>().join(", ");
            let free = if !is_owner {
                String::new()
            } else if owners.iter().any(|o| o.eq_ignore_ascii_case(key)) {
                " <span class=\"none\">free now</span>".to_string()
            } else if owner_shape(key) {
                format!("<button class=\"act\" data-setup=\"owner-add\" data-owner=\"{}\">Add to free owners</button>", esc(key))
            } else {
                String::new()
            };
            format!("<span class=\"owner\"><code>{}</code> {n}: {what}{free}</span>", esc(key))
        })
        .collect();
    let since = first.map(|d| format!(" since {}", esc(&d))).unwrap_or_default();
    format!(
        "<tr><td>would have asked</td><td>{notes}{since}: {rows}<button class=\"act\" data-setup=\"watch-clear\">Clear</button></td></tr>"
    )
}

/// The guard in Claude Code, and which Jaynes this is.
fn setup_html(home: &std::path::Path) -> String {
    let (on, owners) = crate::install::guard();
    let path = crate::install::guard_path().map(|p| p.display().to_string()).unwrap_or_default();
    let state = if !on {
        "<span class=\"chip red\">off</span> <button class=\"act\" data-setup=\"guard-on\">Turn on</button>"
    } else if crate::install::guard_watching() {
        "<span class=\"chip\">watching</span> <button class=\"act\" data-setup=\"guard-on\">Ask again</button> <button class=\"act\" data-setup=\"guard-off\">Turn off</button>"
    } else {
        "<span class=\"chip green\">on</span> <button class=\"act\" data-setup=\"guard-off\">Turn off</button> <button class=\"act\" data-setup=\"guard-watch\">Only watch</button>"
    };
    let notes = guard_notes_html(&owners);
    let rows: String = owners.iter().map(|o| format!("<span class=\"owner\"><code>{}</code><button class=\"act\" data-setup=\"owner-remove\" data-owner=\"{0}\">Remove</button></span>", esc(o))).collect();
    let none = if owners.is_empty() { "<span class=\"none\">none: every held action asks, wherever it lands</span>" } else { "" };
    let installed = crate::install::installed_commit();
    let version = match &installed {
        Some(c) => format!("<code>{}</code> from {}", esc(&c[..7]), crate::install::SOURCE),
        None => "built from a commit it does not know".to_string(),
    };
    let running = format!("<code>{}</code>", esc(&crate::install::version()));
    let log = std::fs::read_to_string(home.join("update.log")).unwrap_or_default();
    let log = if log.trim().is_empty() { String::new() } else { format!("<details class=\"way\"><summary>What the last update said</summary><pre class=\"said\">{}</pre></details>", esc(log.trim())) };
    format!(
        "<h1>Setup</h1>\
         <section class=\"part\"><h2>Guard</h2><p class=\"sub\">In every Claude Code session with the Jaynes plugin, it asks before what cannot be undone in git and GitHub: push, reset --hard, clean, deleting branches or tags, merging or closing pull requests, and creating or deleting repositories. Everything else passes. On a free owner's repositories, a plain push and a pull request opened or merged pass; what cannot be undone asks always. Watching, it asks none of those that can be taken back and notes them here instead.</p>\
         <table class=\"kv\"><tr><td>state</td><td>{state}</td></tr>{notes}<tr><td>free owners</td><td>{rows}{none}<span class=\"owner\"><input id=\"owner\" placeholder=\"GitHub owner\"><button class=\"act\" data-setup=\"owner-add\">Add</button></span></td></tr><tr><td>settings</td><td><code>{}</code></td></tr></table></section>\
         <section class=\"part\"><h2>Version</h2><table class=\"kv\"><tr><td>running</td><td>{running}</td></tr><tr><td>installed</td><td>{version}</td></tr></table>{log}</section>\
         <div class=\"refused\" id=\"setupmsg\"></div>",
        esc(&path)
    )
}

/// The form that starts a run: a role, a project, the request, and how risky the change is.
fn new_html(roles: &[(String, String)], last: Option<(String, String)>) -> String {
    let (role, project) = last.unwrap_or_default();
    let role = if roles.iter().any(|(n, _)| *n == role) { role } else { roles.first().map(|r| r.0.clone()).unwrap_or_default() };
    let options: String = roles.iter().map(|(n, a)| format!("<option value=\"{}\">{}</option>", esc(n), esc(a))).collect();
    let about: String = roles.iter().map(|(n, a)| format!("<li><b>{}</b> {}</li>", esc(n), esc(a))).collect();
    let none = if roles.is_empty() { "<p class=\"sub\">No role is installed: <code>jaynes add</code> installs one, or give a role's folder.</p>" } else { "" };
    format!(
        "<h1>New run</h1><p class=\"sub\">The run goes on by itself in a copy of the project; what needs you comes to this page.</p>{none}\
         <form id=\"start\" class=\"form\">\
         <label><span class=\"label\">Role</span><input name=\"role\" list=\"roles\" value=\"{role}\" placeholder=\"a role's name, or its folder\" required><datalist id=\"roles\">{options}</datalist></label>\
         <ul class=\"roles\">{about}</ul>\
         <label><span class=\"label\">Project</span><input name=\"project\" value=\"{project}\" placeholder=\"/path/to/a/git/repository\" required></label>\
         <label><span class=\"label\">Request</span><textarea name=\"request\" rows=\"6\" placeholder=\"What to change, as you would tell a colleague\" required></textarea></label>\
         <label><span class=\"label\">Risk</span><select name=\"risk\"><option value=\"low\">low: a small change</option><option value=\"high\">high: asks you more, and allows more time and tokens</option></select></label>\
         <button class=\"send\" type=\"submit\">Start the run</button><div class=\"refused\" id=\"startmsg\"></div></form>",
        role = esc(&role),
        project = esc(&project),
    )
}

/// How long ago, in a word or two.
fn ago(seconds: u64) -> String {
    match seconds {
        0..60 => "just now".to_string(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

/// Home: what waits, what runs, and what was delivered, newest first, the last week in front and the rest folded;
/// the runs that delivered nothing at the end.
fn home_html(runs: &[Run], times: &BTreeMap<String, (u64, u64)>, waiting: usize, home: &std::path::Path, remind: &str) -> String {
    use jaynes_core::event::Outcome;
    let now = crate::store::now();
    let at = |r: &Run| times.get(&r.id).map(|t| t.1).unwrap_or(0);
    let row = |r: &Run, said: String, chip: &str| -> String {
        let c = &r.counts;
        let mut facts = vec![esc(&r.manifest.role)];
        if c.corrections > 0 {
            facts.push(format!("{} correction{}", c.corrections, if c.corrections == 1 { "" } else { "s" }));
        }
        if c.sent_back > 0 {
            facts.push(format!("{} sent back", c.sent_back));
        }
        facts.push(format!("{} min, {} tokens", r.spent.seconds / 60, r.spent.tokens));
        facts.push(ago(now.saturating_sub(at(r))));
        format!(
            "<a class=\"hrow\" href=\"/run/{}\"><div class=\"t\">{}</div><div class=\"hq\">{}</div><div class=\"hm\">{chip}{}</div>{}</a>",
            esc(&r.id),
            esc(&short(&r.request)),
            esc(&said),
            facts.iter().map(|f| format!("<span>{f}</span>")).collect::<String>(),
            strip(r)
        )
    };
    let landed = |r: &Run| -> (String, &'static str) {
        let merges = drive::merges(&home.join("runs").join(&r.id));
        if r.undone {
            ("reverted by you".into(), "<span class=\"chip red\">undone</span>")
        } else if !merges.is_empty() {
            let into = crate::project::start_branch_of(&home.join("runs").join(&r.id).join("project"));
            let into = into.trim();
            (if into.is_empty() { "merged into your branch".into() } else { format!("merged into {into}") }, "<span class=\"chip green\">merged</span>")
        } else {
            ("its result is on the run's page".into(), "<span class=\"chip green\">delivered</span>")
        }
    };
    let mut done: Vec<&Run> = runs.iter().filter(|r| matches!(r.phase, Phase::Finished { outcome: Outcome::Delivered })).collect();
    done.sort_by_key(|r| std::cmp::Reverse(at(r)));
    let week = now.saturating_sub(7 * 86_400);
    let (new, earlier): (Vec<&Run>, Vec<&Run>) = done.into_iter().partition(|r| at(r) >= week);
    let delivered = |list: &[&Run]| list.iter().map(|r| { let (said, chip) = landed(r); row(r, said, chip) }).collect::<String>();
    let mut moving: Vec<&Run> = runs.iter().filter(|r| !r.finished()).collect();
    moving.sort_by_key(|r| std::cmp::Reverse(at(r)));
    let moving: String = moving.iter().map(|r| row(r, format!("{}, {}", where_in(r), phase_word(r)).replace("<span class=\"goes\">", "").replace("<span class=\"holds\">", "").replace("<span class=\"bad\">", "").replace("</span>", ""), "")).collect();
    let mut dropped: Vec<&Run> = runs.iter().filter(|r| matches!(r.phase, Phase::Finished { outcome: Outcome::Abandoned })).collect();
    dropped.sort_by_key(|r| std::cmp::Reverse(at(r)));
    let abandoned = dropped.len();
    let dropped: String = dropped.iter().map(|r| row(r, "abandoned".into(), "<span class=\"chip\">abandoned</span>")).collect();
    let part = |title: &str, sub: &str, rows: &str, none: &str| {
        let body = if rows.is_empty() { format!("<div class=\"none\">{none}</div>") } else { format!("<div class=\"rows\">{rows}</div>") };
        format!("<section class=\"part\"><h2>{title}</h2><p class=\"sub\">{sub}</p>{body}</section>")
    };
    let fold = |title: &str, n: usize, rows: &str| if n == 0 { String::new() } else { format!("<details class=\"fold\"><summary>{title} <span>{n}</span></summary><div class=\"rows\">{rows}</div></details>") };
    let wait = if waiting == 0 { "Nothing waits for you.".to_string() } else { format!("{waiting} question{} wait{} for you, in the list beside.", if waiting == 1 { "" } else { "s" }, if waiting == 1 { "s" } else { "" }) };
    format!(
        "<h1>Home</h1><p class=\"sub\">{wait}</p>{remind}{}{}{}",
        part("Running", "Runs not finished: working, or waiting for you.", &moving, "Nothing runs."),
        part("Delivered", "The last 7 days, newest first. Undo and Correct are on each run's page.", &delivered(&new), "Nothing was delivered this week.") + &fold("Earlier", earlier.len(), &delivered(&earlier)),
        fold("Abandoned", abandoned, &dropped),
    )
}

/// The reminders due, in a section at the top of what the page shows at a pause, each a link to its question. The
/// question on the screen is not listed again. Every one shown is named in `data-due`, which the page's script sends
/// back (`/api/reminded`) once it knows the person holds the key: only then were they reminded.
fn reminders_html(due: &[drive::Reminder], runs: &[Run], on: Option<(&str, &str)>) -> String {
    if due.is_empty() {
        return String::new();
    }
    let rows: String = due
        .iter()
        .filter(|d| on != Some((d.run.as_str(), d.question.as_str())))
        .map(|d| {
            let run = runs.iter().find(|r| r.id == d.run);
            let request = run.map(|r| short(&r.request)).unwrap_or_default();
            let role = run.map(|r| r.manifest.role.clone()).unwrap_or_default();
            let about = if d.about.trim().is_empty() { String::new() } else { format!(": {}", d.about.trim()) };
            format!(
                "<a class=\"hrow\" href=\"/q/{}/{}\"><div class=\"t\">{}</div><div class=\"hq\">at {}{}</div><div class=\"hm\"><span>{}</span></div></a>",
                esc(&d.run), esc(&d.question), esc(&request), esc(&d.step), esc(&about), esc(&role)
            )
        })
        .collect();
    let named: Vec<[&str; 2]> = due.iter().map(|d| [d.run.as_str(), d.question.as_str()]).collect();
    let data = esc(&serde_json::to_string(&named).unwrap_or_default());
    if rows.is_empty() {
        return format!("<div id=\"remind\" data-due=\"{data}\" hidden></div>");
    }
    format!(
        "<section class=\"part remind\" id=\"remind\" data-due=\"{data}\"><h2>Still waiting for you</h2><p class=\"sub\">Asked a day ago or more, and nobody answered.</p><div class=\"rows\">{rows}</div></section>"
    )
}

/// The person read the reminders named in `due` (pairs of run and question), at `now`. Only the person's reading
/// counts, and only a reminder due now is marked: a name the request brings is never a path to write.
pub fn reminded_in(home: &std::path::Path, body: &Value, by: &By, now: u64) -> Result<Value, String> {
    if !by.person() {
        return Err("only the person is reminded: open the link the page printed".into());
    }
    let store = Store::open(&home.join("record.sqlite3"))?;
    let due = drive::reminders_due(home, &store, now)?;
    let named: Vec<(&str, &str)> = body["due"].as_array().into_iter().flatten().filter_map(|p| Some((p[0].as_str()?, p[1].as_str()?))).collect();
    let mut marked = 0;
    for d in due.iter().filter(|d| named.contains(&(d.run.as_str(), d.question.as_str()))) {
        drive::mark_reminded(home, &d.run, &d.question, now)?;
        marked += 1;
    }
    Ok(json!({"ok": true, "marked": marked}))
}

/// Tokens as a person reads them: 950, 12k, 1.4M.
fn tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{}k", n / 1_000),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// A connection read against one deadline for all of what it sends, however it sends it: each read waits at most
/// what is left.
struct Deadlined<'a> {
    stream: &'a TcpStream,
    until: std::time::Instant,
}

impl Read for Deadlined<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.until.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the request took longer than it may"));
        }
        self.stream.set_read_timeout(Some(left))?;
        self.stream.read(buf)
    }
}

/// A connection written against one deadline for all of a reply, however slowly the other side takes it: each write
/// waits at most what is left.
struct DeadlinedWrite<'a> {
    stream: &'a TcpStream,
    until: std::time::Instant,
}

impl Write for DeadlinedWrite<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let left = self.until.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the reply took longer than it may"));
        }
        self.stream.set_write_timeout(Some(left))?;
        let mut s = self.stream;
        s.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A reply's parts, all sent by `until`, or the connection given up.
fn send(stream: &TcpStream, until: std::time::Instant, parts: &[&[u8]]) -> Result<(), String> {
    let mut w = DeadlinedWrite { stream, until };
    for part in parts {
        w.write_all(part).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn read(stream: &TcpStream, until: std::time::Instant) -> Result<Request, String> {
    // never more than a head and a body may hold, nor longer than a request may take, however the other side goes on
    let mut r = BufReader::new(Read::take(Deadlined { stream, until }, MOST_HEAD + MOST_BODY));
    let mut line = String::new();
    r.read_line(&mut line).map_err(|e| e.to_string())?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let (path, q) = target.split_once('?').map(|(p, q)| (p.to_string(), q.to_string())).unwrap_or((target.clone(), String::new()));
    let query = q.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let mut headers = BTreeMap::new();
    let mut head = line.len() as u64;
    loop {
        let mut h = String::new();
        let n = r.read_line(&mut h).map_err(|e| e.to_string())?;
        head += n as u64;
        if head > MOST_HEAD {
            return Err("the request's head is too large".into());
        }
        if n == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0).min(MOST_BODY as usize);
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).map_err(|e| e.to_string())?;
    Ok(Request { method, target, path, query, headers, body })
}

/// The script that holds the page's key, served from /key.js before the page's own. The key comes once in the link's
/// fragment, which no browser sends to any server, and is kept for this page's origin alone (scheme, host and port),
/// where no other site or app can read it, and only once the page says it is its own: a link from anywhere with
/// another key never replaces the person's. It goes only with this page's own requests, in a header.
pub const KEY_SCRIPT: &str = r##"let pageKey = "";
for (const store of [localStorage, sessionStorage]) { try { pageKey = pageKey || store.getItem("jaynes_key") || ""; } catch (_) {} }
const sendFetch = window.fetch.bind(window);
// whether this page counts what a key sends as the person's
const theirs = key => sendFetch("/api/whoami", { method: "POST", headers: { "Content-Type": "application/json", ...(key ? { "X-Jaynes-Key": key } : {}) }, body: "{}" })
  .then(r => r.json()).then(r => r.person === true, () => false);
const fromLink = (/^#key=([0-9A-Za-z]+)$/.exec(location.hash) || [])[1] || "";
// the address loses the key at once, before anything else reads it
if (location.hash.startsWith("#key=")) history.replaceState(null, "", location.href.split("#")[0]);
window.fetch = (url, opts = {}) => {
  if ((opts.method || "GET").toUpperCase() === "POST" && pageKey) opts = { ...opts, headers: { ...(opts.headers || {}), "X-Jaynes-Key": pageKey } };
  return sendFetch(url, opts);
};
window.jaynesKey = (async () => {
  if (fromLink && fromLink !== pageKey && await theirs(fromLink)) {
    pageKey = fromLink;
    for (const store of [localStorage, sessionStorage]) { try { store.setItem("jaynes_key", pageKey); break; } catch (_) {} }
  }
  // without the key, or with one this page does not know, answers from here count as an agent's: say so
  const person = pageKey ? await theirs(pageKey) : false;
  if (!person) {
    const n = document.createElement("div");
    n.className = "notice";
    n.textContent = "You opened the page without its key: answers from here count as an agent's. Open the link the page printed.";
    const box = document.getElementById("nokey");
    if (box) box.replaceWith(n);
  }
  return person;
})();
"##;

/// The page's own script, served from /page.js: the policy lets no other script run, inline or from elsewhere.
const SCRIPT: &str = r##"const el = document.getElementById("data");
const base = el ? JSON.parse(el.textContent) : null;
if (!el || base.choose) document.getElementById("fxbar").classList.add("hide");
// a stop that chooses: the answer one click on an option sends, and what it will do, written under each option
const noteOf = id => ((document.querySelector(`.adjust[data-i="${CSS.escape(id)}"]`) || {}).value || "").trim();
const ctxOf = () => (document.getElementById("ctx") || {}).value || "";
// one option: kept as it is, or adjusted with its note
function choice(id) {
  const note = noteOf(id), v = {}, n = {};
  for (const x of base.items) v[x] = x === id ? (note ? "change" : "keep") : "drop";
  if (note) n[id] = note;
  return JSON.stringify({ ...base, verdicts: v, notes: n, context: ctxOf() });
}
// finalists: each developed by the agent with its note, the rest leave
const finals = new Set();
function finalists() {
  const v = {}, n = {};
  for (const x of base.items) { v[x] = finals.has(x) ? "change" : "drop"; if (finals.has(x)) n[x] = noteOf(x) || "A finalist: develop it further, keeping what it is."; }
  return JSON.stringify({ ...base, verdicts: v, notes: n, context: ctxOf() });
}
// bring back one dropped in an earlier round: it goes on, made again by the agent, and the current options leave
const bringOf = id => ((document.querySelector(`.bring[data-b="${CSS.escape(id)}"]`) || {}).value || "").trim();
function bringBack(id) {
  const v = {}; for (const x of base.items) v[x] = "drop";
  return JSON.stringify({ ...base, verdicts: v, notes: {}, back: { [id]: bringOf(id) }, context: ctxOf() });
}
// none: every option leaves, and the person's words say what was missing
function none() {
  const v = {}; for (const x of base.items) v[x] = "drop";
  const why = (document.getElementById("why") || {}).value || "";
  return JSON.stringify({ ...base, verdicts: v, notes: {}, context: [why, ctxOf()].filter(t => t.trim()).join(" ") });
}
const esc = t => t.replace(/[&<>]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]));
// the person's words, read into the answer they mean; Jaynes says what it does, and the person sends it or not
let readBody = null, pressedBody = null;
async function readWords(text, pressed, asPressed) {
  const box = document.getElementById("understood"); if (!box || !base) return;
  box.hidden = false; box.innerHTML = '<div class="label">Reading your words…</div>';
  box.scrollIntoView({ block: "nearest" });
  pressedBody = asPressed || null;
  const r = await (await fetch("/api/understand", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ ...base, text, pressed }) })).json();
  readBody = r.body || null;
  const fallback = pressedBody ? '<button id="sendpressed">Send what I pressed</button>' : "";
  if (!readBody) { box.innerHTML = `<div class="refused">${esc(r.refused || "")}</div><div class="btns">${fallback}<button id="byhand">Answer with the buttons</button></div>`; return; }
  box.innerHTML = '<div class="label">What Jaynes read · this is what it does when you send</div>'
    + `<ul>${(r.sentences || []).map(s => `<li>${esc(s)}</li>`).join("")}</ul>`
    + ((r.repeats || []).length ? `<div class="repeat">You asked this in an earlier round: ${r.repeats.map(esc).join("; ")}</div>` : "")
    + (r.unsure ? `<div class="unsure">Not sure: ${esc(r.unsure)}</div>` : "")
    + (r.ok ? "" : `<div class="refused">${esc(r.refused || "")}</div>`)
    + `<div class="btns"><button class="go" id="sendread" ${r.ok ? "" : "disabled"}>Send this</button>${fallback}<button id="byhand">Answer with the buttons</button></div>`;
}
document.addEventListener("click", async e => {
  const b = e.target.closest("button"); if (!b || !base) return;
  if (b.id === "read") { const t = ctxOf().trim(); if (t) readWords(t, null); return; }
  if (b.id === "byhand") { document.getElementById("understood").hidden = true; return; }
  if (b.id === "sendread" || b.id === "sendpressed") {
    const body = b.id === "sendread" ? JSON.stringify({ ...base, ...readBody }) : pressedBody;
    b.disabled = true;
    const r = await (await fetch("/api/answer", { method: "POST", headers: { "Content-Type": "application/json" }, body })).json();
    if (r.ok) { b.textContent = "Sent"; setTimeout(() => location.href = "/", 900); }
    else { b.disabled = false; document.getElementById("understood").insertAdjacentHTML("beforeend", `<div class="refused">${esc(r.refused || "")}</div>`); }
  }
});
document.addEventListener("keydown", e => { if (e.target.id === "ctx" && e.key === "Enter" && e.target.value.trim()) { e.preventDefault(); readWords(e.target.value.trim(), null); } });
async function said(id, body, ulId, btn) {
  // asked as the person, with the key a link brought, once the page knows it: asked before, the engine reads it as
  // an agent's answer, refuses it, and the person's buttons go off
  await window.jaynesKey;
  const r = await (await fetch("/api/effects", { method: "POST", headers: { "Content-Type": "application/json" }, body: body || choice(id) })).json();
  const ul = ulId ? document.getElementById(ulId) : document.querySelector(`.fx-row[data-i="${CSS.escape(id)}"]`);
  // what follows stays in front; what happens to each option is folded
  const all = r.sentences || [], each = all.filter(s => s.startsWith("“")), next = all.filter(s => !s.startsWith("“"));
  if (ul) ul.innerHTML = next.map(s => `<li>${esc(s)}</li>`).join("")
    + (each.length ? `<li class="more"><details><summary>What happens to each option</summary><ul>${each.map(s => `<li>${esc(s)}</li>`).join("")}</ul></details></li>` : "")
    + (r.ok ? "" : `<li class="bad">${esc(r.refused || "")}</li>`);
  const b = btn ? document.getElementById(btn) : document.querySelector(`.choose[data-choose="${CSS.escape(id)}"]`);
  if (b) b.disabled = !r.ok;
}
function sayFinals() {
  const box = document.getElementById("finals"); if (!box) return;
  box.hidden = finals.size < 2;
  box.querySelector(".finsay").textContent = finals.size >= 2 ? `${finals.size} finalists: ${[...finals].join(", ")}` : "";
  if (finals.size >= 2) said(null, finalists(), "fx-finals", "develop");
}
const later = {};
if (base && base.choose) {
  base.items.forEach(id => said(id));
  (base.earlier || []).forEach(id => said(null, bringBack(id), `fx-b-${id}`, `bring-${id}`));
  document.addEventListener("input", e => {
    if (e.target.classList.contains("bring")) { const id = e.target.dataset.b; clearTimeout(later["b" + id]); later["b" + id] = setTimeout(() => said(null, bringBack(id), `fx-b-${id}`, `bring-${id}`), 250); return; }
    if (e.target.id === "why") { clearTimeout(later.why); later.why = setTimeout(() => e.target.value.trim() ? said(null, none(), "fx-none", "none") : (document.getElementById("none").disabled = true), 250); return; }
    if (!e.target.classList.contains("adjust")) return;
    const id = e.target.dataset.i; clearTimeout(later[id]); later[id] = setTimeout(() => { said(id); if (finals.has(id)) sayFinals(); }, 250);
  });
  document.addEventListener("click", async e => {
    const f = e.target.closest("button.fin");
    if (f) { const id = f.dataset.fin; if (finals.has(id)) finals.delete(id); else finals.add(id); f.classList.toggle("on", finals.has(id)); sayFinals(); return; }
    const b = e.target.closest("button.choose"); if (!b || b.disabled) return;
    if (b.id === "none") {
      const words = [(document.getElementById("why") || {}).value || "", ctxOf()].filter(t => t.trim()).join(" ");
      readWords(words, "None of these", none()); return;
    }
    const body = b.id === "develop" ? finalists() : b.id === "none" ? none() : b.dataset.back ? bringBack(b.dataset.back) : choice(b.dataset.choose);
    document.querySelectorAll("button.choose").forEach(x => x.disabled = true);
    const r = await (await fetch("/api/answer", { method: "POST", headers: { "Content-Type": "application/json" }, body })).json();
    if (r.ok) { b.textContent = "Sent"; setTimeout(() => location.href = "/", 900); }
    else { const ul = b.closest(".row").querySelector(".fx-row"); if (ul) ul.innerHTML = `<li class="bad">${esc(r.refused || "")}</li>`; document.querySelectorAll("button.choose").forEach(x => x.disabled = false); }
  });
}
const verdicts = {}, notes = {}, back = {};
function body() { return JSON.stringify({ ...base, verdicts, notes, back, context: (document.getElementById("ctx") || {}).value || "" }); }
async function post(path) {
  const r = await fetch(path, { method: "POST", headers: { "Content-Type": "application/json" }, body: body() });
  return r.json();
}
let timer = null;
function refresh() {
  clearTimeout(timer);
  timer = setTimeout(async () => {
    const r = await post("/api/effects");
    document.getElementById("fx").innerHTML = (r.sentences || []).map(s => `<li>${s.replace(/[&<>]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]))}</li>`).join("");
    document.getElementById("refused").textContent = r.ok ? "" : (r.refused || "");
    document.getElementById("send").disabled = !r.ok;
  }, 150);
}
document.addEventListener("click", e => { if (e.target.matches(".pics img")) e.target.classList.toggle("big"); });
document.addEventListener("click", async e => {
  const b = e.target.closest("button"); if (!b || !base) return;
  if (b.classList.contains("bringt")) {
    const id = b.dataset.back;
    if (id in back) delete back[id]; else back[id] = (document.querySelector(`.bring[data-b="${CSS.escape(id)}"]`) || {}).value || "";
    b.classList.toggle("on", id in back); refresh(); return;
  }
  if (b.dataset.v) {
    const i = b.dataset.i; verdicts[i] = b.dataset.v;
    const row = b.closest(".row");
    row.querySelectorAll(".seg button").forEach(x => x.classList.toggle("on", x === b));
    const note = row.querySelector(".note");
    const needs = b.dataset.v === "change" || b.dataset.v === "redo";
    note.classList.toggle("hide", !needs);
    note.placeholder = b.dataset.v === "change" ? "What to change" : "What is wrong";
    refresh(); return;
  }
  if (b.id === "send" && !b.disabled) {
    b.disabled = true;
    const r = await post("/api/answer");
    if (r.ok) { b.textContent = r.recorded === "inbox" ? "Sent; the run takes it after the work under way" : "Sent"; setTimeout(() => location.href = "/", 900); }
    else { document.getElementById("refused").textContent = r.refused; b.disabled = false; }
  }
});
document.addEventListener("input", e => {
  if (e.target.classList.contains("note")) notes[e.target.dataset.i] = e.target.value;
  if (e.target.classList.contains("bring") && e.target.dataset.b in back) back[e.target.dataset.b] = e.target.value;
  if (!base || base.choose) return;
  refresh();
});
if (base && !base.choose) refresh();
const rd = document.getElementById("rundata");
document.addEventListener("click", async e => {
  const b = e.target.closest("button.act"); if (!b || !rd) return;
  if ((b.dataset.act === "abandon" || b.dataset.act === "undo") && !b.classList.contains("armed")) { b.classList.add("armed"); b.textContent += ": press again"; return; }
  const run = JSON.parse(rd.textContent).run;
  const extra = b.dataset.act === "correct" ? { note: document.getElementById("cnote").value, step: document.getElementById("cstep").value } : {};
  const r = await (await fetch("/api/act", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ run, action: b.dataset.act, ...extra }) })).json();
  if (r.ok) location.reload(); else document.getElementById("actmsg").textContent = r.refused;
});
const copysum = document.getElementById("copysum");
if (copysum) copysum.addEventListener("click", async () => {
  const pre = document.getElementById("sumtext");
  try { await navigator.clipboard.writeText(pre.textContent); copysum.textContent = "Copied"; }
  catch { getSelection().selectAllChildren(pre); copysum.textContent = "Selected: copy it with Command-C"; }
  setTimeout(() => copysum.textContent = "Copy as Markdown", 2500);
});
// While the person is on a stop's screen (in front, focused, and touched in the last minute), it says so every 15 s:
// the people's minutes of the run's summary are measured from that, and nothing else.
if (base) {
  let touched = Date.now();
  for (const t of ["keydown", "pointerdown", "pointermove", "wheel", "scroll", "touchstart"]) addEventListener(t, () => { touched = Date.now(); }, { passive: true, capture: true });
  setInterval(() => {
    if (document.visibilityState !== "visible" || !document.hasFocus() || Date.now() - touched > 60000) return;
    fetch("/api/look", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ run: base.run, question: base.question }) }).catch(() => {});
  }, 15000);
}
// The page follows the record, but never reloads under the person's hands.
document.querySelectorAll("[data-setup]").forEach(b => b.addEventListener("click", async () => {
  const act = b.dataset.setup;
  const owner = b.dataset.owner || (document.getElementById("owner") || {}).value || "";
  const r = await (await fetch("/api/setup", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ action: act, owner }) })).json();
  const msg = document.getElementById("setupmsg");
  if (!r.ok) { if (msg) msg.textContent = r.refused; else alertless(r.refused); return; }
  if (act === "restart") { document.body.style.opacity = .4; setTimeout(function back() { fetch("/api/version").then(() => location.reload(), () => setTimeout(back, 500)); }, 1200); }
  else location.reload();
}));
function alertless(text) { const n = document.createElement("div"); n.className = "notice bad"; n.textContent = text; document.querySelector("main").prepend(n); }
const sf = document.getElementById("start");
if (sf) sf.addEventListener("submit", async ev => {
  ev.preventDefault();
  const r = await (await fetch("/api/start", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(Object.fromEntries(new FormData(sf))) })).json();
  if (r.ok) location.href = "/run/" + r.run;
  else document.getElementById("startmsg").textContent = r.refused;
});
let seen = null;
const busy = () => Object.keys(verdicts).length || [...document.querySelectorAll("input")].some(i => i.value) || document.activeElement.tagName === "INPUT";
setInterval(async () => {
  try {
    const v = (await (await fetch("/api/version")).json()).seq;
    if (seen !== null && v !== seen && !busy()) location.reload();
    seen = v;
  } catch (_) {}
}, 3000);
// The reminders this page shows: the person read them once the page knows they hold the key, never before.
const remind = document.getElementById("remind");
if (remind) window.jaynesKey.then(person => {
  if (person) fetch("/api/reminded", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ due: JSON.parse(remind.dataset.due || "[]") }) }).catch(() => {});
});
// Coming back after ten minutes away is a pause too: the first page shows again what waits, reminders with it.
let away = null;
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "hidden") { away = Date.now(); return; }
  if (away !== null && Date.now() - away >= 600000 && location.pathname === "/" && !busy()) location.reload();
  away = null;
});
"##;

/// What a page of Jaynes may load and run. Only its own script runs: what agents write is shown as text, and a script
/// of this origin could read the page's key.
const POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self'; img-src 'self' data:; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'";

/// The page's fonts and their licences, served from /fonts/ by these names only: the page fetches nothing from the
/// internet to draw itself. Both are under the SIL Open Font License, whose text goes with them.
const FONTS: &[(&str, &str, &[u8])] = &[
    ("InstrumentSans-Variable.ttf", "font/ttf", include_bytes!("../assets/fonts/InstrumentSans-Variable.ttf")),
    ("IBMPlexMono-Regular.ttf", "font/ttf", include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf")),
    ("IBMPlexMono-Medium.ttf", "font/ttf", include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf")),
    ("OFL-InstrumentSans.txt", "text/plain; charset=utf-8", include_bytes!("../assets/fonts/OFL-InstrumentSans.txt")),
    ("OFL-IBMPlexMono.txt", "text/plain; charset=utf-8", include_bytes!("../assets/fonts/OFL-IBMPlexMono.txt")),
];

/// What clears the cookie earlier versions kept the key in: every server on 127.0.0.1 received it, whatever its port.
/// How a request names the page.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Host {
    Name,
    Number,
    Other,
}

/// What clears the cookie earlier versions kept the key in.
const CLEAR: &str = "jaynes_key=; Path=/; Max-Age=0";

const PAGE: &str = r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Jaynes</title>
<style>
/* the page's own fonts, under the SIL Open Font License: /fonts/OFL-InstrumentSans.txt and /fonts/OFL-IBMPlexMono.txt */
@font-face { font-family:"Instrument Sans"; src:url(/fonts/InstrumentSans-Variable.ttf) format("truetype"); font-weight:400 700; font-display:swap; }
@font-face { font-family:"IBM Plex Mono"; src:url(/fonts/IBMPlexMono-Regular.ttf) format("truetype"); font-weight:400; font-display:swap; }
@font-face { font-family:"IBM Plex Mono"; src:url(/fonts/IBMPlexMono-Medium.ttf) format("truetype"); font-weight:500; font-display:swap; }
:root { --bg:#07090d; --panel:#0e1218; --panel-2:#121823; --line:#222a36; --line-2:#2c3646; --ink:#eef2f7; --dim:#93a0b2; --faint:#5d6a7c;
  --blue:#6fb4ff; --blue-soft:rgba(111,180,255,.12); --amber:#ffb35c; --amber-soft:rgba(255,179,92,.13); --green:#69dba0; --green-soft:rgba(105,219,160,.13);
  --red:#ff6f6f; --red-soft:rgba(255,111,111,.12); --display:"Instrument Sans","Helvetica Neue",Arial,sans-serif; --mono:"IBM Plex Mono",ui-monospace,Menlo,monospace; }
* { box-sizing:border-box; } html,body { margin:0; background:var(--bg); color:var(--ink); font-family:var(--display); font-size:15px; line-height:1.5; }
button { font:inherit; color:inherit; cursor:pointer; } a { color:inherit; text-decoration:none; }
.top { display:flex; align-items:center; gap:18px; padding:14px 24px; border-bottom:1px solid var(--line); } .top b { font-weight:600; } .nav { margin-left:auto; display:flex; gap:18px; } .nav a { color:var(--dim); text-decoration:none; font-size:13px; } .nav a:hover { color:var(--ink); }
.wrap { display:grid; grid-template-columns:280px 1fr; min-height:calc(100vh - 54px); }
aside { border-right:1px solid var(--line); padding:16px 0; }
.qh { padding:0 18px 8px; font-family:var(--mono); font-size:11px; color:var(--dim); text-transform:uppercase; letter-spacing:.08em; }
.q { display:block; padding:10px 18px; border-left:2px solid transparent; } .q:hover { background:rgba(255,255,255,.025); }
.q.on { border-left-color:var(--amber); background:var(--panel); }
.q small { display:block; color:var(--faint); font-family:var(--mono); font-size:11px; } .goes { color:var(--green); } .holds { color:var(--amber); }
main { padding:22px 32px 240px; max-width:900px; }
.where { font-family:var(--mono); font-size:11px; color:var(--dim); text-transform:uppercase; letter-spacing:.08em; }
h1 { font-size:24px; font-weight:600; margin:6px 0 4px; letter-spacing:-.01em; } .sub { margin:0 0 20px; }
.badge { display:inline-flex; gap:6px; align-items:center; font-size:12px; padding:3px 10px; border-radius:999px; background:var(--green-soft); color:var(--green); }
.badge.wait { background:var(--amber-soft); color:var(--amber); }
.notice { background:var(--amber-soft); color:var(--amber); padding:10px 16px; font-size:13px; border-radius:8px; margin-bottom:14px; }
.say { display:flex; align-items:center; gap:10px; background:var(--panel); border:1px solid var(--line-2); border-radius:10px; padding:6px 12px; margin-bottom:6px; }
.say label { font-family:var(--mono); font-size:11px; color:var(--dim); text-transform:uppercase; letter-spacing:.08em; }
.say input { flex:1; background:transparent; border:0; color:var(--ink); font:inherit; font-size:14px; outline:none; padding:6px 0; }
.say .read { background:var(--panel-2); color:var(--ink); border:1px solid var(--line-2); border-radius:8px; padding:6px 12px; font:inherit; font-size:13px; cursor:pointer; white-space:nowrap; }
.understood { background:var(--panel); border:1px solid var(--line-2); border-left:3px solid var(--blue); border-radius:10px; padding:12px 16px; margin:8px 0 12px; font-size:14px; }
.understood .label { font-family:var(--mono); font-size:11px; color:var(--dim); text-transform:uppercase; letter-spacing:.08em; margin-bottom:6px; }
.understood ul { margin:0 0 8px; padding-left:18px; } .understood .repeat { color:var(--amber); font-size:13px; margin:4px 0; } .understood .unsure { color:var(--amber); font-size:13px; margin:4px 0; }
.understood .btns { display:flex; gap:8px; margin-top:8px; } .understood .btns button { border-radius:8px; padding:7px 14px; font:inherit; font-size:13px; cursor:pointer; border:1px solid var(--line-2); background:var(--panel-2); color:var(--ink); }
.understood .btns .go { background:var(--blue); color:#06101c; border-color:var(--blue); font-weight:600; } .understood .btns .go:disabled { opacity:.4; cursor:default; }
.row { padding:16px 0; border-top:1px solid var(--line); } .row h3 { margin:0 0 4px; font-size:16px; font-weight:600; }
.seg { display:flex; flex-wrap:wrap; gap:6px; margin-top:8px; }
.pick { display:flex; gap:8px; margin-top:10px; } .pick .adjust { flex:1; background:var(--panel); color:var(--ink); border:1px solid var(--line-2); border-radius:8px; padding:8px 11px; font:inherit; font-size:13.5px; }
.pick .fin { background:var(--panel); color:var(--dim); border:1px solid var(--line-2); border-radius:8px; padding:8px 14px; } .pick .fin.on { border-color:var(--amber); color:var(--amber); background:var(--amber-soft); }
.finsay { flex:1; align-self:center; font-size:13.5px; color:var(--dim); }
.pick .choose { background:var(--blue); color:#04121f; border:0; border-radius:8px; padding:8px 20px; font-weight:600; } .pick .choose:disabled { background:var(--line-2); color:var(--faint); }
.fx-row { margin:6px 0 0; padding-left:18px; font-size:12.5px; color:var(--dim); } .fx-row .bad { color:var(--red); list-style:none; margin-left:-18px; } .fx-row .more { list-style:none; margin-left:-18px; } .fx-row summary { cursor:pointer; color:var(--faint); }
.seg button { background:var(--panel); border:1px solid var(--line-2); border-radius:8px; padding:6px 12px; font-size:13px; color:var(--dim); }
.seg button.on.keep { border-color:var(--green); color:var(--green); background:var(--green-soft); }
.seg button.on.change,.seg button.on.redo { border-color:var(--amber); color:var(--amber); background:var(--amber-soft); }
.seg button.on.drop { border-color:var(--red); color:var(--red); background:var(--red-soft); }
.seg button.on.skip { border-color:var(--line-2); color:var(--ink); background:var(--panel-2); }
.note { margin-top:8px; width:100%; background:var(--panel); border:1px solid var(--line-2); border-radius:8px; color:var(--ink); padding:8px 10px; font:inherit; font-size:14px; }
.detail { margin:2px 0 8px; color:var(--dim); font-size:13.5px; } .pics { display:flex; flex-wrap:wrap; gap:10px; margin:8px 0; }
.pics img { max-width:min(100%, 420px); max-height:320px; border-radius:8px; border:1px solid var(--line-2); background:#fff; cursor:zoom-in; }
.pics img.big { max-width:100%; max-height:none; cursor:zoom-out; }
.strip { display:flex; gap:3px; margin:4px 0 10px; } .strip i { width:14px; height:4px; border-radius:2px; background:var(--green); display:block; }
aside .strip { margin:5px 0 0; } aside .strip i { width:auto; flex:0 1 14px; min-width:3px; }
.strip i.waiting { background:var(--amber); } .strip i.blocked { background:var(--red); } .strip i.running { background:var(--blue); } .strip i.halted { background:var(--faint); }
.strip i.todo { background:var(--line-2); } .strip i.sampled { background:none; box-shadow:inset 0 0 0 1px var(--green); }
.stages { list-style:none; margin:8px 0 18px; padding:0; }
.stages li { display:grid; grid-template-columns:14px 1fr; gap:10px; padding:5px 0; position:relative; }
.stages li::before { content:""; position:absolute; left:6px; top:18px; bottom:-6px; width:1px; background:var(--line-2); }
.stages li:last-child::before { display:none; }
.stages .dot { width:9px; height:9px; border-radius:50%; margin:6px 0 0 2px; background:var(--green); position:relative; z-index:1; }
.stages .waiting .dot { background:var(--amber); box-shadow:0 0 0 4px var(--amber-soft); }
.stages .blocked .dot { background:var(--red); box-shadow:0 0 0 4px var(--red-soft); }
.stages .running .dot { background:var(--blue); box-shadow:0 0 0 4px var(--blue-soft); }
.stages .halted .dot { background:var(--faint); } .stages .halted small { color:var(--dim); }
.stages .todo .dot { background:var(--bg); box-shadow:inset 0 0 0 1px var(--line-2); }
.stages .sampled .dot { background:var(--bg); box-shadow:inset 0 0 0 1.5px var(--green); }
.stages .accepted .dot { box-shadow:0 0 0 3px var(--green-soft); }
.stages .r { display:flex; justify-content:space-between; gap:8px; }
.stages b { font-family:var(--mono); font-size:12px; font-weight:500; }
.stages small { font-family:var(--mono); font-size:11px; color:var(--dim); display:block; }
.stages .todo b { color:var(--faint); font-weight:400; }
.stages .waiting b, .stages .waiting small { color:var(--amber); }
.stages .blocked b, .stages .blocked small { color:var(--red); }
.stages.told li { padding:6px 0; }
.stages .who { font-family:var(--mono); font-weight:400; color:var(--faint); margin-left:10px; font-size:11px; }
.stages .about { margin:1px 0 0; font-size:13px; line-height:1.45; color:var(--dim); max-width:64ch; }
details.way { margin-top:18px; border-top:1px solid var(--line); padding-top:10px; } details.way summary { cursor:pointer; color:var(--dim); font-size:13px; }
dl.aspects { display:grid; grid-template-columns:max-content 1fr; gap:4px 14px; margin:6px 0 10px; font-size:13.5px; } dl.aspects dt { color:var(--dim); font-family:var(--mono); font-size:11px; text-transform:uppercase; letter-spacing:.06em; padding-top:2px; } dl.aspects dd { margin:0; }
details.earlier { margin-top:14px; border-top:1px solid var(--line); padding-top:10px; } details.earlier > summary { cursor:pointer; color:var(--dim); font-size:13px; } details.earlier .row { opacity:.85; } details.earlier .row:hover { opacity:1; }
.changes { margin-top:22px; } .changes .qh { margin-bottom:8px; padding-left:0; }
pre.diff { margin:0; padding:10px 0; border:1px solid var(--line); border-radius:6px; overflow-x:auto; font-family:var(--mono); font-size:12px; line-height:1.55; color:var(--dim); white-space:pre; }
.diff > span { display:block; padding:0 14px; min-width:max-content; }
.diff > span > i { font-style:normal; display:inline-block; width:2ch; color:var(--faint); user-select:none; }
.diff > .add, .diff > .del { color:var(--ink); }
.diff > .add { background:var(--green-soft); } .diff > .add > i { color:var(--green); }
.diff > .del { background:var(--red-soft); } .diff > .del > i { color:var(--red); }
.diff > .hunk { color:var(--blue); }
.diff > .file { color:var(--ink); font-weight:600; margin-top:12px; } .diff > .file:first-child { margin-top:0; }
.part { margin-top:26px; } .part h2 { font-size:15px; font-weight:600; margin:0; } .part .sub { margin:2px 0 10px; font-size:13px; }
.part.remind { margin:0 0 34px; }
.rows { border:1px solid var(--line); border-radius:8px; overflow:hidden; } .none { color:var(--faint); font-size:13px; }
.hrow { display:block; padding:11px 16px; border-top:1px solid var(--line); color:inherit; text-decoration:none; } .hrow:first-child { border-top:0; }
.hrow:hover { background:var(--panel); } .hrow .t { font-size:14px; } .hrow .hq { font-size:13px; color:var(--dim); }
.hrow .hm { display:flex; flex-wrap:wrap; gap:10px; align-items:center; margin-top:3px; font-family:var(--mono); font-size:11px; color:var(--faint); }
.chip { font-family:var(--mono); font-size:10.5px; padding:1px 7px; border-radius:10px; border:1px solid var(--line-2); color:var(--dim); }
.chip.green { color:var(--green); border-color:var(--green-soft); background:var(--green-soft); } .chip.red { color:var(--red); border-color:var(--red-soft); background:var(--red-soft); }
details.fold { margin-top:12px; } details.fold summary { cursor:pointer; color:var(--dim); font-size:13px; } details.fold summary span { color:var(--faint); font-family:var(--mono); font-size:11px; } details.fold .rows { margin-top:8px; }
.form { display:flex; flex-direction:column; gap:16px; max-width:640px; margin-top:18px; } .form label { display:flex; flex-direction:column; gap:4px; }
.form input, .form textarea { background:var(--panel); color:var(--ink); border:1px solid var(--line-2); border-radius:8px; padding:9px 12px; font:inherit; font-size:14px; }
.form input:focus, .form textarea:focus { outline:none; border-color:var(--blue); } .form .send { align-self:flex-start; }
ul.roles { margin:-8px 0 0; padding-left:18px; font-size:13px; color:var(--dim); } ul.roles b { font-family:var(--mono); font-weight:500; font-size:12px; color:var(--ink); margin-right:6px; }
pre.said { font-family:var(--mono); font-size:12px; color:var(--dim); white-space:pre-wrap; margin:12px 0; }
section.summary { margin-top:22px; border-top:1px solid var(--line); padding-top:14px; } .sumhead { display:flex; align-items:center; justify-content:space-between; gap:12px; } .sumhead h2 { margin:0; font-size:15px; font-weight:600; }
button.copy { background:var(--panel); border:1px solid var(--line-2); border-radius:8px; padding:7px 14px; font-size:13px; } button.copy:hover { border-color:var(--blue); }
section.summary pre.said { background:var(--panel); border:1px solid var(--line); border-radius:8px; padding:12px 14px; color:inherit; }
table.kv { border-collapse:collapse; margin-top:6px; } table.kv td { padding:8px 14px 8px 0; vertical-align:top; border-top:1px solid var(--line); font-size:13.5px; }
table.kv td:first-child { font-family:var(--mono); font-size:11.5px; color:var(--dim); width:120px; }
.owner { display:inline-flex; align-items:center; gap:8px; margin:0 12px 6px 0; } .owner input { background:var(--panel); color:var(--ink); border:1px solid var(--line-2); border-radius:6px; padding:5px 9px; font:inherit; font-size:13px; }
.notice.new { display:flex; align-items:center; justify-content:space-between; gap:12px; background:var(--blue-soft); color:var(--ink); }
.hide { display:none; } .empty { color:var(--dim); padding:40px 0; } .bad { color:var(--red); }
.notice.bad { background:var(--red-soft); color:var(--red); } .notice.ok { background:var(--green-soft); color:var(--green); }
.steps { width:100%; border-collapse:collapse; font-size:13.5px; margin:8px 0 18px; } .steps th { text-align:left; font-family:var(--mono); font-size:11px; color:var(--dim); font-weight:400; text-transform:uppercase; letter-spacing:.06em; padding:6px 8px; border-bottom:1px solid var(--line-2); }
.steps td { padding:7px 8px; border-bottom:1px solid var(--line); }
.acts { display:flex; flex-wrap:wrap; gap:8px; } .act { background:var(--panel); border:1px solid var(--line-2); border-radius:8px; padding:7px 14px; font-size:13px; }
.answer { display:inline-block; margin:2px 0 14px; background:var(--blue); color:#04121f; border-radius:8px; padding:9px 18px; font-weight:600; text-decoration:none; }
.act:hover { border-color:var(--blue); } .act.danger { color:var(--red); } .act.armed { border-color:var(--red); background:var(--red-soft); }
select { background:var(--panel-2); color:var(--ink); border:1px solid var(--line-2); border-radius:6px; padding:5px; font:inherit; font-size:13px; }
.effects { position:fixed; left:280px; right:0; bottom:0; background:rgba(14,18,24,.97); border-top:1px solid var(--line-2); padding:14px 32px; display:flex; gap:24px; align-items:flex-start; }
.effects.hide { display:none; } .effects .what { flex:1; max-width:700px; } .label { font-family:var(--mono); font-size:11px; color:var(--dim); text-transform:uppercase; letter-spacing:.08em; margin-bottom:4px; }
.effects ul { margin:0; padding-left:18px; font-size:13.5px; } .refused { color:var(--red); font-size:13px; margin-top:6px; }
.send { background:var(--blue); color:#04121f; border:0; border-radius:10px; padding:10px 22px; font-weight:600; } .send:disabled { background:var(--line-2); color:var(--faint); cursor:default; }
</style></head><body>
<div class="top"><b>Jaynes</b><span style="color:var(--dim);font-size:13px">the new engine</span><nav class="nav"><a href="/home">Home</a><a href="/new">New run</a><a href="/setup">Setup</a></nav></div>
<div class="wrap"><aside>{{aside}}</aside><main>{{who}}{{main}}</main></div>
<div class="effects" id="fxbar"><div class="what"><div class="label">What this answer will do · written by Jaynes, not by the role</div><ul id="fx"></ul><div class="refused" id="refused"></div></div><button class="send" id="send">Send</button></div>
<script src="/key.js"></script><script src="/page.js"></script></body></html>"##;

#[cfg(test)]
mod tests {
    use super::*;
    use jaynes_core::question::QuestionKind;

    /// The ports a test's bind was asked for on ::1, in order.
    type Asked = std::rc::Rc<std::cell::RefCell<Vec<u16>>>;

    /// A bind that finds ::1 held by another program the first `held` times it is asked for it.
    fn six_held(held: u32) -> (impl FnMut(&str, u16) -> std::io::Result<TcpListener>, Asked) {
        let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = asked.clone();
        let mut left = held;
        let bind = move |address: &str, port: u16| {
            if address == "::1" {
                seen.borrow_mut().push(port);
                if left > 0 {
                    left -= 1;
                    return Err(std::io::Error::from(std::io::ErrorKind::AddrInUse));
                }
            }
            TcpListener::bind((address, port))
        };
        (bind, asked)
    }

    #[test]
    fn on_any_port_a_port_held_on_ipv6_is_let_go_and_another_asked_for_a_few_times() {
        // held twice: the third port the system gives is the page's, on both addresses
        let (bind, asked) = six_held(2);
        let (four, six) = hold(0, bind).expect("a port");
        let port = four.local_addr().expect("address").port();
        assert_eq!(asked.borrow().len(), 3, "{:?}", asked.borrow());
        assert_eq!(asked.borrow().last(), Some(&port));
        if let Some(six) = six {
            assert_eq!(six.local_addr().expect("address").port(), port, "one port on both addresses");
        }
        // held every time: the page does not start, and says why
        let (bind, asked) = six_held(PORT_TRIES);
        let refused = hold(0, bind).map(|_| ()).expect_err("started with ::1 held");
        assert_eq!(asked.borrow().len(), PORT_TRIES as usize);
        assert!(refused.contains("another program answered on ::1 at each of 5 ports"), "{refused}");
        // a port given: another program on ::1 there is an error at once, as before
        let given = TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("address").port();
        let (bind, asked) = six_held(1);
        let refused = hold(given, bind).map(|_| ()).expect_err("started with ::1 held");
        assert_eq!(asked.borrow().len(), 1);
        assert!(refused.contains(&format!("another program answers on [::1]:{given}")), "{refused}");
    }

    /// A connection to a listener of this test, and the page's end of it.
    fn pair(listener: &TcpListener) -> (TcpStream, TcpStream) {
        let theirs = TcpStream::connect(listener.local_addr().expect("address")).expect("connect");
        let (ours, _) = listener.accept().expect("accept");
        (theirs, ours)
    }

    #[test]
    fn past_its_ceiling_the_oldest_connection_makes_way_even_while_it_takes_its_reply() {
        // every place taken by a connection whose request is read and whose reply it takes slowly, or not at all
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let mut held = Vec::new();
        for _ in 0..MOST_AT_ONCE {
            let (theirs, ours) = pair(&listener);
            let turn = Turn::take(&ours).expect("a place");
            held.push((theirs, ours, turn));
        }
        // a new one is served, never refused, and the oldest is the one closed
        let (_theirs, ours) = pair(&listener);
        assert!(Turn::take(&ours).is_some(), "the new connection was refused");
        let (first, _, _) = &mut held[0];
        first.set_read_timeout(Some(std::time::Duration::from_secs(1))).expect("timeout");
        let mut b = [0u8; 1];
        assert!(matches!(first.read(&mut b), Ok(0)), "the oldest is closed");
        let (second, _, _) = &mut held[1];
        second.set_read_timeout(Some(std::time::Duration::from_millis(200))).expect("timeout");
        assert!(second.read(&mut b).is_err(), "only the oldest is closed");
    }

    #[test]
    fn a_reply_taken_slowly_is_cut_off_at_the_connection_s_deadline() {
        // the other side takes a few bytes at a time: each write goes on a little, the whole reply does not
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let (mut theirs, ours) = pair(&listener);
        let _reader = std::thread::spawn(move || {
            let mut b = [0u8; 4096];
            while theirs.read(&mut b).is_ok_and(|n| n > 0) {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
        let started = std::time::Instant::now();
        let big = vec![b'x'; 40 * 1024 * 1024];
        let sent = send(&ours, started + std::time::Duration::from_secs(1), &[&b"HTTP/1.1 200 OK\r\n\r\n"[..], &big[..]]);
        assert!(sent.is_err(), "the whole reply went");
        assert!(started.elapsed() < std::time::Duration::from_millis(2500), "{:?}", started.elapsed());
    }

    /// A home with run r1 waiting at its plan's question, and the run's folder, where a page's act takes its lock.
    fn waiting(name: &str) -> PathBuf {
        use jaynes_core::question::{Item, Question, VerdictKind};
        use jaynes_core::{Digest, Event, Manifest};
        let home = std::env::temp_dir().join(format!("jaynes-act-in-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("runs/r1")).expect("dir");
        let manifest: Manifest = serde_json::from_value(json!({
            "role": "r", "version": "1", "protocol": 1,
            "steps": [{"name": "plan", "executor": "agent", "stop": {"verdicts": ["keep", "redo"], "holds": true}}],
        }))
        .expect("manifest");
        let start = Input::Start { run: "r1".into(), request: "x".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None };
        let mut events = decide(None, &start).expect("start").events;
        let question = Question {
            id: "q1".into(), step: "plan".into(), round: 0, kind: QuestionKind::Decision, items: vec![Item { id: "a".into(), label: "a".into(), digest: Digest("d".into()) }],
            offered: vec![VerdictKind::Keep], holds: true, about: String::new(), choose: false, pass: 0, passes: 0, back: vec![],
        };
        events.push(Event::QuestionOpened { question });
        Store::open(&home.join("record.sqlite3")).expect("store").append("r1", &events).expect("append");
        home
    }

    /// Two runs, each waiting on its question q1, which the person was told of.
    fn told(name: &str) -> PathBuf {
        use jaynes_core::question::{Item, Question, VerdictKind};
        use jaynes_core::{Digest, Event, Manifest};
        let home = std::env::temp_dir().join(format!("jaynes-remind-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        for run in ["w1", "w2"] {
            std::fs::create_dir_all(home.join("runs").join(run)).expect("dir");
            let manifest: Manifest = serde_json::from_value(json!({
                "role": "r", "version": "1", "protocol": 1,
                "steps": [{"name": "plan", "executor": "agent", "stop": {"verdicts": ["keep", "redo"], "holds": true}}],
            }))
            .expect("manifest");
            let start = Input::Start { run: run.into(), request: format!("the request of {run}"), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None };
            let mut events = decide(None, &start).expect("start").events;
            let question = Question {
                id: "q1".into(), step: "plan".into(), round: 0, kind: QuestionKind::Decision, items: vec![Item { id: "a".into(), label: "a".into(), digest: Digest("d".into()) }],
                offered: vec![VerdictKind::Keep], holds: true, about: "Is the plan right?".into(), choose: false, pass: 0, passes: 0, back: vec![],
            };
            events.push(Event::QuestionOpened { question });
            events.push(Event::Waiting { step: Some("plan".into()), on: vec!["q1".into()], why: String::new() });
            let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
            store.append(run, &events).expect("append");
            store.put_setup(run, &json!({})).expect("setup");
        }
        home
    }

    /// The section of reminders a page shows, or "" when it shows none.
    fn reminders_in(html: &str) -> &str {
        html.split("id=\"remind\"").nth(1).and_then(|r| r.split("</section>").next()).unwrap_or("")
    }

    #[test]
    fn a_reminder_shows_at_a_pause_and_only_the_person_reading_it_puts_it_off_a_day() {
        let page = Page { home: told("page"), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        let now = crate::store::now();
        // told just now: nothing to remind of yet
        assert!(!page.render(View::Home, now).expect("home").contains("id=\"remind\""));
        let later = now + drive::REMIND_EVERY + 1;
        // the first page shows w1's question; the reminders list the other, and name both as shown
        let first = page.render(View::Home, later).expect("home");
        let shown = reminders_in(&first);
        assert!(shown.contains("<h2>Still waiting for you</h2>") && shown.contains("href=\"/q/w2/q1\"") && !shown.contains("href=\"/q/w1/q1\""), "{shown}");
        assert!(shown.contains("the request of w2") && shown.contains("at plan: Is the plan right?"), "{shown}");
        assert!(shown.contains("data-due=\"[[&quot;w1&quot;,&quot;q1&quot;],[&quot;w2&quot;,&quot;q1&quot;]]\""), "{shown}");
        // Home, with no question on the screen, lists both; a run's page and the setup are no pause
        let home = page.render(View::Overview, later).expect("overview");
        assert!(reminders_in(&home).contains("href=\"/q/w1/q1\"") && reminders_in(&home).contains("href=\"/q/w2/q1\""), "{home}");
        for view in [View::Run("w1".into()), View::Setup, View::Question("w1".into(), "q1".into())] {
            assert!(!page.render(view, later).expect("page").contains("id=\"remind\""));
        }
        // an agent's reading reminds nobody, and nothing is put off
        let both = json!({"due": [["w1", "q1"], ["w2", "q1"]]});
        let refused = reminded_in(&page.home, &both, &By::Agent { who: "an agent".into() }, later).expect_err("an agent");
        assert!(refused.contains("only the person is reminded"), "{refused}");
        assert_eq!(drive::reminders_due(&page.home, &page.store().expect("store"), later).expect("due").len(), 2);
        // a name that is not a reminder due now is never written, a path least of all
        let odd = json!({"due": [["../../escaped", "q1"], ["w9", "q1"], ["w2", "q9"], "w1", [1, 2]]});
        assert_eq!(reminded_in(&page.home, &odd, &By::Person, later), Ok(json!({"ok": true, "marked": 0})));
        assert!(!page.home.join("../escaped").exists() && !page.home.join("runs/w9").exists());
        // the person read w2's: it is off for a day, and w1's, on the screen, is still named to be marked
        assert_eq!(reminded_in(&page.home, &json!({"due": [["w2", "q1"]]}), &By::Person, later), Ok(json!({"ok": true, "marked": 1})));
        let after = page.render(View::Home, later + 1).expect("home");
        assert!(after.contains("<div id=\"remind\" data-due=\"[[&quot;w1&quot;,&quot;q1&quot;]]\" hidden></div>") && !after.contains("Still waiting for you"), "{after}");
        assert_eq!(reminded_in(&page.home, &both, &By::Person, later + 1), Ok(json!({"ok": true, "marked": 1})));
        assert!(!page.render(View::Overview, later + 2).expect("overview").contains("id=\"remind\""));
        // and again a day later
        assert!(reminders_in(&page.render(View::Overview, later + 2 + drive::REMIND_EVERY).expect("overview")).contains("href=\"/q/w2/q1\""));
        let _gone = std::fs::remove_dir_all(&page.home);
    }

    #[test]
    fn a_halted_run_s_step_under_way_is_drawn_halted_not_running() {
        use jaynes_core::run::Status;
        let manifest: jaynes_core::Manifest = serde_json::from_value(json!({"role": "r", "version": "1", "protocol": 1, "steps": [{"name": "plan", "executor": "agent"}]})).expect("manifest");
        let start = Input::Start { run: "h".into(), request: "x".into(), manifest, role_fingerprint: jaynes_core::Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None };
        let mut r = Run::fold(&decide(None, &start).expect("start").events).expect("run");
        for status in [Status::Running { attempt: 1, fix: 0 }, Status::Checking { attempt: 1, fix: 0 }, Status::Probing] {
            r.steps[0].status = status;
            r.halted = false;
            assert!(strip(&r).contains("class=\"running\""), "{}", strip(&r));
            r.halted = true;
            let drawn = strip(&r);
            assert!(drawn.contains("class=\"halted\"") && !drawn.contains("class=\"running\""), "{drawn}");
        }
    }

    #[test]
    fn act_in_refuses_and_records_what_the_page_s_act_does() {
        use std::path::Path;
        // the app sends a run's controls through act_in; the page through its /api/act: one home each, the same
        // bodies from the same people, and the same replies and the same record after each
        let exe = PathBuf::from("/usr/bin/true");
        let page = Page { home: waiting("page"), port: 8750, key: "k3y".into(), exe: exe.clone() };
        let app = waiting("app");
        let agent = || By::Agent { who: "an agent".into() };
        let steps: Vec<(Value, By)> = vec![
            (json!({"action": "halt"}), By::Person),
            (json!({"run": "nope", "action": "halt"}), By::Person),
            (json!({"run": "r1", "action": "fly"}), By::Person),
            (json!({"run": "r1", "action": "resume"}), agent()),
            (json!({"run": "r1", "action": "raise-risk"}), agent()),
            (json!({"run": "r1", "action": "halt"}), agent()),
            (json!({"run": "r1", "action": "resume"}), agent()),
            (json!({"run": "r1", "action": "halt"}), By::Person),
            (json!({"run": "r1", "action": "raise-risk"}), By::Person),
            (json!({"run": "r1", "action": "raise-ceiling"}), By::Person),
            (json!({"run": "r1", "action": "correct", "note": "the plan forgot the tests", "step": "plan"}), By::Person),
            (json!({"run": "r1", "action": "undo", "note": "back"}), By::Person),
            (json!({"run": "r1", "action": "resume"}), By::Person),
            (json!({"run": "r1", "action": "abandon"}), agent()),
            (json!({"run": "r1", "action": "abandon"}), By::Person),
            (json!({"run": "r1", "action": "halt"}), By::Person),
        ];
        let (mut accepted, mut refused) = (0, 0);
        for (body, by) in steps {
            let by_page = page.act(&body, by.clone());
            let by_app = act_in(&app, &exe, &body, by.clone());
            assert_eq!(by_page, by_app, "{body} by {by:?}");
            if by_page.is_ok() { accepted += 1 } else { refused += 1 }
            let record = |home: &Path| {
                let store = Store::open(&home.join("record.sqlite3")).expect("store");
                serde_json::to_value(store.events("r1").expect("events")).expect("json")
            };
            assert_eq!(record(&page.home), record(&app), "after {body} by {by:?}");
        }
        assert!(accepted >= 5 && refused >= 5, "the steps try both sides: {accepted} accepted, {refused} refused");
        let taken = |home: &Path| Store::open(&home.join("record.sqlite3")).expect("store").inbox_take("r1").expect("inbox");
        assert_eq!(serde_json::to_value(taken(&page.home)).expect("json"), serde_json::to_value(taken(&app)).expect("json"));
        for home in [&page.home, &app] {
            let _gone = std::fs::remove_dir_all(home);
        }
    }

    #[test]
    fn no_key_is_made_from_randomness_that_does_not_read() {
        assert!(key_from(std::path::Path::new("/no/such/randomness")).is_err());
        let made = key_from(std::path::Path::new("/dev/urandom")).expect("key");
        assert_eq!(made.len(), 32);
        assert_ne!(made, "0".repeat(32));
    }

    #[test]
    fn a_name_from_outside_is_read_by_bytes_and_never_cut_inside_a_letter() {
        let odd = format!("{}\u{e9}html", "a".repeat(63));
        assert_eq!(odd.len(), 69);
        assert!(!kept_page(&odd));
        assert!(kept_page(&format!("{}.html", "a".repeat(64))));
        let home = std::env::temp_dir().join(format!("jaynes-page-names-{}", std::process::id()));
        assert_eq!(picture(&home, &format!("{}\u{e9}png", "a".repeat(63))), None);
        assert_eq!(picture(&home, &format!("{}\u{e9}\u{e9}png", "b".repeat(62))), None);
    }
}
