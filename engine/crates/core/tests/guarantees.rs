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
use jaynes_core::attention::{stop_key, Count};
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
                    let kind = if rng.chance(50) { CheckKind::Open } else { CheckKind::Hidden };
                    let name = match kind {
                        CheckKind::Open => format!("shapecheck{i}x{k}"),
                        CheckKind::Hidden => format!("hiddencheck{i}x{k}"),
                    };
                    // a hidden check sometimes reads an earlier hidden step, as a proof reads its cases
                    let reads: Vec<String> = match kind {
                        CheckKind::Hidden => steps.iter().filter(|s| s.hidden && rng.chance(70)).map(|s| s.name.clone()).collect(),
                        CheckKind::Open => Vec::new(),
                    };
                    checks.push(CheckDecl { name, kind, about: format!("holds {k}"), reads });
                }
            }
            let stop = if executor == Executor::Person || rng.chance(45) {
                let mut verdicts: Vec<VerdictKind> = ALL_VERDICTS.iter().copied().filter(|_| rng.chance(60)).collect();
                if !verdicts.contains(&VerdictKind::Keep) {
                    verdicts.insert(0, VerdictKind::Keep);
                }
                let stop_risk = if from_risk == Risk::High || rng.chance(20) { Risk::High } else { Risk::Low };
                let release = executor != Executor::Person && rng.chance(40);
                let budget = (executor != Executor::Person && rng.chance(50)).then_some(jaynes_core::manifest::Rate(0.2));
                Some(StopDecl { verdicts, holds: rng.chance(30), from_risk: stop_risk, release, budget, audit_rate: jaynes_core::manifest::Rate(0.3), about: String::new() })
            } else {
                None
            };
            let effect = match (executor, rng.below(8)) {
                (Executor::Role, 0) => Effect::Irreversible { idempotent: rng.chance(50) },
                (Executor::Agent | Executor::Role, 1) => Effect::Confined,
                _ => Effect::None,
            };
            let hidden = executor == Executor::Agent && needs.is_empty() && project != ProjectAccess::Write && rng.chance(20);
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
        let delivery = if rng.chance(30) { jaynes_core::manifest::DeliveryKind::Merge } else { jaynes_core::manifest::DeliveryKind::Branch };
        let m = Manifest { role: "random".to_string(), version: "1".to_string(), protocol: 1, steps, delivery };
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
    Manifest { role: "random".to_string(), version: "1".to_string(), protocol: 1, steps: vec![one], delivery: Default::default() }
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
    /// When set, every state the run reaches, as the Lean model reads it, with the move the core makes from it.
    cases: Option<Vec<String>>,
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
            cases: None,
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
                // Every state the core passed through on the way, not only where it stopped: it often takes a
                // move from a state it leaves in the same decision.
                if let (Some(cases), Some(pre)) = (self.cases.as_mut(), before.as_ref())
                    && let Some(c) = gate_case(pre, &input, &out.events) {
                        cases.push(c);
                    }
                if let Some(cases) = self.cases.as_mut() {
                    let mut state = before.clone();
                    for e in &out.events {
                        state = match state {
                            None => Run::started(e),
                            Some(mut r) => {
                                r.apply(e);
                                Some(r)
                            }
                        };
                        if let Some(r) = state.as_ref() {
                            cases.push(model_case(r));
                        }
                    }
                }
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
                        assert!(!rests_on_unconfirmed(&pre, i), "seed {seed}: G14 {step} started on answers you did not give");
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
                Event::StopSkipped { step, round, coin, .. } => {
                    // G17: a stop skips asking only when certified, not braked, and its coin falls outside its sample.
                    let i = pre.manifest.index(step).expect("step");
                    let stop = pre.manifest.steps[i].stop.clone().expect("a stop");
                    let c = pre.attention.get(step).expect("a count");
                    assert_eq!(c.key, stop_key(&pre.manifest, &pre.role_fingerprint, i), "seed {seed}: G17 a count of another stop was used");
                    assert!(!c.braked, "seed {seed}: G17 a braked stop skipped");
                    let b = stop.budget.expect("a budget").0;
                    assert!(jaynes_core::attention::certified(c.n, c.k, b, jaynes_core::DELTA), "seed {seed}: G17 an uncertified stop skipped");
                    assert!(coin.0 >= stop.audit_rate.0, "seed {seed}: G17 a sampled run skipped");
                    assert_eq!(coin.0, jaynes_core::attention::coin(&pre.id, step, *round));
                }
                Event::QuestionOpened { question } if question.kind == QuestionKind::Decision => {
                    // G17: a braked stop holds the run.
                    if pre.attention.get(&question.step).is_some_and(|c| c.braked) {
                        assert!(question.holds, "seed {seed}: G17 a braked stop did not hold");
                    }
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
                        By::Rule { rule } if rule == jaynes_core::decide::ANSWERED_BEFORE => {
                            // only the person's own answer to the same leave, given again
                            let digest = &q.items[0].digest;
                            let earlier = pre.questions.values().filter(|o| o.question.kind == QuestionKind::Permission && &o.question.items[0].digest == digest).filter(|o| o.answer.as_ref().is_some_and(|a| a.by.person())).max_by_key(|o| jaynes_core::decide::asked_order(&o.question.id)).and_then(|o| o.answer.as_ref());
                            assert!(q.kind == QuestionKind::Permission && earlier.is_some_and(|a| a.verdict("leave") == answer.verdict("leave")), "seed {seed}: G9 a leave answered again without the person's earlier answer");
                        }
                        By::Rule { rule } => {
                            assert!(rule == READ_INSIDE_PROJECT || rule == jaynes_core::decide::COMMAND_THE_STEP_MAY_RUN, "seed {seed}: G9 unknown rule");
                            assert_eq!(q.kind, QuestionKind::Permission, "seed {seed}: G9 a rule answered a decision");
                        }
                        By::Agent { .. } => {
                            let i = pre.manifest.index(&q.step).expect("step");
                            let d = &pre.manifest.steps[i];
                            assert!(
                                q.kind == QuestionKind::Decision
                                    && d.executor != Executor::Person
                                    && d.stop.as_ref().is_some_and(|s| s.release)
                                    && !pre.steps[i].inputs.contains(&q.id)
                                    && !pre.attention.get(&q.step).is_some_and(|c| c.braked)
                                    && answer.verdicts.values().all(|v| *v == Verdict::Keep),
                                "seed {seed}: G9 an agent answered what is yours: {answer:?}"
                            );
                        }
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
                    assert!(turn.checks.iter().all(|c| c.kind == CheckKind::Open), "seed {seed}: G3 a hidden check was given");
                    let text = serde_json::to_string(turn).expect("json");
                    for d in &post.manifest.steps {
                        for ch in d.checks.iter().filter(|c| c.kind == CheckKind::Hidden) {
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
            && let Input::AttemptEnded { outcome: AttemptOutcome::Delivered { usage, .. } | AttemptOutcome::Failed { usage, .. } | AttemptOutcome::Impossible { usage, .. }, .. } = input {
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
    used.extend(decl.checks.iter().flat_map(|c| c.reads.iter().cloned()));
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

/// Whether step `i` rests, through what it uses, on a question of the current round that the person has not
/// answered: open, or answered only by an agent. Worked out here, apart from the core.
fn rests_on_unconfirmed(run: &Run, i: usize) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![i];
    while let Some(k) = stack.pop() {
        let decl = &run.manifest.steps[k];
        let mut used: Vec<usize> = decl.needs.iter().chain(decl.checks.iter().flat_map(|c| c.reads.iter())).filter_map(|n| run.manifest.index(n)).collect();
        if decl.reach.project != ProjectAccess::None {
            used.extend(run.manifest.path(run.risk).into_iter().filter(|&j| j < k && run.manifest.steps[j].reach.project == ProjectAccess::Write));
        }
        for j in used {
            if !seen.insert(j) {
                continue;
            }
            let s = &run.steps[j];
            let pending = run.questions.values().any(|q| {
                q.question.step == run.manifest.steps[j].name
                    && q.question.kind == QuestionKind::Decision
                    && !q.superseded
                    && s.question.as_deref() == Some(q.question.id.as_str())
                    && q.answer.as_ref().is_none_or(|a| !a.by.person())
            });
            if pending {
                return true;
            }
            stack.push(j);
        }
    }
    false
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
                    let kind = match self.rng.below(10) { 0..=6 => AskKind::Read, 7 => AskKind::Command, _ => AskKind::Network };
                    self.send(Input::PermissionAsked { step: turn.step.clone(), attempt: turn.attempt, ask: Ask { kind, target } });
                }
                if self.rng.chance(50) {
                    self.send(Input::SessionStarted { step: turn.step.clone(), attempt: turn.attempt, session: format!("session-{}", turn.step) });
                }
                let outcome = match self.rng.below(100) {
                    0..=71 => AttemptOutcome::Delivered {
                        output: digest_pool(&mut self.rng),
                        items: items_for(&mut self.rng, &run, &turn.step),
                        changed: if turn.reach.project == ProjectAccess::Write { changed(&mut self.rng) } else { Vec::new() },
                        usage: usage(&mut self.rng),
                        session: Some(format!("session-{}", turn.step)),
                    },
                    72..=85 => AttemptOutcome::Failed { reasons: vec!["no output".to_string()], usage: usage(&mut self.rng) },
                    86..=88 => AttemptOutcome::Impossible { reason: "the request contradicts itself".to_string(), usage: usage(&mut self.rng), session: Some(format!("session-{}", turn.step)) },
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
            0..=2 => {
                if self.rng.chance(60) {
                    verdicts.clear();
                }
                By::Agent { who: "claude".to_string() }
            }
            3 => By::Rule { rule: READ_INSIDE_PROJECT.to_string() },
            _ => By::Person,
        };
        let state = run.questions[&q.id].clone();
        fn answer_keeps_all(a: &Answer) -> bool {
            a.verdicts.values().all(|v| *v == Verdict::Keep)
        }
        let answer = Answer { question: q.id.clone(), shown, verdicts, by: by.clone() , context: String::new()};
        let accepted = self.send(Input::Answer { answer: answer.clone() });
        if let Some(cases) = self.cases.as_mut() {
            cases.push(answer_case(&run, &answer, accepted));
            if accepted {
                let i = run.manifest.index(&q.step).expect("step");
                let person = run.manifest.steps[i].executor == Executor::Person;
                let items: Vec<serde_json::Value> = q
                    .items
                    .iter()
                    .map(|x| serde_json::json!({"back": answer.verdict(&x.id).sends_back(), "standsFor": if person { run.manifest.index(&x.id) } else { None }}))
                    .collect();
                let case = serde_json::json!({
                    "step": i, "personStep": person, "input": run.steps[i].inputs.contains(&q.id),
                    "permission": q.kind == QuestionKind::Permission, "items": items,
                });
                let show = |mut v: Vec<usize>| {
                    v.sort_unstable();
                    v.dedup();
                    v.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(",")
                };
                let start = self.log.iter().rposition(|e| matches!(e, Event::AnswerRecorded { .. })).expect("recorded");
                let Event::AnswerRecorded { effects, .. } = &self.log[start] else { unreachable!() };
                let said: Vec<usize> = effects.reruns.iter().filter_map(|s| run.manifest.index(s)).collect();
                let did: Vec<usize> = self.log[start..]
                    .iter()
                    .filter_map(|e| match e {
                        Event::StepInvalidated { step, because, .. }
                            if because.iter().any(|b| b.contains("sent it back") || b.contains("being done again")) => run.manifest.index(step),
                        _ => None,
                    })
                    .collect();
                let expected = format!("{}|{}", show(said), show(did));
                if let Some(cases) = self.cases.as_mut() {
                    cases.push(serde_json::json!({"kind": "effects", "case": case, "expected": expected}).to_string());
                }
            }
        }
        // G9 and G10: only for what was shown, only while it stands; an agent only to let a release go on.
        let i = run.manifest.index(&q.step).expect("step");
        let agent_may = q.kind == QuestionKind::Decision
            && run.manifest.steps[i].executor != Executor::Person
            && run.manifest.steps[i].stop.as_ref().is_some_and(|s| s.release)
            && !run.steps[i].inputs.contains(&q.id)
            && !run.attention.get(&q.step).is_some_and(|c| c.braked)
            && answer_keeps_all(&answer)
            && state.answer.is_none();
        let not_allowed = match &by {
            By::Person => false,
            By::Agent { .. } => !agent_may,
            By::Rule { .. } => true,
        };
        if wrong || not_allowed || state.superseded {
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

/// Start a random role, with random counts for its stops, some of another stop.
fn start_random(w: &mut World, seed: u64) -> (Manifest, bool) {
    let manifest = role(&mut w.rng);
    let raise = if w.rng.chance(20) { Some(Risk::High) } else { None };
    let fingerprint = Digest("role".to_string());
    let mut attention = BTreeMap::new();
    for (i, s) in manifest.steps.iter().enumerate() {
        if s.stop.is_none() || !w.rng.chance(60) {
            continue;
        }
        // a count of this stop, or of another one (a wrong key the core must ignore)
        let key = if w.rng.chance(85) { stop_key(&manifest, &fingerprint, i) } else { Digest("another stop".to_string()) };
        let (n, k) = match w.rng.below(3) { 0 => (40, 0), 1 => (30, 12), _ => (5, 0) };
        attention.insert(s.name.clone(), Count { key, n, k, pending: 0, braked: w.rng.chance(25) });
    }
    let started = w.send(Input::Start {
        run: format!("run-{seed}"),
        request: "change the button's text".to_string(),
        manifest: manifest.clone(),
        role_fingerprint: fingerprint,
        agent: "claude:m1".to_string(),
        raise,
        attention,
    });
    (manifest, started)
}

fn simulate(seed: u64) {
    let mut w = World::new(seed);
    let (manifest, started) = start_random(&mut w, seed);
    assert!(started, "seed {seed}: a valid role did not start");
    // G16: the run says what it started on, and does not start twice.
    let Some(Event::RunStarted { engine, protocol, format, .. }) = w.log.first() else { panic!("no start") };
    assert!(!engine.is_empty() && *protocol == 1 && *format == 1);
    assert!(decide(w.run.as_ref(), &Input::Start {
        run: "again".to_string(),
        request: String::new(),
        manifest,
        role_fingerprint: Digest("other".to_string()),
        agent: "claude:m1".to_string(), raise: None,
        attention: BTreeMap::new()
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
    let manifest = Manifest { role: "story".to_string(), version: "1".to_string(), protocol: 1, steps, delivery: Default::default() };
    let out = decide(None, &Input::Start {
        run: "r".to_string(),
        request: "do it".to_string(),
        manifest,
        role_fingerprint: Digest("f".to_string()),
        agent: "claude:m1".to_string(), raise: None,
        attention: BTreeMap::new(),
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
    look.stop = Some(StopDecl { verdicts: vec![VerdictKind::Keep, VerdictKind::Redo], holds: false, from_risk: Risk::Low, release: false, budget: None, audit_rate: jaynes_core::manifest::Rate(0.1), about: String::new() });
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
    let answer = Answer { context: String::new(),
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
    plan.stop = Some(StopDecl { verdicts: vec![VerdictKind::Keep], holds: false, from_risk: Risk::Low, release: false, budget: None, audit_rate: jaynes_core::manifest::Rate(0.1), about: String::new() });
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
    let c = feed(&mut run, Input::Answer { answer: Answer { question: q.id.clone(), shown: q.shown(), verdicts: BTreeMap::new(), by: By::Person , context: String::new()} });
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
    build.checks = vec![CheckDecl { name: "short".to_string(), kind: CheckKind::Open, about: "at most 80 characters".to_string(), reads: Vec::new() }];
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
    let reads = Reach { project: ProjectAccess::Read, ..Reach::default() };
    assert_eq!(routine(&reads, &Ask { kind: AskKind::Read, target: "../secrets".to_string() }), None);
    assert_eq!(routine(&reads, &Ask { kind: AskKind::Network, target: "example.com".to_string() }), None);
    assert_eq!(routine(&reads, &Ask { kind: AskKind::Command, target: "make test".to_string() }), None, "a step that runs no commands");
    let runs = Reach { commands: true, ..reads };
    assert_eq!(routine(&runs, &Ask { kind: AskKind::Command, target: "make test".to_string() }), Some(jaynes_core::decide::COMMAND_THE_STEP_MAY_RUN));
}

#[test]
fn an_agent_that_says_the_request_cannot_be_done_asks_the_person_and_spends_no_attempt() {
    let (mut run, cmds) = start(vec![step("build", Executor::Agent)]);
    probe_ok(&mut run, cmds);
    let c = feed(&mut run, Input::AttemptEnded {
        step: "build".to_string(),
        attempt: 1,
        fix: 0,
        outcome: AttemptOutcome::Impossible {
            reason: "the tests ask for two answers that contradict each other".to_string(),
            usage: Usage { seconds: 30, tokens: 900, cached: 0 },
            session: Some("s1".to_string()),
        },
    });
    let q = c.iter().find_map(|c| match c { Command::Ask { question } => Some(question.clone()), _ => None }).expect("the person is asked");
    assert!(q.items[0].label.contains("contradict"), "{q:?}");
    assert!(matches!(run.phase, Phase::Waiting { .. }));
    assert_eq!((run.steps[0].spent, run.spent.tokens), (0, 900), "no attempt spent, the tokens counted");
    // an agent may not answer it
    let agent = Answer { question: q.id.clone(), shown: q.shown(), verdicts: [("impossible".to_string(), Verdict::Keep)].into_iter().collect(), by: By::Agent { who: "a".into() }, context: String::new() };
    assert!(decide(Some(&run), &Input::Answer { answer: agent }).is_err());
    // the person says how; the step tries again in the same session, with their words
    let c = feed(&mut run, Input::Answer {
        answer: Answer {
            question: q.id.clone(),
            shown: q.shown(),
            verdicts: [("impossible".to_string(), Verdict::Change { text: "keep the first answer".to_string() })].into_iter().collect(),
            by: By::Person,
            context: String::new(),
        },
    });
    let turn = c.iter().find_map(|c| match c { Command::RunAgent(t) => Some(t.clone()), _ => None }).expect("tried again");
    assert_eq!((turn.attempt, turn.session.as_deref()), (2, Some("s1")));
    assert_eq!(run.counts.sent_back, 0, "answering the agent's question is not work sent back");
    assert!(turn.notes.iter().any(|n| n.contains("keep the first answer")), "{:?}", turn.notes);
}

#[test]
fn an_attempt_cut_off_from_outside_goes_on_in_its_session_and_a_failed_one_starts_afresh() {
    let (mut run, cmds) = start(vec![step("build", Executor::Agent)]);
    probe_ok(&mut run, cmds);
    // the session is known as soon as the turn starts
    feed(&mut run, Input::SessionStarted { step: "build".to_string(), attempt: 1, session: "s1".to_string() });
    assert_eq!(run.steps[0].resume.as_deref(), Some("s1"));
    // a session of an attempt no longer under way changes nothing
    let late = decide(Some(&run), &Input::SessionStarted { step: "build".to_string(), attempt: 7, session: "old".to_string() }).expect("recorded as late");
    assert!(late.events.iter().all(|e| matches!(e, Event::StaleReport { .. })));
    // Jaynes dies mid-turn: the next attempt goes on in that session, and spends nothing
    let c = feed(&mut run, Input::Recovered);
    let c = probe_ok(&mut run, c);
    let turn = c.iter().find_map(|c| match c { Command::RunAgent(t) => Some(t.clone()), _ => None }).expect("again");
    assert_eq!((turn.attempt, turn.session.as_deref(), run.steps[0].spent), (2, Some("s1"), 0));
    // the agent's own failure: the next attempt starts afresh
    feed(&mut run, Input::SessionStarted { step: "build".to_string(), attempt: 2, session: "s1".to_string() });
    let c = feed(&mut run, Input::AttemptEnded { step: "build".to_string(), attempt: 2, fix: 0, outcome: AttemptOutcome::Failed { reasons: vec!["no output".to_string()], usage: Usage::default() } });
    let turn = c.iter().find_map(|c| match c { Command::RunAgent(t) => Some(t.clone()), _ => None }).expect("again");
    assert_eq!((turn.attempt, turn.session.as_deref()), (3, None));
}

#[test]
fn g9_a_leave_asked_again_gets_the_person_s_earlier_answer_without_asking_them() {
    let mut build = step("build", Executor::Agent);
    build.reach = Reach { project: ProjectAccess::Read, ..Reach::default() };
    let (mut run, cmds) = start(vec![build]);
    probe_ok(&mut run, cmds);
    let outside = Input::PermissionAsked { step: "build".to_string(), attempt: 1, ask: Ask { kind: AskKind::Read, target: "/etc/passwd".to_string() } };
    let c = feed(&mut run, outside.clone());
    assert!(c.iter().any(|c| matches!(c, Command::Ask { .. })), "the first time, the person is asked");
    let q = run.questions.values().next().expect("asked").question.clone();
    feed(&mut run, Input::Answer {
        answer: Answer { question: q.id.clone(), shown: q.shown(), verdicts: [("leave".to_string(), Verdict::Drop)].into_iter().collect(), by: By::Person, context: "not outside".to_string() },
    });
    let c = feed(&mut run, outside);
    assert!(c.iter().any(|c| matches!(c, Command::Permit { allow: false, .. })) && !c.iter().any(|c| matches!(c, Command::Ask { .. })), "{c:?}");
    let again = run.questions.values().find(|x| x.question.id != q.id).and_then(|x| x.answer.clone()).expect("answered");
    assert_eq!(again.by, By::Rule { rule: jaynes_core::decide::ANSWERED_BEFORE.to_string() });
    assert_eq!(again.context, "not outside");
    assert!(matches!(run.phase, Phase::Running), "nothing waits for the person: {:?}", run.phase);
}

#[test]
fn g16_a_role_that_breaks_the_rules_is_refused_before_it_runs() {
    let mut a = step("a", Executor::Agent);
    a.needs = vec!["b".to_string()];
    let b = step("b", Executor::Agent);
    let manifest = Manifest { role: "bad".to_string(), version: "1".to_string(), protocol: 1, steps: vec![a, b], delivery: Default::default() };
    let refused = decide(None, &Input::Start { run: "r".to_string(), request: String::new(), manifest, role_fingerprint: Digest::default(), agent: "claude:m1".to_string(), raise: None, attention: BTreeMap::new() });
    assert!(refused.is_err());
}

#[allow(dead_code)]
fn unused(_: QuestionKind) {}

/// The random runs must reach the states the guarantees are about, or passing them proves nothing.
#[test]
fn the_random_runs_reach_what_the_guarantees_are_about() {
    let mut seen: BTreeMap<&'static str, u32> = BTreeMap::new();
    for seed in 1..=1000u64 {
        let seed = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut w = World::new(seed);
        start_random(&mut w, seed);
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
                Event::AnswerRecorded { answer, .. } if matches!(answer.by, By::Agent { .. }) => hit("released by an agent"),
                Event::QuestionSuperseded { .. } => hit("question superseded"),
                Event::Delivered { .. } => hit("delivered"),
                Event::StopSkipped { .. } => hit("a certified stop skipped"),
                _ => {}
            }
        }
    }
    eprintln!("{seen:#?}");
    for k in [
        "finished", "rerun because an input changed", "sent back", "late report", "fix in place", "risk raised by a rule",
        "waits before what cannot be undone", "fault of Jaynes", "leave granted by a rule", "released by an agent", "a certified stop skipped", "question superseded", "delivered",
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
    w.send(Input::Start { run: format!("run-{seed}"), request: String::new(), manifest, role_fingerprint: Digest("role".to_string()), agent: "claude:m1".to_string(), raise, attention: BTreeMap::new() });

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

#[test]
fn g9_an_agent_may_release_a_marked_stop_and_nothing_irreversible_rests_on_it() {
    let mut change = step("change", Executor::Agent);
    change.stop = Some(StopDecl { verdicts: vec![VerdictKind::Keep, VerdictKind::Redo], holds: true, from_risk: Risk::Low, release: true, budget: None, audit_rate: jaynes_core::manifest::Rate(0.1), about: String::new() });
    let mut tests = step("tests", Executor::Agent);
    tests.needs = vec!["change".to_string()];
    let mut push = step("push", Executor::Role);
    push.needs = vec!["tests".to_string()];
    push.effect = Effect::Irreversible { idempotent: true };
    let (mut run, cmds) = start(vec![change, tests, push]);
    probe_ok(&mut run, cmds);
    let asked = deliver(&mut run, "change", 1, "c", Vec::new());
    let q = asked.iter().find_map(|c| if let Command::Ask { question } = c { Some(question.clone()) } else { None }).expect("asked");
    let agent = By::Agent { who: "claude".to_string() };
    // sending back is the person's
    let redo = Answer { context: String::new(), question: q.id.clone(), shown: q.shown(), verdicts: [("change".to_string(), Verdict::Redo { note: "no".to_string() })].into_iter().collect(), by: agent.clone() };
    assert!(decide(Some(&run), &Input::Answer { answer: redo }).is_err());
    // letting it go on is allowed, and the run goes on to tests
    let c = feed(&mut run, Input::Answer { answer: Answer { question: q.id.clone(), shown: q.shown(), verdicts: BTreeMap::new(), by: agent , context: String::new()} });
    let c = probe_ok(&mut run, c);
    assert!(c.iter().any(|c| matches!(c, Command::RunAgent(t) if t.step == "tests")), "{c:?}");
    assert!(run.steps[0].locks.is_empty(), "an agent's release locks nothing");
    let c = deliver(&mut run, "tests", 1, "t", Vec::new());
    // push cannot be undone, and rests on an answer only an agent gave: it waits for the person
    assert!(!c.iter().any(|c| matches!(c, Command::RunRole { .. } | Command::Probe { .. })), "{c:?}");
    assert!(matches!(&run.phase, Phase::Waiting { step: Some(s), .. } if s == "push"), "{:?}", run.phase);
    // the person confirms, and push runs
    let c = feed(&mut run, Input::Answer { answer: Answer { question: q.id.clone(), shown: q.shown(), verdicts: BTreeMap::new(), by: By::Person , context: String::new()} });
    let c = probe_ok(&mut run, c);
    assert!(c.iter().any(|c| matches!(c, Command::RunRole { step, .. } if step == "push")), "{c:?}");
}

// --- G17: a stop stops asking only on evidence ----------------------------------------------------------------

/// G17: if a stop's true error rate is at its budget, the chance it is ever made sampled is at most δ, however
/// often the test is checked. 3,000 streams of 300 outcomes each, checked after every outcome.
#[test]
fn g17_a_stop_at_its_budget_is_rarely_made_sampled() {
    let (budget, delta) = (0.2, jaynes_core::DELTA);
    let mut rng = Rng(0x5eed);
    let streams = 3000;
    let mut wrongly = 0;
    for _ in 0..streams {
        let (mut n, mut k) = (0u64, 0u64);
        for _ in 0..300 {
            n += 1;
            if (rng.next() % 10_000) < (budget * 10_000.0) as u64 {
                k += 1;
            }
            if jaynes_core::attention::certified(n, k, budget, delta) {
                wrongly += 1;
                break;
            }
        }
    }
    let rate = wrongly as f64 / streams as f64;
    assert!(rate <= delta + 0.012, "{rate} of the streams were made sampled, more than δ = {delta}");
}

fn history_run(run: &str, at: u64, verdict: Option<Verdict>, by: By, manifest: &Manifest) -> Vec<(u64, String, Event)> {
    use jaynes_core::event::{Cost, Effects, Outcome};
    use jaynes_core::risk::Ceilings;
    let question = Question {
        id: "q1-change".to_string(),
        step: "change".to_string(),
        round: 0,
        kind: QuestionKind::Decision,
        items: vec![item("change", "c")],
        offered: vec![VerdictKind::Keep, VerdictKind::Redo],
        holds: false,
        about: String::new(),
    };
    let verdicts: BTreeMap<String, Verdict> = verdict.into_iter().map(|v| ("change".to_string(), v)).collect();
    let answer = Answer { context: String::new(), question: question.id.clone(), shown: question.shown(), verdicts, by };
    let c = Ceilings::default();
    let r = run.to_string();
    vec![
        (at, r.clone(), Event::RunStarted {
            run: r.clone(), request: String::new(), manifest: manifest.clone(), role_fingerprint: Digest("f".to_string()),
            agent: "claude:m1".to_string(), engine: String::new(), protocol: 1, format: 1, risk: Risk::Low, reasons: Vec::new(), ceilings: c, ceiling: c.low,
            attention: BTreeMap::new(),
        }),
        (at, r.clone(), Event::QuestionOpened { question }),
        (at, r.clone(), Event::AnswerRecorded { answer, effects: Effects::default() }),
        (at, r, Event::RunFinished { outcome: Outcome::Delivered, cost: Cost::default() }),
    ]
}

#[test]
fn g17_the_count_comes_from_what_happened_after_delivery() {
    use jaynes_core::attention::count;
    let mut change = step("change", Executor::Agent);
    change.stop = Some(StopDecl { verdicts: vec![VerdictKind::Keep, VerdictKind::Redo], holds: false, from_risk: Risk::Low, release: true, budget: Some(jaynes_core::manifest::Rate(0.2)), audit_rate: jaynes_core::manifest::Rate(0.1), about: String::new() });
    let manifest = Manifest { role: "story".to_string(), version: "1".to_string(), protocol: 1, steps: vec![change], delivery: Default::default() };
    let key = stop_key(&manifest, &Digest("f".to_string()), 0);
    let day = 24 * 3600;
    let mut h = Vec::new();
    for n in 0..25 {
        h.extend(history_run(&format!("r{n}"), 1000, None, By::Person, &manifest));
    }
    // an accept counts as right only once its delivery held seven days
    assert_eq!(count(&h, &key, "claude:m1", 1000 + day, Some(0.2), 0.05).n, 0);
    assert_eq!(count(&h, &key, "claude:m1", 1000 + day, Some(0.2), 0.05).pending, 25);
    let c = count(&h, &key, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
    assert_eq!((c.n, c.k, c.braked), (25, 0, false));
    assert!(jaynes_core::attention::certified(c.n, c.k, 0.2, 0.05));
    // what an agent let through never earns a right
    let mut agent = h.clone();
    agent.extend(history_run("ra", 1000, None, By::Agent { who: "claude".to_string() }, &manifest));
    assert_eq!(count(&agent, &key, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05).n, 25);
    // a correction after delivery turns a right into an error, and two errors among the last ten brake the stop
    let mut corrected = h.clone();
    for run in ["r23", "r24"] {
        corrected.push((2000, run.to_string(), Event::DeliveryCorrected { note: "wrong".to_string(), by: By::Person, step: Some("change".to_string()) }));
    }
    let c = count(&corrected, &key, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
    assert_eq!((c.n, c.k, c.braked), (25, 2, true));
    // another model starts with the stop's errors and none of its rights
    assert_eq!(count(&h, &key, "claude:m2", 1000 + 8 * day, Some(0.2), 0.05).n, 0, "rights earned by m1 are not m2's");
    let c = count(&corrected, &key, "claude:m2", 1000 + 8 * day, Some(0.2), 0.05);
    assert_eq!((c.n, c.k, c.braked), (2, 2, true), "errors made with m1 count for m2 too");
    // a send-back counts at once
    let mut back = h.clone();
    back.extend(history_run("rb", 1000, Some(Verdict::Redo { note: "no".to_string() }), By::Person, &manifest));
    assert_eq!(count(&back, &key, "claude:m1", 1000 + day, Some(0.2), 0.05).k, 1);
}

#[test]
fn g17_a_certified_stop_asks_only_a_sample_and_a_braked_one_holds() {
    let mut change = step("change", Executor::Agent);
    change.stop = Some(StopDecl { verdicts: vec![VerdictKind::Keep, VerdictKind::Redo], holds: false, from_risk: Risk::Low, release: true, budget: Some(jaynes_core::manifest::Rate(0.2)), audit_rate: jaynes_core::manifest::Rate(0.1), about: String::new() });
    let manifest = Manifest { role: "story".to_string(), version: "1".to_string(), protocol: 1, steps: vec![change], delivery: Default::default() };
    let f = Digest("f".to_string());
    let key = stop_key(&manifest, &f, 0);
    let run_with = |id: &str, braked: bool| {
        let attention = [("change".to_string(), Count { key: key.clone(), n: 40, k: 0, pending: 0, braked })].into_iter().collect();
        let out = decide(None, &Input::Start { run: id.to_string(), request: String::new(), manifest: manifest.clone(), role_fingerprint: f.clone(), agent: "claude:m1".to_string(), raise: None, attention }).expect("start");
        let mut run = Run::fold(&out.events).expect("run");
        let c = probe_ok(&mut run, out.commands);
        let _ = c;
        deliver(&mut run, "change", 1, "o", Vec::new());
        run
    };
    let mut asked = 0;
    for n in 0..400 {
        let run = run_with(&format!("r{n}"), false);
        asked += run.questions.len();
        assert!(run.questions.len() + run.steps[0].skipped.iter().count() == 1);
    }
    assert!((20..=65).contains(&asked), "a certified stop asked {asked} times in 400 runs, about one in ten expected");
    let run = run_with("braked", true);
    let q = run.questions.values().next().expect("a braked stop always asks");
    assert!(q.question.holds);
    let agent = Answer { question: q.question.id.clone(), shown: q.question.shown(), verdicts: BTreeMap::new(), by: By::Agent { who: "claude".to_string() }, context: String::new() };
    assert!(decide(Some(&run), &Input::Answer { answer: agent }).is_err(), "an agent may not release a braked stop");
}

#[test]
fn the_person_s_context_reaches_the_steps_after_the_answer_and_changes_no_verdict() {
    let mut look = step("look", Executor::Agent);
    look.stop = Some(StopDecl { verdicts: vec![VerdictKind::Keep, VerdictKind::Redo], holds: true, from_risk: Risk::Low, release: false, budget: None, audit_rate: jaynes_core::manifest::Rate(0.1), about: String::new() });
    let mut copy = step("copy", Executor::Agent);
    copy.needs = vec!["look".to_string()];
    let (mut run, cmds) = start(vec![look, copy]);
    probe_ok(&mut run, cmds);
    let asked = deliver(&mut run, "look", 1, "blue", vec![item("palette", "blue")]);
    let q = asked.iter().find_map(|c| if let Command::Ask { question } = c { Some(question.clone()) } else { None }).expect("asked");
    let answer = Answer { context: "it is for accountants".to_string(), question: q.id.clone(), shown: q.shown(), verdicts: BTreeMap::new(), by: By::Person };
    let effects = jaynes_core::effects(&run, &q, &answer);
    assert!(effects.sentences.iter().any(|s| s.contains("Your context goes to every step after look") && s.contains("sends nothing back")), "{effects:?}");
    assert!(effects.reruns.is_empty());
    let c = feed(&mut run, Input::Answer { answer });
    let c = probe_ok(&mut run, c);
    let turn = c.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.clone()) } else { None }).expect("copy runs");
    assert_eq!((turn.step.as_str(), turn.context.clone()), ("copy", vec!["it is for accountants".to_string()]));
    assert_eq!(run.steps[0].round, 0, "context sends nothing back");
    let agent = Answer { context: "x".to_string(), question: q.id.clone(), shown: q.shown(), verdicts: BTreeMap::new(), by: By::Agent { who: "claude".to_string() } };
    assert!(decide(Some(&run), &Input::Answer { answer: agent }).is_err());
}

// --- the Lean model (engine/model) decides as the core does --------------------------------------------------

/// The run as the model reads it (`JaynesModel/Model.lean`), with the core's next move from it.
fn model_case(run: &Run) -> String {
    use jaynes_core::run::Phase;
    let mut digests: BTreeMap<Digest, u64> = BTreeMap::new();
    let mut number = |d: &Digest| -> u64 {
        let n = digests.len() as u64;
        *digests.entry(d.clone()).or_insert(n)
    };
    let steps: Vec<serde_json::Value> = run
        .manifest
        .steps
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let s = &run.steps[i];
            let status = match s.status {
                Status::Idle => "idle",
                Status::Probing => "probing",
                Status::Running { .. } => "running",
                Status::Checking { .. } => "checking",
                Status::AwaitingInput { .. } => "awaiting",
                Status::Done => "done",
            };
            let (question, holds) = match s.question.as_ref().and_then(|id| run.questions.get(id)) {
                None => ("none", false),
                Some(q) if q.superseded => ("superseded", q.question.holds),
                Some(q) => match &q.answer {
                    None => ("waiting", q.question.holds),
                    Some(a) if a.by.person() => ("person", q.question.holds),
                    Some(_) => ("agent", q.question.holds),
                },
            };
            let output = s.done.as_ref().map(|x| number(&x.output));
            let reads: Vec<serde_json::Value> = s
                .done
                .as_ref()
                .map(|x| x.reads.iter().map(|r| serde_json::json!({"step": run.manifest.index(&r.step).unwrap_or(usize::MAX), "output": number(&r.output)})).collect())
                .unwrap_or_default();
            serde_json::json!({
                "exec": format!("{:?}", d.executor).to_lowercase(),
                "status": status,
                "sources": run.sources(i),
                "output": output,
                "reads": reads,
                "stopApplies": d.stop.as_ref().is_some_and(|st| st.from_risk <= run.risk),
                "question": question,
                "holds": holds,
                "skipped": s.skipped == Some(s.round),
                "irreversible": d.effect.irreversible(),
                "probed": s.probed,
                "attemptsLeft": s.spent < d.attempts,
                "certified": run.attention.get(&d.name).is_some_and(|c| {
                    d.stop.as_ref().and_then(|st| st.budget).is_some_and(|b| jaynes_core::attention::certified(c.n, c.k, b.0, jaynes_core::DELTA))
                }),
                "braked": run.attention.get(&d.name).is_some_and(|c| c.braked),
                "coinOutside": d.stop.as_ref().is_some_and(|st| jaynes_core::attention::coin(&run.id, &d.name, s.round) >= st.audit_rate.0),
            })
        })
        .collect();
    let model = serde_json::json!({
        "steps": steps,
        "path": run.path(),
        "stopped": run.finished() || run.blocked() || run.halted || run.delivering.is_some(),
        "ceilingReached": run.spent.seconds >= run.ceiling.seconds || run.spent.tokens >= run.ceiling.tokens,
        "deliveredCurrent": run.delivered.as_ref() == Some(&run.delivery().0),
        "anyOpenQuestion": !run.open_questions().is_empty(),
    });
    // The core's first move from this state; a run waiting is put back to running, so its wait shows as a move.
    let mut fresh = run.clone();
    if matches!(fresh.phase, Phase::Waiting { .. }) {
        fresh.phase = Phase::Running;
    }
    let index = |s: &str| run.manifest.index(s).unwrap_or(usize::MAX);
    let expected = match decide(Some(&fresh), &Input::Tick) {
        Err(_) => "none".to_string(),
        Ok(out) => {
            let first = out.events.iter().find(|e| !matches!(e, Event::QuestionSuperseded { .. }));
            match first {
                None => "none".to_string(),
                Some(Event::QuestionOpened { question }) => format!("ask {}", index(&question.step)),
                Some(Event::StopSkipped { step, .. }) => format!("skip {}", index(step)),
                Some(Event::StepInvalidated { step, .. }) => format!("invalidate {}", index(step)),
                Some(Event::Waiting { step: Some(s), .. }) => format!("wait {}", index(s)),
                Some(Event::Waiting { step: None, .. }) => "wait answers".to_string(),
                Some(Event::Blocked { .. }) => "block".to_string(),
                Some(Event::ProbeAsked { step }) => format!("probe {}", index(step)),
                Some(Event::AttemptStarted { step, .. }) => format!("start {}", index(step)),
                Some(Event::DeliveryAsked { .. }) => "deliver".to_string(),
                Some(Event::RunFinished { .. }) => "finish".to_string(),
                Some(other) => format!("unexpected {}", other.name()),
            }
        }
    };
    serde_json::json!({"kind": "move", "run": model, "expected": expected}).to_string()
}

/// An answer as the model judges it (`AnswerCase`), with whether the core took it.
fn answer_case(run: &Run, answer: &Answer, taken: bool) -> String {
    let q = &run.questions[&answer.question];
    let i = run.manifest.index(&q.question.step).expect("step");
    let d = &run.manifest.steps[i];
    let s = &run.steps[i];
    let valid = answer.verdicts.iter().all(|(id, v)| {
        q.question.items.iter().any(|x| &x.id == id)
            && q.question.offered.contains(&v.kind())
            && !matches!(v, Verdict::Change { text } if text.trim().is_empty())
    });
    let case = serde_json::json!({
        "who": match answer.by { By::Person => "person", By::Rule { .. } => "rule", By::Agent { .. } => "agent" },
        "decision": q.question.kind == QuestionKind::Decision,
        "personStep": d.executor == Executor::Person,
        "input": s.inputs.contains(&q.question.id),
        "release": d.stop.as_ref().is_some_and(|st| st.release),
        "braked": run.attention.get(&d.name).is_some_and(|c| c.braked),
        "superseded": q.superseded,
        "shownMatches": answer.shown == q.question.shown(),
        "verdictsValid": valid,
        "allKeep": answer.verdicts.values().all(|v| *v == Verdict::Keep),
        "hasContext": !answer.context.trim().is_empty(),
        "answered": q.answer.is_some(),
        "revisable": q.question.kind == QuestionKind::Decision && d.executor != Executor::Person
            && s.question.as_deref() == Some(q.question.id.as_str()) && s.status == Status::Done,
    });
    serde_json::json!({"kind": "answer", "case": case, "expected": if taken { "taken" } else { "refused" }}).to_string()
}

/// A pass through the gate of the checks (`GateCase`), with what the core made of it.
fn gate_case(pre: &Run, input: &Input, events: &[Event]) -> Option<String> {
    let Input::ChecksEnded { step, attempt, fix, results } = input else { return None };
    let i = pre.manifest.index(step)?;
    let (d, s) = (&pre.manifest.steps[i], &pre.steps[i]);
    if s.status != (Status::Checking { attempt: *attempt, fix: *fix }) {
        return None;
    }
    let mut ids: BTreeMap<String, u64> = BTreeMap::new();
    let mut digests: BTreeMap<Digest, u64> = BTreeMap::new();
    let mut item = |x: &Item| {
        let n = ids.len() as u64;
        let id = *ids.entry(x.id.clone()).or_insert(n);
        let n = digests.len() as u64;
        let digest = *digests.entry(x.digest.clone()).or_insert(n);
        serde_json::json!({"id": id, "digest": digest})
    };
    let locks: Vec<_> = s.locks.values().map(|(x, _)| item(x)).collect();
    let dropped: Vec<_> = s.dropped.iter().map(&mut item).collect();
    let items: Vec<_> = s.pending.as_ref().map(|p| p.items.iter().map(&mut item).collect()).unwrap_or_default();
    let verdict = |name: &str| results.iter().find(|r| r.check == name).map(|r| &r.verdict);
    let fails = |kind: CheckKind| d.checks.iter().filter(|c| c.kind == kind).any(|c| matches!(verdict(&c.name), Some(CheckVerdict::Fail { .. })));
    let blocked = d.checks.iter().any(|c| matches!(verdict(&c.name), None | Some(CheckVerdict::Blocked { .. })));
    let case = serde_json::json!({
        "locks": locks, "dropped": dropped, "items": items,
        "openFails": fails(CheckKind::Open), "hiddenFails": fails(CheckKind::Hidden), "blocked": blocked,
        "roleStep": d.executor == Executor::Role, "fixesLeft": *fix < d.fixes, "session": s.session.is_some(),
    });
    let expected = if events.iter().any(|e| matches!(e, Event::StepDone { .. })) {
        "done"
    } else if events.iter().any(|e| matches!(e, Event::FixAsked { .. })) {
        "fix"
    } else if events.iter().any(|e| matches!(e, Event::Blocked { fault: Fault::Jaynes, .. })) {
        "jaynes"
    } else if events.iter().any(|e| matches!(e, Event::Blocked { fault: Fault::Role, .. })) {
        "role"
    } else {
        "spent"
    };
    Some(serde_json::json!({"kind": "gate", "case": case, "expected": expected}).to_string())
}

fn lake() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let elan = std::path::Path::new(&home).join(".elan/bin/lake");
    if elan.is_file() { Some(elan) } else { None }
}

/// The Lean model chooses the same move as the core from every state thousands of random runs reach, so what is
/// proved of the model (Proofs.lean) holds of the core on those states too.
#[test]
fn the_lean_model_decides_as_the_core_does() {
    let Some(lake) = lake() else {
        eprintln!("skipped: Lean is not installed (elan); the model is in engine/model");
        return;
    };
    let mut cases = Vec::new();
    for seed in 1..=800u64 {
        let seed = seed.wrapping_mul(0x2545_F491_4F6C_DD1D);
        let mut w = World::new(seed);
        w.cases = Some(Vec::new());
        start_random(&mut w, seed);
        for _ in 0..300 {
            w.act();
            if w.run().finished() {
                break;
            }
        }
        cases.extend(w.cases.take().unwrap_or_default());
    }
    let model = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../model");
    // The proofs build with the model; JAYNES_MODEL_TARGET=check builds the checker alone, to try a model whose
    // proofs no longer hold against the core.
    let target = std::env::var("JAYNES_MODEL_TARGET").unwrap_or_default();
    let mut build = std::process::Command::new(&lake);
    build.arg("build").current_dir(&model);
    if !target.is_empty() {
        build.arg(&target);
    }
    let built = build.output().expect("lake");
    assert!(built.status.success(), "the model does not build: {}", String::from_utf8_lossy(&built.stdout));
    use std::io::Write;
    let mut check = std::process::Command::new(model.join(".lake/build/bin/check"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("check");
    let mut stdin = check.stdin.take().expect("stdin");
    let all = cases.join("\n") + "\n";
    let writer = std::thread::spawn(move || stdin.write_all(all.as_bytes()));
    let out = check.wait_with_output().expect("check");
    let _wrote = writer.join();
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    eprintln!("{} states: {}", cases.len(), said.lines().find(|l| l.contains("agreed")).unwrap_or(""));
    assert!(out.status.success(), "{said}");
}
