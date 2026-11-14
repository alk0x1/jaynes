//! The index: a git repository listing roles anyone installs by name (`jaynes add NAME`). Each entry names where
//! the role is (a folder of the index's repository, or `URL#path` at a commit), the fingerprint of its content, and
//! the promises proved of it. Nothing is taken on the index's word: the fingerprint is compared and every promise
//! is proved again, by the index's CI and by each `jaynes add`.

use crate::role::content_fingerprint;
use crate::toml_role::TomlRole;
use jaynes_core::promise::Proof;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const FILE: &str = "index.toml";
/// The index Jaynes reads by default: `$JAYNES_INDEX` names another, a folder or a git URL.
pub const DEFAULT: &str = "https://github.com/alk0x1/jaynes-roles";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexFile {
    #[serde(default)]
    role: Vec<Entry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub name: String,
    pub version: String,
    /// What the role is for, in a sentence.
    #[serde(default)]
    pub about: String,
    /// A folder of the index's repository, or `URL#path`.
    pub source: String,
    /// For a role in another repository: the commit it is listed at.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub commit: String,
    pub fingerprint: String,
    /// What the role promises, in its words: every one proved.
    #[serde(default)]
    pub promises: Vec<String>,
}

/// A folder removed when dropped.
pub struct Gone(pub PathBuf);

impl Drop for Gone {
    fn drop(&mut self) {
        let _gone = std::fs::remove_dir_all(&self.0);
    }
}

fn tmp(what: &str) -> PathBuf {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!("jaynes-{what}-{}-{t}", std::process::id()))
}

fn remote(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("git@") || s.starts_with("file://")
}

/// Clone `url` into a temporary folder, at `commit` when one is given.
fn clone(url: &str, commit: &str) -> Result<(PathBuf, Gone), String> {
    let dir = tmp("index");
    let out = Command::new("git").args(["clone", "-q", "--depth", "1", url]).arg(&dir).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("git could not fetch {url}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let gone = Gone(dir.clone());
    if !commit.is_empty() {
        let at = |args: &[&str]| Command::new("git").arg("-C").arg(&dir).args(args).output().map(|o| o.status.success()).unwrap_or(false);
        if !(at(&["fetch", "-q", "--depth", "1", "origin", commit]) && at(&["checkout", "-q", commit])) {
            return Err(format!("{url} has no commit {commit}"));
        }
    }
    Ok((dir, gone))
}

/// The index's folder: `$JAYNES_INDEX` or the default, cloned when it is a URL.
pub fn fetch() -> Result<(PathBuf, Option<Gone>), String> {
    let at = std::env::var("JAYNES_INDEX").unwrap_or_else(|_| DEFAULT.to_string());
    if remote(&at) {
        let (dir, gone) = clone(&at, "")?;
        Ok((dir, Some(gone)))
    } else {
        Ok((PathBuf::from(at), None))
    }
}

pub fn load(index: &Path) -> Result<Vec<Entry>, String> {
    let text = std::fs::read_to_string(index.join(FILE)).map_err(|e| format!("the index has no {FILE}: {e}"))?;
    let file: IndexFile = toml::from_str(&text).map_err(|e| format!("{FILE}: {}", e.message()))?;
    Ok(file.role)
}

/// The folder of an entry's role, and the commit it was taken at.
pub fn resolve(entry: &Entry, index: &Path) -> Result<(PathBuf, Option<String>, Option<Gone>), String> {
    if remote(&entry.source) {
        let (url, sub) = entry.source.split_once('#').unwrap_or((&entry.source, ""));
        let (dir, gone) = clone(url, &entry.commit)?;
        let commit = (!entry.commit.is_empty()).then(|| entry.commit.clone());
        return Ok((dir.join(sub), commit, Some(gone)));
    }
    if entry.source.starts_with('/') || entry.source.split('/').any(|p| p == "..") {
        return Err(format!("{}: its source {} is outside the index", entry.name, entry.source));
    }
    let commit = Command::new("git").arg("-C").arg(index).args(["rev-parse", "HEAD"]).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    Ok((index.join(&entry.source), commit, None))
}

/// What the role in `folder` is, as an entry says it: its name, version, fingerprint and the promises proved.
/// The lines of each promise's proof come back too.
pub fn describe(folder: &Path, source: &str, commit: &str, about: &str) -> Result<(Entry, Vec<String>), String> {
    let role = TomlRole::load(folder).map_err(|e| format!("the role does not read:\n  {}", e.join("\n  ")))?;
    let promises: Vec<jaynes_core::promise::Promise> = role.promises.iter().map(|(_, p)| p.clone()).collect();
    let proofs = jaynes_core::promise::prove(&role.manifest, &promises);
    let (lines, _) = role.prove_lines(&proofs);
    let proved = promises.iter().zip(&proofs).filter(|(_, p)| matches!(p, Proof::Proved { .. })).map(|(p, _)| p.says.clone()).collect();
    let entry = Entry {
        name: role.manifest.role.clone(),
        version: role.manifest.version.clone(),
        about: about.to_string(),
        source: source.to_string(),
        commit: commit.to_string(),
        fingerprint: content_fingerprint(folder)?.0,
        promises: proved,
    };
    if proofs.iter().any(|p| !matches!(p, Proof::Proved { .. })) {
        return Err(format!("a promise of {} does not hold:\n{}", entry.name, lines.join("\n")));
    }
    Ok((entry, lines))
}

/// Check an entry against the role it names: the same name, version and content, and every promise it lists
/// proved again here.
pub fn verify(entry: &Entry, folder: &Path) -> Result<Vec<String>, String> {
    let (found, lines) = describe(folder, &entry.source, &entry.commit, &entry.about)?;
    let mut wrong = Vec::new();
    if found.name != entry.name || found.version != entry.version {
        wrong.push(format!("the index lists {} {}, and the role is {} {}", entry.name, entry.version, found.name, found.version));
    }
    if found.fingerprint != entry.fingerprint {
        wrong.push(format!("{}'s content is not what the index lists: its fingerprint is {}, the index says {}", entry.name, found.fingerprint, entry.fingerprint));
    }
    let mut listed = entry.promises.clone();
    let mut proved = found.promises.clone();
    listed.sort();
    proved.sort();
    if listed != proved {
        wrong.push(format!("the index lists {}'s promises as {listed:?}, and those proved are {proved:?}", entry.name));
    }
    if wrong.is_empty() { Ok(lines) } else { Err(wrong.join("\n")) }
}

/// Check every entry of the index in `index`: what the index's CI runs. With `write`, the entries of roles in the
/// index's own folders are written again from what their roles are, so a maintainer lists a change with one command.
pub fn check(index: &Path, write: bool) -> Result<Vec<String>, String> {
    let mut entries = load(index)?;
    let mut said = Vec::new();
    let mut failed = false;
    for entry in entries.iter_mut() {
        let (folder, _, _gone) = match resolve(entry, index) {
            Ok(r) => r,
            Err(e) => {
                failed = true;
                said.push(format!("✘ {}: {e}", entry.name));
                continue;
            }
        };
        if write && !remote(&entry.source) {
            match describe(&folder, &entry.source, "", &entry.about) {
                Ok((now, lines)) => {
                    *entry = now;
                    said.push(format!("✔ {} {} (written)", entry.name, entry.version));
                    said.extend(lines.into_iter().map(|l| format!("  {l}")));
                    continue;
                }
                Err(e) => {
                    failed = true;
                    said.push(format!("✘ {}: {e}", entry.name));
                    continue;
                }
            }
        }
        match verify(entry, &folder) {
            Ok(lines) => {
                said.push(format!("✔ {} {}", entry.name, entry.version));
                said.extend(lines.into_iter().map(|l| format!("  {l}")));
            }
            Err(e) => {
                failed = true;
                said.push(format!("✘ {}: {e}", entry.name));
            }
        }
    }
    if write {
        let text = toml::to_string(&IndexFile { role: entries }).map_err(|e| e.to_string())?;
        let head = "# The roles anyone installs by name: jaynes add NAME. Written by `jaynes index check . --write`;\n# the CI proves every entry again.\n\n";
        std::fs::write(index.join(FILE), format!("{head}{text}")).map_err(|e| e.to_string())?;
    }
    if failed { Err(said.join("\n")) } else { Ok(said) }
}

/// Find `name` in the index, and the folder of its role once its entry holds.
pub fn find(name: &str) -> Result<(Entry, PathBuf, Option<String>, Vec<Gone>), String> {
    let (index, gone) = fetch()?;
    let entry = load(&index)?
        .into_iter()
        .find(|e| e.name == name)
        .ok_or_else(|| format!("the index has no role {name}: a folder, or URL#path, installs a role it does not list"))?;
    let (folder, commit, more) = resolve(&entry, &index)?;
    verify(&entry, &folder).map_err(|e| format!("the index's entry for {name} does not hold, so it was not installed:\n{e}"))?;
    let keep: Vec<Gone> = gone.into_iter().chain(more).collect();
    Ok((entry, folder, commit, keep))
}
