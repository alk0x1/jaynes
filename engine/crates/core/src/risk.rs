//! How risky a run is, judged by rules in code from what the change touches. Never by an agent.

use crate::glob;
use crate::manifest::Manifest;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// A small change that can be undone: the short path.
    #[default]
    Low,
    /// Anything else: the full path.
    High,
}

impl Risk {
    pub fn word(self) -> &'static str {
        match self {
            Risk::Low => "low",
            Risk::High => "high",
        }
    }
}

/// Changed lines, added and removed, past which a change is no longer small. The Python engine's small-change
/// role used the same number.
pub const SMALL_LINES: u64 = 30;
/// Files past which a change is no longer small.
pub const SMALL_FILES: usize = 5;

/// Paths that trusted tools run or obey later, outside any box: agents' and editors' settings and instructions,
/// git attributes and hooks, direnv, interpreters and package scripts, test and build entry points, dev
/// containers, CI. Ported from the Python engine (`jaynes.project.FLOOR`).
pub const FLOOR: &[&str] = &[
    "**/.claude/**", "**/.codex/**", "**/.cursor/**", "**/.gemini/**", "**/.vscode/**", "**/.idea/**",
    "**/.zed/**", "**/.mcp.json", "**/CLAUDE.md", "**/AGENTS.md", "**/.cursorrules",
    "**/.git*", "**/.husky/**", "**/.pre-commit-config.yaml", "**/.envrc",
    "**/.venv/**", "**/venv/**", "**/package.json", "**/.npmrc", "**/.yarnrc*", "**/pyproject.toml",
    "**/setup.py", "**/setup.cfg", "**/sitecustomize.py", "**/usercustomize.py", "**/conftest.py",
    "**/Makefile", "**/GNUmakefile", "**/.devcontainer/**", ".gitlab-ci.yml", ".circleci/**", ".buildkite/**",
    "Jenkinsfile", "azure-pipelines.yml", ".travis.yml", "**/Cargo.toml", "**/build.rs",
];

/// A file a step changed, as Jaynes measured it from the step's copy, never as the agent reported it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Changed {
    pub path: String,
    /// Lines added and removed.
    pub lines: u64,
}

/// The paths among `paths` that run later outside any box, in any case, since macOS ignores it.
pub fn floor<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    paths
        .into_iter()
        .filter(|p| FLOOR.iter().any(|g| glob::matches(&p.to_lowercase(), &g.to_lowercase())))
        .map(str::to_string)
        .collect()
}

/// The risk a role's short path has before any change: a step that cannot be undone makes it high.
pub fn of_role(m: &Manifest) -> (Risk, Vec<String>) {
    let undoable: Vec<String> = m
        .path(Risk::Low)
        .into_iter()
        .filter(|&i| m.steps[i].effect.irreversible())
        .map(|i| m.steps[i].name.clone())
        .collect();
    if undoable.is_empty() {
        (Risk::Low, Vec::new())
    } else {
        (Risk::High, vec![format!("the short path has a step that cannot be undone: {}", undoable.join(", "))])
    }
}

/// The risk of a run's change so far, with the reason for each rule that raised it.
pub fn of_change(changed: &[Changed]) -> (Risk, Vec<String>) {
    let mut reasons = Vec::new();
    let floor = floor(changed.iter().map(|c| c.path.as_str()));
    if !floor.is_empty() {
        reasons.push(format!("the change touches files that run later outside any box: {}", floor.join(", ")));
    }
    let lines: u64 = changed.iter().map(|c| c.lines).sum();
    if lines > SMALL_LINES {
        reasons.push(format!("the change has {lines} changed lines, more than {SMALL_LINES}"));
    }
    if changed.len() > SMALL_FILES {
        reasons.push(format!("the change touches {} files, more than {SMALL_FILES}", changed.len()));
    }
    (if reasons.is_empty() { Risk::Low } else { Risk::High }, reasons)
}

/// A run's ceiling of time and tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Ceiling {
    pub seconds: u64,
    /// Tokens sent and received, without those read back from the cache.
    pub tokens: u64,
}

/// The ceilings a run starts with, one per risk, said before the run starts. Working values until the owner
/// sets his own (an open question of the brief).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Ceilings {
    pub low: Ceiling,
    pub high: Ceiling,
}

impl Default for Ceilings {
    fn default() -> Self {
        Ceilings {
            low: Ceiling { seconds: 20 * 60, tokens: 2_000_000 },
            high: Ceiling { seconds: 2 * 3600, tokens: 10_000_000 },
        }
    }
}

impl Ceilings {
    pub fn at(&self, r: Risk) -> Ceiling {
        match r {
            Risk::Low => self.low,
            Risk::High => self.high,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(path: &str, lines: u64) -> Changed {
        Changed { path: path.to_string(), lines }
    }

    #[test]
    fn small_change_is_low() {
        assert_eq!(of_change(&[c("src/a.py", 4)]).0, Risk::Low);
    }

    #[test]
    fn floor_lines_and_files_raise_it() {
        assert_eq!(of_change(&[c(".github/workflows/ci.yml", 1)]).0, Risk::High);
        assert_eq!(of_change(&[c("PyProject.toml", 1)]).0, Risk::High);
        assert_eq!(of_change(&[c("src/a.py", 31)]).0, Risk::High);
        let many: Vec<Changed> = (0..6).map(|i| c(&format!("src/{i}.py"), 1)).collect();
        assert_eq!(of_change(&many).0, Risk::High);
    }
}
