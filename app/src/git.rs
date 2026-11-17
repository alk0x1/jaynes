//! What git says about a folder, for the Changes panel: the files staged for the next commit, the files changed and
//! not staged, the new files git does not follow yet, and the commits not pushed. Read with git itself (porcelain
//! output, stable across versions), and changed with git too: stage, unstage, commit.

use crate::spawn;
use std::process::Stdio;

/// A file and how many lines it gained and lost; None for a binary file, which git does not count in lines.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Entry {
    pub path: String,
    pub add: Option<u32>,
    pub del: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Status {
    /// The repository's top folder: every path below is from it.
    pub root: String,
    pub branch: String,
    pub staged: Vec<Entry>,
    /// Followed by git, changed, not staged.
    pub changed: Vec<Entry>,
    /// Not followed by git yet; a folder whose files are all new is one entry ending in "/".
    pub new: Vec<String>,
    /// The commits on this branch not on its upstream, newest first: short hash and subject.
    pub unpushed: Vec<(String, String)>,
    /// The upstream branch, as "origin/main"; None when the branch has none.
    pub upstream: Option<String>,
}

impl Status {
    /// Every file that differs, once: staged, changed and new.
    pub fn count(&self) -> usize {
        let mut paths: Vec<&str> = self.staged.iter().chain(self.changed.iter()).map(|e| e.path.as_str()).collect();
        paths.sort_unstable();
        paths.dedup();
        paths.len() + self.new.len()
    }
}

/// The paths `git status --porcelain -z` lists, split: (staged, changed, new). A rename lists its new path.
pub fn parse_status(out: &[u8]) -> (Vec<String>, Vec<String>, Vec<String>) {
    let (mut staged, mut changed, mut new) = (vec![], vec![], vec![]);
    let mut fields = out.split(|b| *b == 0).filter(|f| !f.is_empty());
    while let Some(f) = fields.next() {
        if f.len() < 4 {
            continue;
        }
        let (x, y, path) = (f[0], f[1], String::from_utf8_lossy(&f[3..]).into_owned());
        // a rename or copy carries its old path as the next field
        if matches!(x, b'R' | b'C') {
            fields.next();
        }
        match (x, y) {
            (b'?', b'?') => new.push(path),
            (b'!', b'!') => {}
            _ => {
                if x != b' ' {
                    staged.push(path.clone());
                }
                if y != b' ' {
                    changed.push(path);
                }
            }
        }
    }
    (staged, changed, new)
}

/// The lines each file gained and lost, from `git diff --numstat -z`; a rename counts under its new path.
pub fn parse_numstat(out: &[u8]) -> Vec<Entry> {
    let mut entries = vec![];
    let mut fields = out.split(|b| *b == 0);
    while let Some(f) = fields.next() {
        if f.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(f);
        let mut parts = text.splitn(3, '\t');
        let (add, del, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        // a rename leaves the path empty and gives the old path, then the new, as the next fields
        let path = if path.is_empty() {
            fields.next();
            fields.next().map(|p| String::from_utf8_lossy(p).into_owned()).unwrap_or_default()
        } else {
            path.to_owned()
        };
        entries.push(Entry { path, add: add.parse().ok(), del: del.parse().ok() });
    }
    entries
}

/// The commits `git log --format=%h%x09%s` lists: hash and subject.
pub fn parse_log(out: &str) -> Vec<(String, String)> {
    out.lines().filter_map(|l| l.split_once('\t')).map(|(h, s)| (h.to_owned(), s.to_owned())).collect()
}

/// Each path with its counts from the numstat, in the order git status gave them.
fn counted(paths: Vec<String>, numstat: &[Entry]) -> Vec<Entry> {
    paths
        .into_iter()
        .map(|path| numstat.iter().find(|e| e.path == path).cloned().unwrap_or(Entry { path, add: Some(0), del: Some(0) }))
        .collect()
}

/// Runs git in a folder; its output when it succeeds.
fn git(folder: &str, args: &[&str]) -> Option<Vec<u8>> {
    spawn::command("git")
        .arg("-C")
        .arg(folder)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| o.stdout)
}

fn text(b: Vec<u8>) -> String {
    String::from_utf8_lossy(&b).trim().to_owned()
}

/// What git says about the folder; None outside a repository.
pub fn read(folder: &str) -> Option<Status> {
    let root = text(git(folder, &["rev-parse", "--show-toplevel"])?);
    let branch = git(&root, &["branch", "--show-current"]).map(text).unwrap_or_default();
    let (staged, changed, new) = parse_status(&git(&root, &["status", "--porcelain", "-z"])?);
    let staged_n = parse_numstat(&git(&root, &["diff", "--cached", "--numstat", "-z"]).unwrap_or_default());
    let changed_n = parse_numstat(&git(&root, &["diff", "--numstat", "-z"]).unwrap_or_default());
    let upstream = git(&root, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]).map(text).filter(|u| !u.is_empty());
    let unpushed = match &upstream {
        Some(_) => parse_log(&text(git(&root, &["log", "--format=%h%x09%s", "@{u}..HEAD"]).unwrap_or_default())),
        None => vec![],
    };
    Some(Status { staged: counted(staged, &staged_n), changed: counted(changed, &changed_n), new, unpushed, upstream, branch, root })
}

/// Runs git to change the repository; git's own words when it refuses.
fn change(root: &str, args: &[&str]) -> Result<(), String> {
    let out = spawn::command("git").arg("-C").arg(root).args(args).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

/// Puts a file, or a new folder, into the next commit.
pub fn stage(root: &str, path: &str) -> Result<(), String> {
    change(root, &["add", "--", path])
}

/// Takes a file out of the next commit; its changes stay in the file.
pub fn unstage(root: &str, path: &str) -> Result<(), String> {
    change(root, &["restore", "--staged", "--", path])
}

/// Commits what is staged; with nothing staged, every changed file git follows (new files only when staged).
pub fn commit(root: &str, message: &str, anything_staged: bool) -> Result<(), String> {
    if anything_staged {
        change(root, &["commit", "-q", "-m", message])
    } else {
        change(root, &["commit", "-q", "-a", "-m", message])
    }
}

#[cfg(test)]
mod tests {
    use super::{commit, parse_log, parse_numstat, parse_status, read, stage, unstage, Entry};

    #[test]
    fn status_splits_staged_changed_and_new() {
        let out = b"M  app/src/tabs.rs\0 M app/src/main.rs\0MM app/src/agents.rs\0?? research.org\0?? docs/audits/\0R  new.rs\0old.rs\0 D gone.rs\0";
        let (staged, changed, new) = parse_status(out);
        assert_eq!(staged, ["app/src/tabs.rs", "app/src/agents.rs", "new.rs"]);
        assert_eq!(changed, ["app/src/main.rs", "app/src/agents.rs", "gone.rs"]);
        assert_eq!(new, ["research.org", "docs/audits/"]);
    }

    #[test]
    fn numstat_counts_lines_and_knows_binaries_and_renames() {
        let out = b"118\t34\tapp/src/main.rs\0-\t-\tassets/mark.png\x002\t1\t\0old.rs\0new.rs\0";
        assert_eq!(
            parse_numstat(out),
            [
                Entry { path: "app/src/main.rs".into(), add: Some(118), del: Some(34) },
                Entry { path: "assets/mark.png".into(), add: None, del: None },
                Entry { path: "new.rs".into(), add: Some(2), del: Some(1) },
            ]
        );
    }

    #[test]
    fn log_gives_hash_and_subject() {
        assert_eq!(parse_log("6219917\tThe window's frame\n4f00a49\tA notice"), [("6219917".into(), "The window's frame".into()), ("4f00a49".into(), "A notice".into())]);
    }

    /// A throwaway repository with one commit of a.txt and b.txt.
    fn repo() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("jaynes-git-test-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap().status.success(), "git {args:?}");
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(dir.join("b.txt"), "x\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "first"]);
        dir
    }

    #[test]
    fn a_real_repository_reads_stages_and_commits() {
        let dir = repo();
        let root = dir.to_string_lossy().to_string();
        std::fs::write(dir.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        std::fs::write(dir.join("b.txt"), "y\n").unwrap();
        std::fs::write(dir.join("notes.org"), "mine\n").unwrap();
        let s = read(&root).unwrap();
        assert_eq!(s.branch, "main");
        assert!(s.staged.is_empty());
        assert_eq!(s.changed, [Entry { path: "a.txt".into(), add: Some(2), del: Some(1) }, Entry { path: "b.txt".into(), add: Some(1), del: Some(1) }]);
        assert_eq!(s.new, ["notes.org"]);
        assert_eq!(s.count(), 3);

        // staged: only that goes in the commit
        stage(&root, "a.txt").unwrap();
        let s = read(&root).unwrap();
        assert_eq!(s.staged.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["a.txt"]);
        assert_eq!(s.changed.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["b.txt"]);
        unstage(&root, "a.txt").unwrap();
        assert!(read(&root).unwrap().staged.is_empty());

        // nothing staged: every changed file git follows, never the new one
        commit(&root, "both", false).unwrap();
        let s = read(&root).unwrap();
        assert!(s.staged.is_empty() && s.changed.is_empty());
        assert_eq!(s.new, ["notes.org"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn outside_git_there_is_no_status() {
        let dir = std::env::temp_dir().join(format!("jaynes-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read(&dir.to_string_lossy()), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
