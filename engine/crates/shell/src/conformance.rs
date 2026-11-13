//! The conformance suite: what makes a role correct is that it answers these calls as the protocol says, not
//! which library it was written with.

use crate::protocol::{Body, Call, FEATURES};
use crate::role::RoleProgram;
use jaynes_core::manifest::Executor;
use serde_json::json;

pub struct Finding {
    pub case: String,
    pub ok: bool,
    pub detail: String,
}

fn found(case: &str, ok: bool, detail: impl Into<String>) -> Finding {
    Finding { case: case.to_string(), ok, detail: detail.into() }
}

pub fn run(role: &RoleProgram) -> Vec<Finding> {
    let mut out = Vec::new();
    let hello = role.call(Call::Hello { engine: jaynes_core::ENGINE.into(), features: FEATURES.iter().map(|s| s.to_string()).collect(), extensions: Default::default() });
    let name = match &hello {
        Ok(Body::Hello { role: name, lifetime, required, .. }) => {
            out.push(found("hello names the role", !name.trim().is_empty(), name.clone()));
            out.push(found("hello says the lifetime is one process per call", lifetime == "call", lifetime.clone()));
            let unknown: Vec<&String> = required.iter().filter(|r| !FEATURES.contains(&r.as_str())).collect();
            out.push(found("hello requires no feature the engine lacks", unknown.is_empty(), format!("{unknown:?}")));
            name.clone()
        }
        Ok(other) => {
            out.push(found("hello is answered with hello", false, format!("{other:?}")));
            return out;
        }
        Err(e) => {
            out.push(found("hello is answered", false, e.clone()));
            return out;
        }
    };
    let manifest = match role.call(Call::Describe {}) {
        Ok(Body::Manifest { manifest }) => manifest,
        Ok(other) => {
            out.push(found("describe is answered with a manifest", false, format!("{other:?}")));
            return out;
        }
        Err(e) => {
            out.push(found("describe is answered", false, e));
            return out;
        }
    };
    out.push(found("the manifest names the role hello named", manifest.role == name, manifest.role.clone()));
    match manifest.validate() {
        Ok(()) => out.push(found("the manifest keeps every rule of a role", true, "")),
        Err(errors) => out.push(found("the manifest keeps every rule of a role", false, errors.join("; "))),
    }
    let again = role.call(Call::Describe {});
    out.push(found("describe gives the same manifest twice", matches!(&again, Ok(Body::Manifest { manifest: m }) if *m == manifest), ""));
    for s in &manifest.steps {
        if s.executor == Executor::Agent {
            let reply = role.call(Call::Prompt {
                step: s.name.clone(),
                request: "a request".into(),
                round: 0,
                attempt: 1,
                given: Vec::new(),
                previous: None,
                notes: Vec::new(),
                failure: Vec::new(),
                checks: Vec::new(),
            });
            let ok = matches!(&reply, Ok(Body::Prompt { text, schema }) if !text.trim().is_empty() && schema.is_object());
            out.push(found(&format!("{}: prompt gives instructions and an output schema", s.name), ok, short(&reply)));
        }
        if s.executor != Executor::Person {
            let reply = role.call(Call::Items { step: s.name.clone(), output: json!({}) });
            let ok = match &reply {
                Ok(Body::Items { items }) => {
                    let mut ids: Vec<&String> = items.iter().map(|i| &i.id).collect();
                    ids.sort();
                    ids.dedup();
                    ids.len() == items.len() && items.iter().all(|i| !i.label.trim().is_empty())
                }
                _ => false,
            };
            out.push(found(&format!("{}: items lists distinct items with labels", s.name), ok, short(&reply)));
            for c in &s.checks {
                let reply = role.call(Call::Check {
                    step: s.name.clone(),
                    check: c.name.clone(),
                    request: "a request".into(),
                    output: json!({}),
                    given: Vec::new(),
                    project: None,
                    source: None,
                });
                let ok = matches!(reply, Ok(Body::Verdict { .. }));
                out.push(found(&format!("{}: check {} gives a verdict, even on an empty output", s.name, c.name), ok, short(&reply)));
            }
        }
    }
    let unknown = role.call(Call::Items { step: "no-such-step".into(), output: json!({}) });
    out.push(found("a call for a step the role lacks is an error, not a crash", unknown.is_err() && unknown.as_ref().err().is_some_and(|e| e.contains("could not answer")), short(&unknown)));
    out
}

fn short(r: &Result<Body, String>) -> String {
    let s = format!("{r:?}");
    s.chars().take(200).collect()
}
