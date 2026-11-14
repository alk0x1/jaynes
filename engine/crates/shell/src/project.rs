//! The run's copy of the project: a clone of the person's repository at the commit it had, on the run's own
//! branch. Each attempt's change becomes a commit there, measured by Jaynes. The person's working tree is never
//! touched; the delivery is the run's branch, fetched into their repository.

use jaynes_core::glob;
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
                    exclude.push_str(&format!("/{d}\n"));
                }
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
            changed.push(Changed { path: path.to_string(), lines });
        }
        let outside: Vec<String> =
            changed.iter().filter(|c| !glob::allowed(&c.path, write)).map(|c| c.path.clone()).collect();
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
