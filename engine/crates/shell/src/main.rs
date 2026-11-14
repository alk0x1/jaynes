//! `jaynes`: the shell around the core, on the command line.

mod agent;
mod conformance;
mod drive;
mod install;
mod mcp;
mod page;
mod permit;
mod project;
mod protocol;
mod role;
mod store;
mod toml_role;

use drive::{say, Driver, Setup};
use jaynes_core::question::{Answer, By, Verdict};
use jaynes_core::risk::Risk;
use jaynes_core::run::{Phase, Status};
use jaynes_core::{decide, Input, Manifest};
use protocol::{Body, Call};
use role::RoleProgram;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use store::Store;

const USAGE: &str = "jaynes: the redesigned Jaynes engine

  start --role NAME --request TEXT [--project DIR] [--agent claude|command:CMD, or JAYNES_AGENT] [--risk high] [--run ID]
  continue RUN            go on after a stop of the process, or after a block
  show RUN                where the run is, and what it asks you
  answer RUN QUESTION [ITEM=keep|skip|drop|change:TEXT|redo:TEXT ...] [--context TEXT] [--send]
  raise RUN high|--seconds N|--tokens N
  halt RUN | resume RUN | abandon RUN
  undo RUN [--note TEXT]   revert what a run merged
  correct RUN --note TEXT [--step STEP]  record that a delivery was wrong, so the rework it caused is counted
  runs | cost | verify | log RUN
  page [--port N]         the page where you answer, on 127.0.0.1 (default port 8750)
  add SOURCE | remove NAME | roles   install a role (a folder, or URL#path), remove one, list them
  guide                   how an agent carries out a change through Jaynes
  update                  install the newest Jaynes and update roles installed from git
  mcp                     the server agents in Claude Code use (the plugin starts it)
  conformance DIR         check a role against the protocol
  new NAME [DIR]          write a starting role.toml, with the menu in its comments
  check DIR               read a role in TOML strictly, every error with its line, and prove its promises
  schema                  print the protocol's schema

The record lives in $JAYNES_ENGINE_HOME (default ~/.jaynes/engine).";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn home() -> PathBuf {
    std::env::var("JAYNES_ENGINE_HOME").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".jaynes/engine")
    })
}

fn library() -> PathBuf {
    install::library(&home())
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn store() -> Result<Store, String> {
    Store::open(&home().join("record.sqlite3"))
}

/// Who is answering: the person at a terminal, or a program, which the core refuses.
fn who() -> By {
    if std::io::stdin().is_terminal() { By::Person } else { By::Agent { who: "a program without a terminal".into() } }
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(cmd) = args.first().map(String::as_str) else {
        println!("{USAGE}");
        return Ok(());
    };
    let rest = &args[1..];
    // A command on a run of another engine is that engine's: this process becomes it.
    if matches!(cmd, "continue" | "answer" | "raise" | "halt" | "resume" | "abandon" | "undo" | "correct")
        && let Some(id) = rest.first()
        && let Ok(Some(state)) = store().and_then(|s| s.state(id))
        && state.engine != jaynes_core::ENGINE
    {
        use std::os::unix::process::CommandExt;
        let exe = drive::engine_exe(&home(), &state.engine)
            .ok_or_else(|| format!("run {id} started on engine {}, which is no longer on this machine: this one ({}) cannot carry it on", state.engine, jaynes_core::ENGINE))?;
        return Err(format!("the engine of run {id} did not start: {}", std::process::Command::new(exe).args(args).exec()));
    }
    match cmd {
        "start" => start(rest),
        "role-serve" => {
            // the program of every role written in TOML: one request in, one reply out
            let dir = rest.first().ok_or("which role?")?;
            let library = flag(rest, "--library").map(PathBuf::from).unwrap_or_else(library);
            toml_role::serve(Path::new(dir), &library);
            Ok(())
        }
        "new" => {
            let name = rest.first().ok_or("jaynes new NAME [DIR]")?;
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return Err(format!("{name} is not a role's name: letters, digits, - and _"));
            }
            let dir = PathBuf::from(rest.get(1).cloned().unwrap_or_else(|| name.clone()));
            if dir.join(toml_role::FILE).exists() {
                return Err(format!("{} already has a role", dir.display()));
            }
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(dir.join(toml_role::FILE), toml_role::template(name)).map_err(|e| e.to_string())?;
            println!("wrote {}: write the method in it, then `jaynes check {}` until every line is ✔", dir.join(toml_role::FILE).display(), dir.display());
            Ok(())
        }
        "check" => {
            let dir = rest.first().ok_or("which role's folder?")?;
            match toml_role::TomlRole::load(Path::new(dir)) {
                Ok(role) => {
                    println!("✔ {} {} reads: {}", role.manifest.role, role.manifest.version, role.manifest.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(" → "));
                    let (lines, all) = role.prove();
                    for l in &lines {
                        println!("{l}");
                    }
                    if all { Ok(()) } else { Err("a promise of the role does not hold".into()) }
                }
                Err(errors) => Err(format!("✘ the role does not read:\n{}", errors.iter().map(|e| format!("  {e}")).collect::<Vec<_>>().join("\n"))),
            }
        }
        "act" => {
            // an act handed over by another engine, on a run of this one: JSON in, JSON out
            let id = rest.first().ok_or("which run?")?;
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map_err(|e| e.to_string())?;
            let act: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            println!("{}", drive::act_here(&home(), &exe, id, &act));
            Ok(())
        }
        "continue" => {
            let id = rest.first().ok_or("which run?")?;
            let mut s = store()?;
            let setup = Setup::load(&s, &home(), id)?;
            let state = s.state(id)?.ok_or("no such run")?;
            let input = if state.blocked() { Input::Continue { by: who() } } else { Input::Recovered };
            let mut d = Driver::take(&mut s, setup)?;
            let first = if matches!(input, Input::Continue { .. }) {
                d.drive(Input::Recovered)?;
                input
            } else {
                input
            };
            let end = d.drive(first)?;
            print_state(&end);
            Ok(())
        }
        "show" => {
            let id = rest.first().ok_or("which run?")?;
            let s = store()?;
            let state = s.state(id)?.ok_or("no such run")?;
            print_state(&state);
            Ok(())
        }
        "answer" => answer(rest),
        "raise" | "halt" | "resume" | "abandon" => {
            let id = rest.first().ok_or("which run?")?;
            let s = store()?;
            let state = s.state(id)?.ok_or("no such run")?;
            let input = match cmd {
                "halt" => Input::Halt { by: who() },
                "resume" => Input::Resume { by: who() },
                "abandon" => Input::Abandon { by: who() },
                _ => {
                    if rest.get(1).map(String::as_str) == Some("high") {
                        Input::RaiseRisk { to: Risk::High, by: who() }
                    } else {
                        let mut c = state.ceiling;
                        if let Some(n) = flag(rest, "--seconds") {
                            c.seconds = n.parse().map_err(|_| "--seconds takes a number")?;
                        }
                        if let Some(n) = flag(rest, "--tokens") {
                            c.tokens = n.parse().map_err(|_| "--tokens takes a number")?;
                        }
                        Input::RaiseCeiling { ceiling: c, by: who() }
                    }
                }
            };
            send(s, id, input)
        }
        "undo" => {
            let id = rest.first().ok_or("which run?")?;
            let note = flag(rest, "--note").unwrap_or_default();
            let mut s = store()?;
            let reverted = drive::undo(&mut s, &home(), id, note, who())?;
            say(&format!("undone: {reverted}"));
            Ok(())
        }
        "correct" => {
            let id = rest.first().ok_or("which run?")?;
            let note = flag(rest, "--note").ok_or("--note TEXT says what was wrong")?;
            let step = flag(rest, "--step");
            let mut s = store()?;
            let (_, events, _) = drive::decide_and_record(&mut s, id, &Input::Correct { note, by: who(), step })?;
            say(&format!("recorded: {} event", events.len()));
            Ok(())
        }
        "runs" => {
            let s = store()?;
            for id in s.runs()? {
                if let Some(r) = s.state(&id)? {
                    println!("{id}  {}  {}  {}", r.manifest.role, phase(&r.phase), r.request.chars().take(60).collect::<String>());
                }
            }
            Ok(())
        }
        "cost" => cost(),
        "verify" => {
            let s = store()?;
            match s.verify()? {
                None => {
                    println!("the record holds: every entry follows from the ones before it");
                    Ok(())
                }
                Some((seq, body)) => Err(format!("the record was changed at entry {seq}: {body}")),
            }
        }
        "log" => {
            let id = rest.first().ok_or("which run?")?;
            for e in store()?.events(id)? {
                println!("{}", serde_json::to_string(&e).map_err(|e| e.to_string())?);
            }
            Ok(())
        }
        "conformance" => {
            let dir = rest.first().ok_or("which role folder?")?;
            let findings = conformance::run(&RoleProgram::new(Path::new(dir), &library()));
            let failed = findings.iter().filter(|f| !f.ok).count();
            for f in &findings {
                println!("{} {}{}", if f.ok { "ok  " } else { "FAIL" }, f.case, if f.ok || f.detail.is_empty() { String::new() } else { format!(": {}", f.detail) });
            }
            if failed > 0 { Err(format!("{failed} of {} cases failed", findings.len())) } else { Ok(()) }
        }
        "page" => {
            let port = flag(rest, "--port").map(|p| p.parse().map_err(|_| "--port takes a number")).transpose()?.unwrap_or(8750);
            let key = std::env::var("JAYNES_PAGE_KEY").unwrap_or_else(|_| install::page_key(&home()));
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            page::Page { home: home(), port, key, exe }.serve()
        }
        "permit" => {
            let need = |name: &str| flag(rest, name).ok_or(format!("{name} is needed"));
            permit::Permit {
                home: home(),
                run: need("--run")?,
                step: need("--step")?,
                attempt: need("--attempt")?.parse().map_err(|_| "--attempt takes a number")?,
                cwd: PathBuf::from(need("--cwd")?),
            }
            .serve()
        }
        "add" => {
            let source = rest.first().ok_or("which role? a folder, or URL#path/in/the/repository")?;
            let name = install::add(&home(), source)?;
            say(&format!("role {name} installed"));
            Ok(())
        }
        "remove" => {
            let name = rest.first().ok_or("which role?")?;
            install::remove(&home(), name)?;
            say(&format!("role {name} removed; runs that started with it keep their own copy"));
            Ok(())
        }
        "roles" => {
            let roles = install::list(&home());
            if roles.is_empty() {
                println!("no role is installed: `jaynes guide` says how to install the ones that come with Jaynes");
            }
            for (name, version, about, origin) in roles {
                println!("{name} {version}  {about}{}", if origin.is_empty() { String::new() } else { format!("  (from {origin})") });
            }
            Ok(())
        }
        "guide" => {
            print!("{}", install::GUIDE);
            Ok(())
        }
        "update" => {
            for line in install::update(&home())? {
                say(&line);
            }
            Ok(())
        }
        // The Python engine's plugin recorded answers from Claude Code's question dialog here; this engine asks on
        // its page, so there is nothing to record.
        "answered" => {
            let _drained = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
            Ok(())
        }
        "mcp" => mcp::serve(&home()),
        "schema" => {
            println!("{}", serde_json::to_string_pretty(&protocol::schema()).map_err(|e| e.to_string())?);
            Ok(())
        }
        _ => Err(format!("unknown command {cmd}\n\n{USAGE}")),
    }
}

fn start(args: &[String]) -> Result<(), String> {
    let role_given = flag(args, "--role").ok_or("--role NAME (or a role's folder) is needed")?;
    let role_dir = install::find_role(&home(), &role_given)?.display().to_string();
    let request = flag(args, "--request").ok_or("--request TEXT is needed")?;
    let project = flag(args, "--project").map(|p| std::fs::canonicalize(&p).map_err(|e| format!("{p}: {e}"))).transpose()?;
    let agent = flag(args, "--agent").or_else(|| std::env::var("JAYNES_AGENT").ok()).unwrap_or_else(|| "claude".into());
    // the model is fixed for the run here, so every process that carries the run on uses the same one
    let agent = if agent == "claude" { format!("claude:{}", agent::Claude::default().model) } else { agent };
    let raise = match flag(args, "--risk").as_deref() {
        Some("high") => Some(Risk::High),
        Some("low") | None => None,
        Some(other) => return Err(format!("--risk is low or high, not {other}")),
    };
    let id = flag(args, "--run").unwrap_or_else(|| {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        format!("r{t}-{}", std::process::id() % 1000)
    });
    let runs = home().join("runs");
    let dir = runs.join(&id);
    if dir.exists() {
        // a start that died before the run was recorded left its folder whole (it is moved in only once complete):
        // the run never started
        if !store()?.events(&id)?.is_empty() {
            return Err(format!("run {id} exists"));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| format!("Jaynes could not clear what a start that died left in {}: {e}", dir.display()))?;
    }
    // The run's folder is made aside and moved into place whole: a start killed while it copies leaves children
    // (git, the role) still writing, and they write only into a folder nothing reads.
    clear_unfinished_starts(&runs);
    let building = runs.join(format!(".building-{id}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&building);
    let given = RoleProgram::new(Path::new(&role_dir), &library());
    let fingerprint = given.fingerprint()?;
    let role = given.pin(&building)?;
    match role.call(Call::Hello { engine: jaynes_core::ENGINE.into(), features: protocol::FEATURES.iter().map(|s| s.to_string()).collect(), extensions: Default::default() })? {
        Body::Hello { required, .. } => {
            let unknown: Vec<String> = required.into_iter().filter(|r| !protocol::FEATURES.contains(&r.as_str())).collect();
            if !unknown.is_empty() {
                return Err(format!("the role needs features this engine lacks: {}", unknown.join(", ")));
            }
        }
        other => return Err(format!("the role answered hello with {other:?}")),
    }
    let manifest: Manifest = match role.call(Call::Describe {})? {
        Body::Manifest { manifest } => manifest,
        other => return Err(format!("the role answered describe with {other:?}")),
    };
    // the program that starts the run carries it on to the end, after any update
    drive::keep_engine(&home())?;
    let branch = format!("jaynes/{id}");
    if let Some(p) = &project {
        project::Copy::make(p, &building.join("project"), &branch)?;
    }
    drop(role);
    std::fs::rename(&building, &dir).map_err(|e| format!("Jaynes could not put the run's folder in place: {e}"))?;
    let mut s = store()?;
    // Each stop's count, from everything the record holds, as it stands now.
    let history = s.history()?;
    let mut attention = BTreeMap::new();
    for (i, step) in manifest.steps.iter().enumerate() {
        let Some(stop) = &step.stop else { continue };
        let key = jaynes_core::attention::stop_key(&manifest, &fingerprint, i);
        let c = jaynes_core::attention::count(&history, &key, &agent, store::now(), stop.budget.map(|b| b.0), jaynes_core::DELTA);
        attention.insert(step.name.clone(), c);
    }
    s.put_setup(&id, &json!({
        "role_given": role_dir, "fingerprint": fingerprint, "agent": agent,
        "project": project.as_ref().map(|p| p.display().to_string()), "branch": project.as_ref().map(|_| branch.clone()),
    }))?;
    drive::sweep_engines(&home(), &s)?;
    drive::sweep_copies(&home(), &s)?;
    let setup = Setup::load(&s, &home(), &id)?;
    let mut d = Driver::take(&mut s, setup)?;
    say(&format!("run {id}: what needs the person is on the page, {}", install::ensure_page(&home())));
    let end = d.drive(Input::Start { run: id.clone(), request, manifest, role_fingerprint: fingerprint, agent, raise, attention })?;
    print_state(&end);
    Ok(())
}

/// Remove the folders of starts that died while they made them, once nothing can still be writing in them.
fn clear_unfinished_starts(runs: &Path) {
    let old = |p: &Path| p.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 600);
    for e in std::fs::read_dir(runs).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with(".building-") && old(&e.path()) {
            let _gone = std::fs::remove_dir_all(e.path());
        }
    }
}

fn parse_verdict(s: &str) -> Result<Verdict, String> {
    let (kind, text) = s.split_once(':').map(|(k, t)| (k, t.to_string())).unwrap_or((s, String::new()));
    Ok(match kind {
        "keep" => Verdict::Keep,
        "skip" => Verdict::Skip,
        "drop" => Verdict::Drop,
        "change" => Verdict::Change { text },
        "redo" => Verdict::Redo { note: text },
        other => return Err(format!("{other} is not a verdict: keep, skip, drop, change:TEXT or redo:TEXT")),
    })
}

fn answer(args: &[String]) -> Result<(), String> {
    let id = args.first().ok_or("which run?")?;
    let qid = args.get(1).ok_or("which question?")?;
    let s = store()?;
    let state = s.state(id)?.ok_or("no such run")?;
    let q = state.questions.get(qid).ok_or_else(|| format!("run {id} has no question {qid}"))?.question.clone();
    let mut verdicts = BTreeMap::new();
    for a in args[2..].iter().filter(|a| a.contains('=')) {
        let (item, v) = a.split_once('=').unwrap_or((a, "keep"));
        verdicts.insert(item.to_string(), parse_verdict(v)?);
    }
    let context = flag(args, "--context").unwrap_or_default();
    let answer = Answer { question: q.id.clone(), shown: q.shown(), verdicts, by: who(), context };
    let effects = drive::effects(&home(), &state, &q, &answer)?;
    println!("What this answer will do:");
    for line in &effects.sentences {
        println!("  {line}");
    }
    if !args.iter().any(|a| a == "--send") {
        println!("Nothing is sent until you add --send.");
        return Ok(());
    }
    if let Err(r) = decide(Some(&state), &Input::Answer { answer: answer.clone() }) {
        return Err(format!("refused: {r}"));
    }
    send(s, id, Input::Answer { answer })
}

/// Give the run an input: drive it here if no process does, or leave it in the inbox for the one that does.
fn send(mut s: Store, id: &str, input: Input) -> Result<(), String> {
    let state = s.state(id)?.ok_or("no such run")?;
    if let Err(r) = decide(Some(&state), &input) {
        return Err(format!("refused: {r}"));
    }
    // A leave is answered while the agent waits inside a turn: it is recorded at once, for the leave server.
    if let Input::Answer { answer } = &input
        && state.questions.get(&answer.question).is_some_and(|q| q.question.kind == jaynes_core::question::QuestionKind::Permission) {
            s.decide(id, &input)?;
            say("recorded; the agent waiting for it gets it now");
            return Ok(());
        }
    let setup = Setup::load(&s, &home(), id)?;
    match Driver::take(&mut s, setup) {
        Ok(mut d) => {
            let end = d.drive(input)?;
            print_state(&end);
        }
        Err(_) => {
            s.inbox_put(id, &input)?;
            // Abandoning stops the agent at once, after the abandon is left for the run's process.
            if matches!(input, Input::Abandon { .. }) {
                agent::stop_left(&home().join("runs").join(id).join("agent.pid"));
            }
            say("recorded for the process that drives the run; it takes it after the work under way");
        }
    }
    Ok(())
}

fn phase(p: &Phase) -> String {
    match p {
        Phase::Running => "running".into(),
        Phase::Waiting { why, .. } => format!("waiting: {why}"),
        Phase::Blocked { fault, reason, .. } => format!("blocked ({fault:?}): {reason}"),
        Phase::Finished { outcome } => format!("finished: {outcome:?}").to_lowercase(),
    }
}

fn print_state(r: &jaynes_core::Run) {
    println!("run {} — {} {} — {} risk", r.id, r.manifest.role, r.manifest.version, r.risk.word());
    println!("{}{}", phase(&r.phase), if r.halted { " (halted)" } else { "" });
    println!(
        "spent {} of {} s, {} of {} tokens ({} read from cache)",
        r.spent.seconds, r.ceiling.seconds, r.spent.tokens, r.ceiling.tokens, r.spent.cached
    );
    let path = r.path();
    for (i, s) in r.steps.iter().enumerate() {
        let d = &r.manifest.steps[i];
        let mark = if !path.contains(&i) {
            "left out at this risk".to_string()
        } else {
            match &s.status {
                Status::Done if r.fresh(i) => "done".into(),
                Status::Done => "to run again".into(),
                other => format!("{other:?}").to_lowercase(),
            }
        };
        println!("  {:<16} {:<8} round {}  {mark}", d.name, format!("{:?}", d.executor).to_lowercase(), s.round);
    }
    for q in r.open_questions() {
        let q = &q.question;
        println!("? {} at {}{}", q.id, q.step, if q.holds { ", the run waits for it" } else { ", the run goes on" });
        for item in &q.items {
            println!("    {} — {}", item.id, item.label);
        }
        let offered: Vec<String> = q.offered.iter().map(|v| format!("{v:?}").to_lowercase()).collect();
        println!("    verdicts: {}", offered.join(", "));
    }
}

fn cost() -> Result<(), String> {
    let s = store()?;
    let mut by: BTreeMap<String, (u32, u64, u64, u32, u32, u32)> = BTreeMap::new();
    for id in s.runs()? {
        let Some(r) = s.state(&id)? else { continue };
        let Phase::Finished { outcome } = r.phase else { continue };
        let e = by.entry(format!("{} {:?}", r.risk.word(), outcome).to_lowercase()).or_default();
        e.0 += 1;
        e.1 += r.spent.seconds;
        e.2 += r.spent.tokens;
        e.3 += r.counts.sent_back;
        e.4 += r.counts.corrections;
        e.5 += r.counts.spent;
    }
    println!("path, runs, minutes, tokens, sent back, corrected after delivery, attempts the agent spent");
    for (k, (n, sec, tok, back, corr, spent)) in by {
        println!("{k}: {n} runs, {} min, {tok} tokens, {back} sent back, {corr} corrected, {spent} spent", sec / 60);
    }
    Ok(())
}
