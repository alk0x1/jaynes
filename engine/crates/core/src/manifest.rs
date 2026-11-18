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
    /// What the run works on. A role that names none works on a repository, as every role did before targets.
    #[serde(default, skip_serializing_if = "Target::is_repository")]
    pub target: Target,
    /// How the run delivers on a repository: a branch, or a merge into the branch the project had checked out when
    /// the run started.
    #[serde(default)]
    pub delivery: DeliveryKind,
}

/// What a run works on, and so what it delivers, what its reach is given in, how its risk is judged and how its
/// delivery is undone. One engine for every kind: the core's rules are the same, only these readings change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    /// A repository: the run works in a clone of it at the commit it had, its reach is given in the repository's
    /// paths, its risk is judged by the lines and files it changes, and it delivers a branch, or a merge that Undo
    /// reverts.
    #[default]
    Repository,
}

impl Target {
    pub fn is_repository(&self) -> bool {
        *self == Target::Repository
    }

    /// The target in words, for what the person reads.
    pub fn noun(self) -> &'static str {
        match self {
            Target::Repository => "a repository",
        }
    }
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
    /// Doing the step's act again does no harm (the far side keeps the act's key, or the act sets a state rather than
    /// adding one): only then may Jaynes start it again after a fault, with the same key. Any step that acts, whatever
    /// can undo it; for an effect that cannot be undone it may also be said inside the effect, as before.
    #[serde(default, skip_serializing_if = "is_false")]
    pub idempotent: bool,
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
    /// The local ports the step's commands may connect to, such as a service on this machine the step acts on. A
    /// port is reached only when it is named here: none is open by default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub local_ports: Vec<u16>,
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
    /// The agent program's tools that read the web, such as Claude Code's WebFetch and WebSearch.
    Web,
    /// Only the registries packages are installed from (npm, Go modules, PyPI, crates), from the step's commands:
    /// the step installs what the project needs, and reaches nothing else.
    Packages,
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

    /// Whether the effect lands on something real, beyond Jaynes's own copies.
    pub fn acts(&self) -> bool {
        matches!(self, Effect::Compensable { .. } | Effect::Irreversible { .. })
    }
}

impl StepDecl {
    /// Whether doing the step's act again does no harm, said on the step or inside an effect that cannot be undone.
    pub fn repeats_safely(&self) -> bool {
        self.idempotent || matches!(self.effect, Effect::Irreversible { idempotent: true })
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
    /// The person chooses one of the step's options with a click, and may write how to adjust it. The others leave
    /// the running; without a note the run goes on with the chosen one alone, kept by Jaynes and not the agent.
    #[serde(default, skip_serializing_if = "is_false")]
    pub choose: bool,
    /// The earlier step whose work this stop asks about, when the step that asks only reviews it: what the person
    /// sends back goes to that step, and this one looks again at what comes back before the person sees it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redo: Option<String>,
    /// Kept so roles that set it still load; it limits nothing. Every round at a stop is opened by an answer of
    /// the person, who decides when to stop, so no count of them is a ceiling. Agents redoing without a new answer
    /// are what a ceiling guards against, and a step's attempts and fixes already are that ceiling: when they are
    /// spent the run is blocked and calls the person, never ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounds: Option<u32>,
}

pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}

impl Manifest {
    pub fn index(&self, name: &str) -> Option<usize> {
        self.steps.iter().position(|s| s.name == name)
    }

    /// Whether the delivery lands on the person's own thing, beyond a copy they may take or leave, so the person can
    /// undo it: on a repository, a merge into their branch.
    pub fn lands(&self) -> bool {
        match self.target {
            Target::Repository => self.delivery == DeliveryKind::Merge,
        }
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
                // A stop sends the work back to the agent that made it from an agent's step that reviews it, or from a
                // step of the role's own that shows it, such as quotes in the pages they came from; never from the
                // person's step.
                if let Some(r) = &stop.redo {
                    let asks = matches!(s.executor, Executor::Agent | Executor::Role);
                    match self.index(r) {
                        Some(j) if asks && s.needs.contains(r) && self.steps[j].executor == Executor::Agent && !self.steps[j].hidden => {}
                        _ => errors.push(format!(
                            "{at} sends back to {r}: a stop sends work back only from an agent's step or one of Jaynes's own, to an agent's step it needs that is not hidden"
                        )),
                    }
                }
                if stop.rounds == Some(0) {
                    errors.push(format!("{at} has a stop that asks no round"));
                }
                if stop.choose {
                    if s.executor != Executor::Agent {
                        errors.push(format!("{at} chooses among options, and only an agent's step makes them"));
                    }
                    if [VerdictKind::Keep, VerdictKind::Change, VerdictKind::Drop].iter().any(|v| !stop.verdicts.contains(v)) {
                        errors.push(format!("{at} chooses, so it offers keep, change and drop"));
                    }
                    if stop.release || stop.budget.is_some() {
                        errors.push(format!("{at} chooses, a matter of taste: no agent releases it and it always asks"));
                    }
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
            if !s.reach.local_ports.is_empty() && !s.reach.commands {
                errors.push(format!("{at} names local ports and runs no command that could reach them"));
            }
            if s.reach.local_ports.contains(&0) {
                errors.push(format!("{at} names the local port 0, which is no port"));
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }
}

#[cfg(test)]
mod target_tests {
    use super::*;

    const OLD: &str = r#"{"role":"r","version":"1","protocol":1,"delivery":"merge","steps":[{"name":"change","executor":"agent"}]}"#;

    #[test]
    fn a_role_that_names_no_target_works_on_a_repository_and_is_written_as_before() {
        let m: Manifest = serde_json::from_str(OLD).expect("an older role's manifest reads");
        assert_eq!(m.target, Target::Repository);
        let again = serde_json::to_value(&m).expect("json");
        assert!(again.get("target").is_none(), "a repository is the default, so the manifest is written as before");
        let named: Manifest = serde_json::from_str(&OLD.replace(r#""delivery""#, r#""target":"repository","delivery""#)).expect("named");
        assert_eq!(named, m);
    }

    #[test]
    fn a_target_this_engine_does_not_know_is_refused() {
        let other = OLD.replace(r#""delivery""#, r#""target":"a-browser-session","delivery""#);
        assert!(serde_json::from_str::<Manifest>(&other).is_err());
    }

    #[test]
    fn a_step_that_names_no_local_port_is_written_as_before_and_one_that_does_runs_commands() {
        // the key of a stop's earned record is made from its step as written, so a step from before must not change
        let m: Manifest = serde_json::from_str(OLD).expect("an older role's manifest reads");
        assert!(m.steps[0].reach.local_ports.is_empty());
        let again = serde_json::to_value(&m).expect("json");
        assert!(again["steps"][0]["reach"].get("local_ports").is_none(), "a step that names no port is written as before: {again}");
        let mut named = m.clone();
        named.steps[0].reach.local_ports = vec![5432];
        let e = named.validate().expect_err("ports without commands").join("; ");
        assert!(e.contains("step change names local ports and runs no command that could reach them"), "{e}");
        named.steps[0].reach.commands = true;
        assert_eq!(named.validate(), Ok(()));
        assert_eq!(serde_json::to_value(&named).expect("json")["steps"][0]["reach"]["local_ports"], serde_json::json!([5432]));
        named.steps[0].reach.local_ports = vec![0];
        assert!(named.validate().is_err_and(|e| e.join("; ").contains("names the local port 0, which is no port")));
    }

    #[test]
    fn a_repository_lands_on_the_person_s_branch_only_when_it_delivers_by_merge() {
        let mut m: Manifest = serde_json::from_str(OLD).expect("reads");
        assert!(m.lands());
        m.delivery = DeliveryKind::Branch;
        assert!(!m.lands());
        assert_eq!(m.target.noun(), "a repository");
    }
}
