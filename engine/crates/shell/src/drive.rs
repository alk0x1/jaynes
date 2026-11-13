//! The loop around the core: take an input, let the core decide, record what it decided in one transaction, carry
//! out its commands, and turn what happened into the next inputs. One process drives a run at a time; inputs from
//! people wait in the inbox until that process takes them.

use crate::agent::{self, Agent, Ended};
use crate::project::{Copy, EXCHANGE};
use crate::protocol::{Body, Call, Given};
use crate::role::RoleProgram;
use crate::store::Store;
use jaynes_core::event::{Event, Read};
use jaynes_core::manifest::ProjectAccess;
use jaynes_core::question::{Answer, Question};
use jaynes_core::{AgentTurn, AttemptOutcome, CheckResult, CheckVerdict, Command, Digest, Input, Run};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Setup {
    pub run: String,
    pub dir: PathBuf,
    pub role: RoleProgram,
    pub copy: Option<Copy>,
    pub agent: Box<dyn Agent>,
}

impl Setup {
    pub fn load(store: &Store, home: &Path, run: &str) -> Result<Setup, String> {
        let s = store.setup(run)?;
        let dir = home.join("runs").join(run);
        let mut role = RoleProgram::new(&dir.join("role"), &dir.join("library"));
        role.pidfile = Some(dir.join("role.pid"));
        let copy = match (s["project"].as_str(), s["branch"].as_str()) {
            (Some(p), Some(b)) => Some(Copy { source: PathBuf::from(p), dir: dir.join("project"), branch: b.to_string() }),
            _ => None,
        };
        let agent = agent_named(s["agent"].as_str().unwrap_or("claude"));
        Ok(Setup { run: run.to_string(), dir, role, copy, agent })
    }
}

pub fn agent_named(spec: &str) -> Box<dyn Agent> {
    match spec.strip_prefix("command:") {
        Some(cmd) => Box::new(agent::Shell { command: cmd.to_string() }),
        None => Box::new(agent::Claude::default()),
    }
}

pub fn merges(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("merges.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Revert every merge of a run and record it: the core is asked first, so nothing is reverted it would refuse.
pub fn undo(store: &mut Store, home: &Path, run: &str, note: String, by: jaynes_core::question::By) -> Result<String, String> {
    let state = store.state(run)?.ok_or("no such run")?;
    let input = Input::Undo { note, by };
    jaynes_core::decide(Some(&state), &input).map_err(|r| r.0)?;
    let setup = Setup::load(store, home, run)?;
    let copy = setup.copy.as_ref().ok_or("the run has no project")?;
    let merges = merges(&setup.dir);
    let reference = if merges.is_empty() { "nothing had been merged".to_string() } else { copy.undo(&merges, run)? };
    store.decide(run, &input)?;
    Ok(reference)
}

/// Give a run an input from outside its process (the page, an agent): recorded now and carried on in a process
/// of its own when nothing drives the run, or left in its inbox for the process that does. A leave is recorded at
/// once in any case, since an agent inside a turn waits for it. Says which.
pub fn submit(home: &Path, exe: &Path, run: &Run, input: Input) -> Result<&'static str, String> {
    jaynes_core::decide(Some(run), &input).map_err(|r| r.0)?;
    let mut store = Store::open(&home.join("record.sqlite3"))?;
    if let Input::Answer { answer } = &input
        && run.questions.get(&answer.question).is_some_and(|q| q.question.kind == jaynes_core::question::QuestionKind::Permission) {
            store.decide(&run.id, &input)?;
            return Ok("now");
        }
    let dir = home.join("runs").join(&run.id);
    // Abandoning a run stops its agent at once: nothing it would do from here is wanted, and it costs tokens. The
    // abandon is recorded or left for the run's process first, so that process starts no other turn.
    let abandon = matches!(input, Input::Abandon { .. });
    let lock = File::create(dir.join("lock")).map_err(|e| e.to_string())?;
    if lock.try_lock().is_ok() {
        store.decide(&run.id, &input)?;
        drop(lock);
        let log = File::create(dir.join("continue.log")).map_err(|e| e.to_string())?;
        let err = log.try_clone().map_err(|e| e.to_string())?;
        std::process::Command::new(exe)
            .args(["continue", &run.id])
            .env("JAYNES_ENGINE_HOME", home)
            .stdin(std::process::Stdio::null())
            .stdout(log)
            .stderr(err)
            .spawn()
            .map_err(|e| format!("the run could not be carried on: {e}"))?;
        Ok("now")
    } else {
        store.inbox_put(&run.id, &input)?;
        if abandon {
            agent::stop_left(&dir.join("agent.pid"));
        }
        Ok("inbox")
    }
}

/// What a person sees as the process goes: one line each.
pub fn say(line: &str) {
    eprintln!("{line}");
}

/// Decide on the run's latest state and record the events, in one transaction of the record.
pub fn decide_and_record(store: &mut Store, run: &str, input: &Input) -> Result<(Run, Vec<Event>, Vec<Command>), String> {
    store.decide(run, input)
}

pub struct Driver<'a> {
    pub store: &'a mut Store,
    pub setup: Setup,
    _lock: File,
}

impl<'a> Driver<'a> {
    /// Take the run, or say that another process holds it.
    pub fn take(store: &'a mut Store, setup: Setup) -> Result<Driver<'a>, String> {
        std::fs::create_dir_all(&setup.dir).map_err(|e| e.to_string())?;
        let lock = File::create(setup.dir.join("lock")).map_err(|e| e.to_string())?;
        lock.try_lock().map_err(|_| "another process is driving this run".to_string())?;
        // macOS tells no child that its parent died: an agent the last process started may still be writing.
        if agent::stop_left(&setup.dir.join("agent.pid")) {
            say("stopped an agent program that a process which died had left running");
        }
        if agent::stop_left(&setup.dir.join("role.pid")) {
            say("stopped a call to the role that a process which died had left running");
        }
        if let Some((seq, _)) = store.verify()? {
            return Err(format!("the record was changed at entry {seq}: Jaynes drives no run over a record that does not hold"));
        }
        if let Some(run) = store.state(&setup.run)?
            && run.engine != jaynes_core::ENGINE {
                return Err(format!(
                    "run {} started on engine {} and this is {}: a run stays on the engine it started with",
                    run.id, run.engine, jaynes_core::ENGINE
                ));
            }
        Ok(Driver { store, setup, _lock: lock })
    }

    /// Drive the run from `first` until nothing more can happen without someone outside.
    pub fn drive(&mut self, first: Input) -> Result<Run, String> {
        let mut queue: VecDeque<Input> = VecDeque::from([first]);
        let mut last = self.store.state(&self.setup.run)?;
        loop {
            // What came from outside (an answer, a halt, an abandon) goes before what the last turn left to do.
            let mut came = Vec::new();
            while let Some(input) = self.store.inbox_take(&self.setup.run)? {
                came.push(input);
            }
            for input in came.into_iter().rev() {
                queue.push_front(input);
            }
            let Some(input) = queue.pop_front() else { break };
            let (state, events, commands) = match decide_and_record(self.store, &self.setup.run, &input) {
                Ok(x) => x,
                Err(why) => {
                    if !matches!(input, Input::Tick) {
                        say(&format!("refused: {why}"));
                    }
                    continue;
                }
            };
            self.follow_project(&state, &events)?;
            for e in &events {
                narrate(e);
            }
            for e in &events {
                if let Event::AttemptStarted { step, fix: 0, .. } = e {
                    let agent = state.manifest.index(step).map(|i| state.manifest.steps[i].executor) == Some(jaynes_core::manifest::Executor::Agent);
                    if agent {
                        say(&format!("  {step} by {}", self.setup.agent.name().chars().take(60).collect::<String>()));
                    }
                }
            }
            let mut state = state;
            for c in commands {
                // What came from outside goes first before an agent's or a role's turn starts: a run abandoned or
                // halted meanwhile starts no turn, and the turn it had begun counts as interrupted.
                if matches!(c, Command::RunAgent(_) | Command::RunRole { .. }) {
                    let mut came = Vec::new();
                    while let Some(input) = self.store.inbox_take(&self.setup.run)? {
                        came.push(input);
                    }
                    for input in came {
                        match decide_and_record(self.store, &self.setup.run, &input) {
                            Ok((now, events, more)) => {
                                for e in &events {
                                    narrate(e);
                                }
                                state = now;
                                queue.extend(more.into_iter().filter_map(|m| self.carry_out_quietly(&state, m)));
                            }
                            Err(why) => say(&format!("refused: {why}")),
                        }
                    }
                    if state.finished() {
                        continue;
                    }
                    if state.halted {
                        if let Ok((now, events, _)) = decide_and_record(self.store, &self.setup.run, &Input::Recovered) {
                            for e in &events {
                                narrate(e);
                            }
                            state = now;
                        }
                        continue;
                    }
                }
                if let Some(next) = self.carry_out(&state, c) {
                    queue.push_back(next);
                }
            }
            last = Some(state);
        }
        last.ok_or_else(|| "the run did not start".to_string())
    }

    /// Keep the project copy in step with the record: an output that passed becomes a commit; an attempt that
    /// failed leaves nothing behind.
    fn ledger(&self) -> std::collections::BTreeMap<String, String> {
        std::fs::read_to_string(self.setup.dir.join("project-steps.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn keep_ledger(&self, l: &std::collections::BTreeMap<String, String>) -> Result<(), String> {
        std::fs::write(self.setup.dir.join("project-steps.json"), serde_json::to_string(l).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }

    /// Each step's change, by step, for the steps in `names` that changed something, in the order given.
    fn changes(&self, names: &[String]) -> Vec<(String, String)> {
        let l = self.ledger();
        names.iter().filter_map(|n| l.get(n).map(|c| (n.clone(), c.clone()))).collect()
    }

    /// Put the copy where step `i` works: the base with the changes of the earlier steps it uses, and, when it
    /// runs again, its own previous change on top.
    fn prepare(&self, run: &Run, i: usize) -> Result<(), String> {
        let Some(copy) = &self.setup.copy else { return Ok(()) };
        if run.manifest.steps[i].reach.project == ProjectAccess::None {
            return Ok(());
        }
        let writers: Vec<String> = run
            .sources(i)
            .into_iter()
            .filter(|&j| run.manifest.steps[j].reach.project == ProjectAccess::Write)
            .map(|j| run.manifest.steps[j].name.clone())
            .collect();
        let parent = copy.compose(&self.changes(&writers))?;
        let name = run.manifest.steps[i].name.clone();
        let mut parents = self.parents();
        parents.insert(name.clone(), parent);
        std::fs::write(self.setup.dir.join("project-parents.json"), serde_json::to_string(&parents).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if run.manifest.steps[i].reach.project == ProjectAccess::Write && run.steps[i].round > 0 {
            let own = self.changes(std::slice::from_ref(&name));
            let mut all = self.changes(&writers);
            all.extend(own);
            copy.compose(&all).map(|_| ())?;
        }
        Ok(())
    }

    /// The merges the run made, oldest first.
    pub fn merges(&self) -> Vec<String> {
        merges(&self.setup.dir)
    }

    fn parents(&self) -> std::collections::BTreeMap<String, String> {
        std::fs::read_to_string(self.setup.dir.join("project-parents.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn follow_project(&self, run: &Run, events: &[Event]) -> Result<(), String> {
        let Some(copy) = &self.setup.copy else { return Ok(()) };
        for e in events {
            let (step, keep) = match e {
                Event::StepDone { step, .. } => (step, true),
                Event::AttemptFailed { step, .. } | Event::AttemptInterrupted { step, .. } => (step, false),
                _ => continue,
            };
            let Some(i) = run.manifest.index(step) else { continue };
            if run.manifest.steps[i].reach.project != ProjectAccess::Write {
                continue;
            }
            if keep {
                let parent = self.parents().get(step).cloned().unwrap_or(copy.base()?);
                let mut l = self.ledger();
                match copy.result(&parent, &format!("{step}, round {}", run.steps[i].round))? {
                    Some(c) => l.insert(step.clone(), c),
                    None => l.remove(step),
                };
                self.keep_ledger(&l)?;
            } else {
                copy.discard()?;
            }
        }
        Ok(())
    }

    /// Commands that came with an input from outside, other than a turn: shown, asked, delivered. A turn waits for
    /// the loop.
    fn carry_out_quietly(&mut self, run: &Run, c: Command) -> Option<Input> {
        match c {
            Command::RunAgent(_) | Command::RunRole { .. } => None,
            other => self.carry_out(run, other),
        }
    }

    fn carry_out(&mut self, run: &Run, c: Command) -> Option<Input> {
        match c {
            Command::Probe { step, .. } => {
                let i = run.manifest.index(&step)?;
                let cwd = self.cwd(run, i, 0);
                let result = match run.manifest.steps[i].executor {
                    jaynes_core::manifest::Executor::Agent => self.setup.agent.probe(&cwd),
                    _ => self.setup.role.call(Call::Hello { engine: jaynes_core::ENGINE.into(), features: vec![], extensions: Default::default() }).map(|_| ()),
                };
                Some(Input::ProbeEnded { step, ok: result.is_ok(), reason: result.err().unwrap_or_default() })
            }
            Command::RunAgent(turn) => Some(self.agent_turn(run, turn)),
            Command::RunRole { step, round, attempt, given, previous, answers, key } => {
                Some(self.role_step(run, step, round, attempt, given, previous, answers, key))
            }
            Command::RunChecks { step, attempt, fix, output, given, checks } => {
                let results = self.checks(run, &step, &output, &given, &checks);
                Some(Input::ChecksEnded { step, attempt, fix, results })
            }
            Command::Ask { question } => {
                self.show(run, &question);
                None
            }
            Command::Permit { .. } => None,
            Command::Deliver { digest, merge, .. } => {
                let writers: Vec<String> = run
                    .path()
                    .into_iter()
                    .filter(|&j| run.manifest.steps[j].reach.project == ProjectAccess::Write)
                    .map(|j| run.manifest.steps[j].name.clone())
                    .collect();
                let (ok, reference) = match &self.setup.copy {
                    Some(copy) => match copy.deliver(&self.changes(&writers)) {
                        Ok(r) => (true, r),
                        Err(e) => (false, e),
                    },
                    None => (true, "no project; the outputs are the delivery".to_string()),
                };
                let mut reference = reference;
                if ok && merge
                    && let Some(copy) = &self.setup.copy {
                        let head = copy.head().unwrap_or_default();
                        match copy.merge(&head, &self.setup.run) {
                            Ok(Some(m)) => {
                                let mut merges = self.merges();
                                merges.push(m.clone());
                                let _kept = std::fs::write(self.setup.dir.join("merges.json"), serde_json::to_string(&merges).unwrap_or_default());
                                reference.push_str(&format!("; merged as {}", &m[..m.len().min(12)]));
                            }
                            Ok(None) => reference.push_str("; nothing new to merge"),
                            Err(why) => reference.push_str(&format!("; not merged: {why}")),
                        }
                    }
                if ok {
                    say(&format!("delivered: {reference}"));
                }
                Some(Input::DeliveryEnded { digest, ok, reference })
            }
            Command::Notify { text } => {
                say(&format!("! {text}"));
                None
            }
        }
    }

    fn cwd(&self, run: &Run, i: usize, attempt: u32) -> PathBuf {
        let decl = &run.manifest.steps[i];
        match (&self.setup.copy, decl.reach.project) {
            (Some(copy), ProjectAccess::Read | ProjectAccess::Write) => copy.dir.clone(),
            _ => self.setup.dir.join("steps").join(&decl.name).join(format!("attempt-{attempt}")),
        }
    }

    fn given(&self, reads: &[Read]) -> Result<Vec<Given>, String> {
        reads
            .iter()
            .map(|r| Ok(Given { step: r.step.clone(), output: self.store.get(&r.output)?["output"].clone() }))
            .collect()
    }

    fn output_of(&self, d: &Option<Digest>) -> Option<Value> {
        d.as_ref().and_then(|d| self.store.get(d).ok()).map(|v| v["output"].clone())
    }

    fn agent_turn(&mut self, run: &Run, turn: AgentTurn) -> Input {
        let fault = |reason: String| Input::AttemptEnded {
            step: turn.step.clone(),
            attempt: turn.attempt,
            fix: turn.fix,
            outcome: AttemptOutcome::Fault { reason },
        };
        let Some(i) = run.manifest.index(&turn.step) else { return fault("no such step".into()) };
        let cwd = self.cwd(run, i, turn.attempt);
        let exchange = cwd.join(EXCHANGE);
        let given = match self.given(&turn.given) {
            Ok(g) => g,
            Err(e) => return fault(e),
        };
        let previous = self.output_of(&turn.previous);
        if turn.fix == 0
            && let Err(e) = self.prepare(run, i) {
                return fault(e);
            }
        let prompt = if turn.fix == 0 {
            let reply = self.setup.role.call(Call::Prompt {
                step: turn.step.clone(),
                request: run.request.clone(),
                round: turn.round,
                attempt: turn.attempt,
                given: given.clone(),
                previous: previous.clone(),
                notes: turn.notes.clone(),
                failure: turn.failure.clone(),
                checks: turn.checks.clone(),
            });
            let (text, schema) = match reply {
                Ok(Body::Prompt { text, schema }) => (text, schema),
                Ok(other) => return fault(format!("the role answered a prompt with {other:?}")),
                Err(e) => return fault(e),
            };
            if let Err(e) = write_exchange(&exchange, &given, previous.as_ref()) {
                return fault(e);
            }
            compose(&run.request, &text, &schema, &turn, &given)
        } else {
            let _gone = std::fs::remove_file(exchange.join("output.json"));
            fix_prompt(&turn)
        };
        say(&format!("· {} attempt {}{}", turn.step, turn.attempt, if turn.fix > 0 { format!(", fix {}", turn.fix) } else { String::new() }));
        let reads = self.setup.copy.as_ref().map(Copy::reads).unwrap_or_default();
        let mut env: Vec<(String, String)> = vec![("JAYNES_OUTPUT".into(), exchange.join("output.json").display().to_string())];
        if let Some(copy) = &self.setup.copy {
            let venv = copy.dir.join(".venv");
            env.push(("PYTHONPATH".into(), format!("{}:{}", copy.dir.join("src").display(), copy.dir.display())));
            env.push(("UV_NO_SYNC".into(), "1".into()));
            env.push(("UV_OFFLINE".into(), "1".into()));
            if venv.exists() {
                env.push(("VIRTUAL_ENV".into(), venv.display().to_string()));
                env.push(("PATH".into(), format!("{}:{}", venv.join("bin").display(), std::env::var("PATH").unwrap_or_default())));
            }
        }
        let spec = agent::Spec {
            cwd: &cwd,
            prompt: &prompt,
            session: turn.session.as_deref(),
            reach: &turn.reach,
            deadline: Duration::from_secs(turn.deadline_seconds.max(1)),
            reads: &reads,
            env,
            transcript: Some(self.setup.dir.join("steps").join(&turn.step).join(format!("turn-{}-{}.jsonl", turn.attempt, turn.fix))),
            pidfile: Some(self.setup.dir.join("agent.pid")),
            permit: std::env::current_exe().ok().map(|exe| {
                let home = self.setup.dir.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
                let argv = vec![
                    exe.display().to_string(), "permit".into(), "--run".into(), self.setup.run.clone(), "--step".into(),
                    turn.step.clone(), "--attempt".into(), turn.attempt.to_string(), "--cwd".into(), cwd.display().to_string(),
                    "--wait".into(), turn.deadline_seconds.to_string(),
                ];
                (argv, vec![("JAYNES_ENGINE_HOME".to_string(), home.display().to_string())])
            }),
        };
        if let Some(t) = &spec.transcript {
            let _made = std::fs::create_dir_all(t.parent().unwrap_or(Path::new(".")));
        }
        let done = self.setup.agent.turn(spec);
        let _kept = std::fs::write(exchange.join(format!("applied-{}-{}.json", turn.attempt, turn.fix)), done.applied.to_string());
        let outcome = match done.ended {
            Ended::Fault { reason } => AttemptOutcome::Fault { reason },
            Ended::Interrupted { reason } => AttemptOutcome::Interrupted { reason },
            Ended::Failed { reason, usage } => AttemptOutcome::Failed { reasons: vec![reason], usage },
            Ended::Ran { usage, session, .. } => self.delivered(run, i, &turn, &exchange, usage, session),
        };
        Input::AttemptEnded { step: turn.step, attempt: turn.attempt, fix: turn.fix, outcome }
    }

    fn delivered(&self, run: &Run, i: usize, turn: &AgentTurn, exchange: &Path, usage: jaynes_core::event::Usage, session: Option<String>) -> AttemptOutcome {
        let failed = |why: String| AttemptOutcome::Failed { reasons: vec![why], usage };
        let text = match std::fs::read_to_string(exchange.join("output.json")) {
            Ok(t) => t,
            Err(_) => return failed(format!("there is no output: write it as JSON to {EXCHANGE}/output.json")),
        };
        let output: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => return failed(format!("{EXCHANGE}/output.json is not JSON: {e}")),
        };
        let decl = &run.manifest.steps[i];
        if decl.hidden {
            // What a hidden step wrote must not stay where a later agent works.
            let _gone = std::fs::remove_file(exchange.join("output.json"));
        }
        let mut changed = Vec::new();
        let mut tree = Value::Null;
        if let (Some(copy), ProjectAccess::Write | ProjectAccess::Read) = (&self.setup.copy, decl.reach.project) {
            let write = if decl.reach.project == ProjectAccess::Write { decl.reach.write.clone() } else { Vec::new() };
            match copy.stage(&write) {
                Ok((c, t)) => {
                    changed = c;
                    tree = Value::String(t);
                }
                Err(Ok(paths)) => return failed(format!("the step changed files it may not change: {}", paths.join(", "))),
                Err(Err(e)) => return AttemptOutcome::Fault { reason: e },
            }
        }
        let object = json!({"output": output, "tree": tree});
        let digest = match self.store.put(&object) {
            Ok(d) => d,
            Err(e) => return AttemptOutcome::Fault { reason: e },
        };
        let items = match self.setup.role.call(Call::Items { step: turn.step.clone(), output: object["output"].clone() }) {
            Ok(Body::Items { items }) => match self.keep_items(&items, exchange.parent().unwrap_or(exchange)) {
                Ok(i) => i,
                Err(e) => return AttemptOutcome::Fault { reason: e },
            },
            Ok(other) => return AttemptOutcome::Fault { reason: format!("the role answered items with {other:?}") },
            Err(e) => return AttemptOutcome::Fault { reason: e },
        };
        AttemptOutcome::Delivered { output: digest, items, changed, usage, session }
    }

    /// The items as the core knows them, after keeping a copy of each picture by its fingerprint and how each item
    /// is shown. A picture outside the step's folder, or that does not read, is Jaynes's fault to report.
    fn keep_items(&self, items: &[crate::protocol::ShownItem], root: &Path) -> Result<Vec<jaynes_core::question::Item>, String> {
        let home = self.setup.dir.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
        let shelf = home.join("pictures");
        let mut out = Vec::new();
        for x in items {
            let mut kept = Vec::new();
            for p in &x.pictures {
                let path = root.join(p);
                let real = std::fs::canonicalize(&path).map_err(|e| format!("the picture {p} of {} does not read: {e}", x.id))?;
                let base = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
                if !real.starts_with(&base) {
                    return Err(format!("the picture {p} of {} is outside the step's folder", x.id));
                }
                let ext = real.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                if !["png", "jpg", "jpeg", "gif", "webp", "svg"].contains(&ext.as_str()) {
                    return Err(format!("the picture {p} of {} is not an image Jaynes shows", x.id));
                }
                let bytes = std::fs::read(&real).map_err(|e| e.to_string())?;
                let name = format!("{}.{ext}", Digest::of_bytes(&bytes).0);
                std::fs::create_dir_all(&shelf).map_err(|e| e.to_string())?;
                std::fs::write(shelf.join(&name), &bytes).map_err(|e| e.to_string())?;
                kept.push(name);
            }
            let item = x.item_with(&kept);
            if !x.detail.is_empty() || !kept.is_empty() {
                self.store.put_display(&item.digest, &json!({"detail": x.detail, "pictures": kept}))?;
            }
            out.push(item);
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn role_step(&mut self, run: &Run, step: String, round: u32, attempt: u32, given: Vec<Read>, previous: Option<Digest>, answers: Vec<Answer>, key: Option<String>) -> Input {
        let end = |outcome| Input::AttemptEnded { step: step.clone(), attempt, fix: 0, outcome };
        let given = match self.given(&given) {
            Ok(g) => g,
            Err(e) => return end(AttemptOutcome::Fault { reason: e }),
        };
        let i = run.manifest.index(&step).unwrap_or(0);
        if let Err(e) = self.prepare(run, i) {
            return end(AttemptOutcome::Fault { reason: e });
        }
        let reaches = run.manifest.steps[i].reach.project != ProjectAccess::None;
        let project = self.setup.copy.as_ref().filter(|_| reaches).map(|c| c.dir.display().to_string());
        let source = self.setup.copy.as_ref().filter(|_| reaches).map(|c| c.source.display().to_string());
        say(&format!("· {step} (the role's own step)"));
        let reply = self.setup.role.call(Call::Execute {
            step: step.clone(),
            request: run.request.clone(),
            round,
            attempt,
            given,
            previous: self.output_of(&previous),
            answers,
            key,
            project,
            source,
        });
        let outcome = match reply {
            Err(e) => AttemptOutcome::Fault { reason: e },
            Ok(Body::Result { output }) => {
                let object = json!({"output": output, "tree": Value::Null});
                match self.store.put(&object) {
                    Err(e) => AttemptOutcome::Fault { reason: e },
                    Ok(digest) => match self.setup.role.call(Call::Items { step: step.clone(), output: object["output"].clone() }) {
                        Ok(Body::Items { items }) => AttemptOutcome::Delivered {
                            output: digest,
                            items: match self.keep_items(&items, &self.cwd(run, i, attempt)) {
                                Ok(i) => i,
                                Err(e) => return end(AttemptOutcome::Fault { reason: e }),
                            },
                            changed: Vec::new(),
                            usage: Default::default(),
                            session: None,
                        },
                        Ok(other) => AttemptOutcome::Fault { reason: format!("the role answered items with {other:?}") },
                        Err(e) => AttemptOutcome::Fault { reason: e },
                    },
                }
            }
            Ok(Body::NeedsInput { items, offered, about }) => {
                match self.keep_items(&items, &self.cwd(run, i, attempt)) {
                    Ok(items) => AttemptOutcome::NeedsInput { items, offered, about },
                    Err(e) => AttemptOutcome::Fault { reason: e },
                }
            }
            Ok(Body::Refused { reason }) => AttemptOutcome::Refused { reason },
            Ok(other) => AttemptOutcome::Fault { reason: format!("the role answered execute with {other:?}") },
        };
        end(outcome)
    }

    fn checks(&self, run: &Run, step: &str, output: &Digest, given: &[Read], checks: &[String]) -> Vec<CheckResult> {
        let blocked = |check: &str, reason: String| CheckResult { check: check.to_string(), verdict: CheckVerdict::Blocked { reason } };
        let output = match self.store.get(output) {
            Ok(v) => v["output"].clone(),
            Err(e) => return checks.iter().map(|c| blocked(c, e.clone())).collect(),
        };
        let given = match self.given(given) {
            Ok(g) => g,
            Err(e) => return checks.iter().map(|c| blocked(c, e.clone())).collect(),
        };
        let i = run.manifest.index(step).unwrap_or(0);
        let reaches = run.manifest.steps[i].reach.project != ProjectAccess::None;
        let project = self.setup.copy.as_ref().filter(|_| reaches).map(|c| c.dir.display().to_string());
        let source = self.setup.copy.as_ref().filter(|_| reaches).map(|c| c.source.display().to_string());
        checks
            .iter()
            .map(|name| {
                say(&format!("  check {name}"));
                match self.setup.role.call(Call::Check {
                    step: step.to_string(),
                    check: name.clone(),
                    request: run.request.clone(),
                    output: output.clone(),
                    given: given.clone(),
                    project: project.clone(),
                    source: source.clone(),
                }) {
                    Ok(Body::Verdict { verdict }) => CheckResult { check: name.clone(), verdict },
                    Ok(other) => blocked(name, format!("the role answered a check with {other:?}")),
                    Err(e) => blocked(name, e),
                }
            })
            .collect()
    }

    fn show(&self, run: &Run, q: &Question) {
        say(&format!("? {} asks you, at {} ({}):", q.id, q.step, if q.holds { "the run waits" } else { "the run goes on" }));
        if !q.about.is_empty() {
            say(&format!("  {}", q.about));
        }
        for item in &q.items {
            say(&format!("  - {}: {}", item.id, item.label));
        }
        let offered: Vec<String> = q.offered.iter().map(|v| format!("{v:?}").to_lowercase()).collect();
        say(&format!("  answer: jaynes answer {} {} [item=verdict] ({})", run.id, q.id, offered.join(", ")));
    }
}

fn write_exchange(exchange: &Path, given: &[Given], previous: Option<&Value>) -> Result<(), String> {
    let _gone = std::fs::remove_dir_all(exchange.join("inputs"));
    let _gone = std::fs::remove_file(exchange.join("output.json"));
    let _gone = std::fs::remove_file(exchange.join("previous.json"));
    std::fs::create_dir_all(exchange.join("inputs")).map_err(|e| format!("Jaynes cannot write {}: {e}", exchange.display()))?;
    for g in given {
        std::fs::write(exchange.join("inputs").join(format!("{}.json", g.step)), g.output.to_string()).map_err(|e| e.to_string())?;
    }
    if let Some(p) = previous {
        std::fs::write(exchange.join("previous.json"), p.to_string()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The prompt of an attempt: the role's instructions, then what Jaynes says the same way for every role.
pub fn compose(request: &str, text: &str, schema: &Value, turn: &AgentTurn, given: &[Given]) -> String {
    let mut p = String::new();
    p.push_str(&format!("# The request\n\n{request}\n\n# Your step: {}\n\n{text}\n\n", turn.step));
    if !given.is_empty() {
        p.push_str("# What earlier steps made\n\nRead them in .jaynes/inputs/: ");
        p.push_str(&given.iter().map(|g| format!("{}.json", g.step)).collect::<Vec<_>>().join(", "));
        p.push_str(".\n\n");
    }
    if turn.previous.is_some() {
        p.push_str("# Your previous output\n\nThis step runs again on top of what it made before, in .jaynes/previous.json. Keep what nobody asked you to change.\n\n");
    }
    if !turn.notes.is_empty() {
        p.push_str("# The person's words for this round\n\n");
        for n in &turn.notes {
            p.push_str(&format!("- {n}\n"));
        }
        p.push('\n');
    }
    if !turn.context.is_empty() {
        p.push_str("# What the person said for context\n\nIn their own words, at earlier answers. Take it into account; it is not a request on its own.\n\n");
        for c in &turn.context {
            p.push_str(&format!("- {c}\n"));
        }
        p.push('\n');
    }
    if !turn.failure.is_empty() {
        p.push_str("# Why the last attempt was rejected\n\n");
        for f in &turn.failure {
            p.push_str(&format!("- {f}\n"));
        }
        p.push('\n');
    }
    if !turn.locked.is_empty() {
        p.push_str("# Kept by the person: keep each exactly as it is, with its id\n\n");
        for i in &turn.locked {
            p.push_str(&format!("- {} ({})\n", i.label, i.id));
        }
        p.push('\n');
    }
    if !turn.dropped.is_empty() {
        p.push_str("# Dropped by the person: none of these may come back\n\n");
        for i in &turn.dropped {
            p.push_str(&format!("- {} ({})\n", i.label, i.id));
        }
        p.push('\n');
    }
    if !turn.checks.is_empty() {
        p.push_str("# Checks your output must pass\n\nCheck them yourself before you answer; a failure comes back to you here to fix.\n\n");
        for c in &turn.checks {
            p.push_str(&format!("- {}: {}\n", c.name, c.about));
        }
        p.push('\n');
    }
    p.push_str(&format!(
        "# Your output\n\nWhen you are done, write your output as JSON to .jaynes/output.json, following this schema:\n\n{}\n",
        serde_json::to_string_pretty(schema).unwrap_or_default()
    ));
    p
}

fn fix_prompt(turn: &AgentTurn) -> String {
    let mut p = String::from("Your output failed these checks. Fix it in place, then write .jaynes/output.json again:\n\n");
    for f in &turn.failure {
        p.push_str(&format!("- {f}\n"));
    }
    p
}

pub fn narrate(e: &Event) {
    match e {
        Event::RunStarted { run, risk, reasons, ceiling, .. } => say(&format!(
            "run {run} started at {} risk{}; ceiling {} min, {} tokens",
            risk.word(),
            if reasons.is_empty() { String::new() } else { format!(" ({})", reasons.join("; ")) },
            ceiling.seconds / 60,
            ceiling.tokens
        )),
        Event::RiskRaised { to, reasons, .. } => say(&format!("risk raised to {}: {}", to.word(), reasons.join("; "))),
        Event::StepDone { step, round, .. } => say(&format!("✓ {step} (round {round})")),
        Event::AttemptFailed { step, reasons, spent: true, .. } => say(&format!("✗ {step}: {}", reasons.join("; "))),
        Event::FixAsked { step, reasons, .. } => say(&format!("↻ {step} fixes in place: {}", reasons.join("; "))),
        Event::StepInvalidated { step, because, .. } => say(&format!("↺ {step} runs again: {}", because.join("; "))),
        Event::Waiting { why, .. } => say(&format!("… {why}")),
        Event::RunFinished { outcome, cost } => say(&format!(
            "run finished, {outcome:?}: {} s, {} tokens, {} attempts ({} spent by the agent), {} fixes, {} sent back",
            cost.usage.seconds, cost.usage.tokens, cost.attempts, cost.spent, cost.fixes, cost.sent_back
        )),
        _ => {}
    }
}
