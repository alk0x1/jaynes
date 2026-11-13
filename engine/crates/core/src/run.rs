//! A run's state, and how each event changes it. Nothing here decides anything: `apply` only folds.

use crate::digest::Digest;
use crate::event::{Cost, Effects, Event, Fault, Outcome, Read, Usage};
use crate::manifest::{Executor, Manifest, ProjectAccess};
use crate::question::{Answer, Item, Question, QuestionKind, Verdict};
use crate::risk::{Ceiling, Ceilings, Changed, Risk};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Run {
    pub id: String,
    pub request: String,
    pub manifest: Manifest,
    pub role_fingerprint: Digest,
    pub engine: String,
    pub protocol: u32,
    pub format: u32,
    pub risk: Risk,
    pub ceilings: Ceilings,
    pub ceiling: Ceiling,
    pub spent: Usage,
    pub steps: Vec<StepState>,
    pub questions: BTreeMap<String, QuestionState>,
    pub phase: Phase,
    pub halted: bool,
    pub delivering: Option<Digest>,
    pub delivered: Option<Digest>,
    pub counts: Counts,
    /// The person's context, by the step whose question it was answered at, oldest first.
    pub context: Vec<(String, String)>,
    /// Each stop's count as it stood at the start.
    pub attention: std::collections::BTreeMap<String, crate::attention::Count>,
    /// Events folded into this state.
    pub seq: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Counts {
    pub attempts: u32,
    pub spent: u32,
    pub fixes: u32,
    pub questions: u32,
    pub sent_back: u32,
    pub deliveries: u32,
    pub corrections: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    Running,
    Waiting { step: Option<String>, on: Vec<String>, why: String },
    Blocked { step: Option<String>, fault: Fault, reason: String },
    Finished { outcome: Outcome },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StepState {
    pub round: u32,
    /// The last attempt started, so a report for an older one is told apart.
    pub attempt: u32,
    /// Attempts the agent spent by its own fault in this round.
    pub spent: u32,
    pub status: Status,
    pub probed: bool,
    /// The last output that passed its checks. Kept while the step runs again, which works on top of it.
    pub done: Option<Done>,
    /// What an attempt delivered, while its checks run.
    pub pending: Option<Pending>,
    pub session: Option<String>,
    /// The person's words for this round.
    pub notes: Vec<String>,
    /// Why the last attempt or fix was rejected.
    pub failure: Vec<String>,
    /// Items the person kept, by id, with the question that kept them.
    pub locks: BTreeMap<String, (Item, String)>,
    pub dropped: Vec<Item>,
    /// The question of this round's output, once asked.
    pub question: Option<String>,
    /// What a role step asked in this round, in order.
    pub inputs: Vec<String>,
    /// The round a certified stop skipped asking.
    pub skipped: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Status {
    #[default]
    Idle,
    Probing,
    Running { attempt: u32, fix: u32 },
    Checking { attempt: u32, fix: u32 },
    AwaitingInput { question: String },
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Done {
    pub round: u32,
    pub output: Digest,
    pub items: Vec<Item>,
    pub reads: Vec<Read>,
    pub changed: Vec<Changed>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Pending {
    pub output: Digest,
    pub items: Vec<Item>,
    pub changed: Vec<Changed>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QuestionState {
    pub question: Question,
    pub answer: Option<Answer>,
    pub effects: Option<Effects>,
    pub superseded: bool,
}

impl QuestionState {
    pub fn open(&self) -> bool {
        self.answer.is_none() && !self.superseded
    }
}

impl Run {
    /// The state a record gives, or none for a record that does not start with `RunStarted`.
    pub fn fold<'a>(events: impl IntoIterator<Item = &'a Event>) -> Option<Run> {
        let mut run: Option<Run> = None;
        for e in events {
            run = match run {
                None => Run::started(e),
                Some(mut r) => {
                    r.apply(e);
                    Some(r)
                }
            };
        }
        run
    }

    pub fn started(e: &Event) -> Option<Run> {
        let Event::RunStarted { run, request, manifest, role_fingerprint, engine, protocol, format, risk, ceilings, ceiling, attention, .. } = e else {
            return None;
        };
        Some(Run {
            id: run.clone(),
            request: request.clone(),
            steps: vec![StepState::default(); manifest.steps.len()],
            manifest: manifest.clone(),
            role_fingerprint: role_fingerprint.clone(),
            engine: engine.clone(),
            protocol: *protocol,
            format: *format,
            risk: *risk,
            ceilings: *ceilings,
            ceiling: *ceiling,
            spent: Usage::default(),
            questions: BTreeMap::new(),
            phase: Phase::Running,
            halted: false,
            delivering: None,
            delivered: None,
            counts: Counts::default(),
            attention: attention.clone(),
            context: Vec::new(),
            seq: 1,
        })
    }

    fn step_mut(&mut self, name: &str) -> Option<&mut StepState> {
        let i = self.manifest.index(name)?;
        self.steps.get_mut(i)
    }

    fn unwait(&mut self) {
        if matches!(self.phase, Phase::Waiting { .. }) {
            self.phase = Phase::Running;
        }
    }

    pub fn apply(&mut self, e: &Event) {
        self.seq += 1;
        match e {
            Event::RunStarted { .. } => {}
            Event::RiskRaised { to, ceiling, .. } => {
                self.risk = *to;
                self.ceiling = *ceiling;
            }
            Event::CeilingRaised { ceiling, .. } => {
                self.ceiling = *ceiling;
                if matches!(self.phase, Phase::Blocked { fault: Fault::Ceiling, .. }) {
                    self.phase = Phase::Running;
                }
            }
            Event::ProbeAsked { step } => {
                self.unwait();
                if let Some(s) = self.step_mut(step) {
                    s.status = Status::Probing;
                }
            }
            Event::ProbePassed { step } => {
                if let Some(s) = self.step_mut(step) {
                    s.probed = true;
                    s.status = Status::Idle;
                }
            }
            Event::AttemptStarted { step, attempt, fix, session, .. } => {
                self.unwait();
                if *fix == 0 {
                    self.counts.attempts += 1;
                }
                if let Some(s) = self.step_mut(step) {
                    s.attempt = *attempt;
                    s.status = Status::Running { attempt: *attempt, fix: *fix };
                    if session.is_some() {
                        s.session = session.clone();
                    }
                }
            }
            Event::AttemptDelivered { step, attempt, fix, output, items, changed, usage, session } => {
                self.spent.add(*usage);
                if let Some(s) = self.step_mut(step) {
                    s.pending = Some(Pending { output: output.clone(), items: items.clone(), changed: changed.clone() });
                    s.status = Status::Checking { attempt: *attempt, fix: *fix };
                    if session.is_some() {
                        s.session = session.clone();
                    }
                }
            }
            Event::AttemptFailed { step, reasons, usage, spent, .. } => {
                self.spent.add(*usage);
                if *spent {
                    self.counts.spent += 1;
                }
                if let Some(s) = self.step_mut(step) {
                    if *spent {
                        s.spent += 1;
                        // Only the agent's own failure is told to its next attempt: a fault of Jaynes may name
                        // what the agent must not see, such as a hidden check.
                        s.failure = reasons.clone();
                    }
                    s.pending = None;
                    s.status = Status::Idle;
                }
            }
            Event::AttemptInterrupted { step, .. } => {
                if let Some(s) = self.step_mut(step) {
                    s.pending = None;
                    s.status = Status::Idle;
                }
            }
            Event::StaleReport { .. } => {}
            Event::FixAsked { step, attempt, fix, reasons } => {
                self.counts.fixes += 1;
                if let Some(s) = self.step_mut(step) {
                    s.failure = reasons.clone();
                    s.pending = None;
                    s.status = Status::Running { attempt: *attempt, fix: *fix };
                }
            }
            Event::InputNeeded { step, question, .. } => {
                self.counts.questions += 1;
                let id = question.id.clone();
                self.questions.insert(
                    id.clone(),
                    QuestionState { question: question.clone(), answer: None, effects: None, superseded: false },
                );
                if let Some(s) = self.step_mut(step) {
                    s.inputs.push(id.clone());
                    s.status = Status::AwaitingInput { question: id };
                }
            }
            Event::StepDone { step, round, output, items, reads, changed } => {
                self.unwait();
                if let Some(s) = self.step_mut(step) {
                    s.done = Some(Done {
                        round: *round,
                        output: output.clone(),
                        items: items.clone(),
                        reads: reads.clone(),
                        changed: changed.clone(),
                    });
                    s.status = Status::Done;
                    s.pending = None;
                    s.failure.clear();
                }
            }
            Event::StepInvalidated { step, round, notes, .. } => {
                if let Some(s) = self.step_mut(step) {
                    s.round = *round;
                    s.status = Status::Idle;
                    s.spent = 0;
                    s.notes = notes.clone();
                    s.failure.clear();
                    s.pending = None;
                    s.question = None;
                    s.inputs.clear();
                }
            }
            Event::QuestionOpened { question } => {
                self.counts.questions += 1;
                let id = question.id.clone();
                self.questions.insert(
                    id.clone(),
                    QuestionState { question: question.clone(), answer: None, effects: None, superseded: false },
                );
                if question.kind == QuestionKind::Decision {
                    let step = question.step.clone();
                    if let Some(s) = self.step_mut(&step) {
                        s.question = Some(id);
                    }
                }
            }
            Event::QuestionSuperseded { question } => {
                if let Some(q) = self.questions.get_mut(question) {
                    q.superseded = true;
                }
            }
            Event::AnswerRecorded { answer, effects } => {
                self.unwait();
                let Some(q) = self.questions.get_mut(&answer.question) else { return };
                let first = q.answer.is_none();
                q.answer = Some(answer.clone());
                q.effects = Some(effects.clone());
                let question = q.question.clone();
                if answer.by.person() && !answer.context.trim().is_empty() {
                    self.context.push((question.step.clone(), answer.context.trim().to_string()));
                }
                // A leave refused is not work sent back: only a decision about an output counts as rework.
                if answer.sends_back() && first && question.kind == QuestionKind::Decision {
                    self.counts.sent_back += 1;
                }
                let Some(i) = self.manifest.index(&question.step) else { return };
                let executor = self.manifest.steps[i].executor;
                let s = &mut self.steps[i];
                if let Status::AwaitingInput { question: waiting } = &s.status {
                    if *waiting == question.id {
                        s.status = Status::Idle;
                    }
                    return;
                }
                if question.kind != QuestionKind::Decision || executor == Executor::Person || !answer.by.person() {
                    // An agent's release locks nothing: only what the person kept is theirs to keep.
                    return;
                }
                s.locks.retain(|_, (_, by)| *by != question.id);
                for item in &question.items {
                    s.locks.remove(&item.id);
                }
                for item in &question.items {
                    match answer.verdict(&item.id) {
                        Verdict::Keep => {
                            s.locks.insert(item.id.clone(), (item.clone(), question.id.clone()));
                        }
                        Verdict::Drop
                            if !s.dropped.contains(item) => {
                                s.dropped.push(item.clone());
                            }
                        _ => {}
                    }
                }
            }
            Event::Waiting { step, on, why } => {
                self.phase = Phase::Waiting { step: step.clone(), on: on.clone(), why: why.clone() };
            }
            Event::Blocked { step, fault, reason } => {
                if let Some(name) = step
                    && let Some(s) = self.step_mut(name)
                        && matches!(s.status, Status::Probing) {
                            s.status = Status::Idle;
                        }
                self.phase = Phase::Blocked { step: step.clone(), fault: *fault, reason: reason.clone() };
            }
            Event::Continued { .. } => {
                if let Phase::Blocked { step: Some(name), fault, .. } = self.phase.clone()
                    && let Some(s) = self.step_mut(&name) {
                        if fault == Fault::Agent {
                            s.spent = 0;
                        }
                        if matches!(fault, Fault::Jaynes) {
                            s.probed = false;
                        }
                        if matches!(fault, Fault::MayHaveActed | Fault::Role) {
                            s.status = Status::Idle;
                        }
                    }
                self.delivering = None;
                self.phase = Phase::Running;
            }
            Event::Halted { .. } => self.halted = true,
            Event::Resumed { .. } => self.halted = false,
            Event::DeliveryAsked { digest } => {
                self.unwait();
                self.delivering = Some(digest.clone());
            }
            Event::Delivered { digest, .. } => {
                self.delivered = Some(digest.clone());
                self.delivering = None;
                self.counts.deliveries += 1;
            }
            Event::RunFinished { outcome, .. } => self.phase = Phase::Finished { outcome: *outcome },
            Event::DeliveryCorrected { .. } => self.counts.corrections += 1,
            Event::StopSkipped { step, round, .. } => {
                if let Some(s) = self.step_mut(step) {
                    s.skipped = Some(*round);
                }
            }
        }
    }

    // --- reading the state ------------------------------------------------------------------------------------

    pub fn finished(&self) -> bool {
        matches!(self.phase, Phase::Finished { .. })
    }

    pub fn blocked(&self) -> bool {
        matches!(self.phase, Phase::Blocked { .. })
    }

    /// The steps the run takes at its risk, in order.
    pub fn path(&self) -> Vec<usize> {
        self.manifest.path(self.risk)
    }

    pub fn output(&self, i: usize) -> Option<&Digest> {
        self.steps[i].done.as_ref().map(|d| &d.output)
    }

    /// The steps step `i` uses: those it needs, and, for a step on the project, every earlier step that changes
    /// the project, since their changes are in its copy.
    pub fn sources(&self, i: usize) -> Vec<usize> {
        let decl = &self.manifest.steps[i];
        let mut out: Vec<usize> = decl.needs.iter().filter_map(|n| self.manifest.index(n)).collect();
        for c in &decl.checks {
            for r in &c.reads {
                if let Some(j) = self.manifest.index(r)
                    && !out.contains(&j) {
                        out.push(j);
                    }
            }
        }
        if decl.reach.project != ProjectAccess::None {
            for j in self.path() {
                if j < i && self.manifest.steps[j].reach.project == ProjectAccess::Write && !out.contains(&j) {
                    out.push(j);
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// What step `i` would read now.
    pub fn reads(&self, i: usize) -> Vec<Read> {
        self.sources(i)
            .into_iter()
            .filter_map(|j| self.output(j).map(|o| Read { step: self.manifest.steps[j].name.clone(), output: o.clone() }))
            .collect()
    }

    /// A done step that used exactly what it would read now: the same steps, with the same outputs. A step that
    /// came into the run's path before it (the risk went up) makes it stale too, since its change was not there.
    pub fn fresh(&self, i: usize) -> bool {
        let s = &self.steps[i];
        s.status == Status::Done && s.done.as_ref().is_some_and(|d| d.reads == self.reads(i))
    }

    /// The open question of step `i`'s current output, if it has one.
    pub fn open_question(&self, i: usize) -> Option<&QuestionState> {
        let id = self.steps[i].question.as_ref()?;
        self.questions.get(id).filter(|q| q.open())
    }

    /// Whether the current output of step `i` waits for the person: its question is open, or only an agent
    /// answered it.
    pub fn awaits_person(&self, i: usize) -> Option<&QuestionState> {
        let id = self.steps[i].question.as_ref()?;
        self.questions.get(id).filter(|q| !q.superseded && q.answer.as_ref().is_none_or(|a| !a.by.person()))
    }

    /// The questions step `i`'s work rests on that the person has not answered: those of the steps it uses,
    /// directly or through them, open or answered only by an agent.
    pub fn unconfirmed(&self, i: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = vec![false; self.steps.len()];
        let mut stack = self.sources(i);
        while let Some(j) = stack.pop() {
            if std::mem::replace(&mut seen[j], true) {
                continue;
            }
            if let Some(q) = self.awaits_person(j) {
                out.push(q.question.id.clone());
            }
            stack.extend(self.sources(j));
        }
        out.sort();
        out.dedup();
        out
    }

    /// Steps in the path that use step `i`, directly or through others, in order.
    pub fn dependents(&self, i: usize) -> Vec<usize> {
        let mut hit = vec![false; self.steps.len()];
        hit[i] = true;
        let mut out = Vec::new();
        for j in self.path() {
            if j > i && self.sources(j).iter().any(|&k| hit[k]) {
                hit[j] = true;
                out.push(j);
            }
        }
        out
    }

    pub fn open_questions(&self) -> Vec<&QuestionState> {
        self.questions.values().filter(|q| q.open()).collect()
    }

    pub fn cost(&self) -> Cost {
        Cost {
            risk: self.risk,
            usage: self.spent,
            attempts: self.counts.attempts,
            spent: self.counts.spent,
            fixes: self.counts.fixes,
            questions: self.counts.questions,
            sent_back: self.counts.sent_back,
        }
    }

    /// The fingerprint of everything the run delivers: every output of its path.
    pub fn delivery(&self) -> (Digest, Vec<Read>) {
        let given: Vec<Read> = self
            .path()
            .into_iter()
            .filter_map(|i| self.output(i).map(|o| Read { step: self.manifest.steps[i].name.clone(), output: o.clone() }))
            .collect();
        (Digest::of_json(&given), given)
    }
}
