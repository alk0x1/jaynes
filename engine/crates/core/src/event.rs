//! What the record holds: one event per change of a run's state. A run's state is only ever the fold of its
//! events (`Run::fold`). Fields are read by name; a new field comes with a default, and a field is never renamed
//! nor given a new meaning: a new meaning is a new event.

use crate::digest::Digest;
use crate::manifest::Manifest;
use crate::question::{Answer, By, Item, Question};
use crate::risk::{Ceiling, Ceilings, Changed, Risk};
use serde::{Deserialize, Serialize};

/// Time and tokens one attempt took, from the agent program's own meter as the shell read it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Usage {
    pub seconds: u64,
    /// Tokens sent and received, without those read back from the cache.
    pub tokens: u64,
    /// Tokens read back from the cache, kept apart.
    #[serde(default)]
    pub cached: u64,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        self.seconds = self.seconds.saturating_add(other.seconds);
        self.tokens = self.tokens.saturating_add(other.tokens);
        self.cached = self.cached.saturating_add(other.cached);
    }
}

/// What a step used: the output of an earlier step, by its fingerprint at the time.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Read {
    pub step: String,
    pub output: Digest,
}

/// Whose fault stopped a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    /// Jaynes or the machine: no attempt was spent.
    Jaynes,
    /// The agent spent every attempt of the round.
    Agent,
    /// The role program said the step cannot be done, with its reason.
    Role,
    /// The time or token ceiling was reached.
    Ceiling,
    /// A step that cannot be undone was under way when Jaynes stopped, so it may have acted.
    MayHaveActed,
    /// Stopped from outside, such as the agent program's quota: it goes on once that passes.
    Paused,
}

/// What an answer will do, written by the engine before the answer is sent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Effects {
    pub sentences: Vec<String>,
    /// Steps that run again for certain.
    pub reruns: Vec<String>,
    /// Steps that run again only if what they used comes out different.
    pub may_rerun: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Delivered,
    Abandoned,
}

/// What a finished run cost, and how much of it was sent back.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Cost {
    pub risk: Risk,
    pub usage: Usage,
    pub attempts: u32,
    /// Attempts the agent spent by its own fault.
    pub spent: u32,
    pub fixes: u32,
    pub questions: u32,
    /// Answers that sent a step back.
    pub sent_back: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum Event {
    RunStarted {
        run: String,
        request: String,
        manifest: Manifest,
        role_fingerprint: Digest,
        /// The agent program and model, fixed for the run.
        #[serde(default)]
        agent: String,
        engine: String,
        protocol: u32,
        format: u32,
        risk: Risk,
        reasons: Vec<String>,
        ceilings: Ceilings,
        ceiling: Ceiling,
        /// Each stop's count as it stood at the start, by step.
        #[serde(default)]
        attention: std::collections::BTreeMap<String, crate::attention::Count>,
        /// The partner's id of the task, a label (`io::is_label`), when the run is part of an evaluation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task: Option<String>,
        /// The arm of the evaluation the run is in, a label.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arm: Option<String>,
        /// The person's device keys, as paired when the run started: the only keys its proofs are checked by.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        person_keys: Vec<crate::proof::PersonKey>,
        /// A nonce of this start, so the run's fingerprint is its own though another run had the same id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nonce: Option<String>,
        /// The person asked the run to run outside its box: it waits for their proof before any command on the project.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        unboxed: bool,
    },
    RiskRaised {
        to: Risk,
        by: By,
        reasons: Vec<String>,
        ceiling: Ceiling,
    },
    CeilingRaised {
        ceiling: Ceiling,
        by: By,
    },
    ProbeAsked {
        step: String,
    },
    ProbePassed {
        step: String,
    },
    AttemptStarted {
        step: String,
        round: u32,
        attempt: u32,
        fix: u32,
        session: Option<String>,
        deadline_seconds: u64,
        tokens_left: u64,
        /// For a step that cannot be undone: the key that makes a repeated request count once. New each round.
        key: Option<String>,
    },
    AttemptDelivered {
        step: String,
        attempt: u32,
        fix: u32,
        output: Digest,
        items: Vec<Item>,
        changed: Vec<Changed>,
        usage: Usage,
        session: Option<String>,
    },
    AttemptFailed {
        step: String,
        attempt: u32,
        reasons: Vec<String>,
        usage: Usage,
        /// Whether the failure was the agent's and spent the attempt.
        spent: bool,
    },
    AttemptInterrupted {
        step: String,
        attempt: u32,
        reason: String,
    },
    /// A report that came for an attempt no longer under way. Recorded, and changes nothing.
    StaleReport {
        step: String,
        attempt: u32,
        what: String,
    },
    FixAsked {
        step: String,
        attempt: u32,
        fix: u32,
        reasons: Vec<String>,
    },
    InputNeeded {
        step: String,
        attempt: u32,
        question: Question,
    },
    /// What each check of an attempt said, passed or not: its kind, and the earlier steps whose outputs it read.
    ChecksJudged {
        step: String,
        attempt: u32,
        fix: u32,
        checks: Vec<CheckRecord>,
    },
    /// The session an attempt runs in, known as its turn started.
    SessionStarted {
        step: String,
        attempt: u32,
        session: String,
        #[serde(default)]
        confirmed: Confirmed,
    },
    /// The agent said the step cannot be done as asked; the question for the person follows.
    AttemptImpossible {
        step: String,
        attempt: u32,
        reason: String,
        usage: Usage,
        session: Option<String>,
    },
    StepDone {
        step: String,
        round: u32,
        output: Digest,
        items: Vec<Item>,
        reads: Vec<Read>,
        changed: Vec<Changed>,
    },
    /// An attempt of a step that acts on something real ended with no outcome recorded: it may have acted. Nothing
    /// starts it again on its own; the person is asked, and acting again repeats it with the same key.
    MayHaveActed {
        step: String,
        attempt: u32,
    },
    /// The person kept an act that cannot be undone as it was, though what it rested on changed: it stands on what
    /// holds now, as it acted, and is not done again.
    ActKept {
        step: String,
        reads: Vec<Read>,
    },
    StepInvalidated {
        step: String,
        round: u32,
        because: Vec<String>,
        notes: Vec<String>,
        /// The question whose choice this round keeps alone, without the agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        narrow: Option<String>,
        /// The question, at a later step that reviewed this one's work, whose answer sent it back here.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        /// What the person brings back of what they dropped, for this round to make again.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        bring: Vec<crate::io::Brought>,
    },
    QuestionOpened {
        question: Question,
    },
    QuestionSuperseded {
        question: String,
    },
    /// The input that follows came with a proof only the person can give, and it held for one of the run's keys. The
    /// input and the proof are kept whole, so reading the record again checks the proof again.
    Proved {
        nonce: String,
        key: crate::proof::KeyKind,
        input: Box<crate::io::Input>,
        proof: crate::proof::Proof,
    },
    AnswerRecorded {
        answer: Answer,
        effects: Effects,
    },
    Waiting {
        step: Option<String>,
        on: Vec<String>,
        why: String,
    },
    Blocked {
        step: Option<String>,
        fault: Fault,
        reason: String,
    },
    Continued {
        by: By,
    },
    Halted {
        by: By,
    },
    Resumed {
        by: By,
    },
    DeliveryAsked {
        digest: Digest,
    },
    Delivered {
        digest: Digest,
        reference: String,
    },
    RunFinished {
        outcome: Outcome,
        cost: Cost,
    },
    DeliveryCorrected {
        note: String,
        by: By,
        /// The step whose output was wrong, when known: the error counts on the first stop at or after it.
        #[serde(default)]
        step: Option<String>,
    },
    /// The person reverted every merge of the run. It merges nothing again, and what it let through earns no right.
    MergeUndone {
        note: String,
        by: By,
    },
    /// A certified stop did not ask this round: the coin fell outside its sample.
    StopSkipped {
        step: String,
        round: u32,
        key: Digest,
        coin: crate::manifest::Rate,
        n: u64,
        k: u64,
    },
}

impl Event {
    pub fn name(&self) -> &'static str {
        match self {
            Event::RunStarted { .. } => "RunStarted",
            Event::RiskRaised { .. } => "RiskRaised",
            Event::CeilingRaised { .. } => "CeilingRaised",
            Event::ProbeAsked { .. } => "ProbeAsked",
            Event::ProbePassed { .. } => "ProbePassed",
            Event::AttemptStarted { .. } => "AttemptStarted",
            Event::AttemptDelivered { .. } => "AttemptDelivered",
            Event::AttemptFailed { .. } => "AttemptFailed",
            Event::AttemptInterrupted { .. } => "AttemptInterrupted",
            Event::StaleReport { .. } => "StaleReport",
            Event::FixAsked { .. } => "FixAsked",
            Event::InputNeeded { .. } => "InputNeeded",
            Event::AttemptImpossible { .. } => "AttemptImpossible",
            Event::SessionStarted { .. } => "SessionStarted",
            Event::ChecksJudged { .. } => "ChecksJudged",
            Event::StepDone { .. } => "StepDone",
            Event::ActKept { .. } => "ActKept",
            Event::MayHaveActed { .. } => "MayHaveActed",
            Event::StepInvalidated { .. } => "StepInvalidated",
            Event::QuestionOpened { .. } => "QuestionOpened",
            Event::QuestionSuperseded { .. } => "QuestionSuperseded",
            Event::Proved { .. } => "Proved",
            Event::AnswerRecorded { .. } => "AnswerRecorded",
            Event::Waiting { .. } => "Waiting",
            Event::Blocked { .. } => "Blocked",
            Event::Continued { .. } => "Continued",
            Event::Halted { .. } => "Halted",
            Event::Resumed { .. } => "Resumed",
            Event::DeliveryAsked { .. } => "DeliveryAsked",
            Event::Delivered { .. } => "Delivered",
            Event::RunFinished { .. } => "RunFinished",
            Event::DeliveryCorrected { .. } => "DeliveryCorrected",
            Event::StopSkipped { .. } => "StopSkipped",
            Event::MergeUndone { .. } => "MergeUndone",
        }
    }
}

/// One check's verdict on an attempt, as the record keeps it: where its answer comes from (an open check the agent
/// knew, or a hidden proof) and what it could see besides the step's own output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CheckRecord {
    pub check: String,
    pub kind: crate::manifest::CheckKind,
    pub passed: bool,
    /// It could not judge: a fault of Jaynes or of the machine.
    #[serde(default)]
    pub blocked: bool,
    /// The earlier steps whose outputs it read.
    #[serde(default)]
    pub read: Vec<String>,
}

/// What an agent program says, in the first message of a turn, it runs with: what actually held, beside what Jaynes
/// asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Confirmed {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub permission_mode: String,
    #[serde(default)]
    pub tools: Vec<String>,
}
