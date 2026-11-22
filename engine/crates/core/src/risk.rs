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
/// git attributes, submodules, git-lfs settings and hooks (the person's own git obeys them when the delivery lands), the
/// git hook managers' own files (husky, pre-commit, lefthook), direnv, interpreters and package scripts, test and build entry points, dev
/// containers, CI. Ported from the Python engine (`jaynes.project.FLOOR`). A merge that brings one in waits for the
/// person (G19), but a package manifest that changes neither what runs nor where code comes from ([`MANIFESTS`]). A
/// dependency that comes in only through a lock file, without being in the manifest, does not hold the merge.
pub const FLOOR: &[&str] = &[
    "**/.claude/**", "**/.codex/**", "**/.cursor/**", "**/.gemini/**", "**/.vscode/**", "**/.idea/**",
    "**/.zed/**", "**/.mcp.json", "**/CLAUDE.md", "**/AGENTS.md", "**/.cursorrules",
    "**/.git*", "**/.gitattributes", "**/.gitmodules", "**/.lfsconfig", "**/.husky/**", "**/.pre-commit-config.yaml", "**/.envrc",
    "**/lefthook.yml", "**/lefthook.yaml", "**/.lefthook.yml", "**/.lefthook.yaml", "**/lefthook-local.yml", "**/lefthook-local.yaml",
    "**/.venv/**", "**/venv/**", "**/package.json", "**/.npmrc", "**/.yarnrc*", "**/pyproject.toml",
    "**/setup.py", "**/setup.cfg", "**/sitecustomize.py", "**/usercustomize.py", "**/conftest.py",
    "**/Makefile", "**/GNUmakefile", "**/.devcontainer/**", ".gitlab-ci.yml", ".circleci/**", ".buildkite/**",
    "Jenkinsfile", "azure-pipelines.yml", ".travis.yml", "**/Cargo.toml", "**/build.rs",
];

/// One thing a step changed in its target, as Jaynes measured it from the step's copy, never as the agent reported
/// it. On a repository: a file, its lines added and removed, and whether it is on the floor ([`FLOOR`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(from = "ChangedOnRecord")]
pub struct Changed {
    /// What changed, in the target's own words: a path, an element, an action.
    pub what: String,
    /// How much it changed, in the target's unit: lines on a repository.
    pub size: u64,
    /// Of `size`, how much went away: the lines removed, on a repository. The rest were added.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub removed: u64,
    /// It runs or is obeyed later outside any box, or cannot be taken back: it alone makes the change risky.
    pub floor: bool,
    /// The fingerprint of what the change leaves (a file's content), when the measure knows it: a question about the
    /// change is bound to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// The measure's word on a package manifest ([`MANIFESTS`]): whether its change holds a merge. Read only for a
    /// manifest, and only to narrow: every other thing on the floor holds a merge whatever this says (G19).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holds: Option<bool>,
    /// What changes in it, in a line the person reads where a merge waits for them.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
}

impl Changed {
    /// A file of a repository or a folder that changed by `lines`, on the floor when [`FLOOR`] names it.
    pub fn file(path: String, lines: u64) -> Changed {
        let floor = !floor([path.as_str()]).is_empty();
        Changed { what: path, size: lines, floor, ..Default::default() }
    }
}

/// How each kind of file on the floor runs, in plain words, as the stop before a merge says it after the file's name:
/// the first that matches tells (CI before git's own files, since `.github/` starts as they do).
const RUNS_HOW: &[(&[&str], &str)] = &[
    (&["**/conftest.py"], "runs on its own whenever the tests run"),
    (&["**/sitecustomize.py", "**/usercustomize.py", "**/.venv/**", "**/venv/**"], "Python runs it on its own, before your code"),
    (
        &["**/setup.py", "**/setup.cfg", "**/pyproject.toml", "**/package.json", "**/.npmrc", "**/.yarnrc*", "**/Cargo.toml", "**/build.rs"],
        "runs when the project is installed or built",
    ),
    (&["**/Makefile", "**/GNUmakefile"], "runs when you run make"),
    (&[".github/**", ".gitlab-ci.yml", ".circleci/**", ".buildkite/**", "Jenkinsfile", "azure-pipelines.yml", ".travis.yml"], "runs in your CI"),
    (
        &["**/.git*", "**/.lfsconfig", "**/.husky/**", "**/.pre-commit-config.yaml", "**/lefthook*", "**/.lefthook*"],
        "git runs or obeys it on your machine",
    ),
    (&["**/.envrc"], "runs when you enter the folder"),
    (
        &["**/.claude/**", "**/.codex/**", "**/.cursor/**", "**/.gemini/**", "**/.mcp.json", "**/CLAUDE.md", "**/AGENTS.md", "**/.cursorrules"],
        "your agents read it as instructions",
    ),
    (&["**/.vscode/**", "**/.idea/**", "**/.zed/**"], "your editor obeys it"),
    (&["**/.devcontainer/**"], "sets up your dev container"),
];

/// How a file on the floor runs later, in plain words, without the "it" a sentence would start with ("runs on its own
/// whenever the tests run"); a kind the table does not name only "runs later outside any box". In any case, since
/// macOS ignores it.
pub fn runs_how(path: &str) -> &'static str {
    let p = path.to_lowercase();
    RUNS_HOW
        .iter()
        .find(|(globs, _)| globs.iter().any(|g| glob::matches(&p, &g.to_lowercase())))
        .map(|(_, how)| *how)
        .unwrap_or("runs later outside any box")
}

/// The package manifests on the floor: a change to one holds a merge only when it changes what runs or where code
/// comes from (its scripts, its build, a new dependency, a dependency from another source), as the measure says. A
/// dependency that comes in only through a lock file holds nothing.
pub const MANIFESTS: &[&str] = &["**/package.json", "**/pyproject.toml", "**/Cargo.toml"];

/// Whether `path` is a package manifest ([`MANIFESTS`]).
pub fn manifest(path: &str) -> bool {
    MANIFESTS.iter().any(|g| glob::matches(&path.to_lowercase(), &g.to_lowercase()))
}

/// G19: whether a change brought in by a merge waits for the person first. Every thing on the floor does, but a
/// package manifest the measure said changes neither what runs nor where code comes from. Without the measure's
/// word, a manifest holds too.
pub fn holds_merge(target: Target, c: &Changed) -> bool {
    let on_floor = c.floor
        || match target {
            Target::Repository => !floor([c.what.as_str()]).is_empty(),
        };
    on_floor && !(manifest(&c.what) && c.holds == Some(false))
}

/// A change as the record holds it: as above, or as engines before targets wrote it (a path and its lines, the floor
/// found from the path), so every recorded run reads as it did.
#[derive(Deserialize, schemars::JsonSchema)]
struct ChangedOnRecord {
    #[serde(alias = "path")]
    what: String,
    #[serde(alias = "lines")]
    size: u64,
    #[serde(default)]
    removed: u64,
    floor: Option<bool>,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    holds: Option<bool>,
    #[serde(default)]
    about: String,
}

impl From<ChangedOnRecord> for Changed {
    fn from(r: ChangedOnRecord) -> Changed {
        let floor = r.floor.unwrap_or_else(|| !floor([r.what.as_str()]).is_empty());
        Changed { what: r.what, size: r.size, removed: r.removed.min(r.size), floor, digest: r.digest, holds: r.holds, about: r.about }
    }
}

fn is_zero(n: &u64) -> bool {
    *n == 0
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

/// Test files, by the same names the fix-bug role gives them: they run only when the suite runs, inside the box, so
/// their lines and files do not make a change bigger than a small one. A test file on the floor (a `conftest.py`)
/// makes it risky all the same.
pub const TESTS: &[&str] = &[
    "**/tests/**", "**/test/**", "**/__tests__/**", "**/spec/**", "**/test_*.py", "**/*_test.py",
    "**/*.test.*", "**/*.spec.*", "**/*_test.go", "**/*_spec.rb", "**/*Test.java", "**/*Tests.cs",
];

/// Whether `path` is a test file ([`TESTS`]).
pub fn test_file(path: &str) -> bool {
    TESTS.iter().any(|g| glob::matches(path, g))
}

/// Text that nothing runs: Markdown, plain text, reStructuredText. It counts neither as lines nor as files of a change,
/// but what is on the floor ([`FLOOR`]): the instructions an agent obeys (`CLAUDE.md`, `AGENTS.md`) count, and hold.
pub const TEXT: &[&str] = &["**/*.md", "**/*.markdown", "**/*.txt", "**/*.rst"];

/// Whether `path` is text that nothing runs ([`TEXT`]), and not on the floor.
pub fn text_file(path: &str) -> bool {
    TEXT.iter().any(|g| glob::matches(&path.to_lowercase(), g)) && floor([path]).is_empty()
}

/// The risk of a run's change so far on its target, with the reason for each rule that raised it. The same rules for
/// every target: what is on the floor, the size of the change, and how many things it touches, its tests and its text
/// left out of the size and the count ([`TESTS`], [`TEXT`]). A measure may mark more as on the floor than the core knows, and never
/// less: on a repository, a path on [`FLOOR`] is on it whatever the measure said. Each reason names the files on the
/// floor as a stop names them, so it reads the same wherever it is shown.
pub fn of_change(target: Target, changed: &[Changed]) -> (Risk, Vec<String>) {
    let on_floor = |c: &Changed| {
        c.floor
            || match target {
                Target::Repository => !floor([c.what.as_str()]).is_empty(),
            }
    };
    let mut reasons = Vec::new();
    let floor: Vec<&str> = changed.iter().filter(|c| on_floor(c)).map(|c| c.what.as_str()).collect();
    if !floor.is_empty() {
        reasons.push(format!("the change touches {}, which {} later outside any box", named(&floor), if floor.len() == 1 { "runs" } else { "run" }));
    }
    let counted: Vec<&Changed> = changed
        .iter()
        .filter(|c| match target {
            Target::Repository => !test_file(&c.what) && !text_file(&c.what),
        })
        .collect();
    let tests = changed.iter().any(|c| test_file(&c.what));
    let text = changed.iter().any(|c| !test_file(&c.what) && text_file(&c.what));
    let outside = match (tests, text) {
        (true, true) => " outside its tests and text",
        (true, false) => " outside its tests",
        (false, true) => " outside its text",
        (false, false) => "",
    };
    let lines: u64 = counted.iter().map(|c| c.size).sum();
    if lines > SMALL_LINES {
        reasons.push(format!("the change has {lines} changed lines{outside}, more than a small change's {SMALL_LINES}"));
    }
    if counted.len() > SMALL_FILES {
        reasons.push(format!("the change touches {} files{outside}, more than a small change's {SMALL_FILES}", counted.len()));
    }
    (if reasons.is_empty() { Risk::Low } else { Risk::High }, reasons)
}

/// Where a stop that asks only at high risk says why the risk is high, in what its role writes it asks.
pub const RISK_TOLD: &str = "{{risk}}";

/// Why `changed` makes a run's risk high, as a stop says it to the person: a sentence for each rule of
/// [`of_change`] that holds, the files that run later outside any box named, and how much bigger than a small change
/// it is. None when none holds.
pub fn told(target: Target, changed: &[Changed]) -> Vec<String> {
    of_change(target, changed).1.into_iter().map(|r| format!("This{}.", r.strip_prefix("the").unwrap_or(&r))).collect()
}

/// The most files a stop names; the rest are counted.
pub const NAMED_MOST: usize = 5;
/// The most characters of a file's name a stop shows.
const NAME_MOST: usize = 80;

/// Files as a stop names them to the person: each between backquotes, on one line, with nothing a reader would take
/// for words of Jaynes's own (no control character, no line break, no backquote of its own) and cut when long; the
/// first few, and how many more. An agent chose the names, so they are shown as names and never read as sentences.
fn named(files: &[&str]) -> String {
    let one = |f: &str| {
        let clean: String = f.chars().map(|c| if c.is_control() { ' ' } else if c == '`' { '\'' } else { c }).collect();
        let cut: String = clean.chars().take(NAME_MOST).collect();
        format!("`{cut}{}`", if clean.chars().count() > NAME_MOST { "…" } else { "" })
    };
    let shown: Vec<String> = files.iter().take(NAMED_MOST).map(|f| one(f)).collect();
    let more = files.len().saturating_sub(NAMED_MOST);
    match (shown.split_last(), more) {
        (Some((last, [])), 0) => last.clone(),
        (Some((last, first)), 0) => format!("{} and {last}", first.join(", ")),
        (_, more) => format!("{} and {more} more", shown.join(", ")),
    }
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

    #[test]
    fn each_kind_on_the_floor_says_how_it_runs_in_plain_words() {
        for (path, how) in [
            ("tests/conftest.py", "runs on its own whenever the tests run"),
            ("sitecustomize.py", "Python runs it on its own, before your code"),
            ("venv/bin/activate", "Python runs it on its own, before your code"),
            ("pyproject.toml", "runs when the project is installed or built"),
            ("web/package.json", "runs when the project is installed or built"),
            ("crates/x/build.rs", "runs when the project is installed or built"),
            ("Makefile", "runs when you run make"),
            (".github/workflows/ci.yml", "runs in your CI"),
            (".gitlab-ci.yml", "runs in your CI"),
            (".gitattributes", "git runs or obeys it on your machine"),
            (".husky/pre-commit", "git runs or obeys it on your machine"),
            ("lefthook.yml", "git runs or obeys it on your machine"),
            (".envrc", "runs when you enter the folder"),
            (".claude/settings.json", "your agents read it as instructions"),
            ("CLAUDE.md", "your agents read it as instructions"),
            ("docs/AGENTS.md", "your agents read it as instructions"),
            (".vscode/settings.json", "your editor obeys it"),
            (".devcontainer/devcontainer.json", "sets up your dev container"),
            ("TESTS/CONFTEST.PY", "runs on its own whenever the tests run"),
        ] {
            assert_eq!(runs_how(path), how, "{path}");
        }
        // every kind on the floor has words, the table's or the plain ones
        for g in FLOOR {
            let path = g.trim_start_matches("**/").replace("**", "x").replace('*', "x");
            assert!(!floor([path.as_str()]).is_empty(), "{path} is on the floor");
            assert!(!runs_how(&path).is_empty());
        }
        assert_eq!(runs_how("Brewfile"), "runs later outside any box");
    }

    use super::*;

    fn c(path: &str, lines: u64) -> Changed {
        Changed::file(path.to_string(), lines)
    }

    #[test]
    fn the_paths_that_turn_into_code_on_the_person_s_machine_are_on_the_floor() {
        // what runs or is obeyed by itself once the delivery lands, or once the person opens the project: an agent can
        // write each of these in the copy today (measured with Claude Code in the agent's box, by its Write tool or by
        // Bash), so each must make the change risky, wherever it is in the project
        for path in [
            ".vscode/tasks.json", ".vscode/settings.json", ".vscode/launch.json", "web/.vscode/tasks.json",
            ".claude/settings.json", ".claude/settings.local.json", ".claude/commands/x.md", ".claude/hooks/x.sh",
            ".mcp.json", ".envrc", "sub/.envrc", ".devcontainer/devcontainer.json", ".github/workflows/ci.yml",
            ".husky/pre-commit", "lefthook.yml", "lefthook.yaml", ".lefthook.yml", "lefthook-local.yml", "app/lefthook.yml",
            ".pre-commit-config.yaml", ".venv/bin/activate", "venv/bin/python", "CLAUDE.md", "AGENTS.md", ".gitattributes",
        ] {
            assert_eq!(floor([path]), [path], "{path} turns into code on the person's machine and is not on the floor");
        }
        // and what does not stays off it
        for path in ["src/lefthook.rs", "docs/lefthook.md", "README.md", "src/main.rs"] {
            assert!(floor([path]).is_empty(), "{path}");
        }
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
        let marked = Changed { what: "src/a.py".into(), size: 1, floor: true, ..Default::default() };
        assert_eq!(of_change(Target::Repository, &[marked]).0, Risk::High);
        let unknown = Changed { what: "a form sent".into(), size: 1, floor: true, ..Default::default() };
        assert_eq!(of_change(Target::Repository, &[unknown]).0, Risk::High, "a name the core does not know, marked by the measure");
        let unmarked = Changed { what: ".github/workflows/ci.yml".into(), size: 1, floor: false, ..Default::default() };
        assert_eq!(of_change(Target::Repository, &[unmarked]).0, Risk::High);
        let plain = Changed { what: "src/a.py".into(), size: 1, floor: false, ..Default::default() };
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

    // The real-user, 22/11: a kickoff of two text files went high for 41 lines of Markdown. Text counts neither as
    // lines nor as files, but the instructions an agent obeys, which are on the floor.
    #[test]
    fn text_files_count_neither_as_lines_nor_as_files_but_an_agent_s_instructions_hold() {
        assert_eq!(of_change(Target::Repository, &[c("docs/brief.md", 41), c("NOTES.txt", 3)]), (Risk::Low, Vec::new()));
        let text: Vec<Changed> = (0..9).map(|i| c(&format!("docs/{i}.rst"), 50)).collect();
        assert_eq!(of_change(Target::Repository, &text).0, Risk::Low);
        assert_eq!(of_change(Target::Repository, &[c("AGENTS.md", 1)]).0, Risk::High);
        for p in ["docs/CLAUDE.md", ".cursorrules"] {
            assert_eq!(of_change(Target::Repository, &[c(p, 1)]).0, Risk::High, "{p}");
        }
        // an agent's instructions still count as lines and files
        assert_eq!(
            of_change(Target::Repository, &[c("CLAUDE.md", 31)]).1,
            vec!["the change touches `CLAUDE.md`, which runs later outside any box".to_string(), "the change has 31 changed lines, more than a small change's 30".to_string()]
        );
        assert_eq!(
            of_change(Target::Repository, &[c("src/a.go", 31), c("README.md", 40), c("src/a_test.go", 9)]),
            (Risk::High, vec!["the change has 31 changed lines outside its tests and text, more than a small change's 30".to_string()])
        );
        assert_eq!(
            of_change(Target::Repository, &[c("src/a.go", 31), c("README.md", 40)]).1,
            vec!["the change has 31 changed lines outside its text, more than a small change's 30".to_string()]
        );
        // a name that only looks like text is not
        for p in ["src/md.py", "docs/notes.txt.py", "src/rst.go"] {
            assert!(!text_file(p), "{p}");
        }
    }

    #[test]
    fn test_files_count_neither_as_lines_nor_as_files() {
        assert_eq!(of_change(Target::Repository, &[c("src/a.go", 18), c("src/a_test.go", 40)]), (Risk::Low, Vec::new()));
        let tests: Vec<Changed> = (0..9).map(|i| c(&format!("tests/test_{i}.py"), 100)).collect();
        assert_eq!(of_change(Target::Repository, &tests).0, Risk::Low);
        assert_eq!(
            of_change(Target::Repository, &[c("src/a.go", 31), c("src/a_test.go", 40)]),
            (Risk::High, vec!["the change has 31 changed lines outside its tests, more than a small change's 30".to_string()])
        );
        // on the floor whatever folder it is in
        assert_eq!(of_change(Target::Repository, &[c("tests/conftest.py", 1)]).0, Risk::High);
        // a name that only looks like a test's is not one
        for p in ["src/testing.py", "src/latest.go", "src/contest.rs", "docs/test-plan.md"] {
            assert!(!test_file(p), "{p}");
        }
    }
}
