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
use jaynes_core::{decide, AgentTurn, AttemptOutcome, CheckResult, CheckVerdict, Command, Digest, Input, Run};
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
        let role = RoleProgram::new(&dir.join("role"), &dir.join("library"));
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

/// What a person sees as the process goes: one line each.
pub fn say(line: &str) {
    eprintln!("{line}");
}

/// Decide on the run's latest state and record the events, in one transaction of the record.
pub fn decide_and_record(store: &mut Store, run: &str, input: &Input) -> Result<(Run, Vec<Event>, Vec<Command>), String> {
    let state = store.state(run)?;
    let out = decide(state.as_ref(), input).map_err(|r| r.0)?;
    store.append(run, &out.events)?;
    let after = store.state(run)?.ok_or("the run has no state")?;
    Ok((after, out.events, out.commands))
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
        Ok(Driver { store, setup, _lock: lock })
    }

    /// Drive the run from `first` until nothing more can happen without someone outside.
    pub fn drive(&mut self, first: Input) -> Result<Run, String> {
        let mut queue: VecDeque<Input> = VecDeque::from([first]);
        let mut last = self.store.state(&self.setup.run)?;
        loop {
            while let Some(input) = self.store.inbox_take(&self.setup.run)? {
                queue.push_back(input);
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
            for c in commands {
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
                copy.commit(&format!("{step}, round {}", run.steps[i].round))?;
            } else {
                copy.discard()?;
            }
        }
        Ok(())
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
            Command::Deliver { digest, .. } => {
                let (ok, reference) = match &self.setup.copy {
                    Some(copy) => match copy.deliver() {
                        Ok(r) => (true, r),
                        Err(e) => (false, e),
                    },
                    None => (true, "no project; the outputs are the delivery".to_string()),
                };
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
        };
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
            Ok(Body::Items { items }) => items.iter().map(|x| x.item()).collect(),
            Ok(other) => return AttemptOutcome::Fault { reason: format!("the role answered items with {other:?}") },
            Err(e) => return AttemptOutcome::Fault { reason: e },
        };
        AttemptOutcome::Delivered { output: digest, items, changed, usage, session }
    }

    #[allow(clippy::too_many_arguments)]
    fn role_step(&mut self, run: &Run, step: String, round: u32, attempt: u32, given: Vec<Read>, previous: Option<Digest>, answers: Vec<Answer>, key: Option<String>) -> Input {
        let end = |outcome| Input::AttemptEnded { step: step.clone(), attempt, fix: 0, outcome };
        let given = match self.given(&given) {
            Ok(g) => g,
            Err(e) => return end(AttemptOutcome::Fault { reason: e }),
        };
        let i = run.manifest.index(&step).unwrap_or(0);
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
                            items: items.iter().map(|x| x.item()).collect(),
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
                AttemptOutcome::NeedsInput { items: items.iter().map(|x| x.item()).collect(), offered, about }
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
        say(&format!("  answer: jaynes-engine answer {} {} [item=verdict] ({})", run.id, q.id, offered.join(", ")));
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
