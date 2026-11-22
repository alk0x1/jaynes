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
/// Each role's license, by its name (an SPDX identifier, as `Apache-2.0`), beside the index: a file of its own, so the
/// engines that read the index strictly, as every one up to 0.2.2 does, are not broken by a field they do not know.
pub const LICENSES: &str = "licenses.toml";
/// Entries for a Jaynes newer than the ones that read the index strictly, beside it, in the index's own form: each
/// names in `engine` the oldest Jaynes it is for (`">=0.2.5"`), and a Jaynes that is takes it over the index's entry
/// of the same name. One older never opens this file, and keeps the index's entry.
pub const NEWER: &str = "newer.toml";
/// The index Jaynes reads by default: `$JAYNES_INDEX` names another, a folder or a git URL.
pub const DEFAULT: &str = "https://github.com/alk0x1/jaynes-roles";

/// The index as its file holds it. A field this engine does not know is kept as it is and said by the index's
/// check, never refused: a field added later must not break the engines already installed.
#[derive(Serialize, Deserialize)]
struct IndexFile {
    #[serde(default)]
    role: Vec<Entry>,
    #[serde(flatten)]
    unknown: toml::Table,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// The oldest Jaynes the entry is for, as `">=0.2.5"`: only in [`NEWER`], never in the index itself.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub engine: String,
    /// Its license, from [`LICENSES`]: empty when the index says none.
    #[serde(skip)]
    pub license: String,
    /// Fields this engine does not know, kept as they are.
    #[serde(flatten)]
    pub unknown: toml::Table,
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

/// The roles the index lists for this Jaynes: each entry of the index, or, when the file beside it has one of the
/// same name for a Jaynes no newer than this one, the latest of those.
pub fn load(index: &Path) -> Result<Vec<Entry>, String> {
    let mut entries = read(index, FILE)?.map(|f| f.role).unwrap_or_default();
    for e in read(index, NEWER)?.map(|f| f.role).unwrap_or_default() {
        if e.engine.is_empty() || crate::toml_role::engine_wrong(&e.engine).is_some() {
            continue;
        }
        match entries.iter_mut().find(|x| x.name == e.name) {
            Some(x) if !crate::install::newer(&e.version, &x.version) => {}
            Some(x) => *x = e,
            None => entries.push(e),
        }
    }
    Ok(entries)
}

/// A file of the index, each entry with its license: the index itself must be there, the file beside it may not.
fn read(index: &Path, name: &str) -> Result<Option<IndexFile>, String> {
    let text = match std::fs::read_to_string(index.join(name)) {
        Ok(text) => text,
        Err(_) if name != FILE => return Ok(None),
        Err(e) => return Err(format!("the index has no {FILE}: {e}")),
    };
    let mut file: IndexFile = toml::from_str(&text).map_err(|e| format!("{name}: {}", e.message()))?;
    let licenses: std::collections::BTreeMap<String, String> = match std::fs::read_to_string(index.join(LICENSES)) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{LICENSES}: {} (each line is NAME = \"SPDX identifier\")", e.message()))?,
        Err(_) => Default::default(),
    };
    for entry in &mut file.role {
        entry.license = licenses.get(&entry.name).map(|l| l.trim().to_string()).unwrap_or_default();
    }
    Ok(Some(file))
}

/// Why an entry's license does not do: none, or not an SPDX identifier as an index writes one.
fn license_wrong(entry: &Entry) -> Option<String> {
    let l = &entry.license;
    if l.is_empty() {
        return Some(format!("{} has no license in {LICENSES}: add {} = \"Apache-2.0\" (or the SPDX identifier of its license)", entry.name, entry.name));
    }
    let fine = l.len() <= 200 && l.chars().all(|c| c.is_ascii_alphanumeric() || " .-+()".contains(c));
    (!fine).then(|| format!("{}'s license {l:?} is not an SPDX identifier, such as Apache-2.0 or MIT", entry.name))
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
        engine: String::new(),
        license: String::new(),
        unknown: Default::default(),
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

/// Check every entry of the index in `index`, and of the file beside it: what the index's CI runs. With `write`, the
/// entries of roles in the index's own folders are written again from what their roles are, so a maintainer lists a
/// change with one command.
pub fn check(index: &Path, write: bool) -> Result<Vec<String>, String> {
    let mut said = Vec::new();
    let mut failed = false;
    for name in [FILE, NEWER] {
        let Some(mut file) = read(index, name)? else { continue };
        check_file(index, name, &mut file, write, &mut said, &mut failed);
        if write {
            let text = toml::to_string(&file).map_err(|e| e.to_string())?;
            let head = if name == FILE {
                "# The roles anyone installs by name: jaynes add NAME. Written by `jaynes index check . --write`;\n# the CI proves every entry again.\n\n"
            } else {
                "# Roles for a Jaynes newer than the ones that read index.toml strictly: each names in engine the oldest Jaynes\n# it is for, and a Jaynes that is takes it over index.toml's entry of the same name. Written by\n# `jaynes index check . --write`; the CI proves every entry again.\n\n"
            };
            std::fs::write(index.join(name), format!("{head}{text}")).map_err(|e| e.to_string())?;
        }
    }
    if failed { Err(said.join("\n")) } else { Ok(said) }
}

fn check_file(index: &Path, name: &str, file: &mut IndexFile, write: bool, said: &mut Vec<String>, failed: &mut bool) {
    let known = |t: &toml::Table| t.keys().cloned().collect::<Vec<_>>().join(", ");
    if !file.unknown.is_empty() {
        said.push(format!("note: {name} has fields this Jaynes does not know, left as they are: {}", known(&file.unknown)));
    }
    for entry in file.role.iter_mut() {
        if !entry.unknown.is_empty() {
            said.push(format!("note: {}'s entry has fields this Jaynes does not know, left as they are: {}", entry.name, known(&entry.unknown)));
        }
        // the index is read strictly by engines up to 0.2.2, and the file beside it only by those that know engine
        let engine = match (name == FILE, entry.engine.is_empty()) {
            (true, false) => Some(format!("engine goes in {NEWER}, beside the index: the engines that read {FILE} strictly do not know it")),
            (false, true) => Some(format!("an entry of {NEWER} names the oldest Jaynes it needs, as engine = \">=0.2.5\"")),
            (false, false) => crate::toml_role::engine_wrong(&entry.engine),
            (true, true) => None,
        };
        if let Some(why) = engine.or_else(|| license_wrong(entry)) {
            *failed = true;
            said.push(format!("✘ {} {}: {why}", entry.name, entry.version));
            continue;
        }
        let (folder, _, _gone) = match resolve(entry, index) {
            Ok(r) => r,
            Err(e) => {
                *failed = true;
                said.push(format!("✘ {}: {e}", entry.name));
                continue;
            }
        };
        if write && !remote(&entry.source) {
            match describe(&folder, &entry.source, "", &entry.about) {
                Ok((now, lines)) => {
                    *entry = Entry { license: entry.license.clone(), unknown: entry.unknown.clone(), engine: entry.engine.clone(), ..now };
                    said.push(format!("✔ {} {} (written)", entry.name, entry.version));
                    said.extend(lines.into_iter().map(|l| format!("  {l}")));
                    continue;
                }
                Err(e) => {
                    *failed = true;
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
                *failed = true;
                said.push(format!("✘ {}: {e}", entry.name));
            }
        }
    }
}

/// Find `name` in the index, and the folder of its role once its entry holds.
pub fn find(name: &str) -> Result<(Entry, PathBuf, Option<String>, Vec<Gone>), String> {
    let (index, gone) = fetch()?;
    let entry = load(&index)?
        .into_iter()
        .find(|e| e.name == name)
        .ok_or_else(|| format!("the index has no role {name}: a folder, or URL#path, installs a role it does not list"))?;
    if let Some(why) = license_wrong(&entry) {
        return Err(format!("{why}, so it was not installed"));
    }
    let (folder, commit, more) = resolve(&entry, &index)?;
    verify(&entry, &folder).map_err(|e| format!("the index's entry for {name} does not hold, so it was not installed:\n{e}"))?;
    let keep: Vec<Gone> = gone.into_iter().chain(more).collect();
    Ok((entry, folder, commit, keep))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An index with small-change in it twice: 3.0.0 in index.toml, for every Jaynes, and the same role as 4.0.0 in
    /// the file beside it, for a Jaynes no older than `needs`.
    fn two_versions(name: &str, needs: &str) -> PathBuf {
        let index = std::env::temp_dir().join(format!("jaynes-index-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&index);
        let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roles/small-change");
        for (folder, version) in [("roles/small-change", "3.0.0"), ("roles/small-change-4", "4.0.0")] {
            std::fs::create_dir_all(index.join(folder)).expect("dir");
            for f in std::fs::read_dir(&from).expect("role").flatten() {
                let text = std::fs::read_to_string(f.path()).expect("read");
                let text = if f.file_name() == "role.toml" && version == "4.0.0" {
                    text.replacen("version = \"3.0.0\"\n", &format!("version = \"4.0.0\"\nengine = \"{needs}\"\n"), 1)
                } else {
                    text
                };
                std::fs::write(index.join(folder).join(f.file_name()), text).expect("write");
            }
        }
        std::fs::write(index.join(FILE), "[[role]]\nname = \"small-change\"\nversion = \"3.0.0\"\nsource = \"roles/small-change\"\nfingerprint = \"\"\n").expect("write");
        std::fs::write(index.join(NEWER), format!("[[role]]\nname = \"small-change\"\nversion = \"4.0.0\"\nengine = \"{needs}\"\nsource = \"roles/small-change-4\"\nfingerprint = \"\"\n")).expect("write");
        std::fs::write(index.join(LICENSES), "small-change = \"Apache-2.0\"\n").expect("write");
        index
    }

    #[test]
    fn a_role_for_a_newer_jaynes_is_listed_beside_the_index_and_read_only_by_one_it_names() {
        let here = env!("CARGO_PKG_VERSION");
        let index = two_versions("newer", &format!(">={here}"));
        let said = check(&index, true).unwrap_or_else(|e| panic!("{e}"));
        assert!(said.iter().any(|l| l == "✔ small-change 3.0.0 (written)") && said.iter().any(|l| l == "✔ small-change 4.0.0 (written)"), "{said:?}");
        let said = check(&index, false).unwrap_or_else(|e| panic!("{e}"));
        assert!(said.iter().any(|l| l == "✔ small-change 4.0.0"), "{said:?}");
        // the index itself is as an engine up to 0.2.2 reads it, strictly: no engine in it
        let listed = std::fs::read_to_string(index.join(FILE)).expect("read");
        assert!(!listed.contains("engine") && listed.contains("version = \"3.0.0\""), "{listed}");
        let beside = std::fs::read_to_string(index.join(NEWER)).expect("read");
        assert!(beside.contains(&format!("engine = \">={here}\"")) && beside.contains("version = \"4.0.0\""), "{beside}");
        // this Jaynes takes the newer one
        let found = load(&index).expect("loads");
        assert_eq!(found.iter().map(|e| (e.name.as_str(), e.version.as_str())).collect::<Vec<_>>(), [("small-change", "4.0.0")]);
    }

    #[test]
    fn a_role_for_a_later_jaynes_leaves_this_one_the_index_s_entry_and_its_check_says_why() {
        let index = two_versions("later", ">=99.0.0");
        let found = load(&index).expect("loads");
        assert_eq!(found.iter().map(|e| e.version.as_str()).collect::<Vec<_>>(), ["3.0.0"]);
        let e = check(&index, false).expect_err("the newer entry cannot be checked here");
        assert!(e.contains("✘ small-change 4.0.0") && e.contains("needs Jaynes 99.0.0 or newer"), "{e}");
        // an entry beside the index that names no engine is refused: it would be read by every Jaynes that reads it
        std::fs::write(index.join(NEWER), "[[role]]\nname = \"small-change\"\nversion = \"4.0.0\"\nsource = \"roles/small-change-4\"\nfingerprint = \"\"\n").expect("write");
        let e = check(&index, false).expect_err("no engine");
        assert!(e.contains(&format!("an entry of {NEWER} names the oldest Jaynes it needs")), "{e}");
    }
}
