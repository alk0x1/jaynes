//! What the page's key script does in a browser, run in Node with what it touches stood in for: the key a link
//! brings is kept only when the page says it is its own, a key kept stays when a link brings another, the key goes
//! with the page's own POSTs and nothing else, and the address loses it at once.

use std::process::Command;

/// The key script as the page serves it, with a stand-in browser around it, after `hash` and with `stored` kept.
/// What comes back: the key kept, the address after, whether the page said it has no key, the keys each request
/// carried, as one line of JSON.
fn run(hash: &str, stored: &str) -> serde_json::Value {
    let script = jaynes_shell::page::KEY_SCRIPT;
    let harness = format!(
        r##"
const kept = new Map({stored});
const store = {{ getItem: k => kept.has(k) ? kept.get(k) : null, setItem: (k, v) => kept.set(k, v) }};
const sent = [];
let address = "/q/r1/q1" + {hash};
let notice = false;
globalThis.window = globalThis;
globalThis.localStorage = store;
globalThis.sessionStorage = store;
globalThis.location = {{ get hash() {{ const i = address.indexOf("#"); return i < 0 ? "" : address.slice(i); }}, pathname: "/q/r1/q1", search: "" }};
globalThis.history = {{ replaceState: (_s, _t, url) => {{ address = url; }} }};
globalThis.document = {{ getElementById: id => id === "nokey" ? {{ replaceWith: () => {{ notice = true; }} }} : null, createElement: () => ({{}}) }};
globalThis.fetch = async (url, opts = {{}}) => {{
  const key = (opts.headers || {{}})["X-Jaynes-Key"] || "";
  sent.push([url, opts.method || "GET", key]);
  return {{ json: async () => ({{ ok: true, person: key === "good" }}) }};
}};
{script}
(async () => {{
  await window.jaynesKey;
  await fetch("/api/answer", {{ method: "POST", headers: {{ "Content-Type": "application/json" }}, body: "{{}}" }});
  await fetch("/api/version");
  console.log(JSON.stringify({{ kept: kept.get("jaynes_key") || null, address, notice, sent }}));
}})();
"##,
        stored = if stored.is_empty() { "[]".to_string() } else { format!("[[\"jaynes_key\", {stored:?}]]") },
        hash = serde_json::to_string(hash).expect("hash"),
    );
    let out = Command::new("node").arg("-e").arg(&harness).output().expect("node, which these tests need");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

#[test]
fn a_key_a_link_brings_is_kept_only_when_the_page_says_it_is_its_own() {
    let r = run("#key=good", "");
    assert_eq!(r["kept"], "good", "{r}");
    assert_eq!(r["address"], "/q/r1/q1", "the address loses the key at once: {r}");
    assert_eq!(r["notice"], false, "{r}");
    // the page's own POST carries it, a GET does not
    let sent = r["sent"].as_array().expect("sent");
    assert!(sent.iter().any(|s| s[0] == "/api/answer" && s[2] == "good"), "{r}");
    assert!(sent.iter().any(|s| s[0] == "/api/version" && s[2] == ""), "{r}");
}

#[test]
fn a_link_with_another_key_never_replaces_the_person_s() {
    // the person's key is kept; a link from anywhere brings another
    let r = run("#key=bad", "good");
    assert_eq!(r["kept"], "good", "{r}");
    assert_eq!(r["notice"], false, "{r}");
    assert_eq!(r["address"], "/q/r1/q1", "{r}");
    let sent = r["sent"].as_array().expect("sent");
    assert!(sent.iter().any(|s| s[0] == "/api/answer" && s[2] == "good"), "{r}");
}

#[test]
fn without_a_key_the_page_says_so_and_keeps_none() {
    let r = run("#key=bad", "");
    assert_eq!(r["kept"], serde_json::Value::Null, "{r}");
    assert_eq!(r["notice"], true, "{r}");
    let r = run("", "");
    assert_eq!(r["notice"], true, "{r}");
    let sent = r["sent"].as_array().expect("sent");
    assert!(sent.iter().filter(|s| s[0] == "/api/answer").all(|s| s[2] == ""), "{r}");
}
