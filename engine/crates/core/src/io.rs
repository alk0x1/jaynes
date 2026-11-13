//! What comes into the core (inputs, from the shell) and what goes out (commands, for the shell to carry out).
//! The core never touches a disk, a network, a process or a clock: the shell does, and reports back.

use crate::digest::Digest;
use crate::event::{Read, Usage};
use crate::manifest::{CheckDecl, Reach};
use crate::question::{Answer, Ask, By, Item, Question, VerdictKind};
use crate::risk::{Ceiling, Changed, Risk};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "input", rename_all = "snake_case")]
pub enum Input {
    Start {
        run: String,
        request: String,
        manifest: crate::manifest::Manifest,
        /// The fingerprint of the role program's content, so the run stays on this version.
        role_fingerprint: Digest,
        /// The agent program and model the run's agent steps use, fixed for the run: a stop's record of going well
        /// counts only for the agent that earned it.
        #[serde(default)]
        agent: String,
        /// A risk the person raises the run to from the start.
        #[serde(default)]
        raise: Option<Risk>,
        /// Each stop's count from the record (`attention::count`), by step.
        #[serde(default)]
        attention: std::collections::BTreeMap<String, crate::attention::Count>,
    },
    /// Go on as far as the run can without anything from outside.
    Tick,
    ProbeEnded {
        step: String,
        ok: bool,
        #[serde(default)]
        reason: String,
    },
    AttemptEnded {
        step: String,
        attempt: u32,
        fix: u32,
        outcome: AttemptOutcome,
    },
    ChecksEnded {
        step: String,
        attempt: u32,
        fix: u32,
        results: Vec<CheckResult>,
    },
    PermissionAsked {
        step: String,
        attempt: u32,
        ask: Ask,
    },
    /// The agent program said which session the attempt runs in, as the turn started: an attempt cut off from
    /// outside goes on in it.
    SessionStarted {
        step: String,
        attempt: u32,
        session: String,
    },
    Answer {
        answer: Answer,
    },
    RaiseRisk {
        to: Risk,
        by: By,
    },
    RaiseCeiling {
        ceiling: Ceiling,
        by: By,
    },
    Halt {
        by: By,
    },
    Resume {
        by: By,
    },
    /// The shell starts again after it stopped: whatever was under way did not report.
    Recovered,
    /// Go on after a block, once its cause is dealt with.
    Continue {
        by: By,
    },
    DeliveryEnded {
        digest: Digest,
        ok: bool,
        #[serde(default)]
        reference: String,
    },
    Abandon {
        by: By,
    },
    /// The person reverted what the run merged.
    Undo {
        note: String,
        by: By,
    },
    Correct {
        note: String,
        by: By,
        #[serde(default)]
        step: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AttemptOutcome {
    /// An output, with its items as the role listed them and the files changed as Jaynes measured them.
    Delivered {
        output: Digest,
        #[serde(default)]
        items: Vec<Item>,
        #[serde(default)]
        changed: Vec<Changed>,
        usage: Usage,
        #[serde(default)]
        session: Option<String>,
    },
    /// The agent's fault: no output, an output that does not parse, a write outside its reach, its deadline.
    Failed { reasons: Vec<String>, usage: Usage },
    /// Jaynes's or the machine's fault: the agent program did not start, the box refused, the disk is full.
    Fault { reason: String },
    /// Stopped from outside, such as the agent program's quota or the person's halt.
    Interrupted { reason: String },
    /// A role step asks the person something before it can give its output.
    NeedsInput {
        items: Vec<Item>,
        offered: Vec<VerdictKind>,
        #[serde(default)]
        about: String,
    },
    /// A role step cannot be done, for a reason of the role's.
    Refused { reason: String },
    /// An agent says the step cannot be done as asked, and why: the person answers how to go on. It is not a
    /// failure, so it spends no attempt.
    Impossible {
        reason: String,
        usage: Usage,
        #[serde(default)]
        session: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CheckResult {
    pub check: String,
    pub verdict: CheckVerdict,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum CheckVerdict {
    Pass,
    /// Failed. `reasons` is what the agent is told; `detail` only the person sees.
    Fail {
        reasons: Vec<String>,
        #[serde(default)]
        detail: String,
    },
    /// The check could not judge at all, such as a tool it runs that did not start: Jaynes's fault.
    Blocked { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    /// Test, from inside, what the step will reach, before an attempt is spent on it.
    Probe { step: String, reach: Reach },
    RunAgent(AgentTurn),
    RunRole {
        step: String,
        round: u32,
        attempt: u32,
        given: Vec<Read>,
        previous: Option<Digest>,
        /// The answers to what this step asked in this round, in order.
        answers: Vec<Answer>,
        key: Option<String>,
    },
    RunChecks {
        step: String,
        attempt: u32,
        fix: u32,
        output: Digest,
        given: Vec<Read>,
        checks: Vec<String>,
    },
    /// Show the person a question.
    Ask { question: Question },
    /// Tell an agent under way whether it may have what it asked.
    Permit { step: String, attempt: u32, allow: bool },
    /// Make the run's delivery (a branch) from the outputs it is the fingerprint of.
    Deliver {
        digest: Digest,
        given: Vec<Read>,
        /// Also merge it into the branch the run started from: the role delivers by merge and nobody undid it.
        merge: bool,
    },
    /// Tell the person something holds the run.
    Notify { text: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentTurn {
    pub step: String,
    pub round: u32,
    pub attempt: u32,
    /// 0 for a new attempt; from 1, a fix in place of a shape check, in the same session.
    pub fix: u32,
    pub session: Option<String>,
    pub reach: Reach,
    /// The outputs the step is given, by fingerprint.
    pub given: Vec<Read>,
    /// The step's previous output, which a new round works on top of.
    pub previous: Option<Digest>,
    /// The person's words for this round.
    pub notes: Vec<String>,
    /// What the person said for context in answers at this step or before it, oldest first.
    pub context: Vec<String>,
    /// Why the last attempt or fix was rejected, in the checks' own sentences.
    pub failure: Vec<String>,
    /// Only the shape checks: the agent knows them before it delivers.
    pub checks: Vec<CheckDecl>,
    pub locked: Vec<Item>,
    pub dropped: Vec<Item>,
    pub deadline_seconds: u64,
    pub tokens_left: u64,
}
