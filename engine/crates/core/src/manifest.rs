//! A role as data: its steps, what each needs, who runs it, what it may reach, what it does to the world, its
//! checks and its stop. The role program sends it in answer to `describe`; the engine owns the order of the steps.
//!
//! No expressions live here: a condition is the name of a check the role program runs.

use crate::question::VerdictKind;
use crate::risk::Risk;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Manifest {
    pub role: String,
    pub version: String,
    /// The major version of the protocol the role speaks.
    pub protocol: u32,
    pub steps: Vec<StepDecl>,
    /// How the run delivers: a branch, or a merge into the branch the project had checked out when the run started.
    #[serde(default)]
    pub delivery: DeliveryKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryKind {
    #[default]
    Branch,
    /// Merged by Jaynes, never pushed, and undone by the person with Undo.
    Merge,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StepDecl {
    pub name: String,
    #[serde(default)]
    pub needs: Vec<String>,
    pub executor: Executor,
    /// Whether an agent step goes on in the session of the agent step before it, so one agent holds several steps.
    #[serde(default)]
    pub session: SessionKind,
    #[serde(default)]
    pub reach: Reach,
    #[serde(default)]
    pub effect: Effect,
    #[serde(default)]
    pub checks: Vec<CheckDecl>,
    #[serde(default)]
    pub stop: Option<StopDecl>,
    /// The lowest risk at which the step runs: a step from `high` is left out of the short path.
    #[serde(default)]
    pub from_risk: Risk,
    /// Attempts the agent may spend in one round. Only the agent's faults spend one.
    #[serde(default = "three")]
    pub attempts: u32,
    /// Times a failed shape check goes back to the same agent session to be fixed in place, per attempt.
    #[serde(default = "two")]
    pub fixes: u32,
    /// Proof the maker never sees: the step reads only the request, and no agent step reads it.
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub about: String,
}

/// A rate in (0, 1), compared by its bits so a role stays comparable.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct Rate(pub f64);

impl PartialEq for Rate {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}
impl Eq for Rate {}

fn tenth() -> Rate {
    Rate(0.1)
}

fn three() -> u32 {
    3
}
fn two() -> u32 {
    2
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Executor {
    /// An agent program, such as Claude Code, does the step.
    Agent,
    /// The role program does the step itself, for evidence an agent could fake.
    Role,
    /// The person: the step is a question over the steps it needs, and its output is the answer.
    Person,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    #[default]
    Fresh,
    Continue,
}

/// What a step may reach. Jaynes turns it into the agent program's own settings for the step.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Reach {
    #[serde(default)]
    pub project: ProjectAccess,
    /// Project paths a writing step may change; `!` keeps paths out.
    #[serde(default)]
    pub write: Vec<String>,
    #[serde(default)]
    pub network: Network,
    /// Whether the step may run shell commands.
    #[serde(default)]
    pub commands: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectAccess {
    #[default]
    None,
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Network {
    #[default]
    None,
    Web,
}

/// What the step does to the world, by where its effect lands.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Effect {
    /// Nothing outside the step's own output.
    #[default]
    None,
    /// Changes only Jaynes's copies, which are thrown away or kept by Jaynes.
    Confined,
    /// Changes something real that a named operation undoes, such as a local merge.
    Compensable { undo: String },
    /// Cannot be undone: a push, a message to a person, a publication.
    Irreversible {
        #[serde(default)]
        idempotent: bool,
    },
}

impl Effect {
    pub fn irreversible(&self) -> bool {
        matches!(self, Effect::Irreversible { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CheckDecl {
    pub name: String,
    pub kind: CheckKind,
    /// What the check holds the output to. For an open check the agent reads it before it delivers.
    #[serde(default)]
    pub about: String,
    /// Earlier steps the check reads besides what its step uses, such as the hidden cases a proof runs. Only a
    /// hidden check may read a hidden step.
    #[serde(default)]
    pub reads: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    /// The agent knows it before it delivers, and a failure goes back to the same session to be fixed in place:
    /// the form of the output (a field, a length, a file that must exist), or the project's own tests, which the
    /// agent can run itself.
    Open,
    /// Proof the maker never sees: not its name, not its inputs. A failure tells the agent only the check's own
    /// sentence, and spends the attempt.
    Hidden,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StopDecl {
    /// The verdicts the person may give each item, from the closed list.
    pub verdicts: Vec<VerdictKind>,
    /// Whether the steps after it wait for the answer. Otherwise the run goes on and the answer corrects it later.
    #[serde(default)]
    pub holds: bool,
    /// The lowest risk at which the stop asks.
    #[serde(default)]
    pub from_risk: Risk,
    /// The stop only asks whether a result may go on, not a matter of taste or direction: an agent may let it
    /// go on, recorded as the agent's, and the person can still change that.
    #[serde(default)]
    pub release: bool,
    /// The error rate the stop may have once it asks only a sample. None: it always asks.
    #[serde(default)]
    pub budget: Option<Rate>,
    /// The share of runs a certified stop still asks.
    #[serde(default = "tenth")]
    pub audit_rate: Rate,
    #[serde(default)]
    pub about: String,
}

impl Manifest {
    pub fn index(&self, name: &str) -> Option<usize> {
        self.steps.iter().position(|s| s.name == name)
    }

    /// The steps a run at `risk` takes, in order.
    pub fn path(&self, risk: Risk) -> Vec<usize> {
        (0..self.steps.len()).filter(|&i| self.steps[i].from_risk <= risk).collect()
    }

    /// Every rule a role must keep, each broken one as a sentence naming the step.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.role.trim().is_empty() {
            errors.push("the role has no name".to_string());
        }
        if self.steps.is_empty() {
            errors.push("the role has no step".to_string());
        }
        if self.protocol != crate::PROTOCOL {
            errors.push(format!("the role speaks protocol {}, and this engine speaks {}", self.protocol, crate::PROTOCOL));
        }
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let first_writer = self.steps.iter().position(|s| s.reach.project == ProjectAccess::Write);
        for (i, s) in self.steps.iter().enumerate() {
            let at = format!("step {}", s.name);
            if s.name.trim().is_empty() || s.name.contains('/') {
                errors.push(format!("step {i} has no usable name"));
            }
            if !seen.insert(&s.name) {
                errors.push(format!("{at} is defined twice"));
            }
            for n in &s.needs {
                match self.index(n) {
                    Some(j) if j < i => {
                        let need = &self.steps[j];
                        if need.from_risk > s.from_risk {
                            errors.push(format!("{at} needs {n}, which a run at its risk does not take"));
                        }
                        if need.hidden && s.executor == Executor::Agent {
                            errors.push(format!("{at} is an agent's and needs {n}, which is hidden from agents"));
                        }
                    }
                    Some(_) => errors.push(format!("{at} needs {n}, which comes after it")),
                    None => errors.push(format!("{at} needs {n}, which is not a step of the role")),
                }
            }
            let mut names = BTreeSet::new();
            for c in &s.checks {
                if !names.insert(&c.name) {
                    errors.push(format!("{at} has two checks named {}", c.name));
                }
                for r in &c.reads {
                    match self.index(r) {
                        Some(j) if j < i => {
                            if self.steps[j].hidden && c.kind != CheckKind::Hidden {
                                errors.push(format!("{at}: the check {} reads hidden {r}, so it must be hidden", c.name));
                            }
                            if self.steps[j].from_risk > s.from_risk {
                                errors.push(format!("{at}: the check {} reads {r}, which a run at its risk does not take", c.name));
                            }
                        }
                        _ => errors.push(format!("{at}: the check {} reads {r}, which is not an earlier step", c.name)),
                    }
                }
            }
            if s.attempts == 0 {
                errors.push(format!("{at} allows no attempt"));
            }
            match s.executor {
                Executor::Person => {
                    if s.stop.is_none() {
                        errors.push(format!("{at} is the person's and asks nothing"));
                    }
                    if s.needs.is_empty() {
                        errors.push(format!("{at} is the person's and shows nothing"));
                    }
                    if !s.checks.is_empty() || s.effect != Effect::None || s.reach != Reach::default() {
                        errors.push(format!("{at} is the person's and declares checks, an effect or a reach"));
                    }
                }
                Executor::Agent => {
                    if s.effect.irreversible() {
                        errors.push(format!("{at} gives an agent an effect that cannot be undone; only the role does that"));
                    }
                }
                Executor::Role => {}
            }
            if let Some(stop) = &s.stop {
                if stop.verdicts.is_empty() {
                    errors.push(format!("{at} has a stop with no verdict to give"));
                }
                if stop.budget.is_some_and(|b| !(b.0 > 0.0 && b.0 < 1.0)) || !(stop.audit_rate.0 > 0.0 && stop.audit_rate.0 <= 1.0) {
                    errors.push(format!("{at} has a budget or an audit rate outside (0, 1)"));
                }
                if stop.budget.is_some() && s.executor == Executor::Person {
                    errors.push(format!("{at} is the person's, and the person's steps always ask"));
                }
                if stop.release && s.executor == Executor::Person {
                    errors.push(format!("{at} is the person's, so an agent may not release it"));
                }
                if stop.release && !stop.verdicts.contains(&VerdictKind::Keep) {
                    errors.push(format!("{at} can be released and offers no keep"));
                }
                if stop.from_risk < s.from_risk {
                    errors.push(format!("{at} asks at a risk the step does not run at"));
                }
            }
            if s.session == SessionKind::Continue {
                let before = self.steps[..i].iter().rev().find(|b| b.executor == Executor::Agent);
                match before {
                    None => errors.push(format!("{at} goes on in a session, and no agent step comes before it")),
                    Some(b) if b.hidden || s.hidden => {
                        errors.push(format!("{at} would share a session with a hidden step"))
                    }
                    Some(b) if b.from_risk > s.from_risk => {
                        errors.push(format!("{at} goes on in the session of {}, which a run at its risk does not take", b.name))
                    }
                    Some(_) => {}
                }
                if s.executor != Executor::Agent {
                    errors.push(format!("{at} goes on in a session and is not an agent's"));
                }
            }
            if s.hidden {
                if s.executor != Executor::Agent {
                    errors.push(format!("{at} is hidden and is not an agent's"));
                }
                // It may read the project as it was: no step before it has changed it.
                if !s.needs.is_empty() || s.reach.project == ProjectAccess::Write {
                    errors.push(format!("{at} is hidden and reads more than the request and the project as it was"));
                }
                if first_writer.is_some_and(|w| w < i) {
                    errors.push(format!("{at} is hidden and comes after a step that changes the project"));
                }
            }
            if s.reach.project == ProjectAccess::Write && s.reach.write.is_empty() {
                errors.push(format!("{at} writes the project and names no path it may change"));
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }
}
