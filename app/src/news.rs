//! What is worth telling the person when they are not looking: a run that starts to need them (or needs them again, at
//! another question), one that delivered, one that blocked. Only changes count: what stood when the app opened is in
//! the list already, and is not news.

use crate::model::Snapshot;
use jaynes_shell::listing::Mark;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct News {
    pub run: String,
    pub title: String,
    pub body: String,
}

/// The news between two readings of the record.
pub fn between(before: &Snapshot, now: &Snapshot) -> Vec<News> {
    let mut news = Vec::new();
    for (id, d) in &now.details {
        let was = before.details.get(id);
        let asked = now.stops.get(id);
        let new_question = asked.is_some_and(|s| before.stops.get(id).map(|b| &b.question) != Some(&s.question));
        if d.mark == Mark::Needs && new_question {
            let body = asked.map(|s| s.title.clone()).unwrap_or_else(|| d.line.clone());
            news.push(News { run: id.clone(), title: format!("{} needs you", d.project), body });
            continue;
        }
        if was.map(|w| w.mark) == Some(d.mark) {
            continue;
        }
        match d.mark {
            Mark::Delivered | Mark::Merged => news.push(News { run: id.clone(), title: format!("{}: {}", d.project, crate::look::word(d.mark)), body: d.title.clone() }),
            Mark::Blocked => news.push(News { run: id.clone(), title: format!("{} is blocked", d.project), body: format!("{}: {}", d.title, d.line) }),
            _ => {}
        }
    }
    news
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Detail;
    use jaynes_shell::stop::Stop;

    fn detail(id: &str, mark: Mark, line: &str) -> Detail {
        Detail { id: id.into(), title: format!("run {id}"), role: "r".into(), project: "app".into(), mark, line: line.into(), minutes: 0, stages: vec![], now: None, delivered: false }
    }

    fn stop(run: &str, question: &str, title: &str) -> Stop {
        Stop {
            run: run.into(), question: question.into(), shown: String::new(), title: title.into(), asks: String::new(), step: "s".into(), permission: false, kind: jaynes_core::question::QuestionKind::Decision, choose: false,
            offered: vec![], items: vec![], back: vec![], round: 0, about: String::new(), ahead: None, touch: None, into: String::new(), lands: vec![], passed: None, answers: Default::default(), why: None, consequences: Default::default(), wants: None,
        }
    }

    fn snap(runs: &[(Detail, Option<Stop>)]) -> Snapshot {
        let mut s = Snapshot::default();
        for (d, st) in runs {
            if let Some(st) = st {
                s.stops.insert(d.id.clone(), st.clone());
            }
            s.details.insert(d.id.clone(), d.clone());
        }
        s
    }

    #[test]
    fn a_run_that_starts_to_need_the_person_delivers_or_blocks_is_news_and_nothing_else_is() {
        let before = snap(&[
            (detail("a", Mark::Working, "step 1 of 2, running"), None),
            (detail("b", Mark::Working, "step 2 of 2, running"), None),
            (detail("c", Mark::Working, "step 1 of 3, running"), None),
            (detail("d", Mark::Needs, "Which plan?"), Some(stop("d", "q1", "Which plan?"))),
            (detail("e", Mark::Working, "step 1 of 2, running"), None),
        ]);
        let now = snap(&[
            (detail("a", Mark::Needs, "Keep it?"), Some(stop("a", "q1", "Keep it?"))),
            (detail("b", Mark::Merged, "merged"), None),
            (detail("c", Mark::Blocked, "step 1 of 3, blocked: no tests"), None),
            // the same question still waits: no news
            (detail("d", Mark::Needs, "Which plan?"), Some(stop("d", "q1", "Which plan?"))),
            // still working, a step further: no news
            (detail("e", Mark::Working, "step 2 of 2, running"), None),
            // a run that appears already waiting is news too
            (detail("f", Mark::Needs, "May the agent read ~/.ssh?"), Some(stop("f", "q1", "May the agent read ~/.ssh?"))),
        ]);
        let mut news = between(&before, &now);
        news.sort_by(|x, y| x.run.cmp(&y.run));
        let said: Vec<(&str, &str, &str)> = news.iter().map(|n| (n.run.as_str(), n.title.as_str(), n.body.as_str())).collect();
        assert_eq!(
            said,
            [
                ("a", "app needs you", "Keep it?"),
                ("b", "app: merged", "run b"),
                ("c", "app is blocked", "run c: step 1 of 3, blocked: no tests"),
                ("f", "app needs you", "May the agent read ~/.ssh?"),
            ]
        );
    }

    #[test]
    fn another_question_of_a_waiting_run_is_news() {
        let before = snap(&[(detail("a", Mark::Needs, "Keep it?"), Some(stop("a", "q1", "Keep it?")))]);
        let now = snap(&[(detail("a", Mark::Needs, "And the tests?"), Some(stop("a", "q2", "And the tests?")))]);
        assert_eq!(between(&before, &now).len(), 1);
    }
}
