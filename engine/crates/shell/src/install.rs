//! What the program installs and finds for itself: the library roles are written with, the roles installed, the
//! page, the guide for agents, and its own update.

use crate::conformance;
use crate::role::{copy_dir, RoleProgram};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The library for writing a role in Python, in the program itself, so an installed Jaynes needs no source folder.
const LIBRARY: &str = include_str!("../../../python/jaynes_role.py");
/// The box and the test runners roles use for a project's own commands.
const BOX: &str = include_str!("../../../python/jaynes_box.py");

/// Where Jaynes is published, for `jaynes update` and for the roles that come with it.
pub const SOURCE: &str = "https://github.com/alk0x1/jaynes";

/// The folder holding `jaynes_role.py`: the one named by `JAYNES_ROLE_LIBRARY`, or the program's own copy, written
/// under the home when it is missing or older.
pub fn library(home: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("JAYNES_ROLE_LIBRARY") {
        return PathBuf::from(dir);
    }
    let dir = home.join("library");
    for (name, text) in [("jaynes_role.py", LIBRARY), ("jaynes_box.py", BOX)] {
        let file = dir.join(name);
        if std::fs::read_to_string(&file).ok().as_deref() != Some(text) {
            let _made = std::fs::create_dir_all(&dir);
            let _written = std::fs::write(&file, text);
        }
    }
    dir
}

pub fn roles_dir(home: &Path) -> PathBuf {
    home.join("roles")
}

/// A role by name among those installed, or by its folder.
pub fn find_role(home: &Path, given: &str) -> Result<PathBuf, String> {
    let as_path = Path::new(given);
    if as_path.join("role.py").is_file() || as_path.join("role").is_file() {
        return Ok(as_path.to_path_buf());
    }
    let installed = roles_dir(home).join(given);
    if installed.is_dir() {
        return Ok(installed);
    }
    Err(format!("there is no role {given}: `jaynes roles` lists the installed ones, and `jaynes add` installs one"))
}

/// Install a role from a folder, or from git as `URL#path/in/the/repository`. Only a role that passes the
/// conformance suite is installed; its name is the one it gives.
pub fn add(home: &Path, source: &str) -> Result<String, String> {
    let (folder, commit, _tmp) = fetch(source)?;
    install(home, source, &folder, commit)
}

/// What a role's steps may reach, one line each, to tell whether a new version asks for more.
fn reach_of(m: &jaynes_core::Manifest) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for s in &m.steps {
        let r = &s.reach;
        out.insert(format!("{}: the project, {:?}", s.name, r.project).to_lowercase());
        for w in &r.write {
            out.insert(format!("{}: writing {w}", s.name));
        }
        out.insert(format!("{}: the network, {:?}", s.name, r.network).to_lowercase());
        if r.commands {
            out.insert(format!("{}: running commands", s.name));
        }
    }
    out.retain(|l| !l.ends_with(", none"));
    out
}

fn manifest_of(role: &RoleProgram) -> Result<jaynes_core::Manifest, String> {
    match role.call(crate::protocol::Call::Describe {}) {
        Ok(crate::protocol::Body::Manifest { manifest }) => Ok(manifest),
        other => Err(format!("the role did not describe itself: {other:?}")),
    }
}

/// Update the roles installed from git. One whose new version reaches more than the installed one (the project,
/// writing, the network, commands) is not updated: the person reads what it asks and installs it with `jaynes add`.
pub fn update_roles(home: &Path) -> Vec<String> {
    let mut said = Vec::new();
    for (name, _, _, origin) in list(home) {
        if !(origin.starts_with("https://") || origin.starts_with("git@") || origin.starts_with("file://")) {
            continue;
        }
        let attempt = || -> Result<String, String> {
            let (folder, commit, _tmp) = fetch(&origin)?;
            let new = manifest_of(&RoleProgram::new(&folder, &library(home)))?;
            let old = manifest_of(&RoleProgram::new(&roles_dir(home).join(&name), &library(home)))?;
            let more: Vec<String> = reach_of(&new).difference(&reach_of(&old)).cloned().collect();
            if !more.is_empty() {
                return Ok(format!(
                    "role {name} not updated: its new version reaches more ({}). Install it with `jaynes add {origin}` once you accept that",
                    more.join("; ")
                ));
            }
            install(home, &origin, &folder, commit)?;
            Ok(format!("role {name} updated"))
        };
        said.push(attempt().unwrap_or_else(|e| format!("role {name} not updated: {e}")));
    }
    said
}

/// Install the role in `folder`, fetched from `source` at `commit`, once it passes the conformance suite: the
/// commit and the fingerprint of what was installed are kept with it.
fn install(home: &Path, source: &str, folder: &Path, commit: Option<String>) -> Result<String, String> {
    let role = RoleProgram::new(folder, &library(home));
    let findings = conformance::run(&role);
    let failed: Vec<String> = findings.iter().filter(|f| !f.ok).map(|f| format!("{}: {}", f.case, f.detail)).collect();
    if !failed.is_empty() {
        return Err(format!("the role does not conform, so it was not installed:\n  {}", failed.join("\n  ")));
    }
    let name = match role.call(crate::protocol::Call::Describe {}) {
        Ok(crate::protocol::Body::Manifest { manifest }) => manifest.role,
        other => return Err(format!("the role did not describe itself: {other:?}")),
    };
    let into = roles_dir(home).join(&name);
    if into.exists() {
        std::fs::remove_dir_all(&into).map_err(|e| e.to_string())?;
    }
    copy_dir(folder, &into)?;
    let fingerprint = RoleProgram::new(&into, &library(home)).fingerprint()?;
    let origin = serde_json::json!({"source": source, "commit": commit, "fingerprint": fingerprint.0});
    std::fs::write(into.join(".jaynes-origin"), origin.to_string()).map_err(|e| e.to_string())?;
    Ok(name)
}

/// The folder a source names, fetched from git into a temporary folder when it is a URL.
/// The folder a source names and the commit it was fetched at, fetched from git into a temporary folder (removed
/// when the last value is dropped) when it is a URL.
fn fetch(source: &str) -> Result<(PathBuf, Option<String>, Option<Gone>), String> {
    if !(source.starts_with("https://") || source.starts_with("git@") || source.starts_with("file://")) {
        let p = std::fs::canonicalize(source).map_err(|e| format!("{source}: {e}"))?;
        return Ok((p, None, None));
    }
    let (url, sub) = source.split_once('#').unwrap_or((source, ""));
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("jaynes-add-{}-{t}", std::process::id()));
    let out = Command::new("git").args(["clone", "-q", "--depth", "1", url]).arg(&tmp).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("git could not fetch {url}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let commit = Command::new("git").arg("-C").arg(&tmp).args(["rev-parse", "HEAD"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    Ok((tmp.join(sub), commit, Some(Gone(tmp))))
}

/// A temporary folder, removed when dropped.
struct Gone(PathBuf);

impl Drop for Gone {
    fn drop(&mut self) {
        let _gone = std::fs::remove_dir_all(&self.0);
    }
}

pub fn remove(home: &Path, name: &str) -> Result<(), String> {
    let dir = roles_dir(home).join(name);
    if !dir.is_dir() {
        return Err(format!("there is no role {name}"));
    }
    std::fs::remove_dir_all(dir).map_err(|e| e.to_string())
}

/// Installed roles: name, version, what they are for, where they came from.
pub fn list(home: &Path) -> Vec<(String, String, String, String)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(roles_dir(home)) else { return out };
    for e in entries.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        let origin = std::fs::read_to_string(dir.join(".jaynes-origin")).unwrap_or_default();
        // kept as JSON with its commit and fingerprint; a plain line before roles were pinned
        let origin = serde_json::from_str::<serde_json::Value>(&origin).ok().and_then(|v| v["source"].as_str().map(str::to_string)).unwrap_or(origin);
        let role = RoleProgram::new(&dir, &library(home));
        let (version, about) = match role.call(crate::protocol::Call::Describe {}) {
            Ok(crate::protocol::Body::Manifest { manifest }) => {
                let about = manifest.steps.iter().map(|s| s.name.clone()).collect::<Vec<_>>().join(", ");
                (manifest.version, format!("steps: {about}"))
            }
            _ => ("?".into(), "does not describe itself".into()),
        };
        out.push((e.file_name().to_string_lossy().to_string(), version, about, origin));
    }
    out.sort();
    out
}

/// The page's key, kept under the home so the page's address stays the same across its restarts.
pub fn page_key(home: &Path) -> String {
    let file = home.join("page.key");
    if let Ok(k) = std::fs::read_to_string(&file)
        && k.trim().len() >= 16 {
            return k.trim().to_string();
        }
    let k = crate::page::key();
    let _made = std::fs::create_dir_all(home);
    let _written = std::fs::write(&file, &k);
    k
}

pub const PORT: u16 = 8750;

/// Start the page in a process of its own if nothing answers on its port, and open it in the browser with its
/// key, which is never printed where an agent reads it. The page's address without the key.
pub fn ensure_page(home: &Path) -> String {
    let address = format!("http://127.0.0.1:{PORT}/");
    if std::env::var("JAYNES_NO_PAGE").is_ok() || std::net::TcpStream::connect(("127.0.0.1", PORT)).is_ok() {
        return address;
    }
    let Ok(exe) = std::env::current_exe() else { return address };
    let log = std::fs::File::create(home.join("page.log")).ok();
    let mut c = Command::new(exe);
    c.args(["page", "--port", &PORT.to_string()]).env("JAYNES_ENGINE_HOME", home).stdin(Stdio::null());
    match log {
        Some(l) => {
            let e = l.try_clone().ok();
            c.stdout(l);
            if let Some(e) = e {
                c.stderr(e);
            }
        }
        None => {
            c.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    if c.spawn().is_ok() {
        std::thread::sleep(std::time::Duration::from_millis(400));
        if std::env::var("JAYNES_NO_BROWSER").is_err() {
            let _opened = Command::new("open").arg(format!("{address}?key={}", page_key(home))).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        }
    }
    address
}

/// What an agent reads to carry out a change through Jaynes.
pub const GUIDE: &str = "How to carry out an agreed change through Jaynes

1. Choose a role. `jaynes roles` lists the installed ones. small-change is for a few lines; fix-bug proves a bug
   with a failing test, then fixes it. If none is installed, install them:
     jaynes add https://github.com/alk0x1/jaynes#engine/roles/small-change
     jaynes add https://github.com/alk0x1/jaynes#engine/roles/fix-bug
2. Start the run from the project's folder, with the whole agreed plan as the request, and in the background, since it
   runs agents for minutes:
     jaynes start --role <role> --project . --request \"<the plan, in full>\"
   If the change is not small, add --risk high: the run then takes the full path and its ceiling is higher.
3. Tell the person that the run goes on by itself and that what needs them is on the page Jaynes opens in their
   browser (http://127.0.0.1:8750). Do not answer for them.
4. `jaynes show <run>` says where a run is; `jaynes runs` lists them. When it finishes, its branch is jaynes/<run> in
   the project, or, for a role that merges, the change is already on the branch the run started from.

An agent may only let a result go on at a stop the role marks as a release: `jaynes answer <run> <question> --send`
from a program counts as the agent's. Everything else is the person's: never send work back for them, and never
answer a question of taste or direction.
";

/// Where cargo installs Jaynes: `~/.local`, or `$JAYNES_INSTALL_ROOT`.
pub fn install_root() -> Result<PathBuf, String> {
    if let Ok(root) = std::env::var("JAYNES_INSTALL_ROOT") {
        return Ok(PathBuf::from(root));
    }
    std::env::var("HOME").map(|h| PathBuf::from(h).join(".local")).map_err(|e| e.to_string())
}

/// The commit of Jaynes that cargo installed from git, as cargo recorded it; None when it was not installed so.
pub fn installed_commit() -> Option<String> {
    let text = std::fs::read_to_string(install_root().ok()?.join(".crates.toml")).ok()?;
    let line = text.lines().find(|l| l.contains("\"jaynes-shell ") && l.contains('#'))?;
    let after = line.split('#').nth(1)?;
    let commit: String = after.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    (commit.len() >= 7).then_some(commit)
}

/// The newest commit of Jaynes where it is published, asked of git.
pub fn latest_commit() -> Option<String> {
    let out = Command::new("git").args(["ls-remote", SOURCE, "refs/heads/main"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let commit = text.split_whitespace().next()?.to_string();
    (out.status.success() && commit.len() == 40).then_some(commit)
}

/// Where the guard in Claude Code keeps whether it is on and the GitHub owners that pass freely.
pub fn guard_path() -> Result<PathBuf, String> {
    let home = match std::env::var("JAYNES_HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => std::env::var("HOME").map(|h| PathBuf::from(h).join(".jaynes")).map_err(|e| e.to_string())?,
    };
    Ok(home.join("guard.json"))
}

/// The guard's settings, as the guard reads them: on unless it says otherwise, and its free owners.
pub fn guard() -> (bool, Vec<String>) {
    let v: serde_json::Value = guard_path().ok().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let on = v["on"].as_bool().unwrap_or(true);
    let owners = v["owners"].as_array().map(|a| a.iter().filter_map(|o| o.as_str().map(str::to_string)).collect()).unwrap_or_default();
    (on, owners)
}

/// Write the guard's settings whole, through a file renamed into place so the guard never reads half of it.
pub fn set_guard(on: bool, owners: &[String]) -> Result<(), String> {
    let path = guard_path()?;
    let mut v: serde_json::Value = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| serde_json::json!({}));
    v["on"] = serde_json::json!(on);
    v["owners"] = serde_json::json!(owners);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// Build and install the newest Jaynes from where it is published, and update the roles that came from git.
pub fn update(home: &Path) -> Result<Vec<String>, String> {
    let mut said = Vec::new();
    let root = install_root()?;
    let status = Command::new("cargo")
        .args(["install", "--locked", "--quiet", "--git", SOURCE, "jaynes-shell", "--root"])
        .arg(&root)
        .status()
        .map_err(|e| format!("cargo is needed to install Jaynes: {e}"))?;
    if !status.success() {
        return Err("cargo could not build the newest Jaynes".into());
    }
    said.push(format!("Jaynes installed in {}", root.join("bin").display()));
    // The plugin, through Claude Code's own commands, so the skill, the guard and this server match the program.
    let plugin = ["marketplace update jaynes", "update jaynes@jaynes"];
    for args in plugin {
        let done = Command::new("claude").arg("plugin").args(args.split(' ')).output();
        match done {
            Ok(d) if d.status.success() => {}
            Ok(d) => {
                said.push(format!("the Jaynes plugin was not updated: {}", String::from_utf8_lossy(&d.stderr).trim()));
                break;
            }
            Err(_) => {
                said.push("the Jaynes plugin was not updated: Claude Code is not on the PATH; update it with /plugin".into());
                break;
            }
        }
    }
    said.extend(update_roles(home));
    Ok(said)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).status().expect("git").success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn a_role_installed_from_git_is_pinned_and_an_update_that_asks_for_more_waits_for_the_person() {
        let base = std::env::temp_dir().join(format!("jaynes-install-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let (repo, home) = (base.join("repo"), base.join("home"));
        let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../roles/small-change");
        copy_dir(&here, &repo.join("r")).expect("copy");
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "one"]);
        let source = format!("file://{}#r", repo.display());
        assert_eq!(add(&home, &source).expect("added"), "small-change");
        let origin: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.join("roles/small-change/.jaynes-origin")).expect("origin")).expect("json");
        assert_eq!(origin["source"], source.as_str());
        assert_eq!(origin["commit"].as_str().map(str::len), Some(40), "the commit is pinned: {origin}");
        assert!(origin["fingerprint"].as_str().is_some_and(|f| f.len() == 64), "{origin}");
        // the next version reaches the network: an update does not install it on its own
        let role = repo.join("r/role.py");
        let text = std::fs::read_to_string(&role).expect("read");
        std::fs::write(&role, text.replace(r#"reach={"project": "write", "write": ["**"], "commands": True},"#, r#"reach={"project": "write", "write": ["**"], "commands": True, "network": "web"},"#)).expect("write");
        git(&repo, &["commit", "-qam", "two"]);
        let said = update_roles(&home).join("\n");
        assert!(said.contains("not updated") && said.contains("network"), "{said}");
        assert!(!std::fs::read_to_string(home.join("roles/small-change/role.py")).expect("read").contains("\"network\": \"web\""));
        // the person installing it by hand accepts it
        add(&home, &source).expect("accepted");
        assert!(std::fs::read_to_string(home.join("roles/small-change/role.py")).expect("read").contains("\"network\": \"web\""));
        // a version that asks for nothing more is updated
        std::fs::write(&role, std::fs::read_to_string(&role).expect("read") + "\n# a comment\n").expect("write");
        git(&repo, &["commit", "-qam", "three"]);
        let said = update_roles(&home).join("\n");
        assert!(said.contains("role small-change updated"), "{said}");
    }
}
