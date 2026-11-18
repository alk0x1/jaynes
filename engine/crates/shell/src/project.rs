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

/// git with nothing of its environment from outside: no GIT_DIR, GIT_CONFIG*, GIT_EXEC_PATH, GIT_ASKPASS or any other
/// variable that would point it at another repository, configuration or program.
fn git_command() -> Command {
    let mut c = Command::new("git");
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("GIT_") {
            c.env_remove(&k);
        }
    }
    c.env_remove("SSH_ASKPASS");
    c.args(["-c", "user.name=Jaynes", "-c", "user.email=jaynes@localhost", "-c", "commit.gpgsign=false"]);
    c
}

fn run(mut c: Command, args: &[&str]) -> Result<String, String> {
    let out = c.args(args).output().map_err(|e| format!("git did not start: {e}"))?;
    if !out.status.success() {
        return Err(format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// git in the person's own repository, as they set it up.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut c = git_command();
    c.arg("-C").arg(dir);
    run(c, args)
}

/// git on a run's copy, through the engine's own repository for it (`git_dir`), never one the copy holds: what an agent
/// writes in the copy's .git (hooks, configuration that runs a program, refs) is the agent's and is never read here.
/// Hooks and the file monitor are off besides, whatever the configuration of this machine says.
fn git_on(git_dir: &Path, work: &Path, args: &[&str]) -> Result<String, String> {
    let mut c = git_command();
    c.arg("--git-dir").arg(git_dir).arg("--work-tree").arg(work);
    c.args(["-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false"]);
    // nor the person's own configuration, nor the system's: a filter they set up (git-lfs) would run on the agent's
    // files, out of any box, and reach what the agent's attributes point it at
    c.env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1");
    run(c, args)
}

/// What a step changed between two of the copy's states, as git shows it, through the engine's repository.
pub fn diff(copy: &Path, from: &str, to: &str) -> Option<String> {
    git_on(&git_dir(copy), copy, &["diff", "--no-color", "--no-ext-diff", "--no-textconv", from, to]).ok().map(|d| d + "\n")
}

/// Every .git, folder or file, below the copy's top that an agent left: a repository nested in the copy would have the
/// engine's git run its configuration and hooks, so the change that holds one is refused. Links are not followed.
fn nested_git(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, at: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(at) else { return };
        for e in entries.flatten() {
            let path = e.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            let rel = path.strip_prefix(root).map(|r| r.display().to_string()).unwrap_or_default();
            let name = e.file_name().to_string_lossy().to_lowercase();
            // compared without case: on a file system that ignores it, as macOS's does, .GIT is .git to git
            if name == ".git" {
                if at != root {
                    out.push(rel);
                }
                continue;
            }
            // installed dependencies, with a .git of their own or not, are never part of the change
            if DEPENDENCIES.iter().any(|d| name == *d) {
                continue;
            }
            if meta.is_dir() && rel != EXCHANGE {
                walk(root, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// What the person is told before a run starts on their repository: what of it the copy does not hold.
pub fn warnings(source: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let attributes = std::fs::read_to_string(source.join(".gitattributes")).unwrap_or_default();
    if attributes.lines().any(|l| l.split_whitespace().skip(1).any(|a| a == "filter=lfs")) {
        out.push("the project keeps files in git-lfs: the run's copy holds their pointers, not their content, so a step or a check that needs them fails".to_string());
    }
    if source.join(".gitmodules").is_file() {
        out.push("the project has submodules: the run's copy comes without their content, so a step or a check that needs them fails".to_string());
    }
    out
}

/// The engine's own repository for the copy in `copy`, beside it in the run's folder, out of every agent's reach.
/// A copy made before it lived there holds it in its own .git, which is read as it was.
pub fn git_dir(copy: &Path) -> PathBuf {
    let beside = beside(copy);
    if beside.exists() || !copy.join(".git").is_dir() { beside } else { copy.join(".git") }
}

fn beside(copy: &Path) -> PathBuf {
    let name = copy.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
    copy.with_file_name(format!("{name}.git"))
}

/// The branch the person had checked out when the run started, where a run that delivers by merge merges.
pub fn start_branch_of(copy: &Path) -> String {
    std::fs::read_to_string(git_dir(copy).join("jaynes-start")).unwrap_or_default().trim().to_string()
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
        Target::Repository => Box::new(Copy { source: source.to_path_buf(), dir: dir.to_path_buf(), git: git_dir(dir), branch: name.to_string() }),
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
        if !self.git.exists() {
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
    /// The engine's own repository for the copy.
    pub git: PathBuf,
    pub branch: String,
}

impl Copy {
    /// Clone the person's repository at its current commit, with their dependencies linked in. The engine's
    /// repository for it lives beside it; the copy gets a repository of its own for the agent (git status, git diff),
    /// which the engine never reads.
    pub fn make(source: &Path, dir: &Path, branch: &str) -> Result<Copy, String> {
        let g = beside(dir);
        let copy = Copy { source: source.to_path_buf(), dir: dir.to_path_buf(), git: g.clone(), branch: branch.to_string() };
        if g.exists() {
            return Ok(copy);
        }
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut c = git_command();
        c.args(["clone", "-q", "--bare", "--no-hardlinks"]).arg(source).arg(&g);
        run(c, &[]).map_err(|e| format!("the project could not be copied: {e}"))?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        copy.g(&["config", "core.bare", "false"])?;
        copy.g(&["checkout", "-q", "-b", branch])?;
        let base = copy.g(&["rev-parse", "HEAD"])?;
        std::fs::write(g.join("jaynes-base"), base).map_err(|e| e.to_string())?;
        // The branch the person had checked out: where a run that delivers by merge merges.
        let start = git(source, &["symbolic-ref", "-q", "--short", "HEAD"]).unwrap_or_default();
        std::fs::write(g.join("jaynes-start"), start).map_err(|e| e.to_string())?;
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
        std::fs::create_dir_all(g.join("info")).map_err(|e| e.to_string())?;
        std::fs::write(g.join("info/exclude"), &exclude).map_err(|e| e.to_string())?;
        // The agent's own repository: the same history, read through the engine's objects and never written to them.
        let tmp = dir.with_file_name(format!(".{}-agent", dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        let _gone = std::fs::remove_dir_all(&tmp);
        let mut c = git_command();
        c.args(["clone", "-q", "--shared", "--no-checkout"]).arg(&g).arg(&tmp);
        run(c, &[])?;
        std::fs::rename(tmp.join(".git"), dir.join(".git")).map_err(|e| e.to_string())?;
        let _gone = std::fs::remove_dir_all(&tmp);
        // read from the engine's objects by a path relative to the copy, so the run's folder may move whole
        let name = g.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        std::fs::write(dir.join(".git/objects/info/alternates"), format!("../../../{name}/objects\n")).map_err(|e| e.to_string())?;
        git_on(&dir.join(".git"), dir, &["reset", "-q"])?;
        std::fs::write(dir.join(".git/info/exclude"), &exclude).map_err(|e| e.to_string())?;
        Ok(copy)
    }

    /// git on the copy, through the engine's repository for it.
    fn g(&self, args: &[&str]) -> Result<String, String> {
        git_on(&self.git, &self.dir, args)
    }

    pub fn head(&self) -> Result<String, String> {
        self.g(&["rev-parse", "HEAD"])
    }

    /// Stage what an attempt changed and say what it changed, with the fingerprint of the whole tree. Paths
    /// `write` does not let through are refused: the copy goes back to where it was, and the paths come back.
    pub fn stage(&self, write: &[String]) -> Result<(Vec<Changed>, String), Result<Vec<String>, String>> {
        // before git looks at the tree: a repository nested in it is refused, and removed with the rest of the attempt
        let nested = nested_git(&self.dir);
        if !nested.is_empty() {
            for n in &nested {
                let p = self.dir.join(n);
                let gone = if std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
                gone.map_err(|e| Err(format!("Jaynes could not remove the repository {n} nested in the copy: {e}")))?;
            }
            self.discard().map_err(Err)?;
            return Err(Ok(nested.iter().map(|n| format!("{n} (a repository nested in the copy: a run creates or changes no submodule)")).collect()));
        }
        self.g(&["add", "-A"]).map_err(Err)?;
        // and after: a repository that came in anyway, between the look and the add, is a gitlink in the index
        let raw = self.g(&["diff", "--cached", "--raw", "--no-renames"]).map_err(Err)?;
        let links: Vec<String> = raw
            .lines()
            .filter(|l| l.split_whitespace().nth(1) == Some("160000"))
            .filter_map(|l| l.split('\t').nth(1).map(|p| format!("{p} (a repository nested in the copy: a run creates or changes no submodule)")))
            .collect();
        if !links.is_empty() {
            self.g(&["reset", "-q"]).map_err(Err)?;
            for l in &links {
                let p = self.dir.join(l.split(' ').next().unwrap_or_default());
                let _gone = std::fs::remove_dir_all(&p);
            }
            self.discard().map_err(Err)?;
            return Err(Ok(links));
        }
        let stat = self.g(&["diff", "--cached", "--numstat", "--no-renames"]).map_err(Err)?;
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
        let tree = self.g(&["write-tree"]).map_err(Err)?;
        Ok((changed, tree))
    }

    /// The commit the run started from.
    pub fn base(&self) -> Result<String, String> {
        std::fs::read_to_string(self.git.join("jaynes-base")).map(|s| s.trim().to_string()).map_err(|e| e.to_string())
    }

    /// Put the copy at the base with `commits` applied in order, each one step's own change. A change that does
    /// not apply over the ones before it is an error naming it.
    pub fn compose(&self, commits: &[(String, String)]) -> Result<String, String> {
        self.g(&["reset", "-q", "--hard"])?;
        self.g(&["clean", "-fdq", "-e", EXCHANGE])?;
        self.g(&["checkout", "-q", "--detach", &self.base()?])?;
        for (step, c) in commits {
            if let Err(e) = self.g(&["cherry-pick", "--allow-empty", "--keep-redundant-commits", c]) {
                let _abort = self.g(&["cherry-pick", "--abort"]);
                return Err(format!("the change of {step} does not apply over the steps before it: {e}"));
            }
        }
        self.head()
    }

    /// One commit holding the whole change of a step: the copy's tree now, over `parent`, the copy as the steps
    /// before it left it. None when the step changed nothing.
    pub fn result(&self, parent: &str, message: &str) -> Result<Option<String>, String> {
        self.g(&["add", "-A"])?;
        let tree = self.g(&["write-tree"])?;
        if tree == self.g(&["rev-parse", &format!("{parent}^{{tree}}")])? {
            return Ok(None);
        }
        self.g(&["commit-tree", &tree, "-p", parent, "-m", message]).map(Some)
    }

    /// One commit of `tree` over `parent`; None when the tree is the parent's own.
    pub fn commit_tree(&self, tree: &str, parent: &str, message: &str) -> Result<Option<String>, String> {
        if tree == self.tree_of(parent)? {
            return Ok(None);
        }
        self.g(&["commit-tree", tree, "-p", parent, "-m", message]).map(Some)
    }

    /// The tree of a commit.
    pub fn tree_of(&self, commit: &str) -> Result<String, String> {
        self.g(&["rev-parse", &format!("{commit}^{{tree}}")])
    }

    /// Throw away what is not committed.
    pub fn discard(&self) -> Result<(), String> {
        self.g(&["reset", "-q", "--hard", "HEAD"])?;
        self.g(&["clean", "-fdq", "-e", EXCHANGE])?;
        Ok(())
    }

    /// Put the run's branch into the person's repository: every step's change over the base, in order. Their
    /// working tree is not touched.
    pub fn deliver(&self, commits: &[(String, String)]) -> Result<String, String> {
        let head = self.compose(commits)?;
        let refspec = format!("+{head}:refs/heads/{}", self.branch);
        git(&self.source, &["fetch", "-q", &self.git.display().to_string(), &refspec])?;
        Ok(format!("{} at {}", self.branch, &head[..head.len().min(12)]))
    }

    fn start_branch(&self) -> Result<String, String> {
        let b = std::fs::read_to_string(self.git.join("jaynes-start")).unwrap_or_default().trim().to_string();
        if b.is_empty() { Err("the project was on no branch when the run started".to_string()) } else { Ok(b) }
    }

    /// Land `commit`, made in the copy over the branch's tip `tip`, on the person's branch: by fast-forward where it
    /// is checked out, and only over a clean checkout; moved directly where it is not. Never pushed.
    fn land(&self, branch: &str, tip: &str, commit: &str) -> Result<(), String> {
        self.g(&["update-ref", "refs/jaynes/landing", commit])?;
        git(&self.source, &["fetch", "-q", &self.git.display().to_string(), "+refs/jaynes/landing:refs/jaynes/landing"])?;
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
        self.g(&["fetch", "-q", &self.source.display().to_string(), &format!("+refs/heads/{branch}:refs/jaynes/target")])?;
        if self.g(&["merge-base", "--is-ancestor", head, &tip]).is_ok() {
            return Ok(None);
        }
        let message = format!("Jaynes: run {run}, merged");
        self.g(&["checkout", "-q", "--detach", &tip])?;
        if let Err(e) = self.g(&["merge", "-q", "--no-ff", "--no-edit", "-m", &message, head]) {
            let _abort = self.g(&["merge", "--abort"]);
            return Err(format!("it conflicts with what {branch} has now: {e}"));
        }
        let merged = self.head()?;
        if self.tree_of(&merged)? == self.tree_of(&tip)? {
            // The branch has the change already: a process that died after the merge landed and before it was
            // recorded. The run's merge on the branch is the one to record, so Undo reverts it.
            self.g(&["checkout", "-q", "--detach", &tip])?;
            let found = self.g(&["log", "-1", "--format=%H", "--fixed-strings", &format!("--grep={message}"), &tip])?;
            return Ok(Some(found).filter(|f| !f.is_empty()));
        }
        self.land(&branch, &tip, &merged)?;
        Ok(Some(merged))
    }

    /// Revert the run's merges, the latest first, on the branch they landed on.
    pub fn undo(&self, merges: &[String], run: &str) -> Result<String, String> {
        let branch = self.start_branch()?;
        let tip = git(&self.source, &["rev-parse", &format!("refs/heads/{branch}")])?;
        self.g(&["fetch", "-q", &self.source.display().to_string(), &format!("+refs/heads/{branch}:refs/jaynes/target")])?;
        // compare, then swap: undo starts only from the state the run left in what it touches; already undone, it does
        // nothing, so doing it again does no harm
        let blob = |rev: &str, path: &str| self.g(&["rev-parse", "--verify", "-q", &format!("{rev}:{path}")]).ok();
        // each file, as it was before the first merge that touched it and as the last one left it
        let mut touched: std::collections::BTreeMap<String, (Option<String>, Option<String>)> = Default::default();
        for m in merges {
            let paths = self.g(&["diff", "--name-only", "--no-renames", &format!("{m}^1"), m])?;
            for path in paths.lines().filter(|p| !p.is_empty()) {
                let left = blob(m, path);
                touched.entry(path.to_string()).and_modify(|e| e.1 = left.clone()).or_insert_with(|| (blob(&format!("{m}^1"), path), left));
            }
        }
        let undone = touched.iter().all(|(path, (before, _))| blob(&tip, path) == *before);
        let changed: Vec<String> = touched.iter().filter(|(path, (_, left))| blob(&tip, path) != *left).map(|(p, _)| p.clone()).collect();
        if undone {
            return Ok(format!("already undone: {branch} has none of what the run merged"));
        }
        if !changed.is_empty() {
            return Err(format!(
                "{} changed since the run merged it: Jaynes does not undo over what is not as the run left it. Revert by hand what you want back",
                changed.join(", ")
            ));
        }
        self.g(&["checkout", "-q", "--detach", &tip])?;
        for m in merges.iter().rev() {
            if let Err(e) = self.g(&["revert", "--no-edit", "-m", "1", m]) {
                let _abort = self.g(&["revert", "--abort"]);
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
        // the agent's git reads its history through the engine's objects, which it may read and never write
        if !self.git.starts_with(&self.dir) {
            out.push(self.git.join("objects"));
        }
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
mod git_tests {
    use super::*;

    fn repo(base: &Path) -> PathBuf {
        let source = base.join("source");
        std::fs::create_dir_all(&source).expect("dir");
        std::fs::write(source.join("a.txt"), "a\n").expect("write");
        for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["commit", "-q", "-m", "start"]] {
            git(&source, &args).expect("git");
        }
        source
    }

    fn deliver_merge_undo(copy: &Copy) {
        std::fs::write(copy.dir.join("a.txt"), "b\n").expect("write");
        let (changed, tree) = copy.stage(&["**".to_string()]).expect("staged");
        assert_eq!(changed.iter().map(|c| c.what.as_str()).collect::<Vec<_>>(), ["a.txt"]);
        let c = copy.commit_tree(&tree, &copy.base().expect("base"), "change").expect("commit").expect("a change");
        let steps = vec![("change".to_string(), c)];
        copy.deliver(&steps).expect("delivered");
        let merged = copy.merge(&copy.head().expect("head"), "r").expect("merged").expect("a merge");
        assert_eq!(std::fs::read_to_string(copy.source.join("a.txt")).expect("read"), "b\n");
        Workspace::undo(copy, &[merged], "r").expect("undone");
        assert_eq!(std::fs::read_to_string(copy.source.join("a.txt")).expect("read"), "a\n");
    }

/// A copy whose change of a.txt the run merged into the person's branch; the merge.
    fn merged(name: &str) -> (PathBuf, Copy, String) {
        let base = std::env::temp_dir().join(format!("jaynes-undo-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let source = repo(&base);
        std::fs::write(source.join("other.txt"), "o\n").expect("write");
        git(&source, &["add", "-A"]).expect("add");
        git(&source, &["commit", "-q", "-m", "other"]).expect("commit");
        let copy = Copy::make(&source, &base.join("run/project"), "jaynes/r").expect("made");
        std::fs::write(copy.dir.join("a.txt"), "b\n").expect("write");
        let (_, tree) = copy.stage(&["**".to_string()]).expect("staged");
        let c = copy.commit_tree(&tree, &copy.base().expect("base"), "change").expect("commit").expect("a change");
        copy.deliver(&[("change".to_string(), c)]).expect("delivered");
        let m = copy.merge(&copy.head().expect("head"), "r").expect("merged").expect("a merge");
        (base, copy, m)
    }

    #[test]
    fn undo_refuses_when_what_it_would_touch_changed_since_the_run_merged() {
        let (base, copy, m) = merged("changed");
        // the person changed the merged file since
        std::fs::write(copy.source.join("a.txt"), "theirs\n").expect("write");
        git(&copy.source, &["commit", "-qam", "theirs"]).expect("commit");
        let tip = git(&copy.source, &["rev-parse", "HEAD"]).expect("tip");
        let refused = Workspace::undo(&copy, std::slice::from_ref(&m), "r");
        assert!(refused.as_ref().is_err_and(|e| e.contains("a.txt") && e.contains("changed since")), "{refused:?}");
        assert_eq!(git(&copy.source, &["rev-parse", "HEAD"]).expect("tip"), tip, "undo touched the branch");
        assert_eq!(std::fs::read_to_string(copy.source.join("a.txt")).expect("read"), "theirs\n");
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn undo_of_a_run_that_merged_the_same_file_twice_undoes_both() {
        let base = std::env::temp_dir().join(format!("jaynes-undo-twice-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let source = repo(&base);
        std::fs::write(source.join("a.txt"), "1\n2\n3\n").expect("write");
        git(&source, &["commit", "-qam", "lines"]).expect("commit");
        let copy = Copy::make(&source, &base.join("run/project"), "jaynes/r").expect("made");
        let deliver_merge = |text: &str| -> String {
            copy.compose(&[]).expect("at the base");
            std::fs::write(copy.dir.join("a.txt"), text).expect("write");
            let (_, tree) = copy.stage(&["**".to_string()]).expect("staged");
            let c = copy.commit_tree(&tree, &copy.base().expect("base"), "change").expect("commit").expect("a change");
            copy.deliver(&[("change".to_string(), c)]).expect("delivered");
            copy.merge(&copy.head().expect("head"), "r").expect("merged").expect("a merge")
        };
        // the run merges line 1; corrected, it delivers again with line 3 too, and the second merge lands clean
        let first = deliver_merge("one\n2\n3\n");
        let second = deliver_merge("one\n2\nthree\n");
        assert_eq!(std::fs::read_to_string(source.join("a.txt")).expect("read"), "one\n2\nthree\n");
        // nobody touched the branch since: both merges are undone
        Workspace::undo(&copy, &[first, second], "r").expect("undone");
        assert_eq!(std::fs::read_to_string(source.join("a.txt")).expect("read"), "1\n2\n3\n");
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn undo_goes_on_over_other_changes_and_doing_it_again_does_nothing() {
        let (base, copy, m) = merged("again");
        // the person changed another file since: not what the undo touches
        std::fs::write(copy.source.join("other.txt"), "changed\n").expect("write");
        git(&copy.source, &["commit", "-qam", "other changed"]).expect("commit");
        Workspace::undo(&copy, std::slice::from_ref(&m), "r").expect("undone");
        assert_eq!(std::fs::read_to_string(copy.source.join("a.txt")).expect("read"), "a\n");
        assert_eq!(std::fs::read_to_string(copy.source.join("other.txt")).expect("read"), "changed\n");
        let tip = git(&copy.source, &["rev-parse", "HEAD"]).expect("tip");
        let again = Workspace::undo(&copy, std::slice::from_ref(&m), "r").expect("again");
        assert!(again.contains("already undone"), "{again}");
        assert_eq!(git(&copy.source, &["rev-parse", "HEAD"]).expect("tip"), tip, "a second undo made a commit");
        let _gone = std::fs::remove_dir_all(&base);
    }

        #[test]
    fn the_engine_s_repository_for_a_copy_lives_beside_it_and_the_copy_s_own_is_the_agent_s() {
        let base = std::env::temp_dir().join(format!("jaynes-git-beside-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let source = repo(&base);
        let dir = base.join("run/project");
        let copy = Copy::make(&source, &dir, "jaynes/r").expect("made");
        assert_eq!(copy.git, base.join("run/project.git"));
        assert!(dir.join(".git").is_dir(), "the agent has a repository of its own in the copy");
        assert_eq!(git_dir(&dir), copy.git);
        // the agent's repository works for the agent, and its history is the project's
        assert_eq!(git(&dir, &["log", "--format=%s"]).expect("log"), "start");
        // the run's folder may move whole, as a start moves it into place
        std::fs::rename(base.join("run"), base.join("moved")).expect("moved");
        let dir = base.join("moved/project");
        assert_eq!(git(&dir, &["status", "--porcelain"]).expect("status"), "");
        let copy = Copy { source: source.clone(), dir: dir.clone(), git: git_dir(&dir), branch: "jaynes/r".into() };
        deliver_merge_undo(&copy);
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_nested_repository_is_found_whatever_its_case_and_installed_dependencies_are_not_looked_in() {
        let base = std::env::temp_dir().join(format!("jaynes-nested-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("a/.GIT")).expect("dir");
        std::fs::create_dir_all(base.join("b")).expect("dir");
        std::fs::write(base.join("b/.Git"), "gitdir: /elsewhere").expect("write");
        std::fs::create_dir_all(base.join("node_modules/pkg/.git")).expect("dir");
        std::fs::create_dir_all(base.join("lib/node_modules/x/.git")).expect("dir");
        std::fs::create_dir_all(base.join(".git")).expect("dir");
        assert_eq!(nested_git(&base), ["a/.GIT", "b/.Git"]);
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_repository_that_reaches_the_index_anyway_is_refused() {
        let base = std::env::temp_dir().join(format!("jaynes-gitlink-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let source = repo(&base);
        let copy = Copy::make(&source, &base.join("run/project"), "jaynes/r").expect("made");
        // a repository under a dependency's name, which the look before the add passes over
        let inner = copy.dir.join("vendor/node_modules/inner");
        std::fs::create_dir_all(&inner).expect("dir");
        git(&inner, &["init", "-q"]).expect("init");
        std::fs::write(inner.join("f"), "f").expect("write");
        git(&inner, &["add", "-A"]).expect("add");
        git(&inner, &["commit", "-q", "-m", "inner"]).expect("commit");
        let refused = copy.stage(&["**".to_string()]);
        assert!(matches!(&refused, Err(Ok(paths)) if paths.iter().any(|p| p.starts_with("vendor/node_modules/inner"))), "{refused:?}");
        assert!(!inner.exists(), "the nested repository is left in the copy");
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_person_is_told_what_of_their_project_the_copy_does_not_hold() {
        let base = std::env::temp_dir().join(format!("jaynes-git-warn-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("dir");
        assert!(warnings(&base).is_empty());
        std::fs::write(base.join(".gitattributes"), "*.txt text\n*.bin filter=lfs diff=lfs merge=lfs -text\n").expect("write");
        std::fs::write(base.join(".gitmodules"), "[submodule \"lib\"]\n\tpath = lib\n").expect("write");
        let w = warnings(&base);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[0].contains("git-lfs") && w[1].contains("submodules"), "{w:?}");
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn no_git_of_the_engine_runs_on_a_copy_but_through_its_own_repository() {
        // only project.rs runs git on a run's copy; install.rs and index.rs clone roles and indexes, never a copy. A net,
        // not a proof: it reads the text, so git started another way (a shell, a name held in a variable) passes it
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for e in std::fs::read_dir(&src).expect("src").flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if ["project.rs", "install.rs", "index.rs"].contains(&name.as_str()) {
                continue;
            }
            let text = std::fs::read_to_string(e.path()).expect("read");
            assert!(!text.contains("Command::new(\"git\")"), "{name} runs git itself: it goes through project.rs");
        }
    }

    #[test]
    fn a_copy_made_before_the_engine_s_repository_moved_out_still_delivers_and_undoes() {
        let base = std::env::temp_dir().join(format!("jaynes-git-legacy-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let source = repo(&base);
        // the copy as engines before made it: a plain clone, with its marks in its own .git
        let dir = base.join("run/project");
        let mut c = git_command();
        c.args(["clone", "-q", "--no-hardlinks"]).arg(&source).arg(&dir);
        run(c, &[]).expect("cloned");
        git(&dir, &["checkout", "-q", "-b", "jaynes/r"]).expect("branch");
        let head = git(&dir, &["rev-parse", "HEAD"]).expect("head");
        std::fs::write(dir.join(".git/jaynes-base"), head).expect("base");
        std::fs::write(dir.join(".git/jaynes-start"), "main").expect("start");
        assert_eq!(git_dir(&dir), dir.join(".git"));
        assert_eq!(start_branch_of(&dir), "main");
        let copy = Copy { source: source.clone(), dir: dir.clone(), git: git_dir(&dir), branch: "jaynes/r".into() };
        deliver_merge_undo(&copy);
        let _gone = std::fs::remove_dir_all(&base);
    }
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
