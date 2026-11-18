//! The menu's pages, through the real binary: a role in TOML that has Jaynes download the pages a step lists and
//! holds a later step's quotes to their text. No test reaches the network: what is asked here is refused before
//! anything is sent, and the check is given the text of a page as the step would have kept it.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const ROLE: &str = r#"# A test role: an agent lists sources, Jaynes downloads them, and a note may quote only what was downloaded.

role = "cites"
version = "1.0.0"

[[step]]
name = "read"
reach = { project = "none", network = "web" }
instructions = "Find sources for {{request}}."
checks = ["output-follows-schema"]

[step.output]
sources = { type = "array", items = { type = "object", fields = { url = "string", quotes = { type = "array", items = "string" } } } }

[[step]]
name = "fetch"
by = "jaynes"
use = "fetch-pages"
needs = ["read"]
with = { pages = "read.sources" }
reach = { network = "web" }

[[step]]
name = "note"
needs = ["read", "fetch"]
reach = { project = "write", write = ["notes/**"] }
instructions = "Write the note from {{read.sources}}, quoting only pages that were read: {{fetch.pages}}"
checks = ["output-follows-schema", { use = "quotes-found", field = "claims", pages = "fetch" }]

[step.output]
claims = { type = "array", items = { type = "object", fields = { url = "string", quote = "string" } } }

[[promise]]
says = "No note is delivered unless every quote in it is in a page Jaynes downloaded"
never = "deliver"
unless = "check quotes-found of note passed"
"#;

struct Scratch {
    home: PathBuf,
    role: PathBuf,
}

fn scratch(name: &str) -> Scratch {
    let base = std::env::temp_dir().join(format!("jaynes-pages-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let role = base.join("cites");
    std::fs::create_dir_all(&role).expect("dir");
    std::fs::write(role.join("role.toml"), ROLE).expect("write");
    Scratch { home: base.join("home"), role }
}

fn engine(s: &Scratch, args: &[&str]) -> (bool, String) {
    let out = Command::new(BIN)
        .args(args)
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
        .output()
        .expect("engine");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// One call to the role over the protocol, as the engine makes it.
fn serve(s: &Scratch, call: Value) -> Value {
    let mut child = Command::new(BIN)
        .args(["role-serve", s.role.to_str().expect("path")])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
        .stdin(Stdio::piped()).stdout(Stdio::piped())
        .spawn().expect("serve");
    use std::io::Write;
    child.stdin.take().expect("stdin").write_all(format!("{call}\n").as_bytes()).expect("write");
    let out = child.wait_with_output().expect("out");
    serde_json::from_slice::<Value>(&out.stdout).unwrap_or_else(|e| panic!("the role's reply does not read ({e}): {}", String::from_utf8_lossy(&out.stdout)))
}

#[test]
fn the_menu_s_pages_hold_their_rules() {
    // the rules themselves (what is refused, the limits, how two texts are compared) are tested where they are
    // written, in Python, with the name lookup and the connection passed in
    let python = root().join("python");
    let out = Command::new("python3")
        .args(["-m", "unittest", "discover", "-s"]).arg(python.join("tests"))
        .env("PYTHONPATH", &python).env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("python3");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{said}");
    assert!(said.contains("OK") && !said.contains("Ran 0 tests"), "{said}");
}

#[test]
fn a_role_that_downloads_pages_reads_and_its_promise_about_quotes_is_proved() {
    let s = scratch("reads");
    let (ok, out) = engine(&s, &["check", s.role.to_str().expect("path")]);
    assert!(ok, "{out}");
    assert!(out.contains("cites 1.0.0 reads: read → fetch → note"), "{out}");
    assert!(out.contains("✔ \"No note is delivered unless every quote in it is in a page Jaynes downloaded\": proved"), "{out}");
    // a new role's comments list the step and the check
    let (ok, out) = engine(&s, &["new", "fresh", s.home.parent().expect("base").join("fresh").to_str().expect("path")]);
    assert!(ok, "{out}");
    let fresh = std::fs::read_to_string(s.home.parent().expect("base").join("fresh/role.toml")).expect("read");
    assert!(fresh.contains("#   fetch-pages (pages): Jaynes downloads the pages an earlier step lists"), "{fresh}");
    assert!(fresh.contains("#   quotes-found (field, pages), open: every quote in FIELD is, word for word, in the text Jaynes downloaded"), "{fresh}");
}

#[test]
fn an_address_jaynes_may_not_download_is_a_page_not_read_and_nothing_is_sent() {
    let s = scratch("refuses");
    let sources = json!([
        {"url": "http://example.org/plain", "quotes": ["anything said over plain http"]},
        {"url": "https://127.0.0.1/admin", "quotes": ["anything said on this machine"]},
        {"url": "https://user:secret@example.org/", "quotes": ["anything said behind a password"]},
        {"url": "https://example.org:8443/", "quotes": ["anything said on another port"]},
    ]);
    let reply = serve(&s, json!({"jaynes": 1, "id": "x", "call": "execute", "step": "fetch", "request": "r", "round": 0, "attempt": 1,
        "given": [{"step": "read", "output": {"sources": sources}}], "previous": null, "answers": [], "key": null, "project": null, "source": null}));
    let output = &reply["output"];
    assert_eq!(output["any_read"], false, "{reply}");
    let why: Vec<&str> = output["pages"].as_array().expect("pages").iter().map(|p| p["why"].as_str().unwrap_or("")).collect();
    assert_eq!(why, ["only https addresses are downloaded", "its address is not on the public internet",
                     "its address carries a name or password", "its address names the port 8443, and only the usual one is used"], "{reply}");
    for page in output["pages"].as_array().expect("pages") {
        assert_eq!((page["read"].as_bool(), page["quotes"][0]["found"].as_bool()), (Some(false), Some(false)), "{page}");
    }
    assert_eq!(output["texts"], json!({}), "{reply}");
}

#[test]
fn a_quote_is_held_to_the_text_jaynes_downloaded() {
    let s = scratch("quotes");
    let text = "HTTP Caching\nA cache MUST NOT store a response\n  when the no-store directive is present.\n";
    let fingerprint = Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>();
    let fetched = json!({
        "pages": [
            {"url": "https://example.org/caching", "read": true, "why": "", "text_sha256": fingerprint},
            {"url": "https://example.org/blocked", "read": false, "why": "the server answered 403"},
        ],
        "texts": {"https://example.org/caching": text},
        "any_read": true,
    });
    let verdict = |claims: Value, fetched: &Value| -> Value {
        serve(&s, json!({"jaynes": 1, "id": "x", "call": "check", "step": "note", "check": "quotes-found", "request": "r",
            "output": {"claims": claims}, "given": [{"step": "read", "output": {"sources": []}}, {"step": "fetch", "output": fetched}],
            "project": null, "source": null}))["verdict"].clone()
    };
    // the words as the page has them, whatever their spacing
    let real = verdict(json!([{"url": "https://example.org/caching", "quote": "A cache MUST NOT store a response when the no-store directive is present."}]), &fetched);
    assert_eq!(real["verdict"], "pass", "{real}");
    // an invented quote is refused, with the quote and its page
    let invented = verdict(json!([{"url": "https://example.org/caching", "quote": "A cache MAY store anything it likes, forever."}]), &fetched);
    assert_eq!(invented["verdict"], "fail", "{invented}");
    let said = invented["reasons"][0].as_str().unwrap_or("");
    assert_eq!(invented["reasons"].as_array().map(Vec::len), Some(1), "{invented}");
    assert!(said.starts_with("\"A cache MAY store anything it likes, forever.\": these words are not in the text Jaynes downloaded from https://example.org/caching"), "{said}");
    // and what Jaynes read nearest to it, for the agent to copy what stands there
    assert!(said.contains("the nearest text Jaynes read there: \"HTTP Caching A cache MUST NOT store a response when the no-store directive is present.\""), "{said}");
    // a page that could not be read proves nothing, and the reason is said
    let blocked = verdict(json!([{"url": "https://example.org/blocked", "quote": "Whatever the page may have said."}]), &fetched);
    assert_eq!(blocked["verdict"], "fail", "{blocked}");
    assert!(blocked["reasons"][0].as_str().unwrap_or("").contains("Jaynes could not read https://example.org/blocked (the server answered 403)"), "{blocked}");
    // a page Jaynes never downloaded proves nothing either
    let elsewhere = verdict(json!([{"url": "https://example.org/other", "quote": "A cache MUST NOT store a response"}]), &fetched);
    assert!(elsewhere["reasons"][0].as_str().unwrap_or("").contains("was not downloaded by Jaynes in this run"), "{elsewhere}");
    // and a text that is not the one downloaded is not trusted
    let mut changed = fetched.clone();
    changed["texts"]["https://example.org/caching"] = json!(format!("{text} A cache MAY store anything it likes, forever."));
    let distrusted = verdict(json!([{"url": "https://example.org/caching", "quote": "A cache MAY store anything it likes, forever."}]), &changed);
    assert_eq!(distrusted["verdict"], "blocked", "{distrusted}");
}

const WINDOWS: &str = r#"# A test role: the note's quotes shown in their pages, at a stop that sends work back to the note.

role = "windows"
version = "1.0.0"

[[step]]
name = "read"
reach = { project = "none", network = "web" }
instructions = "Find sources for {{request}}."

[step.output]
sources = { type = "array", items = { type = "object", fields = { url = "string", quotes = { type = "array", items = "string" } } } }

[[step]]
name = "fetch"
by = "jaynes"
use = "fetch-pages"
needs = ["read"]
with = { pages = "read.sources" }
reach = { network = "web" }

[[step]]
name = "note"
needs = ["read", "fetch"]
reach = { project = "write", write = ["notes/**"] }
instructions = "Write the note from {{fetch.pages}}."

checks = [{ use = "quotes-found", field = "claims", pages = "fetch" }]

[step.output]
reading = "string"
claims = { type = "array", items = { type = "object", fields = { url = "string", quote = "string" } } }

[[step]]
name = "windows"
by = "jaynes"
use = "quote-windows"
needs = ["note", "fetch"]
with = { claims = "note.claims", pages = "fetch", beside = "note.reading" }
stop = { ask = "Do the sources say what the note says?", verdicts = ["keep", "change"], redo = "note" }
show = { label = "The note's quotations in their pages", detail = "{{beside}}", aspects = [{ label = "Quotations", text = "{{windows}}" }] }

[[promise]]
says = "No note is delivered unless every quote in it is in a page Jaynes downloaded"
never = "deliver"
unless = "check quotes-found of note passed"
"#;

/// A promise of the same shape that does not hold: the windows' stop does not hold the run, so the note can be
/// delivered before the person answered there.
const WINDOWS_FALSE: &str = r#"
[[promise]]
says = "No note is delivered before you answered at the windows"
never = "deliver"
unless = "person answered at windows"
"#;

#[test]
fn the_quotes_of_a_note_are_shown_in_their_pages_at_a_stop_of_jaynes_s_own() {
    let s = scratch("windows");
    std::fs::write(s.role.join("role.toml"), WINDOWS).expect("write");
    let (ok, out) = engine(&s, &["check", s.role.to_str().expect("path")]);
    assert!(ok && out.contains("windows 1.0.0 reads: read → fetch → note → windows"), "{out}");
    // the prover goes round the loop where the windows send the note back, and the promise holds over it
    assert!(out.contains("✔ \"No note is delivered unless every quote in it is in a page Jaynes downloaded\": proved"), "{out}");
    std::fs::write(s.role.join("role.toml"), format!("{WINDOWS}{WINDOWS_FALSE}")).expect("write");
    let (ok, out) = engine(&s, &["check", s.role.to_str().expect("path")]);
    assert!(!ok && out.contains("✘ \"No note is delivered before you answered at the windows\""), "{out}");
    std::fs::write(s.role.join("role.toml"), WINDOWS).expect("write");
    let text = "HTTP Caching\n\nA cache MUST NOT store a response when the no-store directive is present.\n";
    let fingerprint = Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>();
    let fetched = json!({"pages": [{"url": "https://example.org/caching", "read": true, "why": "", "sha256": "abc", "text_sha256": fingerprint, "asked": ["https://example.org/caching"]}],
                         "texts": {"https://example.org/caching": text}, "any_read": true});
    let note = json!({"reading": "Reading, not measured: it matters.", "claims": [{"url": "https://example.org/caching", "quote": "A cache MUST NOT store a response"}]});
    let reply = serve(&s, json!({"jaynes": 1, "id": "x", "call": "execute", "step": "windows", "request": "r", "round": 0, "attempt": 1,
        "given": [{"step": "note", "output": note}, {"step": "fetch", "output": fetched}], "previous": null, "answers": [], "key": null, "project": null, "source": null}));
    let output = &reply["output"];
    assert_eq!((output["judged"].as_u64(), output["quotes"][0]["found"].as_bool()), (Some(1), Some(true)), "{reply}");
    assert!(output["windows"].as_str().unwrap_or("").contains("HTTP Caching \u{2016} \u{bb}A cache MUST NOT store a response\u{ab} when the no-store"), "{reply}");
    let items = serve(&s, json!({"jaynes": 1, "id": "y", "call": "items", "step": "windows", "output": output}));
    let item = &items["items"][0];
    assert_eq!(item["detail"], "Reading, not measured: it matters.", "{items}");
    assert!(item["aspects"][0]["text"].as_str().unwrap_or("").contains("1. \"A cache MUST NOT store a response\""), "{items}");
}

/// A terminal for the person's answer: the engine takes an answer as the person's only from one.
const TTY: &str = r#"
import os, pty, sys
pid, fd = pty.fork()
if pid == 0:
    os.execvp(sys.argv[1], sys.argv[1:])
while True:
    try:
        data = os.read(fd, 65536)
    except OSError:
        break
    if not data:
        break
    os.write(1, data)
sys.exit(os.waitstatus_to_exitcode(os.waitpid(pid, 0)[1]))
"#;

/// The windows role run end to end, with an agent that is a shell script and pages written as fetch-pages writes
/// them (no network in a test): the agent's fetch step stands in for Jaynes's own here, which is what lets the test
/// run offline, and changes nothing of what the windows step and its stop do.
const SENT_BACK: &str = r#"# A test role: the windows of a note's quotes send the note back.

role = "sent-back"
version = "1.0.0"

[[step]]
name = "read"
reach = { project = "read" }
instructions = "Find sources for {{request}}."

[step.output]
links = { type = "array", items = "string" }

[[step]]
name = "fetch"
needs = ["read"]
reach = { project = "read" }
instructions = "Fetched pages of {{read.links}}."

[step.output]
pages = { type = "array", items = { type = "object", fields = { url = "string", read = "boolean", sha256 = "string", text_sha256 = "string" } } }
texts = "object"
any_read = "boolean"

[[step]]
name = "note"
needs = ["read", "fetch"]
reach = { project = "write", write = ["notes/**"] }
instructions = "Write the note."
checks = [{ use = "quotes-found", field = "claims", pages = "fetch" }]

[step.output]
reading = "string"
claims = { type = "array", items = { type = "object", fields = { url = "string", quote = "string" } } }

[[step]]
name = "windows"
by = "jaynes"
use = "quote-windows"
needs = ["note", "fetch"]
with = { claims = "note.claims", pages = "fetch", beside = "note.reading" }
stop = { ask = "Do the sources say what the note says?", verdicts = ["keep", "change"], redo = "note" }
show = { label = "The note's quotations in their pages", detail = "{{beside}}", aspects = [{ label = "Quotations", text = "{{windows}}" }] }
"#;

#[test]
fn sent_back_at_the_windows_the_note_is_written_again_and_the_windows_ask_again_with_it() {
    let s = scratch("sent-back");
    std::fs::write(s.role.join("role.toml"), SENT_BACK).expect("write");
    let base = s.home.parent().expect("base").to_path_buf();
    let project = base.join("project");
    std::fs::create_dir_all(project.join("notes")).expect("dir");
    std::fs::write(project.join("README.md"), "lab\n").expect("write");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
        assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success());
    }
    let text = "HTTP Caching\n\nA cache MUST NOT store a response when the no-store directive is present.\n\nSome guides claim that a cache MAY keep a no-store response for an hour. That is wrong.\n";
    let digest = Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>();
    let fetched = json!({"pages": [{"url": "https://example.org/caching", "read": true, "sha256": "page", "text_sha256": digest}],
                         "texts": {"https://example.org/caching": text}, "any_read": true});
    std::fs::write(base.join("fetch.json"), fetched.to_string()).expect("write");
    // the note quotes the denied claim first; sent back, it quotes what the page says
    let first = json!({"reading": "Reading, not measured: one.", "claims": [{"url": "https://example.org/caching", "quote": "a cache MAY keep a no-store response for an hour"}]});
    let second = json!({"reading": "Reading, not measured: two.", "claims": [{"url": "https://example.org/caching", "quote": "A cache MUST NOT store a response when the no-store directive is present."}]});
    std::fs::write(base.join("note-1.json"), first.to_string()).expect("write");
    std::fs::write(base.join("note-2.json"), second.to_string()).expect("write");
    let agent = base.join("agent.sh");
    std::fs::write(&agent, format!(r#"prompt=$(cat)
case "$prompt" in
  *"Find sources"*) echo '{{"links":["https://example.org/caching"]}}' > .jaynes/output.json ;;
  *"Fetched pages"*) cp {b}/fetch.json .jaynes/output.json ;;
  *"Write the note"*) n=$(cat {b}/notes 2>/dev/null || echo 0); n=$((n+1)); echo $n > {b}/notes; cp {b}/note-$n.json .jaynes/output.json ;;
esac
echo '{{"tokens":1}}'
"#, b = base.display())).expect("write");
    let agent_arg = format!("command:sh {}", agent.display());
    let (ok, out) = engine(&s, &["start", "--run", "back", "--role", s.role.to_str().expect("path"), "--project", project.to_str().expect("path"),
                                 "--request", "what no-store asks", "--agent", &agent_arg]);
    assert!(ok && out.contains("Do the sources say what the note says?"), "{out}");
    let log = |s: &Scratch| -> Vec<Value> {
        let (_, out) = engine(s, &["log", "back"]);
        out.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    };
    let asked: Vec<Value> = log(&s).into_iter().filter(|e| e["type"] == "QuestionOpened").map(|e| e["question"].clone()).collect();
    assert_eq!(asked.len(), 1, "{asked:?}");
    let q = asked[0]["id"].as_str().expect("id").to_string();
    // the person, at a terminal, sends the note back from the windows
    let said = Command::new("python3").args(["-c", TTY, BIN, "answer", "back", &q, "windows=change:S1 quotes that claim only to deny it", "--send"])
        .env("JAYNES_ENGINE_HOME", &s.home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("JAYNES_AGENT", &agent_arg)
        .output().expect("answer");
    let said = String::from_utf8_lossy(&said.stdout).to_string();
    let events = log(&s);
    let ran = |step: &str| events.iter().filter(|e| e["type"] == "AttemptStarted" && e["step"] == step).count();
    assert_eq!((ran("read"), ran("fetch"), ran("note")), (1, 1, 2), "only the note is written again: {said}");
    assert_eq!(ran("windows"), 2, "the windows are made again from the new note: {said}");
    let again: Vec<Value> = events.iter().filter(|e| e["type"] == "QuestionOpened").map(|e| e["question"].clone()).collect();
    assert_eq!(again.len(), 2, "{said}");
    assert_eq!(again[1]["step"], "windows");
    let shown = again[1]["items"].to_string();
    assert!(!shown.is_empty());
    // the run asks again, at the windows, about what the new note made
    let (_, show) = engine(&s, &["show", "back"]);
    assert!(show.contains("? q2-windows at windows") && show.contains("windows          role     round 1  done"), "{show}");
    // and the note that came back was told why
    let notes: Vec<String> = events.iter().filter(|e| e["type"] == "StepInvalidated" && e["step"] == "note").map(|e| e.to_string()).collect();
    assert!(notes.iter().any(|e| e.contains("S1 quotes that claim only to deny it")), "{notes:?}");
}

