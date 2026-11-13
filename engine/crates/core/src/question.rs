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
}

impl Answer {
    pub fn verdict(&self, item: &str) -> Verdict {
        self.verdicts.get(item).cloned().unwrap_or(Verdict::Keep)
    }

    pub fn sends_back(&self) -> bool {
        self.verdicts.values().any(Verdict::sends_back)
    }
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
