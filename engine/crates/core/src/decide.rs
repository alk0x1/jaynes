//! The core's one function: from a run's state and one input, the events to record and what to do next.
//!
//! `decide` touches nothing outside its arguments. The shell records the events in one transaction, then carries
//! out the commands, and reports what happened as the next input. A refusal records nothing.

use crate::digest::Digest;
use crate::event::{Effects, Event, Fault, Outcome, Read};
use crate::io::{AgentTurn, AttemptOutcome, CheckResult, CheckVerdict, Command, Input};
use crate::manifest::{CheckKind, Executor, ProjectAccess, SessionKind};
use crate::question::{Answer, Ask, AskKind, By, Choice, Item, Question, QuestionKind, Verdict, VerdictKind};
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
    if let Input::Start { run: id, request, manifest, role_fingerprint, agent, raise, attention, task, arm } = input {
        if run.is_some() {
            return refuse("the run already started");
        }
        for (what, label) in [("task", task), ("arm", arm)] {
            if label.as_deref().is_some_and(|l| !crate::io::is_label(l)) {
                return refuse(format!("the {what} is a label: 1 to 64 letters, digits and hyphens, the first a letter or a digit"));
            }
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
            task: task.clone(),
            arm: arm.clone(),
        };
        let Some(state) = Run::started(&start) else { return refuse("the run could not start") };
        let mut c = Ctx { run: state, out: Decided { events: vec![start], commands: Vec::new() }, again: None };
        c.advance();
        return Ok(c.out);
    }
    let Some(run) = run else { return refuse("there is no such run") };
    let mut c = Ctx { run: run.clone(), out: Decided::default(), again: None };
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
        Input::SessionStarted { step, attempt, session, confirmed } => {
            let i = c.index(step)?;
            if matches!(c.run.steps[i].status, Status::Running { attempt: a, .. } if a == *attempt) {
                c.emit(Event::SessionStarted { step: step.clone(), attempt: *attempt, session: session.clone(), confirmed: confirmed.clone() });
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
            if !c.run.manifest.lands() || c.run.delivered.is_none() {
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
    /// The person ordered a second act of this step: the one invalidation of a step that already acted.
    again: Option<usize>,
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
            // a new wait on the person's answers tells them, once; when to show it, at a pause of theirs, is the
            // shell's to choose
            // waiting on the person: every question it waits on is theirs to answer, open or let go on only by an
            // agent; and they are told of each question once, so only one not told of yet tells them
            let theirs = !on.is_empty() && on.iter().all(|id| self.run.questions.get(id).is_some_and(|q| !q.superseded && q.answer.as_ref().is_none_or(|a| !a.by.person())));
            let new = on.iter().any(|id| !self.run.told.contains(id));
            self.emit(Event::Waiting { step, on, why: why.clone() });
            if theirs && new {
                self.command(Command::Notify { text: why });
            }
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
                let because: Vec<String> = changed.iter().map(|s| format!("{s} came out different")).collect();
                // G14: an act is never done again on its own; the run holds at it and asks the person
                if self.run.acted(i) {
                    self.hold_act(i, &because);
                    return false;
                }
                self.invalidate(i, because, Vec::new());
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
            choose: stop.choose,
            pass: self.run.sent_back_at(i) + 1,
            passes: 0,
            back: s.dropped.clone(),
        };
        self.emit(Event::QuestionOpened { question: question.clone() });
        self.command(Command::Ask { question });
    }

    /// A stop with two errors among its last ten outcomes asks every run and holds it.
    fn braked(&self, i: usize) -> bool {
        self.run.attention.get(&self.run.manifest.steps[i].name).is_some_and(|c| c.braked)
    }

    fn invalidate(&mut self, i: usize, because: Vec<String>, notes: Vec<String>) {
        self.invalidate_narrowing(i, because, notes, None);
    }

    /// Run the step again; `narrow`, the question whose choice the round keeps alone, without the agent.
    fn invalidate_narrowing(&mut self, i: usize, because: Vec<String>, notes: Vec<String>, narrow: Option<String>) {
        self.invalidate_full(i, because, notes, narrow, None);
    }

    /// Run the step again; `narrow`, the question whose choice the round keeps alone; `from`, the question at a later
    /// step that reviewed this one's work and sent it back here.
    fn invalidate_full(&mut self, i: usize, because: Vec<String>, notes: Vec<String>, narrow: Option<String>, from: Option<String>) {
        self.invalidate_bringing(i, because, notes, narrow, from, Vec::new());
    }

    /// Run the step again, bringing back what the person names of what they dropped.
    fn invalidate_bringing(&mut self, i: usize, because: Vec<String>, notes: Vec<String>, narrow: Option<String>, from: Option<String>, bring: Vec<crate::io::Brought>) {
        // G14: a step that cannot be undone and already acted is done again only on the person's order of a second act
        if self.run.acted(i) && self.again != Some(i) {
            self.hold_act(i, &because);
            return;
        }
        if let Some(q) = self.run.open_question(i) {
            let id = q.question.id.clone();
            self.emit(Event::QuestionSuperseded { question: id });
        }
        let round = self.run.steps[i].round + 1;
        let step = self.name(i);
        self.emit(Event::StepInvalidated { step, round, because, notes, narrow, from, bring });
    }

    fn start(&mut self, i: usize, executor: &Executor, name: String) -> bool {
        let decl = self.run.manifest.steps[i].clone();
        if *executor == Executor::Person {
            return self.ask_person(i);
        }
        // G14: a step that may have acted is started again only when the person says so, knowing it repeats with the
        // same key; never on a bare continue
        if self.run.steps[i].maybe_acted {
            self.hold_act(i, &[format!("its outcome was not recorded, so it may have acted; acting again repeats it with the same key {}", self.act_key(i))]);
            return false;
        }
        // G14: a step that acted in this intent is not started again on its own, whatever its checks said after
        if self.run.acted_unfinished(i) {
            self.hold_act(i, &["its checks did not pass after it acted".to_string()]);
            return false;
        }
        // nothing goes on from an act the person is asked about until they answer
        for j in self.run.sources(i) {
            if let Some(q) = self.run.held(j) {
                let on = vec![q.question.id.clone()];
                let why = format!("{name} waits for your answer about what {} did", self.name(j));
                self.wait(Some(name), on, why);
                return false;
            }
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
        let key = decl.effect.irreversible().then(|| self.act_key(i));
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
            // the person's words reach every step from the stop on, and the maker that stop sends work back to
            context: self
                .run
                .context
                .iter()
                .filter(|(at, _)| {
                    self.run.manifest.index(at).is_some_and(|j| {
                        j <= i || self.run.manifest.steps[j].stop.as_ref().and_then(|st| st.redo.as_deref()) == Some(decl.name.as_str())
                    })
                })
                .map(|(_, text)| text.clone())
                .collect(),
            failure: s.failure.clone(),
            checks: decl.checks.iter().filter(|c| c.kind == CheckKind::Open).cloned().collect(),
            locked: s.locks.values().map(|(item, _)| item.clone()).collect(),
            dropped: s.dropped.clone(),
            review: s.from.as_ref().and_then(|q| self.run.questions.get(q)).and_then(|q| {
                let k = self.run.manifest.index(&q.question.step)?;
                self.run.output(k).map(|o| Read { step: q.question.step.clone(), output: o.clone() })
            }),
            earlier: s.earlier.clone(),
            back: s.bring.clone(),
            keep: s
                .narrow
                .as_ref()
                .and_then(|q| self.run.questions.get(q))
                .and_then(|q| q.answer.as_ref().and_then(|a| a.kept_alone(&q.question))),
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
        let question = Question { back: Default::default(),
            id: self.question_id(&name),
            step: name.clone(),
            round: self.run.steps[i].round,
            kind: QuestionKind::Decision,
            items,
            offered: stop.verdicts,
            holds: true,
            about: stop.about,
            choose: false,
            pass: 0,
            passes: 0,
        };
        let on = vec![question.id.clone()];
        self.emit(Event::QuestionOpened { question: question.clone() });
        self.command(Command::Ask { question });
        self.wait(Some(name.clone()), on, format!("{name} is yours to answer"));
        false
    }

    /// Whether the delivery `digest` merges into the person's branch: none while G19 waits for the person.
    fn lands(&self, digest: &Digest) -> Option<bool> {
        if !(self.run.manifest.lands() && !self.run.undone) {
            return Some(false);
        }
        if self.run.merge_holds().is_empty() {
            return Some(true);
        }
        self.run.merge_answer(digest)
    }

    /// G19: the merge would bring in what runs or is obeyed later outside any box, and the person has not answered
    /// about this delivery: ask them, naming each such file with what changes in it, and wait. A question about an
    /// earlier delivery is replaced, since what it showed is not what would land.
    fn ask_merge(&mut self, digest: &Digest) {
        const WHY: &str = "the merge waits for you: it brings into your branch what runs later outside any box";
        if let Some(q) = self.run.merge_open(digest) {
            let on = vec![q.question.id.clone()];
            self.wait(None, on, WHY.to_string());
            return;
        }
        let stale: Vec<String> = self.run.open_questions().iter().filter(|q| q.question.kind == QuestionKind::Merge).map(|q| q.question.id.clone()).collect();
        for question in stale {
            self.emit(Event::QuestionSuperseded { question });
        }
        let held = self.run.merge_holds();
        let step = held
            .iter()
            .filter_map(|(_, s)| self.run.manifest.index(s))
            .max()
            .map(|i| self.name(i))
            .unwrap_or_default();
        let round = self.run.manifest.index(&step).map(|i| self.run.steps[i].round).unwrap_or(0);
        let items = held
            .iter()
            .map(|(c, _)| Item {
                id: c.what.clone(),
                label: if c.about.is_empty() { c.what.clone() } else { format!("{}: {}", c.what, c.about) },
                digest: c.digest.clone().map(Digest).unwrap_or_else(|| Digest::of_json(c)),
            })
            .collect();
        let question = Question { back: Default::default(),
            id: self.question_id("merge"),
            step,
            round,
            kind: QuestionKind::Merge,
            items,
            offered: vec![VerdictKind::Keep, VerdictKind::Drop],
            holds: true,
            about: format!(
                "This delivery brings into your branch files that run or are obeyed later outside any box. Keep merges them; drop delivers the change on the branch jaynes/{} only, and nothing lands.",
                self.run.id
            ),
            choose: false,
            pass: 0,
            passes: 0,
        };
        let on = vec![question.id.clone()];
        self.emit(Event::QuestionOpened { question: question.clone() });
        self.command(Command::Ask { question });
        self.wait(None, on, WHY.to_string());
    }

    fn deliver(&mut self) -> bool {
        let (digest, given) = self.run.delivery();
        if self.run.delivered.as_ref() != Some(&digest) {
            let Some(merge) = self.lands(&digest) else {
                self.ask_merge(&digest);
                return false;
            };
            self.emit(Event::DeliveryAsked { digest: digest.clone() });
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
            AttemptOutcome::Delivered { output, items, changed, usage, session, tree } => {
                self.emit(Event::AttemptDelivered {
                    step: step.to_string(),
                    attempt,
                    fix,
                    output: output.clone(),
                    items: items.clone(),
                    changed: changed.clone(),
                    usage: *usage,
                    session: session.clone(),
                    tree: tree.clone(),
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
                // G4: the fault is Jaynes's either way; an act may have happened, and the person is told to look first
                if self.may_have_acted(i) {
                    self.emit(Event::MayHaveActed { step: step.to_string(), attempt });
                    let reason = format!("a fault of Jaynes or of the machine stopped {step}: {reason}. No attempt was spent. {step} acts on something real and its outcome is not known, so it may have acted: continuing asks you whether to act again");
                    self.block(Some(step.to_string()), Fault::Jaynes, reason);
                } else {
                    let reason = format!("a fault of Jaynes or of the machine stopped {step}: {reason}. No attempt was spent");
                    self.block(Some(step.to_string()), Fault::Jaynes, reason);
                }
            }
            AttemptOutcome::Interrupted { reason } => {
                self.emit(Event::AttemptInterrupted { step: step.to_string(), attempt, reason: reason.clone() });
                if self.may_have_acted(i) {
                    self.emit(Event::MayHaveActed { step: step.to_string(), attempt });
                    self.block(Some(step.to_string()), Fault::MayHaveActed, format!("{step}, which acts on something real, was stopped from outside before its outcome was known: it may have acted ({reason}). Look, then continue or abandon the run"));
                } else {
                    self.block(Some(step.to_string()), Fault::Paused, format!("{step} was stopped from outside: {reason}"));
                }
            }
            AttemptOutcome::NeedsInput { items, offered, about } => {
                if !role {
                    return refuse("only a step of the role asks the person through its result");
                }
                if offered.is_empty() || items.is_empty() {
                    return refuse("a question needs items and verdicts");
                }
                let question = Question { back: Default::default(),
                    id: self.question_id(step),
                    step: step.to_string(),
                    round: self.run.steps[i].round,
                    kind: QuestionKind::Decision,
                    items: items.clone(),
                    offered: offered.clone(),
                    holds: true,
                    about: about.clone(),
                    choose: false,
                    pass: 0,
                    passes: 0,
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
                let question = Question { back: Default::default(),
                    id: self.question_id(step),
                    step: step.to_string(),
                    round: self.run.steps[i].round,
                    kind: QuestionKind::Decision,
                    items: vec![Item { id: "impossible".to_string(), label: reason.clone(), digest: Digest::of_json(reason) }],
                    offered: vec![VerdictKind::Change, VerdictKind::Redo],
                    holds: true,
                    about: format!("{step} says it cannot be done as asked"),
                    choose: false,
                    pass: 0,
                    passes: 0,
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

    /// An attempt of step `i` ended with no outcome recorded: whether it may have acted, so that nothing starts it again
    /// on its own (G14). A step whose act repeats safely is started again, with the same key.
    fn may_have_acted(&self, i: usize) -> bool {
        let d = &self.run.manifest.steps[i];
        d.effect.acts() && !d.repeats_safely()
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
        let (level, reasons) = risk::of_change(self.run.manifest.target, &all);
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
        // a check judged the step's output only if it was given the state of the project the output was measured
        // at: one that judged another, or none where there was one, could not judge it
        let measured = self.run.steps[i].pending.as_ref().and_then(|p| p.tree.clone());
        let judged: Vec<CheckResult>;
        let results = match &measured {
            None => results,
            Some(t) => {
                judged = results
                    .iter()
                    .map(|r| match &r.tree {
                        Some(x) if x == t => r.clone(),
                        other => CheckResult {
                            verdict: CheckVerdict::Blocked {
                                reason: format!(
                                    "it judged {}, not the state of the project the step left ({t})",
                                    other.as_deref().map(|x| format!("another state of the project ({x})")).unwrap_or_else(|| "no state of the project Jaynes measured".to_string())
                                ),
                            },
                            ..r.clone()
                        },
                    })
                    .collect();
                &judged[..]
            }
        };
        let sources: Vec<String> = self.run.sources(i).into_iter().map(|j| self.name(j)).collect();
        let checks = decl
            .checks
            .iter()
            .filter_map(|c| {
                let result = results.iter().find(|r| r.check == c.name)?;
                let verdict = &result.verdict;
                let mut read = sources.clone();
                read.extend(c.reads.iter().filter(|r| !read.contains(r)).cloned().collect::<Vec<_>>());
                Some(crate::event::CheckRecord {
                    check: c.name.clone(),
                    kind: c.kind,
                    passed: *verdict == CheckVerdict::Pass,
                    blocked: matches!(verdict, CheckVerdict::Blocked { .. }),
                    read,
                    tree: result.tree.clone(),
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
        if decl.executor == Executor::Role || self.run.steps[i].narrow.is_some() {
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
                    // G14: an act with no recorded outcome may have happened: never started again on its own, unless
                    // repeating it does no harm, and then with the same key
                    if decl.effect.acts() && !decl.repeats_safely() {
                        self.emit(Event::MayHaveActed { step: name.clone(), attempt });
                        let reason = format!(
                            "{name} acts on something real and was under way when Jaynes stopped, so it may have acted. Look, then continue (which asks you whether to act again) or abandon the run"
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
            // a delivery under way was asked only once G19 let it: the person's answer decides again what it does
            let merge = self.lands(&digest).unwrap_or(false);
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
        let question = Question { back: Default::default(),
            id: self.question_id(step),
            step: step.to_string(),
            round: self.run.steps[i].round,
            kind: QuestionKind::Permission,
            items: vec![Item { id: "leave".to_string(), label: about.clone(), digest: Digest::of_json(ask) }],
            offered: vec![VerdictKind::Keep, VerdictKind::Drop],
            holds: true,
            about,
            choose: false,
            pass: 0,
            passes: 0,
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
            let answer = Answer { back: Default::default(),
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
            let answer = Answer { back: Default::default(),
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
        if q.question.kind == QuestionKind::Acted {
            return self.answer_acted(&q, answer);
        }
        if q.question.kind == QuestionKind::Merge {
            return self.answer_merge(&q, answer);
        }
        if let By::Rule { .. } = &answer.by {
            return refuse("a rule answers only inside the engine, and only routine leaves");
        }
        if q.superseded {
            return refuse("this question was replaced by a newer one, since the step ran again; answer that one");
        }
        if answer.shown != q.question.shown() {
            return refuse("the question changed since it was shown to you; look at it again");
        }
        if !answer.back.is_empty() && !answer.by.person() {
            return refuse("only you bring back what you dropped: that undoing is yours");
        }
        if let Some(id) = answer.back.keys().find(|id| !q.question.back.iter().any(|x| &x.id == *id)) {
            return refuse(format!("the question has no earlier option {id} to bring back"));
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
        if q.question.kind == QuestionKind::Decision && answer.by.person() && answer.sends_back() {
            let redo = self.run.manifest.index(&q.question.step).and_then(|i| self.run.manifest.steps[i].stop.as_ref()).is_some_and(|s| s.redo.is_some());
            if redo && answer.kept_alone(&q.question).is_none() && q.question.items.iter().any(|x| answer.verdict(&x.id) == Verdict::Keep) {
                return refuse("the work goes back to the step that made it: send all of it back, or keep all of it");
            }
        }
        if q.question.choose {
            match answer.choice(&q.question) {
                None => return refuse("choose one: keep or change one option, or name finalists for the agent to develop, or choose none; the others leave"),
                Some(Choice::None) if answer.context.trim().is_empty() => return refuse("none goes on: say what was missing, for the new options"),
                Some(_) => {}
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
                && self.run.steps[i].status == Status::Done
                && self.run.steps[i].done.as_ref().is_some_and(|d| d.round == q.question.round);
            if !revisable {
                return refuse("this answer was already acted on");
            }
        }
        let effects = effects(&self.run, &q.question, answer);
        // what comes back, with the output it was in, read before the answer undoes the drop
        let bring = |k: usize| -> Vec<crate::io::Brought> {
            answer
                .back
                .iter()
                .filter_map(|(id, words)| {
                    let item = q.question.back.iter().find(|x| &x.id == id)?.clone();
                    let output = self.run.steps[k].dropped_from.get(id).or_else(|| self.run.steps[i].dropped_from.get(id)).cloned().unwrap_or_default();
                    Some(crate::io::Brought { item, output, words: words.clone() })
                })
                .collect()
        };
        let brought = (bring(maker(&self.run, i).unwrap_or(i)), bring(i));
        self.emit(Event::AnswerRecorded { answer: answer.clone(), effects });
        match q.question.kind {
            QuestionKind::Permission => {
                if let Status::Running { attempt, .. } = self.run.steps[i].status {
                    let allow = answer.verdict("leave") == Verdict::Keep;
                    self.command(Command::Permit { step: q.question.step.clone(), attempt, allow });
                }
            }
            // answered in answer_acted and answer_merge, never here
            QuestionKind::Acted | QuestionKind::Merge => {}
            QuestionKind::Decision if executor == Executor::Person => self.answered_person(i, &q, answer),
            QuestionKind::Decision => {
                let input = self.run.steps[i].inputs.contains(&q.question.id);
                if answer.sends_back() && !input {
                    let notes = notes(&q.question, answer);
                    let narrow = answer.kept_alone(&q.question).map(|_| q.question.id.clone());
                    let maker = maker(&self.run, i).filter(|_| narrow.is_none());
                    match maker {
                        // the work goes back to the step that made it; this one looks again at what comes back
                        Some(j) => {
                            let because = vec![format!("you sent it back at {}", q.question.step)];
                            self.invalidate_bringing(j, because, notes, None, Some(q.question.id.clone()), brought.0);
                        }
                        None => {
                            let because = if narrow.is_some() { "you chose one, and sent it back to keep only that" } else { "you sent it back" };
                            self.invalidate_bringing(i, vec![because.to_string()], notes, narrow, None, brought.1);
                        }
                    }
                }
            }
        }
        self.advance();
        Ok(())
    }

    /// The key of step `i`'s act, its intent: the run, the step, what it acts on, and how many second acts the person
    /// ordered. The same on every attempt and recovery of that intent; never the round, which other steps' changes move.
    fn act_key(&self, i: usize) -> String {
        Digest::of_json(&(&self.run.id, &self.name(i), self.run.reads(i), self.run.steps[i].repeat)).0
    }

    /// Hold the run at step `i`, which cannot be undone and already acted, and ask the person about the act: keep it as
    /// it is, or act again, a second act they order as such. Asked once while open.
    fn hold_act(&mut self, i: usize, because: &[String]) {
        let name = self.name(i);
        if let Some(q) = self.run.held(i) {
            let on = vec![q.question.id.clone()];
            self.wait(Some(name.clone()), on, format!("{name} cannot be undone and already acted: it waits for you to keep it or act again"));
            return;
        }
        let output = self.run.steps[i].done.as_ref().map(|d| d.output.clone()).or_else(|| self.run.steps[i].pending.as_ref().map(|p| p.output.clone())).unwrap_or_default();
        let why = if because.is_empty() { "it was sent back".to_string() } else { because.join("; ") };
        let about = format!(
            "{name} cannot be undone, and it already acted. Since then {why}. It is not done again on its own: keep the act as it is, or act again, a second act with its own key."
        );
        let question = Question {
            back: Default::default(),
            id: self.question_id(&name),
            step: name.clone(),
            round: self.run.steps[i].round,
            kind: QuestionKind::Acted,
            items: vec![Item { id: name.clone(), label: format!("what {name} did"), digest: output }],
            // an act whose checks did not pass cannot be kept as done: only a second act, or the person stops the run
            offered: if self.run.steps[i].status == Status::Done { vec![VerdictKind::Keep, VerdictKind::Redo] } else { vec![VerdictKind::Redo] },
            holds: true,
            about,
            choose: false,
            pass: 0,
            passes: 0,
        };
        let on = vec![question.id.clone()];
        self.emit(Event::QuestionOpened { question: question.clone() });
        self.command(Command::Ask { question });
        self.wait(Some(name.clone()), on, format!("{name} cannot be undone and already acted: it waits for you to keep it or act again"));
    }

    /// G19: the person's answer about what a merge brings in: keep each file shown (it merges), or drop one (the
    /// change stays on the run's branch only). Theirs alone, and only for the delivery it was asked about.
    fn answer_merge(&mut self, q: &QuestionState, answer: &Answer) -> Result<(), Refusal> {
        if q.superseded {
            return refuse("this question was replaced by a newer one, since what the delivery brings in changed; answer that one");
        }
        if !q.open() {
            return refuse("this merge was already answered, and that answer stands");
        }
        if !answer.by.person() {
            return refuse("only you decide what lands on your branch: no agent and no rule answers this");
        }
        if self.run.merges.get(&q.question.id) != Some(&self.run.delivery().0) {
            return refuse("what the delivery brings in changed since this was asked; answer the new question");
        }
        if answer.shown != q.question.shown() {
            return refuse("the question changed since it was shown to you; look at it again");
        }
        let items: Vec<&str> = q.question.items.iter().map(|x| x.id.as_str()).collect();
        if answer.verdicts.keys().any(|id| !items.contains(&id.as_str())) || !answer.back.is_empty() {
            return refuse(format!("the question's items are the files it shows: {}", items.join(", ")));
        }
        if !items.iter().all(|id| matches!(answer.verdicts.get(*id), Some(Verdict::Keep | Verdict::Drop))) {
            return refuse("keep or drop each file it shows");
        }
        let effects = effects(&self.run, &q.question, answer);
        self.emit(Event::AnswerRecorded { answer: answer.clone(), effects });
        self.advance();
        Ok(())
    }

    /// The person's answer about an act: keep it as it is, on what holds now, or order a second act.
    fn answer_acted(&mut self, q: &QuestionState, answer: &Answer) -> Result<(), Refusal> {
        if !q.open() {
            return refuse("this act was already answered, and that answer stands");
        }
        if !answer.by.person() {
            return refuse("only you decide about an act that cannot be undone");
        }
        if answer.shown != q.question.shown() {
            return refuse("the question changed since it was shown to you; look at it again");
        }
        let step = q.question.step.clone();
        if answer.verdicts.keys().any(|id| *id != step) || !answer.back.is_empty() {
            return refuse(format!("the question has one item, {step}"));
        }
        let v = answer.verdict(&step);
        if !matches!(v, Verdict::Keep | Verdict::Redo { .. }) || !q.question.offered.contains(&v.kind()) {
            return refuse("that is not offered here: keep the act as it is when it is done, or act again");
        }
        let i = self.index(&step)?;
        let unknown = self.run.steps[i].maybe_acted;
        let effects = effects(&self.run, &q.question, answer);
        self.emit(Event::AnswerRecorded { answer: answer.clone(), effects });
        if v == Verdict::Keep {
            // kept as it acted: it stands on what holds now, and what comes after reads what it did
            if !self.run.fresh(i) {
                let reads = self.run.reads(i);
                self.emit(Event::ActKept { step: step.clone(), reads });
            }
        } else if unknown {
            // it may have acted: the person said to act again, the same intent with the same key; the answer, folded,
            // let it start
        } else {
            self.again = Some(i);
            self.invalidate(i, vec![format!("it is being done again: you ordered a second act of {step}")], notes(&q.question, answer));
            self.again = None;
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
                let note = note(item, &v, None);
                self.invalidate(j, vec![format!("you sent it back at {here}")], vec![note]);
            }
        }
        self.invalidate(i, vec!["what it shows is being done again".to_string()], Vec::new());
    }
}

/// What follows a send-back: the reviewing step looks again when the work went back to its maker, and what else may
/// run again.
fn reviewed(sentences: &mut Vec<String>, run: &Run, i: usize, q: &Question, may: &[String]) {
    let back = maker(run, i).is_some();
    if back {
        sentences.push(format!("{} looks at it again before you see it.", q.step));
    }
    let others: Vec<&String> = may.iter().filter(|m| !back || **m != q.step).collect();
    if !others.is_empty() {
        sentences.push(format!("Run again only if what they used comes out different: {}.", others.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
}

/// The step a stop sends its work back to, when the step that asks only reviews it.
fn maker(run: &Run, i: usize) -> Option<usize> {
    run.manifest.steps[i].stop.as_ref().and_then(|s| s.redo.as_ref()).and_then(|r| run.manifest.index(r))
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

fn note(item: &Item, v: &Verdict, choose: Option<Choice>) -> String {
    let choose_one = choose.is_some();
    // the person's words end the sentence they are put in, so their own last period is not doubled
    let v = &match v {
        Verdict::Change { text } => Verdict::Change { text: text.trim().trim_end_matches('.').to_string() },
        other => other.clone(),
    };
    match v {
        Verdict::Change { text } if choose == Some(Choice::Finalists) => format!(
            "“{}” is a finalist: develop it further as the person writes, keeping its name: {text}",
            item.label
        ),
        Verdict::Change { text } if choose_one => format!(
            "“{}” was chosen. Give it alone, changed as the person writes: {text}. You may take from the other options, in your previous output, what the person names.",
            item.label
        ),
        Verdict::Drop if choose_one => format!("“{}” was not chosen: do not give it again as an option.", item.label),
        Verdict::Keep if choose_one => format!("“{}” was chosen: keep it as it is.", item.label),
        Verdict::Change { text } => format!("Change “{}”: {text}", item.label),
        Verdict::Redo { note } if note.trim().is_empty() => format!("Do “{}” again.", item.label),
        Verdict::Redo { note } => format!("Do “{}” again: {note}", item.label),
        Verdict::Drop => format!("Drop “{}” for good: it may not come back.", item.label),
        Verdict::Keep => format!("Keep “{}” as it is.", item.label),
        Verdict::Skip => String::new(),
    }
}

pub(crate) fn notes(q: &Question, answer: &Answer) -> Vec<String> {
    let choice = if q.choose { answer.choice(q) } else { None };
    let mut out: Vec<String> = q
        .items
        .iter()
        .map(|item| (item, answer.verdict(&item.id)))
        .filter(|(_, v)| *v != Verdict::Skip)
        .map(|(item, v)| note(item, &v, choice))
        .collect();
    for (id, words) in &answer.back {
        let label = q.back.iter().find(|x| &x.id == id).map_or(id.as_str(), |x| x.label.as_str());
        out.push(if words.trim().is_empty() {
            format!("“{label}”, which the person dropped in an earlier round, comes back as it was: give it again, from the output it was in.")
        } else {
            format!("“{label}”, which the person dropped in an earlier round, comes back, changed as the person writes: {}. Start from the output it was in.", words.trim())
        });
    }
    if choice == Some(Choice::None) {
        out.push(
            "None was chosen: make new options from what the person said, which comes first: these may not come back, but what the person said they liked may stay."
                .to_string(),
        );
    }
    out
}

/// What an answer will do, in Jaynes's words, from the run as it is now. The page shows it before the person
/// sends, and the record keeps it with the answer.
pub fn effects(run: &Run, q: &Question, answer: &Answer) -> Effects {
    let mut sentences = Vec::new();
    if q.kind == QuestionKind::Merge {
        let files: Vec<&str> = q.items.iter().map(|x| x.id.as_str()).collect();
        sentences.push(if q.items.iter().all(|x| answer.verdict(&x.id) == Verdict::Keep) {
            format!("The delivery merges into your branch, with {}.", files.join(", "))
        } else {
            format!("Nothing lands on your branch: the change stays on the branch jaynes/{}, where you can look at it.", run.id)
        });
        return Effects { sentences, reruns: Vec::new(), may_rerun: Vec::new() };
    }
    let Some(i) = run.manifest.index(&q.step) else { return Effects::default() };
    if q.kind == QuestionKind::Acted {
        let again = answer.verdict(&q.step).sends_back();
        if again && run.manifest.index(&q.step).is_some_and(|k| run.steps[k].maybe_acted) {
            sentences.push(format!("{} acts again, the same intent with the same key: if its first attempt acted, it is done twice, unless the far side knows the key.", q.step));
            return Effects { sentences, reruns: Vec::new(), may_rerun: Vec::new() };
        }
        sentences.push(if again {
            format!("{} acts again: a second act, with its own key, which you order now as such.", q.step)
        } else {
            format!("{} stays as it acted, and the steps after it read what it did. Nothing runs again.", q.step)
        });
        let reruns = if again { vec![q.step.clone()] } else { Vec::new() };
        return Effects { sentences, reruns, may_rerun: Vec::new() };
    }
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
            (Verdict::Keep, _) if q.choose => format!("“{label}” is chosen: no later round of {} may change it.", q.step),
            (Verdict::Change { text }, _) if q.choose && answer.choice(q) == Some(Choice::Finalists) => {
                format!("“{label}” is a finalist: the agent develops it as you wrote: “{text}”.")
            }
            (Verdict::Change { text }, _) if q.choose => format!("“{label}” is chosen, and is adjusted as you wrote: “{text}”."),
            (Verdict::Drop, _) if q.choose => format!("“{label}” leaves the running: it does not come back as an option unless you bring it back."),
            (Verdict::Keep, false) if answer.by.person() => format!("“{label}” is kept: no later round of {} may change it.", q.step),
            (Verdict::Keep, false) => format!("“{label}” goes on as it was made."),
            (Verdict::Keep, true) => format!("“{label}” is accepted."),
            (Verdict::Skip, _) => format!("“{label}” stays as it was made, and is not judged."),
            (Verdict::Drop, _) => format!("“{label}” is dropped: it does not come back in this run unless you bring it back."),
            (Verdict::Change { text }, _) => format!("“{label}” is changed as you wrote: “{text}”."),
            (Verdict::Redo { .. }, _) => format!("“{label}” is done again, with your note."),
        });
        if v.sends_back() {
            let target = if person {
                run.manifest.index(&item.id)
            } else if answer.kept_alone(q).is_none() {
                Some(maker(run, i).unwrap_or(i))
            } else {
                Some(i)
            };
            if let Some(t) = target
                && !reruns.contains(&t) {
                    reruns.push(t);
                }
        }
    }
    for (id, words) in &answer.back {
        let label = q.back.iter().find(|x| &x.id == id).map_or(id.as_str(), |x| x.label.as_str());
        let to = maker(run, i).unwrap_or(i);
        sentences.push(if words.trim().is_empty() {
            format!("“{label}”, which you dropped in an earlier round, comes back as it was: {} makes it again.", run.manifest.steps[to].name)
        } else {
            format!("“{label}”, which you dropped in an earlier round, comes back, changed as you wrote: “{}”.", words.trim())
        });
        if !reruns.contains(&to) {
            reruns.push(to);
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
    // G14: an act already done is never done again on its own; the run holds at it and asks
    let held: Vec<usize> = reruns.iter().copied().filter(|&k| run.acted(k)).collect();
    reruns.retain(|k| !held.contains(k));
    for &k in &held {
        sentences.push(format!(
            "{} cannot be undone and already acted: it is not done again on its own. The run holds at it and asks you whether it acts again.",
            run.manifest.steps[k].name
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
    if person && (!reruns.is_empty() || !held.is_empty()) && !reruns.contains(&i) {
        reruns.push(i);
        may.retain(|&d| d != i);
    }
    may.retain(|&k| !run.acted(k));
    reruns.sort_unstable();
    may.sort_unstable();
    let names = |v: &[usize]| v.iter().map(|&k| run.manifest.steps[k].name.clone()).collect::<Vec<_>>();
    let (reruns, may_rerun) = (names(&reruns), names(&may));
    if reruns.is_empty() {
        sentences.push("Nothing runs again.".to_string());
        if run.open_question(i).is_some_and(|open| open.question.holds) || person {
            sentences.push("The run goes on from here.".to_string());
        }
    } else if answer.kept_alone(q).is_some() {
        sentences.push(format!("{} keeps only what you chose, without the agent, and is not asked again.", q.step));
        if !may_rerun.is_empty() {
            sentences.push(format!("Run again only if what they used comes out different: {}.", may_rerun.join(", ")));
        }
    } else if q.choose {
        let by = maker(run, i).map(|j| run.manifest.steps[j].name.clone()).unwrap_or_else(|| q.step.clone());
        sentences.push(match answer.choice(q) {
            Some(Choice::Finalists) => format!("The agent at {by} develops the finalists, and you choose again."),
            Some(Choice::None) => format!("The agent at {by} makes new options from your words; these do not come back."),
            _ => format!("The agent at {by} adjusts what you chose, seeing every option it made, and you see the result."),
        });
        reviewed(&mut sentences, run, i, q, &may_rerun);
    } else if maker(run, i).is_some() && !reruns.contains(&q.step) {
        sentences.push(format!("Goes back to {}, which made it, on top of what it made.", reruns.join(", ")));
        reviewed(&mut sentences, run, i, q, &may_rerun);
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
