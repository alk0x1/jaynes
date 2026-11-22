//! A run's state, and how each event changes it. Nothing here decides anything: `apply` only folds.

use crate::digest::Digest;
use crate::event::{Cost, Effects, Event, Fault, Outcome, Read, Usage};
use crate::manifest::{Executor, Manifest, ProjectAccess};
use crate::question::{Answer, By, Item, Question, QuestionKind, Verdict};
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
    /// The agent program and model, fixed for the run.
    #[serde(default)]
    pub agent: String,
    pub protocol: u32,
    pub format: u32,
    pub risk: Risk,
    /// Why the risk is what it is, as the rules or the answer that last set it said: empty while nothing raised it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risk_why: Vec<String>,
    pub ceilings: Ceilings,
    pub ceiling: Ceiling,
    pub spent: Usage,
    pub steps: Vec<StepState>,
    pub questions: BTreeMap<String, QuestionState>,
    pub phase: Phase,
    pub halted: bool,
    /// The agent's process that started the run, by pid and start time: it may continue a block of Jaynes's that
    /// judges the same change again, and set the run aside. None: only the person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starter: Option<String>,
    /// Set aside by the agent that started it, which abandoned it: halted with its work kept, until the person
    /// brings it back or abandons it for good.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_aside: Option<By>,
    pub delivering: Option<Digest>,
    pub delivered: Option<Digest>,
    pub counts: Counts,
    /// The person reverted what the run merged.
    pub undone: bool,
    /// The person's context, by the step whose question it was answered at, oldest first.
    pub context: Vec<(String, String)>,
    /// Each stop's count as it stood at the start.
    pub attention: std::collections::BTreeMap<String, crate::attention::Count>,
    /// The questions the person was told of, by a wait on them: each is told of once.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub told: std::collections::BTreeSet<String>,
    /// Events folded into this state.
    pub seq: u64,
    /// G19: each question that held a merge, with the delivery it was asked about: an answer counts only for it.
    #[serde(default)]
    pub merges: BTreeMap<String, Digest>,
    /// The run never merges: it was started so, and nothing answered in it lifts that.
    #[serde(default)]
    pub never_merge: bool,
    /// The defects an evaluation planted and has not taken out, each by its id with its step: nothing is delivered
    /// while one is in.
    #[serde(default)]
    pub planted: BTreeMap<String, String>,
    /// The defects planted and taken out since, by id.
    #[serde(default)]
    pub unplanted: std::collections::BTreeSet<String>,
    /// What the person chose for the project's ports since the run's setup, the last choice: none, as the setup says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<Ports>,
}

/// The project's ports as the person chose them: every local port (`any`), test servers on ports the system picks
/// (`servers`), opened at `opened` (seconds since 1970), or only the ones a run reserves and a step declares.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Ports {
    pub any: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub servers: bool,
    #[serde(default)]
    pub opened: u64,
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
    /// The step's output each dropped item was in, by id, so the person can bring it back from there.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dropped_from: BTreeMap<String, Digest>,
    /// What the person brings back for this round to make again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bring: Vec<crate::io::Brought>,
    /// The question of this round's output, once asked.
    pub question: Option<String>,
    /// What a role step asked in this round, in order.
    pub inputs: Vec<String>,
    /// The round a certified stop skipped asking.
    pub skipped: Option<u32>,
    /// The session of the attempt under way or last cut off from outside: the next attempt goes on in it. An attempt
    /// that ended by itself clears it.
    #[serde(default)]
    pub resume: Option<String>,
    /// The tokens the attempts in `session` used, all told: what a round that goes on in it carries.
    #[serde(default, skip_serializing_if = "nothing_carried")]
    pub carried: u64,
    /// The person dropped everything the step showed last, and nothing was delivered since: its next round is new.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dropped_all: bool,
    /// What the checks said of the last attempt they judged. For the person: an agent's turn never carries it.
    #[serde(default)]
    pub checks: Vec<crate::event::CheckRecord>,
    /// The open question about this step's act, when it cannot be undone, already acted and was sent back or what it
    /// rested on changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<String>,
    /// The step acts on something real, is not safe to repeat, and delivered in this intent: it acted, whatever its
    /// checks say after; nothing but the person's order of a second act starts it again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub acted: bool,
    /// An attempt of this step, which acts, ended with no outcome recorded: it may have acted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub maybe_acted: bool,
    /// Acts the person ordered again: part of the act's key, so a second act is a new intent.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub repeat: u32,
    /// The question whose choice this round keeps alone: once done, its output is what that question was about.
    #[serde(default)]
    pub narrow: Option<String>,
    /// The question at a later step whose answer sent this step's work back here.
    #[serde(default)]
    pub from: Option<String>,
    /// The person's words for this step in earlier rounds, oldest first.
    #[serde(default)]
    pub earlier: Vec<String>,
    /// A check could not judge the output under its checks (`pending`): it waits for the person's continue, which has
    /// them judge that same output again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unjudged: bool,
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
    /// The state of the project the output was measured at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<String>,
    /// The agent's few words beside the output, which go to the stop's thread once the output is done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
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

fn nothing_carried(n: &u64) -> bool {
    *n == 0
}

impl StepState {
    /// An attempt that ended in `session`, having used `tokens`: in the session the step had, its tokens add to what
    /// that session carries; in a new one, they are all it carries.
    fn carry(&mut self, session: Option<&String>, tokens: u64) {
        let Some(new) = session else { return };
        self.carried = if self.session.as_ref() == Some(new) { self.carried + tokens } else { tokens };
    }
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Run {
    /// Whether step `i` cannot be undone and already acted on what it rests on: from then on it is never done again on
    /// its own (G14).
    pub fn acted(&self, i: usize) -> bool {
        self.manifest.steps[i].effect.irreversible() && self.steps[i].status == Status::Done && self.steps[i].done.is_some()
    }

    /// Whether step `i` acted in this intent and is not done: its checks did not pass, or could not judge, after the act.
    pub fn acted_unfinished(&self, i: usize) -> bool {
        self.steps[i].acted && self.steps[i].status != Status::Done
    }

    /// The open question about step `i`'s act, if any.
    pub fn held(&self, i: usize) -> Option<&QuestionState> {
        self.steps[i].held.as_ref().and_then(|id| self.questions.get(id)).filter(|q| q.open())
    }

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
        let Event::RunStarted { run, request, manifest, role_fingerprint, agent, engine, protocol, format, risk, reasons, ceilings, ceiling, attention, never_merge, starter, .. } = e else {
            return None;
        };
        Some(Run {
            id: run.clone(),
            request: request.clone(),
            steps: vec![StepState::default(); manifest.steps.len()],
            manifest: manifest.clone(),
            role_fingerprint: role_fingerprint.clone(),
            engine: engine.clone(),
            agent: agent.clone(),
            protocol: *protocol,
            format: *format,
            risk: *risk,
            risk_why: if *risk > Risk::Low { reasons.clone() } else { Vec::new() },
            ceilings: *ceilings,
            ceiling: *ceiling,
            spent: Usage::default(),
            questions: BTreeMap::new(),
            phase: Phase::Running,
            halted: false,
            starter: starter.clone(),
            set_aside: None,
            delivering: None,
            delivered: None,
            counts: Counts::default(),
            attention: attention.clone(),
            context: Vec::new(),
            undone: false,
            told: Default::default(),
            seq: 1,
            merges: BTreeMap::new(),
            never_merge: *never_merge,
            planted: BTreeMap::new(),
            unplanted: Default::default(),
            ports: None,
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
            Event::DefectPlanted { id, step, .. } => {
                self.planted.insert(id.clone(), step.clone());
            }
            Event::DefectRemoved { id, .. } => {
                self.planted.remove(id);
                self.unplanted.insert(id.clone());
            }
            Event::RiskRaised { to, ceiling, reasons, .. } => {
                self.risk = *to;
                self.risk_why = reasons.clone();
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
                    s.unjudged = false;
                    s.status = Status::Running { attempt: *attempt, fix: *fix };
                    if session.is_some() {
                        s.session = session.clone();
                    }
                }
            }
            Event::AttemptDelivered { step, attempt, fix, output, items, changed, usage, session, tree, reply } => {
                self.spent.add(*usage);
                let acts = self.manifest.index(step).is_some_and(|i| self.manifest.steps[i].effect.acts() && !self.manifest.steps[i].repeats_safely());
                if let Some(s) = self.step_mut(step) {
                    s.carry(session.as_ref(), usage.tokens);
                    s.dropped_all = false;
                    s.acted |= acts;
                    s.resume = None;
                    s.pending = Some(Pending { output: output.clone(), items: items.clone(), changed: changed.clone(), tree: tree.clone(), reply: reply.clone() });
                    s.unjudged = false;
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
                    s.resume = None;
                    if *spent {
                        s.spent += 1;
                        // Only the agent's own failure is told to its next attempt: a fault of Jaynes may name
                        // what the agent must not see, such as a hidden check.
                        s.failure = reasons.clone();
                    }
                    s.pending = None;
                    s.unjudged = false;
                    s.status = Status::Idle;
                }
            }
            Event::SessionStarted { step, session, .. } => {
                if let Some(s) = self.step_mut(step) {
                    s.resume = Some(session.clone());
                }
            }
            Event::AttemptImpossible { step, usage, session, .. } => {
                self.spent.add(*usage);
                if let Some(s) = self.step_mut(step) {
                    s.carry(session.as_ref(), usage.tokens);
                    s.resume = None;
                    s.pending = None;
                    s.status = Status::Idle;
                    if session.is_some() {
                        s.session = session.clone();
                    }
                }
            }
            Event::AttemptInterrupted { step, .. } => {
                if let Some(s) = self.step_mut(step) {
                    s.pending = None;
                    s.unjudged = false;
                    s.status = Status::Idle;
                }
            }
            Event::StaleReport { .. } => {}
            Event::ChecksJudged { step, checks, .. } => {
                if let Some(s) = self.step_mut(step) {
                    s.checks = checks.clone();
                }
            }
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
                    s.unjudged = false;
                    s.failure.clear();
                    // what a choice kept alone was already answered: the question stays the round's
                    if let Some(q) = s.narrow.take() {
                        s.question = Some(q);
                    }
                }
            }
            Event::StepInvalidated { step, round, notes, narrow, from, bring, .. } => {
                if let Some(s) = self.step_mut(step) {
                    s.bring = bring.clone();
                    s.narrow = narrow.clone();
                    s.from = from.clone();
                    let said = std::mem::take(&mut s.notes);
                    s.earlier.extend(said);
                    s.round = *round;
                    s.status = Status::Idle;
                    s.spent = 0;
                    s.notes = notes.clone();
                    s.failure.clear();
                    s.pending = None;
                    s.unjudged = false;
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
                if question.kind == QuestionKind::Acted
                    && let Some(s) = self.step_mut(&question.step.clone()) {
                        s.held = Some(id.clone());
                    }
                if question.kind == QuestionKind::Merge {
                    let delivery = self.delivery().0;
                    self.merges.insert(id.clone(), delivery);
                }
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
            Event::MayHaveActed { step, .. } => {
                if let Some(s) = self.step_mut(step) {
                    s.maybe_acted = true;
                }
            }
            Event::ActKept { step, reads } => {
                if let Some(s) = self.step_mut(step)
                    && let Some(d) = s.done.as_mut() {
                        d.reads = reads.clone();
                    }
            }
            Event::AnswerRecorded { answer, effects } => {
                self.unwait();
                let Some(q) = self.questions.get_mut(&answer.question) else { return };
                if q.question.kind == QuestionKind::Acted {
                    q.answer = Some(answer.clone());
                    q.effects = Some(effects.clone());
                    let step = q.question.step.clone();
                    let again = answer.verdict(&step).sends_back();
                    if let Some(s) = self.step_mut(&step) {
                        s.held = None;
                        if again && s.maybe_acted {
                            // the same intent again, with its own key: the outcome of the first was never known
                            s.maybe_acted = false;
                        } else if again {
                            s.repeat += 1;
                            s.acted = false;
                        }
                    }
                    return;
                }
                let first = q.answer.is_none();
                q.answer = Some(answer.clone());
                q.effects = Some(effects.clone());
                let question = q.question.clone();
                if answer.by.person() && !answer.context.trim().is_empty() {
                    self.context.push((question.step.clone(), answer.context.trim().to_string()));
                }
                // A leave refused is not work sent back, nor an answer to what a step asked: only a decision about an
                // output counts as rework.
                let input = self.manifest.index(&question.step).is_some_and(|i| self.steps[i].inputs.contains(&question.id));
                if answer.sends_back() && first && question.kind == QuestionKind::Decision && !input && !question.choose {
                    self.counts.sent_back += 1;
                }
                let Some(i) = self.manifest.index(&question.step) else { return };
                let executor = self.manifest.steps[i].executor;
                let s = &mut self.steps[i];
                if let Status::AwaitingInput { question: waiting } = &s.status {
                    if *waiting == question.id {
                        s.status = Status::Idle;
                        // an agent goes on from the person's words; a role step reads its answers itself
                        if executor == Executor::Agent {
                            s.notes.extend(crate::decide::notes(&question, answer));
                        }
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
                let here = s.done.as_ref().map(|d| d.output.clone()).unwrap_or_default();
                let drops_all = answer.sends_back() && answer.drops_all(&question);
                s.dropped_all = drops_all;
                for item in &question.items {
                    match answer.verdict(&item.id) {
                        Verdict::Keep => {
                            s.locks.insert(item.id.clone(), (item.clone(), question.id.clone()));
                        }
                        Verdict::Drop
                            if !s.dropped.contains(item) => {
                                s.dropped.push(item.clone());
                                s.dropped_from.insert(item.id.clone(), here.clone());
                            }
                        _ => {}
                    }
                }
                // what the person brings back may come again: they undo their own drop, never an agent
                for id in answer.back.keys() {
                    s.dropped.retain(|x| &x.id != id);
                    s.dropped_from.remove(id);
                }
                // what goes back to the step that made the work may not bring back what was dropped there either
                let maker = self.manifest.steps[i].stop.as_ref().and_then(|st| st.redo.clone()).and_then(|r| self.manifest.index(&r));
                if let Some(j) = maker.filter(|_| answer.sends_back() && answer.kept_alone(&question).is_none()) {
                    let made = self.steps[j].done.as_ref().map(|d| d.output.clone()).unwrap_or_default();
                    let m = &mut self.steps[j];
                    m.dropped_all = drops_all;
                    for item in &question.items {
                        if answer.verdict(&item.id) == Verdict::Drop && !m.dropped.contains(item) {
                            m.dropped.push(item.clone());
                            m.dropped_from.insert(item.id.clone(), made.clone());
                        }
                    }
                    for id in answer.back.keys() {
                        m.dropped.retain(|x| &x.id != id);
                        m.dropped_from.remove(id);
                    }
                }
            }
            Event::Waiting { step, on, why } => {
                self.told.extend(on.iter().cloned());
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
                            // the checks judge the same output again, under way from here
                            s.unjudged = false;
                        }
                        if matches!(fault, Fault::MayHaveActed | Fault::Role) {
                            s.status = Status::Idle;
                        }
                    }
                self.delivering = None;
                self.phase = Phase::Running;
            }
            Event::ChecksUnjudged { step, .. } => {
                if let Some(s) = self.step_mut(step) {
                    s.unjudged = true;
                }
            }
            Event::PortsChosen { any, servers, opened } => self.ports = Some(Ports { any: *any, servers: *servers, opened: *opened }),
            Event::Halted { .. } => self.halted = true,
            // the thread of a stop: folded with the asking, which the core does not take yet
            Event::AgentAsked { .. } | Event::AgentAnswered { .. } | Event::AskFailed { .. } => {}
            Event::SetAside { by } => {
                self.halted = true;
                self.set_aside = Some(by.clone());
            }
            Event::Resumed { .. } => {
                self.halted = false;
                self.set_aside = None;
            }
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
            Event::MergeUndone { .. } => self.undone = true,
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

    /// The step whose output waits, blocked, for its checks to judge it again: a check could not judge it, and the
    /// person's continue has them judge that same output, the agent working no more.
    pub fn checks_again(&self) -> Option<usize> {
        let Phase::Blocked { step: Some(name), fault: Fault::Jaynes, .. } = &self.phase else { return None };
        let i = self.manifest.index(name)?;
        let s = &self.steps[i];
        (s.unjudged && matches!(s.status, Status::Checking { .. }) && s.pending.is_some()).then_some(i)
    }

    /// The steps step `i` uses: those it needs, and, for a step on the project, every earlier step that changes
    /// the project, since their changes are in its copy.
    /// The rounds the person sent step `i`'s work back from its own stop: a choice kept alone is no round.
    pub fn sent_back_at(&self, i: usize) -> u32 {
        let name = &self.manifest.steps[i].name;
        self.questions
            .values()
            .filter(|q| &q.question.step == name && q.question.kind == QuestionKind::Decision && !self.steps[i].inputs.contains(&q.question.id))
            .filter_map(|q| q.answer.as_ref().map(|a| (q, a)))
            .filter(|(q, a)| a.by.person() && a.sends_back() && a.kept_alone(&q.question).is_none())
            .count() as u32
    }

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

    /// G19: what the delivery brings in that holds a merge, each with the last step on the path that changed it, in
    /// the order the path changed them.
    pub fn merge_holds(&self) -> Vec<(Changed, String)> {
        self.delivered_files().into_iter().filter(|(c, _)| crate::risk::holds_merge(self.manifest.target, c)).collect()
    }

    /// Every file the delivery brings, as the last step of the path that changed it left it, with that step's name:
    /// what a merge lands on the person's branch, and what the stop before it shows.
    pub fn delivered_files(&self) -> Vec<(Changed, String)> {
        let mut out: Vec<(Changed, String)> = Vec::new();
        for i in self.path() {
            let Some(done) = self.steps[i].done.as_ref() else { continue };
            for c in &done.changed {
                out.retain(|(o, _)| o.what != c.what);
                out.push((c.clone(), self.manifest.steps[i].name.clone()));
            }
        }
        out
    }

    /// G19: the person's answer about merging the delivery `digest`, if they gave one about it: keep (it merges) when
    /// they kept every file it showed, else drop (the run's branch only).
    pub fn merge_answer(&self, digest: &Digest) -> Option<bool> {
        self.merges
            .iter()
            .filter(|(_, d)| *d == digest)
            .filter_map(|(id, _)| self.questions.get(id))
            .filter(|q| !q.superseded)
            .find_map(|q| q.answer.as_ref().filter(|a| a.by.person()).map(|a| q.question.items.iter().all(|x| a.verdict(&x.id) == Verdict::Keep)))
    }

    /// G19: the question that holds the merge of the delivery `digest`, while it waits for the person.
    pub fn merge_open(&self, digest: &Digest) -> Option<&QuestionState> {
        self.merges.iter().filter(|(_, d)| *d == digest).filter_map(|(id, _)| self.questions.get(id)).find(|q| q.open())
    }

    /// Whether `by`, giving `starter`, is the agent that started the run: never the person, nor a run with none.
    pub fn started_by(&self, by: &By, starter: Option<&str>) -> bool {
        matches!(by, By::Agent { .. }) && starter.is_some() && starter == self.starter.as_deref()
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
