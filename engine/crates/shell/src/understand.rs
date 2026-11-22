//! The person's own words at a stop, read into the answer they mean. A reading is only a proposal: the person sees
//! it with what Jaynes says it will do, written by the engine and not by the reader, and sends it or corrects it. It
//! answers nothing by itself. The reader gives only verdicts: what a change or a bringing back says is the person's
//! words as they wrote them, never the reader's retelling, which can add a fact they did not say.

use jaynes_core::question::{Item, Question, VerdictKind};
use jaynes_core::Run;
use serde_json::{json, Map, Value};
use std::io::Write;
use std::process::{Command, Stdio};

/// What the reader is told: the stop, what is in it now, what was dropped before, what the person said in earlier
/// rounds, and their words now.
pub fn prompt(run: &Run, q: &Question, detail: &dyn Fn(&Item) -> String, text: &str, pressed: Option<&str>) -> String {
    let mut p = String::from(
        "You read what a person wrote at a stop of a run and turn it into the answer they mean. You do not decide for \
         them: they see your reading, with what it will do, and send it or correct it.\n\n",
    );
    let about = if q.about.trim().is_empty() { q.step.clone() } else { q.about.clone() };
    p.push_str(&format!("# The stop\n\n{about}\n\n"));
    if q.choose {
        p.push_str(
            "It chooses among options. One option kept alone is chosen as it is. One option changed is chosen and \
             adjusted. Two or three changed are finalists the agent develops before asking again. All dropped is none: \
             new options are made from the person's words.\n\n",
        );
        if q.one_option() {
            p.push_str(
                "With one option there is nothing to choose among: keep is “go with it”; change is “not yet”: their words \
                 go to the agent as a question or a change, and nothing is chosen; drop is “not this one”. A question or \
                 a doubt is never keep.\n\n",
            );
        }
    } else {
        p.push_str("Each item is judged on its own.\n\n");
    }
    p.push_str("# Now\n\n");
    for it in &q.items {
        p.push_str(&format!("- {} (id {}){}\n", it.label, it.id, line(&detail(it))));
    }
    if !q.back.is_empty() {
        p.push_str("\n# Dropped in earlier rounds, which the person may bring back\n\n");
        for it in &q.back {
            p.push_str(&format!("- {} (id {}){}\n", it.label, it.id, line(&detail(it))));
        }
    }
    let earlier = said_before(run, q);
    if !earlier.is_empty() {
        p.push_str("\n# What the person asked in earlier rounds, oldest first\n\n");
        for e in &earlier {
            p.push_str(&format!("- {e}\n"));
        }
    }
    let offered: Vec<&str> = q
        .offered
        .iter()
        .filter_map(|v| match v {
            VerdictKind::Keep => Some("keep (as it is)"),
            VerdictKind::Change => Some("change (they want it different)"),
            VerdictKind::Drop => Some("drop (it leaves; it comes back only if the person brings it back)"),
            _ => None,
        })
        .collect();
    p.push_str(&format!("\n# What they wrote now\n\n«{}»\n\n", text.trim()));
    if let Some(b) = pressed {
        p.push_str(&format!("They also pressed: {b}. A button is only a hint: their words come first.\n\n"));
    }
    p.push_str(&format!(
        "# How to read it\n\n\
         - Verdicts for an item now: {}. An item you leave out is kept as it is.\n\
         - To bring back something dropped earlier, name it in back: \"as it was\", or \"changed\" when they want it \
         different.\n\
         - Only reject what they reject. When they say they liked a part of an option, it is not dropped: change it.\n\
         - Their words go with a change as they wrote them: you write no note and no summary of them.\n\
         - If they ask again for something they asked in an earlier round, quote it in repeats.\n\
         - If you cannot tell what they mean, still give your best reading, and ask one short question in unsure, in \
         their language.\n\n\
         Answer with this JSON only:\n\
         {{\"verdicts\": {{\"<id>\": \"keep|change|drop\"}}, \"back\": {{\"<id>\": \"as it was|changed\"}},\"unsure\": \"\", \"repeats\": []}}\n",
        offered.join(", ")
    ));
    p
}

fn line(detail: &str) -> String {
    let d = detail.trim();
    if d.is_empty() { String::new() } else { format!(": {d}") }
}

/// The person's words for the step this stop is about in earlier rounds, and their context there.
fn said_before(run: &Run, q: &Question) -> Vec<String> {
    let Some(i) = run.manifest.index(&q.step) else { return Vec::new() };
    let maker = run.manifest.steps[i].stop.as_ref().and_then(|s| s.redo.as_ref()).and_then(|r| run.manifest.index(r));
    let mut out: Vec<String> = maker.map(|j| run.steps[j].earlier.clone()).unwrap_or_default();
    out.extend(run.steps[i].earlier.iter().cloned());
    out.extend(run.context.iter().filter(|(at, _)| *at == q.step).map(|(_, t)| t.clone()));
    out.dedup();
    out
}

/// What the reader proposed, kept only where it fits the question: items and verdicts it offers, things that were
/// dropped. Of the reader only the verdicts are kept: a change, and a thing brought back changed, carry the person's
/// words as they wrote them, and the words always go as context. Nothing the reader wrote is kept as theirs (the
/// real-user, 22/11: "do jeito que você fez. entrega assim." came back as "you wrote: … A 5432 continua ocupada", which
/// the person never said).
pub fn proposal(q: &Question, read: &Value, text: &str) -> Value {
    let mut verdicts = Map::new();
    let mut notes = Map::new();
    let words = text.trim();
    for (id, v) in read["verdicts"].as_object().into_iter().flatten() {
        let Some(v) = v.as_str() else { continue };
        let kind = match v {
            "keep" => VerdictKind::Keep,
            "change" => VerdictKind::Change,
            "drop" => VerdictKind::Drop,
            _ => continue,
        };
        if !q.items.iter().any(|x| &x.id == id) || !q.offered.contains(&kind) {
            continue;
        }
        // a change with nothing said is no change: it would be refused
        if kind == VerdictKind::Change && words.is_empty() {
            continue;
        }
        verdicts.insert(id.clone(), json!(v));
        if kind == VerdictKind::Change {
            notes.insert(id.clone(), json!(words));
        }
    }
    let mut back = Map::new();
    for (id, how) in read["back"].as_object().into_iter().flatten() {
        if q.back.iter().any(|x| &x.id == id) {
            // as it was is empty; changed carries the person's words, never the reader's
            let as_it_was = matches!(how.as_str().map(|h| h.trim().to_lowercase()).as_deref(), None | Some("" | "as it was"));
            back.insert(id.clone(), json!(if as_it_was { "" } else { words }));
        }
    }
    let repeats: Vec<Value> = read["repeats"].as_array().into_iter().flatten().filter_map(|r| r.as_str()).map(|r| json!(r)).collect();
    json!({
        "verdicts": verdicts,
        "notes": notes,
        "back": back,
        "context": words,
        "unsure": read["unsure"].as_str().unwrap_or("").trim(),
        "repeats": repeats,
    })
}

/// Ask the run's model to read the words: Claude Code with no tool, no project and no settings, one answer in JSON.
/// A run driven by a command reads through `JAYNES_UNDERSTAND`, a command given the prompt on its input.
pub fn read(agent: &str, prompt: &str) -> Result<Value, String> {
    let mut cmd = if let Ok(c) = std::env::var("JAYNES_UNDERSTAND") {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &c]);
        cmd
    } else if agent.starts_with("codex:") {
        return Err("this run's agent is Codex, and Jaynes does not yet read your words with it: answer with the buttons".into());
    } else if agent.starts_with("command:") {
        return Err("this run's agent is a command, so no model reads your words: answer with the buttons".into());
    } else {
        let model = agent.strip_prefix("claude:").filter(|m| !m.is_empty()).map(str::to_string).unwrap_or_else(|| {
            std::env::var("JAYNES_CLAUDE_MODEL").unwrap_or_else(|_| "claude-opus-5-5".to_string())
        });
        let dir = std::env::temp_dir().join("jaynes-understand");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut cmd = Command::new("claude");
        cmd.args(["-p", "--output-format", "json", "--setting-sources", "", "--tools", "", "--strict-mcp-config"])
            .args(["--mcp-config", "{\"mcpServers\":{}}", "--no-session-persistence", "--model", &model, "--effort", "low"])
            .current_dir(&dir)
            .env("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1");
        cmd
    };
    use std::os::unix::process::CommandExt;
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("the reader could not start: {e}"))?;
    child.stdin.take().ok_or("no input to the reader")?.write_all(prompt.as_bytes()).map_err(|e| e.to_string())?;
    // what it prints is read as it comes, so a reader that prints much never stalls on a full pipe
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            if let Some(mut p) = pipe {
                let _read = p.read_to_end(&mut kept);
            }
            kept
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let seconds = deadline();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if std::time::Instant::now() >= until {
            // the reader and whatever it started are stopped together
            let _killed = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).stderr(Stdio::null()).status();
            let _reaped = child.wait();
            return Err(format!("Jaynes stopped reading your words: the model took longer than {seconds} s. Answer with the buttons, or try again."));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let (stdout, stderr) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if !status.success() {
        return Err(format!("the reader failed: {}", String::from_utf8_lossy(&stderr).trim()));
    }
    let said = String::from_utf8_lossy(&stdout).to_string();
    // Claude Code wraps the text in its result; a command prints the JSON itself
    let text = serde_json::from_str::<Value>(&said)
        .ok()
        .and_then(|v| v["result"].as_str().map(str::to_string))
        .unwrap_or(said);
    json_in(&text).ok_or_else(|| "the reader did not answer in JSON".to_string())
}

/// How long a reading of the person's words may take before its model is stopped: JAYNES_UNDERSTAND_SECONDS, or
/// 90 seconds.
fn deadline() -> u64 {
    std::env::var("JAYNES_UNDERSTAND_SECONDS").ok().and_then(|s| s.parse().ok()).filter(|s| *s > 0).unwrap_or(90)
}

/// The first JSON object in a text, fenced or not.
fn json_in(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jaynes_core::digest::Digest;
    use jaynes_core::question::QuestionKind;

    fn item(id: &str) -> Item {
        Item { named: None, id: id.into(), label: id.into(), digest: Digest(id.into()) }
    }

    fn question() -> Question {
        Question { rides: Vec::new(),
            back: vec![item("Lastro")],
            id: "q1".into(),
            step: "pick".into(),
            round: 1,
            kind: QuestionKind::Decision,
            items: vec![item("Razão"), item("Prumo")],
            offered: vec![VerdictKind::Keep, VerdictKind::Change, VerdictKind::Drop],
            holds: true,
            about: "Choose the colours".into(),
            choose: true,
            pass: 2,
            passes: 0,
        }
    }

    #[test]
    fn a_reading_keeps_only_what_fits_the_question_and_the_words_go_as_context() {
        let read = json!({
            "verdicts": {"Razão": "drop", "Prumo": "change", "Nope": "keep", "Ghost": "explode"},
            "notes": {"Prumo": "keep its colours, money in green"},
            "back": {"Lastro": "money in green", "Never": "x"},
            "unsure": "",
            "repeats": ["money in green"]
        });
        let p = proposal(&question(), &read, "  I liked the colours, but money should be green ");
        assert_eq!(p["verdicts"], json!({"Razão": "drop", "Prumo": "change"}));
        // the reader's notes are never kept: the change carries the person's words
        assert_eq!(p["notes"], json!({"Prumo": "I liked the colours, but money should be green"}));
        assert_eq!(p["back"], json!({"Lastro": "I liked the colours, but money should be green"}));
        assert_eq!(p["context"], "I liked the colours, but money should be green");
        let as_it_was = proposal(&question(), &json!({"back": {"Lastro": "as it was"}}), "bring Lastro back");
        assert_eq!(as_it_was["back"], json!({"Lastro": ""}));
        assert_eq!(p["repeats"], json!(["money in green"]));
    }

    #[test]
    fn a_change_with_nothing_said_is_left_out_rather_than_refused() {
        let p = proposal(&question(), &json!({"verdicts": {"Prumo": "change"}}), "  ");
        assert_eq!(p["verdicts"], json!({}));
        // a note the reader wrote is not words the person said
        let p = proposal(&question(), &json!({"verdicts": {"Prumo": "change"}, "notes": {"Prumo": "money in green"}}), " ");
        assert_eq!(p["verdicts"], json!({}));
    }

    // Alan, 21/11: "entender onde o python entra antes de decidir"
    #[test]
    fn with_one_option_a_question_or_a_doubt_is_never_keep() {
        use jaynes_core::manifest::{Executor, Manifest, Reach, SessionKind, StepDecl};
        let step = StepDecl {
            name: "pick".into(), needs: vec![], executor: Executor::Agent, session: SessionKind::Fresh, reach: Reach::default(),
            effect: Default::default(), checks: vec![], stop: None, from_risk: Default::default(), attempts: 3, fixes: 2, hidden: false, idempotent: false, about: String::new(),
            model: Default::default(),
            turns: None, delivered: true,
        };
        let manifest = Manifest { role: "r".into(), version: "1".into(), protocol: 1, steps: vec![step], target: Default::default(), delivery: Default::default() };
        let start = jaynes_core::Input::Start { run: "r".into(), request: "x".into(), manifest, role_fingerprint: Digest("f".into()), agent: "claude:m".into(), raise: None, attention: Default::default(), task: None, arm: None, never_merge: false, starter: None };
        let run = Run::fold(&jaynes_core::decide(None, &start).expect("start").events).expect("run");
        let sentence = "With one option there is nothing to choose among: keep is “go with it”; change is “not yet”: their words go to the agent as a question or a change, and nothing is chosen; drop is “not this one”. A question or a doubt is never keep.";
        let words = "entender onde o python entra antes de decidir";
        let one = Question { items: vec![item("Python + FastAPI")], back: vec![], ..question() };
        assert!(prompt(&run, &one, &|_| String::new(), words, None).contains(sentence));
        // two or more: nothing changes
        assert!(!prompt(&run, &question(), &|_| String::new(), words, None).contains("With one option"));
    }

    #[test]
    fn the_json_is_found_in_a_fenced_answer() {
        assert_eq!(json_in("Here:\n```json\n{\"verdicts\": {}}\n```"), Some(json!({"verdicts": {}})));
        assert_eq!(json_in("no json"), None);
    }
}
