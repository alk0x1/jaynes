//! A stop as the Jaynes app shows it: the question that waits for the person, each item with what it shows, the
//! verdicts the person may give, what lies ahead, and when the app asks for the person's touch before the answer
//! goes: before an answer that leads to a step that cannot be undone, and once at the merge into their branch, never
//! at a leave. What an answer will do is still written by the core (`drive::effects`).

use jaynes_core::question::{Answer, By, QuestionKind, Verdict, VerdictKind};
use jaynes_core::run::Run;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub label: String,
    pub detail: String,
    /// The parts the person judges apart, each with its label.
    pub aspects: Vec<(String, String)>,
    /// Pictures of the item, as files.
    pub pictures: Vec<PathBuf>,
    /// The page the role kept of it, by name, served sandboxed by the page at `/page/<name>`.
    pub page: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stop {
    pub run: String,
    pub question: String,
    /// The fingerprint of the question as shown: an answer counts only for what the person saw (G10).
    pub shown: String,
    pub title: String,
    /// The role's question at this stop ("Which stack goes on?"), empty when it has none. With one option to choose
    /// the title asks whether to go with it, and this goes on the line under it.
    pub asks: String,
    pub step: String,
    pub permission: bool,
    /// The question's kind, for the names of its verdicts ([`label_for`]).
    pub kind: QuestionKind,
    pub choose: bool,
    pub offered: Vec<VerdictKind>,
    pub items: Vec<Item>,
    /// What the person dropped at this stop in earlier rounds: only they bring it back.
    pub back: Vec<Item>,
    /// The round of asking at this stop, from 1, when the role counts rounds.
    pub round: u32,
    /// What the stop's step does, as its role says it.
    pub about: String,
    /// What lies ahead of this stop, in a sentence, if anything does: a step that cannot be undone, or the merge into
    /// the person's branch, which asks them before it lands. Never at a leave, nor at the merge itself.
    pub ahead: Option<String>,
    /// Why macOS asks for the person's touch before this answer goes, when it does: an answer that leads to a step
    /// that cannot be undone, and the merge into their branch, once. Never a leave, nor a decision before a merge.
    pub touch: Option<String>,
    /// The branch the run merges into, at the stop before the merge; empty elsewhere, or when the copy does not say.
    pub into: String,
    /// What the merge lands, at the stop before it: every file with its lines, what runs later outside any box said.
    pub lands: Vec<Lands>,
    /// When the run went on without the person's answer to this stop: where it is now and what an answer still
    /// changes. Such a stop never covers what holds the run now: it is answered from the line below.
    pub passed: Option<Passed>,
    /// The role's words for its verdicts, which the buttons say ([`Stop::label`]): "Yes" for keep, "No, it's
    /// something else" for redo. Only at a stop of the role's: a question of Jaynes's own (a leave, a merge) has
    /// none. What each verdict does is still Jaynes's sentence, shown under the button.
    pub answers: BTreeMap<VerdictKind, String>,
    /// The role's line under the question, said before the person looks ("The layout of your screens comes
    /// later."), drawn dim under it; empty at a question of Jaynes's own, and when the role says none.
    pub note: String,
    /// Why the person is asked here, when Jaynes has more to say than the title: a leave's "Why you: ...".
    pub why: Option<String>,
    /// What each answer does, in a sentence beside its button, where Jaynes says it there (a leave's).
    pub consequences: BTreeMap<VerdictKind, String>,
    /// What the agent wants to write, for a leave to change a file: the file as it is in the step's copy, and as the
    /// agent would leave it.
    pub wants: Option<Wants>,
    /// The run's engine, when an older one started it and is no longer on this machine: no answer reaches the run,
    /// and the card says so before Send.
    pub gone: Option<Gone>,
    /// What the person may grant here beside their answer, each with its box marked or not beforehand: sent with the
    /// answer as the page's act "grant", the person's alone (`Input::Grant`). Empty at most stops.
    pub offers: Vec<Offered>,
}

/// Something a stop offers the person to grant, as the card shows it: a box to mark, with its words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offered {
    pub offer: jaynes_core::manifest::Offer,
    /// The box's words.
    pub says: String,
    /// What granting it does and what it still keeps, under the box.
    pub then: String,
    /// The box is marked beforehand, as the step's output says (the role's offers_default): for a field of the
    /// items, as the first item says.
    pub marked: bool,
    /// For a field of the items the stop shows, how each item marks the box, by its id: when the person picks
    /// another item, the box follows it, unless they set the box themselves. Empty for a field of the whole output.
    pub marked_for: BTreeMap<String, bool>,
}

/// What `q`'s stop offers the person to grant and they have not: on macOS only, where the box keeps tests from
/// serving, and only while the project's test servers are not let yet. Marked beforehand by the boolean field the
/// role names of the step's output.
pub fn offered(store: Option<&crate::store::Store>, run: &Run, q: &jaynes_core::run::QuestionState) -> Vec<Offered> {
    use jaynes_core::manifest::Offer;
    if !q.open() || !cfg!(target_os = "macos") {
        return Vec::new();
    }
    let Some(i) = run.manifest.index(&q.question.step) else { return Vec::new() };
    let Some(stop) = run.manifest.steps[i].stop.as_ref() else { return Vec::new() };
    // the output the stop shows: the step's last delivery, as the record keeps it ({"output", "tree"})
    let step = &q.question.step;
    let delivered = store.and_then(|st| {
        let events = st.events(&run.id).ok()?;
        let digest = events.iter().rev().find_map(|e| match e {
            jaynes_core::event::Event::AttemptDelivered { step: s, output, .. } if s == step => Some(output.clone()),
            _ => None,
        })?;
        st.get(&digest).ok()
    });
    let output = delivered.map(|d| d["output"].clone());
    let servers = run.ports.as_ref().is_some_and(|p| p.servers || p.any);
    stop.offers
        .list
        .iter()
        .filter(|o| !run.granted.contains(o) && !(**o == Offer::TestServers && servers))
        .map(|o| {
            let field = stop.offers.marked_by.get(o);
            let marked_for: BTreeMap<String, bool> = match field.and_then(|f| f.split_once('.')) {
                Some((list, each)) => {
                    let elements = output.as_ref().and_then(|out| out[list].as_array().cloned()).unwrap_or_default();
                    // the items are the list's elements in order, as the role showed them
                    if elements.len() == q.question.items.len() {
                        q.question.items.iter().zip(&elements).map(|(it, e)| (it.id.clone(), e[each].as_bool().unwrap_or(false))).collect()
                    } else {
                        BTreeMap::new()
                    }
                }
                None => BTreeMap::new(),
            };
            let marked = match field {
                Some(f) if f.contains('.') => q.question.items.first().and_then(|it| marked_for.get(&it.id)).copied().unwrap_or(false),
                Some(f) => output.as_ref().and_then(|out| out.get(f.as_str())?.as_bool()).unwrap_or(false),
                None => false,
            };
            Offered {
            offer: *o,
            says: match o {
                Offer::TestServers => TEST_SERVERS_SAYS.into(),
            },
            then: match o {
                Offer::TestServers => TEST_SERVERS_THEN.into(),
            },
            marked,
            marked_for,
        }
        })
        .collect()
}

/// The words of the box that lets a new project's tests start their own test servers (the tech-lead's T5, 23/11).
pub const TEST_SERVERS_SAYS: &str = "Let this project's tests start their own test servers";
/// What it does, under the box: as `jaynes project test-servers` says it, and what stays held.
pub const TEST_SERVERS_THEN: &str = "As when you run the tests yourself, but they still cannot reach your services or the Jaynes page. Jaynes still holds the ports of the usual databases and caches.";

/// An older engine a run started on, gone from the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gone {
    /// Its version, the number alone: "0.2.3".
    pub version: String,
    /// What the engine says when an answer goes to the run.
    pub said: String,
    /// Where what it made is, when it made something.
    pub place: Option<crate::listing::Place>,
    /// When the person moved it to Finished, if they did.
    pub moved: Option<u64>,
}

/// Whether an open run can no longer go on with `engine` here: another engine started it, and that engine's program is
/// not kept on this machine. `engine` is a whole engine ("0.2.5+9f3c…"), or a version alone ("0.2.5", all `jaynes
/// update` knows of the release it brings), compared then by version. One criterion for the card, the listing and the
/// update's warning.
pub fn left_behind(home: &Path, run: &Run, engine: &str) -> bool {
    let started = run.engine.split('+').next().unwrap_or(&run.engine);
    let same = if engine.contains('+') { run.engine == engine } else { started == engine };
    !run.finished() && !same && crate::drive::engine_exe(home, &run.engine).is_none()
}

/// The engine a run started on, when it is older and gone from the machine; kept, it takes the run's answers.
pub fn gone(home: &Path, run: &Run) -> Option<Gone> {
    left_behind(home, run, jaynes_core::ENGINE).then(|| Gone {
        version: run.engine.split('+').next().unwrap_or(&run.engine).to_string(),
        said: crate::drive::gone_said(run),
        place: crate::listing::run_place(home, run),
        moved: crate::moved::when(&crate::moved::read(home).unwrap_or_default(), &run.id),
    })
}

/// What an agent asked leave to write, kept beside the run as it asked (`runs/<run>/wants/<question>.json`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Wants {
    /// The file, relative to the project when it is inside it.
    pub file: String,
    /// The file as it is in the step's copy, none when it does not exist yet.
    pub before: Option<String>,
    /// The file as the agent would leave it.
    pub after: String,
}

/// Why a leave comes to the person: the agent asked past what its step lets it do.
pub const WHY_YOU: &str = "Why you: the agent asked for something its step's settings do not allow.";

impl Stop {
    /// A verdict's name on its button: the role's word for it at this stop, or Jaynes's name ([`label_for`]).
    pub fn label(&self, v: VerdictKind) -> &str {
        self.answers.get(&v).map(String::as_str).unwrap_or_else(|| label_for(v, self.kind))
    }

    /// What answering `v` does, in the sentence beside its button, or nothing where Jaynes says none there.
    pub fn says(&self, v: VerdictKind) -> &str {
        self.consequences.get(&v).map(String::as_str).unwrap_or("")
    }
}

/// What allowing and refusing a leave do, from what it asks (`change tests/conftest.py`): a file that runs later
/// outside any box, in a run that merges, is asked about again before the merge (G19).
pub fn leave_consequences(run: &Run, about: &str) -> BTreeMap<VerdictKind, String> {
    let (verb, target) = about.split_once(' ').unwrap_or((about, ""));
    let (allow, refuse) = match verb {
        "change" => {
            let again = run.manifest.lands() && !jaynes_core::risk::floor([target]).is_empty();
            let more = if again { " Before it lands in your branch, the merge asks you again." } else { "" };
            (format!("It writes the file and goes on.{more}"), "It goes on without writing it.".to_string())
        }
        "read" => ("It reads it and goes on.".into(), "It goes on without reading it.".into()),
        "reach" => ("It reaches it and goes on.".into(), "It goes on without reaching it.".into()),
        _ => ("It runs it and goes on.".into(), "It goes on without running it.".into()),
    };
    [(VerdictKind::Keep, allow), (VerdictKind::Drop, refuse)].into_iter().collect()
}

/// What the agent wants to write, line by line from `before` to `after`, each with its mark: '+' comes, '-' goes, ' '
/// stays, and '⋯' (no text) marks lines that stay left out. A new file is all added; an edit is its changes among
/// three lines of what stays around each. The page and the app draw the same lines.
pub fn wants_lines(before: Option<&str>, after: &str) -> Vec<(char, String)> {
    let Some(before) = before else { return after.lines().map(|l| ('+', l.to_string())).collect() };
    let (a, b): (Vec<&str>, Vec<&str>) = (before.lines().collect(), after.lines().collect());
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let mut ops: Vec<(char, &str)> = a[..pre].iter().map(|l| (' ', *l)).collect();
    if am.len().saturating_mul(bm.len()) <= 1_000_000 {
        // the longest run of lines both keep, read from the end so the walk goes forward
        let w = bm.len() + 1;
        let mut keep = vec![0u32; (am.len() + 1) * w];
        for i in (0..am.len()).rev() {
            for j in (0..bm.len()).rev() {
                keep[i * w + j] = if am[i] == bm[j] { keep[(i + 1) * w + j + 1] + 1 } else { keep[(i + 1) * w + j].max(keep[i * w + j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < am.len() || j < bm.len() {
            if i < am.len() && j < bm.len() && am[i] == bm[j] {
                ops.push((' ', am[i]));
                (i, j) = (i + 1, j + 1);
            } else if j == bm.len() || (i < am.len() && keep[(i + 1) * w + j] >= keep[i * w + j + 1]) {
                ops.push(('-', am[i]));
                i += 1;
            } else {
                ops.push(('+', bm[j]));
                j += 1;
            }
        }
    } else {
        // too long to measure: what goes, then what comes
        ops.extend(am.iter().map(|l| ('-', *l)));
        ops.extend(bm.iter().map(|l| ('+', *l)));
    }
    ops.extend(a[a.len() - suf..].iter().map(|l| (' ', *l)));
    let changed: Vec<usize> = ops.iter().enumerate().filter(|(_, (k, _))| *k != ' ').map(|(n, _)| n).collect();
    let near = |n: usize| changed.iter().any(|&c| c.abs_diff(n) <= 3);
    let mut out: Vec<(char, String)> = Vec::new();
    let mut gap = false;
    for (n, (k, l)) in ops.iter().enumerate() {
        if *k == ' ' && !near(n) {
            gap = true;
            continue;
        }
        if gap && !out.is_empty() {
            out.push(('⋯', String::new()));
        }
        gap = false;
        out.push((*k, l.to_string()));
    }
    out
}

/// A stop the run went on without: it did not hold the run, and the run is now at a later step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Passed {
    /// The step the run is at now, and whether it is blocked there.
    pub now: String,
    pub blocked: bool,
    /// "The run went on without your answer. It is now at “fix”, blocked."
    pub went_on: String,
    /// What each answer still does: "“Redo” would do “found” and “fix” again. “Keep” changes nothing."
    pub still: String,
}

/// A file the merge lands on the person's branch, as the stop before it shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Lands {
    pub path: String,
    pub added: u64,
    pub removed: u64,
    /// How it runs later outside any box, in plain words, when it does (G19: the merge names it with this).
    pub runs: Option<&'static str>,
}

/// The stop a run waits at: its first open question the run has not gone on without.
pub fn waiting(home: &Path, run: &Run) -> Option<Stop> {
    if run.finished() {
        return None;
    }
    let q = run.questions.values().find(|q| q.open() && went_on(run, q).is_none())?;
    Some(stop_of(home, run, q))
}

/// The stops the run went on without, oldest first: each answered from a line under what holds the run now.
pub fn passed(home: &Path, run: &Run) -> Vec<Stop> {
    run.questions.values().filter(|q| went_on(run, q).is_some()).map(|q| stop_of(home, run, q)).collect()
}

/// Where the run is now, and whether it is blocked there, when it went on without the person's answer to `q`: an
/// open question that does not hold the run, at a step before the one the run is at.
pub fn went_on(run: &Run, q: &jaynes_core::run::QuestionState) -> Option<(String, bool)> {
    if run.finished() || !q.open() || q.question.holds {
        return None;
    }
    let now = match &run.phase {
        jaynes_core::run::Phase::Blocked { step: Some(s), .. } | jaynes_core::run::Phase::Waiting { step: Some(s), .. } => s.clone(),
        _ => crate::listing::step_at(run),
    };
    let path = run.path();
    let at = |name: &str| run.manifest.index(name).and_then(|i| path.iter().position(|&j| j == i));
    match (at(&q.question.step), at(&now)) {
        (Some(asked), Some(here)) if here > asked => Some((now, matches!(run.phase, jaynes_core::run::Phase::Blocked { .. }))),
        _ => None,
    }
}

/// The role's words for the verdicts of a question: its stop's answers at a decision, none at a question of Jaynes's own.
fn role_words(run: &Run, q: &jaynes_core::question::Question) -> BTreeMap<VerdictKind, String> {
    match q.kind {
        QuestionKind::Decision => run.manifest.index(&q.step).and_then(|i| run.manifest.steps[i].stop.as_ref()).map(|s| s.answers.clone()).unwrap_or_default(),
        _ => BTreeMap::new(),
    }
}

/// The role's line under a question: its stop's note at a decision, none at a question of Jaynes's own.
pub fn role_note(run: &Run, q: &jaynes_core::question::Question) -> String {
    match q.kind {
        QuestionKind::Decision => run.manifest.index(&q.step).and_then(|i| run.manifest.steps[i].stop.as_ref()).map(|s| s.note.clone()).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Whether the request settled this stop for the person: the role says what the request settles there (its stop's
/// `settled`), or a merge's touch named it. Any other stop the run went on without is one the person did not answer.
pub fn settled_by_request(run: &Run, q: &jaynes_core::run::QuestionState) -> bool {
    run.manifest.index(&q.question.step).and_then(|i| run.manifest.steps[i].stop.as_ref()).is_some_and(|s| s.settled.is_some())
        || run.questions.values().any(|m| m.question.rides.iter().any(|x| x.question == q.question.id))
}

/// The answer a stop the request settled offers in place of what the request said, on the button that opens it from
/// the run's story: "Change" at a choice, otherwise the first verdict other than keep in the role's words ("Not this
/// bug"), or Jaynes's name for it.
pub fn other_answer(run: &Run, q: &jaynes_core::question::Question) -> String {
    if q.choose {
        return label_for(VerdictKind::Change, q.kind).to_string();
    }
    let words = role_words(run, q);
    let v = q.offered.iter().copied().find(|v| *v != VerdictKind::Keep).unwrap_or(VerdictKind::Redo);
    words.get(&v).cloned().unwrap_or_else(|| label_for(v, q.kind).to_string())
}

/// What each answer the stop offers still makes run again, as the core says it would (jaynes_core::effects).
pub fn still_redoes(run: &Run, q: &jaynes_core::question::Question) -> String {
    let every = |v: VerdictKind| -> Answer {
        let verdict = match v {
            VerdictKind::Keep => Verdict::Keep,
            VerdictKind::Skip => Verdict::Skip,
            VerdictKind::Drop => Verdict::Drop,
            VerdictKind::Change => Verdict::Change { text: String::new() },
            VerdictKind::Redo => Verdict::Redo { note: String::new() },
        };
        Answer {
            question: q.id.clone(),
            shown: q.shown(),
            verdicts: q.items.iter().map(|i| (i.id.clone(), verdict.clone())).collect(),
            by: By::Person,
            context: String::new(),
            back: BTreeMap::new(),
        }
    };
    // each answer by the word its button says: the role's at a stop of its own, Jaynes's otherwise (Stop::label)
    let words = role_words(run, q);
    let label = |v: VerdictKind| words.get(&v).cloned().unwrap_or_else(|| label_for(v, q.kind).to_string());
    let said: Vec<(String, Vec<String>)> = q.offered.iter().map(|&v| (label(v), jaynes_core::effects(run, q, &every(v)).reruns)).collect();
    still_sentence(&said)
}

/// What each answer, by its label, still makes run again, in a line: "“Redo” would do “found” and “fix” again.
/// “Keep” changes nothing.", "Either answer does “fix” again.", or "Your answer goes to the record; nothing runs again."
pub fn still_sentence(said: &[(String, Vec<String>)]) -> String {
    let and = |words: Vec<String>| match words.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, first)) => format!("{} and {last}", first.join(", ")),
    };
    let quoted = |v: &[String]| and(v.iter().map(|x| format!("“{x}”")).collect());
    let (redo, none): (Vec<_>, Vec<_>) = said.iter().partition(|(_, r)| !r.is_empty());
    if redo.is_empty() {
        return "Your answer goes to the record; nothing runs again.".into();
    }
    if none.is_empty() && redo.iter().all(|(_, r)| *r == redo[0].1) {
        return format!("{} answer does {} again.", if redo.len() == 2 { "Either" } else { "Every" }, quoted(&redo[0].1));
    }
    let mut line: Vec<String> = redo.iter().map(|(l, r)| format!("“{l}” would do {} again.", quoted(r))).collect();
    if !none.is_empty() {
        let labels: Vec<String> = none.iter().map(|(l, _)| l.clone()).collect();
        line.push(format!("{} {}.", quoted(&labels), if none.len() == 1 { "changes nothing" } else { "change nothing" }));
    }
    line.join(" ")
}

fn stop_of(home: &Path, run: &Run, q: &jaynes_core::run::QuestionState) -> Stop {
    let store = crate::store::Store::open(&home.join("record.sqlite3")).ok();
    let shown = |it: &jaynes_core::question::Item| {
        let d = store.as_ref().map(|s| s.display(&it.digest)).unwrap_or_default();
        let aspects = d["aspects"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|a| (a["label"].as_str().unwrap_or("").to_string(), a["text"].as_str().unwrap_or("").to_string()))
            .collect();
        let names: Vec<String> = d["pictures"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p.as_str())
            .filter(|name| !name.contains('/') && !name.starts_with('.'))
            .map(str::to_string)
            .collect();
        let page = names.iter().find_map(|n| d["pages"][n.as_str()].as_str()).filter(|p| kept_page(p)).map(str::to_string);
        let pictures = names.iter().map(|name| home.join("pictures").join(name)).filter(|p| p.exists()).collect();
        Item { id: it.id.clone(), label: it.label.clone(), detail: d["detail"].as_str().unwrap_or("").to_string(), aspects, pictures, page }
    };
    let items = q.question.items.iter().map(&shown).collect();
    let back = q.question.back.iter().map(&shown).collect();
    let about = run.manifest.index(&q.question.step).map(|i| run.manifest.steps[i].about.clone()).unwrap_or_default();
    let merge = q.question.kind == QuestionKind::Merge;
    let into = if merge { crate::project::start_branch_of(&home.join("runs").join(&run.id).join("project")) } else { String::new() };
    let name = crate::listing::short(&run.request.split_whitespace().collect::<Vec<_>>().join(" "));
    let (ahead, touch) = match q.question.kind {
        QuestionKind::Permission => (None, None),
        QuestionKind::Merge => (None, Some(format!("merge “{name}” into {}", if into.is_empty() { "your branch" } else { &into }))),
        _ => match ahead_of(run, &q.question.step) {
            Some((a, true)) => (Some(a.clone()), Some(format!("send your answer to Jaynes: {a}"))),
            Some((a, false)) => (Some(a), None),
            None => (None, None),
        },
    };
    let lands = if merge {
        run.delivered_files()
            .into_iter()
            .filter(|(c, _)| q.question.items.iter().any(|x| x.id == c.what))
            .map(|(c, _)| Lands {
                added: c.size - c.removed.min(c.size),
                removed: c.removed.min(c.size),
                runs: jaynes_core::risk::holds_merge(run.manifest.target, &c).then(|| jaynes_core::risk::runs_how(&c.what)),
                path: c.what,
            })
            .collect()
    } else {
        Vec::new()
    };
    let passed = went_on(run, q).map(|(now, blocked)| Passed {
        went_on: format!("The run went on without your answer. It is now at “{now}”{}.", if blocked { ", blocked" } else { "" }),
        still: still_redoes(run, &q.question),
        now,
        blocked,
    });
    let answers = role_words(run, &q.question);
    let note = role_note(run, &q.question);
    let permission = q.question.kind == QuestionKind::Permission;
    let wants = permission.then(|| crate::permit::wants_kept(home, &run.id, &q.question.id)).flatten();
    Stop {
        run: run.id.clone(),
        question: q.question.id.clone(),
        shown: q.question.shown().0,
        title: crate::listing::title_into(q, &into),
        asks: if merge { format!("“{name}” is done and checked. This is what lands on your branch.") } else { q.question.about.clone() },
        step: q.question.step.clone(),
        permission,
        kind: q.question.kind,
        choose: q.question.choose,
        offered: q.question.offered.clone(),
        items,
        back,
        round: q.question.pass,
        about,
        ahead,
        touch,
        into,
        lands,
        passed,
        answers,
        note,
        why: permission.then(|| WHY_YOU.to_string()),
        consequences: if permission { leave_consequences(run, &q.question.about) } else { BTreeMap::new() },
        wants,
        gone: gone(home, run),
        offers: offered(store.as_ref(), run, q),
    }
}

/// A page the role kept, named by its fingerprint: nothing else is served.
fn kept_page(name: &str) -> bool {
    // by bytes: the name comes from what a role kept, and need not cut at a letter's edge
    let b = name.as_bytes();
    b.len() == 69 && b[..64].iter().all(u8::is_ascii_hexdigit) && &b[64..] == b".html"
}

/// The answer the person's marks make: a verdict per item, with their words for a change or a redo.
pub fn answer(stop: &Stop, marks: &BTreeMap<String, (VerdictKind, String)>, context: &str) -> Answer {
    let verdicts = marks
        .iter()
        .map(|(id, (kind, words))| {
            let v = match kind {
                VerdictKind::Keep => Verdict::Keep,
                VerdictKind::Skip => Verdict::Skip,
                VerdictKind::Drop => Verdict::Drop,
                VerdictKind::Change => Verdict::Change { text: words.clone() },
                VerdictKind::Redo => Verdict::Redo { note: words.clone() },
            };
            (id.clone(), v)
        })
        .collect();
    Answer {
        question: stop.question.clone(),
        shown: jaynes_core::Digest(stop.shown.clone()),
        verdicts,
        // who answered is the engine's to say, once the person's key has signed it
        by: By::Agent { who: "the Jaynes app, unsigned".into() },
        context: context.to_string(),
        back: BTreeMap::new(),
    }
}

/// What lies ahead of `step`: a step whose effect cannot be undone, or a merge into the person's branch with no
/// other stop of theirs before it.
pub fn ahead(run: &Run, step: &str) -> Option<String> {
    ahead_of(run, step).map(|(a, _)| a)
}

/// What lies ahead of `step`, and whether an answer that leads there asks for the person's touch: a step that cannot
/// be undone does; the merge does not, since it asks the person itself before it lands, with their touch once.
fn ahead_of(run: &Run, step: &str) -> Option<(String, bool)> {
    let from = run.manifest.index(step)?;
    let path = run.path();
    let later: Vec<usize> = path.into_iter().filter(|&j| j > from).collect();
    if let Some(&j) = later.iter().find(|&&j| run.manifest.steps[j].effect.irreversible()) {
        return Some((format!("“{}” cannot be undone", run.manifest.steps[j].name), true));
    }
    let another_stop = later.iter().any(|&j| run.manifest.steps[j].stop.is_some() || run.manifest.steps[j].executor == jaynes_core::manifest::Executor::Person);
    (run.manifest.lands() && !another_stop && !run.never_merge).then(|| ("the run merges into your branch".to_string(), false))
}

/// How a verdict is offered, in a word.
pub fn label(v: VerdictKind, permission: bool) -> &'static str {
    label_for(v, if permission { QuestionKind::Permission } else { QuestionKind::Decision })
}

/// A verdict's name as the person reads it on a question of `kind`: a leave is allowed or refused, and a merge that
/// brings in what runs later (G19) is merged, or the change is taken as a branch only.
pub fn label_for(v: VerdictKind, kind: QuestionKind) -> &'static str {
    match (v, kind) {
        (VerdictKind::Keep, QuestionKind::Permission) => "Allow",
        (VerdictKind::Drop, QuestionKind::Permission) => "Refuse",
        (VerdictKind::Keep, QuestionKind::Merge) => "Merge",
        (VerdictKind::Drop, QuestionKind::Merge) => "Branch only",
        (VerdictKind::Keep, _) => "Keep",
        (VerdictKind::Change, _) => "Change",
        (VerdictKind::Redo, _) => "Redo",
        (VerdictKind::Drop, _) => "Drop",
        (VerdictKind::Skip, _) => "I don't know",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_merge_s_verdicts_say_merge_or_a_branch_only_and_a_leave_s_allow_or_refuse() {
        assert_eq!((label_for(VerdictKind::Keep, QuestionKind::Merge), label_for(VerdictKind::Drop, QuestionKind::Merge)), ("Merge", "Branch only"));
        assert_eq!((label(VerdictKind::Keep, true), label(VerdictKind::Drop, true)), ("Allow", "Refuse"));
        assert_eq!((label(VerdictKind::Keep, false), label_for(VerdictKind::Drop, QuestionKind::Decision)), ("Keep", "Drop"));
    }
    use jaynes_core::manifest::{DeliveryKind, Effect, Executor, Manifest, Reach, SessionKind, StepDecl};
    use jaynes_core::question::{Item as QItem, Question};
    use jaynes_core::run::{Phase, QuestionState};
    use jaynes_core::Input;

    fn run(steps: &[(&str, bool)], delivery: DeliveryKind) -> Run {
        let decl = |name: &str, irreversible: bool| StepDecl {
            name: name.into(), needs: vec![], executor: if irreversible { Executor::Role } else { Executor::Agent }, session: SessionKind::Fresh, reach: Reach::default(),
            effect: if irreversible { Effect::Irreversible { idempotent: false } } else { Default::default() },
            checks: vec![], stop: None, from_risk: Default::default(), attempts: 3, fixes: 2, hidden: false, idempotent: false, about: if name == "plan" { "Plans the work.".into() } else { String::new() },
            model: Default::default(),
            turns: None, delivered: true,
        };
        let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: steps.iter().map(|(n, i)| decl(n, *i)).collect(), target: Default::default(), delivery };
        let start = Input::Start { run: "r1".into(), request: "x".into(), manifest, role_fingerprint: jaynes_core::Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        Run::fold(&jaynes_core::decide(None, &start).expect("start").events).expect("run")
    }

    fn asking(mut r: Run) -> Run {
        let item = |id: &str, label: &str| QItem { named: None, id: id.into(), label: label.into(), digest: jaynes_core::Digest(format!("d-{id}")) };
        let question = Question { rides: Vec::new(),
            id: "q1".into(), step: "plan".into(), round: 1, kind: QuestionKind::Decision, items: vec![item("a", "Plan A"), item("b", "Plan B")],
            offered: vec![VerdictKind::Keep, VerdictKind::Redo, VerdictKind::Drop], holds: true, about: "Which plan goes on?".into(),
            choose: false, pass: 2, passes: 0, back: vec![item("c", "Plan C")],
        };
        r.questions.insert("q1".into(), QuestionState { question, answer: None, effects: None, superseded: false });
        r.phase = Phase::Waiting { step: Some("plan".into()), on: vec!["q1".into()], why: "waits".into() };
        r
    }

    // Alan, 21/11: "entender onde o python entra antes de decidir"
    #[test]
    fn a_stop_with_one_option_asks_whether_to_go_with_it_and_keeps_the_role_s_question_under() {
        let home = std::env::temp_dir().join(format!("jaynes-stop-one-{}", std::process::id()));
        let mut r = asking(run(&[("plan", false), ("change", false)], DeliveryKind::Branch));
        let q = &mut r.questions.get_mut("q1").expect("asked").question;
        q.choose = true;
        q.items.truncate(1);
        q.items[0].label = "Python + FastAPI: small and known".into();
        let stop = waiting(&home, &r).expect("a stop");
        assert_eq!((stop.title.as_str(), stop.asks.as_str()), ("Go with “Python + FastAPI”?", "Which plan goes on?"));
    }

    // the engineer lead, 22/11 (row 9): at a stop of a run an older engine started, the app heard the refusal only after
    // Send; the stop says before it that the run's engine left the Mac, when it did
    #[test]
    fn asked_a_stop_of_a_run_whose_engine_left_the_mac_says_so_before_send() {
        let home = std::env::temp_dir().join(format!("jaynes-stop-gone-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(&home).expect("home");
        let mut r = asking(run(&[("plan", false), ("change", false)], DeliveryKind::Branch));
        assert_eq!(waiting(&home, &r).expect("a stop").gone, None, "a run of this engine goes on");
        r.engine = "0.2.3+abc123".into();
        let gone = waiting(&home, &r).expect("a stop").gone.expect("its engine left");
        assert_eq!(gone.version, "0.2.3", "the number alone, without what follows the +");
        assert_eq!(gone.said, crate::drive::gone_said(&r), "the engine's own sentence, as Send would be refused with");
        // one criterion for the card, the listing and `jaynes update`'s warning (the engineer lead, 22/11): the update
        // knows the coming release by its version alone, and compares by it
        assert!(left_behind(&home, &r, jaynes_core::ENGINE));
        assert!(left_behind(&home, &r, "0.2.5") && !left_behind(&home, &r, "0.2.3"), "by version, when that is all there is");
        assert!(!left_behind(&home, &r, "0.2.3+abc123"), "its own engine carries it on");
        assert!(gone.said.contains("0.2.3+abc123") && gone.said.contains("no longer on this machine"), "{}", gone.said);
        // kept on the machine, the older engine takes the answer: nothing to say
        let kept = home.join("engines").join("0.2.3+abc123");
        std::fs::create_dir_all(&kept).expect("dir");
        std::fs::write(kept.join("jaynes"), "").expect("program");
        assert_eq!(waiting(&home, &r).expect("a stop").gone, None, "its engine is here");
        assert!(!left_behind(&home, &r, "0.2.5"), "kept, the older engine carries it on after an update too");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_waiting_run_is_a_stop_and_the_marks_make_its_answer() {
        let home = std::env::temp_dir().join(format!("jaynes-stop-{}", std::process::id()));
        let r = asking(run(&[("plan", false), ("change", false)], DeliveryKind::Branch));
        let stop = waiting(&home, &r).expect("a stop");
        assert_eq!((stop.question.as_str(), stop.title.as_str(), stop.step.as_str()), ("q1", "Which plan goes on?", "plan"));
        assert_eq!(stop.asks, "Which plan goes on?");
        assert_eq!(stop.shown, r.questions["q1"].question.shown().0);
        assert_eq!(stop.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["Plan A", "Plan B"]);
        assert_eq!(stop.offered, [VerdictKind::Keep, VerdictKind::Redo, VerdictKind::Drop]);
        assert_eq!(stop.ahead, None);
        assert_eq!(stop.back.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["Plan C"]);
        assert_eq!((stop.round, stop.about.as_str()), (2, "Plans the work."));
        let mut marks = BTreeMap::new();
        marks.insert("a".to_string(), (VerdictKind::Keep, String::new()));
        marks.insert("b".to_string(), (VerdictKind::Redo, "it misses the tests".to_string()));
        let a = answer(&stop, &marks, "go on");
        assert_eq!((a.question.as_str(), a.shown.0.as_str(), a.context.as_str()), ("q1", stop.shown.as_str(), "go on"));
        assert_eq!(a.verdicts["a"], Verdict::Keep);
        assert_eq!(a.verdicts["b"], Verdict::Redo { note: "it misses the tests".into() });
        assert!(!a.by.person(), "the engine sets who answered, never the app");
    }

    /// A run of reproduce, found and fix, blocked at fix, with reproduce's stop still open: a stop that does not hold
    /// the run, which went on without the answer.
    fn went_on_from_reproduce(holds: bool) -> Run {
        let mut r = run(&[("reproduce", false), ("found", false), ("fix", false)], DeliveryKind::Branch);
        let item = QItem { named: None, id: "examples".into(), label: "The examples".into(), digest: jaynes_core::Digest("d-examples".into()) };
        let question = Question { rides: Vec::new(),
            id: "q1".into(), step: "reproduce".into(), round: 1, kind: QuestionKind::Decision, items: vec![item],
            offered: vec![VerdictKind::Keep, VerdictKind::Redo], holds, about: "Is this the bug?".into(),
            choose: false, pass: 1, passes: 0, back: vec![],
        };
        r.questions.insert("q1".into(), QuestionState { question, answer: None, effects: None, superseded: false });
        r.phase = Phase::Blocked { step: Some("fix".into()), fault: jaynes_core::event::Fault::Agent, reason: "the change could not be checked".into() };
        r
    }

    // The tech-lead's rule, 21/11, from Alan's run r1795301086-403, where reproduce's stop (which does not hold the
    // run) stayed drawn over the card of fix, blocked, and he could not tell which one counted: "uma parada de um passo
    // que a execução já passou nunca fica por cima do estado de agora". The design-lead's parada-passada.md.
    #[test]
    fn asked_a_stop_the_run_passed_never_covers_now() {
        let home = std::env::temp_dir().join(format!("jaynes-stop-passed-{}", std::process::id()));
        let r = went_on_from_reproduce(false);
        assert_eq!(waiting(&home, &r), None, "the stop the run passed is not the one it waits at");
        let passed = passed(&home, &r);
        assert_eq!(passed.iter().map(|s| (s.step.as_str(), s.title.as_str())).collect::<Vec<_>>(), [("reproduce", "Is this the bug?")]);
        let p = passed[0].passed.clone().expect("it says the run went on");
        assert_eq!((p.now.as_str(), p.blocked), ("fix", true));
        assert_eq!(p.went_on, "The run went on without your answer. It is now at “fix”, blocked.");
        assert_eq!(p.still, "“Redo” would do “reproduce” again. “Keep” changes nothing.");
        // the same stop, holding the run, is the one it waits at
        let held = went_on_from_reproduce(true);
        assert_eq!(waiting(&home, &held).map(|s| (s.step, s.passed)), Some(("reproduce".to_string(), None)));
        assert!(super::passed(&home, &held).is_empty());
    }

    // A passed stop says what an answer still does with the words its buttons say: the role's, where it names them
    // (fix-bug's "Yes" and "No, it's something else"), never Jaynes's "Redo" beside a button that says otherwise
    #[test]
    fn a_passed_stop_says_what_an_answer_still_does_in_the_role_s_words() {
        let home = std::env::temp_dir().join(format!("jaynes-stop-passed-words-{}", std::process::id()));
        let mut r = went_on_from_reproduce(false);
        let answers = BTreeMap::from([(VerdictKind::Keep, "Yes".to_string()), (VerdictKind::Redo, "No, it's something else".to_string())]);
        r.manifest.steps[0].stop = Some(jaynes_core::manifest::StopDecl { settled: None, holds_unless: None, vouched_by: None,
            verdicts: vec![VerdictKind::Keep, VerdictKind::Redo], holds: false, from_risk: Default::default(), release: false, budget: None,
            audit_rate: jaynes_core::manifest::Rate(0.1), about: "Is this the bug?".into(), choose: false, redo: None, rounds: None, answers, note: String::new(),
            offers: Default::default(),
        });
        let p = passed(&home, &r)[0].passed.clone().expect("it says the run went on");
        assert_eq!(p.still, "“No, it's something else” would do “reproduce” again. “Yes” changes nothing.");
    }

    #[test]
    fn what_an_answer_to_a_passed_stop_still_does_is_said_in_one_line() {
        let s = |v: &[(&str, &[&str])]| still_sentence(&v.iter().map(|(l, r)| (l.to_string(), r.iter().map(|x| x.to_string()).collect())).collect::<Vec<_>>());
        assert_eq!(s(&[("Yes", &[]), ("No, it's something else", &["found", "fix"])]), "“No, it's something else” would do “found” and “fix” again. “Yes” changes nothing.");
        assert_eq!(s(&[("Keep", &["fix"]), ("Redo", &["fix"])]), "Either answer does “fix” again.");
        assert_eq!(s(&[("Keep", &[]), ("Drop", &[])]), "Your answer goes to the record; nothing runs again.");
        assert_eq!(s(&[("Keep", &[]), ("Redo", &["a", "b", "c"]), ("Drop", &[])]), "“Redo” would do “a”, “b” and “c” again. “Keep” and “Drop” change nothing.");
    }

    /// A run of one agent step that may write the whole project, delivered by merge, waiting at the merge after it
    /// changed `changed`, in a home whose copy merges into `main`.
    fn at_the_merge(home: &Path, changed: Vec<jaynes_core::risk::Changed>) -> Run {
        let mut build = StepDecl {
            name: "build".into(), needs: vec![], executor: Executor::Agent, session: SessionKind::Fresh, reach: Reach::default(), effect: Default::default(),
            checks: vec![], stop: None, from_risk: Default::default(), attempts: 3, fixes: 2, hidden: false, idempotent: false, about: String::new(),
            model: Default::default(),
            turns: None, delivered: true,
        };
        build.reach = Reach { project: jaynes_core::manifest::ProjectAccess::Write, write: vec!["**".into()], ..Reach::default() };
        let manifest = Manifest { role: "build".into(), version: "1".into(), protocol: 1, steps: vec![build], target: Default::default(), delivery: DeliveryKind::Merge };
        let start = Input::Start { run: "r1".into(), request: "Scaffold the\nengram service".into(), manifest, role_fingerprint: jaynes_core::Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let feed = |r: &mut Option<Run>, input: Input| -> Vec<jaynes_core::Command> {
            let out = jaynes_core::decide(r.as_ref(), &input).expect("taken");
            match r {
                Some(run) => out.events.iter().for_each(|e| run.apply(e)),
                None => *r = Some(Run::fold(&out.events).expect("run")),
            }
            out.commands
        };
        let mut r = None;
        let c = feed(&mut r, start);
        let mut turn = Vec::new();
        for cmd in c {
            if let jaynes_core::Command::Probe { step, .. } = cmd {
                turn.extend(feed(&mut r, Input::ProbeEnded { step, ok: true, reason: String::new() }));
            }
        }
        let attempt = turn.iter().find_map(|c| if let jaynes_core::Command::RunAgent(t) = c { Some(t.attempt) } else { None }).expect("a turn");
        let outcome = jaynes_core::AttemptOutcome::Delivered { output: jaynes_core::Digest("o".into()), items: vec![], changed, usage: Default::default(), session: None, tree: None, reply: None };
        feed(&mut r, Input::AttemptEnded { step: "build".into(), attempt, fix: 0, outcome });
        std::fs::create_dir_all(home.join("runs/r1/project.git")).expect("copy");
        std::fs::write(home.join("runs/r1/project.git/jaynes-start"), "main\n").expect("start");
        r.expect("run")
    }

    fn file(path: &str, added: u64, removed: u64) -> jaynes_core::risk::Changed {
        let mut c = jaynes_core::risk::Changed::file(path.into(), added + removed);
        c.removed = removed;
        c.digest = Some(format!("d-{path}"));
        c
    }

    // Alan, 21/11: "tive que dar uma porrada de allow com o meu touchid"
    #[test]
    fn a_leave_asks_no_touch_and_the_merge_asks_it_once_showing_what_lands() {
        let home = std::env::temp_dir().join(format!("jaynes-stop-touch-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&home);
        // a leave: no "Ahead", no touch, though the run merges later
        let mut r = asking(run(&[("plan", false), ("change", false)], DeliveryKind::Merge));
        r.questions.get_mut("q1").expect("asked").question.kind = QuestionKind::Permission;
        let stop = waiting(&home, &r).expect("a stop");
        assert_eq!((stop.ahead.as_deref(), stop.touch.as_deref()), (None, None), "a leave");
        // a decision before the merge: the merge is ahead, and asks itself, so no touch here
        let r = asking(run(&[("plan", false), ("change", false)], DeliveryKind::Merge));
        let stop = waiting(&home, &r).expect("a stop");
        assert_eq!((stop.ahead.as_deref(), stop.touch.as_deref()), (Some("the run merges into your branch"), None));
        // a step that cannot be undone keeps its touch
        let r = asking(run(&[("plan", false), ("send", true)], DeliveryKind::Branch));
        let stop = waiting(&home, &r).expect("a stop");
        assert_eq!((stop.ahead.as_deref(), stop.touch.as_deref()), (Some("“send” cannot be undone"), Some("send your answer to Jaynes: “send” cannot be undone")));
        // the merge: every file that lands with its lines, what runs later said in plain words, the touch once
        let r = at_the_merge(&home, vec![file("src/app.py", 64, 12), file("tests/conftest.py", 14, 0), file("Makefile", 3, 1)]);
        let stop = waiting(&home, &r).expect("the merge waits");
        assert_eq!(stop.kind, QuestionKind::Merge);
        assert_eq!(stop.title, "Merge into “main”?");
        assert_eq!(stop.asks, "“Scaffold the engram service” is done and checked. This is what lands on your branch.");
        assert_eq!((stop.ahead.as_deref(), stop.touch.as_deref()), (None, Some("merge “Scaffold the engram service” into main")));
        assert_eq!(stop.into, "main");
        assert_eq!(
            stop.lands,
            [
                Lands { path: "src/app.py".into(), added: 64, removed: 12, runs: None },
                Lands { path: "tests/conftest.py".into(), added: 14, removed: 0, runs: Some("runs on its own whenever the tests run") },
                Lands { path: "Makefile".into(), added: 3, removed: 1, runs: Some("runs when you run make") },
            ]
        );
        assert_eq!(stop.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["src/app.py", "tests/conftest.py", "Makefile"]);
        // one answer for every file, and what it does names the branch and only what runs later
        let marks: BTreeMap<String, (VerdictKind, String)> = stop.items.iter().map(|i| (i.id.clone(), (VerdictKind::Keep, String::new()))).collect();
        let q = &r.questions[&stop.question].question;
        let fx = crate::drive::effects(&home, &r, q, &answer(&stop, &marks, "")).expect("effects");
        assert_eq!(fx.sentences, ["The delivery merges into “main”, with tests/conftest.py and Makefile."]);
        let marks: BTreeMap<String, (VerdictKind, String)> = stop.items.iter().map(|i| (i.id.clone(), (VerdictKind::Drop, String::new()))).collect();
        let fx = crate::drive::effects(&home, &r, q, &answer(&stop, &marks, "")).expect("effects");
        assert_eq!(fx.sentences, ["Nothing lands on your branch: the change stays on the branch jaynes/r1, where you can look at it."]);
        // nothing that runs later: the same stop, nothing said of any file, the branch named
        let r = at_the_merge(&home, vec![file("src/app.py", 1, 0)]);
        let stop = waiting(&home, &r).expect("the merge waits");
        assert_eq!((stop.title.as_str(), stop.lands.len(), stop.lands[0].runs), ("Merge into “main”?", 1, None));
        let marks: BTreeMap<String, (VerdictKind, String)> = stop.items.iter().map(|i| (i.id.clone(), (VerdictKind::Keep, String::new()))).collect();
        let fx = crate::drive::effects(&home, &r, &r.questions[&stop.question].question, &answer(&stop, &marks, "")).expect("effects");
        assert_eq!(fx.sentences, ["The delivery merges into “main”."]);
        let _gone = std::fs::remove_dir_all(&home);
    }

    // Alan, 21/11: "olha essa bosta, acho que a role tá mal escrita"
    #[test]
    fn a_stop_names_a_verdict_in_the_role_s_words_and_in_jaynes_s_otherwise() {
        let home = std::env::temp_dir().join(format!("jaynes-stop-answers-{}", std::process::id()));
        let mut r = asking(run(&[("plan", false), ("change", false)], DeliveryKind::Branch));
        let answers = BTreeMap::from([(VerdictKind::Keep, "Yes".to_string()), (VerdictKind::Redo, "No, it's something else".to_string())]);
        r.manifest.steps[0].stop = Some(jaynes_core::manifest::StopDecl { settled: None, holds_unless: None, vouched_by: None,
            verdicts: vec![VerdictKind::Keep, VerdictKind::Redo, VerdictKind::Drop], holds: true, from_risk: Default::default(), release: false, budget: None,
            audit_rate: jaynes_core::manifest::Rate(0.1), about: "Is this the bug?".into(), choose: false, redo: None, rounds: None, answers: answers.clone(), note: String::new(),
            offers: Default::default(),
        });
        let stop = waiting(&home, &r).expect("a stop");
        assert_eq!(stop.answers, answers);
        assert_eq!((stop.label(VerdictKind::Keep), stop.label(VerdictKind::Redo), stop.label(VerdictKind::Drop)), ("Yes", "No, it's something else", "Drop"));
        // a question of Jaynes's own at that step (a leave the agent asks) keeps Jaynes's words
        r.questions.get_mut("q1").expect("asked").question.kind = QuestionKind::Permission;
        let leave = waiting(&home, &r).expect("a stop");
        assert!(leave.answers.is_empty());
        assert_eq!((leave.label(VerdictKind::Keep), leave.label(VerdictKind::Drop)), ("Allow", "Refuse"));
    }

    #[test]
    fn asked_a_stop_carries_the_role_s_note_under_its_question_and_a_leave_none() {
        let home = std::env::temp_dir().join(format!("jaynes-stop-note-{}", std::process::id()));
        let mut r = asking(run(&[("plan", false), ("change", false)], DeliveryKind::Branch));
        r.manifest.steps[0].stop = Some(jaynes_core::manifest::StopDecl {
            verdicts: vec![VerdictKind::Keep, VerdictKind::Redo], holds: true, from_risk: Default::default(), release: false, budget: None,
            audit_rate: jaynes_core::manifest::Rate(0.1), about: "Which colours and fonts do we keep?".into(), choose: false, redo: None, rounds: None,
            answers: BTreeMap::new(), note: "The layout of your screens comes later.".into(), settled: None, holds_unless: None, vouched_by: None,
            offers: Default::default(),
        });
        assert_eq!(waiting(&home, &r).expect("a stop").note, "The layout of your screens comes later.");
        r.questions.get_mut("q1").expect("asked").question.kind = QuestionKind::Permission;
        assert_eq!(waiting(&home, &r).expect("a stop").note, "", "a question of Jaynes's own says no note of the role's");
    }

    #[test]
    fn what_cannot_be_undone_ahead_is_said() {
        assert_eq!(ahead(&run(&[("plan", false), ("change", false)], DeliveryKind::Branch), "plan"), None);
        let deploy = run(&[("plan", false), ("deploy", true)], DeliveryKind::Branch);
        // the step's name in curly quotes, as every sentence names a step ("Ahead: “deploy” cannot be undone.")
        assert_eq!(ahead(&deploy, "plan").as_deref(), Some("“deploy” cannot be undone"));
        let merge = run(&[("plan", false), ("change", false)], DeliveryKind::Merge);
        assert_eq!(ahead(&merge, "plan").as_deref(), Some("the run merges into your branch"));
    }

    fn leave(mut r: Run, about: &str) -> Run {
        let question = Question { rides: Vec::new(),
            id: "q1".into(), step: "plan".into(), round: 1, kind: QuestionKind::Permission,
            items: vec![QItem { named: None, id: "leave".into(), label: about.into(), digest: jaynes_core::Digest("d-leave".into()) }],
            offered: vec![VerdictKind::Keep, VerdictKind::Drop], holds: true, about: about.into(),
            choose: false, pass: 0, passes: 0, back: vec![],
        };
        r.questions.insert("q1".into(), QuestionState { question, answer: None, effects: None, superseded: false });
        r.phase = Phase::Waiting { step: Some("plan".into()), on: vec!["q1".into()], why: "waits".into() };
        r
    }

    // Alan, 21/11: "pq to sendo chamado aqui? a role?"
    #[test]
    fn asked_permission_says_why_you() {
        let home = std::env::temp_dir().join(format!("jaynes-stop-leave-{}", std::process::id()));
        let _fresh = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("home");
        let r = leave(run(&[("plan", false), ("change", false)], DeliveryKind::Merge), "change tests/conftest.py");
        // what the agent sent with its request, kept beside the run as it asked
        let wants = Wants { file: "tests/conftest.py".into(), before: None, after: "import pytest\n".into() };
        crate::permit::keep_wants(&home, "r1", "q1", &wants).expect("kept");
        let stop = waiting(&home, &r).expect("a stop");
        assert_eq!(stop.title, "May the agent change `tests/conftest.py`?");
        assert_eq!(stop.why.as_deref(), Some("Why you: the agent asked for something its step's settings do not allow."));
        // an answer to a leave never takes the run to the merge: the merge asks by itself
        assert_eq!(stop.ahead, None);
        assert_eq!(stop.says(VerdictKind::Keep), "It writes the file and goes on. Before it lands in your branch, the merge asks you again.");
        assert_eq!(stop.says(VerdictKind::Drop), "It goes on without writing it.");
        assert_eq!(stop.wants, Some(wants));
        // a file that runs nowhere later: the merge has no more to ask about it
        let plain = leave(run(&[("plan", false), ("change", false)], DeliveryKind::Merge), "change src/app.py");
        let stop = waiting(&home, &plain).expect("a stop");
        assert_eq!(stop.says(VerdictKind::Keep), "It writes the file and goes on.");
        // the other kinds of leave, each in its own verb, and nothing to see written
        for (about, title, allow, refuse) in [
            ("read /etc/hosts", "May the agent read `/etc/hosts`?", "It reads it and goes on.", "It goes on without reading it."),
            ("reach https://example.com", "May the agent reach `https://example.com`?", "It reaches it and goes on.", "It goes on without reaching it."),
            ("run make deploy", "May the agent run `make deploy`?", "It runs it and goes on.", "It goes on without running it."),
        ] {
            let mut r = leave(run(&[("plan", false), ("change", false)], DeliveryKind::Merge), about);
            r.id = "r2".into();
            let stop = waiting(&home, &r).expect("a stop");
            assert_eq!((stop.title.as_str(), stop.says(VerdictKind::Keep), stop.says(VerdictKind::Drop)), (title, allow, refuse));
            assert_eq!(stop.wants, None);
        }
        // a stop that is no leave says no why of this kind, and its ahead is as before
        let decision = waiting(&home, &asking(run(&[("plan", false), ("change", false)], DeliveryKind::Merge))).expect("a stop");
        assert_eq!((decision.why, decision.ahead.as_deref()), (None, Some("the run merges into your branch")));
        let _gone = std::fs::remove_dir_all(&home);
    }
}
