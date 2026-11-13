//! The core's one function: from a run's state and one input, the events to record and what to do next.
//!
//! `decide` touches nothing outside its arguments. The shell records the events in one transaction, then carries
//! out the commands, and reports what happened as the next input. A refusal records nothing.

use crate::digest::Digest;
use crate::event::{Effects, Event, Fault, Outcome, Read};
use crate::io::{AgentTurn, AttemptOutcome, CheckResult, CheckVerdict, Command, Input};
use crate::manifest::{CheckKind, Executor, ProjectAccess, SessionKind};
use crate::question::{Answer, Ask, AskKind, By, Item, Question, QuestionKind, Verdict, VerdictKind};
use crate::risk::{self, Ceiling, Ceilings, Changed};
use crate::run::{Phase, QuestionState, Run, Status};
use crate::{ENGINE, FORMAT, PROTOCOL};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decided {
    pub events: Vec<Event>,
    pub commands: Vec<Command>,
}

/// Why an input was refused. Nothing is recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal(pub String);

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn refuse<T>(why: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal(why.into()))
}

/// The rule that answers routine leaves: reading inside the project.
pub const READ_INSIDE_PROJECT: &str = "read-inside-project";
/// The rule that lets a step run a command when its role says the step runs commands: the agent program's own box
/// confines every command it runs.
pub const COMMAND_THE_STEP_MAY_RUN: &str = "command-the-step-may-run";
/// The rule that gives a leave asked again in a run the answer the person already gave it: they are not asked twice.
pub const ANSWERED_BEFORE: &str = "answered-before";
/// The rule that raises a run's risk from what its change touches.
pub const WHAT_THE_CHANGE_TOUCHES: &str = "what-the-change-touches";
/// The rule that continues a run paused from outside once the pause is over.
pub const PAUSE_OVER: &str = "pause-over";

/// Steps `advance` may take before it stops, a guard against a loop no event can end.
const MAX_MOVES: usize = 10_000;

pub fn decide(run: Option<&Run>, input: &Input) -> Result<Decided, Refusal> {
    if let Input::Start { run: id, request, manifest, role_fingerprint, agent, raise, attention } = input {
        if run.is_some() {
            return refuse("the run already started");
        }
        if let Err(errors) = manifest.validate() {
            return refuse(format!("the role is refused: {}", errors.join("; ")));
        }
        let (mut level, mut reasons) = risk::of_role(manifest);
        if let Some(r) = raise
            && *r > level {
                level = *r;
                reasons.push("you raised it at the start".to_string());
            }
        let ceilings = Ceilings::default();
        let start = Event::RunStarted {
            run: id.clone(),
            request: request.clone(),
            manifest: manifest.clone(),
            role_fingerprint: role_fingerprint.clone(),
            agent: agent.clone(),
            engine: ENGINE.to_string(),
            protocol: PROTOCOL,
            format: FORMAT,
            risk: level,
            reasons,
            ceilings,
            ceiling: ceilings.at(level),
            // A count for another stop than the one this run has (another key) is ignored: the stop starts at none.
            attention: attention
                .iter()
                .filter(|(step, c)| manifest.index(step).is_some_and(|i| c.key == crate::attention::stop_key(manifest, role_fingerprint, i)))
                .map(|(s, c)| (s.clone(), c.clone()))
                .collect(),
        };
        let Some(state) = Run::started(&start) else { return refuse("the run could not start") };
        let mut c = Ctx { run: state, out: Decided { events: vec![start], commands: Vec::new() } };
        c.advance();
        return Ok(c.out);
    }
    let Some(run) = run else { return refuse("there is no such run") };
    let mut c = Ctx { run: run.clone(), out: Decided::default() };
    if c.run.finished() && !matches!(input, Input::Correct { .. } | Input::Undo { .. }) {
        return refuse("the run is finished");
    }
    match input {
        Input::Start { .. } => return refuse("the run already started"),
        Input::Tick => c.advance(),
        Input::ProbeEnded { step, ok, reason } => c.probe_ended(step, *ok, reason)?,
        Input::AttemptEnded { step, attempt, fix, outcome } => c.attempt_ended(step, *attempt, *fix, outcome)?,
        Input::ChecksEnded { step, attempt, fix, results } => c.checks_ended(step, *attempt, *fix, results)?,
        Input::PermissionAsked { step, attempt, ask } => c.permission(step, *attempt, ask)?,
        Input::SessionStarted { step, attempt, session } => {
            let i = c.index(step)?;
            if matches!(c.run.steps[i].status, Status::Running { attempt: a, .. } if a == *attempt) {
                c.emit(Event::SessionStarted { step: step.clone(), attempt: *attempt, session: session.clone() });
            } else {
                c.stale(step, *attempt, "a session was named by an attempt that is no longer under way");
            }
        }
        Input::Answer { answer } => c.answer(answer)?,
        Input::RaiseRisk { to, by } => {
            if !by.person() {
                return refuse("only you raise a run's risk");
            }
            if *to <= c.run.risk {
                return refuse(format!("the run is already at {} risk; a risk only goes up", c.run.risk.word()));
            }
            let ceiling = higher(c.run.ceiling, c.run.ceilings.at(*to));
            c.emit(Event::RiskRaised { to: *to, by: by.clone(), reasons: vec!["you raised it".to_string()], ceiling });
            c.advance();
        }
        Input::RaiseCeiling { ceiling, by } => {
            if !by.person() {
                return refuse("only you raise a run's ceiling");
            }
            if ceiling.seconds < c.run.ceiling.seconds || ceiling.tokens < c.run.ceiling.tokens {
                return refuse("a ceiling only goes up");
            }
            c.emit(Event::CeilingRaised { ceiling: *ceiling, by: by.clone() });
            c.advance();
        }
        Input::Halt { by } => {
            if c.run.halted {
                return refuse("the run is already halted");
            }
            c.emit(Event::Halted { by: by.clone() });
        }
        Input::Resume { by } => {
            if !by.person() {
                return refuse("only you resume a halted run");
            }
            if !c.run.halted {
                return refuse("the run is not halted");
            }
            c.emit(Event::Resumed { by: by.clone() });
            c.advance();
        }
        Input::Recovered => c.recovered(),
        Input::Continue { by } => {
            let Phase::Blocked { fault, .. } = &c.run.phase else { return refuse("the run is not blocked") };
            let allowed = by.person() || (*fault == Fault::Paused && *by == By::Rule { rule: PAUSE_OVER.to_string() });
            if !allowed {
                return refuse("only you continue a blocked run");
            }
            c.emit(Event::Continued { by: by.clone() });
            c.advance();
        }
        Input::DeliveryEnded { digest, ok, reference } => {
            if c.run.delivering.as_ref() != Some(digest) {
                return refuse("no such delivery is under way");
            }
            if *ok {
                c.emit(Event::Delivered { digest: digest.clone(), reference: reference.clone() });
                c.advance();
            } else {
                c.emit(Event::Blocked {
                    step: None,
                    fault: Fault::Jaynes,
                    reason: format!("Jaynes could not make the delivery: {reference}"),
                });
            }
        }
        Input::Abandon { by } => {
            if !by.person() {
                return refuse("only you abandon a run");
            }
            let cost = c.run.cost();
            c.emit(Event::RunFinished { outcome: Outcome::Abandoned, cost });
        }
        Input::Undo { note, by } => {
            if !by.person() {
                return refuse("only you undo what a run merged");
            }
            if c.run.manifest.delivery != crate::manifest::DeliveryKind::Merge || c.run.delivered.is_none() {
                return refuse("this run merged nothing");
            }
            if c.run.undone {
                return refuse("this run's merges were already undone");
            }
            c.emit(Event::MergeUndone { note: note.clone(), by: by.clone() });
        }
        Input::Correct { note, by, step } => {
            if c.run.delivered.is_none() {
                return refuse("only a delivered run takes a correction; while it runs, answer its questions");
            }
            if note.trim().is_empty() {
                return refuse("say what was wrong");
            }
            if let Some(s) = step {
                c.index(s)?;
            }
            c.emit(Event::DeliveryCorrected { note: note.clone(), by: by.clone(), step: step.clone() });
        }
    }
    Ok(c.out)
}

fn higher(a: Ceiling, b: Ceiling) -> Ceiling {
    Ceiling { seconds: a.seconds.max(b.seconds), tokens: a.tokens.max(b.tokens) }
}

struct Ctx {
    run: Run,
    out: Decided,
}

impl Ctx {
    fn emit(&mut self, e: Event) {
        self.run.apply(&e);
        self.out.events.push(e);
    }

    fn command(&mut self, c: Command) {
        self.out.commands.push(c);
    }

    fn name(&self, i: usize) -> String {
        self.run.manifest.steps[i].name.clone()
    }

    fn index(&self, step: &str) -> Result<usize, Refusal> {
        self.run.manifest.index(step).ok_or_else(|| Refusal(format!("the role has no step {step}")))
    }

    fn question_id(&self, step: &str) -> String {
        // `asked_order` reads the number back
        format!("q{}-{step}", self.run.questions.len() + 1)
    }

    fn wait(&mut self, step: Option<String>, on: Vec<String>, why: String) {
        let same = matches!(&self.run.phase, Phase::Waiting { step: s, on: o, .. } if *s == step && *o == on);
        if !same {
            self.emit(Event::Waiting { step, on, why });
        }
    }

    fn block(&mut self, step: Option<String>, fault: Fault, reason: String) {
        self.emit(Event::Blocked { step, fault, reason: reason.clone() });
        self.command(Command::Notify { text: reason });
    }

    // --- moving the run on -----------------------------------------------------------------------------------

    fn advance(&mut self) {
        for _ in 0..MAX_MOVES {
            if !self.move_once() {
                return;
            }
        }
    }

    /// One move of the run, if one can be made now. True when it may make another.
    fn move_once(&mut self) -> bool {
        if self.run.finished() || self.run.blocked() || self.run.halted || self.run.delivering.is_some() {
            return false;
        }
        let path = self.run.path();
        if path.iter().any(|&i| matches!(self.run.steps[i].status, Status::Running { .. } | Status::Checking { .. } | Status::Probing)) {
            return false;
        }
        if let Some(i) = self.owed_question(&path) {
            if !self.sample_skips(i) {
                self.ask_output(i);
            }
            return true;
        }
        let Some(&i) = path.iter().find(|&&i| !self.run.fresh(i)) else {
            return self.deliver();
        };
        let decl = self.run.manifest.steps[i].clone();
        let name = decl.name.clone();
        match self.run.steps[i].status.clone() {
            Status::Running { .. } | Status::Checking { .. } | Status::Probing => false,
            Status::AwaitingInput { question } => {
                self.wait(Some(name), vec![question], "the role asks you something before it goes on".to_string());
                false
            }
            Status::Done => {
                let now = self.run.reads(i);
                let then = self.run.steps[i].done.as_ref().map(|d| d.reads.clone()).unwrap_or_default();
                let changed: Vec<String> = now.iter().filter(|r| !then.contains(r)).map(|r| r.step.clone()).collect();
                self.invalidate(i, changed.iter().map(|s| format!("{s} came out different")).collect(), Vec::new());
                true
            }
            Status::Idle => self.start(i, &decl.executor, name),
        }
    }

    /// A done step whose output the person has not been asked about, though its stop asks at the run's risk.
    fn owed_question(&self, path: &[usize]) -> Option<usize> {
        path.iter().copied().find(|&i| {
            let d = &self.run.manifest.steps[i];
            d.executor != Executor::Person
                && d.stop.as_ref().is_some_and(|s| s.from_risk <= self.run.risk)
                && self.run.fresh(i)
                && self.run.steps[i].question.is_none()
                && self.run.steps[i].skipped != Some(self.run.steps[i].round)
        })
    }

    /// Whether a certified stop skips this round, by its coin: then it is recorded as skipped and not asked.
    fn sample_skips(&mut self, i: usize) -> bool {
        let Some(stop) = self.run.manifest.steps[i].stop.clone() else { return false };
        let name = self.name(i);
        let Some(count) = self.run.attention.get(&name).cloned() else { return false };
        let budget = stop.budget.map(|b| b.0);
        if count.braked || crate::attention::status(&count, budget, crate::DELTA) != crate::attention::Status::Certified {
            return false;
        }
        let round = self.run.steps[i].round;
        let coin = crate::attention::coin(&self.run.id, &name, round);
        if coin < stop.audit_rate.0 {
            return false;
        }
        self.emit(Event::StopSkipped {
            step: name,
            round,
            key: count.key.clone(),
            coin: crate::manifest::Rate(coin),
            n: count.n,
            k: count.k,
        });
        true
    }

    fn ask_output(&mut self, i: usize) {
        let Some(stop) = self.run.manifest.steps[i].stop.clone() else { return };
        let name = self.name(i);
        let s = &self.run.steps[i];
        let items = s.done.as_ref().map(|d| d.items.clone()).unwrap_or_default();
        let items = if items.is_empty() { vec![whole(&name, s.done.as_ref().map(|d| &d.output))] } else { items };
        let question = Question {
            id: self.question_id(&name),
            step: name,
            round: s.round,
            kind: QuestionKind::Decision,
            items,
            offered: stop.verdicts.clone(),
            holds: stop.holds || self.braked(i),
            about: stop.about.clone(),
        };
        self.emit(Event::QuestionOpened { question: question.clone() });
        self.command(Command::Ask { question });
    }

    /// A stop with two errors among its last ten outcomes asks every run and holds it.
    fn braked(&self, i: usize) -> bool {
        self.run.attention.get(&self.run.manifest.steps[i].name).is_some_and(|c| c.braked)
    }

    fn invalidate(&mut self, i: usize, because: Vec<String>, notes: Vec<String>) {
        if let Some(q) = self.run.open_question(i) {
            let id = q.question.id.clone();
            self.emit(Event::QuestionSuperseded { question: id });
        }
        let round = self.run.steps[i].round + 1;
        let step = self.name(i);
        self.emit(Event::StepInvalidated { step, round, because, notes });
    }

    fn start(&mut self, i: usize, executor: &Executor, name: String) -> bool {
        let decl = self.run.manifest.steps[i].clone();
        if *executor == Executor::Person {
            return self.ask_person(i);
        }
        for j in self.run.sources(i) {
            // The question as asked says whether it holds: a braked stop holds even when its role says it need not.
            if let Some(q) = self.run.open_question(j).filter(|q| q.question.holds) {
                let on = vec![q.question.id.clone()];
                let why = format!("{name} waits for your answer about {}", self.name(j));
                self.wait(Some(name), on, why);
                return false;
            }
        }
        if decl.effect.irreversible() {
            let on = self.run.unconfirmed(i);
            if !on.is_empty() {
                let why = format!("{name} cannot be undone, and rests on answers you have not given");
                self.wait(Some(name), on, why);
                return false;
            }
        }
        let (spent, ceiling) = (self.run.spent, self.run.ceiling);
        if spent.seconds >= ceiling.seconds || spent.tokens >= ceiling.tokens {
            let reason = format!(
                "the run reached its ceiling: {} of {} seconds, {} of {} tokens",
                spent.seconds, ceiling.seconds, spent.tokens, ceiling.tokens
            );
            self.block(Some(name), Fault::Ceiling, reason);
            return false;
        }
        let s = self.run.steps[i].clone();
        if s.spent >= decl.attempts {
            let reason = format!("{name} spent its {} attempts: {}", decl.attempts, s.failure.join("; "));
            self.block(Some(name), Fault::Agent, reason);
            return false;
        }
        if !s.probed {
            self.emit(Event::ProbeAsked { step: name.clone() });
            self.command(Command::Probe { step: name, reach: decl.reach.clone() });
            return false;
        }
        let attempt = s.attempt + 1;
        let key = decl.effect.irreversible().then(|| Digest::of_json(&(&self.run.id, &name, s.round)).0);
        // after the person answered the agent's "it cannot be done", it goes on in that session
        let answered = decl.executor == Executor::Agent && !s.inputs.is_empty() && s.session.is_some();
        let session = match decl.session {
            // an attempt cut off from outside goes on where it was
            _ if decl.executor == Executor::Agent && s.resume.is_some() => s.resume.clone(),
            _ if answered => s.session.clone(),
            SessionKind::Continue => self.run.manifest.steps[..i]
                .iter()
                .rposition(|b| b.executor == Executor::Agent)
                .and_then(|j| self.run.steps[j].session.clone()),
            SessionKind::Fresh => None,
        };
        let deadline_seconds = ceiling.seconds.saturating_sub(spent.seconds);
        let tokens_left = ceiling.tokens.saturating_sub(spent.tokens);
        self.emit(Event::AttemptStarted {
            step: name.clone(),
            round: s.round,
            attempt,
            fix: 0,
            session: session.clone(),
            deadline_seconds,
            tokens_left,
            key: key.clone(),
        });
        let given = self.given(i);
        let previous = (s.round > 0).then(|| s.done.as_ref().map(|d| d.output.clone())).flatten();
        match decl.executor {
            Executor::Agent => {
                let turn = self.turn(i, attempt, 0, session);
                self.command(Command::RunAgent(turn));
            }
            Executor::Role => {
                let answers = s
                    .inputs
                    .iter()
                    .filter_map(|q| self.run.questions.get(q).and_then(|q| q.answer.clone()))
                    .collect();
                self.command(Command::RunRole { step: name, round: s.round, attempt, given, previous, answers, key });
            }
            Executor::Person => {}
        }
        false
    }

    /// What step `i` is given: what it reads, never a hidden step's output when an agent does the step.
    fn given(&self, i: usize) -> Vec<Read> {
        let agent = self.run.manifest.steps[i].executor == Executor::Agent;
        self.run
            .reads(i)
            .into_iter()
            .filter(|r| !agent || self.run.manifest.index(&r.step).is_none_or(|j| !self.run.manifest.steps[j].hidden))
            .collect()
    }

    fn turn(&self, i: usize, attempt: u32, fix: u32, session: Option<String>) -> AgentTurn {
        let decl = &self.run.manifest.steps[i];
        let s = &self.run.steps[i];
        AgentTurn {
            step: decl.name.clone(),
            round: s.round,
            attempt,
            fix,
            session,
            reach: decl.reach.clone(),
            given: self.given(i),
            previous: (s.round > 0).then(|| s.done.as_ref().map(|d| d.output.clone())).flatten(),
            notes: s.notes.clone(),
            context: self
                .run
                .context
                .iter()
                .filter(|(at, _)| self.run.manifest.index(at).is_some_and(|j| j <= i))
                .map(|(_, text)| text.clone())
                .collect(),
            failure: s.failure.clone(),
            checks: decl.checks.iter().filter(|c| c.kind == CheckKind::Open).cloned().collect(),
            locked: s.locks.values().map(|(item, _)| item.clone()).collect(),
            dropped: s.dropped.clone(),
            deadline_seconds: self.run.ceiling.seconds.saturating_sub(self.run.spent.seconds),
            tokens_left: self.run.ceiling.tokens.saturating_sub(self.run.spent.tokens),
        }
    }

    /// A step of the person's: a question over the outputs of the steps it needs, which the run waits for.
    fn ask_person(&mut self, i: usize) -> bool {
        let name = self.name(i);
        if let Some(q) = self.run.open_question(i) {
            let on = vec![q.question.id.clone()];
            self.wait(Some(name.clone()), on, format!("{name} is yours to answer"));
            return false;
        }
        let decl = self.run.manifest.steps[i].clone();
        let Some(stop) = decl.stop else { return false };
        let items = decl
            .needs
            .iter()
            .filter_map(|n| self.run.manifest.index(n))
            .map(|j| whole(&self.name(j), self.run.output(j)))
            .collect();
        let question = Question {
            id: self.question_id(&name),
            step: name.clone(),
            round: self.run.steps[i].round,
            kind: QuestionKind::Decision,
            items,
            offered: stop.verdicts,
            holds: true,
            about: stop.about,
        };
        let on = vec![question.id.clone()];
        self.emit(Event::QuestionOpened { question: question.clone() });
        self.command(Command::Ask { question });
        self.wait(Some(name.clone()), on, format!("{name} is yours to answer"));
        false
    }

    fn deliver(&mut self) -> bool {
        let (digest, given) = self.run.delivery();
        if self.run.delivered.as_ref() != Some(&digest) {
            self.emit(Event::DeliveryAsked { digest: digest.clone() });
            let merge = self.run.manifest.delivery == crate::manifest::DeliveryKind::Merge && !self.run.undone;
            self.command(Command::Deliver { digest, given, merge });
            return false;
        }
        let open: Vec<String> = self.run.open_questions().iter().map(|q| q.question.id.clone()).collect();
        if !open.is_empty() {
            self.wait(None, open, "delivered; your answers can still correct it".to_string());
            return false;
        }
        let cost = self.run.cost();
        self.emit(Event::RunFinished { outcome: Outcome::Delivered, cost });
        false
    }

    // --- reports from the shell ------------------------------------------------------------------------------

    fn probe_ended(&mut self, step: &str, ok: bool, reason: &str) -> Result<(), Refusal> {
        let i = self.index(step)?;
        if self.run.steps[i].status != Status::Probing {
            return refuse(format!("no probe of {step} is under way"));
        }
        if ok {
            self.emit(Event::ProbePassed { step: step.to_string() });
            self.advance();
        } else {
            let reason = format!(
                "Jaynes tested what {step} reaches before any attempt, and it failed: {reason}. No attempt was spent"
            );
            self.block(Some(step.to_string()), Fault::Jaynes, reason);
        }
        Ok(())
    }

    fn stale(&mut self, step: &str, attempt: u32, what: &str) {
        self.emit(Event::StaleReport { step: step.to_string(), attempt, what: what.to_string() });
    }

    fn attempt_ended(&mut self, step: &str, attempt: u32, fix: u32, outcome: &AttemptOutcome) -> Result<(), Refusal> {
        let i = self.index(step)?;
        if self.run.steps[i].status != (Status::Running { attempt, fix }) {
            self.stale(step, attempt, "an attempt that is no longer under way ended");
            return Ok(());
        }
        let decl = self.run.manifest.steps[i].clone();
        let role = decl.executor == Executor::Role;
        match outcome {
            AttemptOutcome::Delivered { output, items, changed, usage, session } => {
                self.emit(Event::AttemptDelivered {
                    step: step.to_string(),
                    attempt,
                    fix,
                    output: output.clone(),
                    items: items.clone(),
                    changed: changed.clone(),
                    usage: *usage,
                    session: session.clone(),
                });
                self.reassess(i, changed);
                if decl.checks.is_empty() {
                    self.checks_ended(step, attempt, fix, &[])?;
                } else {
                    let given = self.run.reads(i); // a check reads what its step uses, hidden steps included
                    let checks = decl.checks.iter().map(|c| c.name.clone()).collect();
                    self.command(Command::RunChecks { step: step.to_string(), attempt, fix, output: output.clone(), given, checks });
                }
            }
            AttemptOutcome::Failed { reasons, usage } => {
                self.emit(Event::AttemptFailed {
                    step: step.to_string(),
                    attempt,
                    reasons: reasons.clone(),
                    usage: *usage,
                    spent: !role,
                });
                if role {
                    self.block(Some(step.to_string()), Fault::Role, format!("the role's own step {step} failed: {}", reasons.join("; ")));
                } else {
                    self.after_spent(i);
                }
            }
            AttemptOutcome::Fault { reason } => {
                self.emit(Event::AttemptFailed {
                    step: step.to_string(),
                    attempt,
                    reasons: vec![reason.clone()],
                    usage: Default::default(),
                    spent: false,
                });
                let reason = format!("a fault of Jaynes or of the machine stopped {step}: {reason}. No attempt was spent");
                self.block(Some(step.to_string()), Fault::Jaynes, reason);
            }
            AttemptOutcome::Interrupted { reason } => {
                self.emit(Event::AttemptInterrupted { step: step.to_string(), attempt, reason: reason.clone() });
                self.block(Some(step.to_string()), Fault::Paused, format!("{step} was stopped from outside: {reason}"));
            }
            AttemptOutcome::NeedsInput { items, offered, about } => {
                if !role {
                    return refuse("only a step of the role asks the person through its result");
                }
                if offered.is_empty() || items.is_empty() {
                    return refuse("a question needs items and verdicts");
                }
                let question = Question {
                    id: self.question_id(step),
                    step: step.to_string(),
                    round: self.run.steps[i].round,
                    kind: QuestionKind::Decision,
                    items: items.clone(),
                    offered: offered.clone(),
                    holds: true,
                    about: about.clone(),
                };
                let on = vec![question.id.clone()];
                self.emit(Event::InputNeeded { step: step.to_string(), attempt, question: question.clone() });
                self.command(Command::Ask { question });
                self.wait(Some(step.to_string()), on, "the role asks you something before it goes on".to_string());
            }
            AttemptOutcome::Impossible { reason, usage, session } => {
                if role {
                    return refuse("only an agent says a step cannot be done; a role step refuses");
                }
                self.emit(Event::AttemptImpossible { step: step.to_string(), attempt, reason: reason.clone(), usage: *usage, session: session.clone() });
                let question = Question {
                    id: self.question_id(step),
                    step: step.to_string(),
                    round: self.run.steps[i].round,
                    kind: QuestionKind::Decision,
                    items: vec![Item { id: "impossible".to_string(), label: reason.clone(), digest: Digest::of_json(reason) }],
                    offered: vec![VerdictKind::Change, VerdictKind::Redo],
                    holds: true,
                    about: format!("{step} says it cannot be done as asked"),
                };
                let on = vec![question.id.clone()];
                self.emit(Event::InputNeeded { step: step.to_string(), attempt, question: question.clone() });
                self.command(Command::Ask { question });
                self.wait(Some(step.to_string()), on, format!("{step} says it cannot be done as asked, and waits for you"));
            }
            AttemptOutcome::Refused { reason } => {
                self.emit(Event::AttemptFailed {
                    step: step.to_string(),
                    attempt,
                    reasons: vec![reason.clone()],
                    usage: Default::default(),
                    spent: false,
                });
                self.block(Some(step.to_string()), Fault::Role, format!("the role says {step} cannot be done: {reason}"));
            }
        }
        Ok(())
    }

    fn after_spent(&mut self, i: usize) {
        let decl = &self.run.manifest.steps[i];
        let s = &self.run.steps[i];
        if s.spent >= decl.attempts {
            let reason = format!("{} spent its {} attempts: {}", decl.name, decl.attempts, s.failure.join("; "));
            let step = decl.name.clone();
            self.block(Some(step), Fault::Agent, reason);
        } else {
            self.advance();
        }
    }

    /// Raise the run's risk when what its change touches says so. Never lowers it.
    fn reassess(&mut self, i: usize, changed: &[Changed]) {
        let mut all: Vec<Changed> = Vec::new();
        for j in self.run.path() {
            if j == i {
                continue;
            }
            if let Some(d) = &self.run.steps[j].done {
                all.extend(d.changed.iter().cloned());
            }
        }
        all.extend(changed.iter().cloned());
        let (level, reasons) = risk::of_change(&all);
        if level > self.run.risk {
            let ceiling = higher(self.run.ceiling, self.run.ceilings.at(level));
            self.emit(Event::RiskRaised {
                to: level,
                by: By::Rule { rule: WHAT_THE_CHANGE_TOUCHES.to_string() },
                reasons,
                ceiling,
            });
        }
    }

    fn checks_ended(&mut self, step: &str, attempt: u32, fix: u32, results: &[CheckResult]) -> Result<(), Refusal> {
        let i = self.index(step)?;
        if self.run.steps[i].status != (Status::Checking { attempt, fix }) {
            self.stale(step, attempt, "checks of an attempt that is no longer under way ended");
            return Ok(());
        }
        let decl = self.run.manifest.steps[i].clone();
        let sources: Vec<String> = self.run.sources(i).into_iter().map(|j| self.name(j)).collect();
        let checks = decl
            .checks
            .iter()
            .filter_map(|c| {
                let verdict = &results.iter().find(|r| r.check == c.name)?.verdict;
                let mut read = sources.clone();
                read.extend(c.reads.iter().filter(|r| !read.contains(r)).cloned().collect::<Vec<_>>());
                Some(crate::event::CheckRecord {
                    check: c.name.clone(),
                    kind: c.kind,
                    passed: *verdict == CheckVerdict::Pass,
                    blocked: matches!(verdict, CheckVerdict::Blocked { .. }),
                    read,
                })
            })
            .collect();
        self.emit(Event::ChecksJudged { step: step.to_string(), attempt, fix, checks });
        let mut shape = Vec::new();
        let mut behaviour = Vec::new();
        for c in &decl.checks {
            match results.iter().find(|r| r.check == c.name).map(|r| &r.verdict) {
                None => {
                    let reason = format!("the check {} of {step} gave no verdict. No attempt was spent", c.name);
                    self.fail_unspent(step, attempt, &reason);
                    return Ok(());
                }
                Some(CheckVerdict::Blocked { reason }) => {
                    let reason = format!("the check {} of {step} could not judge: {reason}. No attempt was spent", c.name);
                    self.fail_unspent(step, attempt, &reason);
                    return Ok(());
                }
                Some(CheckVerdict::Pass) => {}
                Some(CheckVerdict::Fail { reasons, .. }) => match c.kind {
                    CheckKind::Open => shape.extend(reasons.iter().cloned()),
                    CheckKind::Hidden => behaviour.extend(reasons.iter().cloned()),
                },
            }
        }
        shape.extend(self.lock_breaks(i));
        let Some(pending) = self.run.steps[i].pending.clone() else {
            return refuse(format!("{step} has no output to check"));
        };
        if shape.is_empty() && behaviour.is_empty() {
            let round = self.run.steps[i].round;
            let reads = self.run.reads(i);
            self.emit(Event::StepDone {
                step: step.to_string(),
                round,
                output: pending.output,
                items: pending.items,
                reads,
                changed: pending.changed,
            });
            self.advance();
            return Ok(());
        }
        if decl.executor == Executor::Role {
            let reasons: Vec<String> = shape.into_iter().chain(behaviour).collect();
            self.emit(Event::AttemptFailed { step: step.to_string(), attempt, reasons: reasons.clone(), usage: Default::default(), spent: false });
            self.block(Some(step.to_string()), Fault::Role, format!("the role's own step {step} failed its checks: {}", reasons.join("; ")));
            return Ok(());
        }
        let session = self.run.steps[i].session.clone();
        if behaviour.is_empty() && fix < decl.fixes && session.is_some() {
            self.emit(Event::FixAsked { step: step.to_string(), attempt, fix: fix + 1, reasons: shape });
            let turn = self.turn(i, attempt, fix + 1, session);
            self.command(Command::RunAgent(turn));
            return Ok(());
        }
        let reasons = shape.into_iter().chain(behaviour).collect();
        self.emit(Event::AttemptFailed { step: step.to_string(), attempt, reasons, usage: Default::default(), spent: true });
        self.after_spent(i);
        Ok(())
    }

    fn fail_unspent(&mut self, step: &str, attempt: u32, reason: &str) {
        self.emit(Event::AttemptFailed {
            step: step.to_string(),
            attempt,
            reasons: vec![reason.to_string()],
            usage: Default::default(),
            spent: false,
        });
        self.block(Some(step.to_string()), Fault::Jaynes, reason.to_string());
    }

    /// What a delivered output breaks of what the person kept or dropped.
    fn lock_breaks(&self, i: usize) -> Vec<String> {
        let s = &self.run.steps[i];
        let Some(p) = &s.pending else { return Vec::new() };
        let mut out = Vec::new();
        for (id, (kept, _)) in &s.locks {
            match p.items.iter().find(|x| &x.id == id) {
                Some(now) if now.digest == kept.digest => {}
                Some(_) => out.push(format!("“{}” was kept by the person and must stay as it was", kept.label)),
                None => out.push(format!("“{}” was kept by the person and is missing", kept.label)),
            }
        }
        for gone in &s.dropped {
            if p.items.iter().any(|x| x.id == gone.id || x.digest == gone.digest) {
                out.push(format!("“{}” was dropped by the person and may not come back", gone.label));
            }
        }
        out
    }

    fn recovered(&mut self) {
        for i in 0..self.run.steps.len() {
            let name = self.name(i);
            let decl = self.run.manifest.steps[i].clone();
            match self.run.steps[i].status.clone() {
                Status::Running { attempt, .. } => {
                    if decl.effect.irreversible() {
                        let reason = format!(
                            "{name} cannot be undone and was under way when Jaynes stopped, so it may have acted. Look, then continue or abandon the run"
                        );
                        self.block(Some(name), Fault::MayHaveActed, reason);
                    } else {
                        self.emit(Event::AttemptInterrupted { step: name, attempt, reason: "Jaynes stopped while it ran".to_string() });
                    }
                }
                Status::Checking { attempt, fix } => {
                    if let Some(p) = self.run.steps[i].pending.clone() {
                        let given = self.run.reads(i); // a check reads what its step uses, hidden steps included
                        let checks = decl.checks.iter().map(|c| c.name.clone()).collect();
                        self.command(Command::RunChecks { step: name, attempt, fix, output: p.output, given, checks });
                    }
                }
                Status::Probing => self.command(Command::Probe { step: name, reach: decl.reach.clone() }),
                _ => {}
            }
        }
        if let Some(digest) = self.run.delivering.clone() {
            let (_, given) = self.run.delivery();
            let merge = self.run.manifest.delivery == crate::manifest::DeliveryKind::Merge && !self.run.undone;
            self.command(Command::Deliver { digest, given, merge });
        }
        let open: Vec<Question> = self.run.open_questions().iter().map(|q| q.question.clone()).collect();
        for question in open {
            self.command(Command::Ask { question });
        }
        self.advance();
    }

    // --- questions and answers -------------------------------------------------------------------------------

    fn permission(&mut self, step: &str, attempt: u32, ask: &Ask) -> Result<(), Refusal> {
        let i = self.index(step)?;
        if !matches!(self.run.steps[i].status, Status::Running { attempt: a, .. } if a == attempt) {
            self.stale(step, attempt, "a leave was asked by an attempt that is no longer under way");
            return Ok(());
        }
        let about = describe_ask(ask);
        let question = Question {
            id: self.question_id(step),
            step: step.to_string(),
            round: self.run.steps[i].round,
            kind: QuestionKind::Permission,
            items: vec![Item { id: "leave".to_string(), label: about.clone(), digest: Digest::of_json(ask) }],
            offered: vec![VerdictKind::Keep, VerdictKind::Drop],
            holds: true,
            about,
        };
        // the same leave, answered by the person earlier in the run
        let before = self
            .run
            .questions
            .values()
            .filter(|q| q.question.kind == QuestionKind::Permission && q.question.items.first().map(|x| &x.digest) == question.items.first().map(|x| &x.digest))
            .filter(|q| q.answer.as_ref().is_some_and(|a| a.by.person()))
            .max_by_key(|q| asked_order(&q.question.id))
            .and_then(|q| q.answer.clone());
        self.emit(Event::QuestionOpened { question: question.clone() });
        if let Some(earlier) = before {
            let allow = earlier.verdict("leave") == Verdict::Keep;
            let answer = Answer {
                question: question.id.clone(),
                shown: question.shown(),
                verdicts: [("leave".to_string(), earlier.verdict("leave"))].into_iter().collect(),
                by: By::Rule { rule: ANSWERED_BEFORE.to_string() },
                context: earlier.context.clone(),
            };
            let effects = effects(&self.run, &question, &answer);
            self.emit(Event::AnswerRecorded { answer, effects });
            self.command(Command::Permit { step: step.to_string(), attempt, allow });
        } else if let Some(rule) = routine(&self.run.manifest.steps[i].reach, ask) {
            let answer = Answer {
                question: question.id.clone(),
                shown: question.shown(),
                verdicts: [("leave".to_string(), Verdict::Keep)].into_iter().collect(),
                by: By::Rule { rule: rule.to_string() },
                context: String::new(),
            };
            let effects = effects(&self.run, &question, &answer);
            self.emit(Event::AnswerRecorded { answer, effects });
            self.command(Command::Permit { step: step.to_string(), attempt, allow: true });
        } else {
            let on = vec![question.id.clone()];
            self.command(Command::Ask { question });
            self.wait(Some(step.to_string()), on, format!("{step} asks you for a leave"));
        }
        Ok(())
    }

    fn answer(&mut self, answer: &Answer) -> Result<(), Refusal> {
        let Some(q) = self.run.questions.get(&answer.question).cloned() else {
            return refuse(format!("there is no question {}", answer.question));
        };
        if let By::Rule { .. } = &answer.by {
            return refuse("a rule answers only inside the engine, and only routine leaves");
        }
        if q.superseded {
            return refuse("this question was replaced by a newer one, since the step ran again; answer that one");
        }
        if answer.shown != q.question.shown() {
            return refuse("the question changed since it was shown to you; look at it again");
        }
        for (id, v) in &answer.verdicts {
            if !q.question.items.iter().any(|x| &x.id == id) {
                return refuse(format!("the question has no item {id}"));
            }
            if !q.question.offered.contains(&v.kind()) {
                return refuse(format!("the verdict {:?} is not offered here", v.kind()));
            }
            match v {
                Verdict::Change { text } if text.trim().is_empty() => return refuse("say what to change"),
                _ => {}
            }
        }
        let i = self.index(&q.question.step)?;
        let executor = self.run.manifest.steps[i].executor;
        if let By::Agent { .. } = &answer.by {
            if !answer.context.trim().is_empty() {
                return refuse("context is the person's words; an agent's answer carries none");
            }
            let release = self.run.manifest.steps[i].stop.as_ref().is_some_and(|s| s.release);
            let input = self.run.steps[i].inputs.contains(&q.question.id);
            if q.question.kind != QuestionKind::Decision || executor == Executor::Person || input || !release {
                return refuse("only you answer this: an agent may only let a result go on, at a stop the role marks as a release");
            }
            if answer.verdicts.values().any(|v| *v != Verdict::Keep) {
                return refuse("an agent may only let the result go on as it is; sending it back is yours");
            }
            if self.braked(i) {
                return refuse("this stop is braked after errors: only you answer it until it proves right again");
            }
            if q.answer.is_some() {
                return refuse("this question was already answered");
            }
        }
        if q.answer.is_some() {
            let revisable = q.question.kind == QuestionKind::Decision
                && executor != Executor::Person
                && self.run.steps[i].question.as_deref() == Some(q.question.id.as_str())
                && self.run.steps[i].status == Status::Done;
            if !revisable {
                return refuse("this answer was already acted on");
            }
        }
        let effects = effects(&self.run, &q.question, answer);
        self.emit(Event::AnswerRecorded { answer: answer.clone(), effects });
        match q.question.kind {
            QuestionKind::Permission => {
                if let Status::Running { attempt, .. } = self.run.steps[i].status {
                    let allow = answer.verdict("leave") == Verdict::Keep;
                    self.command(Command::Permit { step: q.question.step.clone(), attempt, allow });
                }
            }
            QuestionKind::Decision if executor == Executor::Person => self.answered_person(i, &q, answer),
            QuestionKind::Decision => {
                let input = self.run.steps[i].inputs.contains(&q.question.id);
                if answer.sends_back() && !input {
                    let notes = notes(&q.question, answer);
                    self.invalidate(i, vec!["you sent it back".to_string()], notes);
                }
            }
        }
        self.advance();
        Ok(())
    }

    fn answered_person(&mut self, i: usize, q: &QuestionState, answer: &Answer) {
        if !answer.sends_back() {
            let reads = self.run.reads(i);
            let output = Digest::of_json(&(&q.question.items, &answer.verdicts));
            let step = self.name(i);
            let round = self.run.steps[i].round;
            self.emit(Event::StepDone { step, round, output, items: Vec::new(), reads, changed: Vec::new() });
            return;
        }
        let here = self.name(i);
        for item in &q.question.items {
            let v = answer.verdict(&item.id);
            if !v.sends_back() {
                continue;
            }
            if let Some(j) = self.run.manifest.index(&item.id) {
                let note = note(item, &v);
                self.invalidate(j, vec![format!("you sent it back at {here}")], vec![note]);
            }
        }
        self.invalidate(i, vec!["what it shows is being done again".to_string()], Vec::new());
    }
}

/// An item that stands for a step's whole output.
fn whole(step: &str, output: Option<&Digest>) -> Item {
    Item { id: step.to_string(), label: format!("what {step} made"), digest: output.cloned().unwrap_or_default() }
}

fn describe_ask(ask: &Ask) -> String {
    match ask.kind {
        AskKind::Read => format!("read {}", ask.target),
        AskKind::Write => format!("change {}", ask.target),
        AskKind::Network => format!("reach {}", ask.target),
        AskKind::Command => format!("run {}", ask.target),
    }
}

/// The rule that may grant a leave without the person, if any: reading a path inside the step's copy of the project,
/// for a step that may read it, or running a command, for a step its role lets run commands. Anything else is the
/// person's.
/// The order in which a question was asked, from its id.
pub fn asked_order(id: &str) -> usize {
    id.strip_prefix('q').and_then(|r| r.split('-').next()).and_then(|n| n.parse().ok()).unwrap_or(0)
}

pub fn routine(reach: &crate::manifest::Reach, ask: &Ask) -> Option<&'static str> {
    let t = &ask.target;
    let inside = !t.is_empty() && !t.starts_with('/') && !t.starts_with('~') && !t.split('/').any(|part| part == "..");
    match ask.kind {
        AskKind::Read if reach.project != ProjectAccess::None && inside => Some(READ_INSIDE_PROJECT),
        AskKind::Command if reach.commands => Some(COMMAND_THE_STEP_MAY_RUN),
        _ => None,
    }
}

fn note(item: &Item, v: &Verdict) -> String {
    match v {
        Verdict::Change { text } => format!("Change “{}”: {text}", item.label),
        Verdict::Redo { note } if note.trim().is_empty() => format!("Do “{}” again.", item.label),
        Verdict::Redo { note } => format!("Do “{}” again: {note}", item.label),
        Verdict::Drop => format!("Drop “{}” for good: it may not come back.", item.label),
        Verdict::Keep => format!("Keep “{}” as it is.", item.label),
        Verdict::Skip => String::new(),
    }
}

pub(crate) fn notes(q: &Question, answer: &Answer) -> Vec<String> {
    q.items
        .iter()
        .map(|item| (item, answer.verdict(&item.id)))
        .filter(|(_, v)| *v != Verdict::Skip)
        .map(|(item, v)| note(item, &v))
        .collect()
}

/// What an answer will do, in Jaynes's words, from the run as it is now. The page shows it before the person
/// sends, and the record keeps it with the answer.
pub fn effects(run: &Run, q: &Question, answer: &Answer) -> Effects {
    let mut sentences = Vec::new();
    let Some(i) = run.manifest.index(&q.step) else { return Effects::default() };
    if q.kind == QuestionKind::Permission {
        let about = &q.about;
        sentences.push(match answer.verdict("leave") {
            Verdict::Keep => format!("The agent at {} may {about}.", q.step),
            _ => format!("The agent at {} is refused leave to {about}, and goes on without it.", q.step),
        });
        return Effects { sentences, reruns: Vec::new(), may_rerun: Vec::new() };
    }
    let person = run.manifest.steps[i].executor == Executor::Person;
    let input = run.steps[i].inputs.contains(&q.id);
    let mut reruns: Vec<usize> = Vec::new();
    for item in &q.items {
        let v = answer.verdict(&item.id);
        let label = &item.label;
        sentences.push(match (&v, person) {
            (Verdict::Keep, false) if answer.by.person() => format!("“{label}” is kept: no later round of {} may change it.", q.step),
            (Verdict::Keep, false) => format!("“{label}” goes on as it was made."),
            (Verdict::Keep, true) => format!("“{label}” is accepted."),
            (Verdict::Skip, _) => format!("“{label}” stays as it was made, and is not judged."),
            (Verdict::Drop, _) => format!("“{label}” is dropped: it does not come back in this run."),
            (Verdict::Change { text }, _) => format!("“{label}” is changed as you wrote: “{text}”."),
            (Verdict::Redo { .. }, _) => format!("“{label}” is done again, with your note."),
        });
        if v.sends_back() {
            let target = if person { run.manifest.index(&item.id) } else { Some(i) };
            if let Some(t) = target
                && !reruns.contains(&t) {
                    reruns.push(t);
                }
        }
    }
    if input {
        sentences.push(format!("{} goes on with your answer.", q.step));
        return Effects { sentences, reruns: Vec::new(), may_rerun: Vec::new() };
    }
    let context = answer.context.trim();
    if !context.is_empty() && !input {
        let back: Vec<&str> = reruns.iter().map(|&k| run.manifest.steps[k].name.as_str()).collect();
        sentences.push(if back.is_empty() {
            format!("Your context goes to every step after {}: “{context}”. It sends nothing back.", q.step)
        } else {
            format!("Your context goes with it to {} and to every step after it: “{context}”.", back.join(", "))
        });
    }
    if let By::Agent { who } = &answer.by {
        sentences.push(format!(
            "{who} lets the result go on as it is. It counts as the agent's, not yours, and nothing that cannot be undone rests on it. You can still change it while the run is open, or correct the delivery later."
        ));
    }
    let mut may: Vec<usize> = Vec::new();
    for &r in &reruns {
        for d in run.dependents(r) {
            if !reruns.contains(&d) && !may.contains(&d) && run.steps[d].done.is_some() {
                may.push(d);
            }
        }
    }
    if person && !reruns.is_empty() && !reruns.contains(&i) {
        reruns.push(i);
        may.retain(|&d| d != i);
    }
    reruns.sort_unstable();
    may.sort_unstable();
    let names = |v: &[usize]| v.iter().map(|&k| run.manifest.steps[k].name.clone()).collect::<Vec<_>>();
    let (reruns, may_rerun) = (names(&reruns), names(&may));
    if reruns.is_empty() {
        sentences.push("Nothing runs again.".to_string());
        if run.open_question(i).is_some_and(|open| open.question.holds) || person {
            sentences.push("The run goes on from here.".to_string());
        }
    } else {
        sentences.push(format!("Runs again, on top of what it made: {}.", reruns.join(", ")));
        if may_rerun.is_empty() {
            sentences.push("No other step used it.".to_string());
        } else {
            sentences.push(format!("Run again only if what they used comes out different: {}.", may_rerun.join(", ")));
        }
    }
    Effects { sentences, reruns, may_rerun }
}
