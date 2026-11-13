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

    /// Commit what is staged, if anything is.
    pub fn commit(&self, message: &str) -> Result<(), String> {
        git(&self.dir, &["add", "-A"])?;
        if git(&self.dir, &["diff", "--cached", "--quiet"]).is_err() {
            git(&self.dir, &["commit", "-q", "--no-verify", "-m", message])?;
        }
        Ok(())
    }

    /// Throw away what is not committed.
    pub fn discard(&self) -> Result<(), String> {
        git(&self.dir, &["reset", "-q", "--hard", "HEAD"])?;
        git(&self.dir, &["clean", "-fdq", "-e", EXCHANGE])?;
        Ok(())
    }

    /// Put the run's branch into the person's repository, as it is now. Their working tree is not touched.
    pub fn deliver(&self) -> Result<String, String> {
        let head = self.head()?;
        let refspec = format!("+{head}:refs/heads/{}", self.branch);
        git(&self.source, &["fetch", "-q", &self.dir.display().to_string(), &refspec])?;
        Ok(format!("{} at {}", self.branch, &head[..head.len().min(12)]))
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
