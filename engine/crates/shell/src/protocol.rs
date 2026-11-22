//! The protocol between the engine and a role, version 1.
//!
//! A role is a program. For each call the engine starts it, writes one request as a line of JSON on its standard
//! input, and reads one reply as a line of JSON from its standard output; then the program exits (`lifetime:
//! "call"`, the only lifetime of version 1). Every message carries the protocol's major version in `jaynes` and
//! the request's `id`, which the reply repeats. A field a side does not know is ignored; a feature is used only
//! when both sides list it; a `required` feature the engine does not know refuses the role.
//!
//! The engine owns the order of the steps. The role says what its steps are (`describe`), writes the agent's
//! instructions for a step (`prompt`), does its own steps (`execute`), judges outputs (`check`) and lists what a
//! person is shown of an output (`items`). It never decides what runs next, and never writes what an answer will
//! do. The schema of every message is `protocol/role-protocol-1.schema.json`, generated from these types.

use jaynes_core::manifest::{CheckDecl, Manifest};
use jaynes_core::question::{Answer, VerdictKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const MAJOR: u32 = jaynes_core::PROTOCOL;

/// The features this engine knows. Version 1 has none beyond the base.
pub const FEATURES: &[&str] = &[];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Request {
    pub jaynes: u32,
    pub id: String,
    #[serde(flatten)]
    pub call: Call,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "call", rename_all = "snake_case")]
pub enum Call {
    Hello {
        engine: String,
        features: Vec<String>,
        /// Room to grow: anything a later version adds before it has a field of its own.
        #[serde(default)]
        extensions: Map<String, Value>,
    },
    Describe {},
    /// Write the instructions for an agent's attempt at a step.
    Prompt {
        step: String,
        request: String,
        round: u32,
        attempt: u32,
        given: Vec<Given>,
        previous: Option<Value>,
        notes: Vec<String>,
        failure: Vec<String>,
        /// The step's open checks, which the agent knows.
        checks: Vec<CheckDecl>,
    },
    /// Do a step of the role's own.
    Execute {
        step: String,
        request: String,
        round: u32,
        attempt: u32,
        given: Vec<Given>,
        previous: Option<Value>,
        answers: Vec<Answer>,
        /// For a step that cannot be undone: pass it to the service that acts, so a repeat counts once.
        key: Option<String>,
        /// The run's copy of the project, for a step that reaches it.
        project: Option<String>,
        /// The person's own folder the run started from, never to be written.
        source: Option<String>,
        /// The person opened this project's checks to every local port (`jaynes project open-ports`), as the run's
        /// setup recorded it when it started: the role's box then lets its commands reach any local TCP port. Left
        /// out when not.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        any_port: bool,
        /// The person let this project's tests start their own test servers (`jaynes project test-servers`), as the
        /// run's setup recorded it or the person chose since: macOS's box then lets its commands serve on a port the
        /// system picks, and reach the window of ports the system gives next. Left out when not.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        test_servers: bool,
        /// With `test_servers`, the ports of the usual databases and caches that macOS's box holds while the commands
        /// run which the person let this project's tests use (`jaynes project test-servers DIR PORT`): never held, and
        /// bound and reached as a step's own. Left out when none.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        freed_ports: Vec<u16>,
    },
    Check {
        step: String,
        check: String,
        request: String,
        output: Value,
        given: Vec<Given>,
        /// For a step that reaches the project: the files of the project as Jaynes measured them when the attempt
        /// ended, and as the delivery carries them, laid out in a folder of their own beside the run's copy, with no
        /// repository and with the copy's installed dependencies linked in; never the run's copy itself. What git
        /// ignores is not there. The folder goes when the check is over.
        project: Option<String>,
        source: Option<String>,
        /// The project as the run began, for a check that compares with it: the engine's repository beside the
        /// run's copy, out of every agent's reach, and the commit the run began from. Given by the engine from the
        /// run's copy, never read from where `project` is, the state judged laid out apart. Left out when not known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start: Option<Start>,
        /// The person opened this project's checks to every local port (`jaynes project open-ports`), as the run's
        /// setup recorded it when it started: the role's box then lets its commands reach any local TCP port. Left
        /// out when not.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        any_port: bool,
        /// The person let this project's tests start their own test servers (`jaynes project test-servers`), as the
        /// run's setup recorded it or the person chose since: macOS's box then lets its commands serve on a port the
        /// system picks, and reach the window of ports the system gives next. Left out when not.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        test_servers: bool,
        /// With `test_servers`, the ports of the usual databases and caches that macOS's box holds while the commands
        /// run which the person let this project's tests use (`jaynes project test-servers DIR PORT`): never held, and
        /// bound and reached as a step's own. Left out when none.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        freed_ports: Vec<u16>,
    },
    /// What the person is shown of an output: one item per thing they may judge.
    Items { step: String, output: Value },
    /// The output with only the items the person chose, each as it was: a stop that chooses asks for it when the
    /// person chose without a note, so no agent works. The reply is a `result`.
    Narrow { step: String, output: Value, keep: Vec<String> },
}

/// Where the project stood when the run began: a repository and a commit of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Start {
    pub git: String,
    pub commit: String,
}

/// An earlier step's output given to a call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Given {
    pub step: String,
    pub output: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Reply {
    pub jaynes: u32,
    pub id: String,
    #[serde(flatten)]
    pub body: Body,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Body {
    Hello {
        role: String,
        lifetime: String,
        #[serde(default)]
        features: Vec<String>,
        /// Features without which the role cannot run.
        #[serde(default)]
        required: Vec<String>,
        #[serde(default)]
        extensions: Map<String, Value>,
    },
    Manifest { manifest: Manifest },
    Prompt {
        text: String,
        /// The JSON Schema the output must follow, shown to the agent.
        schema: Value,
    },
    /// The output of a role's own step.
    Result { output: Value },
    /// A role's own step asks the person something before it can give its output.
    NeedsInput { items: Vec<ShownItem>, offered: Vec<VerdictKind>, #[serde(default)] about: String },
    /// The step cannot be done, for a reason of the role's.
    Refused { reason: String },
    Verdict { verdict: jaynes_core::CheckVerdict },
    Items { items: Vec<ShownItem> },
    /// The role could not answer the call at all: the engine counts it as its own fault, never the agent's.
    Error { message: String },
}

/// An item as the role lists it. The engine computes its fingerprint from `content`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ShownItem {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub content: Value,
    /// A few plain sentences shown under the label.
    #[serde(default)]
    pub detail: String,
    /// Pictures of the item: files in the step's copy of the project, by path from its root. Jaynes keeps a copy of
    /// each, by its fingerprint, so what the person sees never changes under them.
    #[serde(default)]
    pub pictures: Vec<String>,
    /// The parts of the item the person judges apart, each with its label, such as an option's palette and fonts.
    /// Shown, and read with the person's words; the engine judges the item as a whole.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aspects: Vec<Aspect>,
    /// The words that named this option before it was made, copied from the request or the brief, and where they
    /// were: a stop with `holds_unless` does not hold the run for the one option that has them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub named: Option<jaynes_core::question::Named>,
}

/// One part of an item, shown apart: its label and what it is, in plain words.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Aspect {
    pub label: String,
    pub text: String,
}

impl ShownItem {
    /// The item as the core knows it. Its fingerprint covers its content and its pictures, so an answer is bound to
    /// what was shown and a kept item keeps its pictures.
    pub fn item_with(&self, pictures: &[String]) -> jaynes_core::question::Item {
        let digest = if pictures.is_empty() {
            jaynes_core::Digest::of_json(&self.content)
        } else {
            jaynes_core::Digest::of_json(&serde_json::json!({"content": self.content, "pictures": pictures}))
        };
        jaynes_core::question::Item { named: self.named.clone(), id: self.id.clone(), label: self.label.clone(), digest }
    }
}

/// The schema of every message, as one document.
pub fn schema() -> Value {
    #[derive(JsonSchema)]
    #[allow(dead_code)]
    struct RoleProtocol1 {
        request: Request,
        reply: Reply,
    }
    let mut s = serde_json::to_value(schemars::schema_for!(RoleProtocol1)).unwrap_or_default();
    if let Some(o) = s.as_object_mut() {
        o.insert("title".into(), Value::String("Jaynes role protocol, version 1".into()));
    }
    s
}
