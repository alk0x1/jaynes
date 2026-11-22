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
use jaynes_core::io::AskOutcome;
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
                    local_ports: Vec::new(),
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
                // the step that made the work this one reviews, or that one of the role's own shows: what the person
                // sends back may go there
                let maker = if matches!(executor, Executor::Agent | Executor::Role) && rng.chance(40) {
                    needs.iter().find(|n| steps.iter().any(|s: &StepDecl| &s.name == *n && s.executor == Executor::Agent && !s.hidden && s.from_risk <= from_risk)).cloned()
                } else {
                    None
                };
                let rounds = rng.chance(30).then(|| 1 + rng.below(3) as u32);
                // a stop that chooses among options: an agent's, always asked, offering keep, change and drop
                if executor == Executor::Agent && rng.chance(25) {
                    let verdicts = vec![VerdictKind::Keep, VerdictKind::Change, VerdictKind::Drop];
                    Some(StopDecl { settled: None, verdicts, holds: rng.chance(50), from_risk: stop_risk, release: false, budget: None, audit_rate: jaynes_core::manifest::Rate(0.3), about: String::new(), choose: true, redo: maker, rounds, answers: Default::default(), note: String::new() })
                } else {
                    Some(StopDecl { settled: None, verdicts, holds: rng.chance(30), from_risk: stop_risk, release, budget, audit_rate: jaynes_core::manifest::Rate(0.3), about: String::new(), choose: false, redo: maker, rounds, answers: Default::default(), note: String::new() })
                }
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
                hidden, idempotent: false,
                about: String::new(),
                model: Default::default(),
                turns: None,
            });
        }
        let delivery = if rng.chance(30) { jaynes_core::manifest::DeliveryKind::Merge } else { jaynes_core::manifest::DeliveryKind::Branch };
        let m = Manifest { role: "random".to_string(), version: "1".to_string(), protocol: 1, steps, target: Default::default(), delivery };
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
        hidden: false, idempotent: false,
        about: String::new(),
        model: Default::default(),
        turns: None,
    };
    Manifest { role: "random".to_string(), version: "1".to_string(), protocol: 1, steps: vec![one], target: Default::default(), delivery: Default::default() }
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
    /// Continues that had the checks judge again a change they could not judge.
    rechecked: u32,
    /// Questions the person asked a stop's agent that the core took.
    asked: u32,
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
            rechecked: 0,
            asked: 0,
        }
    }

    fn run(&self) -> &Run {
        self.run.as_ref().expect("the run started")
    }

    /// Send an input, check every guarantee it could break, and record what it decided.
    /// The person, the agent that started the run, or another agent.
    fn who(&mut self) -> (By, Option<String>) {
        let agent = By::Agent { who: "claude".to_string() };
        match self.rng.below(10) {
            0..=6 => (By::Person, None),
            7 | 8 => (agent, Some(OWN.to_string())),
            _ => (agent, Some("claude:8@1763779999".to_string())),
        }
    }

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
                            cases.extend(rides_case(r));
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
                            assert!(rule == READ_INSIDE_PROJECT || rule == jaynes_core::decide::COMMAND_THE_STEP_MAY_RUN || rule == jaynes_core::decide::WRITE_THE_STEP_MAY_WRITE, "seed {seed}: G9 unknown rule");
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
        assert!(post.risk >= risk::of_change(post.manifest.target, &changed).0, "seed {seed}: G7 the risk is below its rules");
        // and whatever a measure marked: a change it put on the floor, or one on a repository's floor it did not mark
        let on_floor = changed.iter().any(|c| c.floor || !risk::floor([c.what.as_str()]).is_empty());
        assert!(!on_floor || post.risk == risk::Risk::High, "seed {seed}: G7 a change on the floor left the run below high");
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
                // G19: a merge goes out only on the person's keep about this very delivery, every file it showed kept,
                // what runs later outside any box among them; dropped, it delivers the branch only. Every merge waits
                // so, whatever it brings
                Command::Deliver { digest, merge, given } if !post.delivered_files().is_empty() && post.manifest.lands() && !post.undone && !post.never_merge => {
                    let answer = post.merge_answer(digest);
                    assert!(answer.is_some(), "seed {seed}: G19 a merge bringing in {:?} went out unanswered", post.delivered_files().iter().map(|(c, _)| &c.what).collect::<Vec<_>>());
                    assert_eq!(Some(*merge), answer, "seed {seed}: G19 the merge did not do what the person answered");
                    for r in given {
                        let j = post.manifest.index(&r.step).expect("read");
                        assert_eq!(post.output(j), Some(&r.output), "seed {seed}: G2 an unchecked output was given");
                    }
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
        // G4: on the person's continue, a change a check could not judge is judged again as the attempt delivered it,
        // and no attempt starts for it.
        if let (Input::Continue { .. }, Some(b)) = (input, before)
            && let Some(i) = b.checks_again()
            && events.iter().any(|e| matches!(e, Event::Continued { .. }))
        {
            let (name, output) = (&b.manifest.steps[i].name, &b.steps[i].pending.as_ref().expect("a change waits").output);
            assert!(
                !events.iter().any(|e| matches!(e, Event::AttemptStarted { step, .. } if step == name)),
                "seed {seed}: G4 a continue of a change the checks could not judge started an attempt"
            );
            assert!(
                commands.iter().any(|c| matches!(c, Command::RunChecks { step, output: o, .. } if step == name && o == output)),
                "seed {seed}: G4 a continue did not have the checks judge the same change again: {commands:?}"
            );
            self.rechecked += 1;
        }
        // A question to a stop's agent redoes nothing and chooses nothing: no attempt starts, no answer is recorded,
        // no output changes, and the stop stays open in the same round with the same items; only the agent's turn
        // that reads is asked for. Its answer changes no step either, and starts nothing.
        if let (Input::AskAgent { .. } | Input::AskEnded { .. }, Some(b)) = (input, before)
            && !events.is_empty()
        {
            let mut after = b.clone();
            for e in events {
                after.apply(e);
            }
            for (x, y) in b.steps.iter().zip(&after.steps) {
                assert!(x.done == y.done && x.round == y.round && x.status == y.status, "seed {seed}: an ask or its answer changed a step");
            }
            if let Input::AskAgent { question, .. } = input {
                assert!(events.iter().all(|e| matches!(e, Event::AgentAsked { .. })), "seed {seed}: an ask did more than ask: {events:?}");
                assert!(commands.len() == 1 && matches!(commands[0], Command::AnswerAsk(_)), "seed {seed}: an ask started more: {commands:?}");
                let (was, now) = (&b.questions[question], &after.questions[question]);
                assert!(now.open() && was.question.shown() == now.question.shown(), "seed {seed}: an ask changed or closed the stop");
                self.asked += 1;
            } else {
                assert!(commands.is_empty(), "seed {seed}: an answer to an ask started something: {commands:?}");
            }
        }
        // The agent that started the run goes on only where Jaynes judges the same change again, and sets the run
        // aside, never ends it; another agent does neither. Each tells the person.
        if let Some(b) = before {
            let notified = commands.iter().any(|c| matches!(c, Command::Notify { .. }));
            match input {
                Input::Continue { by: by @ By::Agent { .. }, starter } if events.iter().any(|e| matches!(e, Event::Continued { .. })) => {
                    assert!(b.started_by(by, starter.as_deref()) && b.checks_again().is_some() && notified, "seed {seed}: an agent continued what it may not: {events:?}");
                }
                Input::Abandon { by: by @ By::Agent { .. }, starter } if !events.is_empty() => {
                    assert!(b.started_by(by, starter.as_deref()) && b.set_aside.is_none() && notified, "seed {seed}: an agent abandoned what it may not");
                    assert!(!events.iter().any(|e| matches!(e, Event::RunFinished { .. })), "seed {seed}: an agent's abandon ended the run");
                }
                Input::Resume { by: By::Agent { .. } } if !events.is_empty() => panic!("seed {seed}: an agent brought back a run"),
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
        // and so are those of the agent's turn that answered an ask
        if !events.is_empty()
            && let Input::AskEnded { outcome: AskOutcome::Answered { usage, .. } | AskOutcome::Failed { usage, .. }, .. } = input {
                self.usage.add(*usage);
            }
        assert_eq!(post.spent, self.usage, "seed {seed}: G5 the run's spending differs from what was reported");
    }
}

/// Inputs the world sends that the core may refuse.
/// The agent process that started half the random runs.
const OWN: &str = "claude:7@1763770000";

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
        // raw, as any measure may report it: the mark at random, also off a path the core knows is on the floor
        0 => vec![Changed { what: ".github/workflows/ci.yml".to_string(), size: 2, floor: rng.chance(50), ..Default::default() }],
        1 => vec![Changed { what: "src/big.rs".to_string(), size: 80, floor: rng.chance(20), ..Default::default() }],
        2..=5 => vec![Changed { what: format!("src/f{}.rs", rng.below(3)), size: 1 + rng.below(5) as u64, floor: rng.chance(10), ..Default::default() }],
        _ => Vec::new(),
    }
}

fn usage(rng: &mut Rng) -> Usage {
    Usage { seconds: 10 + rng.below(200) as u64, tokens: 1000 + rng.below(300_000) as u64, cached: rng.below(1000) as u64, ..Default::default() }
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
            // the agent answers the person's ask, or does not; it may come after the stop was answered
            Command::AnswerAsk(t) => {
                let outcome = if self.rng.chance(80) {
                    AskOutcome::Answered { text: "it would be warmer".to_string(), usage: Usage { tokens: 50, ..Usage::default() }, session: self.rng.chance(50).then(|| "asked".to_string()) }
                } else {
                    AskOutcome::Failed { reason: "the agent wrote nothing".to_string(), usage: Usage::default() }
                };
                self.send(Input::AskEnded { ask: t.ask, outcome });
            }
            Command::Probe { step, .. } => {
                let ok = self.rng.chance(95);
                self.send(Input::ProbeEnded { step, ok, reason: "the box refused /tmp".to_string() });
            }
            Command::RunAgent(turn) => {
                if self.rng.chance(6) {
                    let target = if self.rng.chance(50) { "src/a.rs".to_string() } else { "/etc/passwd".to_string() };
                    let kind = match self.rng.below(10) { 0..=5 => AskKind::Read, 6 => AskKind::Write, 7 => AskKind::Command, _ => AskKind::Network };
                    self.send(Input::PermissionAsked { step: turn.step.clone(), attempt: turn.attempt, ask: Ask { kind, target } });
                }
                if self.rng.chance(50) {
                    self.send(Input::SessionStarted { step: turn.step.clone(), attempt: turn.attempt, session: format!("session-{}", turn.step), confirmed: Default::default() });
                }
                // a round that keeps a choice alone: the role keeps the chosen items, as they were, most of the time
                let kept = turn.keep.as_ref().filter(|_| self.rng.chance(90)).map(|keep| {
                    let i = run.manifest.index(&turn.step).expect("step");
                    let done = run.steps[i].done.as_ref().map(|d| d.items.clone()).unwrap_or_default();
                    done.into_iter().filter(|x| keep.contains(&x.id)).collect::<Vec<_>>()
                });
                let outcome = match self.rng.below(100) {
                    _ if kept.is_some() => AttemptOutcome::Delivered {
                        output: Digest(format!("kept-{}", turn.round)),
                        items: kept.unwrap_or_default(),
                        changed: Vec::new(),
                        usage: Usage::default(),
                        session: None,
                        tree: None,
                        reply: None,
                    },
                    0..=71 => AttemptOutcome::Delivered {
                        output: digest_pool(&mut self.rng),
                        items: items_for(&mut self.rng, &run, &turn.step),
                        changed: if turn.reach.project == ProjectAccess::Write { changed(&mut self.rng) } else { Vec::new() },
                        usage: usage(&mut self.rng),
                        session: Some(format!("session-{}", turn.step)),
                        tree: None,
                        reply: None,
                    },
                    72..=85 => AttemptOutcome::Failed { reasons: vec!["no output".to_string()], usage: usage(&mut self.rng) },
                    86..=88 => AttemptOutcome::Impossible { reason: "the request contradicts itself".to_string(), usage: usage(&mut self.rng), session: Some(format!("session-{}", turn.step)) },
                    89..=94 => AttemptOutcome::Fault { usage: Default::default(), reason: "claude did not start".to_string() },
                    _ => AttemptOutcome::Interrupted { usage: Default::default(), reason: "quota".to_string() },
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
                        tree: None,
                        reply: None,
                    },
                    80..=91 => AttemptOutcome::NeedsInput {
                        items: vec![Item { id: "which".to_string(), label: "which test".to_string(), digest: Digest("w".to_string()) }],
                        offered: vec![VerdictKind::Keep, VerdictKind::Change],
                        about: String::new(),
                    },
                    92..=95 => AttemptOutcome::Refused { reason: "no test runner".to_string() },
                    _ => AttemptOutcome::Fault { usage: Default::default(), reason: "the role program crashed".to_string() },
                };
                self.send(Input::AttemptEnded { step, attempt, fix: 0, outcome });
            }
            Command::RunChecks { step, attempt, fix, checks, .. } => {
                let mut results = Vec::new();
                for name in checks {
                    let verdict = match self.rng.below(100) {
                        0..=79 => CheckVerdict::Pass,
                        80..=95 => CheckVerdict::Fail { reasons: vec!["it does not hold".to_string()], detail: String::new(), cases: vec![] },
                        96..=97 => CheckVerdict::Blocked { reason: "the tool did not start".to_string() },
                        _ => continue,
                    };
                    results.push(CheckResult { check: name, verdict, tree: None });
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
        // now and then the person asks the stop's agent instead: refused where it does not apply
        if self.rng.chance(20) {
            let item = if self.rng.chance(60) { q.items.first().map(|x| x.id.clone()) } else { None };
            self.send(Input::AskAgent { question: q.id.clone(), item, text: "what if it were warmer?".to_string(), by: By::Person });
            return;
        }
        let mut verdicts = BTreeMap::new();
        // a merge is answered file by file, so most of its answers say something of each file
        let each = if q.kind == QuestionKind::Merge { 95 } else { 70 };
        for item in &q.items {
            if self.rng.chance(each) {
                verdicts.insert(item.id.clone(), verdict(&mut self.rng, &q.offered));
            }
        }
        // a choice, most of the time: one kept or changed, the rest dropped
        let mut context = String::new();
        if q.choose && self.rng.chance(15) {
            // none of them: every option leaves, with the person's words
            verdicts = q.items.iter().map(|x| (x.id.clone(), Verdict::Drop)).collect();
            context = "something warmer".to_string();
        } else if q.choose && q.items.len() > 2 && self.rng.chance(20) {
            // finalists: all but one developed further
            let out = self.rng.below(q.items.len());
            verdicts = q.items.iter().enumerate().map(|(n, x)| (x.id.clone(), if n == out { Verdict::Drop } else { Verdict::Change { text: "develop it".to_string() } })).collect();
        } else if q.choose && self.rng.chance(80) && !q.items.is_empty() {
            let chosen = self.rng.below(q.items.len());
            verdicts = q.items.iter().enumerate().map(|(n, x)| {
                let v = if n != chosen { Verdict::Drop } else if self.rng.chance(50) { Verdict::Keep } else { Verdict::Change { text: "the font of the other".to_string() } };
                (x.id.clone(), v)
            }).collect();
        }
        let wrong = self.rng.chance(5);
        let shown = if wrong { Digest("stale".to_string()) } else { q.shown() };
        // a merge and an act already done are the person's alone: most answers to them are theirs, so the runs held
        // there go on to what follows (an agent's or a rule's answer is still tried, and refused)
        let theirs = matches!(q.kind, QuestionKind::Merge | QuestionKind::Acted);
        let roll = if theirs && self.rng.chance(80) { 19 } else { self.rng.below(20) };
        let by = match roll {
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
        let context = if by.person() { context } else { String::new() };
        let answer = Answer { back: Default::default(), question: q.id.clone(), shown, verdicts, by: by.clone() , context };
        let logged = self.log.len();
        let accepted = self.send(Input::Answer { answer: answer.clone() });
        if let Some(cases) = self.cases.as_mut() {
            cases.push(answer_case(&run, &answer, accepted));
            if q.kind == QuestionKind::Merge {
                let kept: Vec<&str> = if accepted { self.log[logged..].iter().filter_map(|e| if let Event::AnswerRecorded { answer: a, .. } = e { Some(a.question.as_str()) } else { None }).filter(|id| *id != q.id).collect() } else { Vec::new() };
                cases.push(touch_case(&run, &q, &answer, &kept));
            }
            if accepted {
                let i = run.manifest.index(&q.step).expect("step");
                let person = run.manifest.steps[i].executor == Executor::Person;
                let items: Vec<serde_json::Value> = q
                    .items
                    .iter()
                    .map(|x| serde_json::json!({"back": answer.verdict(&x.id).sends_back(), "standsFor": if person { run.manifest.index(&x.id) } else { None }}))
                    .collect();
                // the step the work goes back to: the stop's maker, unless the choice was kept alone
                let back_to = if person || q.kind == QuestionKind::Acted { i } else {
                    run.manifest.steps[i].stop.as_ref().and_then(|st| st.redo.as_ref()).and_then(|r| run.manifest.index(r))
                        .filter(|_| answer.kept_alone(&q).is_none()).unwrap_or(i)
                };
                // an answer that sends back an act already done holds the run at it, which the model of effects
                // leaves out: the core's own check of G11 covers it, just below
                // and acting again where the act may have happened is the same intent, which runs nothing again
                let held_target = (q.kind == QuestionKind::Acted && run.steps[i].maybe_acted) || q.kind != QuestionKind::Acted && answer.sends_back() && (run.acted(back_to)
                    || (person && q.items.iter().any(|x| answer.verdict(&x.id).sends_back() && run.manifest.index(&x.id).is_some_and(|j| run.acted(j)))));
                let case = serde_json::json!({
                    "step": back_to, "personStep": person, "input": run.steps[i].inputs.contains(&q.id),
                    "permission": q.kind == QuestionKind::Permission, "merge": q.kind == QuestionKind::Merge, "items": items,
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
                if let Some(cases) = self.cases.as_mut()
                    && !held_target {
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
                    outcome: AttemptOutcome::Delivered { output: Digest("late".to_string()), items: Vec::new(), changed: Vec::new(), usage: Usage::default(), session: None, tree: None, reply: None },
                });
                assert!(self.log.len() <= before + 1, "seed {}: G6 a late report did more than being recorded", self.seed);
            }
        } else if roll < 86 {
            if run.blocked() {
                let (by, starter) = self.who();
                self.send(Input::Continue { by, starter });
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
            } else if self.rng.chance(70) {
                self.send(Input::Halt { by: By::Agent { who: "claude".to_string() } });
            } else {
                // an agent abandons: set aside if it started the run, refused otherwise
                let (by, starter) = self.who();
                if !by.person() {
                    self.send(Input::Abandon { by, starter });
                }
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
    let starter = w.rng.chance(50).then(|| OWN.to_string());
    let started = w.send(Input::Start {
        run: format!("run-{seed}"),
        request: "change the button's text".to_string(),
        manifest: manifest.clone(),
        role_fingerprint: fingerprint,
        agent: "claude:m1".to_string(),
        raise,
        attention, task: None, arm: None, never_merge: false,
        // half the runs an agent started, which may then continue a block of Jaynes's and set the run aside
        starter,
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
        attention: BTreeMap::new(), task: None, arm: None, never_merge: false,
        starter: None,
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

/// An evaluation's planted defects on random runs: planted into a done step whose stop waits, by the person or by an
/// agent (refused), and taken out, at random beside everything else the world does. Nothing is asked to be delivered,
/// delivered or finished while one is in. Not in the Lean model yet: its random runs plant nothing.
fn simulate_planting(seed: u64) -> (usize, usize) {
    let mut w = World::new(seed);
    let (_, started) = start_random(&mut w, seed);
    assert!(started, "seed {seed}: a valid role did not start");
    let (mut n, mut held) = (0, 0);
    for _ in 0..400 {
        let run = w.run().clone();
        let roll = w.rng.below(100);
        if roll < 10 {
            let step = run.manifest.steps[w.rng.below(run.manifest.steps.len())].name.clone();
            let by = if w.rng.chance(85) { By::Person } else { By::Agent { who: "agent".to_string() } };
            let agent = !by.person();
            n += 1;
            let planted = w.send(Input::Plant { id: format!("d{n}"), step, by });
            assert!(!(agent && planted), "seed {seed}: an agent planted a defect");
        } else if roll < 16 && !run.planted.is_empty() {
            let ids: Vec<String> = run.planted.keys().cloned().collect();
            let id = ids[w.rng.below(ids.len())].clone();
            let by = if w.rng.chance(85) { By::Person } else { By::Agent { who: "agent".to_string() } };
            let agent = !by.person();
            let out = w.send(Input::Unplant { id, by });
            assert!(!(agent && out), "seed {seed}: an agent took a defect out");
        } else {
            w.act();
        }
        if matches!(w.run().phase, Phase::Waiting { ref on, .. } if on.is_empty()) && !w.run().planted.is_empty() {
            held += 1;
        }
        if w.run().finished() {
            break;
        }
    }
    let mut planted = BTreeSet::new();
    for e in &w.log {
        match e {
            Event::DefectPlanted { id, .. } => {
                planted.insert(id.clone());
            }
            Event::DefectRemoved { id, .. } => {
                assert!(planted.remove(id), "seed {seed}: a defect taken out that was not in");
            }
            Event::DeliveryAsked { .. } | Event::Delivered { .. } | Event::RunFinished { .. } => {
                assert!(planted.is_empty(), "seed {seed}: {e:?} while a planted defect was in");
            }
            _ => {}
        }
    }
    (w.log.iter().filter(|e| matches!(e, Event::DefectPlanted { .. })).count(), held)
}

#[test]
fn the_random_runs_plant_defects_and_hold_on_them() {
    let (mut planted, mut held) = (0, 0);
    for seed in 0..400u64 {
        let (p, h) = simulate_planting(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        planted += p;
        held += h;
    }
    assert!(planted > 50 && held > 10, "the random runs planted {planted} defects and held on one {held} times");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn every_guarantee_holds_on_random_runs(seed in any::<u64>()) {
        simulate(seed);
    }

    /// Nothing is delivered, nor does the run end, while an evaluation's planted defect is in.
    #[test]
    fn nothing_goes_out_while_a_planted_defect_is_in(seed in any::<u64>()) {
        simulate_planting(seed);
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
        hidden: false, idempotent: false,
        about: String::new(),
        model: Default::default(),
        turns: None,
    }
}

fn start(steps: Vec<StepDecl>) -> (Run, Vec<Command>) {
    let manifest = Manifest { role: "story".to_string(), version: "1".to_string(), protocol: 1, steps, target: Default::default(), delivery: Default::default() };
    let out = decide(None, &Input::Start {
        run: "r".to_string(),
        request: "do it".to_string(),
        manifest,
        role_fingerprint: Digest("f".to_string()),
        agent: "claude:m1".to_string(), raise: None,
        attention: BTreeMap::new(), task: None, arm: None, never_merge: false,
        starter: None,
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
            usage: Usage { seconds: 5, tokens: 100, cached: 0, ..Default::default() },
            session: Some("s".to_string()),
            tree: None,
            reply: None,
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
    look.stop = Some(StopDecl { settled: None, verdicts: vec![VerdictKind::Keep, VerdictKind::Redo], holds: false, from_risk: Risk::Low, release: false, budget: None, audit_rate: jaynes_core::manifest::Rate(0.1), about: String::new(), choose: false, redo: None, rounds: None, answers: Default::default(), note: String::new() });
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
    let answer = Answer { back: Default::default(), context: String::new(),
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
    plan.stop = Some(StopDecl { settled: None, verdicts: vec![VerdictKind::Keep], holds: false, from_risk: Risk::Low, release: false, budget: None, audit_rate: jaynes_core::manifest::Rate(0.1), about: String::new(), choose: false, redo: None, rounds: None, answers: Default::default(), note: String::new() });
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
    let c = feed(&mut run, Input::Answer { answer: Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts: BTreeMap::new(), by: By::Person , context: String::new()} });
    let c = probe_ok(&mut run, c);
    assert!(c.iter().any(|c| matches!(c, Command::RunRole { key: Some(_), .. })), "push runs once answered, with a key");
}

fn stop_of(verdicts: Vec<VerdictKind>, holds: bool) -> StopDecl {
    StopDecl { settled: None, verdicts, holds, from_risk: Risk::Low, release: false, budget: None, audit_rate: jaynes_core::manifest::Rate(0.1), about: String::new(), choose: false, redo: None, rounds: None, answers: Default::default(), note: String::new() }
}

fn asked_in(cmds: &[Command]) -> Option<Question> {
    cmds.iter().find_map(|c| if let Command::Ask { question } = c { Some(question.clone()) } else { None })
}

fn answer_to(q: &Question, verdicts: &[(&str, Verdict)]) -> Input {
    Input::Answer { answer: Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts: verdicts.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(), by: By::Person, context: String::new() } }
}

fn redo(note: &str) -> Verdict {
    Verdict::Redo { note: note.to_string() }
}

/// The keys of the push's acts among `cmds`.
fn pushes(cmds: &[Command]) -> Vec<Option<String>> {
    cmds.iter().filter_map(|c| if let Command::RunRole { step, key, .. } = c { (step == "push").then(|| key.clone()) } else { None }).collect()
}

/// A run whose plan the person kept and whose push, which cannot be undone, acted on it; the person's own step look,
/// after it, keeps the run open. With the plan's question, the push's own question and the key it acted with.
fn pushed() -> (Run, Question, Question, String) {
    let mut plan = step("plan", Executor::Agent);
    plan.stop = Some(stop_of(vec![VerdictKind::Keep, VerdictKind::Redo], false));
    let mut push = step("push", Executor::Role);
    push.needs = vec!["plan".to_string()];
    push.effect = Effect::Irreversible { idempotent: false };
    push.stop = Some(stop_of(vec![VerdictKind::Keep, VerdictKind::Redo], false));
    let mut look = step("look", Executor::Person);
    look.needs = vec!["push".to_string()];
    look.stop = Some(stop_of(vec![VerdictKind::Keep, VerdictKind::Redo], true));
    let (mut run, cmds) = start(vec![plan, push, look]);
    probe_ok(&mut run, cmds);
    let asked = deliver(&mut run, "plan", 1, "p", vec![item("a", "a1")]);
    let plan_q = asked_in(&asked).expect("plan asked");
    let c = feed(&mut run, answer_to(&plan_q, &[]));
    let c = probe_ok(&mut run, c);
    let key = pushes(&c).first().cloned().flatten().expect("push acts, with a key");
    let asked = deliver(&mut run, "push", 1, "pushed", vec![item("x", "x1")]);
    let push_q = asked_in(&asked).expect("push asked");
    assert_eq!(run.steps[run.manifest.index("push").expect("push")].status, Status::Done, "push is done");
    (run, plan_q, push_q, key)
}

/// (a) An answer above it changes what it rested on: it is not run again; the run holds at it and asks the person.
#[test]
fn g14_when_what_an_act_rested_on_changes_the_run_holds_there_and_asks() {
    let (mut run, plan_q, _, _) = pushed();
    let c = feed(&mut run, answer_to(&plan_q, &[("a", redo("another plan"))]));
    let c = probe_ok(&mut run, c);
    let t = c.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.attempt) } else { None }).expect("the plan is made again");
    let asked = deliver(&mut run, "plan", t, "p2", vec![item("a", "a2")]);
    let q = asked_in(&asked).expect("the new plan is asked about");
    let c = feed(&mut run, answer_to(&q, &[]));
    let c = probe_ok(&mut run, c);
    assert!(pushes(&c).is_empty(), "push acted again on its own: {c:?}");
    let held = held_at_push(&run, &c);
    assert_eq!(held.step, "push");
    assert!(matches!(run.phase, Phase::Waiting { step: Some(ref s), .. } if s == "push"), "{:?}", run.phase);
    // kept as it is: push stays done, and the run goes on to the person's look
    let c = feed(&mut run, answer_to(&held, &[]));
    assert!(pushes(&c).is_empty(), "{c:?}");
    assert_eq!(run.steps[run.manifest.index("push").expect("push")].status, Status::Done);
    assert!(asked_in(&c).is_some_and(|q| q.step == "look") || run.open_questions().iter().any(|q| q.question.step == "look"), "{c:?}");
}

/// Acting again is the person's order, a second act with its own key.
#[test]
fn g14_acting_again_is_a_second_act_the_person_orders_with_a_new_key() {
    let (mut run, plan_q, _, first) = pushed();
    let c = feed(&mut run, answer_to(&plan_q, &[("a", redo("another plan"))]));
    let c = probe_ok(&mut run, c);
    let t = c.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.attempt) } else { None }).expect("plan again");
    let asked = deliver(&mut run, "plan", t, "p2", vec![item("a", "a2")]);
    let c = feed(&mut run, answer_to(&asked_in(&asked).expect("asked"), &[]));
    let c = probe_ok(&mut run, c);
    let held = held_at_push(&run, &c);
    let c = feed(&mut run, answer_to(&held, &[("push", redo("act on the new plan"))]));
    let c = probe_ok(&mut run, c);
    let again = pushes(&c);
    assert_eq!(again.len(), 1, "{c:?}");
    assert_ne!(again[0].as_deref(), Some(first.as_str()), "a second act has its own key");
}

/// The question the run holds at, about push's act, among `cmds` or still open.
fn held_at_push(run: &Run, cmds: &[Command]) -> Question {
    asked_in(cmds).filter(|q| q.kind == QuestionKind::Acted).or_else(|| {
        run.open_questions().iter().find(|q| q.question.kind == QuestionKind::Acted).map(|q| q.question.clone())
    }).expect("the run asks about push's act")
}

/// (b) Sending it back at its own stop does not do it again: the run holds there and asks.
#[test]
fn g14_sending_an_act_back_at_its_own_stop_holds_the_run_there() {
    let (mut run, _, push_q, _) = pushed();
    let c = feed(&mut run, answer_to(&push_q, &[("x", redo("again"))]));
    let c = probe_ok(&mut run, c);
    assert!(pushes(&c).is_empty(), "push acted again on its own: {c:?}");
    let held = held_at_push(&run, &c);
    assert_eq!(held.step, "push");
    assert_eq!(run.steps[run.manifest.index("push").expect("push")].status, Status::Done);
}

/// (c) A person's step after it sending it back: the same.
#[test]
fn g14_sending_an_act_back_from_a_later_step_of_the_person_holds_the_run_there() {
    let (mut run, _, push_q, _) = pushed();
    let c = feed(&mut run, answer_to(&push_q, &[]));
    let look = asked_in(&c).or_else(|| run.open_questions().iter().find(|q| q.question.step == "look").map(|q| q.question.clone())).expect("look asks");
    let c = feed(&mut run, answer_to(&look, &[("push", redo("again"))]));
    let c = probe_ok(&mut run, c);
    assert!(pushes(&c).is_empty(), "push acted again on its own: {c:?}");
    assert_eq!(held_at_push(&run, &c).step, "push");
    assert_eq!(run.steps[run.manifest.index("push").expect("push")].status, Status::Done);
}

/// (a) Raising the risk after the act cannot change what it rests on: a role with a step that cannot be undone on its
/// short path starts every run at high risk, the full path, so the risk has nowhere to go after the act.
#[test]
fn g14_raising_the_risk_after_an_act_cannot_change_what_it_rested_on() {
    let (run, _, _, _) = pushed();
    assert_eq!(run.risk, Risk::High, "a run with an act starts on the full path");
    let raise = Input::RaiseRisk { to: Risk::High, by: By::Person };
    let Err(r) = decide(Some(&run), &raise) else { panic!("raised") };
    assert!(r.to_string().contains("already at high"), "{r}");
    // and a rule raising it from what the change touches leaves it where it was: nothing of the path moves
    assert_eq!(run.path(), (0..run.manifest.steps.len()).collect::<Vec<_>>());
}

/// A run whose one step acts on something real (`effect`), started and under way.
fn acting(effect: Effect, idempotent: bool) -> (Run, String) {
    let mut send = step("send", Executor::Role);
    send.effect = effect;
    send.idempotent = idempotent;
    let (mut run, cmds) = start(vec![send]);
    let c = probe_ok(&mut run, cmds);
    let key = c.iter().find_map(|c| if let Command::RunRole { key, .. } = c { Some(key.clone().unwrap_or_default()) } else { None }).expect("send runs");
    (run, key)
}

#[test]
fn g14_an_act_whose_outcome_was_not_recorded_is_not_started_again_on_its_own() {
    type Ending = Box<dyn Fn(&mut Run) -> Vec<Command>>;
    let outcomes: Vec<(&str, Ending)> = vec![
        ("Jaynes stopped", Box::new(|run: &mut Run| feed(run, Input::Recovered))),
        ("stopped from outside", Box::new(|run: &mut Run| {
            let attempt = run.steps[0].attempt;
            feed(run, Input::AttemptEnded { step: "send".to_string(), attempt, fix: 0, outcome: AttemptOutcome::Interrupted { usage: Default::default(), reason: "halted".to_string() } })
        })),
        ("a fault", Box::new(|run: &mut Run| {
            let attempt = run.steps[0].attempt;
            feed(run, Input::AttemptEnded { step: "send".to_string(), attempt, fix: 0, outcome: AttemptOutcome::Fault { usage: Default::default(), reason: "the box died".to_string() } })
        })),
    ];
    for effect in [Effect::Irreversible { idempotent: false }, Effect::Compensable { undo: "revert".to_string() }] {
        for (how, end) in &outcomes {
            let (mut run, key) = acting(effect.clone(), false);
            let c = end(&mut run);
            assert!(!c.iter().any(|c| matches!(c, Command::RunRole { .. })), "{effect:?}, {how}: started again: {c:?}");
            assert!(matches!(run.phase, Phase::Blocked { .. }), "{effect:?}, {how}: {:?}", run.phase);
            // a bare continue does not act again: it asks, saying it repeats with the same key
            let c = feed(&mut run, Input::Continue { by: By::Person, starter: None });
            let c = probe_ok(&mut run, c);
            assert!(!c.iter().any(|c| matches!(c, Command::RunRole { .. })), "{effect:?}, {how}: continue acted again: {c:?}");
            let q = asked_in(&c).filter(|q| q.kind == QuestionKind::Acted).unwrap_or_else(|| panic!("{effect:?}, {how}: asked nothing: {c:?}"));
            assert!(q.about.contains("may have acted") && q.about.contains("same key"), "{}", q.about);
            assert_eq!(q.offered, vec![VerdictKind::Redo]);
            // the person says to act again: the same intent, with the same key
            let c = feed(&mut run, answer_to(&q, &[("send", redo("go on"))]));
            let c = probe_ok(&mut run, c);
            let again: Vec<Option<String>> = c.iter().filter_map(|c| if let Command::RunRole { key, .. } = c { Some(key.clone()) } else { None }).collect();
            assert_eq!(again.len(), 1, "{effect:?}, {how}: {c:?}");
            if effect.irreversible() {
                assert_eq!(again[0].as_deref(), Some(key.as_str()), "{how}: the same intent keeps its key");
            }
        }
    }
}

#[test]
fn g14_an_act_that_repeats_safely_starts_again_after_a_stop_with_the_same_key() {
    let (mut run, key) = acting(Effect::Compensable { undo: "revert".to_string() }, true);
    let c = feed(&mut run, Input::Recovered);
    let c = probe_ok(&mut run, c);
    assert!(!matches!(run.phase, Phase::Blocked { .. }), "{:?}", run.phase);
    let (mut run2, key2) = acting(Effect::Irreversible { idempotent: true }, false);
    let c2 = feed(&mut run2, Input::Recovered);
    let c2 = probe_ok(&mut run2, c2);
    let again = c2.iter().find_map(|c| if let Command::RunRole { key, .. } = c { key.clone() } else { None }).expect("started again");
    assert_eq!(again, key2, "the same intent keeps its key");
    let _ = (c, key);
}

/// A step that acts, delivered (so it acted) and whose check then failed or could not judge: continuing does not act
/// again. A check that could not judge judges the same act again; one that failed has the run ask the person, and only
/// their order is a second act.
#[test]
fn g14_an_act_whose_check_failed_is_not_done_again_when_the_person_continues() {
    for verdict in [CheckVerdict::Fail { reasons: vec!["not landed".to_string()], detail: String::new(), cases: vec![] }, CheckVerdict::Blocked { reason: "could not look".to_string() }] {
        let mut send = step("send", Executor::Role);
        send.effect = Effect::Irreversible { idempotent: false };
        send.checks = vec![CheckDecl { name: "landed".to_string(), kind: CheckKind::Open, about: String::new(), reads: Vec::new() }];
        let (mut run, cmds) = start(vec![send]);
        let c = probe_ok(&mut run, cmds);
        let first = pushes_of(&c, "send");
        assert_eq!(first.len(), 1, "{c:?}");
        let attempt = run.steps[0].attempt;
        let c = deliver(&mut run, "send", attempt, "sent", Vec::new());
        assert!(c.iter().any(|c| matches!(c, Command::RunChecks { .. })), "{c:?}");
        let attempt = run.steps[0].attempt;
        feed(&mut run, Input::ChecksEnded { step: "send".to_string(), attempt, fix: 0, results: vec![CheckResult { check: "landed".to_string(), verdict: verdict.clone(), tree: None }] });
        assert!(matches!(run.phase, Phase::Blocked { .. }), "{:?}", run.phase);
        if matches!(verdict, CheckVerdict::Blocked { .. }) {
            // the check judges the same act again, and nothing acts
            let c = feed(&mut run, Input::Continue { by: By::Person, starter: None });
            assert!(pushes_of(&c, "send").is_empty(), "continuing acted again: {c:?}");
            assert!(c.iter().any(|c| matches!(c, Command::RunChecks { step, attempt: a, .. } if step == "send" && *a == attempt)), "{c:?}");
            let failed = CheckVerdict::Fail { reasons: vec!["not landed".to_string()], detail: String::new(), cases: vec![] };
            feed(&mut run, Input::ChecksEnded { step: "send".to_string(), attempt, fix: 0, results: vec![CheckResult { check: "landed".to_string(), verdict: failed, tree: None }] });
            assert!(matches!(run.phase, Phase::Blocked { .. }), "{:?}", run.phase);
        }
        let c = feed(&mut run, Input::Continue { by: By::Person, starter: None });
        let c = probe_ok(&mut run, c);
        assert!(pushes_of(&c, "send").is_empty(), "{verdict:?}: continuing acted again: {c:?}");
        let held = asked_in(&c).filter(|q| q.kind == QuestionKind::Acted).expect("the run asks about the act");
        assert_eq!(held.offered, vec![VerdictKind::Redo], "an act whose check did not pass cannot be kept as it is");
        // the person orders a second act: it acts, with a key of its own
        let c = feed(&mut run, answer_to(&held, &[("send", redo("send it again"))]));
        let c = probe_ok(&mut run, c);
        let again = pushes_of(&c, "send");
        assert_eq!(again.len(), 1, "{c:?}");
        assert_ne!(again, first, "a second act has its own key");
    }
}

fn pushes_of(cmds: &[Command], name: &str) -> Vec<Option<String>> {
    cmds.iter().filter_map(|c| if let Command::RunRole { step, key, .. } = c { (step == name).then(|| key.clone()) } else { None }).collect()
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
        results: vec![CheckResult { check: "short".to_string(), verdict: CheckVerdict::Fail { reasons: vec!["93 characters".to_string()], detail: String::new(), cases: vec![] }, tree: None }],
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
        outcome: AttemptOutcome::Failed { reasons: vec!["slow".to_string()], usage: Usage { seconds: 99_999, tokens: 1, cached: 0, ..Default::default() } },
    });
    assert!(matches!(run.phase, Phase::Blocked { fault: Fault::Ceiling, .. }), "{:?}", run.phase);
}

#[test]
fn g7_what_the_person_s_git_obeys_when_the_delivery_lands_raises_the_risk() {
    for path in [".gitattributes", "lib/.gitattributes", ".gitmodules", ".lfsconfig", "vendor/.lfsconfig"] {
        let mut change = step("change", Executor::Agent);
        change.reach = Reach { project: ProjectAccess::Write, write: vec!["**".to_string()], ..Reach::default() };
        let (mut run, cmds) = start(vec![change]);
        assert_eq!(run.risk, Risk::Low);
        let c = probe_ok(&mut run, cmds);
        let t = c.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.attempt) } else { None }).expect("a turn");
        feed(&mut run, Input::AttemptEnded {
            step: "change".to_string(),
            attempt: t,
            fix: 0,
            outcome: AttemptOutcome::Delivered { output: Digest("o".to_string()), items: Vec::new(), changed: vec![Changed { what: path.to_string(), size: 1, floor: false, ..Default::default() }], usage: Usage::default(), session: None, tree: None, reply: None },
        });
        assert_eq!(run.risk, Risk::High, "G7 {path} left the run at low risk");
    }
}

/// A run of one agent step on the whole project, delivered by merge, whose attempt brings in `changed`: what it
/// commands once the change is in.
fn merging(changed: Vec<Changed>) -> (Run, Vec<Command>) {
    let mut change = step("change", Executor::Agent);
    change.reach = Reach { project: ProjectAccess::Write, write: vec!["**".to_string()], ..Reach::default() };
    let manifest = Manifest { role: "story".to_string(), version: "1".to_string(), protocol: 1, steps: vec![change], target: Default::default(), delivery: jaynes_core::manifest::DeliveryKind::Merge };
    let out = decide(None, &Input::Start { run: "r".to_string(), request: "do it".to_string(), manifest, role_fingerprint: Digest("f".to_string()), agent: "claude:m1".to_string(), raise: None, attention: BTreeMap::new(), task: None, arm: None, never_merge: false, starter: None }).expect("start");
    let mut run = Run::fold(&out.events).expect("run");
    let c = probe_ok(&mut run, out.commands);
    let attempt = c.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.attempt) } else { None }).expect("a turn");
    let c = feed(&mut run, Input::AttemptEnded { step: "change".to_string(), attempt, fix: 0, outcome: AttemptOutcome::Delivered { output: Digest("o".to_string()), items: Vec::new(), changed, usage: Usage::default(), session: None, tree: None, reply: None } });
    (run, c)
}

fn merge_asked(c: &[Command]) -> Option<Question> {
    c.iter().find_map(|c| if let Command::Ask { question } = c { (question.kind == QuestionKind::Merge).then(|| question.clone()) } else { None })
}

fn delivered_merge(c: &[Command]) -> Option<bool> {
    c.iter().find_map(|c| if let Command::Deliver { merge, .. } = c { Some(*merge) } else { None })
}

#[test]
fn g19_a_merge_that_brings_in_what_runs_later_waits_for_the_person_who_alone_answers_it() {
    let envrc = Changed { what: ".envrc".to_string(), size: 1, floor: true, digest: Some("b1".to_string()), ..Default::default() };
    let (mut run, c) = merging(vec![envrc.clone(), Changed::file("src/a.rs".to_string(), 1)]);
    // the run waits before the merge, on a question that names each such file and is bound to its change
    assert_eq!(delivered_merge(&c), None, "G19 the merge went out unasked: {c:?}");
    let q = merge_asked(&c).expect("G19 the merge asks the person");
    assert!(q.holds);
    assert_eq!(q.items.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), [".envrc", "src/a.rs"], "every file that lands");
    assert_eq!(q.items.iter().find(|x| x.id == ".envrc").map(|x| x.digest.0.as_str()), Some("b1"));
    assert!(q.about.contains("run or are obeyed later outside any box"), "{}", q.about);
    assert_eq!(run.risk, Risk::High, "G7 still raises the risk");
    // no agent and no rule answers it
    let keep = |by: By| Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts: q.items.iter().map(|x| (x.id.clone(), Verdict::Keep)).collect(), by, context: String::new() };
    let says = jaynes_core::effects(&run, &q, &keep(By::Person)).sentences;
    assert_eq!(says, ["The delivery merges into your branch, with .envrc."], "only what runs later is named");
    for by in [By::Agent { who: "claude".to_string() }, By::Rule { rule: "anything".to_string() }] {
        assert!(decide(Some(&run), &Input::Answer { answer: keep(by.clone()) }).is_err(), "G19 {by:?} answered the merge");
    }
    // the person keeps it: it merges
    assert_eq!(delivered_merge(&feed(&mut run, Input::Answer { answer: keep(By::Person) })), Some(true));
    // dropped: it is delivered on the run's branch only
    let (mut run, c) = merging(vec![envrc]);
    let q = merge_asked(&c).expect("asked");
    let drop = Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts: BTreeMap::from([(".envrc".to_string(), Verdict::Drop)]), by: By::Person, context: String::new() };
    assert_eq!(delivered_merge(&feed(&mut run, Input::Answer { answer: drop })), Some(false));
    // a manifest the measure says changes neither what runs nor where code comes from does not hold it, though it
    // raises the risk; without the measure's word, a manifest holds it
    let quiet = Changed { what: "package.json".to_string(), size: 1, floor: true, holds: Some(false), ..Default::default() };
    let (run, c) = merging(vec![quiet.clone()]);
    assert_eq!((run.risk, delivered_merge(&c)), (Risk::High, None), "{c:?}");
    assert!(merge_asked(&c).is_some_and(|q| q.about.starts_with("This is what lands on your branch.")), "the merge waits, holding nothing that runs later: {c:?}");
    let (_, c) = merging(vec![Changed { holds: None, ..quiet }]);
    assert!(merge_asked(&c).is_some(), "G19 a manifest with no word from the measure went out unasked");
    // nothing on the floor: the merge still waits for the person, showing what lands, and names nothing as running
    // later outside any box
    let (_, c) = merging(vec![Changed::file("src/a.rs".to_string(), 1)]);
    assert_eq!(delivered_merge(&c), None);
    assert_eq!(merge_asked(&c).map(|q| q.items.into_iter().map(|x| x.id).collect::<Vec<_>>()), Some(vec!["src/a.rs".to_string()]));
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
            changed: vec![Changed::file(".github/workflows/ci.yml".to_string(), 1)],
            usage: Usage::default(),
            session: None,
            tree: None,
            reply: None,
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
            usage: Usage { seconds: 30, tokens: 900, cached: 0, ..Default::default() },
            session: Some("s1".to_string()),
        },
    });
    let q = c.iter().find_map(|c| match c { Command::Ask { question } => Some(question.clone()), _ => None }).expect("the person is asked");
    assert!(q.items[0].label.contains("contradict"), "{q:?}");
    assert!(matches!(run.phase, Phase::Waiting { .. }));
    assert_eq!((run.steps[0].spent, run.spent.tokens), (0, 900), "no attempt spent, the tokens counted");
    // an agent may not answer it
    let agent = Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts: [("impossible".to_string(), Verdict::Keep)].into_iter().collect(), by: By::Agent { who: "a".into() }, context: String::new() };
    assert!(decide(Some(&run), &Input::Answer { answer: agent }).is_err());
    // the person says how; the step tries again in the same session, with their words
    let c = feed(&mut run, Input::Answer {
        answer: Answer { back: Default::default(),
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
    feed(&mut run, Input::SessionStarted { step: "build".to_string(), attempt: 1, session: "s1".to_string(), confirmed: Default::default() });
    assert_eq!(run.steps[0].resume.as_deref(), Some("s1"));
    // a session of an attempt no longer under way changes nothing
    let late = decide(Some(&run), &Input::SessionStarted { step: "build".to_string(), attempt: 7, session: "old".to_string(), confirmed: Default::default() }).expect("recorded as late");
    assert!(late.events.iter().all(|e| matches!(e, Event::StaleReport { .. })));
    // Jaynes dies mid-turn: the next attempt goes on in that session, and spends nothing
    let c = feed(&mut run, Input::Recovered);
    let c = probe_ok(&mut run, c);
    let turn = c.iter().find_map(|c| match c { Command::RunAgent(t) => Some(t.clone()), _ => None }).expect("again");
    assert_eq!((turn.attempt, turn.session.as_deref(), run.steps[0].spent), (2, Some("s1"), 0));
    // the agent's own failure: the next attempt starts afresh
    feed(&mut run, Input::SessionStarted { step: "build".to_string(), attempt: 2, session: "s1".to_string(), confirmed: Default::default() });
    let c = feed(&mut run, Input::AttemptEnded { step: "build".to_string(), attempt: 2, fix: 0, outcome: AttemptOutcome::Failed { reasons: vec!["no output".to_string()], usage: Usage::default() } });
    let turn = c.iter().find_map(|c| match c { Command::RunAgent(t) => Some(t.clone()), _ => None }).expect("again");
    assert_eq!((turn.attempt, turn.session.as_deref()), (3, None));
}

#[test]
fn every_check_leaves_its_kind_its_verdict_and_what_it_read_in_the_record() {
    use jaynes_core::manifest::{CheckDecl, CheckKind};
    let cases = step("cases", Executor::Agent);
    let mut build = step("build", Executor::Agent);
    build.checks = vec![
        CheckDecl { name: "suite".to_string(), kind: CheckKind::Open, about: String::new(), reads: Vec::new() },
        CheckDecl { name: "proof".to_string(), kind: CheckKind::Hidden, about: String::new(), reads: vec!["cases".to_string()] },
    ];
    let (mut run, cmds) = start(vec![cases, build]);
    let c = probe_ok(&mut run, cmds);
    let _ = c;
    let c = deliver(&mut run, "cases", 1, "c", Vec::new());
    probe_ok(&mut run, c);
    deliver(&mut run, "build", 1, "b", Vec::new());
    let out = decide(Some(&run), &Input::ChecksEnded {
        step: "build".to_string(),
        attempt: 1,
        fix: 0,
        results: vec![
            CheckResult { check: "suite".to_string(), verdict: CheckVerdict::Pass, tree: None },
            CheckResult { check: "proof".to_string(), verdict: CheckVerdict::Pass, tree: None },
        ],
    })
    .expect("checked");
    let judged = out.events.iter().find_map(|e| match e { Event::ChecksJudged { checks, .. } => Some(checks.clone()), _ => None }).expect("recorded");
    assert_eq!(judged.len(), 2);
    assert_eq!((judged[0].check.as_str(), judged[0].kind, judged[0].passed), ("suite", CheckKind::Open, true));
    assert_eq!((judged[1].check.as_str(), judged[1].kind), ("proof", CheckKind::Hidden));
    assert_eq!(judged[1].read, vec!["cases".to_string()], "the hidden check read the cases");
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
        answer: Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts: [("leave".to_string(), Verdict::Drop)].into_iter().collect(), by: By::Person, context: "not outside".to_string() },
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
    let manifest = Manifest { role: "bad".to_string(), version: "1".to_string(), protocol: 1, steps: vec![a, b], target: Default::default(), delivery: Default::default() };
    let refused = decide(None, &Input::Start { run: "r".to_string(), request: String::new(), manifest, role_fingerprint: Digest::default(), agent: "claude:m1".to_string(), raise: None, attention: BTreeMap::new(), task: None, arm: None, never_merge: false, starter: None });
    assert!(refused.is_err());
}

#[allow(dead_code)]
fn unused(_: QuestionKind) {}

/// The random runs must reach the states the guarantees are about, or passing them proves nothing.
#[test]
fn the_random_runs_reach_what_the_guarantees_are_about() {
    let mut seen: BTreeMap<&'static str, u32> = BTreeMap::new();
    let mut role_makers = 0;
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
        let run = w.run().clone();
        let from_role = |q: &String| {
            run.questions.get(q).and_then(|q| run.manifest.index(&q.question.step)).is_some_and(|i| run.manifest.steps[i].executor == Executor::Role)
        };
        role_makers += w.log.iter().filter(|e| matches!(e, Event::StepInvalidated { from: Some(q), .. } if from_role(q))).count();
        let mut hit = |k: &'static str| *seen.entry(k).or_default() += 1;
        if w.run().finished() {
            hit("finished");
        }
        if w.rechecked > 0 {
            hit("a change judged again on the person's continue");
        }
        if w.asked > 0 {
            hit("a stop's agent asked");
        }
        for e in &w.log {
            match e {
                Event::StepInvalidated { because, .. } if because.iter().any(|b| b.contains("second act")) => hit("a second act ordered"),
                Event::StepInvalidated { because, .. } if because.iter().any(|b| b.contains("came out different")) => hit("rerun because an input changed"),
                Event::StepInvalidated { narrow: Some(_), .. } => hit("a choice kept alone"),
                Event::StepInvalidated { from: Some(_), .. } => hit("sent back to its maker"),
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
                Event::QuestionOpened { question } if question.kind == QuestionKind::Acted => hit("an act held, not done again"),
                Event::ActKept { .. } => hit("an act kept as it acted"),
                Event::QuestionOpened { question } if question.kind == QuestionKind::Merge => hit("a merge held for the person"),
                Event::Continued { by: By::Agent { .. } } => hit("continued by the agent that started the run"),
                Event::SetAside { .. } => hit("set aside by the agent that started the run"),
                Event::AgentAnswered { late: false, .. } => hit("an ask answered in time"),
                Event::AgentAnswered { late: true, .. } => hit("an ask answered late"),
                Event::AskFailed { .. } => hit("an ask that got no answer"),
                _ => {}
            }
        }
    }
    eprintln!("{seen:#?}");
    eprintln!("sent back to its maker from a step of the role's own: {role_makers}");
    assert!(role_makers >= 5, "the random runs sent work back from a step of the role's own to its maker only {role_makers} times");
    for k in [
        "finished", "rerun because an input changed", "sent back", "late report", "fix in place", "risk raised by a rule",
        "waits before what cannot be undone", "fault of Jaynes", "leave granted by a rule", "released by an agent", "a certified stop skipped", "question superseded", "delivered",
        "a choice kept alone", "sent back to its maker", "an act held, not done again", "an act kept as it acted", "a second act ordered", "a merge held for the person",
        "a change judged again on the person's continue", "continued by the agent that started the run", "set aside by the agent that started the run",
        "a stop's agent asked", "an ask answered in time", "an ask answered late", "an ask that got no answer",
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
    w.send(Input::Start { run: format!("run-{seed}"), request: String::new(), manifest, role_fingerprint: Digest("role".to_string()), agent: "claude:m1".to_string(), raise, attention: BTreeMap::new(), task: None, arm: None, never_merge: false, starter: None });

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
    change.stop = Some(StopDecl { settled: None, verdicts: vec![VerdictKind::Keep, VerdictKind::Redo], holds: true, from_risk: Risk::Low, release: true, budget: None, audit_rate: jaynes_core::manifest::Rate(0.1), about: String::new(), choose: false, redo: None, rounds: None, answers: Default::default(), note: String::new() });
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
    let redo = Answer { back: Default::default(), context: String::new(), question: q.id.clone(), shown: q.shown(), verdicts: [("change".to_string(), Verdict::Redo { note: "no".to_string() })].into_iter().collect(), by: agent.clone() };
    assert!(decide(Some(&run), &Input::Answer { answer: redo }).is_err());
    // letting it go on is allowed, and the run goes on to tests
    let c = feed(&mut run, Input::Answer { answer: Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts: BTreeMap::new(), by: agent , context: String::new()} });
    let c = probe_ok(&mut run, c);
    assert!(c.iter().any(|c| matches!(c, Command::RunAgent(t) if t.step == "tests")), "{c:?}");
    assert!(run.steps[0].locks.is_empty(), "an agent's release locks nothing");
    let c = deliver(&mut run, "tests", 1, "t", Vec::new());
    // push cannot be undone, and rests on an answer only an agent gave: it waits for the person
    assert!(!c.iter().any(|c| matches!(c, Command::RunRole { .. } | Command::Probe { .. })), "{c:?}");
    assert!(matches!(&run.phase, Phase::Waiting { step: Some(s), .. } if s == "push"), "{:?}", run.phase);
    // the person confirms, and push runs
    let c = feed(&mut run, Input::Answer { answer: Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts: BTreeMap::new(), by: By::Person , context: String::new()} });
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
    let question = Question { rides: Vec::new(), back: Default::default(),
        id: "q1-change".to_string(),
        step: "change".to_string(),
        round: 0,
        kind: QuestionKind::Decision,
        items: vec![item("change", "c")],
        offered: vec![VerdictKind::Keep, VerdictKind::Redo],
        holds: false,
        about: String::new(),
        choose: false,
        pass: 0,
        passes: 0,
    };
    let verdicts: BTreeMap<String, Verdict> = verdict.into_iter().map(|v| ("change".to_string(), v)).collect();
    let answer = Answer { back: Default::default(), context: String::new(), question: question.id.clone(), shown: question.shown(), verdicts, by };
    let c = Ceilings::default();
    let r = run.to_string();
    vec![
        (at, r.clone(), Event::RunStarted {
            run: r.clone(), request: String::new(), manifest: manifest.clone(), role_fingerprint: Digest("f".to_string()),
            agent: "claude:m1".to_string(), engine: String::new(), protocol: 1, format: 1, risk: Risk::Low, reasons: Vec::new(), ceilings: c, ceiling: c.low,
            attention: BTreeMap::new(), task: None, arm: None, never_merge: false,
            starter: None,
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
    change.stop = Some(StopDecl { settled: None, verdicts: vec![VerdictKind::Keep, VerdictKind::Redo], holds: false, from_risk: Risk::Low, release: true, budget: Some(jaynes_core::manifest::Rate(0.2)), audit_rate: jaynes_core::manifest::Rate(0.1), about: String::new(), choose: false, redo: None, rounds: None, answers: Default::default(), note: String::new() });
    let manifest = Manifest { role: "story".to_string(), version: "1".to_string(), protocol: 1, steps: vec![change], target: Default::default(), delivery: Default::default() };
    let key = stop_key(&manifest, &Digest("f".to_string()), 0);
    let lin = jaynes_core::attention::lineage(&manifest, 0);
    let day = 24 * 3600;
    let mut h = Vec::new();
    for n in 0..25 {
        h.extend(history_run(&format!("r{n}"), 1000, None, By::Person, &manifest));
    }
    // an accept counts as right only once its delivery held seven days
    assert_eq!(count(&h, &key, &lin, "claude:m1", 1000 + day, Some(0.2), 0.05).n, 0);
    assert_eq!(count(&h, &key, &lin, "claude:m1", 1000 + day, Some(0.2), 0.05).pending, 25);
    let c = count(&h, &key, &lin, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
    assert_eq!((c.n, c.k, c.braked), (25, 0, false));
    assert!(jaynes_core::attention::certified(c.n, c.k, 0.2, 0.05));
    // what an agent let through never earns a right
    let mut agent = h.clone();
    agent.extend(history_run("ra", 1000, None, By::Agent { who: "claude".to_string() }, &manifest));
    assert_eq!(count(&agent, &key, &lin, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05).n, 25);
    // a correction after delivery turns a right into an error, and two errors among the last ten brake the stop
    let mut corrected = h.clone();
    for run in ["r23", "r24"] {
        corrected.push((2000, run.to_string(), Event::DeliveryCorrected { note: "wrong".to_string(), by: By::Person, step: Some("change".to_string()) }));
    }
    let c = count(&corrected, &key, &lin, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
    assert_eq!((c.n, c.k, c.braked), (25, 2, true));
    // another model starts with the stop's errors and none of its rights
    assert_eq!(count(&h, &key, &lin, "claude:m2", 1000 + 8 * day, Some(0.2), 0.05).n, 0, "rights earned by m1 are not m2's");
    let c = count(&corrected, &key, &lin, "claude:m2", 1000 + 8 * day, Some(0.2), 0.05);
    assert_eq!((c.n, c.k, c.braked), (2, 2, true), "errors made with m1 count for m2 too");
    // a send-back counts at once
    let mut back = h.clone();
    back.extend(history_run("rb", 1000, Some(Verdict::Redo { note: "no".to_string() }), By::Person, &manifest));
    assert_eq!(count(&back, &key, &lin, "claude:m1", 1000 + day, Some(0.2), 0.05).k, 1);
}

#[test]
fn g17_i_do_not_know_counts_for_nothing() {
    use jaynes_core::attention::count;
    let mut change = step("change", Executor::Agent);
    change.stop = Some(StopDecl { settled: None, verdicts: vec![VerdictKind::Keep, VerdictKind::Redo, VerdictKind::Skip], holds: false, from_risk: Risk::Low, release: true, budget: Some(jaynes_core::manifest::Rate(0.2)), audit_rate: jaynes_core::manifest::Rate(0.1), about: String::new(), choose: false, redo: None, rounds: None, answers: Default::default(), note: String::new() });
    let manifest = Manifest { role: "story".to_string(), version: "1".to_string(), protocol: 1, steps: vec![change], target: Default::default(), delivery: Default::default() };
    let key = stop_key(&manifest, &Digest("f".to_string()), 0);
    let lin = jaynes_core::attention::lineage(&manifest, 0);
    let day = 24 * 3600;
    // the person said "I don't know" on 25 runs: no accept, so nothing is right, nothing pending, nothing certified
    let mut skipped = Vec::new();
    for n in 0..25 {
        skipped.extend(history_run(&format!("s{n}"), 1000, Some(Verdict::Skip), By::Person, &manifest));
    }
    let c = count(&skipped, &key, &lin, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
    assert_eq!((c.n, c.k, c.pending), (0, 0, 0), "I don't know counted");
    assert!(!jaynes_core::attention::certified(c.n, c.k, 0.2, 0.05));
    // among accepts and a send-back, they change neither how many outcomes are known nor the errors among them
    let mut mixed = Vec::new();
    for n in 0..10 {
        mixed.extend(history_run(&format!("k{n}"), 1000, None, By::Person, &manifest));
    }
    mixed.extend(history_run("b", 1000, Some(Verdict::Redo { note: "no".to_string() }), By::Person, &manifest));
    let without = count(&mixed, &key, &lin, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
    let mut with = mixed.clone();
    for n in 0..10 {
        with.extend(history_run(&format!("s{n}"), 1000, Some(Verdict::Skip), By::Person, &manifest));
    }
    let c = count(&with, &key, &lin, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
    assert_eq!((c.n, c.k, c.pending), (without.n, without.k, without.pending), "I don't know changed the count");
    assert_eq!((c.n, c.k), (11, 1));
    // an answer that kept one item and did not know the other left one unjudged: it is no accept either
    let mut partly = mixed.clone();
    for n in 0..10 {
        let mut run = history_run(&format!("p{n}"), 1000, None, By::Person, &manifest);
        for (_, _, e) in run.iter_mut() {
            match e {
                Event::QuestionOpened { question } => question.items.push(item("other", "o")),
                Event::AnswerRecorded { answer, .. } => {
                    answer.verdicts = [("change".to_string(), Verdict::Keep), ("other".to_string(), Verdict::Skip)].into_iter().collect();
                }
                _ => {}
            }
        }
        partly.extend(run);
    }
    let c = count(&partly, &key, &lin, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
    assert_eq!((c.n, c.k, c.pending), (without.n, without.k, without.pending), "an answer with an item unjudged counted");
}

#[test]
fn g17_a_stop_that_showed_a_planted_defect_is_out_of_the_count_whatever_is_answered() {
    use jaynes_core::attention::count;
    let manifest = released_stop();
    let key = stop_key(&manifest, &Digest("f".to_string()), 0);
    let lin = jaynes_core::attention::lineage(&manifest, 0);
    let day = 24 * 3600;
    // a run whose stop showed a defect an evaluation planted, answered with `verdict` while it was in, or after
    let planted = |run: &str, verdict: Option<Verdict>, while_in: bool| {
        let mut events = history_run(run, 1000, verdict, By::Person, &manifest);
        let answered = events.iter().position(|(_, _, e)| matches!(e, Event::AnswerRecorded { .. })).expect("answered");
        let out = if while_in { answered + 1 } else { answered };
        events.insert(out, (1000, run.to_string(), Event::DefectRemoved { id: "d1".to_string(), by: By::Person }));
        events.insert(answered, (1000, run.to_string(), Event::DefectPlanted { id: "d1".to_string(), step: "change".to_string(), by: By::Person }));
        events
    };
    let mut plain = Vec::new();
    for n in 0..10 {
        plain.extend(history_run(&format!("k{n}"), 1000, None, By::Person, &manifest));
    }
    let without = count(&plain, &key, &lin, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
    // kept, sent back, changed, or not known, while the defect was in: none of it moves the count
    let answers: [(&str, Option<Verdict>); 4] = [
        ("keep", None),
        ("redo", Some(Verdict::Redo { note: "no".to_string() })),
        ("change", Some(Verdict::Change { text: "this instead".to_string() })),
        ("skip", Some(Verdict::Skip)),
    ];
    for (name, verdict) in answers.iter() {
        let mut with = plain.clone();
        for n in 0..10 {
            with.extend(planted(&format!("{name}{n}"), verdict.clone(), true));
        }
        let c = count(&with, &key, &lin, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
        assert_eq!((c.n, c.k, c.pending, c.braked), (without.n, without.k, without.pending, without.braked), "{name} at a stop that showed a planted defect moved the count");
    }
    // a correction of that step's delivery while the defect is in finds no outcome of that stop to mark: none was counted
    let mut corrected = plain.clone();
    for n in 0..10 {
        let mut run = planted(&format!("c{n}"), None, true);
        let out = run.iter().position(|(_, _, e)| matches!(e, Event::DefectRemoved { .. })).expect("taken out");
        run.insert(out, (1000, format!("c{n}"), Event::DeliveryCorrected { note: "wrong".to_string(), by: By::Person, step: Some("change".to_string()) }));
        corrected.extend(run);
    }
    let c = count(&corrected, &key, &lin, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
    assert_eq!((c.n, c.k, c.pending), (without.n, without.k, without.pending), "a correction while a planted defect was in moved the count");
    // answered once it was out, each counts as any answer: a keep is an outcome, a send-back an error
    let mut after = plain.clone();
    for n in 0..10 {
        after.extend(planted(&format!("a{n}"), None, false));
    }
    assert_eq!(count(&after, &key, &lin, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05).n, without.n + 10, "a keep after the defect was out did not count");
    let mut back = plain.clone();
    back.extend(planted("b", Some(Verdict::Redo { note: "no".to_string() }), false));
    assert_eq!(count(&back, &key, &lin, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05).k, without.k + 1, "a send-back after the defect was out did not count");
}

fn released_stop() -> Manifest {
    let mut change = step("change", Executor::Agent);
    change.stop = Some(StopDecl { settled: None, verdicts: vec![VerdictKind::Keep, VerdictKind::Redo, VerdictKind::Skip], holds: false, from_risk: Risk::Low, release: true, budget: Some(jaynes_core::manifest::Rate(0.2)), audit_rate: jaynes_core::manifest::Rate(0.1), about: String::new(), choose: false, redo: None, rounds: None, answers: Default::default(), note: String::new() });
    Manifest { role: "story".to_string(), version: "1".to_string(), protocol: 1, steps: vec![change], target: Default::default(), delivery: Default::default() }
}

#[test]
fn g17_a_stop_s_errors_cross_a_change_of_its_role_and_its_rights_do_not() {
    use jaynes_core::attention::{count, lineage};
    let manifest = released_stop();
    let (key, lin) = (stop_key(&manifest, &Digest("f".to_string()), 0), lineage(&manifest, 0));
    let day = 24 * 3600;
    let mut h = Vec::new();
    for n in 0..10 {
        h.extend(history_run(&format!("r{n}"), 1000, None, By::Person, &manifest));
    }
    for run in ["r8", "r9"] {
        h.push((2000, run.to_string(), Event::DeliveryCorrected { note: "wrong".to_string(), by: By::Person, step: Some("change".to_string()) }));
    }
    let c = count(&h, &key, &lin, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
    assert_eq!((c.n, c.k, c.braked), (10, 2, true));
    // the role changes (its content, its library): the braked stop stays braked, with its errors and none of its rights
    let changed = stop_key(&manifest, &Digest("f2".to_string()), 0);
    let c = count(&h, &changed, &lin, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
    assert_eq!((c.n, c.k, c.braked), (2, 2, true), "a change of the role set a braked stop free");
    // the step changed as declared, same role and name: the same
    let mut stricter = manifest.clone();
    stricter.steps[0].attempts = 1;
    let c = count(&h, &stop_key(&stricter, &Digest("f".to_string()), 0), &lineage(&stricter, 0), "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
    assert_eq!((c.n, c.k, c.braked), (2, 2, true));
    // another role with a step of the same name inherits nothing
    let mut other = manifest.clone();
    other.role = "another".to_string();
    let c = count(&h, &stop_key(&other, &Digest("f".to_string()), 0), &lineage(&other, 0), "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
    assert_eq!((c.n, c.k, c.braked), (0, 0, false));
}

#[test]
fn g17_what_an_agent_lets_go_on_in_your_name_earns_nothing_until_you_confirm_it() {
    use jaynes_core::attention::{count, lineage};
    let manifest = released_stop();
    let (key, lin) = (stop_key(&manifest, &Digest("f".to_string()), 0), lineage(&manifest, 0));
    let day = 24 * 3600;
    let mut released = Vec::new();
    for n in 0..25 {
        released.extend(history_run(&format!("a{n}"), 1000, None, By::Agent { who: "an agent through the Jaynes plugin".to_string() }, &manifest));
    }
    let c = count(&released, &key, &lin, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
    assert_eq!((c.n, c.k, c.pending), (0, 0, 0), "what an agent let go on earned");
    // the person confirms it later, on the same question: then it counts
    let mut confirmed = Vec::new();
    for mut run in released.chunks(4).map(|c| c.to_vec()) {
        let Some((_, id, Event::AnswerRecorded { answer, .. })) = run.get(2).cloned() else { panic!("an answer") };
        let mut mine = answer.clone();
        mine.by = By::Person;
        run.insert(3, (1000, id, Event::AnswerRecorded { answer: mine, effects: Default::default() }));
        confirmed.extend(run);
    }
    let c = count(&confirmed, &key, &lin, "claude:m1", 1000 + 8 * day, Some(0.2), 0.05);
    assert_eq!((c.n, c.k), (25, 0));
}

#[test]
fn g17_a_certified_stop_asks_only_a_sample_and_a_braked_one_holds() {
    let mut change = step("change", Executor::Agent);
    change.stop = Some(StopDecl { settled: None, verdicts: vec![VerdictKind::Keep, VerdictKind::Redo], holds: false, from_risk: Risk::Low, release: true, budget: Some(jaynes_core::manifest::Rate(0.2)), audit_rate: jaynes_core::manifest::Rate(0.1), about: String::new(), choose: false, redo: None, rounds: None, answers: Default::default(), note: String::new() });
    let manifest = Manifest { role: "story".to_string(), version: "1".to_string(), protocol: 1, steps: vec![change], target: Default::default(), delivery: Default::default() };
    let f = Digest("f".to_string());
    let key = stop_key(&manifest, &f, 0);
    let run_with = |id: &str, braked: bool| {
        let attention = [("change".to_string(), Count { key: key.clone(), n: 40, k: 0, pending: 0, braked })].into_iter().collect();
        let out = decide(None, &Input::Start { run: id.to_string(), request: String::new(), manifest: manifest.clone(), role_fingerprint: f.clone(), agent: "claude:m1".to_string(), raise: None, attention, task: None, arm: None, never_merge: false, starter: None }).expect("start");
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
    let agent = Answer { back: Default::default(), question: q.question.id.clone(), shown: q.question.shown(), verdicts: BTreeMap::new(), by: By::Agent { who: "claude".to_string() }, context: String::new() };
    assert!(decide(Some(&run), &Input::Answer { answer: agent }).is_err(), "an agent may not release a braked stop");
}

#[test]
fn the_person_s_context_reaches_the_steps_after_the_answer_and_changes_no_verdict() {
    let mut look = step("look", Executor::Agent);
    look.stop = Some(StopDecl { settled: None, verdicts: vec![VerdictKind::Keep, VerdictKind::Redo], holds: true, from_risk: Risk::Low, release: false, budget: None, audit_rate: jaynes_core::manifest::Rate(0.1), about: String::new(), choose: false, redo: None, rounds: None, answers: Default::default(), note: String::new() });
    let mut copy = step("copy", Executor::Agent);
    copy.needs = vec!["look".to_string()];
    let (mut run, cmds) = start(vec![look, copy]);
    probe_ok(&mut run, cmds);
    let asked = deliver(&mut run, "look", 1, "blue", vec![item("palette", "blue")]);
    let q = asked.iter().find_map(|c| if let Command::Ask { question } = c { Some(question.clone()) } else { None }).expect("asked");
    let answer = Answer { back: Default::default(), context: "it is for accountants".to_string(), question: q.id.clone(), shown: q.shown(), verdicts: BTreeMap::new(), by: By::Person };
    let effects = jaynes_core::effects(&run, &q, &answer);
    assert!(effects.sentences.iter().any(|s| s.contains("Your context goes to every step after look") && s.contains("sends nothing back")), "{effects:?}");
    assert!(effects.reruns.is_empty());
    let c = feed(&mut run, Input::Answer { answer });
    let c = probe_ok(&mut run, c);
    let turn = c.iter().find_map(|c| if let Command::RunAgent(t) = c { Some(t.clone()) } else { None }).expect("copy runs");
    assert_eq!((turn.step.as_str(), turn.context.clone()), ("copy", vec!["it is for accountants".to_string()]));
    assert_eq!(run.steps[0].round, 0, "context sends nothing back");
    let agent = Answer { back: Default::default(), context: "x".to_string(), question: q.id.clone(), shown: q.shown(), verdicts: BTreeMap::new(), by: By::Agent { who: "claude".to_string() } };
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
                "held": run.held(i).is_some(),
                "actedPending": run.acted_unfinished(i) || s.maybe_acted,
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
        "mergeHeld": run.manifest.lands() && !run.undone && !run.never_merge && !run.delivered_files().is_empty(),
        "mergeAnswered": run.merge_answer(&run.delivery().0).is_some(),
        "mergeAsked": run.merge_open(&run.delivery().0).is_some(),
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
                Some(Event::QuestionOpened { question }) if question.kind == QuestionKind::Merge => "ask merge".to_string(),
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

/// When the core's next move asks about a merge: the stops open then, as the model's `Open`, and the ones the
/// question names (`ridesOf`).
fn rides_case(run: &Run) -> Option<String> {
    let mut fresh = run.clone();
    if matches!(fresh.phase, Phase::Waiting { .. }) {
        fresh.phase = Phase::Running;
    }
    let out = decide(Some(&fresh), &Input::Tick).ok()?;
    // the stops open just before the question about the merge, which the same move may have asked first
    let mut run = fresh;
    let mut merge = None;
    for e in &out.events {
        if let Event::QuestionOpened { question } = e
            && question.kind == QuestionKind::Merge
        {
            merge = Some(question.clone());
            break;
        }
        run.apply(e);
    }
    let merge = merge?;
    let run = &run;
    let opens: Vec<serde_json::Value> = run
        .path()
        .into_iter()
        .filter_map(|j| run.open_question(j).map(|q| (j, &q.question)))
        .map(|(j, q)| serde_json::json!({"step": j, "decision": q.kind == QuestionKind::Decision, "holds": q.holds, "keepOffered": q.offered.contains(&VerdictKind::Keep), "keepAnswers": !q.choose || q.items.len() == 1}))
        .collect();
    let rides: Vec<String> = merge.rides.iter().map(|r| run.manifest.index(&r.step).expect("step").to_string()).collect();
    Some(serde_json::json!({"kind": "rides", "opens": opens, "expected": rides.join(",")}).to_string())
}

/// The person's touch at a merge (`TouchCase`): the stops its question named, whether each is still open, and the ones
/// the core kept with it.
fn touch_case(run: &Run, q: &Question, answer: &Answer, kept: &[&str]) -> String {
    let case: serde_json::Value = serde_json::from_str::<serde_json::Value>(&answer_case(run, answer, false)).expect("json")["case"].clone();
    let rides: Vec<serde_json::Value> = q.rides.iter().map(|r| serde_json::json!({"step": run.manifest.index(&r.step).expect("step"), "stillOpen": run.questions.get(&r.question).is_some_and(|x| x.open())})).collect();
    let lands = q.items.iter().all(|x| answer.verdicts.get(&x.id) == Some(&Verdict::Keep));
    let expected: Vec<String> = kept.iter().map(|id| run.manifest.index(&run.questions[*id].question.step).expect("step").to_string()).collect();
    serde_json::json!({"kind": "touch", "case": {"answer": case, "lands": lands, "rides": rides}, "expected": expected.join(",")}).to_string()
}

/// An answer as the model judges it (`AnswerCase`), with whether the core took it.
fn answer_case(run: &Run, answer: &Answer, taken: bool) -> String {
    let q = &run.questions[&answer.question];
    let i = run.manifest.index(&q.question.step).expect("step");
    let d = &run.manifest.steps[i];
    let s = &run.steps[i];
    let acted = q.question.kind == QuestionKind::Acted;
    let merge = q.question.kind == QuestionKind::Merge;
    let valid = if merge {
        answer.verdicts.keys().all(|id| q.question.items.iter().any(|x| &x.id == id)) && answer.back.is_empty()
            && q.question.items.iter().all(|x| matches!(answer.verdicts.get(&x.id), Some(Verdict::Keep | Verdict::Drop)))
    } else if acted {
        answer.verdicts.keys().all(|id| *id == q.question.step) && matches!(answer.verdict(&q.question.step), Verdict::Keep | Verdict::Redo { .. })
            && q.question.offered.contains(&answer.verdict(&q.question.step).kind()) && answer.back.is_empty()
    } else { answer.verdicts.iter().all(|(id, v)| {
        q.question.items.iter().any(|x| &x.id == id)
            && q.question.offered.contains(&v.kind())
            && !matches!(v, Verdict::Change { text } if text.trim().is_empty())
    }) };
    let case = serde_json::json!({
        "who": match answer.by { By::Person => "person", By::Rule { .. } => "rule", By::Agent { .. } => "agent" },
        "decision": q.question.kind == QuestionKind::Decision,
        "personStep": d.executor == Executor::Person,
        "input": s.inputs.contains(&q.question.id),
        "release": d.stop.as_ref().is_some_and(|st| st.release),
        "braked": run.attention.get(&d.name).is_some_and(|c| c.braked),
        "superseded": q.superseded,
        "shownMatches": answer.shown == q.question.shown(),
        "verdictsValid": valid && answer.back.keys().all(|id| q.question.back.iter().any(|x| &x.id == id)) && (!q.question.choose || answer.choice(&q.question).is_some_and(|c| c != jaynes_core::question::Choice::None || !answer.context.trim().is_empty()))
            && !(q.question.kind == QuestionKind::Decision && answer.by.person() && answer.sends_back() && answer.kept_alone(&q.question).is_none()
                && d.stop.as_ref().is_some_and(|st| st.redo.is_some()) && q.question.items.iter().any(|x| answer.verdict(&x.id) == Verdict::Keep)),
        "allKeep": answer.verdicts.values().all(|v| *v == Verdict::Keep) && answer.back.is_empty(),
        "hasContext": !answer.context.trim().is_empty(),
        "answered": q.answer.is_some(),
        "revisable": q.question.kind == QuestionKind::Decision && d.executor != Executor::Person
            && s.question.as_deref() == Some(q.question.id.as_str()) && s.status == Status::Done
            && s.done.as_ref().is_some_and(|x| x.round == q.question.round),
        "acted": acted,
        "merge": merge,
        "current": !merge || run.merges.get(&q.question.id) == Some(&run.delivery().0),
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
        "roleStep": d.executor == Executor::Role || s.narrow.is_some(), "fixesLeft": *fix < d.fixes, "session": s.session.is_some(),
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

/// The model's case for every state each run of a record passed through, read with the sqlite3 program.
fn record_cases(path: &std::path::Path) -> Vec<String> {
    let out = std::process::Command::new("sqlite3").arg("-json").arg(path).arg("select run, body from events order by seq").output().expect("sqlite3");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let mut states: BTreeMap<String, Run> = BTreeMap::new();
    let mut cases = Vec::new();
    for row in rows {
        let (Some(run), Some(body)) = (row["run"].as_str(), row["body"].as_str()) else { continue };
        let Ok(e) = serde_json::from_str::<Event>(body) else { panic!("an entry of {run} does not read: {body}") };
        let next = match states.remove(run) {
            None => Run::started(&e),
            Some(mut r) => {
                r.apply(&e);
                Some(r)
            }
        };
        if let Some(r) = next {
            cases.push(model_case(&r));
            cases.extend(rides_case(&r));
            states.insert(run.to_string(), r);
        }
    }
    cases
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
        // on GitHub the comparison must run: there a missing Lean is a failure, not a skip
        assert!(std::env::var("JAYNES_REQUIRE_LEAN").is_err(), "Lean is required here and is not installed");
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
    // and every state of real runs: JAYNES_MODEL_RECORD names records (record.sqlite3), separated by ':'
    let real = std::env::var("JAYNES_MODEL_RECORD").unwrap_or_default();
    let mut from_records = 0;
    for path in real.split(':').filter(|p| !p.is_empty()) {
        let found = record_cases(std::path::Path::new(path));
        from_records += found.len();
        cases.extend(found);
    }
    if !real.is_empty() {
        assert!(from_records > 0, "no state was read from {real}");
        eprintln!("{from_records} states from real records");
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

