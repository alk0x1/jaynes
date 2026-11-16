//! The one kind of question the engine asks the person: items shown, a verdict per item from a closed list, and
//! who answered. The role says what the items are; the engine says what each answer will do.

use crate::digest::Digest;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The closed list of verdicts on an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerdictKind {
    /// Keep the item as it is. A later round may not change it.
    Keep,
    /// Change it as the person writes.
    Change,
    /// Have the step do it again, with the person's note.
    Redo,
    /// Drop it for good: it does not come back in the run.
    Drop,
    /// "I don't know": leave it as the agent made it, unjudged.
    Skip,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    Keep,
    Change { text: String },
    Redo { note: String },
    Drop,
    Skip,
}

impl Verdict {
    pub fn kind(&self) -> VerdictKind {
        match self {
            Verdict::Keep => VerdictKind::Keep,
            Verdict::Change { .. } => VerdictKind::Change,
            Verdict::Redo { .. } => VerdictKind::Redo,
            Verdict::Drop => VerdictKind::Drop,
            Verdict::Skip => VerdictKind::Skip,
        }
    }

    /// Whether the verdict sends the step back to work.
    pub fn sends_back(&self) -> bool {
        matches!(self, Verdict::Change { .. } | Verdict::Redo { .. } | Verdict::Drop)
    }
}

/// One thing shown to the person, as the role listed it from the step's output.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Item {
    pub id: String,
    pub label: String,
    /// The fingerprint of the item's content, so a kept item can be told apart from a changed one.
    pub digest: Digest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// A decision of taste or direction: always the person's.
    Decision,
    /// Leave for an agent to reach something: a routine one may be answered by a rule in code.
    Permission,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Question {
    pub id: String,
    pub step: String,
    pub round: u32,
    pub kind: QuestionKind,
    pub items: Vec<Item>,
    pub offered: Vec<VerdictKind>,
    /// Whether the run waits for the answer.
    pub holds: bool,
    #[serde(default)]
    pub about: String,
    /// The person chooses one item, and may write how to adjust it; the others leave the running.
    #[serde(default, skip_serializing_if = "crate::manifest::is_false")]
    pub choose: bool,
    /// The round of asking at this stop, from 1, shown as information. `passes` is no longer set: rounds the
    /// person opens have no ceiling. Questions recorded before keep theirs, and they no longer limit.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pass: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub passes: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Question {
    /// The fingerprint of the question as it is shown. An answer carries it.
    pub fn shown(&self) -> Digest {
        Digest::of_json(self)
    }
}

/// Who gave an answer, and under which authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum By {
    /// The person, from the page or a channel only they hold.
    Person,
    /// A rule in code, by its name.
    Rule { rule: String },
    /// An agent or any other program.
    Agent { who: String },
}

impl By {
    pub fn person(&self) -> bool {
        matches!(self, By::Person)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Answer {
    pub question: String,
    /// The fingerprint of the question the answerer was shown.
    pub shown: Digest,
    /// A verdict per item id. An item left out is kept.
    #[serde(default)]
    pub verdicts: BTreeMap<String, Verdict>,
    pub by: By,
    /// The person's own words for the agents that work after this answer. It changes no verdict.
    #[serde(default)]
    pub context: String,
}

impl Answer {
    pub fn verdict(&self, item: &str) -> Verdict {
        self.verdicts.get(item).cloned().unwrap_or(Verdict::Keep)
    }

    pub fn sends_back(&self) -> bool {
        self.verdicts.values().any(Verdict::sends_back)
    }

    /// What an answer to a stop that chooses does: one option goes on, two or three finalists are developed and
    /// asked about again, or none goes on and new options are made. None when it is no valid choice.
    pub fn choice(&self, q: &Question) -> Option<Choice> {
        let going: Vec<Verdict> = q.items.iter().map(|x| self.verdict(&x.id)).filter(|v| *v != Verdict::Drop).collect();
        let judged = q.items.iter().all(|x| matches!(self.verdict(&x.id), Verdict::Keep | Verdict::Change { .. } | Verdict::Drop));
        match going.len() {
            _ if !judged => None,
            0 => Some(Choice::None),
            1 => Some(Choice::One),
            _ if going.iter().all(|v| matches!(v, Verdict::Change { .. })) => Some(Choice::Finalists),
            _ => None,
        }
    }

    /// The items a choice keeps as they are, when it changes none: Jaynes keeps them alone, without the agent.
    pub fn kept_alone(&self, q: &Question) -> Option<Vec<String>> {
        let changes = q.items.iter().any(|x| matches!(self.verdict(&x.id), Verdict::Change { .. } | Verdict::Redo { .. }));
        (q.choose && self.choice(q) == Some(Choice::One) && self.sends_back() && !changes).then(|| q.items.iter().filter(|x| self.verdict(&x.id) == Verdict::Keep).map(|x| x.id.clone()).collect())
    }
}

/// What a choice among options does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// One option goes on, as it is or adjusted.
    One,
    /// Two or more go on as finalists, each developed by the agent, and the person chooses again.
    Finalists,
    /// None goes on: the agent makes new options from the person's words, and these never come back.
    None,
}

/// A leave an agent asks for in the middle of an attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Ask {
    pub kind: AskKind,
    /// A path relative to the step's copy of the project, a host, or a command.
    pub target: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AskKind {
    Read,
    Write,
    Network,
    Command,
}
