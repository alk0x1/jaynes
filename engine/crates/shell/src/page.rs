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

/// The commit of Jaynes installed when the page started: what it runs.
static RUNNING: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
/// The newest commit where Jaynes is published, asked once an hour.
static LATEST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
/// Whether an update the page started is still building.
static UPDATING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl Page {
    pub fn serve(&self) -> Result<(), String> {
        let listener = TcpListener::bind(("127.0.0.1", self.port)).map_err(|e| format!("cannot listen on {}: {e}", self.port))?;
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
                ["home"] => View::Overview,
                ["new"] => View::New,
                ["setup"] => View::Setup,
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
                "/api/start" => self.start(&body, by),
                "/api/setup" => self.setup(&body, by),
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
    /// Change the guard, update Jaynes, or restart the page; only the person does these.
    fn setup(&self, body: &Value, by: By) -> Result<Value, String> {
        use crate::install::{guard, set_guard};
        if by != By::Person {
            return Err("only you change the setup: open the link the page printed".into());
        }
        let (on, mut owners) = guard();
        let owner = body["owner"].as_str().unwrap_or("").trim().to_string();
        match body["action"].as_str().unwrap_or("") {
            "guard-on" => set_guard(true, &owners)?,
            "guard-off" => set_guard(false, &owners)?,
            "owner-add" => {
                if owner.is_empty() || !owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
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
        let recorded = drive::submit(&self.home, &self.exe, run, input)?;
        Ok(json!({"ok": true, "recorded": recorded}))
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
        let main = match (&view, &current) {
            (View::Run(id), _) => match runs.iter().find(|r| &r.id == id) {
                Some(r) => run_html(r, &changes_html(&self.home, r, None)),
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
            (_, None) => home_html(&runs, &store.times()?, waiting.len(), &self.home),
            (_, Some((run, q))) => {
                let shown = |it: &jaynes_core::question::Item| -> String {
                    let d = store.display(&it.digest);
                    let mut html = String::new();
                    if let Some(t) = d["detail"].as_str().filter(|t| !t.is_empty()) {
                        html.push_str(&format!("<p class=\"detail\">{}</p>", esc(t)));
                    }
                    let pics: Vec<String> = d["pictures"].as_array().into_iter().flatten().filter_map(|p| p.as_str()).filter_map(|name| picture(&self.home, name)).collect();
                    if !pics.is_empty() {
                        html.push_str(&format!("<div class=\"pics\">{}</div>", pics.join("")));
                    }
                    html
                };
                let i = run.manifest.index(&q.question.step);
                question_html(run, q, &shown, &changes_html(&self.home, run, i))
            }
        };
        let who = if person { "" } else { "<div class=\"notice\">You opened the page without its key: answers from here count as an agent's. Open the link the page printed.</div>" };
        Ok(PAGE.replace("{{aside}}", &aside).replace("{{main}}", &main).replace("{{who}}", &format!("{}{who}", banner())))
    }
}

/// The run's steps as a strip, one mark per step of its path, coloured by where each is, as the earlier page drew
/// it: done, waiting for you, blocked, running, not yet, or asked only a sample and skipped.
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
    use jaynes_core::manifest::Executor;
    use jaynes_core::question::By;
    use jaynes_core::run::Status;
    let path = r.path();
    let blocked = match &r.phase {
        Phase::Blocked { step: Some(s), .. } => Some(s.as_str()),
        _ => None,
    };
    let rows: String = r
        .manifest
        .steps
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let s = &r.steps[i];
            let tries = format!("{} attempt{}", s.attempt, if s.attempt == 1 { "" } else { "s" });
            let round = if s.round > 0 { format!(", round {}", s.round + 1) } else { String::new() };
            let answer = s.question.as_ref().and_then(|q| r.questions.get(q)).and_then(|q| q.answer.clone());
            let (class, detail) = if !path.contains(&i) {
                ("todo", "left out at this risk".to_string())
            } else if blocked == Some(d.name.as_str()) {
                ("blocked", format!("blocked, {tries}"))
            } else {
                match &s.status {
                    Status::Running { attempt, .. } | Status::Checking { attempt, .. } if !r.finished() => ("running", format!("running, attempt {attempt}{round}")),
                    Status::Probing if !r.finished() => ("running", "testing what it reaches".to_string()),
                    Status::AwaitingInput { .. } => ("waiting", format!("waiting for you{round}")),
                    Status::Done if r.fresh(i) && r.open_question(i).is_some() => ("waiting", format!("waiting for you{round}")),
                    Status::Done if r.fresh(i) && s.skipped == Some(s.round) => ("sampled", "not asked, sampled".to_string()),
                    Status::Done if r.fresh(i) => match (&answer, d.executor) {
                        (Some(a), _) if matches!(a.by, By::Agent { .. }) => ("checked", "an agent let it go on".to_string()),
                        (Some(_), Executor::Person) => ("accepted", "you answered".to_string()),
                        (Some(_), _) => ("accepted", format!("you kept it, {tries}")),
                        (None, Executor::Role) => ("jaynes", "by jaynes".to_string()),
                        (None, _) => ("checked", format!("checked, {tries}{round}")),
                    },
                    _ if matches!(&r.phase, Phase::Waiting { step: Some(w), .. } if *w == d.name) => ("waiting", "waiting for you".to_string()),
                    _ if s.done.is_some() => ("todo", format!("to run again{round}")),
                    _ => ("todo", String::new()),
                }
            };
            let mark = who(d, r);
            let mark = if mark.is_empty() { String::new() } else { format!("<span class=\"who\">{}</span>", esc(&mark)) };
            let about = if d.about.is_empty() { String::new() } else { format!("<p class=\"about\">{}</p>", esc(&d.about)) };
            let detail = if detail.is_empty() { String::new() } else { format!("<small>{}</small>", esc(&detail)) };
            format!("<li class=\"{class}\"><span class=\"dot\"></span><div><div class=\"r\"><b>{}{mark}</b></div>{about}{detail}</div></li>", esc(&d.name))
        })
        .collect();
    format!("<ul class=\"stages told\">{rows}</ul>")
}

/// Where a run is, in words: the step it is on, of how many.
fn where_in(r: &Run) -> String {
    let path = r.path();
    let at = path.iter().position(|&i| !r.fresh(i)).map(|n| n + 1).unwrap_or(path.len());
    format!("step {at} of {}", path.len())
}

enum View {
    Home,
    Overview,
    New,
    Setup,
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
fn run_html(r: &Run, changes: &str) -> String {
    let path = r.path();
    let steps = stage_list(r);
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
        "<div class=\"where\">{role} {version} · run {id} · {risk} risk</div><h1>{request}</h1>{strip}{phase}\
         <p class=\"sub\">Spent {sm} of {cm} min, {st} of {ct} tokens ({cached} read from the cache) · {att} attempts, {spent} spent by the agent, {fixes} fixes, {back} sent back{corr}</p>\
         {steps}\
         <div class=\"acts\">{buttons}</div>{correct}<div class=\"refused\" id=\"actmsg\"></div>{changes}<script id=\"rundata\" type=\"application/json\">{data}</script>",
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

/// A picture Jaynes kept, by its fingerprint, as an image inside the page: no address reaches a file.
fn picture(home: &std::path::Path, name: &str) -> Option<String> {
    let ok = name.len() > 65
        && name[..64].chars().all(|c| c.is_ascii_hexdigit())
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
                "<div class=\"row\" data-item=\"{id}\"><h3>{label}</h3>{more}<div class=\"seg\">{buttons}</div><input class=\"note hide\" data-i=\"{id}\"></div>",
                id = esc(&it.id),
                label = esc(&it.label),
                more = shown(it)
            )
        })
        .collect();
    let data = json!({"run": run.id, "question": question.id, "shown": question.shown().0}).to_string();
    format!(
        "<div class=\"where\"><a href=\"/run/{run_id}\">{role} · run {run_id}</a> · step {step} · round {round} · step {at} of {n}</div><h1>{title}</h1>{strip}<p class=\"sub\">{badge}</p>{phase}\
         <div class=\"say\"><label>Context</label><input id=\"ctx\" placeholder=\"Anything the agent should know, in any language (optional)\"></div>\
         {rows}{changes}<details class=\"way\"><summary>Steps</summary>{stages}</details><script id=\"data\" type=\"application/json\">{data}</script>",
        role = esc(&run.manifest.role),
        run_id = esc(&run.id),
        step = esc(&question.step),
        round = question.round,
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
    let dir = home.join("runs").join(&run.id);
    let read = |name: &str| -> BTreeMap<String, String> { std::fs::read_to_string(dir.join(name)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default() };
    let (commits, parents) = (read("project-steps.json"), read("project-parents.json"));
    let copy = dir.join("project");
    let base = std::fs::read_to_string(copy.join(".git/jaynes-base")).unwrap_or_default().trim().to_string();
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git").arg("-C").arg(&copy).args(args).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    run.manifest
        .steps
        .iter()
        .enumerate()
        .filter(|(i, _)| only.is_none_or(|o| o == *i))
        .filter_map(|(_, d)| {
            let commit = commits.get(&d.name)?;
            let parent = parents.get(&d.name).cloned().unwrap_or_else(|| base.clone());
            let mut text = git(&["diff", "--no-color", &parent, commit])?;
            if text.trim().is_empty() {
                return None;
            }
            // never cut silently: what is not shown here must still be read
            if text.len() > 40_000 {
                let cut = (0..=40_000).rev().find(|&n| text.is_char_boundary(n)).unwrap_or(0);
                let more = text.len() - cut;
                text.truncate(cut);
                text.push_str(&format!("\n[{more} more bytes not shown: read the branch before it lands]"));
            }
            let files = git(&["diff", "--name-only", &parent, commit]).unwrap_or_default();
            let n = files.lines().count();
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

/// The guard in Claude Code, and which Jaynes this is.
fn setup_html(home: &std::path::Path) -> String {
    let (on, owners) = crate::install::guard();
    let path = crate::install::guard_path().map(|p| p.display().to_string()).unwrap_or_default();
    let state = if on {
        "<span class=\"chip green\">on</span> <button class=\"act\" data-setup=\"guard-off\">Turn off</button>"
    } else {
        "<span class=\"chip red\">off</span> <button class=\"act\" data-setup=\"guard-on\">Turn on</button>"
    };
    let rows: String = owners.iter().map(|o| format!("<span class=\"owner\"><code>{}</code><button class=\"act\" data-setup=\"owner-remove\" data-owner=\"{0}\">Remove</button></span>", esc(o))).collect();
    let none = if owners.is_empty() { "<span class=\"none\">none: every held action asks, wherever it lands</span>" } else { "" };
    let installed = crate::install::installed_commit();
    let version = match &installed {
        Some(c) => format!("<code>{}</code> from {}", esc(&c[..7]), crate::install::SOURCE),
        None => "not installed with cargo from git".to_string(),
    };
    let log = std::fs::read_to_string(home.join("update.log")).unwrap_or_default();
    let log = if log.trim().is_empty() { String::new() } else { format!("<details class=\"way\"><summary>What the last update said</summary><pre class=\"said\">{}</pre></details>", esc(log.trim())) };
    format!(
        "<h1>Setup</h1>\
         <section class=\"part\"><h2>Guard</h2><p class=\"sub\">In every Claude Code session with the Jaynes plugin, it asks before what cannot be undone in git and GitHub: push, reset --hard, clean, deleting branches or tags, merging or closing pull requests, and creating or deleting repositories. Everything else passes. Repositories whose GitHub owner is a free owner pass freely.</p>\
         <table class=\"kv\"><tr><td>state</td><td>{state}</td></tr><tr><td>free owners</td><td>{rows}{none}<span class=\"owner\"><input id=\"owner\" placeholder=\"GitHub owner\"><button class=\"act\" data-setup=\"owner-add\">Add</button></span></td></tr><tr><td>settings</td><td><code>{}</code></td></tr></table></section>\
         <section class=\"part\"><h2>Version</h2><table class=\"kv\"><tr><td>installed</td><td>{version}</td></tr></table>{log}</section>\
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
fn home_html(runs: &[Run], times: &BTreeMap<String, (u64, u64)>, waiting: usize, home: &std::path::Path) -> String {
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
            let into = std::fs::read_to_string(home.join("runs").join(&r.id).join("project/.git/jaynes-start")).unwrap_or_default();
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
        "<h1>Home</h1><p class=\"sub\">{wait}</p>{}{}{}",
        part("Running", "Runs not finished: working, or waiting for you.", &moving, "Nothing runs."),
        part("Delivered", "The last 7 days, newest first. Undo and Correct are on each run's page.", &delivered(&new), "Nothing was delivered this week.") + &fold("Earlier", earlier.len(), &delivered(&earlier)),
        fold("Abandoned", abandoned, &dropped),
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
.row { padding:16px 0; border-top:1px solid var(--line); } .row h3 { margin:0 0 4px; font-size:16px; font-weight:600; }
.seg { display:flex; flex-wrap:wrap; gap:6px; margin-top:8px; }
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
.strip i.waiting { background:var(--amber); } .strip i.blocked { background:var(--red); } .strip i.running { background:var(--blue); }
.strip i.todo { background:var(--line-2); } .strip i.sampled { background:none; box-shadow:inset 0 0 0 1px var(--green); }
.stages { list-style:none; margin:8px 0 18px; padding:0; }
.stages li { display:grid; grid-template-columns:14px 1fr; gap:10px; padding:5px 0; position:relative; }
.stages li::before { content:""; position:absolute; left:6px; top:18px; bottom:-6px; width:1px; background:var(--line-2); }
.stages li:last-child::before { display:none; }
.stages .dot { width:9px; height:9px; border-radius:50%; margin:6px 0 0 2px; background:var(--green); position:relative; z-index:1; }
.stages .waiting .dot { background:var(--amber); box-shadow:0 0 0 4px var(--amber-soft); }
.stages .blocked .dot { background:var(--red); box-shadow:0 0 0 4px var(--red-soft); }
.stages .running .dot { background:var(--blue); box-shadow:0 0 0 4px var(--blue-soft); }
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
table.kv { border-collapse:collapse; margin-top:6px; } table.kv td { padding:8px 14px 8px 0; vertical-align:top; border-top:1px solid var(--line); font-size:13.5px; }
table.kv td:first-child { font-family:var(--mono); font-size:11.5px; color:var(--dim); width:120px; }
.owner { display:inline-flex; align-items:center; gap:8px; margin:0 12px 6px 0; } .owner input { background:var(--panel); color:var(--ink); border:1px solid var(--line-2); border-radius:6px; padding:5px 9px; font:inherit; font-size:13px; }
.notice.new { display:flex; align-items:center; justify-content:space-between; gap:12px; background:var(--blue-soft); color:var(--ink); }
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
<div class="top"><b>Jaynes</b><span style="color:var(--dim);font-size:13px">the new engine</span><nav class="nav"><a href="/home">Home</a><a href="/new">New run</a><a href="/setup">Setup</a></nav></div>
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
document.addEventListener("click", e => { if (e.target.matches(".pics img")) e.target.classList.toggle("big"); });
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
setInterval(async () => {
  try {
    const v = (await (await fetch("/api/version")).json()).seq;
    const busy = Object.keys(verdicts).length || [...document.querySelectorAll("input")].some(i => i.value) || document.activeElement.tagName === "INPUT";
    if (seen !== null && v !== seen && !busy) location.reload();
    seen = v;
  } catch (_) {}
}, 3000);
</script></body></html>"##;
