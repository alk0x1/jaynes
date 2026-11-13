//! When a stop may stop asking: the anytime-valid test of the Python engine (`jaynes.bounds`), ported, and the
//! count of a stop's outcomes made from the record (`jaynes.outcome`).
//!
//! A stop with a budget becomes sampled only while the mixture likelihood ratio against "its error rate is at or
//! above the budget" reaches 1/δ. If the true rate is at or above the budget, that statistic is a nonnegative
//! supermartingale starting at 1, so by Ville's inequality the chance it ever reaches 1/δ is at most δ, however
//! often it is checked.
//!
//! The count is a pure function of the record and of the time it is asked at, which the shell passes in.

use crate::digest::Digest;
use crate::event::{Event, Outcome};
use crate::manifest::{Executor, Manifest};
use crate::question::{By, QuestionKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A release counts as right once its delivery held this long with no correction.
pub const SETTLE: u64 = 7 * 24 * 3600;
/// Errors among a stop's last outcomes that brake it.
pub const BRAKE_ERRORS: usize = 2;
pub const BRAKE_LAST: usize = 10;

// --- the statistics ------------------------------------------------------------------------------------------

fn ln_gamma(x: f64) -> f64 {
    // Lanczos, g = 7, nine coefficients: about 15 significant digits for x > 0.
    const C: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        let pi = std::f64::consts::PI;
        return (pi / (pi * x).sin()).ln() - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let mut a = C[0];
    let t = x + 7.5;
    for (i, c) in C.iter().enumerate().skip(1) {
        a += c / (x + i as f64);
    }
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
}

fn log_beta(a: f64, b: f64) -> f64 {
    ln_gamma(a) + ln_gamma(b) - ln_gamma(a + b)
}

fn betacf(x: f64, a: f64, b: f64) -> f64 {
    const MAXIT: usize = 400;
    const EPS: f64 = 3e-15;
    const FPMIN: f64 = 1e-300;
    let clamp = |v: f64| if v.abs() > FPMIN { v } else { FPMIN };
    let (qab, qap, qam) = (a + b, a + 1.0, a - 1.0);
    let mut c = 1.0;
    let mut d = 1.0 / clamp(1.0 - qab * x / qap);
    let mut h = d;
    for m in 1..=MAXIT {
        let m = m as f64;
        let m2 = 2.0 * m;
        let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
        d = 1.0 / clamp(1.0 + aa * d);
        c = clamp(1.0 + aa / c);
        h *= d * c;
        let aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        d = 1.0 / clamp(1.0 + aa * d);
        c = clamp(1.0 + aa / c);
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < EPS {
            break;
        }
    }
    h
}

/// log of the regularized incomplete beta I_x(a, b), accurate in the lower tail.
fn log_betainc_lower(x: f64, a: f64, b: f64) -> f64 {
    if x <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if x >= 1.0 {
        return 0.0;
    }
    let front = a * x.ln() + b * (-x).ln_1p() - log_beta(a, b);
    if x < (a + 1.0) / (a + b + 2.0) {
        front + betacf(x, a, b).ln() - a.ln()
    } else {
        let upper = (front + betacf(1.0 - x, b, a).ln() - b.ln()).exp();
        if upper < 1.0 { (-upper).ln_1p() } else { f64::NEG_INFINITY }
    }
}

/// log e-value against "the error rate is at or above `budget`" after `k` errors in `n` outcomes.
pub fn log_certify(n: u64, k: u64, budget: f64) -> f64 {
    let (a, b) = (k as f64 + 1.0, (n - k) as f64 + 1.0);
    log_betainc_lower(budget, a, b) + log_beta(a, b) - (k as f64 + 1.0) * budget.ln() - (n - k) as f64 * (-budget).ln_1p()
}

/// log e-value against "the error rate is at or below `budget`".
pub fn log_refute(n: u64, k: u64, budget: f64) -> f64 {
    let (a, b) = (k as f64 + 1.0, (n - k) as f64 + 1.0);
    log_betainc_lower(1.0 - budget, b, a) + log_beta(a, b) - k as f64 * budget.ln() - ((n - k) as f64 + 1.0) * (-budget).ln_1p()
}

pub fn certified(n: u64, k: u64, budget: f64, delta: f64) -> bool {
    n > 0 && log_certify(n, k, budget) >= (1.0 / delta).ln()
}

pub fn refuted(n: u64, k: u64, budget: f64, delta: f64) -> bool {
    n > 0 && log_refute(n, k, budget) >= (1.0 / delta).ln()
}

/// A deterministic draw in [0, 1) for a run's step and round, so reading the record again reproduces it.
pub fn coin(run: &str, step: &str, round: u32) -> f64 {
    let d = Digest::of_json(&(run, step, round));
    let n = u64::from_str_radix(d.0.get(..16).unwrap_or("0"), 16).unwrap_or(0);
    n as f64 / 18_446_744_073_709_551_616.0
}

// --- the count of a stop -------------------------------------------------------------------------------------

/// Which stop an outcome belongs to: the role, the step as declared, and the role's content. Any change to the
/// step or to the role starts a new count.
pub fn stop_key(manifest: &Manifest, role_fingerprint: &Digest, step: usize) -> Digest {
    Digest::of_json(&(&manifest.role, &manifest.steps[step], role_fingerprint))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The stop has no budget: it always asks.
    Floor,
    Certified,
    Refuted,
    Undetermined,
}

/// A stop's count, as it stood when a run started.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Count {
    pub key: Digest,
    /// Outcomes known, and errors among them.
    pub n: u64,
    pub k: u64,
    /// Releases still waiting for their delivery to hold.
    pub pending: u64,
    pub braked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    Pending,
    Right,
    Error,
    Nothing,
}

#[derive(Clone, Debug)]
struct Item {
    key: Digest,
    at: u64,
    state: State,
    earns: bool,
}

/// Every outcome of every stop in `history`: each event of the record with its run and the time it was recorded.
fn items(history: &[(u64, String, Event)], now: u64) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::new();
    let mut runs: BTreeMap<String, (Manifest, Digest)> = BTreeMap::new();
    let mut last: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut questions: BTreeMap<(String, String), (String, QuestionKind)> = BTreeMap::new();
    let mut finished: BTreeMap<String, (u64, Outcome)> = BTreeMap::new();
    let mut corrected: BTreeMap<String, Vec<Option<String>>> = BTreeMap::new();
    for (at, run, e) in history {
        let key_of = |step: &str, runs: &BTreeMap<String, (Manifest, Digest)>| {
            runs.get(run).and_then(|(m, f)| m.index(step).map(|i| (stop_key(m, f, i), m.steps[i].executor)))
        };
        match e {
            Event::RunStarted { manifest, role_fingerprint, .. } => {
                runs.insert(run.clone(), (manifest.clone(), role_fingerprint.clone()));
            }
            Event::QuestionOpened { question } => {
                questions.insert((run.clone(), question.id.clone()), (question.step.clone(), question.kind));
            }
            Event::StopSkipped { step, .. } => {
                if let Some((key, _)) = key_of(step, &runs) {
                    release(&mut out, &mut last, run, step, Item { key, at: *at, state: State::Pending, earns: true });
                }
            }
            Event::AnswerRecorded { answer, .. } => {
                let Some((step, kind)) = questions.get(&(run.clone(), answer.question.clone())).cloned() else { continue };
                if kind != QuestionKind::Decision {
                    continue;
                }
                let Some((key, executor)) = key_of(&step, &runs) else { continue };
                if executor == Executor::Person {
                    continue;
                }
                let person = matches!(answer.by, By::Person);
                if answer.sends_back() {
                    if !person {
                        continue;
                    }
                    // A person sending back what they let through before: that release was an error.
                    if let Some(&j) = last.get(&(run.clone(), step.clone()))
                        && (out[j].state == State::Pending || out[j].state == State::Right) {
                            out[j].state = State::Error;
                            last.remove(&(run.clone(), step.clone()));
                            continue;
                        }
                    out.push(Item { key, at: *at, state: State::Error, earns: false });
                } else {
                    release(&mut out, &mut last, run, &step, Item { key, at: *at, state: State::Pending, earns: person });
                }
            }
            Event::StepInvalidated { step, .. } => {
                // Ran again: only the release of what was finally delivered counts.
                if let Some(j) = last.remove(&(run.clone(), step.clone()))
                    && out[j].state == State::Pending {
                        out[j].state = State::Nothing;
                    }
            }
            Event::RunFinished { outcome, .. } => {
                finished.insert(run.clone(), (*at, *outcome));
            }
            Event::DeliveryCorrected { step, .. } => corrected.entry(run.clone()).or_default().push(step.clone()),
            _ => {}
        }
    }
    for (run, steps) in &corrected {
        let Some((m, _)) = runs.get(run) else { continue };
        for step in steps {
            // The correction counts on the first stop at or after the step that erred that let it through.
            let from = step.as_ref().and_then(|s| m.index(s)).unwrap_or(0);
            let hit = m.steps.iter().enumerate().skip(from).find_map(|(i, s)| last.get(&(run.clone(), s.name.clone())).map(|&j| (i, j)));
            let hit = hit.or_else(|| {
                m.steps.iter().enumerate().rev().find_map(|(i, s)| last.get(&(run.clone(), s.name.clone())).map(|&j| (i, j)))
            });
            if let Some((_, j)) = hit {
                out[j].state = State::Error;
            }
        }
    }
    // Pending releases of finished runs: right once the delivery held, nothing if the run was abandoned.
    let owner: BTreeMap<usize, String> = last.iter().map(|((run, _), &j)| (j, run.clone())).collect();
    for (j, item) in out.iter_mut().enumerate() {
        if item.state != State::Pending {
            continue;
        }
        let Some(run) = owner.get(&j) else { continue };
        match finished.get(run) {
            Some((_, Outcome::Abandoned)) => item.state = State::Nothing,
            // Nobody looked at what an agent let through: it never earns a right, and a correction still counts.
            Some((when, Outcome::Delivered)) if item.earns && now >= when.saturating_add(SETTLE) => item.state = State::Right,
            _ => {}
        }
    }
    out
}

fn release(out: &mut Vec<Item>, last: &mut BTreeMap<(String, String), usize>, run: &str, step: &str, item: Item) {
    if let Some(j) = last.get(&(run.to_string(), step.to_string()))
        && out[*j].state == State::Pending {
            out[*j].state = State::Nothing;
        }
    out.push(item);
    last.insert((run.to_string(), step.to_string()), out.len() - 1);
}

/// The count of the stop `key`, from the record, at time `now`. With its budget and δ, whether it is braked.
pub fn count(history: &[(u64, String, Event)], key: &Digest, now: u64, budget: Option<f64>, delta: f64) -> Count {
    let mut mine: Vec<Item> = items(history, now).into_iter().filter(|i| &i.key == key).collect();
    mine.sort_by_key(|i| i.at);
    let known: Vec<&Item> = mine.iter().filter(|i| i.state == State::Right || i.state == State::Error).collect();
    let pending = mine.iter().filter(|i| i.state == State::Pending && i.earns).count() as u64;
    let n = known.len() as u64;
    let k = known.iter().filter(|i| i.state == State::Error).count() as u64;
    let mut braked = false;
    if let Some(budget) = budget {
        let mut start = 0;
        for j in 0..known.len() {
            let seen = &known[start..=j];
            let tail = &seen[seen.len().saturating_sub(BRAKE_LAST)..];
            let errors = tail.iter().filter(|i| i.state == State::Error).count();
            if !braked && errors >= BRAKE_ERRORS {
                braked = true;
                start = j + 1;
            } else if braked {
                let sn = seen.len() as u64;
                let sk = seen.iter().filter(|i| i.state == State::Error).count() as u64;
                if certified(sn, sk, budget, delta) {
                    braked = false;
                    start = j + 1;
                }
            }
        }
    }
    Count { key: key.clone(), n, k, pending, braked }
}

/// What a stop does with its count: always ask, ask a sample, or ask every time.
pub fn status(count: &Count, budget: Option<f64>, delta: f64) -> Status {
    match budget {
        None => Status::Floor,
        Some(b) if certified(count.n, count.k, b, delta) => Status::Certified,
        Some(b) if refuted(count.n, count.k, b, delta) => Status::Refuted,
        Some(_) => Status::Undetermined,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_needed_with_no_error_match_the_python_engine() {
        // SPEC 6.4: at δ = 5%, 88 clean outcomes for a 5% budget, 43 for 10%, 20 for 20%, 13 for 30%.
        for (budget, need) in [(0.05, 88), (0.10, 43), (0.20, 20), (0.30, 13)] {
            assert!(!certified(need - 1, 0, budget, 0.05), "{budget}: {} certified too early", need - 1);
            assert!(certified(need, 0, budget, 0.05), "{budget}: {need} does not certify");
        }
    }

    #[test]
    fn many_errors_refute() {
        assert!(refuted(20, 10, 0.05, 0.05));
        assert!(!refuted(20, 1, 0.05, 0.05));
    }

    #[test]
    fn the_coin_is_uniform_enough_and_fixed() {
        assert_eq!(coin("r", "s", 0), coin("r", "s", 0));
        let below = (0..2000).filter(|i| coin(&format!("r{i}"), "s", 0) < 0.1).count();
        assert!((140..260).contains(&below), "{below}");
    }
}
