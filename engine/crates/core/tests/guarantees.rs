//! The tests of `docs/guarantees.md`. Each guarantee is checked on runs of random roles in a random world: the
//! agent delivers, fails or crashes, checks pass, fail or cannot judge, the person answers late or wrongly, the
//! shell stops and starts again, old reports arrive late. After every input the test checks every guarantee
//! that input could break. A failure prints the seed that reproduces it.

use jaynes_core::decide::{routine, READ_INSIDE_PROJECT};
use jaynes_core::event::{Event, Fault, Usage};
use jaynes_core::manifest::{CheckDecl, CheckKind, Effect, Executor, Manifest, ProjectAccess, Reach, SessionKind, StepDecl, StopDecl};
use jaynes_core::question::{Answer, Ask, AskKind, By, Item, Question, QuestionKind, Verdict, VerdictKind};
use jaynes_core::risk::{self, Changed, Risk};
use jaynes_core::run::{Phase, Run, Status};
use jaynes_core::{chain, decide, AttemptOutcome, CheckResult, CheckVerdict, Command, Digest, Input};
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

// --- a small deterministic source of choices ------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> Option<&'a T> {
        if v.is_empty() { None } else { v.get(self.below(v.len())) }
    }
}

const ALL_VERDICTS: [VerdictKind; 5] =
    [VerdictKind::Keep, VerdictKind::Change, VerdictKind::Redo, VerdictKind::Drop, VerdictKind::Skip];

/// A random role that keeps every rule of `Manifest::validate`.
fn role(rng: &mut Rng) -> Manifest {
    for _ in 0..200 {
        let n = 1 + rng.below(6);
        let mut steps: Vec<StepDecl> = Vec::new();
        for i in 0..n {
            let executor = match rng.below(10) {
                0..=5 => Executor::Agent,
                6..=7 => Executor::Role,
                _ => Executor::Person,
            };
            let mut needs = Vec::new();
            for j in 0..i {
                if rng.chance(40) {
                    needs.push(format!("s{j}"));
                }
            }
            let from_risk = if rng.chance(25) { Risk::High } else { Risk::Low };
            let project = match (executor, rng.below(3)) {
                (Executor::Person, _) => ProjectAccess::None,
                (_, 0) => ProjectAccess::None,
                (_, 1) => ProjectAccess::Read,
                _ => ProjectAccess::Write,
            };
            let reach = if executor == Executor::Person {
                Reach::default()
            } else {
                Reach {
                    project,
                    write: if project == ProjectAccess::Write { vec!["**".to_string()] } else { Vec::new() },
                    network: Default::default(),
                    commands: rng.chance(50),
                }
            };
            let mut checks = Vec::new();
            if executor != Executor::Person {
                for k in 0..rng.below(3) {
                    let kind = if rng.chance(50) { CheckKind::Shape } else { CheckKind::Behaviour };
                    let name = match kind {
                        CheckKind::Shape => format!("shapecheck{i}x{k}"),
                        CheckKind::Behaviour => format!("hiddencheck{i}x{k}"),
                    };
                    checks.push(CheckDecl { name, kind, about: format!("holds {k}") });
                }
            }
            let stop = if executor == Executor::Person || rng.chance(45) {
                let mut verdicts: Vec<VerdictKind> = ALL_VERDICTS.iter().copied().filter(|_| rng.chance(60)).collect();
                if !verdicts.contains(&VerdictKind::Keep) {
                    verdicts.insert(0, VerdictKind::Keep);
                }
                let stop_risk = if from_risk == Risk::High || rng.chance(20) { Risk::High } else { Risk::Low };
                Some(StopDecl { verdicts, holds: rng.chance(30), from_risk: stop_risk, about: String::new() })
            } else {
                None
            };
            let effect = match (executor, rng.below(8)) {
                (Executor::Role, 0) => Effect::Irreversible { idempotent: rng.chance(50) },
                (Executor::Agent | Executor::Role, 1) => Effect::Confined,
                _ => Effect::None,
            };
            let hidden = executor == Executor::Agent && needs.is_empty() && project == ProjectAccess::None && rng.chance(20);
            let session = if executor == Executor::Agent && rng.chance(30) { SessionKind::Continue } else { SessionKind::Fresh };
            steps.push(StepDecl {
                name: format!("s{i}"),
                needs,
                executor,
                session,
                reach,
                effect,
                checks,
                stop,
                from_risk,
                attempts: 1 + rng.below(3) as u32,
                fixes: rng.below(3) as u32,
                hidden,
                about: String::new(),
            });
        }
        let m = Manifest { role: "random".to_string(), version: "1".to_string(), protocol: 1, steps };
        if m.validate().is_ok() {
            return m;
        }
    }
    let one = StepDecl {
        name: "s0".to_string(),
        needs: Vec::new(),
        executor: Executor::Agent,
        session: SessionKind::Fresh,
        reach: Reach::default(),
        effect: Effect::None,
        checks: Vec::new(),
        stop: None,
        from_risk: Risk::Low,
        attempts: 1,
        fixes: 0,
        hidden: false,
        about: String::new(),
    };
    Manifest { role: "random".to_string(), version: "1".to_string(), protocol: 1, steps: vec![one] }
}

// --- the world --------------------------------------------------------------------------------------------------

struct World {
    rng: Rng,
    run: Option<Run>,
    log: Vec<Event>,
    commands: Vec<Command>,
    shown: BTreeMap<String, Question>,
    /// Usage of every report the core accepted, to check the run counts all of it.
    usage: Usage,
    /// Every attempt that ended, by (step, attempt), and how many times.
    ended: BTreeMap<(String, u32), u32>,
    seed: u64,
}

impl World {
    fn new(seed: u64) -> World {
        World {
            rng: Rng(seed | 1),
            run: None,
            log: Vec::new(),
            commands: Vec::new(),
            shown: BTreeMap::new(),
            usage: Usage::default(),
            ended: BTreeMap::new(),
            seed,
        }
    }

    fn run(&self) -> &Run {
        self.run.as_ref().expect("the run started")
    }

    /// Send an input, check every guarantee it could break, and record what it decided.
    fn send(&mut self, input: Input) -> bool {
        let before = self.run.clone();
        match decide(before.as_ref(), &input) {
            Err(_) => {
                assert!(refusal_allowed(&input), "seed {}: refused {input:?}", self.seed);
                false
            }
            Ok(out) => {
                self.check(before.as_ref(), &input, &out.events, &out.commands);
                self.log.extend(out.events.iter().cloned());
                let folded = Run::fold(&self.log);
                // G16: reading the record again gives the same state.
                let after = match (&before, out.events.is_empty()) {
                    (Some(b), true) => Some(b.clone()),
                    _ => folded.clone(),
                };
                assert_eq!(folded, after, "seed {}: G16 the record folds to another state", self.seed);
                self.run = folded;
                for c in out.commands {
                    if let Command::Ask { question } = &c {
                        self.shown.insert(question.id.clone(), question.clone());
                    }
                    self.commands.push(c);
                }
                true
            }
        }
    }

    fn check(&mut self, before: Option<&Run>, input: &Input, events: &[Event], commands: &[Command]) {
        let seed = self.seed;
        let mut state = before.cloned();
        for e in events {
            let pre = state.clone();
            state = match state {
                None => Run::started(e),
                Some(mut r) => {
                    r.apply(e);
                    Some(r)
                }
            };
            let (Some(pre), Some(post)) = (pre, state.as_ref()) else { continue };
            // G7: the risk only goes up, by a rule or by the person.
            assert!(post.risk >= pre.risk, "seed {seed}: G7 the risk went down");
            if let Event::RiskRaised { by, .. } = e {
                assert!(matches!(by, By::Person | By::Rule { .. }), "seed {seed}: G7 risk raised by {by:?}");
            }
            match e {
                Event::AttemptStarted { step, fix: 0, .. } => {
                    let i = pre.manifest.index(step).expect("step");
                    // G1: in order, one at a time, after what it needs.
                    for j in pre.path() {
                        if j < i {
                            assert!(really_fresh(&pre, j), "seed {seed}: G1 {step} started before {} was done", pre.manifest.steps[j].name);
                        }
                    }
                    for n in &pre.manifest.steps[i].needs {
                        let j = pre.manifest.index(n).expect("need");
                        assert!(really_fresh(&pre, j), "seed {seed}: G1 {step} started before its need {n}");
                    }
                    assert!(
                        pre.steps.iter().all(|s| !matches!(s.status, Status::Running { .. } | Status::Checking { .. })),
                        "seed {seed}: G1 two steps at once"
                    );
                    // G5: no attempt starts at the ceiling.
                    assert!(
                        pre.spent.seconds < pre.ceiling.seconds && pre.spent.tokens < pre.ceiling.tokens,
                        "seed {seed}: G5 an attempt started at the ceiling"
                    );
                    // G14: nothing that cannot be undone rests on an answer not given.
                    if pre.manifest.steps[i].effect.irreversible() {
                        assert!(pre.unconfirmed(i).is_empty(), "seed {seed}: G14 {step} started on unconfirmed answers");
                    }
                }
                Event::StepDone { step, items, .. } => {
                    let i = pre.manifest.index(step).expect("step");
                    let decl = &pre.manifest.steps[i];
                    // G12: what the person kept stays, and what they dropped does not come back.
                    for (id, (kept, _)) in &pre.steps[i].locks {
                        assert!(
                            items.iter().any(|x| &x.id == id && x.digest == kept.digest),
                            "seed {seed}: G12 kept item {id} of {step} changed"
                        );
                    }
                    for gone in &pre.steps[i].dropped {
                        assert!(
                            !items.iter().any(|x| x.id == gone.id || x.digest == gone.digest),
                            "seed {seed}: G12 dropped item {} came back in {step}", gone.id
                        );
                    }
                    // G2: an output is done only once every check passed.
                    if decl.executor != Executor::Person {
                        match input {
                            Input::ChecksEnded { results, step: s, .. } if s == step => {
                                for c in &decl.checks {
                                    assert!(
                                        results.iter().any(|r| r.check == c.name && r.verdict == CheckVerdict::Pass),
                                        "seed {seed}: G2 {step} done without check {} passing", c.name
                                    );
                                }
                            }
                            _ => assert!(decl.checks.is_empty(), "seed {seed}: G2 {step} done without its checks"),
                        }
                        let attempt = match pre.steps[i].status {
                            Status::Checking { attempt, .. } => attempt,
                            _ => panic!("seed {seed}: G2 {step} done without an output under check"),
                        };
                        *self.ended.entry((step.clone(), attempt)).or_default() += 1;
                    }
                }
                Event::AttemptFailed { step, attempt, .. } | Event::AttemptInterrupted { step, attempt, .. } => {
                    *self.ended.entry((step.clone(), *attempt)).or_default() += 1;
                }
                Event::StepInvalidated { step, because, .. } => {
                    // G13: a step runs again only when what it used changed, or when it was sent back.
                    let i = pre.manifest.index(step).expect("step");
                    let sent_back = because.iter().any(|b| b.contains("sent it back") || b.contains("being done again"));
                    assert!(sent_back || !really_fresh(&pre, i), "seed {seed}: G13 {step} ran again though nothing it used changed");
                }
                Event::AnswerRecorded { answer, .. } => {
                    // G9: who answered.
                    let q = &pre.questions[&answer.question].question;
                    match &answer.by {
                        By::Person => {}
                        By::Rule { rule } => {
                            assert_eq!(rule, READ_INSIDE_PROJECT, "seed {seed}: G9 unknown rule");
                            assert_eq!(q.kind, QuestionKind::Permission, "seed {seed}: G9 a rule answered a decision");
                        }
                        By::Agent { .. } => panic!("seed {seed}: G9 an agent answered"),
                    }
                }
                _ => {}
            }
        }
        let Some(post) = state.as_ref() else { return };
        // G7: the risk is never below what the rules say of the change.
        let changed: Vec<Changed> =
            post.path().into_iter().filter_map(|i| post.steps[i].done.as_ref()).flat_map(|d| d.changed.clone()).collect();
        assert!(post.risk >= risk::of_change(&changed).0, "seed {seed}: G7 the risk is below its rules");
        // G13: once delivered, every step kept used the values that hold now.
        if !post.halted && post.delivered.is_some() && post.delivering.is_none() && matches!(post.phase, Phase::Waiting { step: None, .. } | Phase::Finished { .. }) {
            for i in post.path() {
                assert!(really_fresh(post, i), "seed {seed}: G13 {} was delivered on stale inputs", post.manifest.steps[i].name);
            }
        }
        for c in commands {
            match c {
                Command::RunAgent(turn) => {
                    let i = post.manifest.index(&turn.step).expect("step");
                    // G3: no hidden check, no hidden step's output.
                    assert!(turn.checks.iter().all(|c| c.kind == CheckKind::Shape), "seed {seed}: G3 a hidden check was given");
                    let text = serde_json::to_string(turn).expect("json");
                    for d in &post.manifest.steps {
                        for ch in d.checks.iter().filter(|c| c.kind == CheckKind::Behaviour) {
                            assert!(!text.contains(&ch.name), "seed {seed}: G3 the agent was told of {}", ch.name);
                        }
                        if d.hidden && d.name != turn.step {
                            assert!(turn.given.iter().all(|r| r.step != d.name), "seed {seed}: G3 hidden {} given", d.name);
                        }
                    }
                    // G2: only outputs that passed their checks are given.
                    for r in &turn.given {
                        let j = post.manifest.index(&r.step).expect("read");
                        assert_eq!(post.output(j), Some(&r.output), "seed {seed}: G2 an unchecked output was given");
                    }
                    let _ = i;
                }
                Command::RunRole { given, .. } | Command::Deliver { given, .. } => {
                    for r in given {
                        let j = post.manifest.index(&r.step).expect("read");
                        assert_eq!(post.output(j), Some(&r.output), "seed {seed}: G2 an unchecked output was given");
                    }
                }
                _ => {}
            }
        }
        // G4: a fault of Jaynes spends no attempt.
        let faulted = match input {
            Input::AttemptEnded { outcome: AttemptOutcome::Fault { .. }, .. } => true,
            Input::ProbeEnded { ok: false, .. } => true,
            Input::ChecksEnded { results, step, .. } => {
                let i = post.manifest.index(step).expect("step");
                post.manifest.steps[i].checks.iter().any(|c| {
                    results.iter().find(|r| r.check == c.name).is_none_or(|r| matches!(r.verdict, CheckVerdict::Blocked { .. }))
                })
            }
            _ => false,
        };
        if faulted && !events.iter().any(|e| matches!(e, Event::StaleReport { .. })) {
            let b = before.expect("a run");
            assert_eq!(b.counts.spent, post.counts.spent, "seed {seed}: G4 a fault spent an attempt");
            assert!(
                matches!(post.phase, Phase::Blocked { fault: Fault::Jaynes, .. }),
                "seed {seed}: G4 a fault did not stop the run as Jaynes's: {:?}", post.phase
            );
        }
        // G6: a late report changes nothing.
        if events.iter().any(|e| matches!(e, Event::StaleReport { .. })) {
            assert_eq!(events.len(), 1, "seed {seed}: G6 a late report did more than being recorded");
        }
        // G5: every accepted report's time and tokens are counted.
        if !events.iter().any(|e| matches!(e, Event::StaleReport { .. }))
            && let Input::AttemptEnded { outcome: AttemptOutcome::Delivered { usage, .. } | AttemptOutcome::Failed { usage, .. }, .. } = input {
                self.usage.add(*usage);
            }
        assert_eq!(post.spent, self.usage, "seed {seed}: G5 the run's spending differs from what was reported");
    }
}

/// Inputs the world sends that the core may refuse.
fn refusal_allowed(input: &Input) -> bool {
    !matches!(input, Input::Tick | Input::Recovered)
}

/// Whether step `i` is done on what holds now, worked out here and not by the core, so the test does not grade
/// the core with its own answer.
fn really_fresh(run: &Run, i: usize) -> bool {
    let s = &run.steps[i];
    let Some(done) = (s.status == Status::Done).then_some(s.done.as_ref()).flatten() else { return false };
    let decl = &run.manifest.steps[i];
    let mut used: BTreeSet<String> = decl.needs.iter().cloned().collect();
    if decl.reach.project != ProjectAccess::None {
        for j in run.manifest.path(run.risk) {
            if j < i && run.manifest.steps[j].reach.project == ProjectAccess::Write {
                used.insert(run.manifest.steps[j].name.clone());
            }
        }
    }
    used.iter().all(|n| {
        let j = run.manifest.index(n).expect("step");
        let now = run.steps[j].done.as_ref().map(|d| &d.output);
        now.is_some() && done.reads.iter().any(|r| &r.step == n && Some(&r.output) == now)
    })
}

fn digest_pool(rng: &mut Rng) -> Digest {
    Digest(format!("out{}", rng.below(4)))
}

fn items_for(rng: &mut Rng, run: &Run, step: &str) -> Vec<Item> {
    let i = run.manifest.index(step).expect("step");
    let s = &run.steps[i];
    let mut items: Vec<Item> = Vec::new();
    if rng.chance(75) {
        for (item, _) in s.locks.values() {
            items.push(item.clone());
        }
    }
    for k in 0..rng.below(3) {
        let id = format!("i{}", rng.below(4) + k);
        if items.iter().any(|x| x.id == id) {
            continue;
        }
        if rng.chance(75) && s.dropped.iter().any(|d| d.id == id) {
            continue;
        }
        let digest = Digest(format!("{id}-{}", rng.below(3)));
        if rng.chance(75) && s.dropped.iter().any(|d| d.digest == digest) {
            continue;
        }
        items.push(Item { id: id.clone(), label: format!("item {id}"), digest });
    }
    items
}

fn changed(rng: &mut Rng) -> Vec<Changed> {
    match rng.below(10) {
        0 => vec![Changed { path: ".github/workflows/ci.yml".to_string(), lines: 2 }],
        1 => vec![Changed { path: "src/big.rs".to_string(), lines: 80 }],
        2..=5 => vec![Changed { path: format!("src/f{}.rs", rng.below(3)), lines: 1 + rng.below(5) as u64 }],
        _ => Vec::new(),
    }
}

fn usage(rng: &mut Rng) -> Usage {
    Usage { seconds: 10 + rng.below(200) as u64, tokens: 1000 + rng.below(300_000) as u64, cached: rng.below(1000) as u64 }
}

fn verdict(rng: &mut Rng, offered: &[VerdictKind]) -> Verdict {
    match rng.pick(offered).copied().unwrap_or(VerdictKind::Keep) {
        VerdictKind::Keep => Verdict::Keep,
        VerdictKind::Change => Verdict::Change { text: "make it bluer".to_string() },
        VerdictKind::Redo => Verdict::Redo { note: "try again".to_string() },
        VerdictKind::Drop => Verdict::Drop,
        VerdictKind::Skip => Verdict::Skip,
    }
}

impl World {
    fn respond(&mut self, c: Command) {
        let run = self.run().clone();
        match c {
            Command::Probe { step, .. } => {
                let ok = self.rng.chance(95);
                self.send(Input::ProbeEnded { step, ok, reason: "the box refused /tmp".to_string() });
            }
            Command::RunAgent(turn) => {
                if self.rng.chance(6) {
                    let target = if self.rng.chance(50) { "src/a.rs".to_string() } else { "/etc/passwd".to_string() };
                    let kind = if self.rng.chance(80) { AskKind::Read } else { AskKind::Network };
                    self.send(Input::PermissionAsked { step: turn.step.clone(), attempt: turn.attempt, ask: Ask { kind, target } });
                }
                let outcome = match self.rng.below(100) {
                    0..=71 => AttemptOutcome::Delivered {
                        output: digest_pool(&mut self.rng),
                        items: items_for(&mut self.rng, &run, &turn.step),
                        changed: if turn.reach.project == ProjectAccess::Write { changed(&mut self.rng) } else { Vec::new() },
                        usage: usage(&mut self.rng),
                        session: Some(format!("session-{}", turn.step)),
                    },
                    72..=88 => AttemptOutcome::Failed { reasons: vec!["no output".to_string()], usage: usage(&mut self.rng) },
                    89..=94 => AttemptOutcome::Fault { reason: "claude did not start".to_string() },
                    _ => AttemptOutcome::Interrupted { reason: "quota".to_string() },
                };
                self.send(Input::AttemptEnded { step: turn.step, attempt: turn.attempt, fix: turn.fix, outcome });
            }
            Command::RunRole { step, attempt, .. } => {
                let outcome = match self.rng.below(100) {
                    0..=79 => AttemptOutcome::Delivered {
                        output: digest_pool(&mut self.rng),
                        items: items_for(&mut self.rng, &run, &step),
                        changed: Vec::new(),
                        usage: Usage::default(),
                        session: None,
                    },
                    80..=91 => AttemptOutcome::NeedsInput {
                        items: vec![Item { id: "which".to_string(), label: "which test".to_string(), digest: Digest("w".to_string()) }],
                        offered: vec![VerdictKind::Keep, VerdictKind::Change],
                        about: String::new(),
                    },
                    92..=95 => AttemptOutcome::Refused { reason: "no test runner".to_string() },
                    _ => AttemptOutcome::Fault { reason: "the role program crashed".to_string() },
                };
                self.send(Input::AttemptEnded { step, attempt, fix: 0, outcome });
            }
            Command::RunChecks { step, attempt, fix, checks, .. } => {
                let mut results = Vec::new();
                for name in checks {
                    let verdict = match self.rng.below(100) {
                        0..=79 => CheckVerdict::Pass,
                        80..=95 => CheckVerdict::Fail { reasons: vec!["it does not hold".to_string()], detail: String::new() },
                        96..=97 => CheckVerdict::Blocked { reason: "the tool did not start".to_string() },
                        _ => continue,
                    };
                    results.push(CheckResult { check: name, verdict });
                }
                self.send(Input::ChecksEnded { step, attempt, fix, results });
            }
            Command::Deliver { digest, .. } => {
                let ok = self.rng.chance(95);
                self.send(Input::DeliveryEnded { digest, ok, reference: "jaynes/run".to_string() });
            }
            Command::Ask { .. } | Command::Permit { .. } | Command::Notify { .. } => {}
        }
    }

    fn answer(&mut self) {
        let run = self.run().clone();
        let open: Vec<&jaynes_core::run::QuestionState> = run.questions.values().collect();
        let Some(q) = self.rng.pick(&open).map(|q| q.question.clone()) else { return };
        let mut verdicts = BTreeMap::new();
        for item in &q.items {
            if self.rng.chance(70) {
                verdicts.insert(item.id.clone(), verdict(&mut self.rng, &q.offered));
            }
        }
        let wrong = self.rng.chance(5);
        let shown = if wrong { Digest("stale".to_string()) } else { q.shown() };
        let by = match self.rng.below(20) {
            0 => By::Agent { who: "claude".to_string() },
            1 => By::Rule { rule: READ_INSIDE_PROJECT.to_string() },
            _ => By::Person,
        };
        let state = run.questions[&q.id].clone();
        let answer = Answer { question: q.id.clone(), shown, verdicts, by: by.clone() };
        let accepted = self.send(Input::Answer { answer: answer.clone() });
        // G9 and G10: only the person, only for what was shown, only while it stands.
        if wrong || !by.person() || state.superseded {
            assert!(!accepted, "seed {}: G9/G10 an answer was taken: {answer:?}", self.seed);
        }
        if accepted {
            // G11: what the answer said it would do is what it did, at once.
            let Some(Event::AnswerRecorded { effects, .. }) =
                self.log.iter().rev().find(|e| matches!(e, Event::AnswerRecorded { .. })).cloned()
            else {
                panic!("seed {}: G11 no answer recorded", self.seed)
            };
            let start = self.log.iter().rposition(|e| matches!(e, Event::AnswerRecorded { .. })).expect("answer");
            let rerun: BTreeSet<String> = self.log[start..]
                .iter()
                .filter_map(|e| match e {
                    Event::StepInvalidated { step, because, .. }
                        if because.iter().any(|b| b.contains("sent it back") || b.contains("being done again")) =>
                    {
                        Some(step.clone())
                    }
                    _ => None,
                })
                .collect();
            let said: BTreeSet<String> = effects.reruns.iter().cloned().collect();
            assert_eq!(rerun, said, "seed {}: G11 the answer said {said:?} run again, and {rerun:?} did", self.seed);
            for m in &effects.may_rerun {
                let k = run.manifest.index(m).expect("step");
                assert!(
                    effects.reruns.iter().any(|r| run.dependents(run.manifest.index(r).expect("step")).contains(&k)),
                    "seed {}: G11 {m} named as maybe, and it uses none of what runs again", self.seed
                );
            }
        }
    }

    fn act(&mut self) {
        let run = self.run().clone();
        if run.finished() {
            return;
        }
        let roll = self.rng.below(100);
        if roll < 55 && !self.commands.is_empty() {
            let k = self.rng.below(self.commands.len());
            let c = self.commands.remove(k);
            self.respond(c);
        } else if roll < 75 {
            self.answer();
        } else if roll < 78 {
            // the shell stops and starts again: what it was doing is lost
            self.commands.clear();
            self.send(Input::Recovered);
        } else if roll < 82 {
            // a late report from an older attempt
            if let Some((i, s)) = run.steps.iter().enumerate().find(|(_, s)| s.attempt > 1) {
                let step = run.manifest.steps[i].name.clone();
                let before = self.log.len();
                self.send(Input::AttemptEnded {
                    step,
                    attempt: s.attempt - 1,
                    fix: 0,
                    outcome: AttemptOutcome::Delivered { output: Digest("late".to_string()), items: Vec::new(), changed: Vec::new(), usage: Usage::default(), session: None },
                });
                assert!(self.log.len() <= before + 1, "seed {}: G6 a late report did more than being recorded", self.seed);
            }
        } else if roll < 86 {
            if run.blocked() {
                let by = if self.rng.chance(80) { By::Person } else { By::Agent { who: "claude".to_string() } };
                self.send(Input::Continue { by });
            } else {
                self.send(Input::Tick);
            }
        } else if roll < 88 {
            let by = if self.rng.chance(70) { By::Person } else { By::Agent { who: "claude".to_string() } };
            let accepted = self.send(Input::RaiseRisk { to: Risk::High, by: by.clone() });
            assert!(!accepted || by.person(), "seed {}: G7 an agent raised the risk", self.seed);
        } else if roll < 90 {
            let accepted = self.send(Input::RaiseRisk { to: Risk::Low, by: By::Agent { who: "claude".to_string() } });
            assert!(!accepted, "seed {}: G7 an agent lowered the risk", self.seed);
        } else if roll < 92 {
            if run.halted {
                self.send(Input::Resume { by: By::Person });
            } else {
                self.send(Input::Halt { by: By::Agent { who: "claude".to_string() } });
            }
        } else if roll < 94 {
            let c = run.ceiling;
            self.send(Input::RaiseCeiling {
                ceiling: jaynes_core::risk::Ceiling { seconds: c.seconds + 600, tokens: c.tokens + 1_000_000 },
                by: By::Person,
            });
        } else {
            self.send(Input::Tick);
        }
    }

    /// G6: every attempt ended at most once.
    fn check_ends(&self) {
        for ((step, attempt), n) in &self.ended {
            assert!(*n <= 1, "seed {}: G6 attempt {attempt} of {step} ended {n} times", self.seed);
        }
    }
}

fn simulate(seed: u64) {
    let mut w = World::new(seed);
    let manifest = role(&mut w.rng);
    let raise = if w.rng.chance(20) { Some(Risk::High) } else { None };
    let started = w.send(Input::Start {
        run: format!("run-{seed}"),
        request: "change the button's text".to_string(),
        manifest: manifest.clone(),
        role_fingerprint: Digest("role".to_string()),
        raise,
    });
    assert!(started, "seed {seed}: a valid role did not start");
    // G16: the run says what it started on, and does not start twice.
    let Some(Event::RunStarted { engine, protocol, format, .. }) = w.log.first() else { panic!("no start") };
    assert!(!engine.is_empty() && *protocol == 1 && *format == 1);
    assert!(decide(w.run.as_ref(), &Input::Start {
        run: "again".to_string(),
        request: String::new(),
        manifest,
        role_fingerprint: Digest("other".to_string()),
        raise: None
    })
    .is_err());
    for _ in 0..400 {
        w.act();
        if w.run().finished() {
            break;
        }
    }
    w.check_ends();
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn every_guarantee_holds_on_random_runs(seed in any::<u64>()) {
        simulate(seed);
    }

    /// G8: a riskier run takes every step a less risky one takes.
    #[test]
    fn g8_riskier_paths_contain_safer_ones(seed in any::<u64>()) {
        let m = role(&mut Rng(seed | 1));
        let low: BTreeSet<usize> = m.path(Risk::Low).into_iter().collect();
        let high: BTreeSet<usize> = m.path(Risk::High).into_iter().collect();
        prop_assert!(low.is_subset(&high));
    }
}

// --- G15: the record's chain --------------------------------------------------------------------------------

fn chained(bodies: &[&str]) -> Vec<(String, Digest)> {
    let mut prev = chain::genesis();
    bodies
        .iter()
        .map(|b| {
            prev = chain::link(&prev, b);
            (b.to_string(), prev.clone())
        })
        .collect()
}

fn first_break(entries: &[(String, Digest)]) -> Option<usize> {
    chain::first_break(entries.iter().map(|(b, d)| (b.as_str(), d)))
}

#[test]
fn g15_an_untouched_record_holds() {
    assert_eq!(first_break(&chained(&["a", "b", "c"])), None);
}

#[test]
fn g15_a_changed_entry_shows() {
    let mut e = chained(&["a", "b", "c"]);
    e[1].0 = "B".to_string();
    assert_eq!(first_break(&e), Some(1));
}

#[test]
fn g15_a_dropped_or_moved_entry_shows() {
    let mut e = chained(&["a", "b", "c"]);
    e.remove(1);
    assert_eq!(first_break(&e), Some(1));
    let mut e = chained(&["a", "b", "c"]);
    e.swap(0, 1);
    assert_eq!(first_break(&e), Some(0));
}

#[test]
fn g15_rewriting_every_fingerprint_needs_the_whole_chain() {
    // A forger who changes an entry and recomputes the chain from there changes every later fingerprint, so a
    // copy of any later fingerprint kept elsewhere shows it.
    let honest = chained(&["a", "b", "c"]);
    let forged = chained(&["a", "B", "c"]);
    assert_ne!(honest[2].1, forged[2].1);
}

// --- a few runs written out, so the behaviour reads as a story -------------------------------------------------

fn step(name: &str, executor: Executor) -> StepDecl {
    StepDecl {
        name: name.to_string(),
        needs: Vec::new(),
        executor,
        session: SessionKind::Fresh,
        reach: Reach::default(),
        effect: Effect::None,
        checks: Vec::new(),
        stop: None,
        from_risk: Risk::Low,
        attempts: 3,
        fixes: 2,
        hidden: false,
        about: String::new(),
    }
}

fn start(steps: Vec<StepDecl>) -> (Run, Vec<Command>) {
    let manifest = Manifest { role: "story".to_string(), version: "1".to_string(), protocol: 1, steps };
    let out = decide(None, &Input::Start {
        run: "r".to_string(),
        request: "do it".to_string(),
        manifest,
        role_fingerprint: Digest("f".to_string()),
        raise: None,
    })
    .expect("start");
    (Run::fold(&out.events).expect("run"), out.commands)
}

fn feed(run: &mut Run, input: Input) -> Vec<Command> {
    let out = decide(Some(run), &input).unwrap_or_else(|r| panic!("refused {input:?}: {r}"));
    for e in &out.events {
        run.apply(e);
    }
    out.commands
}

fn probe_ok(run: &mut Run, cmds: Vec<Command>) -> Vec<Command> {
    let mut out = Vec::new();
    for c in cmds {
        if let Command::Probe { step, .. } = c {
            out.extend(feed(run, Input::ProbeEnded { step, ok: true, reason: String::new() }));
        } else {
            out.push(c);
        }
    }
    out
}

fn deliver(run: &mut Run, step: &str, attempt: u32, output: &str, items: Vec<Item>) -> Vec<Command> {
    feed(run, Input::AttemptEnded {
        step: step.to_string(),
        attempt,
        fix: 0,
        outcome: AttemptOutcome::Delivered {
            output: Digest(output.to_string()),
            items,
            changed: Vec::new(),
            usage: Usage { seconds: 5, tokens: 100, cached: 0 },
            session: Some("s".to_string()),
        },
    })
}

fn item(id: &str, d: &str) -> Item {
    Item { id: id.to_string(), label: id.to_string(), digest: Digest(d.to_string()) }
}

#[test]
fn g13_a_correction_reruns_only_what_used_it() {
    // look → copy (uses look) and tests (uses nothing): changing look reruns copy, not tests.
    let mut look = step("look", Executor::Agent);
    look.stop = Some(StopDecl { verdicts: vec![VerdictKind::Keep, VerdictKind::Redo], holds: false, from_risk: Risk::Low, about: String::new() });
    let mut copy = step("copy", Executor::Agent);
    copy.needs = vec!["look".to_string()];
    let tests = step("tests", Executor::Agent);
    let (mut run, cmds) = start(vec![look, copy, tests]);
    probe_ok(&mut run, cmds);
    let asked = deliver(&mut run, "look", 1, "blue", vec![item("palette", "blue")]);
    let q = asked.iter().find_map(|c| if let Command::Ask { question } = c { Some(question.clone()) } else { None }).expect("asked");
    // the run did not wait for the answer: copy started
    assert!(matches!(run.steps[1].status, Status::Probing));
    probe_ok(&mut run, asked.clone());
    let c = deliver(&mut run, "copy", 1, "copy-blue", Vec::new());
    probe_ok(&mut run, c);
    let c = deliver(&mut run, "tests", 1, "tests-1", Vec::new());
    let Some(Command::Deliver { digest, .. }) = c.into_iter().find(|c| matches!(c, Command::Deliver { .. })) else { panic!("delivers") };
    feed(&mut run, Input::DeliveryEnded { digest, ok: true, reference: "jaynes/r".to_string() });
    assert!(matches!(run.phase, Phase::Waiting { step: None, .. }), "delivered, the question still open: {:?}", run.phase);
    // the person sends look back after the run went on
    let answer = Answer {
        question: q.id.clone(),
        shown: q.shown(),
        verdicts: [("palette".to_string(), Verdict::Redo { note: "greener".to_string() })].into_iter().collect(),
        by: By::Person,
    };
    let effects = jaynes_core::effects(&run, &q, &answer);
    assert_eq!(effects.reruns, vec!["look".to_string()]);
    assert_eq!(effects.may_rerun, vec!["copy".to_string()]);
    let c = feed(&mut run, Input::Answer { answer });
    probe_ok(&mut run, c);
    deliver(&mut run, "look", 2, "green", vec![item("palette", "green")]);
    let rounds: Vec<u32> = run.steps.iter().map(|s| s.round).collect();
    assert_eq!(rounds, vec![1, 1, 0], "copy runs again, tests does not");
}

#[test]
fn g14_an_irreversible_step_waits_for_the_answer_it_rests_on() {
    let mut plan = step("plan", Executor::Agent);
    plan.stop = Some(StopDecl { verdicts: vec![VerdictKind::Keep], holds: false, from_risk: Risk::Low, about: String::new() });
    let mut push = step("push", Executor::Role);
    push.needs = vec!["plan".to_string()];
    push.effect = Effect::Irreversible { idempotent: true };
    let (mut run, cmds) = start(vec![plan, push]);
    // the role has a step that cannot be undone, so the run takes the full path
    assert_eq!(run.risk, Risk::High);
    probe_ok(&mut run, cmds);
    let asked = deliver(&mut run, "plan", 1, "p", Vec::new());
    let q = asked.iter().find_map(|c| if let Command::Ask { question } = c { Some(question.clone()) } else { None }).expect("asked");
    assert!(matches!(run.phase, Phase::Waiting { .. }), "push waits: {:?}", run.phase);
    let c = feed(&mut run, Input::Answer { answer: Answer { question: q.id.clone(), shown: q.shown(), verdicts: BTreeMap::new(), by: By::Person } });
    let c = probe_ok(&mut run, c);
    assert!(c.iter().any(|c| matches!(c, Command::RunRole { key: Some(_), .. })), "push runs once answered, with a key");
}

#[test]
fn g4_a_box_that_refuses_spends_no_attempt() {
    let (mut run, cmds) = start(vec![step("build", Executor::Agent)]);
    let Some(Command::Probe { step, .. }) = cmds.first().cloned() else { panic!("probe first: {cmds:?}") };
    feed(&mut run, Input::ProbeEnded { step, ok: false, reason: "the box refused /tmp".to_string() });
    assert!(matches!(run.phase, Phase::Blocked { fault: Fault::Jaynes, .. }));
    assert_eq!(run.counts.attempts, 0, "no attempt even started");
}

#[test]
fn a_failed_shape_check_is_fixed_in_the_same_session() {
    let mut build = step("build", Executor::Agent);
    build.checks = vec![CheckDecl { name: "short".to_string(), kind: CheckKind::Shape, about: "at most 80 characters".to_string() }];
    let (mut run, cmds) = start(vec![build]);
    probe_ok(&mut run, cmds);
    deliver(&mut run, "build", 1, "long", Vec::new());
    let c = feed(&mut run, Input::ChecksEnded {
        step: "build".to_string(),
        attempt: 1,
        fix: 0,
        results: vec![CheckResult { check: "short".to_string(), verdict: CheckVerdict::Fail { reasons: vec!["93 characters".to_string()], detail: String::new() } }],
    });
    let Some(Command::RunAgent(turn)) = c.first() else { panic!("a fix: {c:?}") };
    assert_eq!((turn.attempt, turn.fix, turn.session.as_deref()), (1, 1, Some("s")));
    assert_eq!(run.counts.spent, 0, "a fix in place spends no attempt");
}

#[test]
fn g5_the_ceiling_stops_new_attempts() {
    let (mut run, cmds) = start(vec![step("build", Executor::Agent)]);
    probe_ok(&mut run, cmds);
    feed(&mut run, Input::AttemptEnded {
        step: "build".to_string(),
        attempt: 1,
        fix: 0,
        outcome: AttemptOutcome::Failed { reasons: vec!["slow".to_string()], usage: Usage { seconds: 99_999, tokens: 1, cached: 0 } },
    });
    assert!(matches!(run.phase, Phase::Blocked { fault: Fault::Ceiling, .. }), "{:?}", run.phase);
}

#[test]
fn g7_touching_ci_raises_the_risk_and_owes_the_full_path() {
    let mut review = step("review", Executor::Agent);
    review.from_risk = Risk::High;
    let mut change = step("change", Executor::Agent);
    change.reach = Reach { project: ProjectAccess::Write, write: vec!["**".to_string()], ..Reach::default() };
    let (mut run, cmds) = start(vec![review, change]);
    assert_eq!(run.risk, Risk::Low);
    probe_ok(&mut run, cmds);
    let c = feed(&mut run, Input::AttemptEnded {
        step: "change".to_string(),
        attempt: 1,
        fix: 0,
        outcome: AttemptOutcome::Delivered {
            output: Digest("o".to_string()),
            items: Vec::new(),
            changed: vec![Changed { path: ".github/workflows/ci.yml".to_string(), lines: 1 }],
            usage: Usage::default(),
            session: None,
        },
    });
    assert_eq!(run.risk, Risk::High);
    assert!(c.iter().any(|c| matches!(c, Command::Probe { step, .. } if step == "review")), "the owed step runs: {c:?}");
}

#[test]
fn g9_a_routine_leave_is_granted_by_a_rule_and_recorded() {
    let mut build = step("build", Executor::Agent);
    build.reach = Reach { project: ProjectAccess::Read, ..Reach::default() };
    let (mut run, cmds) = start(vec![build]);
    probe_ok(&mut run, cmds);
    let c = feed(&mut run, Input::PermissionAsked {
        step: "build".to_string(),
        attempt: 1,
        ask: Ask { kind: AskKind::Read, target: "src/lib.rs".to_string() },
    });
    assert!(c.iter().any(|c| matches!(c, Command::Permit { allow: true, .. })));
    let q = run.questions.values().next().expect("recorded");
    assert_eq!(q.answer.as_ref().map(|a| a.by.clone()), Some(By::Rule { rule: READ_INSIDE_PROJECT.to_string() }));
    assert!(!routine(&ProjectAccess::Read, &Ask { kind: AskKind::Read, target: "../secrets".to_string() }));
    assert!(!routine(&ProjectAccess::Read, &Ask { kind: AskKind::Network, target: "example.com".to_string() }));
}

#[test]
fn g16_a_role_that_breaks_the_rules_is_refused_before_it_runs() {
    let mut a = step("a", Executor::Agent);
    a.needs = vec!["b".to_string()];
    let b = step("b", Executor::Agent);
    let manifest = Manifest { role: "bad".to_string(), version: "1".to_string(), protocol: 1, steps: vec![a, b] };
    let refused = decide(None, &Input::Start { run: "r".to_string(), request: String::new(), manifest, role_fingerprint: Digest::default(), raise: None });
    assert!(refused.is_err());
}

#[allow(dead_code)]
fn unused(_: QuestionKind) {}

/// The random runs must reach the states the guarantees are about, or passing them proves nothing.
#[test]
fn the_random_runs_reach_what_the_guarantees_are_about() {
    let mut seen: BTreeMap<&'static str, u32> = BTreeMap::new();
    for seed in 1..=600u64 {
        let mut w = World::new(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let manifest = role(&mut w.rng);
        w.send(Input::Start { run: "r".to_string(), request: String::new(), manifest, role_fingerprint: Digest::default(), raise: None });
        for _ in 0..400 {
            w.act();
            if w.run().finished() {
                break;
            }
        }
        let mut hit = |k: &'static str| *seen.entry(k).or_default() += 1;
        if w.run().finished() {
            hit("finished");
        }
        for e in &w.log {
            match e {
                Event::StepInvalidated { because, .. } if because.iter().any(|b| b.contains("came out different")) => hit("rerun because an input changed"),
                Event::StepInvalidated { .. } => hit("sent back"),
                Event::StaleReport { .. } => hit("late report"),
                Event::FixAsked { .. } => hit("fix in place"),
                Event::RiskRaised { by: By::Rule { .. }, .. } => hit("risk raised by a rule"),
                Event::Waiting { why, .. } if why.contains("cannot be undone") => hit("waits before what cannot be undone"),
                Event::Blocked { fault: Fault::Jaynes, .. } => hit("fault of Jaynes"),
                Event::AnswerRecorded { answer, .. } if matches!(answer.by, By::Rule { .. }) => hit("leave granted by a rule"),
                Event::QuestionSuperseded { .. } => hit("question superseded"),
                Event::Delivered { .. } => hit("delivered"),
                _ => {}
            }
        }
    }
    eprintln!("{seen:#?}");
    for k in [
        "finished", "rerun because an input changed", "sent back", "late report", "fix in place", "risk raised by a rule",
        "waits before what cannot be undone", "fault of Jaynes", "leave granted by a rule", "question superseded", "delivered",
    ] {
        assert!(seen.get(k).copied().unwrap_or(0) >= 5, "the random runs reached {k:?} only {:?} times", seen.get(k));
    }
}

/// `SEED=… cargo test -p jaynes-core --test guarantees replay -- --ignored --nocapture` prints a seed's record.
#[test]
#[ignore]
fn replay() {
    let seed: u64 = std::env::var("SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let r = std::panic::catch_unwind(|| simulate_logged(seed));
    if let Err(e) = r {
        std::panic::resume_unwind(e);
    }
}

fn simulate_logged(seed: u64) {
    let mut w = World::new(seed);
    let manifest = role(&mut w.rng);
    eprintln!("{}", serde_json::to_string_pretty(&manifest).expect("json"));
    let raise = if w.rng.chance(20) { Some(Risk::High) } else { None };
    w.send(Input::Start { run: format!("run-{seed}"), request: String::new(), manifest, role_fingerprint: Digest("role".to_string()), raise });

    let mut printed = 0;
    for _ in 0..400 {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.act()));
        for e in &w.log[printed..] {
            eprintln!("{}", serde_json::to_string(e).expect("json"));
        }
        printed = w.log.len();
        if let Err(e) = r {
            std::panic::resume_unwind(e);
        }
        if w.run().finished() {
            break;
        }
    }
}
