//! The page: the questions waiting for the person, one at a time, answered in rows with a line of context.
//!
//! A local web server whose one origin is http://localhost:PORT: it listens on 127.0.0.1 and on ::1, so no other
//! program answers for that name. Whoever opened the link with the page's key (kept by the browser for that origin)
//! answers as the person; anyone else is an agent, and the core refuses what an agent may not answer. What an answer will do is written by the core (`jaynes_core::effects`) and fetched as the person marks
//! the rows; the page never writes it.
//! Requests must name this server as their host and origin, and posts must be JSON, so another site open in the
//! same browser cannot answer.

use crate::drive::{self, say};
use crate::listing::{short, where_in};
use crate::stop::label_for;
use crate::store::Store;
use jaynes_core::question::{Answer, By, QuestionKind, Verdict};
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
/// The version of the latest release built for this machine, what Update installs, asked once an hour.
static LATEST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
/// Connections the page serves at once, on both of its addresses.
const MOST_AT_ONCE: usize = 64;
/// When a request of the person's last came: a tab of the page that holds the key asks every 3 s while it is open,
/// and a browser asks from a hidden tab about once a minute.
static TAB_ASKED: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
/// How long after the person's last request a tab of the page still counts as open: three of a hidden tab's minutes,
/// so a start does not open a second tab beside one the browser only slowed down.
const TAB_OPEN_FOR: std::time::Duration = std::time::Duration::from_secs(180);

/// Whether a tab of the page that holds the key is open at `now`, its last request having come at `last`.
fn tab_open_at(last: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    last.is_some_and(|l| now.saturating_duration_since(l) < TAB_OPEN_FOR)
}

/// Whether a tab of the page that holds the key asked it lately.
fn tab_open() -> bool {
    tab_open_at(TAB_ASKED.lock().ok().and_then(|t| *t), std::time::Instant::now())
}

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
                if let Some(v) = crate::install::latest_release()
                    && let Ok(mut l) = LATEST.lock()
                {
                    *l = Some(v);
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
        // the app's giraffe, as the page's icon: two names, each bytes built in, asking no key
        if req.method == "GET" && self.host(&req) == Host::Name
            && let Some((_, bytes)) = ICONS.iter().find(|(n, _)| *n == req.path)
        {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/svg+xml\r\nContent-Length: {}\r\nCache-Control: public, max-age=86400\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            return send(&stream, came + WHOLE_DEADLINE, &[head.as_bytes(), bytes.as_bytes()]);
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
                    // and whether a tab with the key is open, so a start opens none beside it: never when it asked
                    Some(n) => ("200 OK", "application/json", json!({"proof": proof(&self.key, n), "tab": tab_open()}).to_string(), vec![]),
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
                // after a Send, the next question with where the answer went
                [""] if req.query.get("sent").is_some_and(|r| !r.is_empty()) => View::Sent(req.query["sent"].clone()),
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
            if by == By::Person
                && let Ok(mut t) = TAB_ASKED.lock()
            {
                *t = Some(std::time::Instant::now());
            }
            let asked_by = by.clone();
            let out = match req.path.as_str() {
                // the page's poll, which with the key also says its tab is open
                "/api/version" => Ok(json!({"ok": true, "seq": self.store().and_then(|s| s.last_seq()).unwrap_or(0), "moved": crate::moved::read(&self.home).map(|m| m.len()).unwrap_or(0)})),
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

/// What a leave to change how the project's tests are defined does, in Jaynes's words, before it goes (G11, T1).
pub const LEAVE_SAYS: &str = "You let this run change how the project's tests are defined: from here its checks run the suite as the agent leaves it, and the merge and the claims name the files changed.";

/// The leave to change the tests a body carries, when the run has none yet: the person's words.
pub fn leave_in(run: &Run, body: &Value) -> Option<String> {
    let words = body["tests_leave"].as_str().map(str::trim).filter(|w| !w.is_empty())?;
    run.tests_leave.is_none().then(|| words.to_string())
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
    let mut fx = drive::effects(home, &run, &q.question, &answer)?;
    if sent.get("tests_leave").is_some() {
        if run.tests_leave.is_some() {
            sent.as_object_mut().map(|m| m.remove("tests_leave"));
        } else {
            fx.sentences.push(LEAVE_SAYS.to_string());
        }
    }
    let refused = decide(Some(&run), &Input::Answer { answer }).err().map(|r| r.0);
    Ok(json!({"ok": refused.is_none(), "body": sent, "sentences": fx.sentences, "refused": refused, "unsure": proposal["unsure"], "repeats": proposal["repeats"]}))
}

impl Page {
    fn answer(&self, body: &Value, by: By) -> Result<Value, String> {
        let (run, _, answer) = self.answer_from(body, by.clone())?;
        // the leave first, so the checks the answer sets going judge with it; refused, nothing goes
        if let Some(words) = leave_in(&run, body) {
            self.submit(&run, Input::LeaveTests { words, by })?;
        }
        self.submit(&run, Input::Answer { answer })
    }

    /// What the person does to a run from its page: halt, resume, continue, raise, abandon, correct.
    /// Change the guard, update Jaynes, or restart the page; only the person does these.
    fn setup(&self, body: &Value, by: By) -> Result<Value, String> {
        use crate::install::{guard, guard_notes_path, guard_off_in, set_guard, set_guard_off_in, set_guard_watch};
        if by != By::Person {
            return Err("only you change the setup: Open it from the Jaynes app (Open the page), or run jaynes open".into());
        }
        let (on, mut owners) = guard();
        let owner = body["owner"].as_str().unwrap_or("").trim().to_string();
        let folder = body["folder"].as_str().unwrap_or("").trim().to_string();
        match body["action"].as_str().unwrap_or("") {
            // the guard off in one project, by its folder: the person's word, which a project's own file cannot say
            "off-in-add" => {
                let named = match folder.strip_prefix('~') {
                    Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{}{rest}", std::env::var("HOME").unwrap_or_default()),
                    _ => folder.clone(),
                };
                let path = std::path::PathBuf::from(&named);
                if !path.is_absolute() {
                    return Err("a project's folder is written from / or ~/".into());
                }
                let real = path.canonicalize().map_err(|e| format!("{named} is not a folder here ({e})"))?;
                if !real.is_dir() {
                    return Err(format!("{named} is not a folder"));
                }
                // a folder that holds the person's home is almost everything, and one that holds Jaynes's own folder
                // would turn the guard off on its own files: neither is a project
                let holds = |inner: &std::path::Path| inner.canonicalize().is_ok_and(|i| i.starts_with(&real));
                if real.parent().is_none() || std::env::var_os("HOME").is_some_and(|h| holds(std::path::Path::new(&h))) {
                    return Err(format!("{} is your home folder or holds it, which is almost everything: to turn the guard off everywhere, use Turn off", real.display()));
                }
                if holds(&self.home) {
                    return Err(format!("{} holds Jaynes's own folder ({}), where the guard stays on", real.display(), self.home.display()));
                }
                let real = real.display().to_string();
                let mut off = guard_off_in();
                if !off.contains(&real) {
                    off.push(real);
                }
                set_guard_off_in(&off)?
            }
            "off-in-remove" => {
                let mut off = guard_off_in();
                off.retain(|f| *f != folder);
                set_guard_off_in(&off)?
            }
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
            return Err("only you start a run from the page: Open it from the Jaynes app (Open the page), or run jaynes open".into());
        }
        let text = |k: &str| body[k].as_str().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| format!("{k} is needed"));
        let (role, project, request) = (text("role")?, text("project")?, text("request")?);
        let project = project.strip_prefix("~/").map(|rest| std::env::var("HOME").map(|h| format!("{h}/{rest}")).unwrap_or_else(|_| project.to_string())).unwrap_or_else(|| project.to_string());
        if !std::path::Path::new(&project).join(".git").exists() {
            return Err(format!("{project} is not a git repository"));
        }
        let run = drive::launch(&self.home, &self.exe, role, &project, request, body["title"].as_str(), body["risk"].as_str() == Some("high"), true, None)?;
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
        // runs whose engine left the Mac, moved to Finished: only the person, and never through the run's engine
        if body["action"].as_str() == Some("move-to-finished") {
            if by != By::Person {
                return Err("only you move a run to Finished: open the page from the Jaynes app, or run jaynes open".into());
            }
            let ids: Vec<String> = match (body["runs"].as_array(), body["run"].as_str()) {
                (Some(all), _) => all.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
                (None, Some(one)) => vec![one.to_string()],
                (None, None) => return Err("which runs?".into()),
            };
            let moved = crate::person::move_runs(home, &ids, None)?;
            return Ok(json!({"ok": true, "moved": moved}));
        }
        let run_id = body["run"].as_str().ok_or("which run?")?;
        let run = Store::open(&home.join("record.sqlite3"))?.state(run_id)?.ok_or("no such run")?;
        let input = match body["action"].as_str().unwrap_or("") {
            "halt" => Input::Halt { by },
            "resume" => Input::Resume { by },
            // with the person's words, a run the agent's attempts left blocked goes back to the agent with them
            "continue" => match body["words"].as_str() {
                Some(words) => Input::SendBack { by, words: words.to_string() },
                None => Input::Continue { by, starter: None },
            },
            "abandon" => Input::Abandon { by, starter: None },
            // what the stop offers beside the answer, such as a new project's test servers: the person's alone, and
            // the core refuses it from anyone else (the tech-lead's T5, 23/11)
            "grant" => Input::Grant {
                question: body["question"].as_str().ok_or("which stop?")?.to_string(),
                offer: serde_json::from_value(body["offer"].clone()).map_err(|_| format!("no offer {}", body["offer"]))?,
                by,
            },
            // a hidden case the person saw fail on the card, taken out of the run: theirs alone (the tech-lead's T3)
            "drop-case" => {
                if by != By::Person {
                    return Err("only you take a hidden case out of a run, after seeing it fail: an agent never does".into());
                }
                crate::listing::case_to_drop(home, &run, body["step"].as_str().ok_or("which step?")?, body["case"].as_str().ok_or("which case?")?)?
            }
            "raise-risk" => Input::RaiseRisk { to: jaynes_core::risk::Risk::High, by },
            "raise-ceiling" => Input::RaiseCeiling { ceiling: crate::listing::ceiling_raise(&run).0, by },
            // the person merges what the run delivered on a branch: the stop every merge waits at opens (G19), and the
            // page goes to it by the question it says
            "merge" => {
                // the person's branch has it already: no stop to open for a merge that would change nothing
                if crate::listing::already_in(home, &run) {
                    let into = crate::listing::into_of(home, run_id);
                    let project = Store::open(&home.join("record.sqlite3"))?
                        .setup(run_id)
                        .ok()
                        .and_then(|s| s["project"].as_str().and_then(|p| std::path::Path::new(p).file_name()).map(|n| n.to_string_lossy().into_owned()))
                        .unwrap_or_else(|| "your project".into());
                    return Ok(json!({"ok": false, "refused": crate::listing::nothing_to_merge_said(if into.is_empty() { "your branch" } else { &into }, &project)}));
                }
                let mut said = submit_in(home, exe, &run, Input::Merge { by })?;
                let now = Store::open(&home.join("record.sqlite3"))?.state(run_id)?;
                if let Some(q) = now.as_ref().and_then(|r| r.open_questions().into_iter().find(|q| q.question.kind == QuestionKind::Merge)) {
                    said["question"] = json!(q.question.id);
                }
                return Ok(said);
            }
            "undo" => {
                let mut store = Store::open(&home.join("record.sqlite3"))?;
                let reverted = drive::undo(&mut store, home, run_id, body["note"].as_str().unwrap_or("").to_string(), by)?;
                return Ok(json!({"ok": true, "recorded": reverted}));
            }
            // the person asks the stop's agent something: nothing is redone and nothing is chosen (thread A)
            "ask" => Input::AskAgent {
                question: body["question"].as_str().ok_or("which stop?")?.to_string(),
                item: body["item"].as_str().filter(|i| !i.is_empty()).map(str::to_string),
                text: body["text"].as_str().unwrap_or("").to_string(),
                by,
            },
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
        // stops the run went on without: under what waits, never over where the run is now
        let mut still: Vec<(Run, QuestionState)> = Vec::new();
        let mut runs: Vec<Run> = Vec::new();
        // each run's name, as the app shows it (listing::name): the project first
        let names: std::collections::BTreeMap<String, crate::listing::Row> = crate::listing::read(&self.home)
            .map(|l| l.needs.into_iter().chain(l.older).chain(l.projects.into_iter().flat_map(|(_, rows)| rows)).map(|r| (r.id.clone(), r)).collect())
            .unwrap_or_default();
        let name_of = |r: &Run| names.get(&r.id).map(name_html).unwrap_or_else(|| esc(&short(&r.request)));
        // a stop says whose it is, its project first in dim, as a run's name does
        let whose = |r: &Run| names.get(&r.id).map(|n| format!("<span class=\"proj\">{}</span><span class=\"sep\"> · </span>", esc(&n.project))).unwrap_or_default();
        // runs whose engine left the Mac (row 28): never waiting for the person nor counted with what does; apart
        // until the person moves them to Finished
        let moved_list = crate::moved::read(&self.home).unwrap_or_default();
        let mut older: Vec<Run> = Vec::new();
        let mut moved: std::collections::BTreeSet<String> = Default::default();
        for id in store.runs()? {
            let Some(mut run) = store.state(&id)? else { continue };
            // the page only says: what the turn under way has spent so far counts too (agent::spent_now)
            run.spent = crate::agent::spent_now(&self.home, &run);
            let run = as_shown(&self.home, run);
            runs.push(run.clone());
            if run.finished() {
                continue;
            }
            if crate::stop::left_behind(&self.home, &run, jaynes_core::ENGINE) {
                if crate::moved::when(&moved_list, &run.id).is_some() {
                    moved.insert(run.id.clone());
                } else {
                    older.push(run);
                }
                continue;
            }
            for q in run.questions.values() {
                if crate::stop::went_on(&run, q).is_some() {
                    still.push((run.clone(), q.clone()));
                } else if q.open() && !run.blocked() {
                    // a blocked run waits as blocked (listing::stands): its open questions stay inside it
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
                .and_then(|(r, q)| waiting.iter().chain(released.iter()).chain(still.iter()).find(|(run, s)| run.id == r && s.question.id == q).cloned())
                .or_else(|| waiting.first().cloned())
                .or_else(|| released.first().cloned())
        };
        // what an item of a list beside reads as: its run's project and its question
        let reads = |run: &Run, q: &QuestionState| (names.get(&run.id).map(|n| n.project.clone()), crate::listing::title_into(q, &crate::listing::into_of(&self.home, &run.id)));
        // two items that read the same are told apart after the project, faint (the design-lead's quatro-itens.md,
        // item 8): by the run's title when the titles differ, by what listing::name tells apart when they are the
        // same
        let head = |run: &Run, q: &QuestionState, list: &[(Run, QuestionState)]| -> String {
            let question = ticks(&crate::listing::title_into(q, &crate::listing::into_of(&self.home, &run.id)));
            let me = reads(run, q);
            let same: Vec<&Run> = list.iter().filter(|(o, oq)| !(o.id == run.id && oq.question.id == q.question.id) && reads(o, oq) == me).map(|(o, _)| o).collect();
            let (Some(row), false) = (names.get(&run.id), same.is_empty()) else { return format!("{}{question}", whose(run)) };
            let one_title = same.iter().any(|o| names.get(&o.id).is_some_and(|n| n.title == row.title));
            // the title gives way first, to 16 characters; then the question, with "…"; the project and when a run
            // started, never
            let cut = |t: &str| if t.chars().count() > 16 { format!("{}…", t.chars().take(16).collect::<String>().trim_end()) } else { t.to_string() };
            let apart = if one_title { row.apart.clone().unwrap_or_else(|| cut(&row.title)) } else { cut(&row.title) };
            format!(
                "<span class=\"nm\"><span class=\"proj\">{}</span><span class=\"sep\"> · </span><span class=\"apart\">{}</span><span class=\"sep\"> · </span><span class=\"tt\">{question}</span></span>",
                esc(&row.project),
                esc(&apart)
            )
        };
        let side = |list: &[(Run, QuestionState)], released: bool| -> String {
            let went_on = |run: &Run, q: &QuestionState| crate::stop::went_on(run, q).is_some();
            list.iter()
                .map(|(run, q)| {
                    let on = current.as_ref().is_some_and(|(r, c)| r.id == run.id && c.question.id == q.question.id);
                    let status = if released {
                        "an agent let it go on · you can change it".to_string()
                    } else if went_on(run, q) && crate::stop::settled_by_request(run, q) {
                        "your request answered it · the run went on".to_string()
                    } else if went_on(run, q) {
                        "the run went on without your answer".to_string()
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
                        head(run, q, list),
                        esc(&run.manifest.role),
                        crate::listing::asked_at(run, &q.question.step),
                        status,
                        strip(run, false)
                    )
                })
                .collect()
        };
        let gone = |r: &Run| moved.contains(&r.id) || older.iter().any(|o| o.id == r.id);
        let live: Vec<Run> = runs.iter().filter(|r| !gone(r)).cloned().collect();
        // a blocked or halted run waits for the person too: only they continue or abandon it
        let blocked = runs_waiting(&live);
        let shown_run = if let View::Run(id) = &view { Some(id.clone()) } else { None };
        let blocked_links: String = blocked
            .iter()
            .map(|r| {
                let on = shown_run.as_deref() == Some(r.id.as_str());
                format!("<a class=\"q{}\" href=\"/run/{}\">{}<small>{} · {} · {}</small>{}</a>", if on { " on" } else { "" }, esc(&r.id), name_of(r), esc(&r.manifest.role), where_in(r), phase_word(&crate::listing::entry_of(&self.home, Run::clone(r))), strip(r, false))
            })
            .collect();
        let mut aside = format!("<div class=\"qh\">Waiting for you · {}</div>{}{}", waiting.len() + blocked.len(), side(&waiting, false), blocked_links);
        if !released.is_empty() {
            aside.push_str(&format!("<div class=\"qh\" style=\"margin-top:18px\">Let through by an agent · {}</div>{}", released.len(), side(&released, true)));
        }
        if !still.is_empty() {
            aside.push_str(&format!("<div class=\"qh\" style=\"margin-top:18px\">Can still answer · {}</div>{}", still.len(), side(&still, false)));
        }
        // a run whose engine left: its name, and how it stands in grey, as the app's column says it
        let older_link = |r: &Run| {
            let on = shown_run.as_deref() == Some(r.id.as_str());
            let said = names.get(&r.id).map(crate::listing::said).unwrap_or_default();
            format!("<a class=\"q{}\" href=\"/run/{}\">{}<small class=\"older\">{}</small>{}</a>", if on { " on" } else { "" }, esc(&r.id), name_of(r), esc(&said), strip(r, true))
        };
        if !older.is_empty() {
            let n = older.len();
            let ids = json!(older.iter().map(|r| r.id.as_str()).collect::<Vec<_>>()).to_string();
            let (all, ask) = if n > 1 {
                (
                    // on a line of its own under the group's head, which it no longer squeezes (the design-lead, 22/11)
                    format!("<div class=\"ogall\"><button class=\"linkish\" id=\"moveall\">{}</button></div>", crate::listing::MOVE_ALL),
                    format!(
                        "<div class=\"moveall\" id=\"moveallask\" hidden>{}<span class=\"mvb\"><button class=\"mv\" id=\"moveallyes\" data-runs=\"{}\">{}</button><button class=\"mv\" id=\"moveallno\">Cancel</button></span><div class=\"refused\" id=\"moveallmsg\"></div></div>",
                        esc(&crate::listing::move_all_asks(n)),
                        esc(&ids),
                        esc(&crate::listing::move_all_button(n))
                    ),
                )
            } else {
                (String::new(), String::new())
            };
            aside.push_str(&format!(
                "<div class=\"qh og\" style=\"margin-top:18px\"><span>{} · {n}</span></div>{all}{ask}{}",
                crate::listing::OLDER_GROUP,
                older.iter().rev().map(older_link).collect::<String>()
            ));
        }
        let open: Vec<&Run> = live.iter().rev().filter(|r| !r.finished()).collect();
        let done: Vec<&Run> = runs.iter().rev().filter(|r| r.finished() || moved.contains(&r.id)).take(8).collect();
        let run_link = |r: &&Run| {
            let on = shown_run.as_deref() == Some(r.id.as_str());
            if moved.contains(&r.id) {
                return older_link(r);
            }
            format!("<a class=\"q{}\" href=\"/run/{}\">{}<small>{} · {} · {}</small>{}</a>", if on { " on" } else { "" }, esc(&r.id), name_of(r), esc(&r.manifest.role), where_in(r), phase_word(&crate::listing::entry_of(&self.home, Run::clone(r))), strip(r, false))
        };
        aside.push_str(&format!("<div class=\"qh\" style=\"margin-top:18px\">Runs · {}</div>{}", open.len(), open.iter().map(run_link).collect::<String>()));
        if !done.is_empty() {
            aside.push_str(&format!("<div class=\"qh\" style=\"margin-top:18px\">Finished</div>{}", done.iter().map(run_link).collect::<String>()));
        }
        // at a pause of the person's (the page opened, or the page they land on after an answer), the questions they
        // were told of a day ago or more and that nobody answered
        let remind = if matches!(view, View::Home | View::Sent(_) | View::Overview) {
            let on = current.as_ref().map(|(r, q)| (r.id.as_str(), q.question.id.as_str()));
            reminders_html(&drive::reminders_due(&self.home, &store, now).unwrap_or_default(), &runs, on, &names)
        } else {
            String::new()
        };
        let main = match (&view, &current) {
            (View::Run(id), _) => match runs.iter().find(|r| &r.id == id) {
                Some(r) => {
                    let who = if crate::listing::stopped_by_jaynes(r) {
                        Some(drive::JAYNES_STOPPED_SAID.to_string())
                    } else if r.halted {
                        self.store().ok().and_then(|s| s.events(&r.id).ok()).map(|e| crate::listing::halted_by(&e))
                    } else {
                        None
                    };
                    let left = if gone(r) { Some(older_html(&self.home, r, crate::moved::when(&moved_list, &r.id))) } else { None };
                    // what each stop the request settled showed, for its line in the run's story
                    let showed: std::collections::BTreeMap<String, String> = r
                        .questions
                        .values()
                        .filter_map(|q| Some((q.question.id.clone(), crate::listing::settled_shows(&store.display(&q.question.items.first()?.digest))?)))
                        .collect();
                    let extra = RunExtra {
                        older: left,
                        blocked: crate::listing::blocked(&self.home, r),
                        showed,
                        into: crate::listing::into_of(&self.home, &r.id),
                        unmerged: crate::listing::unmerged(&self.home, r),
                        already: crate::listing::already_in(&self.home, r)
                            .then(|| store.setup(&r.id).ok().and_then(|s| s["branch"].as_str().map(str::to_string)).unwrap_or_default()),
                    };
                    run_html_of(r, &changes_html(&self.home, r, None), &format!("{}{}", refused_html(&self.home, r), summary_html(&self.home, r)), who.as_deref(), names.get(&r.id), extra)
                }
                None => match std::fs::read_to_string(self.home.join(format!("start-{id}.log"))) {
                    Ok(said) if said.trim().is_empty() => "<div class=\"empty\">The run is starting: its role is read and the project copied.</div>".to_string(),
                    Ok(said) => format!("<div class=\"empty\">The run is not recorded yet. What starting it said:</div><pre class=\"said\">{}</pre>", esc(said.trim())),
                    Err(_) => "<div class=\"empty\">There is no such run.</div>".to_string(),
                },
            },
            (View::New, _) => {
                let last = runs.iter().rev().find_map(|r| store.setup(&r.id).ok().and_then(|s| s["project"].as_str().map(|p| (r.manifest.role.clone(), p.to_string()))));
                new_html(&crate::install::list(&self.home).into_iter().map(|(n, _, a, _)| (n, a)).collect::<Vec<_>>(), last, drive::terminal_env(&self.home).is_some())
            }
            (View::Setup, _) => setup_html(&self.home),
            (_, None) => home_html(&live, &store.times()?, waiting.len(), &blocked, &self.home, &remind, &names),
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
                // at the merge, everything that lands, not the last step's change alone
                let changes = match q.question.kind {
                    QuestionKind::Merge => delivery_html(&self.home, run),
                    // a leave: what the agent wants to write, which is not written yet, never what the step changed
                    QuestionKind::Permission => crate::permit::wants_kept(&self.home, &run.id, &q.question.id).map(|w| wants_html(&w)).unwrap_or_default(),
                    _ => changes_html(&self.home, run, i),
                };
                let project = names.get(&run.id).map(|n| format!("{} · ", esc(&n.project))).unwrap_or_default();
                remind.clone() + &question_html(run, q, &shown, &changes, &crate::listing::into_of(&self.home, &run.id), &project, gone(run), crate::listing::blocked(&self.home, run), &crate::stop::offered(Store::open(&self.home.join("record.sqlite3")).ok().as_ref(), run, q))
            }
        };
        // whether the person holds the key, the page's script asks, and says so here when they do not
        // the browser's tab names the run shown, as the list does
        let shown = match &view {
            View::Run(id) => Some(id.as_str()),
            _ => current.as_ref().map(|(r, _)| r.id.as_str()),
        };
        // after a Send, where it went, on top of what comes next (quatro-itens.md, 3)
        let main = match &view {
            View::Sent(sent) => {
                let project = |id: &str| names.get(id).map(|n| n.project.clone()).filter(|p| p != "no project");
                let said = match &current {
                    None => "Sent. Nothing else waits for you.".to_string(),
                    Some((r, q)) => {
                        let asks = ticks(&crate::listing::title_into(q, &crate::listing::into_of(&self.home, &r.id)));
                        match (project(sent), project(&r.id)) {
                            (from, Some(to)) if from.as_deref() != Some(to.as_str()) => match from {
                                Some(from) => format!("Sent to <span class=\"proj\">{}</span>. Next: <span class=\"proj\">{}</span> · {asks}", esc(&from), esc(&to)),
                                None => format!("Sent. Next: <span class=\"proj\">{}</span> · {asks}", esc(&to)),
                            },
                            _ => format!("Sent. Next: {asks}"),
                        }
                    }
                };
                format!("<div class=\"sent\" id=\"sent\">{said}</div>{main}")
            }
            _ => main,
        };
        let tab = match (&view, &current) {
            // a question: its project, then what it asks (quatro-itens.md, 2)
            (View::Run(_) | View::New | View::Setup, _) | (_, None) => shown.and_then(|id| names.get(id)).map(|row| format!("{} · Jaynes", crate::listing::name(row).join(" · "))),
            (_, Some((r, q))) => {
                let asks = crate::listing::title_into(q, &crate::listing::into_of(&self.home, &r.id)).replace('`', "");
                Some(match names.get(&r.id).map(|n| n.project.as_str()).filter(|p| *p != "no project") {
                    Some(p) => format!("{p} · {asks} · Jaynes"),
                    None => format!("{asks} · Jaynes"),
                })
            }
        }
        .unwrap_or_else(|| "Jaynes".into());
        Ok(PAGE.replacen("<title>Jaynes</title>", &format!("<title>{}</title>", esc(&tab)), 1).replace("{{aside}}", &aside).replace("{{main}}", &main).replace("{{who}}", &format!("{}<div id=\"nokey\"></div>", banner())).replace("{{version}}", env!("CARGO_PKG_VERSION")))
    }
}

/// A run's name as the page shows it, from listing::name: the project dim, the title, and what tells it apart faint.
fn name_html(row: &crate::listing::Row) -> String {
    // in one line: where room is short the title gives way, cut with "…"; the project and what tells it apart never
    let mut html = format!("<span class=\"nm\"><span class=\"proj\">{}</span><span class=\"sep\"> · </span><span class=\"tt\">{}</span>", esc(&row.project), esc(&row.title));
    if let Some(apart) = &row.apart {
        html.push_str(&format!("<span class=\"sep\"> · </span><span class=\"apart\">{}</span>", esc(apart)));
    }
    html.push_str("</span>");
    html
}

/// The run's steps as a strip, one mark per step of its path, coloured by where each is, as the earlier page drew
/// it: done, waiting for you, blocked, running, halted (under way in a run the person halted), not yet, or asked only
/// a sample and skipped.
fn strip(r: &Run, gone: bool) -> String {
    use jaynes_core::run::Status;
    let told = if gone { crate::listing::cannot_go_on(crate::listing::stages(r)) } else { Vec::new() };
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
            // a run whose engine left: where it stopped in grey, never amber, as its list of steps says (the
            // design-lead, 22/11)
            let state = if gone { told.iter().find(|t| t.index == i).map_or(state, |t| if t.class == "gone" { "gone" } else if state.is_empty() || state == "sampled" { state } else { "todo" }) } else { state };
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
fn stage_list(r: &Run, gone: bool) -> String {
    let stages = crate::listing::stages(r);
    let stages = if gone { crate::listing::cannot_go_on(stages) } else { stages };
    let rows: String = stages
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
            format!("<li class=\"{class}\"><span class=\"dot\"></span><div><div class=\"r\"><b>{}{mark}</b></div>{about}{detail}{}</div></li>", esc(&d.name), thread_story(s))
        })
        .collect();
    format!("<ul class=\"stages told\">{rows}</ul>")
}

/// Where a run is, in words: the step it is on, of how many.
enum View {
    Home,
    /// What the person lands on after a Send: the next question, as home, with a line saying where the answer to
    /// run `.0` went.
    Sent(String),
    Overview,
    New,
    Setup,
    Question(String, String),
    Run(String),
}

/// How a run stands, in the word of the app's column and of `jaynes runs` (listing::stands); a run delivered whose
/// answers are open says that they can still correct it, as the app's column does.
/// A run as the page shows it. One that says it runs while the process driving it is gone (drive::left_running) is
/// shown as what it is: halted by Jaynes's own rule, "Jaynes stopped while it ran", with Resume and never Halt, as the
/// app's column says it. Only the showing: the record says so when the person resumes it (drive::settle_left_running).
fn as_shown(home: &std::path::Path, mut run: Run) -> Run {
    if drive::left_running(home, &run) {
        run.halted = true;
        run.halted_by = Some(By::Rule { rule: drive::JAYNES_STOPPED.to_string() });
    }
    run
}

pub(crate) fn phase_word(e: &crate::listing::Entry) -> String {
    use crate::listing::Mark;
    let (mark, _, _) = crate::listing::stands(e);
    let word = mark.word();
    let class = match mark {
        Mark::Needs | Mark::Halted => "holds",
        Mark::Working => "goes",
        Mark::Blocked | Mark::Undone => "bad",
        Mark::Delivered if !e.run.finished() => return format!("<span class=\"goes\">{word} · you can still correct it</span>"),
        Mark::Delivered | Mark::Merged | Mark::Abandoned | Mark::Older | Mark::Moved => return word.to_string(),
        Mark::SetAside => return word.to_string(),
    };
    format!("<span class=\"{class}\">{word}</span>")
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
/// `halted_by` says who halted it, in the column's words ("halted by an agent"), from its entries.
/// Why a high-risk run is so, and what that changes in its ceiling, in one line; nothing for a low-risk run.
fn risk_why(r: &Run) -> String {
    if r.risk == jaynes_core::risk::Risk::Low {
        return String::new();
    }
    let why = if r.risk_why.is_empty() { "the run says no more of why".to_string() } else { r.risk_why.join("; ") };
    let (high, low) = (r.ceilings.high, r.ceilings.low);
    format!(
        "<p class=\"sub\">High risk: {}. It may take up to {} min and {} tokens, where a low-risk run stops at {} min and {} tokens.</p>",
        esc(&why),
        high.seconds / 60,
        tokens_said(high.tokens),
        low.seconds / 60,
        tokens_said(low.tokens),
    )
}

/// A count of tokens as a person reads it: whole millions in words, anything else as it is.
fn tokens_said(n: u64) -> String {
    if n >= 1_000_000 && n.is_multiple_of(1_000_000) { format!("{} million", n / 1_000_000) } else { n.to_string() }
}

/// What the run's page reads beyond the record: a run from an older Jaynes says where its work is (`older`), a blocked
/// run what failed (`blocked`), each stop the request settled what it showed (`showed`, by question), and a merge the
/// branch it merges into (`into`).
#[derive(Default)]
struct RunExtra {
    older: Option<String>,
    blocked: Option<crate::listing::BlockedSaid>,
    showed: std::collections::BTreeMap<String, String>,
    into: String,
    /// Delivered on a branch and not merged: the branch with Copy, and Merge when the engine takes it (T7).
    unmerged: Option<crate::listing::Unmerged>,
    /// The person's branch has the change already: the run's branch, for Copy, and neither Merge nor Undo.
    already: Option<String>,
}

#[cfg(test)]
fn run_html(r: &Run, changes: &str, summary: &str, halted_by: Option<&str>, name: Option<&crate::listing::Row>) -> String {
    run_html_of(r, changes, summary, halted_by, name, RunExtra::default())
}

/// A run delivered on a branch and not merged (the design-lead's quatro-itens.md, item 5): the branch in mono with
/// Copy; Merge, primary, with what it does beside it in dim, when the engine takes it; and, when the last merge did
/// not land, why, in amber, with git's own words closed under it.
fn reach_html(u: &crate::listing::Unmerged) -> String {
    let (before, branch, after) = u.line();
    let line = format!(
        "<div class=\"rline\">{}<code>{}</code>{} <button class=\"copyplace\" data-copy=\"{}\">Copy</button></div>",
        esc(&before),
        esc(&branch),
        esc(&after),
        esc(&branch)
    );
    let button = u.button().map(|b| format!("<div class=\"rmerge\"><button class=\"act primary\" data-act=\"merge\">{}</button><span class=\"odim\">{}</span></div>", ticks(&b), ticks(&u.says()))).unwrap_or_default();
    let failed = u
        .not_landed()
        .map(|(said, git)| {
            let under = if git.trim().is_empty() { String::new() } else { format!("<details class=\"git\"><summary>▸ {}</summary><pre class=\"said\">{}</pre></details>", crate::listing::WHAT_GIT_SAID, esc(&git)) };
            format!("<div class=\"rfail\">{}</div>{under}", ticks(&said))
        })
        .unwrap_or_default();
    format!("<div class=\"reach\">{line}{failed}{button}</div>")
}

/// What a run whose engine left the Mac says in place of its controls (motor-velho.md, row 28): why it cannot go
/// on, where its work is (the engine names the place), and Move to Finished, or the day it was moved.
fn older_html(home: &std::path::Path, r: &Run, moved_at: Option<u64>) -> String {
    let version = crate::listing::engine_version(&r.engine);
    let place = crate::listing::run_place(home, r)
        .map(|p| {
            let (before, at, after) = p.parts();
            format!("<div class=\"odim\">{}<code>{}</code>{} <button class=\"copyplace\" data-copy=\"{}\">Copy</button></div>", esc(&before), esc(&at), esc(&after), esc(&at))
        })
        .unwrap_or_default();
    let tail = match moved_at {
        Some(at) => format!("<div class=\"odim\">{}</div>", esc(&crate::moved::moved_on(at))),
        None => format!("<button class=\"act\" data-act=\"move-to-finished\">{}</button><div class=\"odim\">{}</div>", crate::listing::MOVE_TO_FINISHED, esc(crate::listing::MOVE_SAYS)),
    };
    format!("<div class=\"notice older\"><div class=\"ohead\">{}</div><div class=\"odim\">{}</div>{place}{tail}</div>", esc(&crate::listing::older_head(version)), esc(crate::listing::OLDER_HOW))
}

/// `run_html`, with what a run whose engine left says (`older_html`) in place of its state, its controls and its
/// stops when `older` is given.
/// `blocked`: a blocked run's words as listing::blocked reads them from the engine's home, with the hidden cases that
/// failed; None, its record's reason alone.
fn run_html_of(r: &Run, changes: &str, summary: &str, halted_by: Option<&str>, name: Option<&crate::listing::Row>, extra: RunExtra) -> String {
    let RunExtra { older, blocked, showed, into, unmerged, already } = extra;
    let path = r.path();
    let gone = older.is_some();
    let steps = stage_list(r, gone);
    let phase = match &r.phase {
        // set aside by the agent that started it: the person brings it back or ends it, whatever it was at
        _ if r.set_aside.is_some() && !r.finished() => format!(
            "<div class=\"notice\">{}</div>",
            esc(&format!("Set aside by the agent that started it, at “{}”. Its work is kept: Resume brings it back, Abandon ends it.", crate::listing::halted_step(r).unwrap_or_default()))
        ),
        // in the app's words: where, why, and what Continue does
        Phase::Blocked { step, reason, fault } => blocked_notice(&blocked.unwrap_or_else(|| crate::listing::blocked_said(step.as_deref(), *fault, reason))),
        Phase::Running if r.halted => format!("<div class=\"notice\">{}</div>", esc(&crate::listing::halted_said(halted_by.unwrap_or("halted"), crate::listing::halted_step(r).as_deref()))),
        Phase::Waiting { why, .. } if why.trim().is_empty() => String::new(),
        Phase::Waiting { why, .. } => format!("<div class=\"notice\">{}</div>", esc(why)),
        Phase::Finished { outcome } => format!("<div class=\"notice ok\">Finished: {}</div>", format!("{outcome:?}").to_lowercase()),
        Phase::Running => String::new(),
    };
    // what waits for the person comes first, as a way to its question, so the run's controls are not taken for it
    let answers: String = r
        .questions
        .values()
        .filter(|q| q.open() && crate::stop::went_on(r, q).is_none())
        .map(|q| {
            // a leave is named as its card asks it, the file in mono; the role's stops by what they ask
            let asks = match q.question.kind {
                QuestionKind::Permission => ticks(&crate::listing::title(q)),
                QuestionKind::Merge => esc(&crate::listing::title_into(q, &into)),
                _ => esc(&q.question.about),
            };
            format!("<a class=\"answer\" href=\"/q/{}/{}\">Answer: {asks}</a>", esc(&r.id), esc(&q.question.id))
        })
        .collect();
    let finished = r.finished();
    let mut buttons = String::new();
    let mut tipped = |act: &str, text: &str, cls: &str, tip: Option<&str>| {
        let title = tip.map(|t| format!(" title=\"{}\"", esc(t))).unwrap_or_default();
        buttons.push_str(&format!("<button class=\"act {cls}\" data-act=\"{act}\"{title}>{text}</button>"));
    };
    if !finished {
        // each control only when it can do something: halting needs something at work, raising the ceiling a run
        // stopped by it or near it (four fifths of its time or tokens)
        if r.halted {
            tipped("resume", "Resume", "", None);
        } else if matches!(r.phase, Phase::Running) {
            tipped("halt", "Halt", "", None);
        }
        if r.blocked() {
            tipped("continue", "Continue", "", None);
        }
        // the app's words, and only where the high risk adds steps (listing::risk_adds_steps)
        if r.risk == jaynes_core::risk::Risk::Low && crate::listing::risk_adds_steps(r) {
            tipped("raise-risk", crate::listing::ASK_AT_MORE_STEPS, "", Some(crate::listing::ASK_AT_MORE_STEPS_TIP));
        }
        let near = r.spent.seconds * 5 >= r.ceiling.seconds * 4 || r.spent.tokens * 5 >= r.ceiling.tokens * 4;
        if near || matches!(r.phase, Phase::Blocked { fault: jaynes_core::event::Fault::Ceiling, .. }) {
            tipped("raise-ceiling", &crate::listing::ceiling_raise(r).1, "", None);
        }
        tipped("abandon", "Abandon", "danger", r.delivered.is_some().then_some(crate::listing::ABANDON_DELIVERED_TIP));
    }
    if (r.manifest.delivery == jaynes_core::manifest::DeliveryKind::Merge || r.merged) && r.delivered.is_some() && !r.undone && already.is_none() {
        tipped("undo", "Undo the merge", "danger", None);
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
    // the stops the run went on without, under where it is now, each in one line with its Answer; a stop the request
    // settled says what it holds and that the run went on, or that the merge kept it, with its other answer while it
    // can still be given and what it showed under it (the design-lead's merge.md)
    let passed: String = r
        .questions
        .values()
        .filter(|q| !q.superseded)
        .filter_map(|q| {
            let went_on = crate::stop::went_on(r, q).is_some();
            // a stop the request settled; any other the run went on without keeps its line with Answer (the tech-lead's
            // rule, 21/11)
            if let Some(line) = crate::listing::settled_line(r, q).filter(|_| crate::stop::settled_by_request(r, q)) {
                let other = if went_on {
                    format!("<a href=\"/q/{}/{}\">{}</a>", esc(&r.id), esc(&q.question.id), esc(&crate::stop::other_answer(r, &q.question)))
                } else {
                    String::new()
                };
                let under = showed.get(&q.question.id).map(|s| format!("<small>{}</small>", esc(s))).unwrap_or_default();
                return Some(format!("<div class=\"passed settled\"><span>{}{under}</span>{other}</div>", ticks(&line)));
            }
            went_on.then(|| format!(
                "<div class=\"passed\"><span>◇ {}</span><a href=\"/q/{}/{}\">Answer</a></div>",
                ticks(&crate::listing::passed_line(&q.question.step, &crate::listing::title(q))),
                esc(&r.id),
                esc(&q.question.id)
            ))
        })
        .collect();
    // how the change reaches the person's folder (T7): the branch it is on, with Copy and Merge, or where it merged
    let project_name = name.map(|n| n.project.clone()).filter(|p| !p.is_empty()).unwrap_or_else(|| "your project".into());
    let reach = match (&unmerged, &already) {
        (_, Some(branch)) => {
            let copy = if branch.is_empty() { String::new() } else { format!(" <button class=\"copyplace\" data-copy=\"{}\">Copy</button>", esc(branch)) };
            let said = crate::listing::nothing_to_merge_said(if into.is_empty() { "your branch" } else { &into }, &project_name);
            format!("<div class=\"reach\"><div class=\"rline\">{}{copy}</div></div>", ticks(&said))
        }
        (Some(u), None) => reach_html(u),
        (None, None) if r.merged && !r.undone && !into.is_empty() => format!("<div class=\"reach\"><div>{}</div></div>", ticks(&crate::listing::merged_said(&into, &project_name))),
        (None, None) => String::new(),
    };
    let phase = format!("{phase}{reach}");
    let (phase, answers, buttons, passed, correct) = match older {
        Some(o) => (o, String::new(), String::new(), String::new(), String::new()),
        None => (phase, answers, buttons, passed, correct),
    };
    let data = json!({"run": r.id}).to_string();
    format!(
        "<div class=\"where\">{project}{role} {version} · run {id} · {risk} risk</div><h1>{title}</h1>{request}{why}{strip}{phase}{answers}\
         <p class=\"sub\">{spent}, {st}{cached} · {work}{back}{corr}</p>\
         {steps}\
         <div class=\"acts\">{buttons}</div>{passed}{correct}<div class=\"refused\" id=\"actmsg\"></div>{summary}{changes}<script id=\"rundata\" type=\"application/json\">{data}</script>",
        role = esc(&r.manifest.role),
        version = esc(&r.manifest.version),
        id = esc(&r.id),
        risk = r.risk.word(),
        why = risk_why(r),
        project = name.map(|n| format!("{} · ", esc(&n.project))).unwrap_or_default(),
        title = match name {
            Some(n) => match &n.apart {
                Some(apart) => format!("{}<span class=\"apart\"> · {}</span>", esc(&n.title), esc(apart)),
                None => esc(&n.title),
            },
            None => esc(&short(&r.request)),
        },
        // the whole request, under the title: two lines, and the rest behind "more"
        // only when it says more than the title (nome-da-run.md)
        request = {
            let whole = r.request.trim();
            let title = name.map(|n| n.title.clone()).unwrap_or_else(|| short(&r.request));
            let long = whole.chars().count() > 160 || whole.lines().count() > 2;
            if title.trim() == whole {
                String::new()
            } else if long {
                format!("<details class=\"req\"><summary><span class=\"clamp\">{}</span><span class=\"more\">more</span></summary></details>", esc(whole))
            } else {
                format!("<p class=\"req\">{}</p>", esc(whole))
            }
        },
        strip = strip(r, gone),
        spent = esc(&crate::listing::spent(r.spent.seconds, r.ceiling.seconds)),
        // thousands as the run's summary writes them; what the cache gave only when it gave something (a count of
        // zero is left out; the spent against the ceiling stays, zero or not)
        // a turn whose stream said nothing yet is not counted yet, never 0 (agent::spent_now)
        st = {
            let (t, c) = (crate::summary::thousands(r.spent.tokens), crate::summary::thousands(r.ceiling.tokens));
            match (r.spent.uncounted, r.spent.tokens) {
                (0, _) => format!("{t} of {c} tokens"),
                (_, 0) => format!("tokens not counted yet, of {c}"),
                (1, _) => format!("{t} of {c} tokens, and one attempt's not counted yet"),
                (n, _) => format!("{t} of {c} tokens, and {n} attempts' not counted yet"),
            }
        },
        cached = if r.spent.cached > 0 { format!(" ({} read from the cache)", crate::summary::thousands(r.spent.cached)) } else { String::new() },
        // the agents' work as the app and the summary count it (listing::work)
        work = esc(&crate::listing::work(r).said()),
        // a count of zero is left out, as every counter (the tech-lead's rule)
        back = if r.counts.sent_back > 0 { format!(", {} sent back", r.counts.sent_back) } else { String::new() },
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

/// A blocked run's notice, in the app's words (listing::blocked, which reads what the engine kept; blocked_said where
/// there is no home): "Blocked at “plan”", what happened, why, the hidden cases that failed, the ways out with their
/// commands, and what Continue does.
fn blocked_notice(said: &crate::listing::BlockedSaid) -> String {
    let holds = said.holds.as_deref().map(|h| format!(" {}", esc(h))).unwrap_or_default();
    // each way whole, as the app lays it out: what to do, the command, what follows from it, and what stays open
    let ways: String = said
        .ways
        .iter()
        .map(|w| {
            let command = w.command.as_deref().map(|c| format!(" <code>{}</code>", esc(c))).unwrap_or_default();
            let then = w.then.as_deref().map(|t| format!("<span class=\"then\">{}</span>", esc(t))).unwrap_or_default();
            let still = if w.still_open.is_empty() {
                String::new()
            } else {
                let lines: String = w.still_open.iter().map(|l| format!("<li>{}</li>", esc(l))).collect();
                format!("<details class=\"still\"><summary>{}</summary><ul>{lines}</ul></details>", esc(crate::listing::STILL_OPEN_HEADING))
            };
            format!("<div class=\"way\">{}{command}{then}{still}</div>", esc(&w.says))
        })
        .collect();
    format!(
        "<div class=\"notice bad blocked\"><b>{}</b> · {} {}{holds}{}{ways}<div class=\"way\">{}</div></div>",
        esc(&said.title),
        esc(&said.what),
        esc(&said.why),
        cases_html(&said.cases),
        esc(&said.continue_does)
    )
}

/// The hidden cases that failed, for the person as the app's card shows them (the real-user, 22/11): what each says,
/// who wrote it, what it ran in mono, what it expected and what came, what it printed as text (the first lines, the
/// rest folded), and in how many attempts it failed. Nothing when none was kept.
fn cases_html(cases: &[crate::listing::CaseSaid]) -> String {
    use crate::listing::{ALL_OF_IT, CASES_HEADING, CASE_HEADING, PRINTED_SHOWN};
    if cases.is_empty() {
        return String::new();
    }
    let lines = |ls: &[String]| ls.iter().map(|l| esc(l)).collect::<Vec<_>>().join("\n");
    let each: String = cases
        .iter()
        .map(|c| {
            let printed = if c.printed.is_empty() {
                String::new()
            } else {
                let (shown, rest) = c.printed.split_at(c.printed.len().min(PRINTED_SHOWN));
                let rest = if rest.is_empty() { String::new() } else { format!("<details class=\"all\"><summary>{}</summary><pre>{}</pre></details>", esc(ALL_OF_IT), lines(rest)) };
                format!("<pre>{}</pre>{rest}", lines(shown))
            };
            format!(
                "<div class=\"case\"><b>{}</b><span class=\"by\">{}</span><p>{}</p><p>{} {}</p>{printed}<p class=\"fi\">{}</p></div>",
                esc(&c.says),
                esc(&c.written),
                ticks(&c.ran),
                esc(&c.expected),
                esc(&c.got),
                esc(&c.failed_in)
            )
        })
        .collect();
    format!("<div class=\"cases\"><div class=\"ch\">{}</div>{each}</div>", if cases.len() == 1 { CASE_HEADING } else { CASES_HEADING })
}

/// The title over what a step changed, as the app's diff says it: "Changes made at “directions” · 3 files".
fn changes_title(step: &str, n: usize) -> String {
    format!("Changes made at “{step}” · {n} file{}", if n == 1 { "" } else { "s" })
}

/// The button that chooses an option, as the app's compare mode says it.
fn choose_button(id: &str) -> String {
    format!("<button class=\"choose\" data-choose=\"{id}\">Choose this one</button>", id = esc(id))
}

/// The button of a choice with one option while its field is empty; with words it says "Not yet: send my words",
/// and sends them as a change that chooses nothing (the app's compare.rs, uma-opcao.md).
/// Beside it, once there are words, a grey "Go with this one" goes with it anyway, the words as context, as the app
/// does: who wrote and then decides to go on need not clear the field first (the design-lead, 22/11).
fn go_with_button(id: &str) -> String {
    format!("<button class=\"choose\" data-choose=\"{id}\">Go with this one</button><button class=\"choose alt\" data-keep=\"{id}\" hidden>Go with this one</button>", id = esc(id))
}

/// The row that sends every option away: "Not this one" when there is one, as the app says it.
fn none_row(options: usize) -> String {
    let words = if options == 1 { "Not this one" } else { "None of these" };
    let why = if options == 1 { "What was missing? The agent makes a new one; this one does not come back" } else { "What was missing? The agent makes new options; these do not come back" };
    format!("<div class=\"row choose\"><h3>{words}</h3><div class=\"pick\"><input class=\"adjust\" id=\"why\" placeholder=\"{why}\"><button class=\"choose\" id=\"none\" disabled>{words}</button></div><ul class=\"fx-row\" id=\"fx-none\"></ul></div>")
}

/// The button that shortlists an option at a choosing stop, in the app's words ("☆ Shortlist", "★ Shortlisted").
fn shortlist_button(id: &str) -> String {
    format!("<button class=\"fin\" data-fin=\"{id}\" title=\"Shortlist it: two or three shortlisted are developed by the agent, then you choose again\">☆ Shortlist</button>", id = esc(id))
}

/// The row that sends the shortlist, shown once two are shortlisted.
const FINALS_ROW: &str = "<div class=\"row choose\" id=\"finals\" hidden><div class=\"pick\"><span class=\"finsay\"></span><button class=\"choose\" id=\"develop\">Develop the shortlisted</button></div><ul class=\"fx-row\" id=\"fx-finals\"></ul></div>";

/// Who said an entry of a stop's conversation, and what it was: "You · round 2 · a question".
fn entry_who(e: &jaynes_core::run::Entry, choose: bool) -> String {
    use jaynes_core::run::EntryKind;
    // the step counts its rounds from 0; the person reads the first as round 1
    let round = e.round + 1;
    match e.kind {
        EntryKind::Change => format!("You · round {round} · a change"),
        EntryKind::Ask => format!("You · round {round} · a question"),
        EntryKind::Answer if e.late => "Agent · answered after the stop closed · nothing changed".into(),
        EntryKind::Answer => "Agent · answered · nothing redone".into(),
        EntryKind::AskFailed => "Agent · could not answer".into(),
        EntryKind::Reply if e.round == 0 => format!("Agent · round {round} · what it made"),
        EntryKind::Reply => format!("Agent · round {round} · {}", if choose { "changed the option" } else { "what it changed" }),
    }
}

/// The conversation at a stop's step, above where the person writes: round by round, the newest exchange whole (from
/// the person's last words on), each earlier entry in a line that opens; the agent's newest answer in amber; and,
/// while it answers, that it does.
fn thread_html(run: &Run, q: &QuestionState) -> String {
    use jaynes_core::run::EntryKind;
    let Some(i) = run.manifest.index(&q.question.step) else { return String::new() };
    let entries = &run.steps[i].thread;
    let answering = run.asking.values().any(|x| *x == q.question.id);
    if entries.is_empty() && !answering {
        return String::new();
    }
    let whole_from = entries.iter().rposition(|e| matches!(e.kind, EntryKind::Change | EntryKind::Ask)).unwrap_or(0);
    let mut out = format!("<div class=\"thread\"><div class=\"th\">Conversation at “{}”</div>", esc(&q.question.step));
    let mut round = None;
    for (n, e) in entries.iter().enumerate() {
        if round != Some(e.round) {
            round = Some(e.round);
            out.push_str(&format!("<div class=\"tr\">Round {}</div>", e.round + 1));
        }
        let who = esc(&entry_who(e, q.question.choose));
        let agent = !matches!(e.kind, EntryKind::Change | EntryKind::Ask);
        if n < whole_from {
            out.push_str(&format!("<details class=\"te\"><summary><span class=\"who\">{who}</span><span class=\"tx\">{}</span></summary></details>", esc(&e.text)));
        } else {
            let new = if agent && n + 1 == entries.len() && !answering { " new" } else { "" };
            out.push_str(&format!("<div class=\"te{new}\"><div class=\"who\">{who}</div><div class=\"tx\">{}</div></div>", esc(&e.text)));
        }
    }
    if answering {
        out.push_str("<div class=\"te wait\">The agent is answering. Nothing is redone.</div>");
    }
    out + "</div>"
}

/// A stop's conversation in the run's story, one line each, as the app tells it: the listing's words.
fn thread_story(s: &jaynes_core::run::StepState) -> String {
    crate::listing::talk(s).iter().map(|line| format!("<small class=\"talk\">{}</small>", esc(line))).collect()
}

/// `project`: the stop's project and its separator, ready to show, or "" when the run has no name yet.
#[allow(clippy::too_many_arguments)]
fn question_html(run: &Run, q: &QuestionState, shown: &dyn Fn(&jaynes_core::question::Item) -> String, changes: &str, into: &str, project: &str, gone: bool, blocked: Option<crate::listing::BlockedSaid>, offers: &[crate::stop::Offered]) -> String {
    let question = &q.question;
    let i = run.manifest.index(&question.step).unwrap_or(0);
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
    let went_on = crate::stop::went_on(run, q);
    let badge = if went_on.is_some() {
        ""
    } else if question.holds {
        "<span class=\"badge wait\">● The run waits for your answer</span>"
    } else {
        "<span class=\"badge\">● The run goes on while you look</span>"
    };
    let phase = match (&went_on, &run.phase) {
        // a stop the run went on without says where it is now, and what an answer still does
        (Some((now, blocked)), _) => format!(
            "<div class=\"notice passed-q\">The run went on without your answer. It is now at “{}”{}.<br>{}</div>",
            esc(now),
            if *blocked { ", blocked" } else { "" },
            esc(&crate::stop::still_redoes(run, question))
        ),
        (None, Phase::Blocked { .. }) => blocked.map(|b| blocked_notice(&b)).unwrap_or_default(),
        _ => String::new(),
    };
    // a stop that chooses: one click on an option sends the choice; a note beside it adjusts the chosen one
    // with one option there is nothing to choose among nor to shortlist: go with it, or not yet (uma-opcao.md)
    let one = question.items.len() == 1;
    let choose_rows = || -> String {
        let rows = question
            .items
            .iter()
            .map(|it| {
                let more_ways = if one {
                    format!("<input class=\"adjust\" data-i=\"{id}\" placeholder=\"Not sure yet? Ask the agent, or say what to change\">", id = esc(&it.id))
                } else {
                    format!("<input class=\"adjust\" data-i=\"{id}\" placeholder=\"Adjust it (optional): what to change, what to take from the others\">{}", shortlist_button(&it.id), id = esc(&it.id))
                };
                format!(
                    "<div class=\"row choose\" data-item=\"{id}\"><h3>{label}</h3>{more}<div class=\"pick\">{more_ways}{choose}</div><ul class=\"fx-row\" data-i=\"{id}\"></ul></div>",
                    id = esc(&it.id),
                    label = esc(&it.label),
                    more = shown(it),
                    choose = if one { go_with_button(&it.id) } else { choose_button(&it.id) }
                )
            })
            .collect::<String>();
        rows + if one { "" } else { FINALS_ROW } + &none_row(question.items.len())
    };
    let leave = question.kind == QuestionKind::Permission;
    let rows: String = if question.choose { choose_rows() } else if leave { leave_rows(run, question) } else { question
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
    // the merge: one touch merges every file at once, its files and their changes shown above; file by file only when
    // the person opens it (the tech-lead's S4: in a project where every file is new, never 8 buttons)
    let rows = if question.kind == QuestionKind::Merge && !question.choose {
        let all: String = question
            .offered
            .iter()
            .map(|v| {
                let key = format!("{v:?}").to_lowercase();
                format!("<button class=\"{key}\" data-all=\"1\" data-v=\"{key}\">{}</button>", label_for(*v, question.kind))
            })
            .collect();
        let all = format!("<div class=\"row all\"><div class=\"seg\">{all}</div></div>");
        if question.items.len() > 1 { format!("{all}<details class=\"each\"><summary>File by file</summary>{rows}</details>") } else { all }
    } else {
        rows
    };
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
    let data = json!({"run": run.id, "question": question.id, "shown": question.shown().0, "choose": question.choose, "merge": question.kind == QuestionKind::Merge, "items": items, "earlier": earlier}).to_string();
    // a stop about what an agent's step made may be asked about, while it is open; the agent's answer joins the thread
    let can_ask = question.kind == QuestionKind::Decision && q.open() && run.manifest.steps.get(i).is_some_and(|d| d.executor == jaynes_core::manifest::Executor::Agent);
    let merge = question.kind == QuestionKind::Merge && !question.choose;
    let answering = run.asking.values().any(|x| *x == question.id);
    let solo = question.choose && one;
    format!(
        "<div class=\"where\">{project}<a href=\"/run/{run_id}\">{role} · run {run_id}</a> · step {step} · {asked}</div><h1>{title}</h1>{note}{why}{strip}<p class=\"sub\">{rounds}{badge}</p>{phase}{thread}{lead}\
         {say}{askline}<div class=\"understood\" id=\"understood\" hidden></div>\
         {over}{rides}{mid}{rows}{under}{tail}<details class=\"way\"><summary>Steps</summary>{stages}</details><script id=\"data\" type=\"application/json\">{data}</script>",
        role = esc(&run.manifest.role),
        run_id = esc(&run.id),
        step = esc(&question.step),
        // the same count as its item in Waiting for you: the step that asks
        asked = crate::listing::asked_at(run, &question.step),
        // the project before what it asks, dim, so two questions alike tell apart (quatro-itens.md, 2); none without one
        title = match project.strip_suffix(" · ").filter(|p| !p.is_empty() && *p != "no project") {
            Some(p) => format!("<span class=\"proj\">{p}</span> · {}", ticks(&crate::listing::title_into(q, into))),
            None => ticks(&crate::listing::title_into(q, into)),
        },
        // a leave says why it comes to the person, and what the agent wants to write comes before the answers
        why = if leave { format!("<p class=\"whyyou\">{}</p>", esc(crate::stop::WHY_YOU)) } else { String::new() },
        // the role's line under its question, before the person looks (the design-lead, 22/11)
        note = match crate::stop::role_note(run, question) {
            n if n.is_empty() => String::new(),
            n => format!("<p class=\"stop-note\">{}</p>", esc(&n)),
        },
        lead = if leave { changes } else { "" },
        // at the merge what lands comes first, then the pair (the design-lead, 23/11, as the app's card)
        mid = if merge && !changes.is_empty() { format!("<div class=\"lands\" id=\"lands\">{changes}</div>") } else { String::new() },
        tail = if leave || merge { "" } else { changes },
        strip = strip(run, gone),
        thread = thread_html(run, q),
        // with one option its own field is the stop's words: a second field above it left the person unsure where to
        // write (the design-lead, 22/11)
        say = if solo { String::new() } else { format!(
            "<div class=\"say\"><label>Your words</label>{modes}<input id=\"ctx\" placeholder=\"Say what you think, in any language: what stays, what changes (or answer with the buttons)\"><button class=\"read\" id=\"read\">Read my words</button>{ask}</div>",
            modes = if can_ask { "<span class=\"mode\" hidden><button data-mode=\"ask\">Question</button><button data-mode=\"change\">Change</button></span>" } else { "" },
            ask = if can_ask { "<button class=\"go\" id=\"ask\" hidden>Ask the agent ↵</button>" } else { "" },
        ) },
        askline = if answering {
            format!("<div class=\"askline\" id=\"askline\">Your question went to the agent at “{}”. It answers here; nothing is redone and nothing is chosen.</div>", esc(&question.step))
        } else if can_ask && !solo {
            "<div class=\"askline\" id=\"askline\" hidden>The agent answers here. Nothing is redone and nothing is chosen.</div>".to_string()
        } else {
            String::new()
        },
        stages = stage_list(run, gone),
        data = data.replace('<', "\\u003c"),
        rides = rides_html(&run.id, question),
        // the box goes over the options at a stop that chooses, each option's button sending the answer; elsewhere
        // under the items, before Send
        over = if question.choose { offers_html(offers) } else { String::new() },
        under = if question.choose { String::new() } else { offers_html(offers) },
    )
}

/// What a stop offers the person to grant beside their answer (crate::stop::offered), as a box each: the core's words,
/// and dim under them what granting it does; marked as the step said, and at a stop that chooses, as each option says
/// (data-for), until the person sets it. The page's script grants each marked box before it sends the answer.
fn offers_html(offers: &[crate::stop::Offered]) -> String {
    if offers.is_empty() {
        return String::new();
    }
    let rows: String = offers
        .iter()
        .map(|o| {
            let offer = serde_json::to_value(o.offer).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            let follows = if o.marked_for.is_empty() { String::new() } else { format!(" data-for=\"{}\"", esc(&json!(o.marked_for).to_string()).replace('"', "&quot;")) };
            format!(
                "<label class=\"offer-row\"><input type=\"checkbox\" class=\"offer\" data-offer=\"{}\"{follows}{}><span><span class=\"says\">{}</span><span class=\"then\">{}</span></span></label>",
                esc(&offer),
                if o.marked { " checked" } else { "" },
                esc(&o.says),
                esc(&o.then)
            )
        })
        .collect();
    format!("<div class=\"offers\">{rows}</div>")
}

/// The stops a merge's touch keeps as the request settled them, each with its step, what it holds in Jaynes's words
/// and an Open to answer it differently before the merge (the design-lead's merge.md); nothing at another question.
fn rides_html(run: &str, question: &jaynes_core::question::Question) -> String {
    if question.rides.is_empty() {
        return String::new();
    }
    let rows: String = question
        .rides
        .iter()
        .map(|r| {
            let mut said = r.said.trim().trim_end_matches('.').to_string();
            if let Some(c) = said.chars().next() {
                said.replace_range(..c.len_utf8(), &c.to_uppercase().to_string());
            }
            format!(
                "<div class=\"ride\"><span class=\"st\">“{}”</span><span>{}.</span><a href=\"/q/{}/{}\">Open</a></div>",
                esc(&r.step),
                esc(&said),
                esc(run),
                esc(&r.question)
            )
        })
        .collect();
    format!(
        "<div class=\"rides\"><div class=\"qh\">Merging also keeps these</div>{rows}<p class=\"dim\">Merging keeps each as it is. To answer one differently, open it before you merge.</p></div>"
    )
}

/// A sentence for the page with what it puts between backquotes in mono, without the backquotes, as the app draws it:
/// "May the agent change `tests/conftest.py`?". A backquote left alone is dropped, never shown.
fn ticks(text: &str) -> String {
    let parts: Vec<&str> = text.split('`').collect();
    let n = parts.len();
    parts
        .iter()
        .enumerate()
        .map(|(i, p)| if i % 2 == 1 && i + 1 < n { format!("<code>{}</code>", esc(p)) } else { esc(p) })
        .collect()
}

/// The answers to a leave, each with the sentence of what it does beside its button (stop::leave_consequences).
fn leave_rows(run: &Run, question: &jaynes_core::question::Question) -> String {
    let says = crate::stop::leave_consequences(run, &question.about);
    question
        .items
        .iter()
        .map(|it| {
            let id = esc(&it.id);
            let buttons: String = question
                .offered
                .iter()
                .map(|v| {
                    let key = format!("{v:?}").to_lowercase();
                    let said = says.get(v).map(|s| esc(s)).unwrap_or_default();
                    format!("<div class=\"opt\"><button class=\"{key}\" data-i=\"{id}\" data-v=\"{key}\">{}</button><span class=\"says\">{said}</span></div>", label_for(*v, question.kind))
                })
                .collect();
            format!("<div class=\"row leave\" data-item=\"{id}\"><div class=\"seg says\">{buttons}</div><input class=\"note hide\" data-i=\"{id}\"></div>")
        })
        .collect()
}

/// What the agent wants to write, folded under "See what it wants to write": the file as it would leave it against
/// the step's copy, not written yet.
fn wants_html(w: &crate::stop::Wants) -> String {
    format!(
        "<details class=\"wants\"><summary>See what it wants to write</summary><div class=\"wh\">What the agent wants to write in <code>{}</code> · not written yet</div><pre class=\"diff\">{}</pre></details>",
        esc(&w.file),
        wants_lines(w.before.as_deref(), &w.after)
    )
}

/// What the agent wants to write, as the page draws it: [`crate::stop::wants_lines`], each line marked.
fn wants_lines(before: Option<&str>, after: &str) -> String {
    let line = |class: &str, mark: &str, l: &str| format!("<span class=\"{class}\"><i>{mark}</i>{}</span>", if l.is_empty() { " ".to_string() } else { esc(l) });
    crate::stop::wants_lines(before, after)
        .iter()
        .map(|(k, l)| match k {
            '+' => line("add", "+", l),
            '-' => line("del", "-", l),
            '⋯' => "<span class=\"hunk\">⋯</span>".to_string(),
            _ => line("ctx", " ", l),
        })
        .collect()
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
                "<div class=\"changes\"><div class=\"qh\">{}</div><pre class=\"diff\">{}</pre></div>",
                esc(&changes_title(&d.name, n)),
                diff_html(&text)
            ))
        })
        .collect()
}

/// What a merge lands on the person's branch, as one diff against where the run started: every file as the last step
/// that changed it left it (drive::delivery_diff). Nothing when the run's copy is gone or nothing lands.
fn delivery_html(home: &std::path::Path, run: &Run) -> String {
    let Some(mut text) = drive::delivery_diff(home, run) else { return String::new() };
    let n = text.lines().filter(|l| l.starts_with("diff --git ")).count();
    if text.len() > 40_000 {
        let cut = (0..=40_000).rev().find(|&n| text.is_char_boundary(n)).unwrap_or(0);
        let more = text.len() - cut;
        text.truncate(cut);
        text.push_str(&format!("\n[{more} more bytes not shown: read the branch before it lands]"));
    }
    let into = crate::listing::into_of(home, &run.id);
    format!(
        "{}<div class=\"changes\"><div class=\"qh\">{}</div><pre class=\"diff\">{}</pre></div>",
        tests_changed_html(&crate::listing::changed_tests(&text)),
        esc(&delivery_title(&into, n)),
        diff_html(&text)
    )
}

/// The person's tests the delivery changes, said at the merge, with See them, which shows only those files in what
/// lands: "It changed your tests: `a`, `b`" (four, then how many more), and why to read them (quatro-itens.md, 1).
/// Nothing when it changed none.
fn tests_changed_html(files: &[String]) -> String {
    if files.is_empty() {
        return String::new();
    }
    let shown: Vec<String> = files.iter().take(4).map(|f| format!("<code>{}</code>", esc(f))).collect();
    let more = if files.len() > 4 { format!(" and {} more", files.len() - 4) } else { String::new() };
    let data = esc(&serde_json::to_string(files).unwrap_or_default());
    format!(
        "<div class=\"tests-changed\"><div class=\"r\"><span>It changed your tests: {}{more}</span><button class=\"seethem\" data-files=\"{data}\">See them</button></div><p class=\"dim\">A fix can pass by changing what a test expects. Read these before you merge.</p></div>",
        shown.join(", ")
    )
}

/// The heading over what a merge lands: "What lands on “main” · 3 files".
fn delivery_title(into: &str, files: usize) -> String {
    let on = if into.is_empty() { "your branch".to_string() } else { format!("“{into}”") };
    format!("What lands on {on} · {files} file{}", if files == 1 { "" } else { "s" })
}

/// A unified diff, escaped: each file's header set apart, added lines green, removed lines red.
fn diff_html(text: &str) -> String {
    // each file's lines in a block of its own, named by its path, so See them can show only some
    let mut open = false;
    let mut html: String = text
        .lines()
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
            let line = format!("<span class=\"{class}\">{mark}{}</span>", if rest.is_empty() { " ".to_string() } else { esc(rest) });
            if class == "file" {
                let close = if std::mem::replace(&mut open, true) { "</span>" } else { "" };
                format!("{close}<span class=\"fb\" data-f=\"{}\">{line}", esc(rest))
            } else {
                line
            }
        })
        .collect();
    if open {
        html.push_str("</span>");
    }
    html
}

/// A newer Jaynes: installed since the page started, so the page should restart; or a release newer than the program
/// installed, so it can be updated. Main ahead of the latest release is no newer Jaynes: Update installs the release.
fn banner() -> String {
    let running = RUNNING.get().cloned().flatten();
    let installed = crate::install::installed_commit();
    let latest = LATEST.lock().ok().and_then(|l| l.clone());
    let (text, act, label) = if UPDATING.load(std::sync::atomic::Ordering::SeqCst) {
        return format!("<div class=\"notice new\"><span>{}</span></div>", esc(&updating_says(latest.as_deref())));
    } else if let (Some(r), Some(i)) = (&running, &installed)
        && r != i
    {
        (updated_to(crate::install::installed_version().as_deref(), env!("CARGO_PKG_VERSION"), i), "restart", "Restart the page")
    } else if let (Some(l), Some(i)) = (&latest, crate::install::installed_version())
        && crate::install::newer(l, &i)
    {
        ("A newer Jaynes is out.".to_string(), "update", "Update")
    } else {
        return String::new();
    };
    format!("<div class=\"notice new\"><span>{text}</span><button class=\"act\" data-setup=\"{act}\">{label}</button></div>")
}

/// What the notice says while Update runs, as `jaynes update` does it: the latest release built for this machine
/// (`latest`) is downloaded and installed; with none, cargo builds the newest source.
fn updating_says(latest: Option<&str>) -> String {
    match latest {
        Some(v) => format!("Jaynes is updating: it downloads Jaynes {v} and installs it."),
        None => "Jaynes is updating: no release is built for this machine, so cargo builds the newest source, which takes a few minutes.".to_string(),
    }
}

/// What the notice says of a Jaynes installed since the page started: its version, read from the program installed
/// (`installed`), when it says one other than the page's own (`running`); otherwise its commit, which always differs.
fn updated_to(installed: Option<&str>, running: &str, commit: &str) -> String {
    match installed {
        Some(v) if v != running => format!("Jaynes was updated to {v}."),
        _ => format!("Jaynes was updated to {}.", commit.get(..7).unwrap_or(commit)),
    }
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
    let off_in = crate::install::guard_off_in();
    let off_rows: String = off_in
        .iter()
        .map(|f| format!("<span class=\"owner\"><code>{}</code><button class=\"act\" data-setup=\"off-in-remove\" data-folder=\"{0}\">Turn on here</button></span>", esc(f)))
        .collect();
    let off_none = if off_in.is_empty() { "<span class=\"none\">none: it works in every project</span>" } else { "" };
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
         <table class=\"kv\"><tr><td>state</td><td>{state}</td></tr>{notes}<tr><td>free owners</td><td>{rows}{none}<span class=\"owner\"><input id=\"owner\" placeholder=\"GitHub owner\"><button class=\"act\" data-setup=\"owner-add\">Add</button></span></td></tr><tr><td>off in</td><td>{off_rows}{off_none}<span class=\"owner\"><input id=\"folder\" placeholder=\"a project's folder\"><button class=\"act\" data-setup=\"off-in-add\">Turn off here</button></span></td></tr><tr><td>settings</td><td><code>{}</code></td></tr></table></section>\
         <section class=\"part\"><h2>Version</h2><table class=\"kv\"><tr><td>running</td><td>{running}</td></tr><tr><td>installed</td><td>{version}</td></tr></table>{log}</section>\
         <div class=\"refused\" id=\"setupmsg\"></div>",
        esc(&path)
    )
}

/// The form that starts a run: a role, a project, the request, and how risky the change is. Until the person ran
/// `jaynes` at a terminal (`terminal`), it says that a run started here finds their tools once they do.
fn new_html(roles: &[(String, String)], last: Option<(String, String)>, terminal: bool) -> String {
    let (role, project) = last.unwrap_or_default();
    let role = if roles.iter().any(|(n, _)| *n == role) { role } else { roles.first().map(|r| r.0.clone()).unwrap_or_default() };
    let options: String = roles.iter().map(|(n, a)| format!("<option value=\"{}\">{}</option>", esc(n), esc(a))).collect();
    let about: String = roles.iter().map(|(n, a)| format!("<li><b>{}</b> {}</li>", esc(n), esc(a))).collect();
    let none = if roles.is_empty() { "<p class=\"sub\">No role is installed: <code>jaynes add</code> installs one, or give a role's folder.</p>" } else { "" };
    let tools = if terminal { "" } else { "<p class=\"sub\">For a run started here to find your tools (compilers, package managers) as your terminal does, run <code>jaynes</code> once in a terminal.</p>" };
    format!(
        "<h1>New run</h1><p class=\"sub\">The run goes on by itself in a copy of the project; what needs you comes to this page.</p>{none}{tools}\
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
/// The runs not finished that wait for the person, who alone continues or abandons them: the halted and the
/// blocked, newest first. The app counts them as waiting too ("9 waiting"), and the page says the same.
fn runs_waiting(runs: &[Run]) -> Vec<&Run> {
    runs.iter().rev().filter(|r| !r.finished() && (r.halted || matches!(r.phase, Phase::Blocked { .. }))).collect()
}

/// The home's line under its title: what waits for the person, the questions, the blocked runs and the halted ones.
fn waits_sentence(questions: usize, runs: &[&Run]) -> String {
    let halted = runs.iter().filter(|r| r.halted).count();
    let blocked = runs.len() - halted;
    let parts: Vec<String> = [(questions, "question", "questions"), (blocked, "blocked run", "blocked runs"), (halted, "halted run", "halted runs")]
        .into_iter()
        .filter(|(n, ..)| *n > 0)
        .map(|(n, one, many)| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") })
        .collect();
    let what = match parts.as_slice() {
        [] => return "Nothing waits for you.".into(),
        [one] => one.clone(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    };
    format!("{what} wait{} for you, in the list beside.", if questions + runs.len() == 1 { "s" } else { "" })
}

fn home_html(runs: &[Run], times: &BTreeMap<String, (u64, u64)>, waiting: usize, blocked: &[&Run], home: &std::path::Path, remind: &str, names: &BTreeMap<String, crate::listing::Row>) -> String {
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
        facts.push(if r.spent.uncounted > 0 && r.spent.tokens == 0 {
            format!("{} min, tokens not counted yet", r.spent.seconds / 60)
        } else {
            format!("{} min, {} tokens", r.spent.seconds / 60, r.spent.tokens)
        });
        facts.push(ago(now.saturating_sub(at(r))));
        format!(
            "<a class=\"hrow\" href=\"/run/{}\"><div class=\"t\">{}</div><div class=\"hq\">{}</div><div class=\"hm\">{chip}{}</div>{}</a>",
            esc(&r.id),
            names.get(&r.id).map(name_html).unwrap_or_else(|| esc(&short(&r.request))),
            esc(&said),
            facts.iter().map(|f| format!("<span>{f}</span>")).collect::<String>(),
            strip(r, false)
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
    let moving: String = moving.iter().map(|r| row(r, format!("{}, {}", where_in(r), phase_word(&crate::listing::entry_of(home, Run::clone(r)))).replace("<span class=\"goes\">", "").replace("<span class=\"holds\">", "").replace("<span class=\"bad\">", "").replace("</span>", ""), "")).collect();
    let mut dropped: Vec<&Run> = runs.iter().filter(|r| matches!(r.phase, Phase::Finished { outcome: Outcome::Abandoned })).collect();
    dropped.sort_by_key(|r| std::cmp::Reverse(at(r)));
    let abandoned = dropped.len();
    let dropped: String = dropped.iter().map(|r| row(r, "abandoned".into(), "<span class=\"chip\">abandoned</span>")).collect();
    let part = |title: &str, sub: &str, rows: &str, none: &str| {
        let body = if rows.is_empty() { format!("<div class=\"none\">{none}</div>") } else { format!("<div class=\"rows\">{rows}</div>") };
        format!("<section class=\"part\"><h2>{title}</h2><p class=\"sub\">{sub}</p>{body}</section>")
    };
    let fold = |title: &str, n: usize, rows: &str| if n == 0 { String::new() } else { format!("<details class=\"fold\"><summary>{title} <span>{n}</span></summary><div class=\"rows\">{rows}</div></details>") };
    let wait = waits_sentence(waiting, blocked);
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
fn reminders_html(due: &[drive::Reminder], runs: &[Run], on: Option<(&str, &str)>, names: &BTreeMap<String, crate::listing::Row>) -> String {
    if due.is_empty() {
        return String::new();
    }
    let rows: String = due
        .iter()
        .filter(|d| on != Some((d.run.as_str(), d.question.as_str())))
        .map(|d| {
            let run = runs.iter().find(|r| r.id == d.run);
            let request = names.get(&d.run).map(name_html).or_else(|| run.map(|r| esc(&short(&r.request)))).unwrap_or_default();
            let role = run.map(|r| r.manifest.role.clone()).unwrap_or_default();
            let about = if d.about.trim().is_empty() { String::new() } else { format!(": {}", d.about.trim()) };
            format!(
                "<a class=\"hrow\" href=\"/q/{}/{}\"><div class=\"t\">{}</div><div class=\"hq\">at {}{}</div><div class=\"hm\"><span>{}</span></div></a>",
                esc(&d.run), esc(&d.question), request, esc(&d.step), esc(&about), esc(&role)
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
        return Err("only the person is reminded: Open it from the Jaynes app (Open the page), or run jaynes open".into());
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
    // the command as code, never a raw backtick on the screen
    const command = document.createElement("code");
    command.textContent = "jaynes open";
    n.append("You opened the page without its key: answers from here count as an agent's. Open it from the Jaynes app (Open the page), or run ", command, ".");
    const box = document.getElementById("nokey");
    if (box) box.replaceWith(n);
  }
  return person;
})();
"##;

/// The page's own script, served from /page.js: the policy lets no other script run, inline or from elsewhere.
const SCRIPT: &str = r##"// after a Send, where it went: one line, gone at the next action, and not again on a reload
if (location.search.includes("sent=")) history.replaceState(null, "", location.pathname + location.hash);
for (const t of ["pointerdown", "keydown"]) addEventListener(t, () => { const s = document.getElementById("sent"); if (s) s.remove(); }, { once: true, capture: true });
// See them: what lands, only the person's tests it changed; again, every file
document.addEventListener("click", e => {
  const b = e.target.closest("button.seethem"); if (!b) return;
  const pre = document.querySelector(".changes pre.diff"); if (!pre) return;
  const files = new Set(JSON.parse(b.dataset.files || "[]"));
  const only = !pre.classList.contains("only");
  pre.classList.toggle("only", only);
  pre.querySelectorAll(".fb").forEach(f => f.classList.toggle("on", files.has(f.dataset.f)));
  b.textContent = only ? "See every file" : "See them";
  if (only) pre.scrollIntoView({ block: "start", behavior: "smooth" });
});
const el = document.getElementById("data");
const base = el ? JSON.parse(el.textContent) : null;
// what the stop offers beside the answer (the engineer lead's T5, 23/11, as the app does): a box the person set stays
// as they set it; one they did not follows the option they point at or choose, as that option says
function follow(id) {
  document.querySelectorAll("input.offer").forEach(x => { if (!id || x.dataset.set || !x.dataset.for) return; const f = JSON.parse(x.dataset.for); if (id in f) x.checked = f[id]; });
}
document.addEventListener("change", e => { if (e.target.matches && e.target.matches("input.offer")) e.target.dataset.set = "1"; });
for (const ev of ["mouseover", "focusin"]) document.addEventListener(ev, e => { const r = e.target.closest && e.target.closest(".row"); const c = r && r.querySelector("[data-choose]"); if (c) follow(c.dataset.choose); });
// every answer goes here: each marked box first, as the act "grant", the person's alone; a grant refused sends no
// answer, and a box granted leaves the stop, so a second Send never grants it again
async function answerWith(body) {
  await window.jaynesKey;
  for (const x of document.querySelectorAll("input.offer")) {
    if (!x.checked) continue;
    const g = await (await fetch("/api/act", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ run: base.run, action: "grant", question: base.question, offer: x.dataset.offer }) })).json();
    if (!g.ok) return g;
    x.closest(".offer-row").remove();
  }
  return (await fetch("/api/answer", { method: "POST", headers: { "Content-Type": "application/json" }, body })).json();
}
if (!el || base.choose) document.getElementById("fxbar").classList.add("hide");
// a stop that chooses: the answer one click on an option sends, and what it will do, written under each option
const noteOf = id => ((document.querySelector(`.adjust[data-i="${CSS.escape(id)}"]`) || {}).value || "").trim();
// a question to the agent is not context (thread A)
const ctxOf = () => mode === "ask" ? "" : (document.getElementById("ctx") || {}).value || "";
// one option: kept as it is, or adjusted with its note
function choice(id) {
  const note = noteOf(id), v = {}, n = {};
  for (const x of base.items) v[x] = x === id ? (note ? "change" : "keep") : "drop";
  if (note) n[id] = note;
  return JSON.stringify({ ...base, verdicts: v, notes: n, context: ctxOf() });
}
// one option, with words: go with it anyway, the words as context (the design-lead, 22/11)
function keepWith(id) {
  const v = {}; for (const x of base.items) v[x] = x === id ? "keep" : "drop";
  return JSON.stringify({ ...base, verdicts: v, notes: {}, context: [noteOf(id), ctxOf()].filter(t => t.trim()).join(" ") });
}
// the shortlist: each developed by the agent with its note (the engine's, decide::SHORTLISTED, without one), the rest leave
const SHORTLISTED = "Shortlisted: develop it further, keeping what it is.";
const finals = new Set();
function finalists() {
  const v = {}, n = {};
  for (const x of base.items) { v[x] = finals.has(x) ? "change" : "drop"; if (finals.has(x)) n[x] = noteOf(x) || SHORTLISTED; }
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
    const r = await answerWith(body);
    if (r.ok) { b.textContent = "Sent"; setTimeout(() => location.href = "/?sent=" + encodeURIComponent(base.run), 900); }
    else { b.disabled = false; document.getElementById("understood").insertAdjacentHTML("beforeend", `<div class="refused">${esc(r.refused || "")}</div>`); }
  }
});
// a question to the stop's agent or words for a change: a guess without a model, which a click turns (thread A)
// var: read by ctxOf, which comes before it
var mode = null, picked = false;
const guessMode = t => /\?\s*$/.test(t) || /^(e se|what if|por ?que|why|how|como|what|qual|quais|o que|será)\b/i.test(t) ? "ask" : "change";
function showMode() {
  const box = document.querySelector(".say .mode"), ctx = document.getElementById("ctx"); if (!box || !ctx) return;
  const t = ctx.value.trim(); if (!t) picked = false;
  mode = t ? (picked ? mode : guessMode(t)) : null;
  box.hidden = !t; box.querySelectorAll("button").forEach(b => b.classList.toggle("on", b.dataset.mode === mode));
  document.getElementById("ask").hidden = mode !== "ask";
  document.getElementById("read").hidden = mode === "ask";
  const line = document.getElementById("askline"); if (line && !line.dataset.answering) { line.hidden = mode !== "ask"; line.classList.remove("bad"); line.textContent = "The agent answers here. Nothing is redone and nothing is chosen."; }
}
async function askAgent() {
  const ctx = document.getElementById("ctx"), b = document.getElementById("ask"), t = ctx.value.trim(); if (!t || !base) return;
  b.disabled = true;
  await window.jaynesKey;
  const r = await (await fetch("/api/act", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ run: base.run, action: "ask", question: base.question, text: t }) })).json();
  b.disabled = false;
  if (r.ok) { ctx.value = ""; location.reload(); return; }
  const line = document.getElementById("askline"); line.hidden = false; line.classList.add("bad"); line.textContent = r.refused || "";
}
{ const line = document.getElementById("askline"); if (line && !line.hidden) line.dataset.answering = "1"; }
document.addEventListener("input", e => { if (e.target.id === "ctx") showMode(); });
document.addEventListener("click", e => {
  const b = e.target.closest("button"); if (!b) return;
  if (b.dataset.mode) { picked = true; mode = b.dataset.mode; showMode(); if (base && !base.choose) refresh(); return; }
  if (b.id === "ask") askAgent();
});
document.addEventListener("keydown", e => { if (e.target.id === "ctx" && e.key === "Enter" && e.target.value.trim()) { e.preventDefault(); if (mode === "ask") askAgent(); else readWords(e.target.value.trim(), null); } });
async function said(id, body, ulId, btn) {
  // asked as the person, with the key a link brought, once the page knows it: asked before, the engine reads it as
  // an agent's answer, refuses it, and the person's buttons go off
  await window.jaynesKey;
  const r = await (await fetch("/api/effects", { method: "POST", headers: { "Content-Type": "application/json" }, body: body || choice(id) })).json();
  const ul = ulId ? document.getElementById(ulId) : document.querySelector(`.fx-row[data-i="${CSS.escape(id)}"]`);
  // what follows stays in front; what happens to each option is folded; with one there is no "each option": in
  // their order under the button, what becomes of it first (the design-lead, 22/11)
  const all = r.sentences || [], each = all.filter(s => s.startsWith("“")), next = all.filter(s => !s.startsWith("“"));
  if (base.items.length === 1) { if (ul) ul.innerHTML = [...each, ...next].map(s => `<li>${esc(s)}</li>`).join("") + (r.ok ? "" : `<li class="bad">${esc(r.refused || "")}</li>`); }
  else if (ul) ul.innerHTML = next.map(s => `<li>${esc(s)}</li>`).join("")
    + (each.length ? `<li class="more"><details><summary>What happens to each option</summary><ul>${each.map(s => `<li>${esc(s)}</li>`).join("")}</ul></details></li>` : "")
    + (r.ok ? "" : `<li class="bad">${esc(r.refused || "")}</li>`);
  const b = btn ? document.getElementById(btn) : document.querySelector(`.choose[data-choose="${CSS.escape(id)}"]`);
  if (b) b.disabled = !r.ok;
}
function sayFinals() {
  const box = document.getElementById("finals"); if (!box) return;
  box.hidden = finals.size < 2;
  box.querySelector(".finsay").textContent = finals.size >= 2 ? `${finals.size} shortlisted: ${[...finals].join(", ")}` : "";
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
    const id = e.target.dataset.i;
    // one option: go with it while the field is empty, not yet once it has words (uma-opcao.md)
    if (base.items.length === 1) { const b = document.querySelector(`button.choose[data-choose="${CSS.escape(id)}"]`); if (b) b.textContent = e.target.value.trim() ? "Not yet: send my words" : "Go with this one"; const alt = document.querySelector(`button.alt[data-keep="${CSS.escape(id)}"]`); if (alt) alt.hidden = !e.target.value.trim(); }
    clearTimeout(later[id]); later[id] = setTimeout(() => { said(id); if (finals.has(id)) sayFinals(); }, 250);
  });
  document.addEventListener("click", async e => {
    const f = e.target.closest("button.fin");
    if (f) { const id = f.dataset.fin; if (finals.has(id)) finals.delete(id); else finals.add(id); f.classList.toggle("on", finals.has(id)); f.textContent = finals.has(id) ? "★ Shortlisted" : "☆ Shortlist"; sayFinals(); return; }
    const b = e.target.closest("button.choose"); if (!b || b.disabled) return;
    if (b.id === "none") {
      const words = [(document.getElementById("why") || {}).value || "", ctxOf()].filter(t => t.trim()).join(" ");
      readWords(words, document.getElementById("none").textContent, none()); return;
    }
    const body = b.id === "develop" ? finalists() : b.id === "none" ? none() : b.dataset.back ? bringBack(b.dataset.back) : b.dataset.keep ? keepWith(b.dataset.keep) : choice(b.dataset.choose);
    document.querySelectorAll("button.choose").forEach(x => x.disabled = true);
    follow(b.dataset.choose || b.dataset.keep);
    const r = await answerWith(body);
    if (r.ok) { b.textContent = "Sent"; setTimeout(() => location.href = "/?sent=" + encodeURIComponent(base.run), 900); }
    else { const ul = b.closest(".row").querySelector(".fx-row"); if (ul) ul.innerHTML = `<li class="bad">${esc(r.refused || "")}</li>`; document.querySelectorAll("button.choose").forEach(x => x.disabled = false); }
  });
}
const verdicts = {}, notes = {}, back = {};
function body() { return JSON.stringify({ ...base, verdicts, notes, back, context: ctxOf() }); }
async function post(path) {
  const r = await fetch(path, { method: "POST", headers: { "Content-Type": "application/json" }, body: body() });
  return r.json();
}
let timer = null;
// the fade under what lands shows while more is below its ceiling
const lands = document.getElementById("lands");
const landsPre = lands && lands.querySelector("pre.diff");
function landsMore() { if (landsPre) lands.classList.toggle("more", landsPre.scrollTop + landsPre.clientHeight < landsPre.scrollHeight - 2); }
if (landsPre) { landsPre.addEventListener("scroll", landsMore); addEventListener("resize", landsMore); landsMore(); }
let tried = false;
document.addEventListener("click", e => { if (e.target.closest && e.target.closest("button[data-v], #send, .bringt")) tried = true; }, true);
document.addEventListener("input", e => { if (e.target.matches && e.target.matches(".note, .adjust, #ctx")) tried = true; }, true);
function refresh() {
  clearTimeout(timer);
  timer = setTimeout(async () => {
    const r = await post("/api/effects");
    document.getElementById("fx").innerHTML = (r.sentences || []).map(s => `<li>${s.replace(/[&<>]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]))}</li>`).join("");
    // before the person tried anything there is no error: no refusal in red. At the merge one faint line says what
    // to do (the design-lead, 23/11); red only for a refusal after a try
    const f = document.getElementById("refused");
    const hint = !r.ok && base.merge && (!tried || r.refused === "keep or drop each file it shows");
    f.classList.toggle("hint", hint);
    f.textContent = r.ok ? "" : hint ? "Merge, or keep it on the branch only." : tried ? (r.refused || "") : "";
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
  if (b.dataset.all) {
    // the whole merge at once: every file takes it, and each file's own pair shows it
    for (const i of base.items) verdicts[i] = b.dataset.v;
    document.querySelectorAll(".seg button[data-v]").forEach(x => x.classList.toggle("on", x.dataset.v === b.dataset.v));
    refresh(); return;
  }
  if (b.dataset.v) {
    document.querySelectorAll(".row.all button").forEach(x => x.classList.remove("on"));
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
    const r = await answerWith(body());
    if (r.ok) { b.textContent = r.recorded === "inbox" ? "Sent; the run takes it after the work under way" : "Sent"; setTimeout(() => location.href = "/?sent=" + encodeURIComponent(base.run), 900); }
    else { const f = document.getElementById("refused"); f.classList.remove("hint"); f.textContent = r.refused; b.disabled = false; }
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
  if (r.ok && r.question) location.href = "/q/" + encodeURIComponent(run) + "/" + encodeURIComponent(r.question);
  else if (r.ok) location.reload(); else document.getElementById("actmsg").textContent = r.refused;
});
// Runs whose engine left the Mac: all moved at once after a line that asks, Esc or Cancel closing it; a place copied.
const moveall = document.getElementById("moveall"), moveask = document.getElementById("moveallask");
if (moveall && moveask) {
  const close = () => { moveask.hidden = true; };
  moveall.addEventListener("click", () => { moveask.hidden = false; document.getElementById("moveallyes").focus(); });
  document.getElementById("moveallno").addEventListener("click", close);
  addEventListener("keydown", e => { if (e.key === "Escape" && !moveask.hidden) close(); });
  document.getElementById("moveallyes").addEventListener("click", async e => {
    const r = await (await fetch("/api/act", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ action: "move-to-finished", runs: JSON.parse(e.target.dataset.runs) }) })).json();
    if (r.ok) location.reload(); else document.getElementById("moveallmsg").textContent = r.refused;
  });
}
document.addEventListener("click", async e => {
  const b = e.target.closest("button.copyplace"); if (!b) return;
  try { await navigator.clipboard.writeText(b.dataset.copy); b.textContent = "Copied"; } catch { b.textContent = "Select it and copy with Command-C"; }
  setTimeout(() => b.textContent = "Copy", 2500);
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
  const folder = b.dataset.folder || (document.getElementById("folder") || {}).value || "";
  const r = await (await fetch("/api/setup", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ action: act, owner, folder }) })).json();
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
    // a run moved to Finished writes no event: the engine's list counts too
    const j = await (await fetch("/api/version", { method: "POST", headers: { "Content-Type": "application/json" }, body: "{}" })).json();
    const v = j.seq + "/" + (j.moved || 0);
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
/// The page's icon in a browser's tab, for a light and a dark browser: the giraffe of the app's icon drawn again for 16
/// and 32 px. The app's own icon is in app/assets/AppIcon.icon.
const ICONS: &[(&str, &str)] = &[
    ("/favicon-light.svg", include_str!("../assets/icon/favicon-light.svg")),
    ("/favicon-dark.svg", include_str!("../assets/icon/favicon-dark.svg")),
];

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
<link rel="icon" type="image/svg+xml" href="/favicon-light.svg" media="(prefers-color-scheme: light)">
<link rel="icon" type="image/svg+xml" href="/favicon-dark.svg" media="(prefers-color-scheme: dark)">
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
.foot { margin-top:40px; padding-top:12px; border-top:1px solid var(--line); font-family:var(--mono); font-size:11px; color:var(--faint); }
.where { font-family:var(--mono); font-size:11px; color:var(--dim); text-transform:uppercase; letter-spacing:.08em; }
h1 { font-size:24px; font-weight:600; margin:6px 0 4px; letter-spacing:-.01em; } .sub { margin:0 0 20px; }
h1 .proj { color:var(--dim); font-weight:500; }
.sent { margin:0 0 16px; padding:9px 14px; border:1px solid var(--line); border-radius:10px; background:var(--panel); font-size:13px; color:var(--ink); }
.offers { margin:4px 0 14px; display:flex; flex-direction:column; gap:8px; } .offer-row { display:flex; gap:10px; align-items:flex-start; padding:10px 12px; background:var(--panel); border:1px solid var(--line-2); border-radius:10px; cursor:pointer; } .offer-row input { margin-top:3px; accent-color:var(--blue); width:16px; height:16px; flex:none; } .offer-row .says { display:block; font-size:14px; } .offer-row .then { display:block; margin-top:3px; font-size:12.5px; color:var(--dim); }
.reach { margin:0 0 14px; padding:12px 14px; background:var(--panel); border:1px solid var(--line-2); border-radius:10px; font-size:14px; } .reach code { font-family:var(--mono); font-size:.92em; } .reach .rmerge { display:flex; align-items:center; gap:12px; flex-wrap:wrap; margin-top:10px; } .reach .act.primary { background:var(--blue); color:#04121f; border:0; font-weight:600; } .reach .rfail { margin-top:8px; color:var(--amber); } .reach details.git { margin-top:4px; color:var(--dim); font-size:12.5px; } .reach details.git summary { list-style:none; cursor:pointer; } .reach details.git summary::-webkit-details-marker { display:none; }
.sent .proj { color:var(--dim); }
.tests-changed { margin:14px 0 10px; font-size:13.5px; } .tests-changed .r { display:flex; align-items:baseline; justify-content:space-between; gap:12px; }
.tests-changed code { font-family:var(--mono); font-size:.88em; } .tests-changed p.dim { color:var(--dim); font-size:12.5px; margin:6px 0 0; }
.tests-changed button { border:1px solid var(--line-2); border-radius:8px; padding:3px 12px; background:none; color:var(--ink); white-space:nowrap; cursor:pointer; }
pre.diff .fb { display:block; } pre.diff.only .fb:not(.on) { display:none; }
.badge { display:inline-flex; gap:6px; align-items:center; font-size:12px; padding:3px 10px; border-radius:999px; background:var(--green-soft); color:var(--green); }
.badge.wait { background:var(--amber-soft); color:var(--amber); }
.notice { background:var(--amber-soft); color:var(--amber); padding:10px 16px; font-size:13px; border-radius:8px; margin-bottom:14px; }
.notice.older { background:var(--panel); color:var(--ink); } .notice.older .ohead { font-size:15px; }
.odim { color:var(--dim); margin-top:6px; } .odim code { font-family:var(--mono); color:var(--ink); }
.notice.older button.act { margin-top:12px; }
button.copyplace, .moveall button.mv { background:var(--panel); border:1px solid var(--line-2); border-radius:8px; padding:3px 10px; font-size:12px; margin-left:6px; }
button.copyplace:hover, .moveall button.mv:hover { border-color:var(--blue); } .moveall button.mv { padding:5px 12px; margin:8px 6px 0 0; }
small.older { color:var(--dim); } button.linkish { background:none; border:0; padding:0; color:var(--dim); text-decoration:underline; text-transform:none; letter-spacing:0; font:inherit; cursor:pointer; }
.qh.og { padding-bottom:4px; } .ogall { padding:0 18px 8px; font-size:12px; } .moveall .mvb { display:block; }
.moveall { padding:0 18px 12px; font-size:13px; color:var(--ink); }
.say { display:flex; align-items:center; gap:10px; background:var(--panel); border:1px solid var(--line-2); border-radius:10px; padding:6px 12px; margin-bottom:6px; }
.say label { font-family:var(--mono); font-size:11px; color:var(--dim); text-transform:uppercase; letter-spacing:.08em; }
.say input { flex:1; background:transparent; border:0; color:var(--ink); font:inherit; font-size:14px; outline:none; padding:6px 0; }
.say .mode[hidden], .say .read[hidden] { display:none; } .say .mode { display:inline-flex; border:1px solid var(--line-2); border-radius:7px; overflow:hidden; flex:none; } .say .mode button { background:transparent; border:0; color:var(--faint); font:inherit; font-size:12.5px; padding:4px 10px; cursor:pointer; } .say .mode button.on { background:var(--panel-2); color:var(--ink); }
.say .go { background:var(--blue); color:#06101c; border:1px solid var(--blue); border-radius:8px; padding:6px 12px; font:inherit; font-size:13px; font-weight:600; cursor:pointer; white-space:nowrap; } .say .go:disabled { opacity:.5; }
.askline { font-size:12.5px; color:var(--dim); margin:0 0 12px 2px; } .askline.bad { color:var(--red); }
.thread { margin:0 0 14px; } .thread .th { font-size:12.5px; color:var(--dim); margin-bottom:6px; } .thread .tr { display:flex; align-items:center; gap:10px; font-size:11.5px; color:var(--faint); margin:8px 0 6px; } .thread .tr::after { content:""; flex:1; border-top:1px solid var(--line); }
.thread .te { background:var(--panel); border:1px solid var(--line); border-radius:9px; padding:8px 12px; margin:0 0 6px; font-size:13.5px; } .thread .te .who { font-size:12px; color:var(--faint); margin-bottom:2px; } .thread .te .tx { white-space:pre-wrap; }
.thread details.te > summary { cursor:pointer; list-style:none; color:var(--dim); } .thread details.te .who, .thread details.te .tx { display:block; } .thread details.te .tx { white-space:nowrap; overflow:hidden; text-overflow:ellipsis; } .thread details.te > summary::-webkit-details-marker { display:none; } .thread details.te[open] > summary { color:var(--ink); } .thread details.te[open] .tx { white-space:pre-wrap; }
.thread .te.new { border-color:var(--amber); } .thread .te.new .who { color:var(--amber); } .thread .te.wait { color:var(--dim); border-style:dashed; }
.stages small.talk { font-family:inherit; font-size:12.5px; color:var(--dim); white-space:normal; }
.say .read { background:var(--panel-2); color:var(--ink); border:1px solid var(--line-2); border-radius:8px; padding:6px 12px; font:inherit; font-size:13px; cursor:pointer; white-space:nowrap; }
.understood { background:var(--panel); border:1px solid var(--line-2); border-left:3px solid var(--blue); border-radius:10px; padding:12px 16px; margin:8px 0 12px; font-size:14px; }
.understood .label { font-family:var(--mono); font-size:11px; color:var(--dim); text-transform:uppercase; letter-spacing:.08em; margin-bottom:6px; }
.understood ul { margin:0 0 8px; padding-left:18px; } .understood .repeat { color:var(--amber); font-size:13px; margin:4px 0; } .understood .unsure { color:var(--amber); font-size:13px; margin:4px 0; }
.understood .btns { display:flex; gap:8px; margin-top:8px; } .understood .btns button { border-radius:8px; padding:7px 14px; font:inherit; font-size:13px; cursor:pointer; border:1px solid var(--line-2); background:var(--panel-2); color:var(--ink); }
.understood .btns .go { background:var(--blue); color:#06101c; border-color:var(--blue); font-weight:600; } .understood .btns .go:disabled { opacity:.4; cursor:default; }
.row { padding:16px 0; border-top:1px solid var(--line); } .row h3 { margin:0 0 4px; font-size:16px; font-weight:600; }
.seg { display:flex; flex-wrap:wrap; gap:6px; margin-top:8px; } details.each { margin:10px 0 0; } details.each > summary { cursor:pointer; font-size:13px; color:var(--dim); } details.each .row { margin-top:10px; }
.pick { display:flex; gap:8px; margin-top:10px; } .pick .adjust { flex:1; background:var(--panel); color:var(--ink); border:1px solid var(--line-2); border-radius:8px; padding:8px 11px; font:inherit; font-size:13.5px; }
.pick .fin { background:var(--panel); color:var(--dim); border:1px solid var(--line-2); border-radius:8px; padding:8px 14px; } .pick .fin.on { border-color:var(--amber); color:var(--amber); background:var(--amber-soft); }
.finsay { flex:1; align-self:center; font-size:13.5px; color:var(--dim); }
.pick .choose { background:var(--blue); color:#04121f; border:0; border-radius:8px; padding:8px 20px; font-weight:600; } .pick .choose:disabled { background:var(--line-2); color:var(--faint); }
.pick .choose.alt { background:var(--panel-2); color:var(--dim); border:1px solid var(--line-2); font-weight:500; } .pick .choose[hidden] { display:none; }
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
.strip i.todo { background:var(--line-2); } .strip i.gone { background:var(--faint); } .strip i.sampled { background:none; box-shadow:inset 0 0 0 1px var(--green); }
.stages { list-style:none; margin:8px 0 18px; padding:0; }
.stages li { display:grid; grid-template-columns:14px 1fr; gap:10px; padding:5px 0; position:relative; }
.stages li::before { content:""; position:absolute; left:6px; top:18px; bottom:-6px; width:1px; background:var(--line-2); }
.stages li:last-child::before { display:none; }
.stages .dot { width:9px; height:9px; border-radius:50%; margin:6px 0 0 2px; background:var(--green); position:relative; z-index:1; }
.stages .waiting .dot { background:var(--amber); box-shadow:0 0 0 4px var(--amber-soft); }
.stages .blocked .dot { background:var(--red); box-shadow:0 0 0 4px var(--red-soft); }
.stages .running .dot { background:var(--blue); box-shadow:0 0 0 4px var(--blue-soft); }
.stages .halted .dot { background:var(--faint); } .stages .halted small { color:var(--dim); }
.stages .gone .dot { background:var(--faint); } .stages .gone small { color:var(--faint); }
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
h1 code, .q code, .passed code, .wh code { font-family:var(--mono); font-size:.88em; font-weight:500; background:var(--panel-2); border:1px solid var(--line); border-radius:5px; padding:0 .3em; }
.whyyou { margin:0 0 8px; font-size:12.5px; color:var(--dim); }
.stop-note { margin:0 0 8px; font-size:12.5px; color:var(--dim); }
details.wants { margin:0 0 16px; } details.wants > summary { cursor:pointer; color:var(--blue); font-size:13px; } details.wants .wh { margin:10px 0 8px; font-size:12.5px; color:var(--dim); }
.seg.says { flex-direction:column; align-items:flex-start; gap:8px; } .seg.says .opt { display:flex; align-items:baseline; gap:12px; } .seg.says button { min-width:84px; } .seg.says .says { font-size:12px; color:var(--faint); }
pre.diff { margin:0; padding:10px 0; border:1px solid var(--line); border-radius:6px; overflow-x:auto; font-family:var(--mono); font-size:12px; line-height:1.55; color:var(--dim); white-space:pre; }
/* each line, also inside a file's block (See them shows only some files): a block of its own, never glued to the next */
:where(.diff, .diff .fb) > span:not(.fb) { display:block; padding:0 14px; min-width:max-content; }
:where(.diff, .diff .fb) > span > i { font-style:normal; display:inline-block; width:2ch; color:var(--faint); user-select:none; }
:where(.diff, .diff .fb) > .add, :where(.diff, .diff .fb) > .del { color:var(--ink); }
:where(.diff, .diff .fb) > .add { background:var(--green-soft); } :where(.diff, .diff .fb) > .add > i { color:var(--green); }
:where(.diff, .diff .fb) > .del { background:var(--red-soft); } :where(.diff, .diff .fb) > .del > i { color:var(--red); }
:where(.diff, .diff .fb) > .hunk { color:var(--blue); }
:where(.diff, .diff .fb) > .file { color:var(--ink); font-weight:600; margin-top:12px; } .diff > .file:first-child, .diff > .fb:first-child > .file { margin-top:0; }
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
.proj { color:var(--dim); } .sep, .apart { color:var(--faint); }
.nm { display:flex; min-width:0; white-space:pre; } .nm .proj, .nm .sep, .nm .apart { flex:none; } .nm .tt { min-width:0; overflow:hidden; text-overflow:ellipsis; }
.req { margin:0 0 12px; color:var(--dim); font-size:14px; white-space:pre-line; } .req summary { list-style:none; cursor:pointer; } .req summary::-webkit-details-marker { display:none; }
.req .clamp { display:-webkit-box; -webkit-line-clamp:2; -webkit-box-orient:vertical; overflow:hidden; } .req[open] .clamp { display:block; -webkit-line-clamp:unset; }
.req .more { color:var(--blue); font-size:13px; } .req[open] .more { display:none; }
.notice.passed-q { background:var(--panel); color:var(--dim); }
.passed { display:flex; align-items:center; justify-content:space-between; gap:12px; margin:14px 0 6px; padding:8px 0; font-size:13px; color:var(--dim); border-top:1px solid var(--line); }
.passed.settled small { display:block; margin-top:3px; font-size:12px; color:var(--faint); }
.rides { margin:14px 0 10px; } .rides .ride { display:flex; align-items:baseline; gap:12px; padding:6px 0; font-size:13.5px; border-top:1px solid var(--line); }
.rides .st { color:var(--dim); white-space:nowrap; } .rides .ride span:nth-child(2) { flex:1; }
.rides .ride a { border:1px solid var(--line-2); border-radius:8px; padding:3px 12px; color:var(--ink); white-space:nowrap; }
.rides p.dim { color:var(--dim); font-size:12.5px; margin:8px 0 0; }
.passed span { color:var(--dim); } .passed a { border:1px solid var(--line-2); border-radius:8px; padding:4px 12px; color:var(--ink); white-space:nowrap; }
.notice.bad { background:var(--red-soft); color:var(--red); } .notice.blocked { color:var(--ink); } .notice.blocked > b { color:var(--red); } .notice .cases { margin-top:10px; color:var(--ink); } .notice .cases .ch { color:var(--dim); font-size:12px; margin-bottom:4px; } .notice .case { background:var(--panel); border:1px solid var(--line-2); border-radius:8px; padding:10px 12px; margin-top:6px; } .notice .case .by { display:block; color:var(--dim); font-size:12px; } .notice .case p { margin:4px 0 0; } .notice .case code { color:var(--ink); } .notice .case pre { margin:6px 0 0; font-family:var(--mono); font-size:12px; color:var(--faint); white-space:pre-wrap; } .notice .case details.all summary { list-style:none; cursor:pointer; color:var(--dim); font-size:12px; margin-top:4px; } .notice .case details.all summary::-webkit-details-marker { display:none; } .notice .case .fi { color:var(--dim); font-size:12px; } .notice .way { margin-top:8px; } .notice .way code { color:var(--ink); } .notice .then { display:block; color:var(--dim); margin-top:2px; } .notice details.still { margin-top:4px; color:var(--dim); } .notice details.still summary { cursor:pointer; } .notice details.still ul { margin:4px 0 0 18px; padding:0; } .notice.ok { background:var(--green-soft); color:var(--green); }
.steps { width:100%; border-collapse:collapse; font-size:13.5px; margin:8px 0 18px; } .steps th { text-align:left; font-family:var(--mono); font-size:11px; color:var(--dim); font-weight:400; text-transform:uppercase; letter-spacing:.06em; padding:6px 8px; border-bottom:1px solid var(--line-2); }
.steps td { padding:7px 8px; border-bottom:1px solid var(--line); }
.acts { display:flex; flex-wrap:wrap; gap:8px; } .act { background:var(--panel); border:1px solid var(--line-2); border-radius:8px; padding:7px 14px; font-size:13px; }
.answer code { font-family:var(--mono); font-size:.9em; } .answer { display:inline-block; margin:2px 0 14px; background:var(--blue); color:#04121f; border-radius:8px; padding:9px 18px; font-weight:600; text-decoration:none; }
.act:hover { border-color:var(--blue); } .act.danger { color:var(--red); } .act.armed { border-color:var(--red); background:var(--red-soft); }
select { background:var(--panel-2); color:var(--ink); border:1px solid var(--line-2); border-radius:6px; padding:5px; font:inherit; font-size:13px; }
.effects { position:fixed; left:280px; right:0; bottom:0; background:rgba(14,18,24,.97); border-top:1px solid var(--line-2); padding:14px 32px; display:flex; gap:24px; align-items:flex-start; }
.effects.hide { display:none; } .effects .what { flex:1; max-width:700px; } .label { font-family:var(--mono); font-size:11px; color:var(--dim); text-transform:uppercase; letter-spacing:.08em; margin-bottom:4px; }
.effects ul { margin:0; padding-left:18px; font-size:13.5px; } .refused { color:var(--red); font-size:13px; margin-top:6px; } .refused.hint { color:var(--faint); }
/* at the merge, what lands has a ceiling and scrolls inside, its bottom edge faded while more is below, so the pair
   and File by file show without scrolling the page (the design-lead, 23/11) */
.lands pre.diff { max-height:max(180px, 36vh); overflow:auto; } .lands .changes { position:relative; }
.lands.more .changes::after { content:""; position:absolute; left:1px; right:1px; bottom:1px; height:40px; border-radius:0 0 6px 6px; background:linear-gradient(rgba(7,9,13,0), var(--bg)); pointer-events:none; }
.send { background:var(--blue); color:#04121f; border:0; border-radius:10px; padding:10px 22px; font-weight:600; } .send:disabled { background:var(--line-2); color:var(--faint); cursor:default; }
</style></head><body>
<div class="top"><b>Jaynes</b><span style="color:var(--dim);font-size:13px">the new engine</span><nav class="nav"><a href="/home">Home</a><a href="/new">New run</a><a href="/setup">Setup</a></nav></div>
<div class="wrap"><aside>{{aside}}</aside><main>{{who}}{{main}}<footer class="foot">Jaynes {{version}}</footer></main></div>
<div class="effects" id="fxbar"><div class="what"><div class="label">What this does · written by Jaynes, not by the role</div><ul id="fx"></ul><div class="refused" id="refused"></div></div><button class="send" id="send">Send</button></div>
<script src="/key.js"></script><script src="/page.js"></script></body></html>"##;

#[cfg(test)]
mod tests {
    use super::*;

    // The engineer lead's T5 (23/11): what a stop offers beside the answer is a box on its page, as in the app, with
    // the core's words and, dim under them, what granting it does; marked as the step said, and at a stop that
    // chooses, as each option says (data-for), until the person sets it. Each marked box goes as the act "grant"
    // before the answer, the person's alone.
    #[test]
    fn asked_a_stop_s_page_shows_what_it_offers_as_a_box_granted_before_the_answer() {
        use crate::stop::{Offered, TEST_SERVERS_SAYS, TEST_SERVERS_THEN};
        assert_eq!(offers_html(&[]), "", "nothing offered, no box");
        let o = |marked: bool, marked_for: &[(&str, bool)]| Offered {
            offer: jaynes_core::manifest::Offer::TestServers,
            says: TEST_SERVERS_SAYS.into(),
            then: TEST_SERVERS_THEN.into(),
            marked,
            marked_for: marked_for.iter().map(|(i, m)| (i.to_string(), *m)).collect(),
        };
        let on = offers_html(&[o(true, &[])]);
        assert_eq!(
            on,
            format!(
                "<div class=\"offers\"><label class=\"offer-row\"><input type=\"checkbox\" class=\"offer\" data-offer=\"test_servers\" checked>\
                 <span><span class=\"says\">{}</span><span class=\"then\">{}</span></span></label></div>",
                esc(TEST_SERVERS_SAYS),
                esc(TEST_SERVERS_THEN)
            )
        );
        let off = offers_html(&[o(false, &[("go", true), ("node", false)])]);
        assert!(off.contains(r#"data-offer="test_servers" data-for="{&quot;go&quot;:true,&quot;node&quot;:false}">"#), "{off}");
        assert!(!off.contains("checked"), "unmarked: {off}");
        // the script grants each marked box first, then the answer; a refused grant stops the answer
        assert!(SCRIPT.contains("async function answerWith(body)"), "one way to send an answer");
        assert_eq!(SCRIPT.matches("fetch(\"/api/answer\"").count(), 1, "every answer goes through answerWith");
        assert!(!SCRIPT.contains("post(\"/api/answer\")"), "the Send too");
        assert!(SCRIPT.contains("action: \"grant\""), "the act");
    }
    use jaynes_core::question::QuestionKind;

    /// The reason the core keeps for a check the box kept from a local port, as listing's tests build it.
    fn refused_port(lines: &str, tried: &str) -> String {
        let not_given = crate::project::not_given(&[".claude/".to_string()]).expect("a sentence");
        format!(
            "the check suite-passes of change could not judge: the box refused what the command tried, so its failure says nothing about the project: {lines} It tried local port {tried}. A check's commands bind and reach only the ports in JAYNES_PORTS (65520, 65521, 65522, 65523, 65524, 65525, 65526, 65527) and the ones the check names in its local_ports, never its step's. Two ways out, the first the better: (1) the suite takes its test servers' ports from JAYNES_PORTS, and the box stays closed; (2) the person opens this project with `jaynes project open-ports /p`, and its checks then reach every service listening on this machine, the Jaynes page among them.; {not_given}. No attempt was spent"
        )
    }

    // the rd-engineer, 22/11: the card for a port Jaynes holds says under its way "Only if nothing of yours runs on
    // 5432 while the tests run: a test there would answer in its place."; the page drew only what a way says and its
    // command, so the warning, and what stays open, never reached the page
    #[test]
    fn the_page_draws_each_way_out_whole_with_what_follows_and_what_stays_open() {
        let panic = "panic: httptest: failed to listen on a port: listen tcp6 [::1]:0: bind: operation not permitted";
        for reason in [refused_port("dial tcp 127.0.0.1:5432: connect: operation not permitted", "5432"), refused_port(panic, "0, a port of its own choosing")] {
            let said = crate::listing::blocked_said(Some("change"), jaynes_core::event::Fault::Jaynes, &reason);
            let page = blocked_notice(&crate::listing::blocked_said(Some("change"), jaynes_core::event::Fault::Jaynes, &reason));
            assert!(said.ways.iter().any(|w| w.then.is_some()), "a way with what follows: {:?}", said.ways);
            for w in &said.ways {
                assert!(page.contains(&esc(&w.says)), "{}", w.says);
                if let Some(c) = &w.command {
                    assert!(page.contains(&format!("<code>{}</code>", esc(c))), "{c}");
                }
                if let Some(t) = &w.then {
                    assert!(page.contains(&format!("<span class=\"then\">{}</span>", esc(t))), "what follows: {t}");
                }
                if !w.still_open.is_empty() {
                    assert!(page.contains(&format!("<summary>{}</summary>", esc(crate::listing::STILL_OPEN_HEADING))), "the heading");
                    w.still_open.iter().for_each(|l| assert!(page.contains(&format!("<li>{}</li>", esc(l))), "still open: {l}"));
                }
            }
        }
    }

    // Alan, 20/11, in the video of the choice screen: "o que são esses pins?" (the design-lead's checkup, item 14): the
    // star became "☆ Shortlist", saying what it does, on the page as in the app (compare.rs asked_what_are_these_pins);
    // the page has no parts to keep, so no pin of its own
    #[test]
    fn while_jaynes_updates_the_notice_says_what_update_does() {
        // the notice offers Update only for a release built for this machine, and Update downloads it and installs it
        assert_eq!(updating_says(Some("0.2.5")), "Jaynes is updating: it downloads Jaynes 0.2.5 and installs it.");
        // with no release known for this machine, `jaynes update` builds the newest source with cargo
        let said = updating_says(None);
        assert_eq!(said, "Jaynes is updating: no release is built for this machine, so cargo builds the newest source, which takes a few minutes.");
    }

    // Alan, 20/11, in the video of the choice screen: "o que são esses pins?" (the design-lead's checkup, item 14): the
    // star became "☆ Shortlist", saying what it does, on the page as in the app (compare.rs asked_what_are_these_pins);
    // the page has no parts to keep, so no pin of its own
    #[test]
    fn asked_what_are_these_pins_the_page_shortlists_in_the_app_s_words() {
        let b = shortlist_button("harbor");
        assert!(b.contains(">☆ Shortlist<") && b.contains("data-fin=\"harbor\"") && !b.to_lowercase().contains("finalist"), "{b}");
        assert!(FINALS_ROW.contains(">Develop the shortlisted<"), "{FINALS_ROW}");
        // the note the agent gets is the engine's, which says the option's sentence ("“Harbor” is shortlisted: …")
        assert!(SCRIPT.contains(&format!("\"{}\"", jaynes_core::decide::SHORTLISTED)), "the note");
        assert!(SCRIPT.contains("★ Shortlisted") && SCRIPT.contains("☆ Shortlist") && SCRIPT.contains(" shortlisted: "));
        for old in ["A finalist", "finalists:", "the finalists", ">Finalist<"] {
            assert!(!SCRIPT.contains(old) && !FINALS_ROW.contains(old), "{old}");
        }
    }

    #[test]
    fn the_update_notice_names_the_version_installed_or_its_commit_when_it_cannot() {
        assert_eq!(updated_to(Some("0.2.4"), "0.2.3", "3bf0783abcdef"), "Jaynes was updated to 0.2.4.");
        // no version said (an older program), or the same as this page's (a build between versions): the commit
        assert_eq!(updated_to(None, "0.2.3", "3bf0783abcdef"), "Jaynes was updated to 3bf0783.");
        assert_eq!(updated_to(Some("0.2.3"), "0.2.3", "3bf0783abcdef"), "Jaynes was updated to 3bf0783.");
        assert_eq!(updated_to(None, "0.2.3", "3bf07"), "Jaynes was updated to 3bf07.");
    }

    #[test]
    fn a_tab_counts_as_open_for_three_minutes_after_its_last_request_with_the_key() {
        let now = std::time::Instant::now() + std::time::Duration::from_secs(3600);
        let ago = |s: u64| Some(now - std::time::Duration::from_secs(s));
        assert!(!tab_open_at(None, now), "no request with the key: no tab");
        assert!(tab_open_at(ago(0), now));
        // a hidden tab, which the browser lets ask once a minute, still counts
        assert!(tab_open_at(ago(61), now) && tab_open_at(ago(179), now));
        assert!(!tab_open_at(ago(180), now) && !tab_open_at(ago(600), now));
    }

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
        let start = Input::Start { run: "r1".into(), request: "x".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let mut events = decide(None, &start).expect("start").events;
        let question = Question { rides: Vec::new(),
            id: "q1".into(), step: "plan".into(), round: 0, kind: QuestionKind::Decision, items: vec![Item { named: None, id: "a".into(), label: "a".into(), digest: Digest("d".into()) }],
            offered: vec![VerdictKind::Keep], holds: true, about: String::new(), choose: false, pass: 0, passes: 0, back: vec![],
        };
        events.push(Event::QuestionOpened { question });
        Store::open(&home.join("record.sqlite3")).expect("store").append("r1", &events).expect("append");
        home
    }

    /// Two runs, each waiting on its question q1, which the person was told of.
    fn told(name: &str) -> PathBuf {
        told_with(name, |_| json!({}))
    }

    /// As `told`, each run's setup from `setup`.
    fn told_with(name: &str, setup: impl Fn(&str) -> serde_json::Value) -> PathBuf {
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
            let start = Input::Start { run: run.into(), request: format!("the request of {run}"), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
            let mut events = decide(None, &start).expect("start").events;
            let question = Question { rides: Vec::new(),
                id: "q1".into(), step: "plan".into(), round: 0, kind: QuestionKind::Decision, items: vec![Item { named: None, id: "a".into(), label: "a".into(), digest: Digest("d".into()) }],
                offered: vec![VerdictKind::Keep], holds: true, about: "Is the plan right?".into(), choose: false, pass: 0, passes: 0, back: vec![],
            };
            events.push(Event::QuestionOpened { question });
            events.push(Event::Waiting { step: Some("plan".into()), on: vec!["q1".into()], why: String::new() });
            let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
            store.append(run, &events).expect("append");
            store.put_setup(run, &setup(run)).expect("setup");
        }
        home
    }

    /// A run waiting at a stop of "stack" that chooses among what its agent made: `labels`, one option or more.
    fn a_choice_of(run: &str, labels: &[&str]) -> PathBuf {
        use jaynes_core::question::Item;
        use jaynes_core::{AttemptOutcome, Command, Digest, Manifest};
        let home = std::env::temp_dir().join(format!("jaynes-page-choice-{run}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("runs").join(run)).expect("home");
        let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
        let manifest: Manifest = serde_json::from_value(json!({
            "role": "r", "version": "1", "protocol": 1,
            "steps": [{"name": "stack", "executor": "agent", "stop": {"verdicts": ["keep", "change", "drop"], "holds": true, "choose": true, "about": "Which stack goes on?"}}],
        }))
        .expect("manifest");
        let start = Input::Start { run: run.into(), request: "Choose the stack".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let out = decide(None, &start).expect("start");
        let mut events = out.events.clone();
        let mut r = Run::fold(&out.events).expect("run");
        let mut feed = |r: &mut Run, input: Input| -> Vec<Command> {
            let out = decide(Some(r), &input).unwrap_or_else(|e| panic!("refused {input:?}: {e}"));
            out.events.iter().for_each(|e| r.apply(e));
            events.extend(out.events);
            out.commands
        };
        let cmds = feed(&mut r, Input::ProbeEnded { step: "stack".into(), ok: true, reason: String::new() });
        let t = cmds.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.clone()) } else { None }).expect("a turn");
        let items = labels.iter().enumerate().map(|(n, l)| Item { named: None, id: format!("o{n}"), label: (*l).into(), digest: Digest(format!("d{n}")) }).collect();
        let outcome = AttemptOutcome::Delivered { output: Digest("o".into()), items, changed: vec![], usage: Default::default(), session: Some("s1".into()), tree: None, reply: None };
        let cmds = feed(&mut r, Input::AttemptEnded { step: "stack".into(), attempt: t.attempt, fix: t.fix, outcome });
        if cmds.iter().any(|c| matches!(c, Command::RunChecks { .. })) {
            feed(&mut r, Input::ChecksEnded { step: "stack".into(), attempt: t.attempt, fix: t.fix, results: vec![] });
        }
        if let Some(digest) = r.delivering.clone() {
            feed(&mut r, Input::DeliveryEnded { digest, ok: true, reference: format!("jaynes/{run}"), merged: None });
        }
        store.append(run, &events).expect("append");
        store.put_setup(run, &json!({})).expect("setup");
        home
    }

    // Alan, 21/11, at a choice with one option, the stack: "entender onde o python entra antes de decidir" became a
    // change and the engine said "is chosen": with one option the page asks go with it or not yet (uma-opcao.md)
    #[test]
    fn asked_one_option_with_words_is_not_chosen_yet_on_the_page() {
        let home = a_choice_of("one", &["Python + FastAPI: small and known"]);
        let page = Page { home: home.clone(), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        let now = crate::store::now();
        let run = Store::open(&home.join("record.sqlite3")).expect("store").state("one").expect("read").expect("run");
        let q = run.questions.keys().next().expect("the stop").clone();
        let html = page.render(View::Question("one".into(), q.clone()), now).expect("the stop");
        assert!(html.contains("<h1>Go with “Python + FastAPI”?</h1>"), "the title asks yes or not yet");
        assert!(html.contains("<input class=\"adjust\" data-i=\"o0\" placeholder=\"Not sure yet? Ask the agent, or say what to change\">"), "the field");
        assert!(html.contains("<button class=\"choose\" data-choose=\"o0\">Go with this one</button>"), "the button, field empty");
        assert!(!html.contains("Choose this one") && !html.contains("class=\"fin\"") && !html.contains("id=\"finals\""), "nothing to choose among, nothing to shortlist");
        assert!(html.contains(">Not this one<"), "not this one stays");
        // the design-lead, 22/11, at the photo: the option's field is the stop's words, so no second field above it;
        // with words a grey "Go with this one" beside goes with it anyway, the words as context, as the app does; and
        // the engine's sentences under the button in their order, "is not chosen yet" first, nothing folded, since
        // with one there is no "each option"
        assert!(!html.contains("id=\"ctx\""), "one field of words");
        assert!(html.contains("<button class=\"choose alt\" data-keep=\"o0\" hidden>Go with this one</button>"), "go with it anyway");
        assert!(SCRIPT.contains("alt.hidden = !e.target.value.trim();"), "beside the button once there are words");
        assert!(SCRIPT.contains("function keepWith(id) {"), "keep, the words as context");
        assert!(SCRIPT.contains("if (base.items.length === 1) { if (ul) ul.innerHTML = [...each, ...next].map(s => `<li>${esc(s)}</li>`).join(\"\")"), "in order, not folded");
        // with words the button sends them and chooses nothing, and the engine says so
        assert!(SCRIPT.contains("b.textContent = e.target.value.trim() ? \"Not yet: send my words\" : \"Go with this one\";"), "the button with words");
        let body = json!({"run": "one", "question": q, "verdicts": {"o0": "change"}, "notes": {"o0": "entender onde o python entra antes de decidir"}, "context": ""});
        let fx = page.effects(&body, By::Person).expect("effects");
        let said: Vec<&str> = fx["sentences"].as_array().expect("sentences").iter().filter_map(|s| s.as_str()).collect();
        assert!(said.iter().any(|s| s.contains("is not chosen yet")) && !said.iter().any(|s| s.contains("is chosen")), "{said:?}");
        let _gone = std::fs::remove_dir_all(&home);
        // two options read as before
        let home = a_choice_of("two", &["Python + FastAPI", "Go + Echo"]);
        let page = Page { home: home.clone(), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        let run = Store::open(&home.join("record.sqlite3")).expect("store").state("two").expect("read").expect("run");
        let q = run.questions.keys().next().expect("the stop").clone();
        let html = page.render(View::Question("two".into(), q), now).expect("the stop");
        assert!(html.contains(">Choose this one<") && html.contains("class=\"fin\"") && html.contains("placeholder=\"Adjust it (optional): what to change, what to take from the others\""), "two as before");
        assert!(html.contains("id=\"ctx\"") && !html.contains("data-keep="), "two keep the stop's field");
        let _gone = std::fs::remove_dir_all(&home);
    }

    /// A run waiting at plan's stop, whose agent said what it made, where the person asked it twice: "e se fosse Go só
    /// na API?", answered, then "and the tests?", which the agent is answering.
    fn a_talk_at_a_stop() -> PathBuf {
        use jaynes_core::question::Item;
        use jaynes_core::{AttemptOutcome, Command, Digest, Manifest};
        let home = std::env::temp_dir().join(format!("jaynes-page-talk-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("runs/talk")).expect("home");
        let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
        let manifest: Manifest = serde_json::from_value(json!({
            "role": "r", "version": "1", "protocol": 1,
            "steps": [{"name": "plan", "executor": "agent", "stop": {"verdicts": ["keep", "redo"], "holds": true, "about": "Is the plan right?"}}],
        }))
        .expect("manifest");
        let start = Input::Start { run: "talk".into(), request: "Choose the stack".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let out = decide(None, &start).expect("start");
        let mut events = out.events.clone();
        let mut run = Run::fold(&out.events).expect("run");
        let mut feed = |run: &mut Run, input: Input| -> Vec<Command> {
            let out = decide(Some(run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
            out.events.iter().for_each(|e| run.apply(e));
            events.extend(out.events);
            out.commands
        };
        let cmds = feed(&mut run, Input::ProbeEnded { step: "plan".into(), ok: true, reason: String::new() });
        let t = cmds.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.clone()) } else { None }).expect("a turn");
        let item = Item { named: None, id: "a".into(), label: "Python, Postgres and Qdrant".into(), digest: Digest("d-a".into()) };
        let outcome = AttemptOutcome::Delivered { output: Digest("o".into()), items: vec![item], changed: vec![], usage: Default::default(), session: Some("s1".into()), tree: None, reply: Some("One Python package over Postgres and Qdrant.".into()) };
        let cmds = feed(&mut run, Input::AttemptEnded { step: "plan".into(), attempt: t.attempt, fix: t.fix, outcome });
        if cmds.iter().any(|c| matches!(c, Command::RunChecks { .. })) {
            feed(&mut run, Input::ChecksEnded { step: "plan".into(), attempt: t.attempt, fix: t.fix, results: vec![] });
        }
        if let Some(digest) = run.delivering.clone() {
            feed(&mut run, Input::DeliveryEnded { digest, ok: true, reference: "jaynes/talk".into(), merged: None });
        }
        let q = run.questions.keys().next().expect("the stop").clone();
        let ask = |text: &str| Input::AskAgent { question: q.clone(), item: None, text: text.into(), by: By::Person };
        feed(&mut run, ask("e se fosse Go só na API?"));
        let first = run.asking.keys().next().expect("asking").clone();
        feed(&mut run, Input::AskEnded { ask: first, outcome: jaynes_core::io::AskOutcome::Answered { text: "Daria, mas o ganho é pequeno. Não mudei nada.".into(), usage: Default::default(), session: None } });
        feed(&mut run, ask("and the tests?"));
        store.append("talk", &events).expect("append");
        store.put_setup("talk", &json!({})).expect("setup");
        home
    }

    // Alan, 21/11: "eu queria uma thread de conversa com a jaynes e nao que minha pergunta sumisse"
    // the design-lead, 22/11: "after the stop" did not say whether the stop had closed; "could not answer" as the story
    // says it
    #[test]
    fn a_late_answer_and_a_failed_ask_say_what_happened() {
        use jaynes_core::run::{Entry, EntryKind, StepState};
        let entry = |kind, late| Entry { round: 0, question: "q1".into(), kind, text: "t".into(), item: None, late, ask: Some("a1".into()) };
        assert_eq!(entry_who(&entry(EntryKind::Answer, true), false), "Agent · answered after the stop closed · nothing changed");
        assert_eq!(entry_who(&entry(EntryKind::AskFailed, false), false), "Agent · could not answer");
        let step = StepState { thread: vec![entry(EntryKind::Answer, true), entry(EntryKind::AskFailed, false)], ..Default::default() };
        assert_eq!(thread_story(&step), "<small class=\"talk\">agent answered after the stop closed, nothing changed: t</small><small class=\"talk\">agent could not answer: t</small>");
    }

    #[test]
    fn asked_a_thread_of_conversation_at_the_stop_on_the_page() {
        let home = a_talk_at_a_stop();
        let page = Page { home: home.clone(), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        let now = crate::store::now();
        let run = Store::open(&home.join("record.sqlite3")).expect("store").state("talk").expect("read").expect("run");
        let (q, second) = (run.questions.keys().next().expect("the stop").clone(), run.asking.keys().next().expect("asking").clone());
        let asked = page.render(View::Question("talk".into(), q.clone()), now).expect("the stop");
        // the conversation at the step, above where the person writes
        let (talk, field) = (asked.find("Conversation at “plan”").expect("the thread"), asked.find("id=\"ctx\"").expect("the field"));
        assert!(talk < field, "above the field");
        // the earlier exchange in a line each, the newest whole, and the agent answering it
        assert!(asked.contains("<details class=\"te\"><summary><span class=\"who\">You · round 1 · a question</span><span class=\"tx\">e se fosse Go só na API?</span></summary>"), "the first question, folded");
        assert!(asked.contains("<span class=\"who\">Agent · answered · nothing redone</span><span class=\"tx\">Daria, mas o ganho é pequeno. Não mudei nada.</span></summary>"), "its answer, folded");
        // the design-lead, 22/11: the label always above the text, folded or whole, never two drawings
        assert!(asked.contains(".thread details.te .who, .thread details.te .tx { display:block; }"), "the label above, folded too");
        assert!(asked.contains("<div class=\"te\"><div class=\"who\">You · round 1 · a question</div><div class=\"tx\">and the tests?</div></div>"), "the newest question, whole");
        assert!(asked.contains("<div class=\"te wait\">The agent is answering. Nothing is redone.</div>"), "the agent answering");
        assert!(asked.contains("Your question went to the agent at “plan”. It answers here; nothing is redone and nothing is chosen."), "under the field while it answers");
        // the field asks or changes, and asking says what it does
        assert!(asked.contains("<button data-mode=\"ask\">Question</button><button data-mode=\"change\">Change</button>"), "the switch");
        assert!(asked.contains("<button class=\"go\" id=\"ask\" hidden>Ask the agent ↵</button>"), "the ask button");
        // the switch shows only once the field has text (fio.md 1), Question puts "Ask the agent" where "Read my words"
        // was, and the ask goes as the person's: it waits for the key a link brought, as an answer does
        assert!(asked.contains(".say .mode[hidden], .say .read[hidden] { display:none; }"), "nothing overrides hidden");
        let script = &SCRIPT[SCRIPT.find("async function askAgent").expect("the ask")..];
        assert!(script[..script.find("fetch(").expect("its post")].contains("await window.jaynesKey"), "the ask waits for the key");
        assert!(SCRIPT.contains("document.getElementById(\"read\").hidden = mode === \"ask\";"), "one button at a time");
        // a question is not context: what the answer says and sends leaves it out, and turning the switch says so again
        assert!(SCRIPT.contains("const ctxOf = () => mode === \"ask\" ? \"\" : (document.getElementById(\"ctx\") || {}).value || \"\";"), "a question is not context");
        assert!(SCRIPT.contains("back, context: ctxOf() });"), "Send reads the same");
        assert!(SCRIPT.contains("if (b.dataset.mode) { picked = true; mode = b.dataset.mode; showMode(); if (base && !base.choose) refresh(); return; }"), "the switch redraws what it does");
        // once it answered: its answer whole and in amber, nothing answering, and under the field what asking does
        Store::open(&home.join("record.sqlite3")).expect("store").append("talk", &[jaynes_core::Event::AgentAnswered { ask: second, text: "They stay in Python.".into(), usage: Default::default(), session: None, late: false }]).expect("answered");
        let answered = page.render(View::Question("talk".into(), q.clone()), now).expect("the stop");
        assert!(answered.contains("<div class=\"te new\"><div class=\"who\">Agent · answered · nothing redone</div><div class=\"tx\">They stay in Python.</div></div>"), "the answer, new");
        assert!(!answered.contains("The agent is answering."), "nothing answering");
        assert!(answered.contains("<div class=\"askline\" id=\"askline\" hidden>The agent answers here. Nothing is redone and nothing is chosen.</div>"), "what asking does");
        // the run's story has the same entries, one line each
        let run = page.render(View::Run("talk".into()), now).expect("the run");
        assert!(run.contains("you asked: e se fosse Go só na API?") && run.contains("agent answered, nothing redone: Daria, mas o ganho é pequeno."), "the story");
        // the design-lead, 22/11: the request shows under the title only when it says more (nome-da-run.md), and a
        // count of zero is left out (the tech-lead's counters): "1 attempt by the agent", not "…, 0 sent back"
        assert!(!run.contains("<p class=\"req\">Choose the stack</p>"), "the request said once");
        assert!(run.contains("Spent 0 s of 20 min, 0 of 2,000,000 tokens · 1 attempt by the agent</p>"), "no count of zero, and thousands as the summary writes them");
        // the page's ask goes to the engine as the person's; anyone else is refused by it
        let ask = json!({"run": "talk", "action": "ask", "question": q, "text": "and Rust?"});
        assert_eq!(act_in(&home, &page.exe, &ask, By::Agent { who: "x".into() }).err().as_deref(), Some("only you ask the agent at a stop"), "only the person asks");
        assert_eq!(act_in(&home, &page.exe, &ask, By::Person).expect("asked")["recorded"], "now");
        let taken = Store::open(&home.join("record.sqlite3")).expect("store").inbox_take("talk").expect("inbox");
        assert!(matches!(taken, Some(Input::AskAgent { ref question, ref text, item: None, by: By::Person }) if *question == q && text == "and Rust?"), "the ask waits for the run: {taken:?}");
        let _gone = std::fs::remove_dir_all(&home);
    }

    /// A home with three runs at a leave the agent asked: "new" to write tests/conftest.py, which does not exist yet,
    /// in a run that merges; "edit" to change src/a.py; "peek" to read /etc/hosts, with nothing to write.
    /// A folder of its own for each test that calls it (`test`): two tests in one folder at once removed each
    /// other's record (asked_whose_stop_waits fell with "run new exists").
    fn at_a_leave(test: &str) -> PathBuf {
        use jaynes_core::question::{Item, Question, VerdictKind};
        use jaynes_core::{Digest, Event, Manifest};
        let home = std::env::temp_dir().join(format!("jaynes-page-leave-{test}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("home");
        let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
        for (run, about, wants) in [
            ("new", "change tests/conftest.py", Some(crate::stop::Wants { file: "tests/conftest.py".into(), before: None, after: "import pytest\n".into() })),
            ("edit", "change src/a.py", Some(crate::stop::Wants { file: "src/a.py".into(), before: Some("x = 1\ny = 1\n".into()), after: "x = 2\ny = 1\n".into() })),
            ("peek", "read /etc/hosts", None),
        ] {
            std::fs::create_dir_all(home.join("runs").join(run)).expect("dir");
            let manifest: Manifest = serde_json::from_value(json!({
                "role": "build", "version": "1", "protocol": 1, "delivery": "merge",
                "steps": [{"name": "build", "executor": "agent"}],
            }))
            .expect("manifest");
            let start = Input::Start { run: run.into(), request: format!("Build the {run} thing"), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
            let mut events = decide(None, &start).expect("start").events;
            events.push(Event::QuestionOpened {
                question: Question { rides: Vec::new(),
                    id: "q1".into(), step: "build".into(), round: 1, kind: QuestionKind::Permission,
                    items: vec![Item { named: None, id: "leave".into(), label: about.into(), digest: Digest(format!("d-{run}")) }],
                    offered: vec![VerdictKind::Keep, VerdictKind::Drop], holds: true, about: about.into(), choose: false, pass: 0, passes: 0, back: vec![],
                },
            });
            store.append(run, &events).expect("append");
            // one leave of a named project, the others of none
            store.put_setup(run, &if run == "new" { json!({"project": "/w/tidepool"}) } else { json!({}) }).expect("setup");
            if let Some(w) = wants {
                crate::permit::keep_wants(&home, run, "q1", &w).expect("kept");
            }
        }
        home
    }

    /// What the person sees of a page: its html without the script and the style, which may hold backquotes of their own.
    fn seen(html: &str) -> String {
        let (mut out, mut rest) = (String::new(), html);
        while let Some(at) = [rest.find("<script"), rest.find("<style")].into_iter().flatten().min() {
            out.push_str(&rest[..at]);
            let close = if rest[at..].starts_with("<script") { "</script>" } else { "</style>" };
            rest = rest[at..].find(close).map_or("", |e| &rest[at + e + close.len()..]);
        }
        out + rest
    }

    // Alan, 21/11: "pq to sendo chamado aqui? a role?"
    #[test]
    fn asked_permission_says_why_you_on_the_page() {
        let home = at_a_leave("asked_permission_says_why_you_on_the_page");
        let page = Page { home: home.clone(), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        let now = crate::store::now();
        let asked = page.render(View::Question("new".into(), "q1".into()), now).expect("the leave");
        // the title asks in words, the file in mono, never a backquote in what the person reads
        assert!(asked.contains(" · May the agent change <code>tests/conftest.py</code>?</h1>"), "the title, after the project, the file in mono");
        assert!(!seen(&asked).contains('`'), "no backquote anywhere on the page");
        assert!(asked.contains("<p class=\"whyyou\">Why you: the agent asked for something its step&#39;s settings do not allow.</p>"), "why you");
        assert!(!asked.contains("Ahead:"), "no line of what lies ahead");
        // a sentence beside each answer
        assert!(asked.contains(">Allow</button><span class=\"says\">It writes the file and goes on. Before it lands in your branch, the merge asks you again.</span>"), "allow and what it does");
        assert!(asked.contains(">Refuse</button><span class=\"says\">It goes on without writing it.</span>"), "refuse and what it does");
        // what it wants to write, not written yet: a new file, all of it added
        assert!(asked.contains("<summary>See what it wants to write</summary>"), "the way to see it");
        assert!(asked.contains("What the agent wants to write in <code>tests/conftest.py</code> · not written yet"), "its heading");
        assert!(asked.contains("<span class=\"add\"><i>+</i>import pytest</span>"), "the new file, added");
        // an edit: what goes and what comes, among the lines that stay
        let edit = page.render(View::Question("edit".into(), "q1".into()), now).expect("the edit");
        assert!(edit.contains("<span class=\"del\"><i>-</i>x = 1</span><span class=\"add\"><i>+</i>x = 2</span><span class=\"ctx\"><i> </i>y = 1</span>"), "the edit as a diff");
        // a file that runs nowhere later: the merge has no more to ask about it
        assert!(edit.contains("<span class=\"says\">It writes the file and goes on.</span>"));
        // a leave to read: its own verb, nothing to see written
        let peek = page.render(View::Question("peek".into(), "q1".into()), now).expect("the read");
        assert!(peek.contains("<h1>May the agent read <code>/etc/hosts</code>?</h1>"));
        assert!(peek.contains("<span class=\"says\">It reads it and goes on.</span>") && peek.contains("<span class=\"says\">It goes on without reading it.</span>"));
        assert!(!peek.contains("See what it wants to write"), "nothing to write, nothing to see");
        // beside, and on the run's page, the title has no backquote either
        let on_run = page.render(View::Run("new".into()), now).expect("the run");
        assert!(on_run.contains("Answer: May the agent change <code>tests/conftest.py</code>?</a>"), "the run names the leave as its card does");
        for view in [View::Run("new".into()), View::Overview] {
            let html = page.render(view, now).expect("a page");
            assert!(!seen(&html).contains('`'), "no backquote beside nor on the run");
        }
        let _gone = std::fs::remove_dir_all(&home);
    }

    /// A home with three runs that wait for the person: "asks" at a question, "stuck" blocked, "paused" halted.
    fn three_waiting(name: &str) -> PathBuf {
        use jaynes_core::event::Fault;
        use jaynes_core::question::{By, Item, Question, VerdictKind};
        use jaynes_core::{Digest, Event, Manifest};
        let home = std::env::temp_dir().join(format!("jaynes-waiting-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        let mut store = Store::open(&{
            std::fs::create_dir_all(&home).expect("home");
            home.join("record.sqlite3")
        })
        .expect("store");
        for run in ["asks", "stuck", "paused"] {
            std::fs::create_dir_all(home.join("runs").join(run)).expect("dir");
            let manifest: Manifest = serde_json::from_value(json!({
                "role": "r", "version": "1", "protocol": 1,
                "steps": [{"name": "change", "executor": "agent", "stop": {"verdicts": ["keep", "redo"], "holds": true}}],
            }))
            .expect("manifest");
            let start = Input::Start { run: run.into(), request: format!("the request of {run}"), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
            let mut events = decide(None, &start).expect("start").events;
            events.push(match run {
                "asks" => Event::QuestionOpened {
                    question: Question { rides: Vec::new(),
                        id: "q1".into(), step: "change".into(), round: 0, kind: QuestionKind::Decision, items: vec![Item { named: None, id: "a".into(), label: "a".into(), digest: Digest("d".into()) }],
                        offered: vec![VerdictKind::Keep], holds: true, about: "Is the change right?".into(), choose: false, pass: 0, passes: 0, back: vec![],
                    },
                },
                "stuck" => Event::Blocked { step: Some("change".into()), fault: Fault::Jaynes, reason: "the box refused a port".into() },
                _ => Event::Halted { by: By::Person },
            });
            store.append(run, &events).expect("append");
            store.put_setup(run, &json!({})).expect("setup");
        }
        home
    }

    #[test]
    fn the_home_counts_a_blocked_run_and_a_halted_run_as_waiting_for_you_as_the_app_does() {
        let page = Page { home: three_waiting("home"), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        // the page's /home, the screen of the real-user's photo
        let html = page.render(View::Overview, crate::store::now()).expect("home");
        let aside = html.split("<div class=\"qh\">Waiting for you").nth(1).and_then(|a| a.split("<div class=\"qh\" style").next()).expect("the waiting list");
        // the question, the blocked run and the halted run, each with its own line in the list, never "· 0"
        assert!(aside.starts_with(" · 3</div>"), "{aside}");
        assert!(aside.contains("href=\"/q/asks/q1\""), "the question: {aside}");
        assert!(aside.contains("href=\"/run/stuck\"") && aside.contains(">blocked</span>"), "the blocked run: {aside}");
        assert!(aside.contains("href=\"/run/paused\"") && aside.contains(">halted</span>"), "the halted run: {aside}");
        assert!(html.contains("1 question, 1 blocked run and 1 halted run wait for you, in the list beside."), "{html}");
        assert!(!html.contains("Nothing waits for you."));
        // the sentence for each alone, and for none
        let runs: Vec<Run> = { let store = page.store().expect("store"); ["stuck", "paused"].iter().map(|id| store.state(id).expect("read").expect("run")).collect() };
        assert_eq!(waits_sentence(0, &[&runs[0]]), "1 blocked run waits for you, in the list beside.");
        assert_eq!(waits_sentence(0, &[&runs[1]]), "1 halted run waits for you, in the list beside.");
        assert_eq!(waits_sentence(2, &[]), "2 questions wait for you, in the list beside.");
        assert_eq!(waits_sentence(0, &[]), "Nothing waits for you.");
        let _gone = std::fs::remove_dir_all(&page.home);
    }

    // The tech-lead (S4): the merge's one touch merges everything at once; file by file only when the person asks. In
    // a project where every file is new, never 8 buttons (the real-user saw a pair per file in a scaffold).
    #[test]
    fn asked_a_merge_of_eight_new_files_the_page_shows_one_merge_and_file_by_file_folded() {
        use jaynes_core::question::{Item, Question, VerdictKind};
        use jaynes_core::{Digest, Event, Manifest};
        let home = std::env::temp_dir().join(format!("jaynes-merge-one-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("runs/m/project/.git")).expect("copy");
        std::fs::write(home.join("runs/m/project/.git/jaynes-start"), "main\n").expect("start");
        let manifest: Manifest = serde_json::from_value(json!({
            "role": "r", "version": "1", "protocol": 1,
            "steps": [{"name": "scaffold", "executor": "agent"}],
        }))
        .expect("manifest");
        let start = Input::Start { run: "m".into(), request: "A new app".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let mut events = decide(None, &start).expect("start").events;
        let items: Vec<Item> = (0..8).map(|i| Item { named: None, id: format!("src/f{i}.rs"), label: format!("src/f{i}.rs: new"), digest: Digest(format!("d{i}")) }).collect();
        events.push(Event::QuestionOpened {
            question: Question { rides: Vec::new(),
                id: "merge-1".into(), step: "scaffold".into(), round: 0, kind: QuestionKind::Merge, items, offered: vec![VerdictKind::Keep, VerdictKind::Drop],
                holds: true, about: "This is what lands on your branch.".into(), choose: false, pass: 0, passes: 0, back: vec![],
            },
        });
        let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
        store.append("m", &events).expect("append");
        store.put_setup("m", &json!({})).expect("setup");
        let page = Page { home: home.clone(), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        let asked = page.render(View::Question("m".into(), "merge-1".into()), crate::store::now()).expect("the question");
        let (open, folded) = asked.split_once("<details class=\"each\">").expect("file by file, folded");
        assert_eq!(open.matches("data-v=\"keep\"").count(), 1, "one Merge: {open}");
        assert_eq!(open.matches("data-v=\"drop\"").count(), 1, "one Branch only: {open}");
        assert!(open.contains("<button class=\"keep\" data-all=\"1\" data-v=\"keep\">Merge</button><button class=\"drop\" data-all=\"1\" data-v=\"drop\">Branch only</button>"), "{open}");
        assert!(folded.starts_with("<summary>File by file</summary>"), "{folded}");
        assert_eq!(folded.matches("data-v=\"keep\"").count(), 8, "each file, only when asked");
        // the whole merge's button judges every file, and a file judged alone clears it
        assert!(SCRIPT.contains("b.dataset.all"), "the script judges every file at once");
        // what lands comes first, then the pair (the design-lead, 23/11: the person reads what goes in before deciding)
        let run = jaynes_core::run::Run::fold(&events).expect("the run");
        let q = run.questions.get("merge-1").expect("the merge");
        let html = question_html(&run, q, &|_| String::new(), "<div class=\"changes\">LANDS</div>", "main", "", false, None, &[]);
        assert!(html.find("LANDS").expect("what lands") < html.find("data-all").expect("the pair"), "{html}");
        // what lands has a ceiling and scrolls inside, so the pair and File by file show without scrolling the page
        // (the design-lead, 23/11: ~36vh, never under 180 px, its bottom edge faded while more is below)
        assert!(html.contains("<div class=\"lands\" id=\"lands\"><div class=\"changes\">LANDS</div></div>"), "{html}");
        assert!(PAGE.contains(".lands pre.diff { max-height:max(180px, 36vh); overflow:auto; }"), "the ceiling");
        assert!(SCRIPT.contains("lands.classList.toggle(\"more\""), "the fade while more is below");
        // before any try, no refusal in red: at the merge one faint line says what to do (the design-lead's words)
        assert!(asked.contains("\"merge\":true"), "the script knows it is the merge");
        assert!(SCRIPT.contains("Merge, or keep it on the branch only."), "the merge's hint");
        assert!(PAGE.contains(".refused.hint { color:var(--faint); }"), "the hint is faint, not red");
        let _gone = std::fs::remove_dir_all(&home);
    }

    // What lands, each file in a block of its own (See them shows only some): each line stays on a line of its own
    // inside the block; "src/f1.rs@@ -0,0 +1 @@+pub fn f1() {}" glued on one line was what the merge showed (23/11)
    #[test]
    fn the_lines_of_what_lands_stay_one_per_line_inside_each_file_s_block() {
        let html = diff_html("diff --git a/src/f1.rs b/src/f1.rs\nnew file mode 100644\n--- /dev/null\n+++ b/src/f1.rs\n@@ -0,0 +1 @@\n+pub fn f1() {}\n");
        assert_eq!(html, "<span class=\"fb\" data-f=\"src/f1.rs\"><span class=\"file\">src/f1.rs</span><span class=\"hunk\">@@ -0,0 +1 @@</span><span class=\"add\"><i>+</i>pub fn f1() {}</span></span>");
        assert!(PAGE.contains(":where(.diff, .diff .fb) > span:not(.fb) { display:block;"), "each line a block, inside a file's block too");
        assert!(!PAGE.contains(".diff > .add"), "no line's look only for lines outside a file's block");
    }

    #[test]
    fn the_merge_names_the_branch_on_the_page_as_on_the_app_s_card() {
        use jaynes_core::question::{Question, VerdictKind};
        use jaynes_core::{Digest, Event, Manifest};
        let home = std::env::temp_dir().join(format!("jaynes-merge-title-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("runs/m/project/.git")).expect("copy");
        // the branch the person had checked out when the run started, as the engine keeps it beside the copy
        std::fs::write(home.join("runs/m/project/.git/jaynes-start"), "main\n").expect("start");
        let manifest: Manifest = serde_json::from_value(json!({
            "role": "r", "version": "1", "protocol": 1,
            "steps": [{"name": "fix", "executor": "agent"}],
        }))
        .expect("manifest");
        let start = Input::Start { run: "m".into(), request: "Fix the export".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let mut events = decide(None, &start).expect("start").events;
        events.push(Event::QuestionOpened {
            question: Question { rides: Vec::new(),
                id: "merge-1".into(), step: "fix".into(), round: 0, kind: QuestionKind::Merge, items: vec![], offered: vec![VerdictKind::Keep, VerdictKind::Drop],
                holds: true, about: "This is what lands on your branch.".into(), choose: false, pass: 0, passes: 0, back: vec![],
            },
        });
        let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
        store.append("m", &events).expect("append");
        store.put_setup("m", &json!({})).expect("setup");
        let page = Page { home: home.clone(), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        let now = crate::store::now();
        let asked = page.render(View::Question("m".into(), "merge-1".into()), now).expect("the question");
        assert!(asked.contains("<h1>Merge into “main”?</h1>") || asked.contains(">Merge into “main”?<"), "the question's title");
        assert!(!asked.contains("Merge into your branch?"), "the question's title");
        let listed = page.render(View::Overview, now).expect("home");
        assert!(listed.contains("Merge into “main”?<small>"), "the list beside");
        let _gone = std::fs::remove_dir_all(&home);
    }

    // The tech-lead, 22/11: one moment, the merge, one touch (the design-lead's merge.md). The merge's question on the
    // page names each stop its touch keeps, with what it holds and an Open; the run's story says what such a stop holds
    // and that the run went on, with the stop's other answer to open it and what it showed under it.
    #[test]
    fn asked_one_moment_at_the_merge_on_the_page() {
        use jaynes_core::question::{Item, Question, Ride, VerdictKind};
        use jaynes_core::{Digest, Event, Manifest};
        let home = std::env::temp_dir().join(format!("jaynes-page-one-moment-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("runs/o/project/.git")).expect("copy");
        std::fs::write(home.join("runs/o/project/.git/jaynes-start"), "main\n").expect("start");
        let manifest: Manifest = serde_json::from_value(json!({
            "role": "fix-bug", "version": "4.0.0", "protocol": 1,
            "steps": [
                {"name": "reproduce", "executor": "agent", "stop": {"verdicts": ["keep", "redo"], "holds": false, "settled": "the bug", "answers": {"keep": "Yes", "redo": "Not this bug"}}},
                {"name": "fix", "executor": "agent", "needs": ["reproduce"]},
            ],
        }))
        .expect("manifest");
        let start = Input::Start { run: "o".into(), request: "Fix the export".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let mut events = decide(None, &start).expect("start").events;
        let bug = Question { rides: Vec::new(),
            id: "q-bug".into(), step: "reproduce".into(), round: 1, kind: QuestionKind::Decision,
            items: vec![Item { named: None, id: "bug".into(), label: "Exporting a note with a comma breaks the CSV".into(), digest: Digest("d-bug".into()) }],
            offered: vec![VerdictKind::Keep, VerdictKind::Redo], holds: false, about: "Is this the bug?".into(), choose: false, pass: 0, passes: 0, back: vec![],
        };
        events.push(Event::QuestionOpened { question: bug });
        events.push(Event::Waiting { step: Some("fix".into()), on: vec![], why: String::new() });
        events.push(Event::QuestionOpened {
            question: Question { rides: vec![Ride { question: "q-bug".into(), step: "reproduce".into(), said: "the bug, as your request says".into() }],
                id: "merge-1".into(), step: "fix".into(), round: 0, kind: QuestionKind::Merge, items: vec![], offered: vec![VerdictKind::Keep, VerdictKind::Drop],
                holds: true, about: "This is what lands on your branch.".into(), choose: false, pass: 0, passes: 0, back: vec![],
            },
        });
        let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
        store.append("o", &events).expect("append");
        store.put_setup("o", &json!({"project": "/w/horta"})).expect("setup");
        store.put_display(&Digest("d-bug".into()), &json!({"aspects": [
            {"label": "Now", "text": "Exporting a note with a comma breaks the CSV."},
            {"label": "After the fix", "text": "The comma stays inside its quotes."},
        ]})).expect("display");
        let page = Page { home: home.clone(), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        let now = crate::store::now();
        let asked = page.render(View::Question("o".into(), "merge-1".into()), now).expect("the merge");
        assert!(asked.contains("Merging also keeps these"), "the stops it keeps");
        // the project before what it asks, in the title and the browser's tab (quatro-itens.md, 2)
        assert!(asked.contains("<h1><span class=\"proj\">horta</span> · Merge into “main”?</h1>"), "the project in the title");
        assert!(asked.contains("<title>horta · Merge into “main”? · Jaynes</title>"), "and in the browser's tab");
        assert!(asked.contains("your request answered it · the run went on</small>"), "the list beside: the request answered it");
        assert!(asked.contains("<span class=\"st\">“reproduce”</span><span>The bug, as your request says.</span><a href=\"/q/o/q-bug\">Open</a>"), "each with its step, what it holds, and Open");
        assert!(asked.contains("Merging keeps each as it is. To answer one differently, open it before you merge."), "under them");
        // without a touch on the page, no second line repeating it (the design-lead, 22/11)
        assert!(!asked.contains("also keeps the answer above"), "said once");
        let run = page.render(View::Run("o".into()), now).expect("the run");
        assert!(run.contains("◇ At “reproduce”: the bug, as your request says · the run went on"), "the story's line");
        assert!(run.contains("href=\"/q/o/q-bug\">Not this bug</a>"), "the stop's other answer opens it");
        assert!(run.contains("now: Exporting a note with a comma breaks the CSV. → after the fix: The comma stays inside its quotes."), "what it showed");
        assert!(!run.contains("Is this the bug? · The run went on without your answer."), "never the passed stop's line too");
        assert!(run.contains("Answer: Merge into “main”?</a>"), "the merge's invitation says what it asks");
        assert!(!run.contains("<div class=\"notice\"></div>"), "no notice without words");
        let _gone = std::fs::remove_dir_all(&home);
    }

    // The real-user, 22/11 (uso-real/achados/2026-11-22-01.md): after Send the page opened the next question, of
    // another project, without a word. A line on top of the next one says where it went (quatro-itens.md, 3).
    #[test]
    fn asked_after_send_the_page_says_where_it_went() {
        use jaynes_core::question::{Item, Question, VerdictKind};
        use jaynes_core::{Digest, Event, Manifest};
        let home = std::env::temp_dir().join(format!("jaynes-page-sent-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("home");
        let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
        for (id, project, asks) in [("a", "/w/scaffold-vazio", false), ("b", "/w/horta", true)] {
            let manifest: Manifest = serde_json::from_value(json!({"role": "kickoff", "version": "1.0.0", "protocol": 1, "steps": [{"name": "slice", "executor": "agent"}]})).expect("manifest");
            let start = Input::Start { run: id.into(), request: "Start the app".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
            let mut events = decide(None, &start).expect("start").events;
            if asks {
                events.push(Event::QuestionOpened {
                    question: Question { rides: Vec::new(),
                        id: "q-slice".into(), step: "slice".into(), round: 1, kind: QuestionKind::Decision,
                        items: vec![Item { named: None, id: "slice".into(), label: "The slice".into(), digest: Digest("d".into()) }],
                        offered: vec![VerdictKind::Keep, VerdictKind::Redo], holds: true, about: "Is this the first slice?".into(), choose: false, pass: 0, passes: 0, back: vec![],
                    },
                });
            }
            store.append(id, &events).expect("append");
            store.put_setup(id, &json!({"project": project})).expect("setup");
        }
        let page = Page { home: home.clone(), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        let now = crate::store::now();
        let other = page.render(View::Sent("a".into()), now).expect("the next");
        assert!(other.contains("<div class=\"sent\" id=\"sent\">Sent to <span class=\"proj\">scaffold-vazio</span>. Next: <span class=\"proj\">horta</span> · Is this the first slice?</div>"), "another project");
        let same = page.render(View::Sent("b".into()), now).expect("the next");
        assert!(same.contains("<div class=\"sent\" id=\"sent\">Sent. Next: Is this the first slice?</div>"), "the same project");
        assert!(!page.render(View::Home, now).expect("home").contains("id=\"sent\""), "only after a Send");
        // the page's script goes there after a Send, and the line leaves at the next action
        assert!(SCRIPT.contains("location.href = \"/?sent=\" + encodeURIComponent(base.run)"), "after a Send");
        let _gone = std::fs::remove_dir_all(&home);
        // nothing else waits: the list, saying so
        let home = std::env::temp_dir().join(format!("jaynes-page-sent-none-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("home");
        let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
        let manifest: Manifest = serde_json::from_value(json!({"role": "kickoff", "version": "1.0.0", "protocol": 1, "steps": [{"name": "slice", "executor": "agent"}]})).expect("manifest");
        let start = Input::Start { run: "a".into(), request: "Start the app".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        store.append("a", &decide(None, &start).expect("start").events).expect("append");
        let page = Page { home: home.clone(), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        assert!(page.render(View::Sent("a".into()), now).expect("home").contains("<div class=\"sent\" id=\"sent\">Sent. Nothing else waits for you.</div>"), "nothing else");
        let _gone = std::fs::remove_dir_all(&home);
    }

    // The real-user, 22/11 (uso-real/achados/2026-11-22-01.md): a fix can pass by changing what a test expects. The
    // merge says which of the person's tests it changed, those that were there before (quatro-itens.md, 1).
    #[test]
    fn asked_the_merge_says_when_it_changed_your_tests() {
        let block = |path: &str, new: bool| format!(
            "diff --git a/{path} b/{path}\n{}index 1..2 100644\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-a\n+b\n",
            if new { "new file mode 100644\n" } else { "" }
        );
        let diff = [block("src/export.js", false), block("test/export.test.js", false), block("test/new.test.js", true), block("test/screens.test.js", false)].concat();
        assert_eq!(crate::listing::changed_tests(&diff), ["test/export.test.js", "test/screens.test.js"], "the tests that were there, never a new one");
        let html = tests_changed_html(&crate::listing::changed_tests(&diff));
        assert!(html.contains("It changed your tests: <code>test/export.test.js</code>, <code>test/screens.test.js</code>"), "{html}");
        assert!(html.contains("<button class=\"seethem\""), "See them");
        assert!(html.contains("A fix can pass by changing what a test expects. Read these before you merge."), "why");
        let many: Vec<String> = (1..=6).map(|n| format!("test/t{n}.test.js")).collect();
        assert!(tests_changed_html(&many).contains("<code>test/t4.test.js</code> and 2 more"), "four, then how many more");
        assert_eq!(tests_changed_html(&[]), "", "none changed: nothing");
        assert!(crate::listing::changed_tests(&block("src/export.js", false)).is_empty());
        // the diff's files each in a block of their own, so See them shows only these
        assert!(diff_html(&diff).contains("<span class=\"fb\" data-f=\"test/export.test.js\">"), "a block per file");
    }

    // The tech-lead's rule, 21/11 (Alan's run r1795301086-403): the stop of a step the run already passed never covers
    // where the run is now. On the page the blocked notice stays on top and the passed stop goes under it, in one line
    // with Answer; the list beside has it under "Can still answer", and its question says where the run is now.
    #[test]
    fn asked_a_stop_the_run_passed_never_covers_now_on_the_page() {
        use jaynes_core::event::Fault;
        use jaynes_core::question::{Item, Question, VerdictKind};
        use jaynes_core::{Digest, Event, Manifest};
        let home = std::env::temp_dir().join(format!("jaynes-page-passed-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("home");
        let manifest: Manifest = serde_json::from_value(json!({
            "role": "fix-bug", "version": "4.0.0", "protocol": 1,
            "steps": [{"name": "reproduce", "executor": "agent"}, {"name": "found", "executor": "agent", "needs": ["reproduce"]}, {"name": "fix", "executor": "agent", "needs": ["found"]}],
        }))
        .expect("manifest");
        let start = Input::Start { run: "b".into(), request: "Fix the export".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let mut events = decide(None, &start).expect("start").events;
        events.push(Event::QuestionOpened {
            question: Question { rides: Vec::new(),
                id: "q-repro".into(), step: "reproduce".into(), round: 1, kind: QuestionKind::Decision,
                items: vec![Item { named: None, id: "examples".into(), label: "The examples".into(), digest: Digest("d".into()) }],
                offered: vec![VerdictKind::Keep, VerdictKind::Redo], holds: false, about: "Is this the bug?".into(), choose: false, pass: 0, passes: 0, back: vec![],
            },
        });
        events.push(Event::Blocked { step: Some("fix".into()), fault: Fault::Agent, reason: "the change could not be checked".into() });
        let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
        store.append("b", &events).expect("append");
        store.put_setup("b", &json!({})).expect("setup");
        let page = Page { home: home.clone(), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        let now = crate::store::now();
        let line = "At “reproduce”: Is this the bug? · The run went on without your answer.";
        let run = page.render(View::Run("b".into()), now).expect("the run");
        let (blocked, passed) = (run.find("Blocked at").expect("the blocked notice"), run.find(line).expect("the passed stop's line"));
        assert!(blocked < passed, "the blocked notice on top, the passed stop under it");
        assert!(!run.contains("Answer: Is this the bug?"), "no invitation on top");
        assert!(run.contains("href=\"/q/b/q-repro\">Answer</a>"), "Answer opens its question");
        let home_page = page.render(View::Overview, now).expect("home");
        let (can, waits) = (home_page.find("Can still answer · 1").expect("the group beside"), home_page.find("Waiting for you · 1").expect("only the blocked run waits, its passed stop does not"));
        assert!(waits < can, "under what waits");
        let asked = page.render(View::Question("b".into(), "q-repro".into()), now).expect("its question");
        assert!(asked.contains("The run went on without your answer. It is now at “fix”, blocked."), "where the run is now");
        assert!(asked.contains("“Redo” would do “reproduce” again. “Keep” changes nothing."), "what an answer still does");
        let _gone = std::fs::remove_dir_all(&home);
    }

    // The real-user, 22/11 (r1795330821-771): "O cartão do bloqueio diz só "A check you cannot see found that this
    // does not hold". A pessoa não vê o caso". The engineer lead, 22/11: the page shows the case that failed as the
    // app's card does, from listing::blocked (what the engine kept), never from the reason alone.
    // The tech-lead, T7, 23/11 (the horta: "Finished: delivered" and no way to merge): a run delivered on a branch says
    // how its change reaches the folder, in the design-lead's words (quatro-itens.md, item 5): the branch in mono with
    // Copy; Merge, primary and never amber, when the engine takes it; a checkout with changes keeps the button and
    // says to commit or stash; a conflict takes it away and says to merge by hand from the branch, which Copy keeps;
    // git's own words never in the sentence, closed under "▸ What git said"; a run the engine will not merge (started
    // never to merge) shows the branch alone.
    #[test]
    fn asked_a_run_delivered_on_a_branch_says_how_it_reaches_the_folder() {
        use crate::listing::{NotLanded, Unmerged};
        let u = |refused: Option<&str>, failed: Option<NotLanded>| Unmerged { branch: "jaynes/r17".into(), into: "main".into(), project: "horta".into(), refused: refused.map(str::to_string), failed };
        let line = "The change is on the branch “<code>jaynes/r17</code>”, not in your folder yet. <button class=\"copyplace\" data-copy=\"jaynes/r17\">Copy</button>";
        let button = "<button class=\"act primary\" data-act=\"merge\">Merge into “main”</button><span class=\"odim\">It lands on “main” in horta. Nothing is pushed, and Undo takes it back.</span>";
        let open = reach_html(&u(None, None));
        assert!(open.contains(line) && open.contains(button) && !open.contains("rfail"), "{open}");
        let dirty = reach_html(&u(None, crate::listing::not_landed("jaynes/r17 at 1a2b3c; not merged: your checkout of main has uncommitted changes")));
        assert!(dirty.contains(line) && dirty.contains(button), "the button stays: {dirty}");
        assert!(dirty.contains("<div class=\"rfail\">Not merged: your checkout of “main” has changes you have not committed. Commit or stash them, then merge again.</div>"), "{dirty}");
        assert!(!dirty.contains("What git said"), "nothing of git's to show, no fold: {dirty}");
        let git = "CONFLICT (content): Merge conflict in greeting.txt";
        let conflict = reach_html(&u(None, crate::listing::not_landed(&format!("jaynes/r17 at 1a2b3c; not merged: it conflicts with what main has now: {git}"))));
        assert!(conflict.contains(line) && !conflict.contains("data-act=\"merge\""), "Copy stays, the button goes: {conflict}");
        assert!(conflict.contains("<div class=\"rfail\">Not merged: the change conflicts with what “main” has now. The branch “jaynes/r17” still has it, so you can merge it by hand.</div>"), "{conflict}");
        let said = conflict.split("<details").next().expect("before git");
        assert!(!said.contains("CONFLICT") && conflict.contains(&format!("<summary>▸ What git said</summary><pre class=\"said\">{git}</pre>")), "{conflict}");
        let never = reach_html(&u(Some("the run was started never to merge"), None));
        assert!(never.contains(line) && !never.contains("data-act=\"merge\""), "{never}");
        // the person's branch has it already: said in text, Copy of the branch, neither Merge nor Undo
        let mut r = small_run(false);
        r.phase = Phase::Finished { outcome: jaynes_core::event::Outcome::Delivered };
        r.merged = true;
        let page = run_html_of(&r, "", "", None, None, RunExtra { into: "main".into(), already: Some("jaynes/r17".into()), ..RunExtra::default() });
        assert!(page.contains("<div class=\"reach\"><div class=\"rline\">Nothing to merge: “main” in your project already has this change. <button class=\"copyplace\" data-copy=\"jaynes/r17\">Copy</button></div></div>"), "{page}");
        assert!(!page.contains("data-act=\"undo\"") && !page.contains("data-act=\"merge\"") && !page.contains("Merged into"), "{page}");
        // a delivery that said nothing of a merge, or one that landed, says no merge failed
        assert_eq!(crate::listing::not_landed("jaynes/r17 at 1a2b3c; merged as 9f8e7d"), None);
        assert_eq!(crate::listing::not_landed("jaynes/r17 at 1a2b3c"), None);
    }

    #[test]
    fn asked_a_run_blocked_by_a_hidden_case_shows_the_case_that_failed_on_the_page() {
        use jaynes_core::event::{Fault, Usage};
        use jaynes_core::{Digest, Event, FailedCase, Manifest};
        let home = std::env::temp_dir().join(format!("jaynes-page-hidden-case-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("home");
        let manifest: Manifest = serde_json::from_value(json!({
            "role": "fix-bug", "version": "4.0.0", "protocol": 1,
            "steps": [{"name": "fix", "executor": "agent"}],
        }))
        .expect("manifest");
        let start = Input::Start { run: "h".into(), request: "O total do recibo sai errado".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let says = "the total counts the quantity";
        let reason = format!("A check you cannot see found that this does not hold: {says}");
        let mut events = decide(None, &start).expect("start").events;
        for attempt in [1, 2] {
            events.push(Event::AttemptStarted { step: "fix".into(), round: 0, attempt, fix: 0, session: None, deadline_seconds: 60, tokens_left: 1000, key: None });
            events.push(Event::AttemptFailed { step: "fix".into(), attempt, reasons: vec![reason.clone()], usage: Usage::default(), spent: true });
            let printed = (1..=8).map(|n| format!("line {n} <of> it")).collect::<Vec<_>>().join("\n");
            let case = FailedCase { says: says.into(), run: "make test TOTAL=1".into(), code: 2, printed };
            crate::listing::keep_failed_cases(&home.join("runs/h"), "fix", attempt, "cases", &[case]).expect("keep");
        }
        events.push(Event::Blocked { step: Some("fix".into()), fault: Fault::Agent, reason: format!("fix spent its 2 attempts: {reason}") });
        let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
        store.append("h", &events).expect("append");
        store.put_setup("h", &json!({"project": "/w/recibo"})).expect("setup");
        let page = Page { home: home.clone(), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        let shown = page.render(View::Run("h".into()), crate::store::now()).expect("the run");
        let _gone = std::fs::remove_dir_all(&home);
        // the why as the app says it, then the case: what it says, who wrote it, what ran in mono, what it expected and
        // what came, what it printed as text (six lines, then the rest folded), and in how many attempts it failed
        assert!(shown.contains("“fix” used its 2 attempts. Each time, a case the agent does not see failed."), "{shown}");
        assert!(!shown.contains("A check you cannot see"), "the agent's words, never the person's");
        let case = shown.split("<div class=\"cases\">").nth(1).unwrap_or_else(|| panic!("no case drawn: {shown}"));
        for said in [
            "<div class=\"ch\">The case that failed</div>",
            "<b>“the total counts the quantity”</b>",
            "Written at “cases”, by the agent there.",
            "Ran: <code>make test TOTAL=1</code>",
            "Expected: exit 0.",
            "Got: exit 2.",
            "line 6 &lt;of&gt; it",
            "<summary>▸ all of it</summary>",
            "Failed in 2 of 2 attempts.",
        ] {
            assert!(case.contains(said), "{said}: {case}");
        }
        let shown_lines = case.split("<summary>").next().unwrap_or_default();
        assert!(!shown_lines.contains("line 7"), "six lines, then the fold: {case}");
        // before what the person can do
        let at = |t: &str| shown.find(t).unwrap_or_else(|| panic!("{t}"));
        assert!(at("The case that failed") < at("Continue"), "the case before the ways out");
    }

    // The engineer lead, 23/11, after the backend's orphan run (c50391fc): a run that says it runs while the process
    // driving it is gone (drive::left_running, killed outright) is shown as the app's column says it, "Jaynes stopped
    // while it ran", and its page offers Resume, never Halt: nothing works on it to halt, and the resume settles the
    // record first (drive::settle_left_running). A run whose driver lives is still at work, with Halt.
    #[test]
    fn asked_a_run_whose_process_is_gone_offers_resume_on_the_page() {
        use jaynes_core::{Digest, Event, Manifest};
        let home = std::env::temp_dir().join(format!("jaynes-page-left-running-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("runs/o")).expect("home");
        let manifest: Manifest = serde_json::from_value(json!({
            "role": "fix-bug", "version": "4.0.0", "protocol": 1,
            "steps": [{"name": "fix", "executor": "agent"}],
        }))
        .expect("manifest");
        let start = Input::Start { run: "o".into(), request: "O total do recibo sai errado".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let mut events = decide(None, &start).expect("start").events;
        events.push(Event::AttemptStarted { step: "fix".into(), round: 0, attempt: 1, fix: 0, session: None, deadline_seconds: 60, tokens_left: 1000, key: None });
        let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
        store.append("o", &events).expect("append");
        store.put_setup("o", &json!({"project": "/w/recibo"})).expect("setup");
        let page = Page { home: home.clone(), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        // its driver lives: at work, with Halt
        std::fs::write(home.join("runs/o/driver.pid"), std::process::id().to_string()).expect("pid");
        let working = page.render(View::Run("o".into()), crate::store::now()).expect("the run");
        assert!(working.contains("data-act=\"halt\"") && !working.contains("data-act=\"resume\""), "{working}");
        // its driver is gone
        let mut gone = std::process::Command::new("true").spawn().expect("true");
        let pid = gone.id();
        let _ended = gone.wait();
        std::fs::write(home.join("runs/o/driver.pid"), pid.to_string()).expect("pid");
        let shown = page.render(View::Run("o".into()), crate::store::now()).expect("the run");
        let home_page = page.render(View::Home, crate::store::now()).expect("home");
        let _gone = std::fs::remove_dir_all(&home);
        assert!(shown.contains("data-act=\"resume\"") && !shown.contains("data-act=\"halt\""), "Resume, never Halt: {shown}");
        assert!(shown.contains("<div class=\"notice\">Jaynes stopped while it ran at “fix”</div>"), "{shown}");
        let steps = shown.split("class=\"stages told\"").nth(1).unwrap_or_else(|| panic!("no steps: {shown}"));
        assert!(steps.contains("class=\"halted\"") && !steps.contains("class=\"running\""), "the step stopped, not breathing: {steps}");
        // the column and the home, as the app says it: halted, waiting for the person
        assert!(shown.contains("<span class=\"holds\">halted</span>"), "{shown}");
        assert!(!shown.contains("<i class=\"running\""), "the strip too: {shown}");
        assert!(home_page.contains("1 halted run"), "{home_page}");
    }

    // The real-user's app, 22/11 (recibo, fix-bug): a run blocked at "fix" showed beside as "Is this the bug?", its
    // old question still open. The tech-lead's rule: a blocked run says so first, its open questions stay inside it.
    #[test]
    fn asked_a_blocked_run_waits_as_blocked_on_the_page() {
        use jaynes_core::event::Fault;
        use jaynes_core::question::{Item, Question, VerdictKind};
        use jaynes_core::{Digest, Event, Manifest};
        let home = std::env::temp_dir().join(format!("jaynes-page-blocked-open-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("home");
        let manifest: Manifest = serde_json::from_value(json!({
            "role": "fix-bug", "version": "4.0.0", "protocol": 1,
            "steps": [{"name": "reproduce", "executor": "agent"}, {"name": "fix", "executor": "agent", "needs": ["reproduce"]}],
        }))
        .expect("manifest");
        let start = Input::Start { run: "b".into(), request: "O total do recibo sai errado".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let mut events = decide(None, &start).expect("start").events;
        events.push(Event::QuestionOpened {
            question: Question { rides: Vec::new(),
                id: "q-repro".into(), step: "reproduce".into(), round: 1, kind: QuestionKind::Decision,
                items: vec![Item { named: None, id: "examples".into(), label: "The examples".into(), digest: Digest("d".into()) }],
                offered: vec![VerdictKind::Keep, VerdictKind::Redo], holds: true, about: "Is this the bug?".into(), choose: false, pass: 0, passes: 0, back: vec![],
            },
        });
        events.push(Event::Blocked { step: Some("fix".into()), fault: Fault::Agent, reason: "fix spent its 3 attempts".into() });
        let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
        store.append("b", &events).expect("append");
        store.put_setup("b", &json!({"project": "/w/recibo"})).expect("setup");
        let page = Page { home: home.clone(), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        let listed = page.render(View::Overview, crate::store::now()).expect("home");
        assert!(listed.contains("Waiting for you · 1"), "the run waits once, as blocked");
        assert!(!listed.contains("Is this the bug?<small>"), "its open question is not an item of its own");
        let _gone = std::fs::remove_dir_all(&home);
    }

    // Alan, 21/11: "não consigo saber fácil qual run é de qual projeto". Two runs side by side read "Agreed with Alan on
    // 2026-11-21…". The page names a run as the app does (listing::name): its project first, then its title, then
    // what tells it apart; the run's page and the browser's tab too (the design-lead's nome-da-run.md).
    #[test]
    fn asked_which_run_is_which_project_on_the_page() {
        use jaynes_core::{Digest, Manifest};
        let home = std::env::temp_dir().join(format!("jaynes-page-names-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("home");
        let request = "Agreed with Alan on 2026-11-21: make the export faster. It takes a minute for a small file, and it should take a second.";
        let mut store = Store::open(&home.join("record.sqlite3")).expect("store");
        for (id, project) in [("a", "/w/tidepool"), ("b", "/w/harbor-api")] {
            let manifest: Manifest = serde_json::from_value(json!({"role": "fix-bug", "version": "4.0.0", "protocol": 1, "steps": [{"name": "fix", "executor": "agent"}]})).expect("manifest");
            let start = Input::Start { run: id.into(), request: request.into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
            store.append(id, &decide(None, &start).expect("start").events).expect("append");
            store.put_setup(id, &json!({"project": project})).expect("setup");
        }
        let page = Page { home: home.clone(), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        let now = crate::store::now();
        let title = crate::listing::run_title(None, request);
        assert!(title.starts_with("Make the export faster"), "{title}");
        let named = |p: &str| format!("<span class=\"nm\"><span class=\"proj\">{p}</span><span class=\"sep\"> · </span><span class=\"tt\">{}</span></span>", esc(&title));
        let listed = page.render(View::Overview, now).expect("home");
        for p in ["tidepool", "harbor-api"] {
            assert!(listed.contains(&format!("\">{}<small>", named(p))), "the list beside names {p} first");
        }
        assert!(!listed.contains("Agreed with Alan"), "no run is named by its opening words: {listed}");
        let run = page.render(View::Run("a".into()), now).expect("the run");
        assert!(run.contains(&format!("<title>tidepool · {} · Jaynes</title>", esc(&title))), "the browser's tab");
        assert!(run.contains("<div class=\"where\">tidepool · fix-bug 4.0.0 · run a · low risk</div>"), "the where line starts with the project");
        assert!(run.contains(&format!("<h1>{}</h1>", esc(&title))), "the title is the h1");
        assert!(run.contains(&esc(request)), "the whole request under it");
        let row = crate::listing::Row { id: "c".into(), project: "tidepool".into(), title: "Fix the export".into(), apart: Some("at “fix”".into()), line: String::new(), mark: crate::listing::Mark::Working };
        assert_eq!(name_html(&row), "<span class=\"nm\"><span class=\"proj\">tidepool</span><span class=\"sep\"> · </span><span class=\"tt\">Fix the export</span><span class=\"sep\"> · </span><span class=\"apart\">at “fix”</span></span>");
        let _gone = std::fs::remove_dir_all(&home);
    }

    // The real-user's round 2026-11-22-01: with two projects waiting, "Merge into “main”?" and "Is this the bug?" did
    // not say whose; the stop's own head did not either. Each item of "Waiting for you" starts with its project in
    // dim, as a run's name does (the design-lead's nome-da-run.md, item 7), and so does the stop's where line.
    #[test]
    fn asked_whose_stop_waits() {
        let home = at_a_leave("asked_whose_stop_waits");
        let page = Page { home: home.clone(), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
        let now = crate::store::now();
        let listed = page.render(View::Overview, now).expect("home");
        assert!(
            listed.contains("<span class=\"proj\">tidepool</span><span class=\"sep\"> · </span>May the agent change <code>tests/conftest.py</code>?<small>"),
            "the item beside starts with its project"
        );
        assert!(listed.contains("<span class=\"proj\">no project</span><span class=\"sep\"> · </span>May the agent read <code>/etc/hosts</code>?<small>"), "a run without a project says so");
        let asked = page.render(View::Question("new".into(), "q1".into()), now).expect("the leave");
        assert!(asked.contains("<div class=\"where\">tidepool · <a href=\"/run/new\">"), "the stop's where line starts with its project");
        let _gone = std::fs::remove_dir_all(&home);
    }

    // The design-lead's quatro-itens.md, item 8: an item of "Waiting for you" names the project and the question, not
    // the run. Two items that read the same are told apart after the project, faint: by the run's title when the
    // titles differ, by what listing::name tells apart when they are the same; the question is what gives way.
    #[test]
    fn asked_two_items_that_read_the_same_are_told_apart_in_waiting_for_you() {
        // the item beside of each run whose setup `setup` gives
        let waits = |case: &str, setup: &dyn Fn(&str) -> serde_json::Value| {
            let page = Page { home: told_with(&format!("apart-{case}"), setup), port: 8750, key: "k3y".into(), exe: PathBuf::from("/usr/bin/true") };
            let listed = page.render(View::Overview, crate::store::now()).expect("home");
            let _gone = std::fs::remove_dir_all(&page.home);
            listed.split("<div class=\"qh\">Waiting for you").nth(1).and_then(|a| a.split("<div class=\"qh\" style").next()).expect("the waiting list").to_string()
        };
        let item = |apart: &str| {
            format!("<span class=\"nm\"><span class=\"proj\">tidepool</span><span class=\"sep\"> · </span><span class=\"apart\">{apart}</span><span class=\"sep\"> · </span><span class=\"tt\">Is the plan right?</span></span><small>")
        };
        // one title in one project, at one stop: when each started
        let both = waits("same", &|_| json!({"project": "/w/tidepool", "title": "Fix the export"}));
        assert_eq!(both.matches("<span class=\"nm\"><span class=\"proj\">tidepool</span><span class=\"sep\"> · </span><span class=\"apart\">started ").count(), 2, "{both}");
        assert_eq!(both.matches("</span><span class=\"sep\"> · </span><span class=\"tt\">Is the plan right?</span></span><small>").count(), 2, "{both}");
        // two titles asking the same: each run's title, which gives way first, to 16 characters, before the question
        // does (the design-lead's second word on item 8, after the photo at 1440 left "I…" of the question)
        let titled = waits("titles", &|run| json!({"project": "/w/tidepool", "title": if run == "w1" { "Fix the export" } else { "Rename the footer link" }}));
        assert!(titled.contains(&item("Fix the export")) && titled.contains(&item("Rename the foote…")), "{titled}");
        // another project: nothing reads the same, today's item
        let other = waits("projects", &|run| json!({"project": if run == "w1" { "/w/tidepool" } else { "/w/harbor" }, "title": "Fix the export"}));
        assert!(!other.contains("class=\"apart\""), "{other}");
        assert!(other.contains("<span class=\"proj\">tidepool</span><span class=\"sep\"> · </span>Is the plan right?<small>"), "{other}");
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
        // each run by its name, as the list beside names it (listing::name): no project here, then its title
        assert!(shown.contains("no project</span><span class=\"sep\"> · </span><span class=\"tt\">The request of w2</span>") && shown.contains("at plan: Is the plan right?"), "{shown}");
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

    /// A run of one agent step, "plan", and with `high` a second step only a high-risk run takes.
    fn small_run(high: bool) -> Run {
        let mut steps = vec![json!({"name": "plan", "executor": "agent"})];
        if high {
            steps.push(json!({"name": "review", "executor": "agent", "from_risk": "high"}));
        }
        let manifest: jaynes_core::Manifest = serde_json::from_value(json!({"role": "r", "version": "1", "protocol": 1, "steps": steps})).expect("manifest");
        let start = Input::Start { run: "w".into(), request: "x".into(), manifest, role_fingerprint: jaynes_core::Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        Run::fold(&decide(None, &start).expect("start").events).expect("run")
    }

    // real-user, 21/11: "A página só mostra "HIGH RISK"; o porquê só aparece no terminal do start."
    #[test]
    fn the_page_says_why_a_run_is_high_risk_and_what_that_changes() {
        let mut r = small_run(false);
        assert!(!run_html(&r, "", "", None, None).contains("High risk"), "a low-risk run says nothing of it");
        r.apply(&jaynes_core::event::Event::RiskRaised {
            to: jaynes_core::risk::Risk::High,
            by: By::Rule { rule: "what-the-change-touches".into() },
            reasons: vec!["the change has 45 changed lines outside its tests, more than a small change's 30".into()],
            ceiling: r.ceilings.high,
        });
        let shown = run_html(&r, "", "", None, None);
        assert!(
            shown.contains("High risk: the change has 45 changed lines outside its tests, more than a small change&#39;s 30. It may take up to 120 min and 10 million tokens, where a low-risk run stops at 20 min and 2 million tokens."),
            "{shown}"
        );
        // a name an agent chose is shown as text, never as the page's own markup
        r.apply(&jaynes_core::event::Event::RiskRaised { to: jaynes_core::risk::Risk::High, by: By::Person, reasons: vec!["the change touches `<b>x</b>`".into()], ceiling: r.ceilings.high });
        assert!(run_html(&r, "", "", None, None).contains("High risk: the change touches `&lt;b&gt;x&lt;/b&gt;`."));
    }

    #[test]
    fn the_run_s_controls_say_what_the_app_s_say() {
        // raising the risk asks at more steps, only where the high risk adds some, with the app's hint
        let shown = run_html(&small_run(true), "", "", None, None);
        assert!(shown.contains(">Ask me at more steps<") && shown.contains(&format!("title=\"{}\"", crate::listing::ASK_AT_MORE_STEPS_TIP)), "{shown}");
        assert!(!shown.contains("Raise the risk"), "{shown}");
        assert!(!run_html(&small_run(false), "", "", None, None).contains("Ask me at more steps"));
        // abandoning a run already delivered says what stays
        let mut delivered = small_run(false);
        delivered.delivered = Some(jaynes_core::Digest("d".into()));
        assert!(run_html(&delivered, "", "", None, None).contains(&format!("title=\"{}\"", crate::listing::ABANDON_DELIVERED_TIP)));
    }

    #[test]
    fn a_blocked_or_halted_run_says_where_in_the_app_s_words() {
        use jaynes_core::run::Status;
        let mut r = small_run(false);
        r.phase = Phase::Blocked { step: Some("plan".into()), fault: jaynes_core::event::Fault::Agent, reason: "plan spent its 3 attempts: the agent exited with code 1: no tests".into() };
        let shown = run_html(&r, "", "", None, None);
        assert!(shown.contains("Blocked at “plan”") && shown.contains("“plan” used its 3 attempts. The agent exited with code 1.") && shown.contains("Continue tries “plan” again, with its attempts back."), "{shown}");
        assert!(!shown.contains("Blocked (agent)"), "{shown}");
        let mut h = small_run(false);
        h.halted = true;
        h.steps[0].status = Status::Running { attempt: 1, fix: 0 };
        assert!(run_html(&h, "", "", Some("halted by an agent"), None).contains("Halted by an agent at “plan”"));
        // halted with no step under way in its record: where it would go on
        let mut idle = small_run(false);
        idle.halted = true;
        idle.steps[0].status = Status::Idle;
        assert!(run_html(&idle, "", "", Some("halted by an agent"), None).contains("Halted by an agent at “plan”"));
        // set aside by the agent that started it: said so, never as a halt
        let mut aside = small_run(false);
        aside.halted = true;
        aside.set_aside = Some(By::Agent { who: "claude:1@x".into() });
        let shown = run_html(&aside, "", "", Some("halted by an agent"), None);
        assert!(shown.contains("Set aside by the agent that started it, at “plan”. Its work is kept: Resume brings it back, Abandon ends it.") && !shown.contains("Halted by"), "{shown}");
        // a run delivered whose answers are still open, as the app's column says it
        let mut w = small_run(false);
        w.phase = Phase::Waiting { step: None, on: vec![], why: String::new() };
        let w = crate::listing::Entry { run: w, project: String::new(), title: None, started: 0, at: 0, merged: false, already: false, into: String::new(), gone: None, moved: None, stopped: false, waiting: None };
        assert!(phase_word(&w).contains("delivered · you can still correct it"), "{}", phase_word(&w));
    }

    #[test]
    fn a_choice_and_the_changes_read_as_in_the_app() {
        assert_eq!(changes_title("directions", 3), "Changes made at “directions” · 3 files");
        assert_eq!(changes_title("copy", 1), "Changes made at “copy” · 1 file");
        assert!(choose_button("harbor").contains(">Choose this one<"));
        // one option: not this one; more: none of these
        assert!(none_row(1).contains(">Not this one<") && !none_row(1).contains("None of these"));
        assert!(none_row(3).contains(">None of these<"));
        assert!(PAGE.contains("What this does · written by Jaynes, not by the role") && !PAGE.contains("What this answer will do"));
    }

    #[test]
    fn a_halted_run_s_step_under_way_is_drawn_halted_not_running() {
        use jaynes_core::run::Status;
        let manifest: jaynes_core::Manifest = serde_json::from_value(json!({"role": "r", "version": "1", "protocol": 1, "steps": [{"name": "plan", "executor": "agent"}]})).expect("manifest");
        let start = Input::Start { run: "h".into(), request: "x".into(), manifest, role_fingerprint: jaynes_core::Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let mut r = Run::fold(&decide(None, &start).expect("start").events).expect("run");
        for status in [Status::Running { attempt: 1, fix: 0 }, Status::Checking { attempt: 1, fix: 0 }, Status::Probing] {
            r.steps[0].status = status;
            r.halted = false;
            assert!(strip(&r, false).contains("class=\"running\""), "{}", strip(&r, false));
            r.halted = true;
            let drawn = strip(&r, false);
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
            (json!({"run": "r1", "action": "continue", "words": "the case needs git history"}), agent()),
            (json!({"run": "r1", "action": "continue", "words": "the case needs git history"}), By::Person),
            (json!({"run": "r1", "action": "resume"}), agent()),
            (json!({"run": "r1", "action": "raise-risk"}), agent()),
            (json!({"run": "r1", "action": "halt"}), agent()),
            (json!({"run": "r1", "action": "resume"}), agent()),
            (json!({"run": "r1", "action": "halt"}), By::Person),
            (json!({"run": "r1", "action": "raise-risk"}), By::Person),
            (json!({"run": "r1", "action": "raise-ceiling"}), By::Person),
            (json!({"run": "r1", "action": "drop-case", "step": "plan", "case": "x"}), agent()),
            (json!({"run": "r1", "action": "drop-case", "step": "plan", "case": "x"}), By::Person),
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

    // The engineer lead, 22/11 (row 28): moving a run whose engine left the Mac to Finished is the person's alone,
    // through the page's key or the app's signature as for answers; a run this engine carries on is refused.
    #[test]
    fn asked_only_the_person_moves_a_run_whose_engine_left_to_finished_22_11() {
        use ed25519_dalek::{Signer, SigningKey};
        use jaynes_core::question::{Item, Question, VerdictKind};
        use jaynes_core::{Digest, Event};
        let home = waiting("move");
        let exe = PathBuf::from("/usr/bin/true");
        // a run of a made-up engine that never was on this Mac, at a stop
        let mut events = Store::open(&home.join("record.sqlite3")).expect("store").events("r1").expect("events");
        for e in &mut events {
            if let Event::RunStarted { run, engine, .. } = e {
                *run = "old".into();
                *engine = "0.0.1+nowhere".into();
            }
        }
        let question = Question { rides: Vec::new(),
            id: "q1".into(), step: "plan".into(), round: 0, kind: QuestionKind::Decision, items: vec![Item { named: None, id: "a".into(), label: "a".into(), digest: Digest("d".into()) }],
            offered: vec![VerdictKind::Keep], holds: true, about: String::new(), choose: false, pass: 0, passes: 0, back: vec![],
        };
        let events: Vec<Event> = events.into_iter().filter(|e| matches!(e, Event::RunStarted { .. })).chain([Event::QuestionOpened { question }]).collect();
        Store::open(&home.join("record.sqlite3")).expect("store").append("old", &events).expect("append");
        let file = home.join(crate::moved::FILE);
        // drawn apart, never with what waits: one run, so no "Move all"; its page says why and offers the move
        let page = Page { home: home.clone(), port: 8750, key: "k3y".into(), exe: exe.clone() };
        for id in ["r1", "old"] {
            Store::open(&home.join("record.sqlite3")).expect("store").put_setup(id, &json!({"project": "/p/tidepool"})).expect("setup");
        }
        let side = page.render(View::Overview, 0).expect("page");
        assert!(side.contains("Waiting for you · 1<") && side.contains("From an older Jaynes · 1<"), "{side}");
        assert!(side.contains("<small class=\"older\">can&#39;t go on · Jaynes 0.0.1</small>"), "{side}");
        // its marks in the list beside: none amber, the step it stopped at grey
        let link = side.split("href=\"/run/old\"").nth(1).and_then(|l| l.split("</a>").next()).unwrap_or_default();
        assert!(link.contains("<i class=\"gone\"") && !link.contains("class=\"waiting\""), "{link}");
        assert!(!side.contains("Move all to Finished"), "one run: its card's button is enough");
        let card = page.render(View::Run("old".into()), 0).expect("page");
        for said in ["This run can", "it started on Jaynes 0.0.1, which is no longer on this Mac.", crate::listing::OLDER_HOW, ">Move to Finished</button>", "It leaves Needs you for good."] {
            assert!(card.contains(said), "{said}: {card}");
        }
        assert!(!card.contains("data-act=\"abandon\"") && !card.contains("data-act=\"halt\"") && !card.contains("Answer: "), "nothing that cannot reach it");
        assert!(!card.contains("What it made so far"), "it made nothing yet");
        // its steps: where it stopped, in grey, never "waiting for you"; its marks under the title none amber
        assert!(card.contains("<li class=\"gone\">") && card.contains("<small>stopped here · can&#39;t go on</small>"), "{card}");
        assert!(!card.contains("waiting for you") && !card.contains("<i class=\"waiting\""), "{card}");
        let agent = By::Agent { who: "an agent".into() };
        let err = act_in(&home, &exe, &json!({"run": "old", "action": "move-to-finished"}), agent).expect_err("an agent");
        assert!(err.starts_with("only you move a run to Finished"), "{err}");
        let err = act_in(&home, &exe, &json!({"runs": ["old", "r1"], "action": "move-to-finished"}), By::Person).expect_err("r1 goes on");
        assert!(err.contains("r1 goes on with this Jaynes"), "{err}");
        assert!(!file.exists(), "nothing moved");
        // the app: a signature over the move by its paired key, never over another move
        let me = SigningKey::from_bytes(&[7; 32]);
        let public = crate::person::hex(me.verifying_key().as_bytes());
        crate::person::pair(&home, &public).expect("pair");
        let runs = vec!["old".to_string()];
        let other = crate::person::hex(&me.sign(&crate::person::move_message(&["r1".to_string()])).to_bytes());
        assert!(crate::person::submit_move(&home, &runs, &public, &other).is_err(), "a signature over another move");
        assert!(!file.exists());
        let sig = crate::person::hex(&me.sign(&crate::person::move_message(&runs)).to_bytes());
        assert_eq!(crate::person::submit_move(&home, &runs, &public, &sig), Ok(1));
        assert_eq!(crate::moved::read(&home).expect("read")[0].signature.as_deref(), Some(sig.as_str()));
        // moved: in Finished with its grey line, nowhere else, and its page says when
        let side = page.render(View::Overview, 0).expect("page");
        assert!(!side.contains("From an older Jaynes") && side.contains("moved by you · started on Jaynes 0.0.1, no longer on this Mac"), "{side}");
        let card = page.render(View::Run("old".into()), 0).expect("page");
        assert!(card.contains("You moved it to Finished on ") && !card.contains(">Move to Finished</button>"), "{card}");
        // the page's way, after: already moved, nothing more; the run's record is as it was
        assert_eq!(act_in(&home, &exe, &json!({"run": "old", "action": "move-to-finished"}), By::Person), Ok(json!({"ok": true, "moved": 0})));
        assert_eq!(Store::open(&home.join("record.sqlite3")).expect("store").events("old").expect("events").len(), events.len());
        let _gone = std::fs::remove_dir_all(&home);
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
