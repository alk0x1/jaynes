//! What a finished run did, told to someone who never installed Jaynes: the role and its version, each step with the
//! checks that passed, the promises proved of the role, how it was delivered, and what it cost in agents' work and
//! in people's. A short text in Markdown, to paste in a merge request or a note.
//!
//! [`summarize`] is a pure function of the run's entries in the record, each with the time it was recorded. Two
//! things are never mixed in it: how long the run waited at its stops (the person may have been asleep) and how
//! long a person gave it. The record holds no time a person spent, only when a question opened and when it was
//! answered. A person's time is measured only from their presence on a stop's screen ([`crate::presence`]), while
//! the stop was open; a stop answered with none recorded is said as "not measured", never as zero, never as the wait.

use crate::store::Store;
use jaynes_core::event::{Event, Outcome};
use jaynes_core::manifest::{CheckKind, Executor};
use jaynes_core::question::By;
use jaynes_core::run::{Phase, Run, Status};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// One promise of the role, in the role's words, and whether it was proved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Promised {
    pub says: String,
    pub proved: bool,
}

/// The promises of the run's role, as the summary can tell them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Promises {
    /// The run's copy of the role is gone or does not read, so they could not be proved again.
    Unread,
    /// They are being proved beside the reader, who is not kept waiting: a large role takes minutes.
    Proving,
    /// Each one with whether it was proved. Empty: the role makes none.
    Told(Vec<Promised>),
}

/// One step of the run's role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepLine {
    pub name: String,
    /// Who does it: "agent", "Jaynes" or "person".
    pub by: &'static str,
    /// "done", "not done" or "left out at this risk".
    pub state: &'static str,
    pub attempts: u32,
    pub rounds: u32,
    /// The checks that passed on what the step made, last time they judged it: each name, and whether it was hidden
    /// from the agent that made it.
    pub passed: Vec<(String, bool)>,
    /// Who answered what the step asked, when it asked.
    pub asked: Option<String>,
}

/// What the run took of people.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Person {
    /// Questions a person answered. A rule's answers and an agent's are not theirs.
    pub stops: u32,
    /// Of those, the answers that sent a step back to work.
    pub sent_back: u32,
    /// What else a person did to the run: halt, resume, continue, raise, correct, undo.
    pub acts: u32,
    /// Seconds a person was on the screens of the run's stops while they were open, a second counted once. None: no
    /// presence was recorded.
    pub seconds: Option<u64>,
    /// Stops a person answered with no presence recorded at them: their time is not in `seconds`.
    pub unmeasured: u32,
    /// Seconds the run stood at a stop that holds it, until someone answered. Waiting, not attention.
    pub waited: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub run: String,
    pub request: String,
    pub role: String,
    pub version: String,
    /// The start of the fingerprint of the role's content.
    pub fingerprint: String,
    pub agent: String,
    /// The engine the run ran on, and the one that proved its role's promises again: this one.
    pub engine: String,
    pub prover: String,
    pub risk: &'static str,
    /// What the run delivered, in the target's own words for it, as recorded; None: nothing.
    pub delivered: Option<String>,
    pub abandoned: bool,
    /// A person reverted what the run merged.
    pub undone: bool,
    /// The target, in words ("a repository"), and whether it is one: what a change's size is counted in.
    pub target: &'static str,
    pub repository: bool,
    /// What the run changed, as Jaynes measured it on its copy, never as the agent said: each thing once, in the
    /// order of the steps, with how much it changed.
    pub changed: Vec<(String, u64)>,
    /// What of it is on the floor (jaynes_core::risk::FLOOR on a repository): what tools run or obey later outside any
    /// box, or what cannot be taken back, which alone makes a change risky. Each thing once, by name.
    pub floor: Vec<String>,
    /// What this run did not establish, a sentence each: what no check or no person looked at, what was not measured.
    pub unproved: Vec<String>,
    pub started: u64,
    pub ended: u64,
    pub steps: Vec<StepLine>,
    pub promises: Promises,
    pub agent_seconds: u64,
    pub tokens: u64,
    pub attempts: u32,
    /// Attempts the agent spent by its own fault.
    pub spent: u32,
    pub fixes: u32,
    pub person: Person,
}

/// What the run took of people, from its entries with their times and the spans of a person's presence at its stops.
pub fn person(events: &[(u64, Event)], looks: &[crate::presence::Look]) -> Person {
    let mut p = Person::default();
    // each question: whether it holds the run, whether it is a decision about a step's output (and not a choice
    // among options, where dropping the others sends nothing back), and when it opened
    let mut asked: BTreeMap<&str, (bool, bool, u64)> = BTreeMap::new();
    let mut answered: BTreeSet<&str> = BTreeSet::new();
    let mut theirs: BTreeSet<&str> = BTreeSet::new();
    let mut back: BTreeSet<&str> = BTreeSet::new();
    let mut stood: Vec<(u64, u64)> = Vec::new();
    let mut closed: BTreeMap<&str, u64> = BTreeMap::new();
    let end = events.last().map(|(ts, _)| *ts).unwrap_or(0);
    for (ts, e) in events {
        match e {
            Event::QuestionOpened { question } => {
                let decides = question.kind == jaynes_core::question::QuestionKind::Decision && !question.choose;
                asked.insert(&question.id, (question.holds, decides, *ts));
            }
            Event::InputNeeded { question, .. } => {
                asked.insert(&question.id, (question.holds, false, *ts));
            }
            Event::AnswerRecorded { answer, .. } => {
                let Some(&(holds, decides, opened)) = asked.get(answer.question.as_str()) else { continue };
                if answered.insert(&answer.question) {
                    closed.insert(&answer.question, *ts);
                    if holds {
                        stood.push((opened, *ts));
                    }
                }
                if answer.by.person() {
                    theirs.insert(&answer.question);
                    if decides && answer.sends_back() {
                        back.insert(&answer.question);
                    }
                }
            }
            Event::Halted { by } | Event::Resumed { by } | Event::Continued { by } | Event::RiskRaised { by, .. } | Event::CeilingRaised { by, .. }
            | Event::DeliveryCorrected { by, .. } | Event::MergeUndone { by, .. } if by.person() => p.acts += 1,
            _ => {}
        }
    }
    // a stop nobody answered held the run until its last entry
    stood.extend(asked.iter().filter(|(id, (holds, ..))| *holds && !answered.contains(*id)).map(|(_, &(_, _, opened))| (opened, end)));
    // presence counts only while its stop was open: from when it opened until it was answered or the run's last entry
    let seen: Vec<(&str, u64, u64)> = looks
        .iter()
        .filter_map(|l| {
            let &(_, _, opened) = asked.get(l.question.as_str())?;
            let until = closed.get(l.question.as_str()).copied().unwrap_or(end);
            let (from, to) = (l.from.max(opened), l.to.min(until));
            (to > from).then_some((l.question.as_str(), from, to))
        })
        .collect();
    p.unmeasured = theirs.iter().filter(|q| !seen.iter().any(|(s, ..)| s == *q)).count() as u32;
    p.seconds = (!seen.is_empty()).then(|| union(seen.iter().map(|&(_, from, to)| (from, to)).collect()));
    p.stops = theirs.len() as u32;
    p.sent_back = back.len() as u32;
    p.waited = union(stood);
    p
}

/// The seconds covered by the spans, a second covered by two of them counted once.
fn union(mut spans: Vec<(u64, u64)>) -> u64 {
    spans.sort_unstable();
    let (mut total, mut upto) = (0u64, 0u64);
    for (from, to) in spans {
        let from = from.max(upto);
        if to > from {
            total += to - from;
            upto = to;
        }
    }
    total
}

/// The summary of a finished run, from its entries with their times and the promises proved of its role. A run not
/// yet finished has none.
pub fn summarize(events: &[(u64, Event)], looks: &[crate::presence::Look], promises: Promises) -> Result<Summary, String> {
    let r = Run::fold(events.iter().map(|(_, e)| e)).ok_or("the run has no entry in the record")?;
    let Phase::Finished { outcome } = &r.phase else {
        return Err(format!("run {} is not finished: a summary tells what a finished run did", r.id));
    };
    let mut attempts: BTreeMap<&str, u32> = BTreeMap::new();
    let mut reference = None;
    for (_, e) in events {
        match e {
            Event::AttemptStarted { step, fix: 0, .. } => *attempts.entry(step).or_default() += 1,
            Event::Delivered { reference: r, .. } => reference = Some(r.as_str()),
            _ => {}
        }
    }
    let path = r.path();
    let steps = r
        .manifest
        .steps
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let s = &r.steps[i];
            let on_path = path.contains(&i);
            let done = on_path && s.status == Status::Done && s.done.is_some();
            StepLine {
                name: d.name.clone(),
                by: match d.executor {
                    Executor::Agent => "agent",
                    Executor::Role => "Jaynes",
                    Executor::Person => "person",
                },
                state: if !on_path { "left out at this risk" } else if done { "done" } else { "not done" },
                attempts: attempts.get(d.name.as_str()).copied().unwrap_or(0),
                rounds: s.round + 1,
                passed: if done { s.checks.iter().filter(|c| c.passed && !c.blocked).map(|c| (c.check.clone(), c.kind == CheckKind::Hidden)).collect() } else { Vec::new() },
                asked: if on_path { asked(&r, i) } else { None },
            }
        })
        .collect::<Vec<StepLine>>();
    // what was delivered is what the done steps changed, each thing once, summed over the steps that changed it
    let mut changed: Vec<(String, u64)> = Vec::new();
    let mut floor: Vec<String> = Vec::new();
    for &i in &path {
        for c in r.steps[i].done.as_ref().map(|d| d.changed.as_slice()).unwrap_or_default() {
            match changed.iter_mut().find(|(what, _)| *what == c.what) {
                Some((_, size)) => *size += c.size,
                None => changed.push((c.what.clone(), c.size)),
            }
            if c.floor && !floor.contains(&c.what) {
                floor.push(c.what.clone());
            }
        }
    }
    floor.sort();
    let person = person(events, looks);
    let unproved = unproved(&r, &steps, &person, &promises);
    Ok(Summary {
        run: r.id.clone(),
        request: r.request.trim().to_string(),
        role: r.manifest.role.clone(),
        version: r.manifest.version.clone(),
        fingerprint: r.role_fingerprint.short().to_string(),
        agent: r.agent.clone(),
        engine: r.engine.clone(),
        prover: jaynes_core::ENGINE.to_string(),
        risk: r.risk.word(),
        delivered: reference.map(str::to_string),
        abandoned: *outcome == Outcome::Abandoned,
        undone: r.undone,
        target: r.manifest.target.noun(),
        repository: r.manifest.target.is_repository(),
        changed,
        floor,
        unproved,
        started: events.first().map(|(ts, _)| *ts).unwrap_or(0),
        ended: events.last().map(|(ts, _)| *ts).unwrap_or(0),
        // the agents' own: an attempt of a step Jaynes does itself is not theirs
        attempts: steps.iter().filter(|s| s.by == "agent").map(|s| s.attempts).sum(),
        steps,
        promises,
        agent_seconds: r.spent.seconds,
        tokens: r.spent.tokens,
        spent: r.counts.spent,
        fixes: r.counts.fixes,
        person,
    })
}

/// What a finished run did not establish: an agent's output no check ran on, a stop that did not ask a person, the
/// person's time where it was not measured, a promise of the role not proved.
fn unproved(r: &Run, steps: &[StepLine], person: &Person, promises: &Promises) -> Vec<String> {
    let mut out = Vec::new();
    for &i in &r.path() {
        let (d, line) = (&r.manifest.steps[i], &steps[i]);
        if d.executor != Executor::Agent || line.state != "done" {
            continue;
        }
        let name = &d.name;
        // who let what the step made go on, at its current round's question
        let by = r.steps[i].question.as_ref().and_then(|q| r.questions.get(q)).and_then(|q| q.answer.as_ref()).map(|a| &a.by);
        let let_on = match by {
            Some(By::Person) => " A person kept it.",
            Some(By::Agent { .. }) => " An agent, not a person, let it go on.",
            Some(By::Rule { .. }) => " A rule in code let it go on.",
            None => "",
        };
        if line.passed.is_empty() {
            out.push(format!("What {name} made: no check ran on it.{let_on}"));
        } else if matches!(by, Some(By::Agent { .. })) {
            out.push(format!("What {name} made: an agent, not a person, let it go on."));
        }
        match &d.stop {
            Some(stop) if stop.from_risk > r.risk => out.push(format!(
                "The stop at {name} asks only at {} risk, and this run was {}: no person looked at what {name} made.",
                stop.from_risk.word(),
                r.risk.word()
            )),
            Some(_) if r.steps[i].skipped == Some(r.steps[i].round) => {
                out.push(format!("The stop at {name} asks only a sample, and did not ask this time: no person looked at what {name} made."))
            }
            _ => {}
        }
    }
    let stops = |n: u32| format!("{n} stop{}", if n == 1 { "" } else { "s" });
    match (person.seconds, person.unmeasured) {
        (None, _) if person.stops > 0 => out.push(format!("The person's time at {}: not measured.", stops(person.stops))),
        (Some(_), n) if n > 0 => out.push(format!("The person's time at {n} of {}: not measured.", stops(person.stops))),
        _ => {}
    }
    match promises {
        Promises::Told(all) => {
            out.extend(all.iter().filter(|p| !p.proved).map(|p| format!("The role's promise \"{}\": not proved.", p.says)));
            // a proof stands for the rules of the engine that made it, and the run ran on others
            if all.iter().any(|p| p.proved) && r.engine != jaynes_core::ENGINE {
                out.push(format!("The engine this run ran on ({}) did not prove the role's promises: engine {} proved them again, afterwards.", r.engine, jaynes_core::ENGINE));
            }
        }
        Promises::Proving => out.push("The role's promises: being proved again now.".to_string()),
        Promises::Unread => out.push("The role's promises: not read again, since the run's copy of the role is gone.".to_string()),
    }
    out
}

/// Who answered what step `i` asked: its stop and what it asked in the middle of its work.
fn asked(r: &Run, i: usize) -> Option<String> {
    let name = &r.manifest.steps[i].name;
    let (mut person, mut agent, mut rule, mut open) = (0u32, 0u32, 0u32, 0u32);
    for q in r.questions.values().filter(|q| q.question.step == *name) {
        match q.answer.as_ref().map(|a| &a.by) {
            Some(By::Person) => person += 1,
            Some(By::Agent { .. }) => agent += 1,
            Some(By::Rule { .. }) => rule += 1,
            None if q.superseded => {}
            None => open += 1,
        }
    }
    let questions = |n: u32| format!("{n} question{}", if n == 1 { "" } else { "s" });
    let mut said = Vec::new();
    if person > 0 {
        said.push(format!("A person answered {}.", questions(person)));
    }
    if agent > 0 {
        said.push(format!("An agent let {} go on.", questions(agent)));
    }
    if rule > 0 {
        said.push(format!("A rule in code answered {}.", questions(rule)));
    }
    if open > 0 {
        said.push(format!("{} left unanswered.", questions(open)));
    }
    if r.steps[i].skipped == Some(r.steps[i].round) {
        said.push("Its stop did not ask this time: it asks only a sample.".to_string());
    }
    (!said.is_empty()).then(|| said.join(" "))
}

impl Summary {
    /// The summary as Markdown.
    pub fn markdown(&self) -> String {
        let plural = |n: u32, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
        // a command agent is a line of shell, with the paths of the machine it ran on
        let agent = match self.agent.as_str() {
            "" => "not recorded".to_string(),
            a if a.starts_with("command:") => "a shell command".to_string(),
            a => format!("`{a}`"),
        };
        let mut out = format!("## {}\n\n### Delivered\n\n", title(&self.request));
        match (&self.delivered, self.abandoned) {
            (Some(what), false) => {
                out.push_str(&format!("{what}, in {}.", self.target));
                if self.undone {
                    out.push_str(" A person then reverted the merge.");
                }
                if self.changed.is_empty() {
                    out.push_str(" Jaynes measured no change.\n");
                } else {
                    out.push_str(if self.repository { " The files Jaynes measured:\n\n" } else { " What Jaynes measured:\n\n" });
                    // the largest first, ties by name: what a reader looks at first
                    let mut largest: Vec<&(String, u64)> = self.changed.iter().collect();
                    largest.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                    for (what, size) in largest.into_iter().take(MOST_SHOWN) {
                        out.push_str(&if self.repository {
                            format!("- `{what}`: {size} line{} changed\n", if *size == 1 { "" } else { "s" })
                        } else {
                            format!("- {what}: {size}\n")
                        });
                    }
                    if self.changed.len() > MOST_SHOWN {
                        let more = self.changed.len() - MOST_SHOWN;
                        out.push_str(&format!("- and {more} more{}\n", if self.repository { if more == 1 { " file" } else { " files" } } else { "" }));
                    }
                    // what alone makes a change risky, named each, whether or not among the largest
                    if !self.floor.is_empty() {
                        let n = self.floor.len();
                        out.push_str(&if self.repository {
                            format!("\nOf these, {n} {} run or obeyed later by tools outside any box, which alone makes a change risky:\n\n", if n == 1 { "is" } else { "are" })
                        } else {
                            format!("\nOf these, {n} cannot be taken back or {} obeyed later outside any box, which alone makes a change risky:\n\n", if n == 1 { "is" } else { "are" })
                        });
                        for what in &self.floor {
                            out.push_str(&if self.repository { format!("- `{what}`\n") } else { format!("- {what}\n") });
                        }
                    }
                }
            }
            (None, false) => out.push_str("Delivered, with no record of where.\n"),
            (Some(what), true) => out.push_str(&format!("Nothing kept: the run was abandoned after it delivered {what}.\n")),
            (None, true) => out.push_str("Nothing: the run was abandoned.\n"),
        }
        let proved: Vec<String> = self
            .steps
            .iter()
            .flat_map(|s| {
                s.passed.iter().map(move |(check, hidden)| {
                    let how = if *hidden { ": a hidden check, which the agent that made it never saw" } else { "" };
                    format!("- **{check}**, on what {} made{how}\n", s.name)
                })
            })
            .collect();
        if !self.abandoned && !proved.is_empty() {
            out.push_str(&format!("\n### Proved by\n\n{}", proved.concat()));
        }
        if !self.unproved.is_empty() {
            out.push_str(&format!("\n### Not proved\n\n{}", self.unproved.iter().map(|u| format!("- {u}\n")).collect::<String>()));
        }
        match &self.promises {
            Promises::Told(all) if !all.is_empty() && all.iter().all(|p| p.proved) => {
                out.push_str("\n### Promises proved of the role\n\n");
                all.iter().for_each(|p| out.push_str(&format!("- {}\n", p.says)));
            }
            Promises::Told(all) if !all.is_empty() => {
                out.push_str("\n### Promises of the role\n\n");
                all.iter().for_each(|p| out.push_str(&format!("- {}: {}\n", p.says, if p.proved { "proved" } else { "NOT proved" })));
            }
            Promises::Told(_) => out.push_str("\n### Promises of the role\n\n- The role makes none.\n"),
            Promises::Proving => out.push_str("\n### Promises of the role\n\n- Being proved again now: open the summary again in a moment.\n"),
            Promises::Unread => out.push_str("\n### Promises of the role\n\n- Not read again: the run's copy of the role is gone.\n"),
        }
        out.push_str("\n### Steps\n\n");
        for (n, s) in self.steps.iter().enumerate() {
            let mut line = format!("{}. **{}** ({}): {}", n + 1, s.name, s.by, s.state);
            // a step of Jaynes's own or of the person takes one attempt, which says nothing
            if s.attempts > 1 || (s.attempts == 1 && s.by == "agent") {
                line.push_str(&format!(", {}", plural(s.attempts, "attempt")));
            }
            if s.rounds > 1 {
                line.push_str(&format!(" over {} rounds", s.rounds));
            }
            line.push('.');
            if let Some(asked) = &s.asked {
                line.push_str(&format!(" {asked}"));
            }
            out.push_str(&line);
            out.push('\n');
        }
        let p = &self.person;
        let mut people = if p.stops == 0 {
            "no stop answered by a person.".to_string()
        } else {
            let time = match (p.seconds, p.unmeasured) {
                (None, _) => "not measured".to_string(),
                (Some(s), 0) => span(s),
                (Some(s), n) => format!("{} at least ({} answered with no time measured)", span(s), plural(n, "stop")),
            };
            format!("{} answered by a person, {} sent back. Their time on Jaynes's screens: {time}.", plural(p.stops, "stop"), p.sent_back)
        };
        if p.acts > 0 {
            people.push_str(&format!(" A person also acted on the run {} (halt, resume, continue, raise, correct or undo).", if p.acts == 1 { "once".to_string() } else { format!("{} times", p.acts) }));
        }
        out.push_str(&format!(
            "\n### Cost\n\n- Agents: {} of work, {} tokens, {} ({} spent by the agent's own fault), {}.\n- People: {people}\n",
            span(self.agent_seconds),
            thousands(self.tokens),
            plural(self.attempts, "attempt"),
            self.spent,
            plural(self.fixes, "fix").replace("fixs", "fixes"),
        ));
        if p.waited > 0 {
            out.push_str(&format!("- The run stood {} at its stops, waiting for an answer. That is waiting, not anyone's attention.\n", span(p.waited)));
        }
        out.push_str(&format!(
            "\n### Run\n\n- Request: {}\n- Run `{}`: role {} {} (fingerprint {}), agent {agent}, {} risk, engine {}\n- Started {}, finished {} later\n",
            one_line(&self.request),
            self.run,
            self.role,
            self.version,
            self.fingerprint,
            self.risk,
            self.engine,
            utc(self.started),
            span(self.ended.saturating_sub(self.started)),
        ));
        out
    }
}

/// Things changed listed one by one, the largest first; past it, how many more.
const MOST_SHOWN: usize = 10;

/// The request as a title: one line of at most 72 characters.
fn title(request: &str) -> String {
    let line = request.split_whitespace().collect::<Vec<_>>().join(" ");
    let line = line.trim_start_matches('#').trim();
    if line.chars().count() <= 72 { line.to_string() } else { format!("{}…", line.chars().take(71).collect::<String>()) }
}

/// A text on one line, long ones cut.
fn one_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= 400 { line } else { format!("{}…", line.chars().take(399).collect::<String>()) }
}

/// A length of time as a person reads it: "40 s", "12 min", "11 h 14 min".
fn span(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} min", seconds / 60),
        _ => format!("{} h {} min", seconds / 3600, seconds % 3600 / 60),
    }
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A time of the record, in seconds since 1970, as a date and an hour in UTC.
fn utc(ts: u64) -> String {
    let (days, rest) = (ts / 86_400, ts % 86_400);
    // the civil date of a count of days (Howard Hinnant's days_from_civil, turned around)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC", rest / 3600, rest % 3600 / 60)
}

/// The proofs under way beside the page, by the name of the file each will leave.
static PROVING: std::sync::Mutex<BTreeSet<String>> = std::sync::Mutex::new(BTreeSet::new());

/// The promises of the role a run ran with, each proved again by this engine from the run's own copy of the role.
/// A proof is kept in `proofs/` under the engine's home, by this engine and the fingerprint of the role's content,
/// so each role is proved once. With `wait` a proof not yet kept is made before answering; without it, it is made
/// beside and the answer is [`Promises::Proving`].
pub fn promises(home: &Path, run: &str, wait: bool) -> Promises {
    let dir = home.join("runs").join(run).join("role");
    if !dir.is_dir() {
        return Promises::Unread;
    }
    // a role that is a program of its own states no promise
    if !dir.join("role.toml").is_file() {
        return Promises::Told(Vec::new());
    }
    let (Ok(role), Ok(content)) = (crate::toml_role::TomlRole::load(&dir), crate::role::content_fingerprint(&dir)) else { return Promises::Unread };
    if role.promises.is_empty() {
        return Promises::Told(Vec::new());
    }
    let name = format!("{}-{content}.json", jaynes_core::ENGINE);
    let kept = home.join("proofs").join(&name);
    let read = |path: &Path| -> Option<Vec<Promised>> {
        let rows: Vec<(String, bool)> = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        Some(rows.into_iter().map(|(says, proved)| Promised { says, proved }).collect())
    };
    if let Some(told) = read(&kept) {
        return Promises::Told(told);
    }
    let prove = move || -> Vec<Promised> {
        let all: Vec<jaynes_core::promise::Promise> = role.promises.iter().map(|(_, p)| p.clone()).collect();
        let proofs = jaynes_core::promise::prove(&role.manifest, &all);
        let told: Vec<Promised> = all.iter().zip(&proofs).map(|(p, proof)| Promised { says: p.says.clone(), proved: matches!(proof, jaynes_core::promise::Proof::Proved { .. }) }).collect();
        // written whole or not at all: beside, then moved into place
        let rows: Vec<(&str, bool)> = told.iter().map(|p| (p.says.as_str(), p.proved)).collect();
        let part = kept.with_extension(format!("part-{}", std::process::id()));
        let written = kept.parent().map(std::fs::create_dir_all).and_then(Result::ok).and_then(|()| std::fs::write(&part, serde_json::to_string(&rows).ok()?).ok());
        if written.is_some() {
            let _kept = std::fs::rename(&part, &kept);
        }
        told
    };
    if wait {
        return Promises::Told(prove());
    }
    let first = PROVING.lock().map(|mut under_way| under_way.insert(name.clone())).unwrap_or(false);
    if first {
        std::thread::spawn(move || {
            let _told = prove();
            if let Ok(mut under_way) = PROVING.lock() {
                under_way.remove(&name);
            }
        });
    }
    Promises::Proving
}

/// The summary of a run of the record in `home`. With `wait` the role's promises are proved before it answers,
/// however long that takes; without it, promises not proved yet are told as being proved.
pub fn read(home: &Path, run: &str, wait: bool) -> Result<Summary, String> {
    let store = Store::open(&home.join("record.sqlite3"))?;
    let events: Vec<(u64, Event)> = store.history()?.into_iter().filter(|(_, of, _)| of == run).map(|(ts, _, e)| (ts, e)).collect();
    if events.is_empty() {
        return Err(format!("there is no run {run}"));
    }
    // a run not finished is refused before its role's promises are proved; then it is told with them, since what
    // was not proved depends on them
    let looks = crate::presence::looks(home, run)?;
    summarize(&events, &looks, Promises::Unread)?;
    summarize(&events, &looks, promises(home, run, wait))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jaynes_core::event::{CheckRecord, Cost, Effects, Usage};
    use jaynes_core::manifest::Manifest;
    use jaynes_core::risk::Changed;
    use jaynes_core::question::{Answer, Item, Question, QuestionKind, Verdict, VerdictKind};
    use jaynes_core::{Digest, Input};

    /// A role of three steps: an agent plans and the person is asked, an agent changes under two checks, Jaynes
    /// delivers. Read from JSON, as a role sends it.
    fn started(at: u64) -> Vec<(u64, Event)> {
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "role": "fix-bug", "version": "3", "protocol": 1,
            "steps": [
                {"name": "plan", "executor": "agent", "stop": {"verdicts": ["keep", "redo"], "holds": true}},
                {"name": "change", "executor": "agent", "checks": [{"name": "suite-passes", "kind": "open"}, {"name": "cases-pass", "kind": "hidden"}]},
                {"name": "deliver", "executor": "role"},
            ],
        }))
        .expect("manifest");
        let start = Input::Start {
            run: "r1".into(), request: "Make the login redirect keep the page the person came from".into(), manifest,
            role_fingerprint: Digest("6ab163606a8e72693cddc7a260cd34f5".into()), agent: "claude:some-model".into(), raise: None, attention: Default::default(),
        };
        jaynes_core::decide(None, &start).expect("start").events.into_iter().map(|e| (at, e)).collect()
    }

    fn attempt(step: &str, attempt: u32) -> Event {
        Event::AttemptStarted { step: step.into(), round: 0, attempt, fix: 0, session: None, deadline_seconds: 600, tokens_left: 1000, key: None }
    }

    fn delivered(step: &str, attempt: u32, seconds: u64, tokens: u64) -> Event {
        Event::AttemptDelivered { step: step.into(), attempt, fix: 0, output: Digest("o".into()), items: vec![], changed: vec![], usage: Usage { seconds, tokens, cached: 0 }, session: None }
    }

    fn done(step: &str) -> Event {
        Event::StepDone { step: step.into(), round: 0, output: Digest("o".into()), items: vec![], reads: vec![], changed: vec![] }
    }

    fn question(id: &str, step: &str, holds: bool) -> Question {
        Question {
            id: id.into(), step: step.into(), round: 0, kind: QuestionKind::Decision, items: vec![Item { id: "a".into(), label: "a".into(), digest: Digest("d".into()) }],
            offered: vec![VerdictKind::Keep, VerdictKind::Redo], holds, about: "Does the plan go on?".into(), choose: false, pass: 0, passes: 0, back: vec![],
        }
    }

    fn answer(id: &str, by: By, verdict: Verdict) -> Event {
        let answer = Answer { question: id.into(), shown: Digest("s".into()), verdicts: [("a".to_string(), verdict)].into(), by, context: String::new(), back: Default::default() };
        Event::AnswerRecorded { answer, effects: Effects::default() }
    }

    fn finished(outcome: Outcome) -> Event {
        Event::RunFinished { outcome, cost: Cost::default() }
    }

    /// The whole run: the plan is asked at 1,000 s and answered eleven hours later, the change fails once and passes.
    fn whole() -> Vec<(u64, Event)> {
        let t0 = 1_794_760_000; // 2026-11-15 16:26:40 UTC
        let mut e = started(t0);
        let judged = |passed: bool| Event::ChecksJudged {
            step: "change".into(), attempt: if passed { 2 } else { 1 }, fix: 0,
            checks: vec![
                CheckRecord { check: "suite-passes".into(), kind: CheckKind::Open, passed: true, blocked: false, read: vec![] },
                CheckRecord { check: "cases-pass".into(), kind: CheckKind::Hidden, passed, blocked: false, read: vec![] },
            ],
        };
        e.extend([
            (t0 + 10, attempt("plan", 1)),
            (t0 + 100, delivered("plan", 1, 90, 20_000)),
            (t0 + 100, done("plan")),
            (t0 + 100, Event::QuestionOpened { question: question("q1-plan", "plan", true) }),
            (t0 + 39_700, answer("q1-plan", By::Person, Verdict::Keep)),
            (t0 + 39_710, attempt("change", 1)),
            (t0 + 39_900, delivered("change", 1, 190, 100_000)),
            (t0 + 39_910, judged(false)),
            (t0 + 39_910, Event::AttemptFailed { step: "change".into(), attempt: 1, reasons: vec!["cases-pass".into()], usage: Usage::default(), spent: true }),
            (t0 + 39_920, attempt("change", 2)),
            (t0 + 40_400, delivered("change", 2, 480, 132_250)),
            (t0 + 40_410, judged(true)),
            (t0 + 40_410, Event::StepDone {
                step: "change".into(), round: 0, output: Digest("o".into()), items: vec![], reads: vec![],
                changed: vec![Changed::file("auth/redirect.go".into(), 12), Changed::file("auth/redirect_test.go".into(), 30)],
            }),
            (t0 + 40_420, attempt("deliver", 1)),
            (t0 + 40_430, done("deliver")),
            (t0 + 40_430, Event::Delivered { digest: Digest("x".into()), reference: "jaynes/r1 at 87b9b5dc3549".into() }),
            (t0 + 40_440, finished(Outcome::Delivered)),
        ]);
        e
    }

    #[test]
    fn a_summary_opens_with_what_was_delivered_what_proved_it_and_what_was_not_proved() {
        let promises = vec![Promised { says: "Nothing is delivered before the hidden cases pass".into(), proved: true }];
        let told = summarize(&whole(), &[], Promises::Told(promises)).expect("summary").markdown();
        let expected = "\
## Make the login redirect keep the page the person came from

### Delivered

jaynes/r1 at 87b9b5dc3549, in a repository. The files Jaynes measured:

- `auth/redirect_test.go`: 30 lines changed
- `auth/redirect.go`: 12 lines changed

### Proved by

- **suite-passes**, on what change made
- **cases-pass**, on what change made: a hidden check, which the agent that made it never saw

### Not proved

- What plan made: no check ran on it. A person kept it.
- The person's time at 1 stop: not measured.

### Promises proved of the role

- Nothing is delivered before the hidden cases pass

### Steps

1. **plan** (agent): done, 1 attempt. A person answered 1 question.
2. **change** (agent): done, 2 attempts.
3. **deliver** (Jaynes): done.

### Cost

- Agents: 12 min of work, 252,250 tokens, 3 attempts (1 spent by the agent's own fault), 0 fixes.
- People: 1 stop answered by a person, 0 sent back. Their time on Jaynes's screens: not measured.
- The run stood 11 h 0 min at its stops, waiting for an answer. That is waiting, not anyone's attention.

### Run

- Request: Make the login redirect keep the page the person came from
- Run `r1`: role fix-bug 3 (fingerprint 6ab163606a8e), agent `claude:some-model`, low risk, engine ENGINE
- Started 2026-11-15 16:26 UTC, finished 11 h 14 min later
";
        assert_eq!(told, expected.replace("ENGINE", jaynes_core::ENGINE));
        // proved by another engine than the run's: said under Not proved; and an agent never recorded is said so
        let mut older = whole();
        if let Event::RunStarted { engine, .. } = &mut older[0].1 {
            *engine = "0.1.0+older".into();
        }
        let mut s = summarize(&older, &[], Promises::Told(vec![Promised { says: "x".into(), proved: true }])).expect("summary");
        s.agent = String::new();
        let told = s.markdown();
        // the promises stand for the engine that proved them: the run's own did not, and that is not proved
        assert!(told.contains("### Promises proved of the role\n\n- x\n\n"), "{told}");
        assert!(told.contains(&format!("- The engine this run ran on (0.1.0+older) did not prove the role's promises: engine {} proved them again, afterwards.\n", jaynes_core::ENGINE)), "{told}");
        let not_proved = told.split("### Not proved").nth(1).and_then(|t| t.split("###").next()).expect("not proved");
        assert!(not_proved.contains("did not prove the role's promises"), "{told}");
        assert!(told.contains("agent not recorded, low risk, engine 0.1.0+older\n"), "{told}");
    }

    #[test]
    fn what_tools_run_or_obey_later_outside_any_box_is_named_apart_each_file_of_it() {
        // a change of 14 files, two of them on the floor and among the smallest: past the ten largest, named all the same
        let mut e = whole();
        let mut changed: Vec<Changed> = (0..12).map(|i| Changed::file(format!("src/part{i:02}.go"), 40 + i)).collect();
        changed.push(Changed::file(".github/workflows/ci.yml".into(), 2));
        changed.push(Changed::file("Makefile".into(), 1));
        changed.push(Changed::file("src/part00.go".into(), 5));
        for (_, ev) in &mut e {
            if let Event::StepDone { step, changed: c, .. } = ev
                && step == "change"
            {
                *c = changed.clone();
            }
        }
        let told = summarize(&e, &[], Promises::Unread).expect("summary").markdown();
        let delivered = told.split("### Delivered").nth(1).and_then(|t| t.split("###").next()).expect("delivered");
        assert!(delivered.contains("- and 4 more files\n"), "{told}");
        assert!(
            delivered.ends_with("\nOf these, 2 are run or obeyed later by tools outside any box, which alone makes a change risky:\n\n- `.github/workflows/ci.yml`\n- `Makefile`\n\n"),
            "{delivered}"
        );
        // a file counted once, however many steps changed it
        assert!(delivered.contains("- `src/part00.go`: 45 lines changed\n"), "{delivered}");
        // none on the floor: nothing said of it
        let plain = summarize(&whole(), &[], Promises::Unread).expect("summary").markdown();
        assert!(!plain.contains("outside any box"), "{plain}");
        // another target: what it changed that cannot be taken back or is obeyed later, in its own words
        let mut s = summarize(&e, &[], Promises::Unread).expect("summary");
        s.repository = false;
        let other = s.markdown();
        assert!(other.contains("\nOf these, 2 cannot be taken back or are obeyed later outside any box, which alone makes a change risky:\n\n- .github/workflows/ci.yml\n- Makefile\n"), "{other}");
    }

    #[test]
    fn what_a_stop_did_not_ask_a_person_is_not_proved_and_a_long_request_is_cut_in_the_title() {
        let mut e = started(0);
        if let Event::RunStarted { request, .. } = &mut e[0].1 {
            *request = format!("{} and then much more", "Make the login redirect keep the page the person came from".repeat(2));
        }
        let mut many: Vec<Changed> = (0..32).rev().map(|n| Changed::file(format!("f{n:02}.go"), 1)).collect();
        many.push(Changed::file("big.go".into(), 40));
        e.extend([
            (10, attempt("plan", 1)),
            (20, delivered("plan", 1, 10, 500)),
            (20, done("plan")),
            (20, Event::QuestionOpened { question: question("q1-plan", "plan", true) }),
            (30, answer("q1-plan", By::Agent { who: "an agent".into() }, Verdict::Keep)),
            (40, attempt("change", 1)),
            (50, Event::StepDone { step: "change".into(), round: 0, output: Digest("o".into()), items: vec![], reads: vec![], changed: many }),
            (60, done("deliver")),
            (60, Event::Delivered { digest: Digest("x".into()), reference: "jaynes/r1 at abc; merged as def".into() }),
            (70, finished(Outcome::Delivered)),
            (80, Event::MergeUndone { note: "wrong".into(), by: By::Person }),
        ]);
        let told = summarize(&e, &[], Promises::Told(vec![])).expect("summary").markdown();
        let title = told.lines().next().expect("title");
        assert_eq!(title.chars().count(), 3 + 72, "{title}");
        assert!(title.starts_with("## Make the login redirect") && title.ends_with('…'), "{title}");
        assert!(told.contains("\njaynes/r1 at abc; merged as def, in a repository. A person then reverted the merge. The files Jaynes measured:\n"), "{told}");
        // the ten largest, the largest first and ties by name, then how many more
        assert!(told.contains("- `big.go`: 40 lines changed\n- `f00.go`: 1 line changed\n"), "{told}");
        assert!(told.contains("- `f08.go`: 1 line changed\n- and 23 more files\n"), "{told}");
        assert!(!told.contains("`f09.go`"), "{told}");
        assert!(told.contains("### Not proved\n\n- What plan made: no check ran on it. An agent, not a person, let it go on.\n- What change made: no check ran on it.\n"), "{told}");
        // the change's checks were never judged: nothing to put under Proved by
        assert!(!told.contains("### Proved by"), "{told}");
    }

    #[test]
    fn the_wait_at_a_stop_is_never_the_person_s_time_and_what_was_not_measured_is_not_zero() {
        let p = person(&whole(), &[]);
        assert_eq!(p, Person { stops: 1, sent_back: 0, acts: 0, seconds: None, unmeasured: 1, waited: 39_600 });
        let told = summarize(&whole(), &[], Promises::Unread).expect("summary").markdown();
        let people = told.lines().find(|l| l.starts_with("- People:")).expect("the people's line");
        assert!(people.contains("not measured") && !people.contains("min") && !people.contains(" h "), "{people}");
    }

    #[test]
    fn only_a_person_s_answers_are_stops_and_two_stops_open_at_once_wait_once() {
        let mut e = started(0);
        e.extend([
            (100, Event::QuestionOpened { question: question("q1", "plan", true) }),
            (150, Event::QuestionOpened { question: question("q2", "plan", true) }),
            // one that does not hold the run: it does not wait there
            (150, Event::QuestionOpened { question: question("q3", "plan", false) }),
            (160, Event::QuestionOpened { question: question("q4", "plan", true) }),
            (200, answer("q1", By::Agent { who: "an agent".into() }, Verdict::Keep)),
            (300, answer("q2", By::Person, Verdict::Redo { note: "smaller".into() })),
            (310, answer("q3", By::Rule { rule: "routine".into() }, Verdict::Keep)),
            (320, Event::Halted { by: By::Person }),
            (330, Event::Resumed { by: By::Agent { who: "an agent".into() } }),
            // the last one is never answered: the run stood there until it ended
            (400, finished(Outcome::Abandoned)),
        ]);
        assert_eq!(person(&e, &[]), Person { stops: 1, sent_back: 1, acts: 1, seconds: None, unmeasured: 1, waited: 300 });
    }

    fn seen(question: &str, from: u64, to: u64) -> crate::presence::Look {
        crate::presence::Look { run: "r1".into(), question: question.into(), from, to }
    }

    #[test]
    fn the_person_s_time_is_their_presence_at_open_stops_counted_once_and_a_stop_without_it_is_not_measured() {
        let mut e = started(0);
        e.extend([
            (100, Event::QuestionOpened { question: question("q1", "plan", true) }),
            (100, Event::QuestionOpened { question: question("q2", "plan", true) }),
            (1_000, answer("q1", By::Person, Verdict::Keep)),
            (2_000, answer("q2", By::Person, Verdict::Keep)),
            (2_100, finished(Outcome::Delivered)),
        ]);
        // q1: two spans, one starting before the stop opened and one going on after it was answered, both cut to
        // the time it was open; q2: a span that overlaps q1's, and the person counted once for the overlap
        let looks = vec![seen("q1", 90, 160), seen("q1", 950, 1_020), seen("q2", 130, 200)];
        let p = person(&e, &looks);
        assert_eq!((p.stops, p.seconds, p.unmeasured), (2, Some(150), 0));
        // no span for q2: its time is not measured, and the total says it is at least that
        let p = person(&e, &looks[..2]);
        assert_eq!((p.seconds, p.unmeasured), (Some(110), 1));
        let told = summarize(&e, &looks[..2], Promises::Told(vec![])).expect("summary").markdown();
        assert!(told.contains("- People: 2 stops answered by a person, 0 sent back. Their time on Jaynes's screens: 1 min at least (1 stop answered with no time measured).\n"), "{told}");
        let told = summarize(&e, &looks, Promises::Told(vec![])).expect("summary").markdown();
        assert!(told.contains("Their time on Jaynes's screens: 2 min.\n"), "{told}");
        // waiting and attention are never added together
        assert!(told.contains("- The run stood 31 min at its stops, waiting for an answer."), "{told}");
    }

    #[test]
    fn a_run_that_is_not_finished_has_no_summary() {
        let mut e = whole();
        e.pop();
        assert_eq!(summarize(&e, &[], Promises::Unread), Err("run r1 is not finished: a summary tells what a finished run did".to_string()));
        assert_eq!(summarize(&[], &[], Promises::Unread), Err("the run has no entry in the record".to_string()));
    }

    #[test]
    fn a_run_abandoned_says_so_with_what_stood_undone_and_promises_not_read_are_said_so() {
        let mut e = started(0);
        e.extend([
            (10, attempt("plan", 1)),
            (20, delivered("plan", 1, 10, 500)),
            (20, done("plan")),
            (20, Event::QuestionOpened { question: question("q1-plan", "plan", true) }),
            (80, finished(Outcome::Abandoned)),
        ]);
        let told = summarize(&e, &[], Promises::Unread).expect("summary").markdown();
        assert!(told.contains("### Delivered\n\nNothing: the run was abandoned.\n\n### Not proved\n"), "{told}");
        assert!(!told.contains("### Proved by"), "{told}");
        assert!(told.contains("1. **plan** (agent): done, 1 attempt. 1 question left unanswered.\n2. **change** (agent): not done.\n"), "{told}");
        assert!(told.contains("### Promises of the role\n\n- Not read again: the run's copy of the role is gone.\n"), "{told}");
        assert!(told.contains("- People: no stop answered by a person.\n"), "{told}");
        let none = summarize(&e, &[], Promises::Told(vec![])).expect("summary").markdown();
        assert!(none.contains("### Promises of the role\n\n- The role makes none.\n"), "{none}");
        let proving = summarize(&e, &[], Promises::Proving).expect("summary").markdown();
        assert!(proving.contains("### Promises of the role\n\n- Being proved again now: open the summary again in a moment.\n"), "{proving}");
        let broken = vec![Promised { says: "Nothing is merged before the tests pass".into(), proved: false }];
        let unproved = summarize(&e, &[], Promises::Told(broken)).expect("summary").markdown();
        assert!(unproved.contains("### Promises of the role\n\n- Nothing is merged before the tests pass: NOT proved\n"), "{unproved}");
    }
}
