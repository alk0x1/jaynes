//! The page: the questions waiting for the person, one at a time, answered in rows with a line of context.
//!
//! A local web server on 127.0.0.1. Whoever opened the link with the page's key (kept as a cookie) answers as the
//! person; anyone else is an agent, and the core refuses what an agent may not answer. What an answer will do is
//! written by the core (`jaynes_core::effects`) and fetched as the person marks the rows; the page never writes it.
//! Requests must name this server as their host and origin, and posts must be JSON, so another site open in the
//! same browser cannot answer.

use crate::drive::{self, say};
use crate::store::Store;
use jaynes_core::question::{Answer, By, QuestionKind, Verdict, VerdictKind};
use jaynes_core::run::{Phase, QuestionState, Run};
use jaynes_core::{decide, Input};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

pub struct Page {
    pub home: PathBuf,
    pub port: u16,
    pub key: String,
    pub exe: PathBuf,
}

struct Request {
    method: String,
    path: String,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

pub fn key() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _read = f.read_exact(&mut bytes);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Page {
    pub fn serve(&self) -> Result<(), String> {
        let listener = TcpListener::bind(("127.0.0.1", self.port)).map_err(|e| format!("cannot listen on {}: {e}", self.port))?;
        say(&format!("the page: http://127.0.0.1:{}/?key={}", self.port, self.key));
        for stream in listener.incoming().flatten() {
            let _done = self.handle(stream);
        }
        Ok(())
    }

    fn handle(&self, mut stream: TcpStream) -> Result<(), String> {
        let req = read(&stream)?;
        let (status, kind, body, extra) = self.route(&req);
        let mut head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Frame-Options: DENY\r\nConnection: close\r\n",
            body.len()
        );
        for (k, v) in extra {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
        stream.write_all(body.as_bytes()).map_err(|e| e.to_string())
    }

    fn local(&self, value: Option<&String>, scheme: bool) -> bool {
        let p = self.port;
        let ok: Vec<String> = if scheme {
            vec![format!("http://127.0.0.1:{p}"), format!("http://localhost:{p}")]
        } else {
            vec![format!("127.0.0.1:{p}"), format!("localhost:{p}")]
        };
        value.is_some_and(|v| ok.contains(v))
    }

    fn person(&self, req: &Request) -> bool {
        let cookie = req.headers.get("cookie").cloned().unwrap_or_default();
        cookie.split(';').any(|c| c.trim() == format!("jaynes_key={}", self.key))
    }

    fn route(&self, req: &Request) -> (&'static str, &'static str, String, Vec<(String, String)>) {
        if !self.local(req.headers.get("host"), false) {
            return ("403 Forbidden", "text/plain", "this page answers only on its own local address".into(), vec![]);
        }
        if req.method == "GET" {
            if let Some(k) = req.query.get("key")
                && *k == self.key {
                    let cookie = format!("jaynes_key={}; HttpOnly; SameSite=Strict; Path=/", self.key);
                    return ("303 See Other", "text/plain", String::new(), vec![("Set-Cookie".into(), cookie), ("Location".into(), req.path.clone())]);
                }
            if req.path == "/api/version" {
                let seq = self.store().and_then(|s| s.last_seq()).unwrap_or(0);
                return ("200 OK", "application/json", json!({"seq": seq}).to_string(), vec![]);
            }
            let parts: Vec<&str> = req.path.trim_matches('/').split('/').collect();
            let view = match parts.as_slice() {
                ["q", run, q] => View::Question(run.to_string(), q.to_string()),
                ["run", run] => View::Run(run.to_string()),
                _ => View::Home,
            };
            return match self.render(view, self.person(req)) {
                Ok(html) => ("200 OK", "text/html; charset=utf-8", html, vec![]),
                Err(e) => ("500 Internal Server Error", "text/plain", e, vec![]),
            };
        }
        if req.method == "POST" {
            let json_body = req.headers.get("content-type").is_some_and(|c| c.starts_with("application/json"));
            let origin_ok = req.headers.get("origin").is_none_or(|o| self.local(Some(o), true));
            if !json_body || !origin_ok {
                return ("403 Forbidden", "text/plain", "only JSON from this page".into(), vec![]);
            }
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let by = if self.person(req) { By::Person } else { By::Agent { who: "a client without the page's key".into() } };
            let out = match req.path.as_str() {
                "/api/effects" => self.effects(&body, by),
                "/api/answer" => self.answer(&body, by),
                "/api/act" => self.act(&body, by),
                _ => Err("no such action".to_string()),
            };
            let reply = match out {
                Ok(v) => v,
                Err(e) => json!({"ok": false, "refused": e}),
            };
            return ("200 OK", "application/json", reply.to_string(), vec![]);
        }
        ("405 Method Not Allowed", "text/plain", String::new(), vec![])
    }

    fn store(&self) -> Result<Store, String> {
        Store::open(&self.home.join("record.sqlite3"))
    }

    fn answer_from(&self, body: &Value, by: By) -> Result<(Run, QuestionState, Answer), String> {
        let run_id = body["run"].as_str().ok_or("which run?")?;
        let qid = body["question"].as_str().ok_or("which question?")?;
        let run = self.store()?.state(run_id)?.ok_or("no such run")?;
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
        let answer = Answer { question: q.question.id.clone(), shown, verdicts, by, context };
        Ok((run, q, answer))
    }

    fn effects(&self, body: &Value, by: By) -> Result<Value, String> {
        let (run, q, answer) = self.answer_from(body, by)?;
        let fx = jaynes_core::effects(&run, &q.question, &answer);
        let refused = decide(Some(&run), &Input::Answer { answer }).err().map(|r| r.0);
        Ok(json!({"ok": refused.is_none(), "sentences": fx.sentences, "refused": refused}))
    }

    fn answer(&self, body: &Value, by: By) -> Result<Value, String> {
        let (run, _, answer) = self.answer_from(body, by)?;
        self.submit(&run, Input::Answer { answer })
    }

    /// What the person does to a run from its page: halt, resume, continue, raise, abandon, correct.
    fn act(&self, body: &Value, by: By) -> Result<Value, String> {
        let run_id = body["run"].as_str().ok_or("which run?")?;
        let run = self.store()?.state(run_id)?.ok_or("no such run")?;
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
                let mut store = self.store()?;
                let reverted = drive::undo(&mut store, &self.home, run_id, body["note"].as_str().unwrap_or("").to_string(), by)?;
                return Ok(json!({"ok": true, "recorded": reverted}));
            }
            "correct" => Input::Correct {
                note: body["note"].as_str().unwrap_or("").to_string(),
                by,
                step: body["step"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
            },
            other => return Err(format!("no action {other}")),
        };
        self.submit(&run, input)
    }

    /// Record an input now and carry the run on in a process of its own, or leave it in the inbox of the process
    /// that drives the run.
    fn submit(&self, run: &Run, input: Input) -> Result<Value, String> {
        decide(Some(run), &input).map_err(|r| r.0)?;
        let mut store = self.store()?;
        // A leave is answered while the agent waits for it, inside a turn the run's process is blocked on: it is
        // recorded at once, and the agent's leave server reads it from the record.
        if let Input::Answer { answer } = &input
            && run.questions.get(&answer.question).is_some_and(|q| q.question.kind == QuestionKind::Permission) {
                store.decide(&run.id, &input)?;
                return Ok(json!({"ok": true, "recorded": "now"}));
            }
        let lock = std::fs::File::create(self.home.join("runs").join(&run.id).join("lock")).map_err(|e| e.to_string())?;
        if lock.try_lock().is_ok() {
            drive::decide_and_record(&mut store, &run.id, &input)?;
            drop(lock);
            // The process that carries the run on, so the page never waits on an agent.
            let log = std::fs::File::create(self.home.join("runs").join(&run.id).join("continue.log")).map_err(|e| e.to_string())?;
            let err = log.try_clone().map_err(|e| e.to_string())?;
            std::process::Command::new(&self.exe)
                .args(["continue", &run.id])
                .env("JAYNES_ENGINE_HOME", &self.home)
                .stdin(std::process::Stdio::null())
                .stdout(log)
                .stderr(err)
                .spawn()
                .map_err(|e| format!("the run could not be carried on: {e}"))?;
            Ok(json!({"ok": true, "recorded": "now"}))
        } else {
            store.inbox_put(&run.id, &input)?;
            Ok(json!({"ok": true, "recorded": "inbox"}))
        }
    }

    fn render(&self, view: View, person: bool) -> Result<String, String> {
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
        let current = if let View::Run(_) = view {
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
                        "<a class=\"q{}\" href=\"/q/{}/{}\">{}<small>{} · {}</small></a>",
                        if on { " on" } else { "" },
                        esc(&run.id),
                        esc(&q.question.id),
                        esc(&title(q)),
                        esc(&run.manifest.role),
                        status
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
            format!("<a class=\"q{}\" href=\"/run/{}\">{}<small>{} · {}</small></a>", if on { " on" } else { "" }, esc(&r.id), esc(&short(&r.request)), esc(&r.manifest.role), phase_word(r))
        };
        aside.push_str(&format!("<div class=\"qh\" style=\"margin-top:18px\">Runs · {}</div>{}", open.len(), open.iter().map(run_link).collect::<String>()));
        if !done.is_empty() {
            aside.push_str(&format!("<div class=\"qh\" style=\"margin-top:18px\">Finished</div>{}", done.iter().map(run_link).collect::<String>()));
        }
        let main = match (&view, &current) {
            (View::Run(id), _) => match runs.iter().find(|r| &r.id == id) {
                Some(r) => run_html(r),
                None => "<div class=\"empty\">There is no such run.</div>".to_string(),
            },
            (_, None) => "<div class=\"empty\">Nothing waits for you.</div>".to_string(),
            (_, Some((run, q))) => question_html(run, q),
        };
        let who = if person { "" } else { "<div class=\"notice\">You opened the page without its key: answers from here count as an agent's. Open the link the page printed.</div>" };
        Ok(PAGE.replace("{{aside}}", &aside).replace("{{main}}", &main).replace("{{who}}", who))
    }
}

enum View {
    Home,
    Question(String, String),
    Run(String),
}

fn short(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= 60 { s.to_string() } else { format!("{}…", s.chars().take(59).collect::<String>()) }
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

/// A run: where it is, what it cost, its steps, and what the person may do to it.
fn run_html(r: &Run) -> String {
    use jaynes_core::run::Status;
    let path = r.path();
    let steps: String = r
        .manifest
        .steps
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let s = &r.steps[i];
            let state = if !path.contains(&i) {
                "left out at this risk".to_string()
            } else if s.skipped == Some(s.round) && s.status == Status::Done {
                "done · sampled, not asked".to_string()
            } else {
                match &s.status {
                    Status::Done if r.fresh(i) => "done".into(),
                    Status::Done => "to run again".into(),
                    Status::Running { attempt, fix } => format!("running · attempt {attempt}{}", if *fix > 0 { format!(", fix {fix}") } else { String::new() }),
                    Status::Checking { .. } => "checking".into(),
                    Status::Probing => "testing what it reaches".into(),
                    Status::AwaitingInput { .. } => "asks you".into(),
                    Status::Idle if s.done.is_some() => "to run again".into(),
                    Status::Idle => "not yet".into(),
                }
            };
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&d.name),
                format!("{:?}", d.executor).to_lowercase(),
                s.round,
                s.spent,
                esc(&state)
            )
        })
        .collect();
    let phase = match &r.phase {
        Phase::Blocked { reason, fault, .. } => format!("<div class=\"notice bad\">Blocked ({}): {}</div>", format!("{fault:?}").to_lowercase(), esc(reason)),
        Phase::Waiting { why, .. } => format!("<div class=\"notice\">{}</div>", esc(why)),
        Phase::Finished { outcome } => format!("<div class=\"notice ok\">Finished: {}</div>", format!("{outcome:?}").to_lowercase()),
        Phase::Running => String::new(),
    };
    let finished = r.finished();
    let mut buttons = String::new();
    let mut button = |act: &str, text: &str, cls: &str| buttons.push_str(&format!("<button class=\"act {cls}\" data-act=\"{act}\">{text}</button>"));
    if !finished {
        if r.halted {
            button("resume", "Resume", "");
        } else {
            button("halt", "Halt", "");
        }
        if r.blocked() {
            button("continue", "Continue", "");
        }
        if r.risk == jaynes_core::risk::Risk::Low {
            button("raise-risk", "Raise the risk", "");
        }
        button("raise-ceiling", "Raise the ceiling by 30 min", "");
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
        "<div class=\"where\">{role} {version} · run {id} · {risk} risk</div><h1>{request}</h1>{phase}\
         <p class=\"sub\">Spent {sm} of {cm} min, {st} of {ct} tokens ({cached} read from the cache) · {att} attempts, {spent} spent by the agent, {fixes} fixes, {back} sent back{corr}</p>\
         <table class=\"steps\"><tr><th>Step</th><th>By</th><th>Round</th><th>Attempts spent</th><th>State</th></tr>{steps}</table>\
         <div class=\"acts\">{buttons}</div>{correct}<div class=\"refused\" id=\"actmsg\"></div><script id=\"rundata\" type=\"application/json\">{data}</script>",
        role = esc(&r.manifest.role),
        version = esc(&r.manifest.version),
        id = esc(&r.id),
        risk = r.risk.word(),
        request = esc(&short(&r.request)),
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

fn title(q: &QuestionState) -> String {
    if !q.question.about.trim().is_empty() {
        q.question.about.clone()
    } else if q.question.kind == QuestionKind::Permission {
        format!("May the agent {}?", q.question.about)
    } else {
        format!("Does what {} made go on?", q.question.step)
    }
}

fn label(v: VerdictKind, permission: bool) -> &'static str {
    match (v, permission) {
        (VerdictKind::Keep, true) => "Allow",
        (VerdictKind::Drop, true) => "Refuse",
        (VerdictKind::Keep, _) => "Keep",
        (VerdictKind::Change, _) => "Change",
        (VerdictKind::Redo, _) => "Redo",
        (VerdictKind::Drop, _) => "Drop",
        (VerdictKind::Skip, _) => "I don't know",
    }
}

fn question_html(run: &Run, q: &QuestionState) -> String {
    let question = &q.question;
    let permission = question.kind == QuestionKind::Permission;
    let i = run.manifest.index(&question.step).unwrap_or(0);
    let path = run.path();
    let at = path.iter().position(|&j| j == i).map(|n| n + 1).unwrap_or(0);
    let badge = if question.holds {
        "<span class=\"badge wait\">● The run waits for your answer</span>"
    } else {
        "<span class=\"badge\">● The run goes on while you look</span>"
    };
    let phase = match &run.phase {
        Phase::Blocked { reason, .. } => format!("<div class=\"notice\">The run is blocked: {}</div>", esc(reason)),
        _ => String::new(),
    };
    let rows: String = question
        .items
        .iter()
        .map(|it| {
            let buttons: String = question
                .offered
                .iter()
                .map(|v| {
                    let key = format!("{v:?}").to_lowercase();
                    format!("<button class=\"{key}\" data-i=\"{}\" data-v=\"{key}\">{}</button>", esc(&it.id), label(*v, permission))
                })
                .collect();
            format!(
                "<div class=\"row\" data-item=\"{id}\"><h3>{label}</h3><div class=\"seg\">{buttons}</div><input class=\"note hide\" data-i=\"{id}\"></div>",
                id = esc(&it.id),
                label = esc(&it.label)
            )
        })
        .collect();
    let data = json!({"run": run.id, "question": question.id, "shown": question.shown().0}).to_string();
    format!(
        "<div class=\"where\"><a href=\"/run/{run_id}\">{role} · run {run_id}</a> · step {step} · round {round} · step {at} of {n}</div><h1>{title}</h1><p class=\"sub\">{badge}</p>{phase}\
         <div class=\"say\"><label>Context</label><input id=\"ctx\" placeholder=\"Anything the agent should know, in any language (optional)\"></div>\
         {rows}<script id=\"data\" type=\"application/json\">{data}</script>",
        role = esc(&run.manifest.role),
        run_id = esc(&run.id),
        step = esc(&question.step),
        round = question.round,
        n = path.len(),
        title = esc(&title(q)),
        data = data.replace('<', "\\u003c"),
    )
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

fn read(stream: &TcpStream) -> Result<Request, String> {
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    r.read_line(&mut line).map_err(|e| e.to_string())?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let (path, q) = target.split_once('?').map(|(p, q)| (p.to_string(), q.to_string())).unwrap_or((target, String::new()));
    let query = q.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let mut headers = BTreeMap::new();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).map_err(|e| e.to_string())? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0).min(1 << 20);
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).map_err(|e| e.to_string())?;
    Ok(Request { method, path, query, headers, body })
}

const PAGE: &str = r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Jaynes</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Instrument+Sans:wght@400;500;600&family=IBM+Plex+Mono:wght@400;500&display=swap">
<style>
:root { --bg:#07090d; --panel:#0e1218; --panel-2:#121823; --line:#222a36; --line-2:#2c3646; --ink:#eef2f7; --dim:#93a0b2; --faint:#5d6a7c;
  --blue:#6fb4ff; --blue-soft:rgba(111,180,255,.12); --amber:#ffb35c; --amber-soft:rgba(255,179,92,.13); --green:#69dba0; --green-soft:rgba(105,219,160,.13);
  --red:#ff6f6f; --red-soft:rgba(255,111,111,.12); --display:"Instrument Sans","Helvetica Neue",Arial,sans-serif; --mono:"IBM Plex Mono",ui-monospace,Menlo,monospace; }
* { box-sizing:border-box; } html,body { margin:0; background:var(--bg); color:var(--ink); font-family:var(--display); font-size:15px; line-height:1.5; }
button { font:inherit; color:inherit; cursor:pointer; } a { color:inherit; text-decoration:none; }
.top { display:flex; align-items:center; gap:18px; padding:14px 24px; border-bottom:1px solid var(--line); } .top b { font-weight:600; }
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
.row { padding:16px 0; border-top:1px solid var(--line); } .row h3 { margin:0 0 4px; font-size:16px; font-weight:600; }
.seg { display:flex; flex-wrap:wrap; gap:6px; margin-top:8px; }
.seg button { background:var(--panel); border:1px solid var(--line-2); border-radius:8px; padding:6px 12px; font-size:13px; color:var(--dim); }
.seg button.on.keep { border-color:var(--green); color:var(--green); background:var(--green-soft); }
.seg button.on.change,.seg button.on.redo { border-color:var(--amber); color:var(--amber); background:var(--amber-soft); }
.seg button.on.drop { border-color:var(--red); color:var(--red); background:var(--red-soft); }
.seg button.on.skip { border-color:var(--line-2); color:var(--ink); background:var(--panel-2); }
.note { margin-top:8px; width:100%; background:var(--panel); border:1px solid var(--line-2); border-radius:8px; color:var(--ink); padding:8px 10px; font:inherit; font-size:14px; }
.hide { display:none; } .empty { color:var(--dim); padding:40px 0; } .bad { color:var(--red); }
.notice.bad { background:var(--red-soft); color:var(--red); } .notice.ok { background:var(--green-soft); color:var(--green); }
.steps { width:100%; border-collapse:collapse; font-size:13.5px; margin:8px 0 18px; } .steps th { text-align:left; font-family:var(--mono); font-size:11px; color:var(--dim); font-weight:400; text-transform:uppercase; letter-spacing:.06em; padding:6px 8px; border-bottom:1px solid var(--line-2); }
.steps td { padding:7px 8px; border-bottom:1px solid var(--line); }
.acts { display:flex; flex-wrap:wrap; gap:8px; } .act { background:var(--panel); border:1px solid var(--line-2); border-radius:8px; padding:7px 14px; font-size:13px; }
.act:hover { border-color:var(--blue); } .act.danger { color:var(--red); } .act.armed { border-color:var(--red); background:var(--red-soft); }
select { background:var(--panel-2); color:var(--ink); border:1px solid var(--line-2); border-radius:6px; padding:5px; font:inherit; font-size:13px; }
.effects { position:fixed; left:280px; right:0; bottom:0; background:rgba(14,18,24,.97); border-top:1px solid var(--line-2); padding:14px 32px; display:flex; gap:24px; align-items:flex-start; }
.effects.hide { display:none; } .effects .what { flex:1; max-width:700px; } .label { font-family:var(--mono); font-size:11px; color:var(--dim); text-transform:uppercase; letter-spacing:.08em; margin-bottom:4px; }
.effects ul { margin:0; padding-left:18px; font-size:13.5px; } .refused { color:var(--red); font-size:13px; margin-top:6px; }
.send { background:var(--blue); color:#04121f; border:0; border-radius:10px; padding:10px 22px; font-weight:600; } .send:disabled { background:var(--line-2); color:var(--faint); cursor:default; }
</style></head><body>
<div class="top"><b>Jaynes</b><span style="color:var(--dim);font-size:13px">the new engine</span></div>
<div class="wrap"><aside>{{aside}}</aside><main>{{who}}{{main}}</main></div>
<div class="effects" id="fxbar"><div class="what"><div class="label">What this answer will do · written by Jaynes, not by the role</div><ul id="fx"></ul><div class="refused" id="refused"></div></div><button class="send" id="send">Send</button></div>
<script>
const el = document.getElementById("data");
if (!el) document.getElementById("fxbar").classList.add("hide");
const base = el ? JSON.parse(el.textContent) : null;
const verdicts = {}, notes = {};
function body() { return JSON.stringify({ ...base, verdicts, notes, context: (document.getElementById("ctx") || {}).value || "" }); }
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
document.addEventListener("click", async e => {
  const b = e.target.closest("button"); if (!b || !base) return;
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
  refresh();
});
if (base) refresh();
const rd = document.getElementById("rundata");
document.addEventListener("click", async e => {
  const b = e.target.closest("button.act"); if (!b || !rd) return;
  if ((b.dataset.act === "abandon" || b.dataset.act === "undo") && !b.classList.contains("armed")) { b.classList.add("armed"); b.textContent += ": press again"; return; }
  const run = JSON.parse(rd.textContent).run;
  const extra = b.dataset.act === "correct" ? { note: document.getElementById("cnote").value, step: document.getElementById("cstep").value } : {};
  const r = await (await fetch("/api/act", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ run, action: b.dataset.act, ...extra }) })).json();
  if (r.ok) location.reload(); else document.getElementById("actmsg").textContent = r.refused;
});
// The page follows the record, but never reloads under the person's hands.
let seen = null;
setInterval(async () => {
  try {
    const v = (await (await fetch("/api/version")).json()).seq;
    const busy = Object.keys(verdicts).length || [...document.querySelectorAll("input")].some(i => i.value) || document.activeElement.tagName === "INPUT";
    if (seen !== null && v !== seen && !busy) location.reload();
    seen = v;
  } catch (_) {}
}, 3000);
</script></body></html>"##;
