//! The run's copy of its target, and the operations every kind of target answers ([`Workspace`]): make the copy,
//! measure what an attempt changed, compose the copy as the steps before left it, deliver, and undo what landed.
//!
//! A repository ([`Copy`]) is the kind the engine carries: a clone of the person's repository at the commit it had,
//! on the run's own branch. Each attempt's change becomes a commit there, measured by Jaynes. The person's working
//! tree is never touched; the delivery is the run's branch, fetched into their repository.

use jaynes_core::glob;
use jaynes_core::manifest::Target;
use jaynes_core::risk::Changed;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Folders of installed dependencies, linked from the person's repository rather than copied.
pub const DEPENDENCIES: &[&str] = &[".venv", "node_modules"];
/// The folder in the copy where Jaynes and the agent exchange files. Never part of the change.
pub const EXCHANGE: &str = ".jaynes";

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Jaynes", "-c", "user.email=jaynes@localhost", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .map_err(|e| format!("git did not start: {e}"))?;
    if !out.status.success() {
        return Err(format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The run's copy of its target, as the shell works it. Every change, state and landing is named by an id the
/// workspace gives and alone reads; the run keeps them in its folder, in order, and the delivery is made from them.
pub trait Workspace {
    /// Where the steps that reach the target work.
    fn dir(&self) -> &Path;
    /// The person's own thing the copy was made from.
    fn source(&self) -> &Path;
    /// What an attempt changed, measured by Jaynes, with the id of the state it left. Changes outside `write` are
    /// refused: the copy goes back to where it was, and what was outside comes back.
    #[allow(clippy::type_complexity)]
    fn measure(&self, write: &[String]) -> Result<(Vec<Changed>, String), Result<Vec<String>, String>>;
    /// Put the copy at the base with `changes` applied in order, each one step's own; the id of the state it is in.
    /// A change that does not apply over the ones before it is an error naming its step.
    fn compose(&self, changes: &[(String, String)]) -> Result<String, String>;
    /// Keep a step's change: `state` as measured (or the copy as it is now) over `parent`. None when the step changed
    /// nothing.
    fn keep(&self, state: Option<&str>, parent: &str, message: &str) -> Result<Option<String>, String>;
    /// The state a change or state id stands for, compared by equality.
    fn state_of(&self, id: &str) -> Result<String, String>;
    /// The state the run started from.
    fn base(&self) -> Result<String, String>;
    /// The state the copy is in.
    fn head(&self) -> Result<String, String>;
    /// Throw away what an attempt left that is not kept.
    fn discard(&self) -> Result<(), String>;
    /// Put the run's change where the person takes it, leaving their own thing as it was. What was made, in words.
    fn deliver(&self, changes: &[(String, String)]) -> Result<String, String>;
    /// Land the delivery `head` on the person's own thing. The id of what landed, or None when it was there already.
    fn land(&self, head: &str, run: &str) -> Result<Option<String>, String>;
    /// Undo what the run landed, the latest first, over what the person's thing has now.
    fn undo(&self, landed: &[String], run: &str) -> Result<String, String>;
    /// Folders outside the copy the target's own commands must read.
    fn reads(&self) -> Vec<PathBuf>;
}

/// Make the copy a run of `target` works in, from the person's `source`, named `name` (a repository's branch).
pub fn make(target: Target, source: &Path, dir: &Path, name: &str) -> Result<Box<dyn Workspace>, String> {
    match target {
        Target::Repository => Copy::make(source, dir, name).map(|c| Box::new(c) as Box<dyn Workspace>),
    }
}

/// The copy a run of `target` already made.
pub fn open(target: Target, source: &Path, dir: &Path, name: &str) -> Box<dyn Workspace> {
    match target {
        Target::Repository => Box::new(Copy { source: source.to_path_buf(), dir: dir.to_path_buf(), branch: name.to_string() }),
    }
}

impl Workspace for Copy {
    fn dir(&self) -> &Path {
        &self.dir
    }
    fn source(&self) -> &Path {
        &self.source
    }
    fn measure(&self, write: &[String]) -> Result<(Vec<Changed>, String), Result<Vec<String>, String>> {
        self.stage(write)
    }
    fn compose(&self, changes: &[(String, String)]) -> Result<String, String> {
        Copy::compose(self, changes)
    }
    fn keep(&self, state: Option<&str>, parent: &str, message: &str) -> Result<Option<String>, String> {
        match state {
            Some(tree) => self.commit_tree(tree, parent, message),
            None => self.result(parent, message),
        }
    }
    fn state_of(&self, id: &str) -> Result<String, String> {
        self.tree_of(id)
    }
    fn base(&self) -> Result<String, String> {
        Copy::base(self)
    }
    fn head(&self) -> Result<String, String> {
        Copy::head(self)
    }
    fn discard(&self) -> Result<(), String> {
        Copy::discard(self)
    }
    fn deliver(&self, changes: &[(String, String)]) -> Result<String, String> {
        Copy::deliver(self, changes)
    }
    fn land(&self, head: &str, run: &str) -> Result<Option<String>, String> {
        self.merge(head, run)
    }
    fn undo(&self, landed: &[String], run: &str) -> Result<String, String> {
        if !self.dir.join(".git").exists() {
            return Err(format!(
                "the run's copy of the project was cleared a week after it finished; revert its merge in your repository with git revert -m 1 {}",
                landed.last().map(String::as_str).unwrap_or("<the merge>")
            ));
        }
        Copy::undo(self, landed, run)
    }
    fn reads(&self) -> Vec<PathBuf> {
        Copy::reads(self)
    }
}

/// A repository's copy: a clone on the run's branch.
#[derive(Clone, Debug)]
pub struct Copy {
    pub source: PathBuf,
    pub dir: PathBuf,
    pub branch: String,
}

impl Copy {
    /// Clone the person's repository at its current commit, with their dependencies linked in.
    pub fn make(source: &Path, dir: &Path, branch: &str) -> Result<Copy, String> {
        if !dir.join(".git").exists() {
            if let Some(parent) = dir.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let out = Command::new("git")
                .args(["clone", "-q", "--no-hardlinks"])
                .arg(source)
                .arg(dir)
                .output()
                .map_err(|e| format!("git did not start: {e}"))?;
            if !out.status.success() {
                return Err(format!("the project could not be copied: {}", String::from_utf8_lossy(&out.stderr).trim()));
            }
            git(dir, &["checkout", "-q", "-b", branch])?;
            let base = git(dir, &["rev-parse", "HEAD"])?;
            std::fs::write(dir.join(".git/jaynes-base"), base).map_err(|e| e.to_string())?;
            // The branch the person had checked out: where a run that delivers by merge merges.
            let start = git(source, &["symbolic-ref", "-q", "--short", "HEAD"]).unwrap_or_default();
            std::fs::write(dir.join(".git/jaynes-start"), start).map_err(|e| e.to_string())?;
            // Claude Code leaves its own scratch in every folder it works in; it is never part of the change.
            let mut exclude = format!("{EXCHANGE}/\n/.claude/.cc-writes\n");
            for d in DEPENDENCIES {
                let from = source.join(d);
                if from.is_dir() {
                    std::os::unix::fs::symlink(&from, dir.join(d)).map_err(|e| e.to_string())?;
                }
                // installed here or linked, dependencies are never part of the change
                exclude.push_str(&format!("/{d}\n"));
            }
            std::fs::write(dir.join(".git/info/exclude"), exclude).map_err(|e| e.to_string())?;
        }
        Ok(Copy { source: source.to_path_buf(), dir: dir.to_path_buf(), branch: branch.to_string() })
    }

    pub fn head(&self) -> Result<String, String> {
        git(&self.dir, &["rev-parse", "HEAD"])
    }

    /// Stage what an attempt changed and say what it changed, with the fingerprint of the whole tree. Paths
    /// `write` does not let through are refused: the copy goes back to where it was, and the paths come back.
    pub fn stage(&self, write: &[String]) -> Result<(Vec<Changed>, String), Result<Vec<String>, String>> {
        git(&self.dir, &["add", "-A"]).map_err(Err)?;
        let stat = git(&self.dir, &["diff", "--cached", "--numstat", "--no-renames"]).map_err(Err)?;
        let mut changed = Vec::new();
        for line in stat.lines() {
            let mut parts = line.splitn(3, '\t');
            let (a, d, path) = (parts.next().unwrap_or("0"), parts.next().unwrap_or("0"), parts.next().unwrap_or(""));
            let lines = a.parse::<u64>().unwrap_or(1) + d.parse::<u64>().unwrap_or(0);
            changed.push(Changed::file(path.to_string(), lines));
        }
        let outside: Vec<String> =
            changed.iter().filter(|c| !glob::allowed(&c.what, write)).map(|c| c.what.clone()).collect();
        if !outside.is_empty() {
            self.discard().map_err(Err)?;
            return Err(Ok(outside));
        }
        let tree = git(&self.dir, &["write-tree"]).map_err(Err)?;
        Ok((changed, tree))
    }

    /// The commit the run started from.
    pub fn base(&self) -> Result<String, String> {
        std::fs::read_to_string(self.dir.join(".git/jaynes-base")).map(|s| s.trim().to_string()).map_err(|e| e.to_string())
    }

    /// Put the copy at the base with `commits` applied in order, each one step's own change. A change that does
    /// not apply over the ones before it is an error naming it.
    pub fn compose(&self, commits: &[(String, String)]) -> Result<String, String> {
        git(&self.dir, &["reset", "-q", "--hard"])?;
        git(&self.dir, &["clean", "-fdq", "-e", EXCHANGE])?;
        git(&self.dir, &["checkout", "-q", "--detach", &self.base()?])?;
        for (step, c) in commits {
            if let Err(e) = git(&self.dir, &["cherry-pick", "--allow-empty", "--keep-redundant-commits", c]) {
                let _abort = git(&self.dir, &["cherry-pick", "--abort"]);
                return Err(format!("the change of {step} does not apply over the steps before it: {e}"));
            }
        }
        self.head()
    }

    /// One commit holding the whole change of a step: the copy's tree now, over `parent`, the copy as the steps
    /// before it left it. None when the step changed nothing.
    pub fn result(&self, parent: &str, message: &str) -> Result<Option<String>, String> {
        git(&self.dir, &["add", "-A"])?;
        let tree = git(&self.dir, &["write-tree"])?;
        if tree == git(&self.dir, &["rev-parse", &format!("{parent}^{{tree}}")])? {
            return Ok(None);
        }
        git(&self.dir, &["commit-tree", &tree, "-p", parent, "-m", message]).map(Some)
    }

    /// One commit of `tree` over `parent`; None when the tree is the parent's own.
    pub fn commit_tree(&self, tree: &str, parent: &str, message: &str) -> Result<Option<String>, String> {
        if tree == self.tree_of(parent)? {
            return Ok(None);
        }
        git(&self.dir, &["commit-tree", tree, "-p", parent, "-m", message]).map(Some)
    }

    /// The tree of a commit.
    pub fn tree_of(&self, commit: &str) -> Result<String, String> {
        git(&self.dir, &["rev-parse", &format!("{commit}^{{tree}}")])
    }

    /// Throw away what is not committed.
    pub fn discard(&self) -> Result<(), String> {
        git(&self.dir, &["reset", "-q", "--hard", "HEAD"])?;
        git(&self.dir, &["clean", "-fdq", "-e", EXCHANGE])?;
        Ok(())
    }

    /// Put the run's branch into the person's repository: every step's change over the base, in order. Their
    /// working tree is not touched.
    pub fn deliver(&self, commits: &[(String, String)]) -> Result<String, String> {
        let head = self.compose(commits)?;
        let refspec = format!("+{head}:refs/heads/{}", self.branch);
        git(&self.source, &["fetch", "-q", &self.dir.display().to_string(), &refspec])?;
        Ok(format!("{} at {}", self.branch, &head[..head.len().min(12)]))
    }

    fn start_branch(&self) -> Result<String, String> {
        let b = std::fs::read_to_string(self.dir.join(".git/jaynes-start")).unwrap_or_default().trim().to_string();
        if b.is_empty() { Err("the project was on no branch when the run started".to_string()) } else { Ok(b) }
    }

    /// Land `commit`, made in the copy over the branch's tip `tip`, on the person's branch: by fast-forward where it
    /// is checked out, and only over a clean checkout; moved directly where it is not. Never pushed.
    fn land(&self, branch: &str, tip: &str, commit: &str) -> Result<(), String> {
        git(&self.dir, &["update-ref", "refs/jaynes/landing", commit])?;
        git(&self.source, &["fetch", "-q", &self.dir.display().to_string(), "+refs/jaynes/landing:refs/jaynes/landing"])?;
        let checked_out = git(&self.source, &["symbolic-ref", "-q", "--short", "HEAD"]).unwrap_or_default() == branch;
        if checked_out {
            let dirty = git(&self.source, &["status", "--porcelain", "--untracked-files=no"])?;
            if !dirty.is_empty() {
                return Err(format!("your checkout of {branch} has uncommitted changes"));
            }
            git(&self.source, &["merge", "-q", "--ff-only", "refs/jaynes/landing"])?;
        } else {
            git(&self.source, &["update-ref", &format!("refs/heads/{branch}"), commit, tip])?;
        }
        Ok(())
    }

    /// Merge the run's delivery into the branch it started from. The merge commit, or why there is none.
    pub fn merge(&self, head: &str, run: &str) -> Result<Option<String>, String> {
        let branch = self.start_branch()?;
        let tip = git(&self.source, &["rev-parse", &format!("refs/heads/{branch}")])?;
        git(&self.dir, &["fetch", "-q", &self.source.display().to_string(), &format!("+refs/heads/{branch}:refs/jaynes/target")])?;
        if git(&self.dir, &["merge-base", "--is-ancestor", head, &tip]).is_ok() {
            return Ok(None);
        }
        let message = format!("Jaynes: run {run}, merged");
        git(&self.dir, &["checkout", "-q", "--detach", &tip])?;
        if let Err(e) = git(&self.dir, &["merge", "-q", "--no-ff", "--no-edit", "-m", &message, head]) {
            let _abort = git(&self.dir, &["merge", "--abort"]);
            return Err(format!("it conflicts with what {branch} has now: {e}"));
        }
        let merged = self.head()?;
        if self.tree_of(&merged)? == self.tree_of(&tip)? {
            // The branch has the change already: a process that died after the merge landed and before it was
            // recorded. The run's merge on the branch is the one to record, so Undo reverts it.
            git(&self.dir, &["checkout", "-q", "--detach", &tip])?;
            let found = git(&self.dir, &["log", "-1", "--format=%H", "--fixed-strings", &format!("--grep={message}"), &tip])?;
            return Ok(Some(found).filter(|f| !f.is_empty()));
        }
        self.land(&branch, &tip, &merged)?;
        Ok(Some(merged))
    }

    /// Revert the run's merges, the latest first, on the branch they landed on.
    pub fn undo(&self, merges: &[String], run: &str) -> Result<String, String> {
        let branch = self.start_branch()?;
        let tip = git(&self.source, &["rev-parse", &format!("refs/heads/{branch}")])?;
        git(&self.dir, &["fetch", "-q", &self.source.display().to_string(), &format!("+refs/heads/{branch}:refs/jaynes/target")])?;
        git(&self.dir, &["checkout", "-q", "--detach", &tip])?;
        for m in merges.iter().rev() {
            if let Err(e) = git(&self.dir, &["revert", "--no-edit", "-m", "1", m]) {
                let _abort = git(&self.dir, &["revert", "--abort"]);
                return Err(format!("the merge {} does not revert over what {branch} has now: {e}", &m[..m.len().min(12)]));
            }
        }
        let _amend = run;
        let reverted = self.head()?;
        self.land(&branch, &tip, &reverted)?;
        Ok(reverted)
    }

    /// The folders the project's own commands must read through its linked dependencies: their real places,
    /// and the Python each virtualenv runs.
    pub fn reads(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for d in DEPENDENCIES {
            let link = self.source.join(d);
            if let Ok(real) = std::fs::canonicalize(&link) {
                out.push(real.clone());
                let cfg = real.join("pyvenv.cfg");
                if let Ok(text) = std::fs::read_to_string(cfg) {
                    for line in text.lines() {
                        if let Some((k, v)) = line.split_once('=')
                            && k.trim() == "home" {
                                let home = PathBuf::from(v.trim());
                                if let Some(p) = home.parent() {
                                    out.push(p.to_path_buf());
                                }
                                if let Ok(r) = std::fs::canonicalize(&home)
                                    && let Some(p) = r.parent() {
                                        out.push(p.to_path_buf());
                                    }
                            }
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

/// Toolchains the agent may run, by the name of their program: where one is installed under the person's home, as a
/// version manager such as mise installs it, the agent's box may read that installation and nothing else of home.
pub const TOOLCHAINS: &[&str] = &["go", "node"];

/// The installation folders of the toolchains on `path` that live under `home`: the folder above a program's `bin`,
/// at least two folders deep in home, so a whole dotfolder such as ~/.cargo, with its credentials, is never opened.
pub fn toolchains(path: &str, home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for name in TOOLCHAINS {
        let Some(found) = path.split(':').map(|d| Path::new(d).join(name)).find(|p| p.is_file()) else { continue };
        let Ok(real) = std::fs::canonicalize(&found) else { continue };
        let Some(bin) = real.parent().filter(|b| b.file_name().is_some_and(|n| n == "bin")) else { continue };
        let Some(root) = bin.parent() else { continue };
        let deep = root.strip_prefix(home).map(|r| r.components().count() >= 3).unwrap_or(false);
        if deep && !out.contains(&root.to_path_buf()) {
            out.push(root.to_path_buf());
        }
    }
    out
}

#[cfg(test)]
mod toolchain_tests {
    use super::*;

    #[test]
    fn a_toolchain_installed_deep_in_home_is_readable_and_a_dotfolder_is_not() {
        let base = std::env::temp_dir().join(format!("jaynes-toolchains-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let home = base.join("home");
        let go = home.join(".local/share/mise/installs/go/1.27.1/bin");
        let node = home.join(".nodeish/bin");
        for (dir, name) in [(&go, "go"), (&node, "node")] {
            std::fs::create_dir_all(dir).expect("dir");
            std::fs::write(dir.join(name), "").expect("write");
        }
        let path = format!("{}:{}", go.display(), node.display());
        let real = std::fs::canonicalize(&home).expect("home");
        let found = toolchains(&path, &real);
        assert_eq!(found, vec![real.join(".local/share/mise/installs/go/1.27.1")]);
    }
}
