//! The core of the Jaynes engine.
//!
//! A pure function: [`decide`] takes a run's state and one [`Input`] and returns the [`Event`]s to record and the
//! [`Command`]s for the shell to carry out. It reads no disk, network, process or clock. The run's state is only
//! ever the fold of its events ([`Run::fold`]), so reading the record again gives the same state.
//!
//! What the core promises is in `docs/guarantees.md`; each promise has a test of the same number in
//! `tests/guarantees.rs`.

pub mod attention;
pub mod chain;
pub mod decide;
pub mod digest;
pub mod event;
pub mod glob;
pub mod io;
pub mod manifest;
pub mod question;
pub mod risk;
pub mod run;

pub use decide::{decide, effects, Decided, Refusal};
pub use digest::Digest;
pub use event::Event;
pub use io::{AgentTurn, AttemptOutcome, CheckResult, CheckVerdict, Command, Input};
pub use manifest::Manifest;
pub use run::Run;

/// The engine's version, recorded in every run it starts.
pub const ENGINE: &str = env!("CARGO_PKG_VERSION");
/// The major version of the protocol between the engine and a role.
pub const PROTOCOL: u32 = 1;
/// The version of the record's format.
pub const FORMAT: u32 = 1;
/// The chance a stop is made sampled although its true error rate is at or above its budget.
pub const DELTA: f64 = 0.05;
