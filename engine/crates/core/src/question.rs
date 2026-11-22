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
        self.kind().sends_back()
    }
}

impl VerdictKind {
    /// Whether a verdict of this kind sends the step back to work. The one place that says so: an answer is read by
    /// it, and a role is held by it to offer none where nothing may go back.
    pub fn sends_back(self) -> bool {
        matches!(self, VerdictKind::Change | VerdictKind::Redo | VerdictKind::Drop)
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

impl Item {
    /// The item's name: its label before the colon, as the app names a direction ("Python + FastAPI" of "Python +
    /// FastAPI: small and known"); a label with no colon is its own name.
    pub fn name(&self) -> &str {
        self.label.split_once(':').map_or(self.label.as_str(), |(n, _)| n).trim()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// A decision of taste or direction: always the person's.
    Decision,
    /// Leave for an agent to reach something: a routine one may be answered by a rule in code.
    Permission,
    /// A step that cannot be undone already acted, and something sent it back or what it rested on changed: it is not
    /// done again on its own. The person keeps the act as it is, or orders a second act.
    Acted,
    /// G19: a merge would bring into the person's branch what runs or is obeyed later outside any box. It waits for
    /// the person, who keeps it (it merges) or drops it (the change stays on the run's branch only).
    Merge,
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
    /// What the person dropped at this stop in earlier rounds, kept in view: an answer may bring any of it back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub back: Vec<Item>,
    /// At the merge: the stops still open that do not hold the run, which the person's touch that merges keeps, each
    /// with its own answer in the record. Part of what is shown, so the touch counts only for these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rides: Vec<Ride>,
}

/// A stop still open that the merge's touch keeps: its question, its step, and what it holds now, in Jaynes's words
/// ("the bug, as your request says").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Ride {
    pub question: String,
    pub step: String,
    pub said: String,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Question {
    /// The fingerprint of the question as it is shown. An answer carries it.
    pub fn shown(&self) -> Digest {
        Digest::of_json(self)
    }

    /// A stop that chooses with one option only: there is nothing to choose among, so it asks yes or no. Going with
    /// it is the choice; a change is "not yet", the person's words go to the agent and nothing is chosen.
    pub fn one_option(&self) -> bool {
        self.choose && self.items.len() == 1
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
    /// What the person brings back of what they dropped in earlier rounds, by id, with their words on how; empty
    /// words bring it back as it was. Only the person brings back.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub back: BTreeMap<String, String>,
}

impl Answer {
    pub fn verdict(&self, item: &str) -> Verdict {
        self.verdicts.get(item).cloned().unwrap_or(Verdict::Keep)
    }

    pub fn sends_back(&self) -> bool {
        self.verdicts.values().any(Verdict::sends_back) || !self.back.is_empty()
    }

    /// Whether the answer drops everything the question showed and brings nothing back: the person wants new ideas.
    pub fn drops_all(&self, q: &Question) -> bool {
        !q.items.is_empty() && self.back.is_empty() && q.items.iter().all(|x| self.verdict(&x.id) == Verdict::Drop)
    }

    /// Whether the answer asks the step to start over: it sends everything the question showed back, each item dropped
    /// or to be done again without a word, keeps none and brings nothing back. The person wants new ideas, which what
    /// the step made, and the conversation that made it, would hold back.
    pub fn starts_over(&self, q: &Question) -> bool {
        let wordless = |v: Verdict| matches!(v, Verdict::Drop) || matches!(v, Verdict::Redo { note } if note.trim().is_empty());
        self.drops_all(q)
            || (!q.items.is_empty() && self.back.is_empty() && self.context.trim().is_empty() && q.items.iter().all(|x| wordless(self.verdict(&x.id))))
    }

    /// What an answer to a stop that chooses does: one option goes on, two or three finalists are developed and
    /// asked about again, or none goes on and new options are made. None when it is no valid choice.
    pub fn choice(&self, q: &Question) -> Option<Choice> {
        let mut going: Vec<Verdict> = q.items.iter().map(|x| self.verdict(&x.id)).filter(|v| *v != Verdict::Drop).collect();
        let judged = q.items.iter().all(|x| matches!(self.verdict(&x.id), Verdict::Keep | Verdict::Change { .. } | Verdict::Drop));
        // what comes back goes on too, and the agent makes it again, as it was or as the person writes
        going.extend(self.back.values().map(|words| Verdict::Change { text: words.clone() }));
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
        (q.choose && self.choice(q) == Some(Choice::One) && self.sends_back() && !changes && self.back.is_empty()).then(|| q.items.iter().filter(|x| self.verdict(&x.id) == Verdict::Keep).map(|x| x.id.clone()).collect())
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
