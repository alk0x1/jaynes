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
    assert_eq!(invented["reasons"], json!(["\"A cache MAY store anything it likes, forever.\": these words are not in the text Jaynes downloaded from https://example.org/caching"]));
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
