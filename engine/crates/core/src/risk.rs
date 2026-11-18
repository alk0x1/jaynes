//! How risky a run is, judged by rules in code from what the change touches. Never by an agent.

use crate::glob;
use crate::manifest::{Manifest, Target};
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
/// git attributes, submodules, git-lfs settings and hooks (the person's own git obeys them when the delivery lands), direnv, interpreters and package scripts, test and build entry points, dev
/// containers, CI. Ported from the Python engine (`jaynes.project.FLOOR`).
pub const FLOOR: &[&str] = &[
    "**/.claude/**", "**/.codex/**", "**/.cursor/**", "**/.gemini/**", "**/.vscode/**", "**/.idea/**",
    "**/.zed/**", "**/.mcp.json", "**/CLAUDE.md", "**/AGENTS.md", "**/.cursorrules",
    "**/.git*", "**/.gitattributes", "**/.gitmodules", "**/.lfsconfig", "**/.husky/**", "**/.pre-commit-config.yaml", "**/.envrc",
    "**/.venv/**", "**/venv/**", "**/package.json", "**/.npmrc", "**/.yarnrc*", "**/pyproject.toml",
    "**/setup.py", "**/setup.cfg", "**/sitecustomize.py", "**/usercustomize.py", "**/conftest.py",
    "**/Makefile", "**/GNUmakefile", "**/.devcontainer/**", ".gitlab-ci.yml", ".circleci/**", ".buildkite/**",
    "Jenkinsfile", "azure-pipelines.yml", ".travis.yml", "**/Cargo.toml", "**/build.rs",
];

/// One thing a step changed in its target, as Jaynes measured it from the step's copy, never as the agent reported
/// it. On a repository: a file, its lines added and removed, and whether it is on the floor ([`FLOOR`]).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(from = "ChangedOnRecord")]
pub struct Changed {
    /// What changed, in the target's own words: a path, an element, an action.
    pub what: String,
    /// How much it changed, in the target's unit: lines on a repository.
    pub size: u64,
    /// It runs or is obeyed later outside any box, or cannot be taken back: it alone makes the change risky.
    pub floor: bool,
}

impl Changed {
    /// A file of a repository or a folder that changed by `lines`, on the floor when [`FLOOR`] names it.
    pub fn file(path: String, lines: u64) -> Changed {
        let floor = !floor([path.as_str()]).is_empty();
        Changed { what: path, size: lines, floor }
    }
}

/// A change as the record holds it: as above, or as engines before targets wrote it (a path and its lines, the floor
/// found from the path), so every recorded run reads as it did.
#[derive(Deserialize, schemars::JsonSchema)]
struct ChangedOnRecord {
    #[serde(alias = "path")]
    what: String,
    #[serde(alias = "lines")]
    size: u64,
    floor: Option<bool>,
}

impl From<ChangedOnRecord> for Changed {
    fn from(r: ChangedOnRecord) -> Changed {
        let floor = r.floor.unwrap_or_else(|| !floor([r.what.as_str()]).is_empty());
        Changed { what: r.what, size: r.size, floor }
    }
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

/// The risk of a run's change so far on its target, with the reason for each rule that raised it. The same rules for
/// every target: what is on the floor, the size of the change, and how many things it touches. A measure may mark
/// more as on the floor than the core knows, and never less: on a repository, a path on [`FLOOR`] is on it whatever
/// the measure said.
pub fn of_change(target: Target, changed: &[Changed]) -> (Risk, Vec<String>) {
    let mut reasons = Vec::new();
    let on_floor = |c: &Changed| {
        c.floor
            || match target {
                Target::Repository => !floor([c.what.as_str()]).is_empty(),
            }
    };
    let floor: Vec<&str> = changed.iter().filter(|c| on_floor(c)).map(|c| c.what.as_str()).collect();
    if !floor.is_empty() {
        reasons.push(format!("the change touches files that run later outside any box: {}", floor.join(", ")));
    }
    let lines: u64 = changed.iter().map(|c| c.size).sum();
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
        Changed::file(path.to_string(), lines)
    }

    #[test]
    fn a_change_recorded_by_an_older_engine_reads_with_its_floor_found_again() {
        let old: Vec<Changed> = serde_json::from_str(r#"[{"path":".github/workflows/ci.yml","lines":2},{"path":"src/a.py","lines":3}]"#).expect("reads");
        assert_eq!(old, vec![c(".github/workflows/ci.yml", 2), c("src/a.py", 3)]);
        assert!(old[0].floor && !old[1].floor);
        let again: Vec<Changed> = serde_json::from_str(&serde_json::to_string(&old).expect("json")).expect("reads");
        assert_eq!(again, old);
    }

    #[test]
    fn a_measure_may_put_more_on_the_floor_and_never_take_off_what_the_core_knows() {
        let marked = Changed { what: "src/a.py".into(), size: 1, floor: true };
        assert_eq!(of_change(Target::Repository, &[marked]).0, Risk::High);
        let unknown = Changed { what: "a form sent".into(), size: 1, floor: true };
        assert_eq!(of_change(Target::Repository, &[unknown]).0, Risk::High, "a name the core does not know, marked by the measure");
        let unmarked = Changed { what: ".github/workflows/ci.yml".into(), size: 1, floor: false };
        assert_eq!(of_change(Target::Repository, &[unmarked]).0, Risk::High);
        let plain = Changed { what: "src/a.py".into(), size: 1, floor: false };
        assert_eq!(of_change(Target::Repository, &[plain]).0, Risk::Low);
    }

    #[test]
    fn small_change_is_low() {
        assert_eq!(of_change(Target::Repository, &[c("src/a.py", 4)]).0, Risk::Low);
    }

    #[test]
    fn floor_lines_and_files_raise_it() {
        assert_eq!(of_change(Target::Repository, &[c(".github/workflows/ci.yml", 1)]).0, Risk::High);
        assert_eq!(of_change(Target::Repository, &[c("PyProject.toml", 1)]).0, Risk::High);
        // what the person's own git obeys when the delivery lands in their repository, in any folder
        for p in ["docs/.gitattributes", ".gitmodules", "vendor/.lfsconfig", ".lfsconfig"] {
            assert_eq!(of_change(Target::Repository, &[c(p, 1)]).0, Risk::High, "{p}");
        }
        assert_eq!(of_change(Target::Repository, &[c("src/a.py", 31)]).0, Risk::High);
        let many: Vec<Changed> = (0..6).map(|i| c(&format!("src/{i}.py"), 1)).collect();
        assert_eq!(of_change(Target::Repository, &many).0, Risk::High);
    }
}
