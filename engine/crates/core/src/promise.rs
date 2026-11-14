//! A role's promises about its own method, proved by exploring every way a run of the role can go.
//!
//! The search runs the real core ([`decide`]): from each state it feeds every outside event the run could meet,
//! and checks every promise on each event the core records. Only outcomes are explored, never the content of an
//! agent's work: an attempt delivers (a small or a large change), fails, says it cannot be done, or is cut off; a
//! check passes, fails or cannot judge; the person keeps, sends back or drops; an agent lets a release go on;
//! Jaynes crashes. A run starts at low or at high risk, and with its sampled stops asking or skipping.
//!
//! A state already seen is not explored again. Bounds keep the search finite: a step's attempts and fixes (the
//! role's own), at most [`SEND_BACKS`] send-backs per step, and at most [`TROUBLE`] mishaps per run (a crash, a cut
//! off, a fault, a refusal, a check that cannot judge, a step the agent says cannot be done), and the person
//! continues a blocked run at most [`CONTINUES`] times. Usage is never
//! counted, so the ceiling is never met: what the ceiling does is the same for every role (G5). The person answers
//! between the run's moves, not in the middle of an attempt, and a run's risk is the one it starts at or a rule
//! raises it to.
//!
//! Promises take three forms, no more until a real role needs another:
//!
//! - never EVENT unless CONDITION: whenever EVENT happens, CONDITION holds in the state just before it;
//! - EVENT only after CONDITION: CONDITION held at some point before EVENT;
//! - the agent of STEP never sees STEP: no turn of the first is given the second's output, nor goes on in a session
//!   that was.
//!
//! Events: `merge`, `deliver`, `step NAME starts`. Conditions: `person answered at NAME` (the step's question of its
//! current round was answered by the person), `check NAME of STEP passed` (the step is done and that check passed on
//! its output), `step NAME done`.

use crate::attention::{coin, stop_key, Count};
use crate::decide::decide;
use crate::digest::Digest;
use crate::event::{Event, Usage};
use crate::io::{AgentTurn, AttemptOutcome, CheckResult, CheckVerdict, Command, Input};
use crate::manifest::{DeliveryKind, Executor, Manifest, ProjectAccess, SessionKind};
use crate::question::{Answer, By, Item, QuestionKind, Verdict, VerdictKind};
use crate::risk::{Changed, Risk};
use crate::run::{Run, Status};
use std::collections::{BTreeMap, BTreeSet};

/// Send-backs explored per step.
pub const SEND_BACKS: u32 = 1;
/// Mishaps explored per run.
pub const TROUBLE: u32 = 1;
/// Times the person continues a blocked run, per run: each gives the step its attempts again.
pub const CONTINUES: u32 = 1;
/// States past which the search gives up, saying so: a promise is never called proved on a search cut short.
pub const MOST_STATES: usize = 2_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum On {
    Merge,
    Deliver,
    StepStarts(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Condition {
    PersonAnswered(String),
    CheckPassed { check: String, step: String },
    StepDone(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Form {
    Never { on: On, unless: Condition },
    OnlyAfter { on: On, after: Condition },
    NeverSees { agent_of: String, step: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Promise {
    /// The promise in the author's words, as the person reads it.
    pub says: String,
    pub form: Form,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Proof {
    Proved { states: usize },
    /// The shortest way a run breaks it, move by move.
    Broken { path: Vec<String> },
    /// The search met its bound before it ended.
    Unfinished { states: usize },
}

impl On {
    pub fn parse(text: &str, m: &Manifest) -> Result<On, String> {
        let t = text.trim();
        match t {
            "merge" => {
                if m.delivery != DeliveryKind::Merge {
                    return Err("the role delivers a branch, so it never merges: set delivery = \"merge\" or promise about deliver".into());
                }
                Ok(On::Merge)
            }
            "deliver" => Ok(On::Deliver),
            _ => {
                let name = t.strip_prefix("step ").and_then(|r| r.strip_suffix(" starts")).map(str::trim);
                match name {
                    Some(n) if m.index(n).is_some() => Ok(On::StepStarts(n.to_string())),
                    Some(n) => Err(format!("{n} is not a step of the role")),
                    None => Err(format!("{t:?} is not an event: merge, deliver, or step NAME starts")),
                }
            }
        }
    }

    fn says(&self) -> String {
        match self {
            On::Merge => "merge".into(),
            On::Deliver => "deliver".into(),
            On::StepStarts(s) => format!("{s} starts"),
        }
    }
}

impl Condition {
    pub fn parse(text: &str, m: &Manifest) -> Result<Condition, String> {
        let t = text.trim();
        let step = |n: &str| -> Result<String, String> {
            m.index(n.trim()).map(|_| n.trim().to_string()).ok_or_else(|| format!("{} is not a step of the role", n.trim()))
        };
        if let Some(n) = t.strip_prefix("person answered at ") {
            let s = step(n)?;
            let i = m.index(&s).unwrap_or(0);
            if m.steps[i].stop.is_none() {
                return Err(format!("{s} has no stop, so the person is never asked there"));
            }
            return Ok(Condition::PersonAnswered(s));
        }
        if let Some(rest) = t.strip_prefix("check ").and_then(|r| r.strip_suffix(" passed"))
            && let Some((check, of)) = rest.split_once(" of ")
        {
            let s = step(of)?;
            let i = m.index(&s).unwrap_or(0);
            if !m.steps[i].checks.iter().any(|c| c.name == check.trim()) {
                return Err(format!("{s} has no check {}", check.trim()));
            }
            return Ok(Condition::CheckPassed { check: check.trim().to_string(), step: s });
        }
        if let Some(n) = t.strip_prefix("step ").and_then(|r| r.strip_suffix(" done")) {
            return Ok(Condition::StepDone(step(n)?));
        }
        Err(format!("{t:?} is not a condition: person answered at STEP, check NAME of STEP passed, or step STEP done"))
    }

    fn holds(&self, r: &Run) -> bool {
        let at = |s: &str| r.manifest.index(s);
        match self {
            Condition::PersonAnswered(s) => at(s).is_some_and(|i| {
                r.steps[i].question.as_ref().and_then(|q| r.questions.get(q)).is_some_and(|q| {
                    !q.superseded && q.question.round == r.steps[i].round && q.answer.as_ref().is_some_and(|a| a.by.person())
                })
            }),
            Condition::CheckPassed { check, step } => at(step).is_some_and(|i| {
                r.steps[i].status == Status::Done && r.steps[i].checks.iter().any(|c| &c.check == check && c.passed)
            }),
            Condition::StepDone(s) => at(s).is_some_and(|i| r.steps[i].status == Status::Done),
        }
    }

    fn says(&self) -> String {
        match self {
            Condition::PersonAnswered(s) => format!("with no answer of yours at {s}"),
            Condition::CheckPassed { check, step } => format!("before the check {check} of {step} passed"),
            Condition::StepDone(s) => format!("before {s} was done"),
        }
    }
}

impl Form {
    /// Check the steps it names: the agent of a step that is an agent's, a step that exists.
    pub fn never_sees(agent_of: &str, step: &str, m: &Manifest) -> Result<Form, String> {
        let a = m.index(agent_of).ok_or_else(|| format!("{agent_of} is not a step of the role"))?;
        m.index(step).ok_or_else(|| format!("{step} is not a step of the role"))?;
        if m.steps[a].executor != Executor::Agent {
            return Err(format!("{agent_of} is not an agent's step"));
        }
        Ok(Form::NeverSees { agent_of: agent_of.to_string(), step: step.to_string() })
    }
}

// --- the search ----------------------------------------------------------------------------------------------

#[derive(Clone)]
struct Node {
    run: Run,
    /// What the shell was told to do and has not reported on.
    pending: Vec<Command>,
    trouble: u32,
    continues: u32,
    sent_back: BTreeMap<String, u32>,
    /// For each promise of the form "only after": whether its condition has held.
    held: Vec<bool>,
}

impl Node {
    /// What tells two states apart for what can still happen: the run without what is the same in every state of
    /// the search or only counts the past.
    fn key(&self) -> u128 {
        #[derive(serde::Serialize)]
        struct View<'a> {
            id: &'a str,
            risk: Risk,
            ceiling: &'a crate::risk::Ceiling,
            spent: &'a Usage,
            steps: Vec<crate::run::StepState>,
            questions: &'a BTreeMap<String, crate::run::QuestionState>,
            phase: &'a crate::run::Phase,
            halted: bool,
            delivering: &'a Option<Digest>,
            delivered: &'a Option<Digest>,
            undone: bool,
            context: &'a [(String, String)],
            attention: &'a BTreeMap<String, Count>,
            pending: Vec<Command>,
            trouble: u32,
            continues: u32,
            sent_back: &'a BTreeMap<String, u32>,
            held: &'a [bool],
        }
        let r = &self.run;
        // attempt numbers are only ever matched against the attempt under way
        let steps = r
            .steps
            .iter()
            .map(|s| {
                let mut s = s.clone();
                s.attempt = 0;
                s.status = match s.status {
                    Status::Running { fix, .. } => Status::Running { attempt: 0, fix },
                    Status::Checking { fix, .. } => Status::Checking { attempt: 0, fix },
                    other => other,
                };
                s
            })
            .collect();
        let pending = self
            .pending
            .iter()
            .map(|c| match c.clone() {
                Command::RunAgent(mut t) => {
                    t.attempt = 0;
                    Command::RunAgent(t)
                }
                Command::RunRole { step, round, given, previous, answers, key, .. } => Command::RunRole { step, round, attempt: 0, given, previous, answers, key },
                Command::RunChecks { step, fix, output, given, checks, .. } => Command::RunChecks { step, attempt: 0, fix, output, given, checks },
                other => other,
            })
            .collect();
        let view = View {
            id: &r.id,
            risk: r.risk,
            ceiling: &r.ceiling,
            spent: &r.spent,
            steps,
            questions: &r.questions,
            phase: &r.phase,
            halted: r.halted,
            delivering: &r.delivering,
            delivered: &r.delivered,
            undone: r.undone,
            context: &r.context,
            attention: &r.attention,
            pending,
            trouble: self.trouble,
            continues: self.continues,
            sent_back: &self.sent_back,
            held: &self.held,
        };
        // 128 bits of two keyed hashes: a collision among millions of states is out of reach, and a fingerprint
        // of the state is not needed outside the search
        let bytes = serde_json::to_vec(&view).unwrap_or_default();
        let half = |salt: u8| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            salt.hash(&mut h);
            bytes.hash(&mut h);
            h.finish()
        };
        (u128::from(half(1)) << 64) | u128::from(half(2))
    }
}

/// A move from outside, and how a person reads it on a path.
struct Move {
    input: Input,
    says: String,
    trouble: bool,
    /// The step sent back, to count it.
    sends_back: Option<String>,
    /// Which pending command it answers, removed from the node.
    answers: Option<usize>,
}

/// An output, by what it stands for: the step, its round, and the kind of change. Two attempts of a round that
/// deliver the same kind of change deliver the same output, so their runs meet again.
fn output(step: &str, round: u32, what: &str) -> (Digest, Vec<Item>) {
    let d = Digest(format!("{step}.{round}.{what}"));
    (d.clone(), vec![Item { id: "output".into(), label: format!("what {step} made"), digest: d }])
}

fn agent_moves(turn: &AgentTurn, round: u32, risk: Risk, k: usize) -> Vec<Move> {
    let step = &turn.step;
    let mut out = Vec::new();
    let delivered = |what: &str, changed: Vec<Changed>| {
        let (d, items) = output(step, round, what);
        AttemptOutcome::Delivered { output: d, items, changed, usage: Usage::default(), session: Some(format!("session-{step}")) }
    };
    let ended = |outcome| Input::AttemptEnded { step: step.clone(), attempt: turn.attempt, fix: turn.fix, outcome };
    let mv = |input, says: String, trouble| Move { input, says, trouble, sends_back: None, answers: Some(k) };
    if turn.reach.project == ProjectAccess::Write {
        out.push(mv(ended(delivered("small", vec![Changed { path: "src/a".into(), lines: 1 }])), format!("{step} delivers a small change"), false));
        // a large change only raises the risk, so it is another way only below high risk
        if risk == Risk::Low {
            out.push(mv(ended(delivered("large", vec![Changed { path: "src/a".into(), lines: 80 }])), format!("{step} delivers a large change"), false));
        }
    } else {
        out.push(mv(ended(delivered("output", Vec::new())), format!("{step} delivers"), false));
    }
    out.push(mv(ended(AttemptOutcome::Failed { reasons: vec!["no output".into()], usage: Usage::default() }), format!("{step}'s attempt fails"), false));
    out.push(mv(ended(AttemptOutcome::Impossible { reason: "it cannot be done".into(), usage: Usage::default(), session: None }), format!("{step}'s agent says it cannot be done"), true));
    out.push(mv(ended(AttemptOutcome::Interrupted { reason: "cut off".into() }), format!("{step}'s agent is cut off"), true));
    out.push(mv(ended(AttemptOutcome::Fault { reason: "the agent did not start".into() }), format!("{step}'s agent does not start"), true));
    out
}

fn moves(node: &Node, named: &BTreeSet<String>) -> Vec<Move> {
    let r = &node.run;
    let node_names = named;
    let mut out = Vec::new();
    for (k, c) in node.pending.iter().enumerate() {
        let mv = |input, says: String, trouble| Move { input, says, trouble, sends_back: None, answers: Some(k) };
        match c {
            Command::Probe { step, .. } => {
                out.push(mv(Input::ProbeEnded { step: step.clone(), ok: true, reason: String::new() }, format!("{step}'s reach is tested"), false));
                out.push(mv(Input::ProbeEnded { step: step.clone(), ok: false, reason: "the box refused".into() }, format!("{step}'s reach is refused"), true));
            }
            Command::RunAgent(turn) => {
                let round = r.manifest.index(&turn.step).map(|i| r.steps[i].round).unwrap_or(0);
                out.extend(agent_moves(turn, round, r.risk, k));
            }
            Command::RunRole { step, round, attempt, .. } => {
                let (d, items) = output(step, *round, "role");
                let ended = |outcome| Input::AttemptEnded { step: step.clone(), attempt: *attempt, fix: 0, outcome };
                out.push(mv(ended(AttemptOutcome::Delivered { output: d, items, changed: Vec::new(), usage: Usage::default(), session: None }), format!("{step} done"), false));
                out.push(mv(ended(AttemptOutcome::Refused { reason: "refused".into() }), format!("{step} refuses"), true));
                out.push(mv(ended(AttemptOutcome::Fault { reason: "the role crashed".into() }), format!("{step}'s role program crashes"), true));
            }
            Command::RunChecks { step, attempt, fix, checks, .. } => {
                let all = |v: &dyn Fn(usize) -> CheckVerdict| checks.iter().enumerate().map(|(n, c)| CheckResult { check: c.clone(), verdict: v(n) }).collect::<Vec<_>>();
                let ended = |results| Input::ChecksEnded { step: step.clone(), attempt: *attempt, fix: *fix, results };
                out.push(mv(ended(all(&|_| CheckVerdict::Pass)), format!("{step}'s checks pass"), false));
                // A check fails the same way as any other of its kind (open: fixed in place; hidden: an attempt spent),
                // so one of each kind fails, and every check a promise names.
                let i = r.manifest.index(step);
                let kind = |name: &String| i.and_then(|i| r.manifest.steps[i].checks.iter().find(|c| &c.name == name)).map(|c| c.kind);
                let mut kinds = BTreeSet::new();
                for (n, name) in checks.iter().enumerate() {
                    let named = node_names.contains(name.as_str());
                    if !kinds.insert(format!("{:?}", kind(name))) && !named {
                        continue;
                    }
                    let fails = all(&|j| if j == n { CheckVerdict::Fail { reasons: vec!["it does not hold".into()], detail: String::new() } } else { CheckVerdict::Pass });
                    out.push(mv(ended(fails), format!("{step}'s check {name} fails"), false));
                }
                if let Some(first) = checks.first() {
                    let blocked = all(&|j| if j == 0 { CheckVerdict::Blocked { reason: "cannot judge".into() } } else { CheckVerdict::Pass });
                    out.push(mv(ended(blocked), format!("{step}'s check {first} cannot judge"), true));
                }
            }
            Command::Deliver { digest, merge, .. } => {
                let what = if *merge { "merged" } else { "delivered" };
                out.push(mv(Input::DeliveryEnded { digest: digest.clone(), ok: true, reference: String::new() }, what.to_string(), false));
                out.push(mv(Input::DeliveryEnded { digest: digest.clone(), ok: false, reference: String::new() }, format!("not {what}: it fails"), true));
            }
            Command::Ask { .. } | Command::Permit { .. } | Command::Notify { .. } => {}
        }
    }
    // The person, or an agent, answers an open question, and Jaynes crashes, between the run's moves: an answer
    // in the middle of an attempt is the same answer at its end, with the attempt's report recorded as late.
    let working = node.pending.iter().any(|c| matches!(c, Command::RunAgent(_) | Command::RunRole { .. } | Command::RunChecks { .. } | Command::Probe { .. }));
    if working {
        out.push(Move { input: Input::Recovered, says: "Jaynes crashes and starts again".into(), trouble: true, sends_back: None, answers: None });
        return out;
    }
    for q in r.open_questions() {
        let q = &q.question;
        if q.kind != QuestionKind::Decision {
            continue;
        }
        // keeping and skipping go on with the output; changing, redoing and dropping all send the step back
        let keep = q.offered.iter().find(|k| matches!(k, VerdictKind::Keep | VerdictKind::Skip));
        let back = q.offered.iter().find(|k| matches!(k, VerdictKind::Redo | VerdictKind::Change | VerdictKind::Drop));
        for kind in keep.into_iter().chain(back) {
            let verdict = match kind {
                VerdictKind::Keep => Verdict::Keep,
                VerdictKind::Change => Verdict::Change { text: "change it".into() },
                VerdictKind::Redo => Verdict::Redo { note: "again".into() },
                VerdictKind::Drop => Verdict::Drop,
                VerdictKind::Skip => Verdict::Skip,
            };
            let back = verdict.sends_back();
            let verdicts = q.items.iter().map(|x| (x.id.clone(), verdict.clone())).collect();
            let word = if back { "send back" } else { "keep" };
            out.push(Move {
                input: Input::Answer { answer: Answer { question: q.id.clone(), shown: q.shown(), verdicts, by: By::Person, context: String::new() } },
                says: format!("you {word} {}", q.step),
                trouble: false,
                sends_back: back.then(|| q.step.clone()),
                answers: None,
            });
        }
        out.push(Move {
            input: Input::Answer { answer: Answer { question: q.id.clone(), shown: q.shown(), verdicts: BTreeMap::new(), by: By::Agent { who: "an agent".into() }, context: String::new() } },
            says: format!("an agent lets {} go on", q.step),
            trouble: false,
            sends_back: None,
            answers: None,
        });
    }
    if r.blocked() && node.continues < CONTINUES {
        out.push(Move { input: Input::Continue { by: By::Person }, says: "you continue".into(), trouble: false, sends_back: None, answers: None });
    }
    out
}

/// What the path says of an event the core recorded, when the person needs it to follow.
fn said_of(e: &Event, pre: &Run) -> Option<String> {
    match e {
        Event::RiskRaised { to, by, .. } if !by.person() => Some(format!("risk raised to {}", to.word())),
        Event::StopSkipped { step, .. } => Some(format!("{step}'s stop is certified and skips asking")),
        Event::QuestionOpened { question } if question.kind == QuestionKind::Decision => Some(format!("{} asks you", question.step)),
        Event::StepDone { step, .. } => {
            let i = pre.manifest.index(step)?;
            let stop = pre.manifest.steps[i].stop.as_ref()?;
            (stop.from_risk > pre.risk).then(|| format!("{step}'s stop asks only from {} risk", stop.from_risk.word()))
        }
        _ => None,
    }
}

/// The events a recorded event stands for.
fn happened(e: &Event, pre: &Run) -> Vec<On> {
    match e {
        Event::AttemptStarted { step, fix: 0, .. } => vec![On::StepStarts(step.clone())],
        Event::QuestionOpened { question } if question.kind == QuestionKind::Decision => pre
            .manifest
            .index(&question.step)
            .filter(|&i| pre.manifest.steps[i].executor == Executor::Person && pre.steps[i].question.is_none())
            .map(|_| vec![On::StepStarts(question.step.clone())])
            .unwrap_or_default(),
        Event::DeliveryAsked { .. } => {
            let mut v = vec![On::Deliver];
            if pre.manifest.delivery == DeliveryKind::Merge && !pre.undone {
                v.push(On::Merge);
            }
            v
        }
        _ => Vec::new(),
    }
}

/// Whether the agent of `a` goes on in a session that was given `b`'s output: its step's session chain.
fn session_sees(m: &Manifest, a: usize, b: &str) -> Option<String> {
    let mut i = a;
    while m.steps[i].session == SessionKind::Continue {
        let before = m.steps[..i].iter().rposition(|s| s.executor == Executor::Agent)?;
        if m.steps[before].needs.iter().any(|n| n == b) {
            return Some(m.steps[before].name.clone());
        }
        i = before;
    }
    None
}

/// Each node's parent and the move that reached it, to tell a path.
type Trail = Vec<(Option<usize>, String)>;

fn path(trail: &Trail, mut at: usize) -> Vec<String> {
    let mut out = Vec::new();
    loop {
        let (parent, says) = &trail[at];
        if !says.is_empty() {
            out.push(says.clone());
        }
        match parent {
            Some(p) => at = *p,
            None => break,
        }
    }
    out.reverse();
    out
}

/// A child of a node: the node, the move as the path tells it, and the promises it breaks, each with the end of
/// its path.
type Child = (Node, String, Vec<(usize, Vec<String>)>);

/// Apply a move to `node`, or None when the core refuses it. Every promise not yet broken (`done`) is checked on
/// each event on the way, and on each agent turn the core asks for.
fn apply(promises: &[Promise], done: &[bool], node: &Node, mv: &Move) -> Option<Child> {
    let decided = decide(Some(&node.run), &mv.input).ok()?;
    let mut says = vec![mv.says.clone()];
    let mut breaks = Vec::new();
    let mut run = node.run.clone();
    let mut held = node.held.clone();
    for e in &decided.events {
        for on in happened(e, &run) {
            for (p, promise) in promises.iter().enumerate() {
                if done[p] {
                    continue;
                }
                let (want, ok, cond) = match &promise.form {
                    Form::Never { on: o, unless } => (o, unless.holds(&run), unless),
                    Form::OnlyAfter { on: o, after } => (o, held[p] || after.holds(&run), after),
                    Form::NeverSees { .. } => continue,
                };
                if *want == on && !ok {
                    let mut tail = says.clone();
                    tail.push(format!("{}, {}", on.says(), cond.says()));
                    breaks.push((p, tail));
                }
            }
        }
        if let Some(s) = said_of(e, &run) {
            says.push(s);
        }
        run.apply(e);
        for (p, promise) in promises.iter().enumerate() {
            if let Form::OnlyAfter { after, .. } = &promise.form
                && after.holds(&run)
            {
                held[p] = true;
            }
        }
    }
    for c in &decided.commands {
        let Command::RunAgent(turn) = c else { continue };
        for (p, promise) in promises.iter().enumerate() {
            let Form::NeverSees { agent_of, step } = &promise.form else { continue };
            if done[p] || &turn.step != agent_of {
                continue;
            }
            let given = turn.given.iter().any(|g| &g.step == step);
            let session = run.manifest.index(agent_of).and_then(|a| session_sees(&run.manifest, a, step));
            if given || session.is_some() {
                let mut tail = says.clone();
                tail.push(match session {
                    Some(s) => format!("the agent of {agent_of} goes on in the session of {s}, which was given {step}"),
                    None => format!("the agent of {agent_of} is given {step}'s output"),
                });
                breaks.push((p, tail));
            }
        }
    }
    let mut pending = node.pending.clone();
    if let Some(k) = mv.answers {
        pending.remove(k);
    }
    if matches!(mv.input, Input::Recovered) {
        pending.clear();
    }
    pending.extend(decided.commands.into_iter().filter(|c| !matches!(c, Command::Ask { .. } | Command::Permit { .. } | Command::Notify { .. })));
    let mut sent_back = node.sent_back.clone();
    if let Some(s) = &mv.sends_back {
        *sent_back.entry(s.clone()).or_default() += 1;
    }
    let continues = node.continues + u32::from(matches!(mv.input, Input::Continue { .. }));
    let child = Node { run, pending, trouble: node.trouble + u32::from(mv.trouble), continues, sent_back, held };
    Some((child, says.join(" → "), breaks))
}

/// Every child of the nodes of `part`, with its key.
fn expand(part: &[(Node, usize)], promises: &[Promise], done: &[bool], named: &BTreeSet<String>) -> Vec<(usize, Child, u128)> {
    let mut out = Vec::new();
    for (node, at) in part {
        if node.run.finished() {
            continue;
        }
        for mv in moves(node, named) {
            if mv.trouble && node.trouble >= TROUBLE {
                continue;
            }
            if mv.sends_back.as_ref().is_some_and(|s| node.sent_back.get(s).copied().unwrap_or(0) >= SEND_BACKS) {
                continue;
            }
            if let Some(child) = apply(promises, done, node, &mv) {
                let key = child.0.key();
                out.push((*at, child, key));
            }
        }
    }
    out
}

/// The runs a search starts from: at low and at high risk, and, when a stop may ask only a sample, once with every
/// such stop certified on a run whose draws skip asking.
fn starts(m: &Manifest) -> Vec<(Input, String)> {
    let fingerprint = Digest("promise".into());
    let sampled: Vec<usize> = (0..m.steps.len()).filter(|&i| m.steps[i].stop.as_ref().is_some_and(|s| s.budget.is_some())).collect();
    let mut worlds: Vec<(String, BTreeMap<String, Count>, String)> = vec![("promise".into(), BTreeMap::new(), String::new())];
    if !sampled.is_empty() {
        let id = (0..10_000)
            .map(|n| format!("promise-{n}"))
            .find(|id| {
                sampled.iter().all(|&i| {
                    let audit = m.steps[i].stop.as_ref().map(|s| s.audit_rate.0).unwrap_or(1.0);
                    (0..=SEND_BACKS + 1).all(|round| coin(id, &m.steps[i].name, round) >= audit)
                })
            })
            .unwrap_or_else(|| "promise".into());
        let attention = sampled
            .iter()
            .map(|&i| (m.steps[i].name.clone(), Count { key: stop_key(m, &fingerprint, i), n: 1000, k: 0, pending: 0, braked: false }))
            .collect();
        let names: Vec<&str> = sampled.iter().map(|&i| m.steps[i].name.as_str()).collect();
        worlds.push((id, attention, format!(", {} certified so a run may skip asking", names.join(" and "))));
    }
    let mut out = Vec::new();
    for (id, attention, note) in worlds {
        for raise in [None, Some(Risk::High)] {
            let says = format!("{} risk{note}", if raise.is_some() { "high" } else { "low" });
            out.push((
                Input::Start { run: id.clone(), request: "a request".into(), manifest: m.clone(), role_fingerprint: fingerprint.clone(), agent: "agent".into(), raise, attention: attention.clone() },
                says,
            ));
        }
    }
    out
}

/// Prove each promise of the role in `m`: every way a run can go, within the bounds, keeps it, or the shortest
/// path that breaks it.
pub fn prove(m: &Manifest, promises: &[Promise]) -> Vec<Proof> {
    prove_within(m, promises, MOST_STATES)
}

pub fn prove_within(m: &Manifest, promises: &[Promise], most: usize) -> Vec<Proof> {
    let named: BTreeSet<String> = promises
        .iter()
        .filter_map(|p| match &p.form {
            Form::Never { unless: Condition::CheckPassed { check, .. }, .. } | Form::OnlyAfter { after: Condition::CheckPassed { check, .. }, .. } => Some(check.clone()),
            _ => None,
        })
        .collect();
    let mut broken: Vec<Option<Vec<String>>> = vec![None; promises.len()];
    let mut trail: Trail = Vec::new();
    let mut seen: std::collections::HashSet<u128> = std::collections::HashSet::new();
    let mut frontier: Vec<(Node, usize)> = Vec::new();
    for (start, says) in starts(m) {
        let Ok(decided) = decide(None, &start) else { continue };
        let Some(mut run) = Run::fold(&decided.events) else { continue };
        run.seq = decided.events.len() as u64;
        let held = promises.iter().map(|p| matches!(&p.form, Form::OnlyAfter { after, .. } if after.holds(&run))).collect();
        let pending = decided.commands.into_iter().filter(|c| !matches!(c, Command::Ask { .. } | Command::Permit { .. } | Command::Notify { .. })).collect();
        let node = Node { run, pending, trouble: 0, continues: 0, sent_back: BTreeMap::new(), held };
        if seen.insert(node.key()) {
            trail.push((None, says));
            frontier.push((node, trail.len() - 1));
        }
    }
    // Breadth first, a level at a time, each level's nodes shared among threads and their children taken in order:
    // the first path found that breaks a promise is among the shortest, and the same on every machine.
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 16);
    let mut cut = false;
    while !frontier.is_empty() && !broken.iter().all(Option::is_some) {
        if seen.len() >= most {
            cut = true;
            break;
        }
        let done: Vec<bool> = broken.iter().map(Option::is_some).collect();
        let size = frontier.len().div_ceil(threads).max(1);
        let parts: Vec<Option<Vec<(usize, Child, u128)>>> = std::thread::scope(|s| {
            let handles: Vec<_> = frontier.chunks(size).map(|part| s.spawn(|| expand(part, promises, &done, &named))).collect();
            handles.into_iter().map(|h| h.join().ok()).collect()
        });
        let mut next = Vec::new();
        for part in parts {
            // a thread that died leaves states unexplored: nothing may be called proved
            let Some(part) = part else {
                cut = true;
                continue;
            };
            for (at, (child, says, breaks), key) in part {
                for (p, tail) in breaks {
                    if broken[p].is_none() {
                        let mut whole = path(&trail, at);
                        whole.extend(tail);
                        broken[p] = Some(whole);
                    }
                }
                if seen.insert(key) {
                    trail.push((Some(at), says));
                    next.push((child, trail.len() - 1));
                }
            }
        }
        frontier = next;
    }
    let states = seen.len();
    broken
        .into_iter()
        .map(|b| match b {
            Some(path) => Proof::Broken { path },
            None if cut => Proof::Unfinished { states },
            None => Proof::Proved { states },
        })
        .collect()
}
