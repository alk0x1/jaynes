//! The shell around the core: the record in SQLite, the role's process, the agent program, the page. The `jaynes`
//! command and the Jaynes app both use it.

pub mod agent;
pub mod conformance;
pub mod drive;
pub mod index;
pub mod install;
pub mod listing;
pub mod mcp;
pub mod page;
pub mod permit;
pub mod person;
pub mod project;
pub mod protocol;
pub mod role;
pub mod stop;
pub mod store;
pub mod summary;
pub mod toml_role;
pub mod understand;

/// Where the engine keeps its record, roles and runs: JAYNES_ENGINE_HOME, else ~/.jaynes/engine.
pub fn home() -> std::path::PathBuf {
    std::env::var("JAYNES_ENGINE_HOME").map(std::path::PathBuf::from).unwrap_or_else(|_| {
        std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".jaynes/engine")
    })
}
