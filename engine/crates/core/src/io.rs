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
        /// The partner's own id of the task, when the run is part of an evaluation: a label ([`is_label`]), never
        /// a title. None: the export of numbers names the task by a fingerprint of the run.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task: Option<String>,
        /// The arm of the evaluation the run is in, a label too. None: none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arm: Option<String>,
        /// The run never merges, whatever its role delivers and whatever is answered in it: an evaluation's run
        /// delivers its branch only.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        never_merge: bool,
        /// Who started the run, when an agent did: the agent program's process that the start came from, by its
        /// pid and start time, which the shell finds. That agent may continue a block of Jaynes's that judges the
        /// same change again, and set the run aside; nothing else. None: the person's alone.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        starter: Option<String>,
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
        /// What the agent program says it runs with, in its first message.
        #[serde(default)]
        confirmed: crate::event::Confirmed,
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
        /// The agent's process the input came from, as the start's `starter` names it, when an agent sends it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        starter: Option<String>,
    },
    DeliveryEnded {
        digest: Digest,
        ok: bool,
        #[serde(default)]
        reference: String,
    },
    /// The person's abandon is final. The agent that started the run sets it aside instead: nothing goes on in it,
    /// its work is kept, and only the person brings it back or abandons it for good.
    Abandon {
        by: By,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        starter: Option<String>,
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
    /// A defect an evaluation plants into the change of a done step, before its stop shows it, to see whether the
    /// person who reviews looks. The shell keeps what was planted; the core keeps that one is in, by its id.
    Plant {
        id: String,
        step: String,
        by: By,
    },
    /// The planted defect `id` taken out, its step's change as it was.
    Unplant {
        id: String,
        by: By,
    },
    /// What the person chose for the project's ports since the run's setup, as the run's own process reads it from
    /// the person's file when they continue: every local port (`any`, opened at `opened`, in seconds since 1970) or
    /// only the reserved and declared ones; `servers`: the project's tests may start their own test servers on ports
    /// the system picks (`jaynes project test-servers`); `freed_ports`: with them, the ports of the usual databases and
    /// caches the box holds that the person let the project's tests use. The shell's alone: nothing from outside the
    /// run's process sends it.
    PortsChosen {
        any: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        servers: bool,
        #[serde(default)]
        opened: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        freed_ports: Vec<u16>,
    },
    /// The person asks the agent of a stop's step something about what it shows, without sending it back: the stop
    /// stays open in the same round, with the same items, and the agent answers in a turn that only reads.
    AskAgent {
        question: String,
        /// The item the words are about, when they are about one.
        #[serde(default)]
        item: Option<String>,
        text: String,
        by: By,
    },
    /// The agent's answer to the person's ask `ask`, or why there is none.
    AskEnded {
        ask: String,
        outcome: AskOutcome,
    },
}

/// How a turn that answers the person's ask ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AskOutcome {
    Answered {
        text: String,
        usage: Usage,
        #[serde(default)]
        session: Option<String>,
    },
    /// No answer: the agent did not start, wrote nothing, changed the project, or Jaynes stopped meanwhile.
    Failed { reason: String, usage: Usage },
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
        /// The state of the project the output was measured at (on a repository, its tree): what its checks judge
        /// and what the delivery carries.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tree: Option<String>,
        /// The agent's few words beside its output, never inside it: what it changed and why, and the answer to
        /// what the person asked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply: Option<String>,
    },
    /// The agent's fault: no output, an output that does not parse, a write outside its reach, its deadline.
    Failed { reasons: Vec<String>, usage: Usage },
    /// Jaynes's or the machine's fault: the agent program did not start, the box refused, the disk is full. What
    /// the turn spent before it, as its stream said it, still counts towards the ceiling (G5).
    Fault {
        reason: String,
        #[serde(default, skip_serializing_if = "Usage::is_nothing")]
        usage: Usage,
    },
    /// Stopped from outside, such as the agent program's quota or the person's halt, with what it spent till then.
    Interrupted {
        reason: String,
        #[serde(default, skip_serializing_if = "Usage::is_nothing")]
        usage: Usage,
    },
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
    /// The state of the project the check was given to judge, when it was given one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<String>,
}

impl CheckResult {
    /// A verdict on no state of the project in particular.
    pub fn new(check: impl Into<String>, verdict: CheckVerdict) -> CheckResult {
        CheckResult { check: check.into(), verdict, tree: None }
    }
}

/// A hidden case that failed, as the check ran it, for the person: what it says, the command it ran, the code it
/// exited with (0 is what it expects) and the end of what it printed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FailedCase {
    pub says: String,
    pub run: String,
    pub code: i32,
    #[serde(default)]
    pub printed: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum CheckVerdict {
    Pass,
    /// Failed. `reasons` is what the agent is told; `detail` and `cases` only the person sees.
    Fail {
        reasons: Vec<String>,
        #[serde(default)]
        detail: String,
        /// The hidden cases that failed, each in full: never told to the agent (G3).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        cases: Vec<FailedCase>,
    },
    /// The check could not judge at all, such as a tool it runs that did not start: Jaynes's fault.
    Blocked { reason: String },
}

// A turn is the largest command and the most common one: boxing it would buy nothing.
#[allow(clippy::large_enum_variant)]
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
    /// A turn that only reads, in the session of the step that made what the stop shows, to answer the person's ask.
    /// It is not an attempt: it spends tokens of the run, never an attempt of the step.
    AnswerAsk(AskTurn),
}

/// The person's ask, as the agent answering it is given it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AskTurn {
    /// `{question}-a{n}`: what the answer comes back as.
    pub ask: String,
    pub question: String,
    pub step: String,
    pub round: u32,
    /// The output the stop shows: the agent answers about it, and it does not change.
    pub output: Digest,
    pub item: Option<String>,
    /// The item's label, or empty when the words are about the whole output.
    pub label: String,
    pub text: String,
    /// The session of the step that made the output; none when it cannot be resumed, and a new one reads the output.
    pub session: Option<String>,
    /// Read only: no write in the project, no network.
    pub reach: Reach,
    pub deadline_seconds: u64,
    pub tokens_left: u64,
}

/// Something the person dropped in an earlier round and now brings back: the item, the step's output it was in, and
/// the person's words on how it comes back (empty: as it was).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Brought {
    pub item: Item,
    pub output: Digest,
    #[serde(default)]
    pub words: String,
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
    /// A round that keeps only the items the person chose, by id: the role does it from the previous output, and
    /// no agent works.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<Vec<String>>,
    /// When the work was sent back from a later step that reviewed it: what that step said of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<Read>,
    /// The person's words for this step in earlier rounds, oldest first, so what was asked is not undone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub earlier: Vec<String>,
    /// What the person brings back of what they dropped, with the output it was in, so the agent can make it again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub back: Vec<Brought>,
    /// What the person asked the agent at this step's stop in the round before, with its answer: nothing it changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asked: Vec<Asked>,
    pub deadline_seconds: u64,
    pub tokens_left: u64,
}

/// A question the person asked at a stop, and the agent's answer, if one came.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Asked {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

/// Whether `text` is a label: what a run carries of an evaluation (its task's id, its arm), and so what the export of
/// numbers takes out of the machine. 1 to 64 ASCII letters, digits and hyphens, the first a letter or a digit (so a
/// flag is never taken for one): a label names, it cannot say.
pub fn is_label(text: &str) -> bool {
    (1..=64).contains(&text.len())
        && text.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
