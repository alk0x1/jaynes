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
    let (folder, _tmp) = fetch(source)?;
    let role = RoleProgram::new(&folder, &library(home));
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
    copy_dir(&folder, &into)?;
    std::fs::write(into.join(".jaynes-origin"), source).map_err(|e| e.to_string())?;
    Ok(name)
}

/// The folder a source names, fetched from git into a temporary folder when it is a URL.
fn fetch(source: &str) -> Result<(PathBuf, Option<PathBuf>), String> {
    if !(source.starts_with("https://") || source.starts_with("git@") || source.starts_with("file://")) {
        let p = std::fs::canonicalize(source).map_err(|e| format!("{source}: {e}"))?;
        return Ok((p, None));
    }
    let (url, sub) = source.split_once('#').unwrap_or((source, ""));
    let tmp = std::env::temp_dir().join(format!("jaynes-add-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&tmp);
    let out = Command::new("git").args(["clone", "-q", "--depth", "1", url]).arg(&tmp).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("git could not fetch {url}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok((tmp.join(sub), Some(tmp)))
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

/// Build and install the newest Jaynes from where it is published, and update the roles that came from git.
pub fn update(home: &Path) -> Result<Vec<String>, String> {
    let mut said = Vec::new();
    let root = std::env::var("HOME").map(|h| PathBuf::from(h).join(".local")).map_err(|e| e.to_string())?;
    let status = Command::new("cargo")
        .args(["install", "--locked", "--quiet", "--git", SOURCE, "jaynes-shell", "--root"])
        .arg(&root)
        .status()
        .map_err(|e| format!("cargo is needed to install Jaynes: {e}"))?;
    if !status.success() {
        return Err("cargo could not build the newest Jaynes".into());
    }
    said.push(format!("Jaynes installed in {}", root.join("bin").display()));
    for (name, _, _, origin) in list(home) {
        if origin.starts_with("https://") || origin.starts_with("git@") {
            match add(home, &origin) {
                Ok(_) => said.push(format!("role {name} updated")),
                Err(e) => said.push(format!("role {name} not updated: {e}")),
            }
        }
    }
    Ok(said)
}
