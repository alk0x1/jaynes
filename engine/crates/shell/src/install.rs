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
/// The menu of checks and steps that roles written in TOML pick by name.
const MENU: &str = include_str!("../../../python/jaynes_menu.py");

/// Where Jaynes is published, for `jaynes update` and for the roles that come with it.
pub const SOURCE: &str = "https://github.com/alk0x1/jaynes";

/// The folder holding `jaynes_role.py`: the one named by `JAYNES_ROLE_LIBRARY`, or the program's own copy, written
/// under the home when it is missing or older.
pub fn library(home: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("JAYNES_ROLE_LIBRARY") {
        return PathBuf::from(dir);
    }
    let dir = home.join("library");
    for (name, text) in [("jaynes_role.py", LIBRARY), ("jaynes_box.py", BOX), ("jaynes_menu.py", MENU)] {
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
    if as_path.join(crate::toml_role::FILE).is_file() || as_path.join("role.py").is_file() || as_path.join("role").is_file() {
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
    if let Some(name) = by_name(source) {
        // from the index: its entry was checked and its promises proved again on the way
        let (entry, folder, commit, _gone) = crate::index::find(&name)?;
        // its license is said before anything is installed
        crate::drive::say(&format!("{} {}, license: {}", entry.name, entry.version, entry.license));
        return install(home, &format!("index:{}", entry.name), &folder, commit, true);
    }
    let (folder, commit, _tmp) = fetch(source)?;
    // a role of one's own, from a folder or from git, names no license to Jaynes: it is installed all the same
    crate::drive::say(&format!("{source}: no license named (a role from the index says its license)"));
    install(home, source, &folder, commit, false)
}

/// The name a source gives for the index: a bare name that is no folder, or `index:NAME`; and the official roles'
/// old place in the engine's repository, which moved to the index.
fn by_name(source: &str) -> Option<String> {
    if let Some(n) = source.strip_prefix("index:") {
        return Some(n.to_string());
    }
    if let Some(n) = source.strip_prefix(&format!("{SOURCE}#engine/roles/")) {
        return Some(n.trim_end_matches('/').to_string());
    }
    let plain = !source.is_empty() && source.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    (plain && !Path::new(source).exists()).then(|| source.to_string())
}

/// What a role's steps may reach, one line each, to tell whether a new version asks for more: what its manifest
/// declares, and, for a role in TOML in `dir`, the local ports its checks name, which a check keeps apart from its
/// step.
fn reach_of(m: &jaynes_core::Manifest, dir: &Path) -> std::collections::BTreeSet<String> {
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
        for p in &r.local_ports {
            out.insert(format!("{}: the local port {p}", s.name));
        }
    }
    out.retain(|l| !l.ends_with(", none"));
    if let Ok(role) = crate::toml_role::TomlRole::load(dir) {
        for (step, x) in &role.extras {
            for (check, ports) in &x.check_ports {
                for p in ports {
                    out.insert(format!("{step}: its check {check}, the local port {p}"));
                }
            }
        }
    }
    out
}

/// What a role in TOML promises, by what it means, with its words: to tell whether a new version drops or changes
/// a promise.
fn promises_of(dir: &Path) -> std::collections::BTreeMap<String, String> {
    crate::toml_role::TomlRole::load(dir)
        .map(|r| r.promises.iter().map(|(_, p)| (format!("{:?} {:?}", p.says, p.form), p.says.clone())).collect())
        .unwrap_or_default()
}

/// The promises of the role in `old` that the version in `new` drops or changes, by their words.
fn lost_promises(old: &Path, new: &Path) -> Vec<String> {
    let kept = promises_of(new);
    promises_of(old).into_iter().filter(|(k, _)| !kept.contains_key(k)).map(|(_, says)| format!("\"{says}\"")).collect()
}

fn manifest_of(role: &RoleProgram) -> Result<jaynes_core::Manifest, String> {
    match role.call(crate::protocol::Call::Describe {}) {
        Ok(crate::protocol::Body::Manifest { manifest }) => Ok(manifest),
        other => Err(format!("the role did not describe itself: {other:?}")),
    }
}

/// Update the roles installed from git. One whose new version reaches more than the installed one (the project,
/// writing, the network, commands, a local port) is not updated: the person reads what it asks and installs it with `jaynes add`.
pub fn update_roles(home: &Path) -> Vec<String> {
    let mut said = Vec::new();
    for (name, _, _, origin) in list(home) {
        let named = by_name(&origin).filter(|_| origin.starts_with("index:") || origin.starts_with(SOURCE));
        if named.is_none() && !(origin.starts_with("https://") || origin.starts_with("git@") || origin.starts_with("file://")) {
            continue;
        }
        let attempt = || -> Result<String, String> {
            let (folder, commit, _tmp, _gone, origin) = match &named {
                Some(n) => {
                    let (entry, folder, commit, gone) = crate::index::find(n)?;
                    (folder, commit, None, gone, format!("index:{}", entry.name))
                }
                None => {
                    let (folder, commit, tmp) = fetch(&origin)?;
                    (folder, commit, tmp, Vec::new(), origin.clone())
                }
            };
            let new = manifest_of(&RoleProgram::new(&folder, &library(home)))?;
            let old = manifest_of(&RoleProgram::new(&roles_dir(home).join(&name), &library(home)))?;
            let installed = roles_dir(home).join(&name);
            let more: Vec<String> = reach_of(&new, &folder).difference(&reach_of(&old, &installed)).cloned().collect();
            let lost = lost_promises(&roles_dir(home).join(&name), &folder);
            if !lost.is_empty() {
                return Ok(format!(
                    "role {name} not updated: its new version drops or changes promises ({}). Install it with `jaynes add {origin}` once you accept that",
                    lost.join("; ")
                ));
            }
            if !more.is_empty() {
                return Ok(format!(
                    "role {name} not updated: its new version reaches more ({}). Install it with `jaynes add {origin}` once you accept that",
                    more.join("; ")
                ));
            }
            install(home, &origin, &folder, commit, named.is_some())?;
            Ok(format!("role {name} updated"))
        };
        said.push(attempt().unwrap_or_else(|e| format!("role {name} not updated: {e}")));
    }
    said
}

/// Install the role in `folder`, fetched from `source` at `commit`, once it passes the conformance suite: the
/// commit and the fingerprint of what was installed are kept with it.
fn install(home: &Path, source: &str, folder: &Path, commit: Option<String>, proved: bool) -> Result<String, String> {
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
    // a role in TOML is installed only when every promise it makes is proved again here
    if folder.join(crate::toml_role::FILE).is_file() && !proved {
        let toml = crate::toml_role::TomlRole::load(folder).map_err(|e| format!("the role does not read:\n  {}", e.join("\n  ")))?;
        let (lines, all) = toml.prove();
        if !all {
            return Err(format!("a promise of the role does not hold, so it was not installed:\n{}", lines.join("\n")));
        }
    }
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
/// Readable by its owner alone: whoever reads it answers as the person. A file an earlier version left readable by
/// all is tightened when it is read.
pub fn page_key(home: &Path) -> Result<String, String> {
    secret(&home.join("page.key"))
}

/// A secret of this machine kept in `file`, readable by its owner alone: made of the system's randomness the first
/// time it is asked for, then the same each time.
pub fn secret(file: &Path) -> Result<String, String> {
    if let Ok(k) = std::fs::read_to_string(file)
        && k.trim().len() >= 16 {
            owner_only(file);
            return Ok(k.trim().to_string());
        }
    let k = crate::page::key()?;
    if let Some(home) = file.parent() {
        let _made = std::fs::create_dir_all(home);
    }
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    let _written = open.open(file).and_then(|mut f| std::io::Write::write_all(&mut f, k.as_bytes()));
    owner_only(file);
    Ok(k)
}

/// The file readable and writable by its owner alone.
fn owner_only(file: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _tightened = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600));
    }
}

pub const PORT: u16 = 8750;

/// Start the page in a process of its own if it is not running, and open it in the browser with its key, which is
/// never printed where an agent reads it, unless a tab of the page that holds the key asked it lately: a start shows
/// in the tab already open. The page's address without the key; why the link was not opened, when it was not, goes
/// to standard error.
pub fn ensure_page(home: &Path) -> String {
    let address = format!("http://localhost:{PORT}/");
    if std::env::var("JAYNES_NO_PAGE").is_ok() {
        return address;
    }
    let Ok(exe) = std::env::current_exe() else { return address };
    let open = if std::env::var("JAYNES_NO_BROWSER").is_err() { Open::UnlessATabIsOpen } else { Open::Never };
    match ensure_page_with(home, PORT, &exe, open) {
        Ok(a) => a,
        Err(why) => {
            eprintln!("{why}");
            address
        }
    }
}

/// How the page's two addresses answer.
enum Standing {
    /// The page answers as itself on 127.0.0.1, and on ::1 or nothing does; whether a tab of it that holds the key
    /// asked it lately, as the page says.
    Ours { tab: bool },
    /// Nothing answers.
    Nothing,
    /// Another program answers, on the address said.
    Other(String),
}

/// Whether the page at `port` runs, proving it holds `key` on each of its addresses: localhost may reach either, so
/// another program on one of them would be given the link.
fn standing(port: u16, key: &str) -> Standing {
    // whether the page proved it holds the key, and whether it says a tab with the key is open
    let ask = |address: &str| -> Option<(bool, bool)> {
        let at: std::net::SocketAddr = format!("{}:{port}", if address.contains(':') { format!("[{address}]") } else { address.to_string() }).parse().ok()?;
        let mut s = std::net::TcpStream::connect_timeout(&at, std::time::Duration::from_millis(500)).ok()?;
        let nonce = crate::page::key().unwrap_or_default();
        let _deadline = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
        let said = std::io::Write::write_all(&mut s, format!("GET /api/knows?nonce={nonce} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n").as_bytes());
        let mut reply = String::new();
        let _read = said.ok().map(|()| std::io::Read::read_to_string(&mut s, &mut reply));
        let ours = !nonce.is_empty() && reply.contains(&format!("\"proof\":\"{}\"", crate::page::proof(key, &nonce)));
        Some((ours, ours && reply.contains("\"tab\":true")))
    };
    match (ask("127.0.0.1"), ask("::1")) {
        (_, Some((false, _))) => Standing::Other(format!("another program answers on [::1]:{port}, where the browser may go for localhost")),
        (Some((false, _)), _) => Standing::Other(format!("something that does not prove it holds the page's key answers on 127.0.0.1:{port}: an older page still running, or another program. Stop it and start again")),
        (Some((true, tab)), _) => Standing::Ours { tab },
        (None, _) => Standing::Nothing,
    }
}

/// Whether the page at `port` is this engine's: it proves on its addresses that it holds `key`.
pub fn page_is_ours(port: u16, key: &str) -> bool {
    matches!(standing(port, key), Standing::Ours { .. })
}

/// When the page's link, with its key, is opened in the browser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Open {
    /// Never: the page is only started.
    Never,
    /// Whether the page was up or not, as the app's "Open the page" and `jaynes open` ask.
    Always,
    /// Unless a tab of the page that holds the key asked it lately, as a start asks: a new run then shows there.
    UnlessATabIsOpen,
}

/// The page at `port`, started with `exe` when it is not running, and its link opened with `open` true whether it
/// was running or not, only once the page answers as itself on both of its addresses: the address, or why the link
/// was not opened, which never holds the key. The app's "Open the page" calls it so.
pub fn ensure_page_on(home: &Path, port: u16, exe: &Path, open: bool) -> Result<String, String> {
    if open { ensure_page_at(home, port, exe, "/").map(|_| format!("http://localhost:{port}/")) } else { ensure_page_with(home, port, exe, Open::Never) }
}

/// The page at `port`, started with `exe` when it is not running, and opened at `path` with its key whether it was
/// running or not, as the app opens a stop's live picture: the address at `path`, or why it was not opened, which
/// never holds the key. A path is the page's own: it starts with one "/", and holds no "#" or "?", which would carry
/// another key or a query beside it, and no space or control character; another is refused, and nothing opens.
pub fn ensure_page_at(home: &Path, port: u16, exe: &Path, path: &str) -> Result<String, String> {
    let fits = path.starts_with('/') && !path.starts_with("//") && !path.chars().any(|c| matches!(c, '#' | '?' | '\\') || c.is_whitespace() || c.is_control());
    if !fits {
        return Err(format!("{path:?} is not a path of the page (one \"/\" first, no \"#\", \"?\", \"\\\\\" or space): the page's link was not opened"));
    }
    opened(home, port, exe, Open::Always, path)?;
    Ok(format!("http://localhost:{port}{path}"))
}

/// The page at `port`, started with `exe` when it is not running, and its link opened as `open` says, only once the
/// page answers as itself on both of its addresses: the address, or why the link was not opened, which never holds
/// the key.
pub fn ensure_page_with(home: &Path, port: u16, exe: &Path, open: Open) -> Result<String, String> {
    opened(home, port, exe, open, "/")
}

/// The page at `port`, started with `exe` when it is not running, and its link at `path`, a path of its own, opened
/// as `open` says: the page's address, or why the link was not opened.
fn opened(home: &Path, port: u16, exe: &Path, open: Open, path: &str) -> Result<String, String> {
    let address = format!("http://localhost:{port}/");
    let at = format!("http://localhost:{port}{path}");
    let key = page_key(home)?;
    let not_opened = |why: String| format!("{why}: the page's link was not opened");
    match standing(port, &key) {
        Standing::Ours { tab } => {
            if open == Open::Always || (open == Open::UnlessATabIsOpen && !tab) {
                open_link(&at, &key).map_err(not_opened)?;
            }
            return Ok(address);
        }
        Standing::Other(why) => return Err(not_opened(why)),
        Standing::Nothing => {}
    }
    let log = std::fs::File::create(home.join("page.log")).ok();
    let mut c = Command::new(exe);
    c.args(["page", "--port", &port.to_string()]).env("JAYNES_ENGINE_HOME", home).env_remove("JAYNES_PAGE_KEY").stdin(Stdio::null());
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
    let mut child = c.spawn().map_err(|e| not_opened(format!("the page did not start: {e}")))?;
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let ended = child.try_wait().ok().flatten();
        match standing(port, &key) {
            Standing::Ours { .. } if ended.is_none() => break,
            Standing::Other(why) if ended.is_some() => return Err(not_opened(why)),
            _ if ended.is_some() => return Err(not_opened(format!("the page did not start ({}): see {}", ended.map(|e| e.to_string()).unwrap_or_default(), home.join("page.log").display()))),
            _ if std::time::Instant::now() > until => return Err(not_opened("the page did not answer as itself in time".into())),
            _ => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
    // a page just started has no tab
    if open != Open::Never {
        open_link(&at, &key).map_err(not_opened)?;
    }
    Ok(address)
}

/// Open the page at `address` in the browser with `key`, waiting for the system's opener to say it did: why it did
/// not, without the key, when it did not.
fn open_link(address: &str, key: &str) -> Result<(), String> {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    // the key in the fragment, which the browser keeps for the page and never sends to any server
    let done = Command::new(opener).arg(format!("{address}#key={key}")).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    match done {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("the browser did not open the page ({opener} ended with {s})")),
        Err(e) => Err(format!("the browser could not be asked to open the page ({opener}: {})", e.kind())),
    }
}

/// What an agent reads to carry out a change through Jaynes.
pub const GUIDE: &str = "How to carry out an agreed change through Jaynes

1. Choose a role. `jaynes roles` lists the installed ones. small-change is for a few lines; fix-bug proves a bug
   with a failing test, then fixes it. If none is installed, install them:
     jaynes add small-change
     jaynes add fix-bug
2. Start the run with the Jaynes plugin's MCP tool \"start\" (mcp__plugin_jaynes_jaynes__start): role, the project's
   folder as a full path, the whole agreed plan as the request, a title, and risk \"high\" if the change is not small
   (the run then takes the full path and its ceiling is higher). The title is what the person tells the run by, after
   its project: a few words, up to 40 characters, in their language, saying what changes (\"Login with passkey\"), with
   no date, no one's name and no \"Agreed with…\". The tool runs Jaynes outside your own sandbox, so Jaynes can
   put its box around the project's commands. Only when the tool is not there (no plugin), start it from the
   project's folder with a command, in the background, since it runs agents for minutes:
     jaynes start --role <role> --project . --request \"<the plan, in full>\" --title \"<a few words>\" [--risk high]
   From a shell with a sandbox of its own (as Claude Code's Bash with its sandbox on), Jaynes cannot put its box around
   the project's commands, and the run refuses them: use the tool, or a terminal of the person's.
3. Tell the person that the run goes on by itself and that what needs them is on the page Jaynes opens in their
   browser (http://localhost:8750). Do not answer for them.
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
    cargo_installed_commit().or_else(program_commit)
}

/// The commit cargo installed from git, in its record of what it installed.
fn cargo_installed_commit() -> Option<String> {
    let text = std::fs::read_to_string(install_root().ok()?.join(".crates.toml")).ok()?;
    let line = text.lines().find(|l| l.contains("\"jaynes-shell ") && l.contains('#'))?;
    let after = line.split('#').nth(1)?;
    let commit: String = after.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    (commit.len() >= 7).then_some(commit)
}

/// What the program on disk says it was built from, for one cargo did not install (a release, a build by hand):
/// after an update it is the new one, whatever runs now.
fn program_commit() -> Option<String> {
    commit_of(&program_said()?)
}

/// The version of Jaynes installed now, as the program on disk says it (`jaynes --version`): after an update it is
/// the new one, whatever runs now. None when that program does not say one.
pub fn installed_version() -> Option<String> {
    version_of(&program_said()?)
}

/// What the program on disk says with `--version`. It is asked again only when the file changed, since the page
/// asks on every request.
fn program_said() -> Option<String> {
    static ASKED: std::sync::Mutex<Option<(std::time::SystemTime, Option<String>)>> = std::sync::Mutex::new(None);
    let exe = std::env::current_exe().ok()?;
    let changed = std::fs::metadata(&exe).and_then(|m| m.modified()).ok()?;
    let mut asked = ASKED.lock().ok()?;
    if let Some((at, said)) = asked.as_ref()
        && *at == changed
    {
        return said.clone();
    }
    let said = Command::new(&exe).arg("--version").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    *asked = Some((changed, said.clone()));
    said
}

/// The version `jaynes --version` names ("jaynes 0.2.4 (commit ..."), or None when it names none.
pub fn version_of(said: &str) -> Option<String> {
    let v = said.trim_start().strip_prefix("jaynes ")?.split_whitespace().next()?;
    (v.split('.').count() == 3 && v.split('.').all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))).then(|| v.to_string())
}

/// The commit this program is built from, with "+changes" when its source had changes not committed, or
/// "unknown".
pub const BUILD_COMMIT: &str = env!("JAYNES_BUILD_COMMIT");

/// What `jaynes --version` prints: the version, the commit it is built from, and the engine whose rules it runs.
pub fn version() -> String {
    format!("jaynes {} (commit {}, engine {})", env!("CARGO_PKG_VERSION"), BUILD_COMMIT, jaynes_core::ENGINE)
}

/// The commit `jaynes --version` names, or None for a build that does not know it.
pub fn commit_of(said: &str) -> Option<String> {
    let after = said.split("(commit ").nth(1)?;
    let commit: String = after.chars().take_while(char::is_ascii_hexdigit).collect();
    (commit.len() >= 7).then_some(commit)
}

/// The version of the latest release built for this machine, where the releases are: what `jaynes update` installs.
/// Never the newest commit of main, which no update installs. None when no release could be read or none is built for
/// this machine.
pub fn latest_release() -> Option<String> {
    offered(&manifest(), &target())
}

/// Where the guard in Claude Code keeps whether it is on and the GitHub owners that pass freely.
pub fn guard_path() -> Result<PathBuf, String> {
    let home = match std::env::var("JAYNES_HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => std::env::var("HOME").map(|h| PathBuf::from(h).join(".jaynes")).map_err(|e| e.to_string())?,
    };
    Ok(home.join("guard.json"))
}

/// The guard's settings file, `{}` when it is missing, broken or not an object.
fn guard_json() -> serde_json::Value {
    guard_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}))
}

/// Write the guard's settings whole, through a file renamed into place so the guard never reads half of it.
fn write_guard(v: &serde_json::Value) -> Result<(), String> {
    let path = guard_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// The guard's settings, as the guard reads them: on unless it says otherwise, and its free owners.
pub fn guard() -> (bool, Vec<String>) {
    let v = guard_json();
    let on = v["on"].as_bool().unwrap_or(true);
    let owners = v["owners"].as_array().map(|a| a.iter().filter_map(|o| o.as_str().map(str::to_string)).collect()).unwrap_or_default();
    (on, owners)
}

/// Whether the guard, being on, only watches: what it would ask that can be taken back it notes instead, in
/// [`guard_notes_path`], as the guard reads it.
pub fn guard_watching() -> bool {
    let v = guard_json();
    v["on"].as_bool().unwrap_or(true) && v["watch"].as_bool() == Some(true)
}

/// Turn the guard on or off, and set its free owners; whether it only watches stays as it was.
pub fn set_guard(on: bool, owners: &[String]) -> Result<(), String> {
    let mut v = guard_json();
    v["on"] = serde_json::json!(on);
    v["owners"] = serde_json::json!(owners);
    write_guard(&v)
}

/// Turn the guard on, asking or only watching, in one write.
pub fn set_guard_watch(watch: bool) -> Result<(), String> {
    let mut v = guard_json();
    v["on"] = serde_json::json!(true);
    v["watch"] = serde_json::json!(watch);
    write_guard(&v)
}

/// The folders where the person turned the guard off, as the guard reads them ("off_in").
pub fn guard_off_in() -> Vec<String> {
    guard_json()["off_in"].as_array().map(|a| a.iter().filter_map(|f| f.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// Set the folders where the guard is off; the rest of its settings stay as they were.
pub fn set_guard_off_in(folders: &[String]) -> Result<(), String> {
    let mut v = guard_json();
    v["off_in"] = serde_json::json!(folders);
    write_guard(&v)
}

/// Where the watching guard notes what it would have asked: beside its settings, one JSON line a note.
pub fn guard_notes_path() -> Result<PathBuf, String> {
    Ok(guard_path()?.with_file_name("guard-watch.jsonl"))
}

/// Build and install the newest Jaynes from where it is published, and update the roles that came from git.
/// Where the newest release's files are: GitHub's latest release, unless `JAYNES_RELEASES` names another place.
fn releases() -> String {
    std::env::var("JAYNES_RELEASES").unwrap_or_else(|_| format!("{SOURCE}/releases/latest/download"))
}

/// The script that installs a release: the app, the program, herdr-core when missing, and the plugin.
fn installer() -> String {
    std::env::var("JAYNES_INSTALLER").unwrap_or_else(|_| "https://jaynes.dev/install.sh".to_string())
}

/// The machine this program was built for, as the releases name their builds.
fn target() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH).replace("macos-arm64", "macos-aarch64")
}

/// What the newest release offers this machine.
#[derive(Debug, PartialEq, Eq)]
pub enum Release {
    /// A newer version, built for this machine.
    Newer(String),
    /// The installed program is that version, or ahead of it.
    Current(String),
    /// No build for this machine, or no release could be read.
    None,
}

/// What the release manifest (`latest.json`) offers a machine of `target` whose program is version `have`.
pub fn release(manifest: &str, target: &str, have: &str) -> Release {
    match offered(manifest, target) {
        None => Release::None,
        Some(v) if newer(&v, have) => Release::Newer(v),
        Some(v) => Release::Current(v),
    }
}

/// The version the release manifest offers a machine of `target`, when it has a build for it.
fn offered(manifest: &str, target: &str) -> Option<String> {
    let m = serde_json::from_str::<serde_json::Value>(manifest).ok()?;
    m["targets"][target]["url"].as_str()?;
    m["version"].as_str().map(str::to_string)
}

/// Whether version `version` is newer than `have`, number by number.
pub fn newer(version: &str, have: &str) -> bool {
    let parts = |v: &str| v.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parts(version) > parts(have)
}

/// The latest release's manifest, read where the releases are; empty when it could not be read.
fn manifest() -> String {
    let read = Command::new("curl").args(["-fsSL", &format!("{}/latest.json", releases())]).stderr(Stdio::null()).output();
    read.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// Whether the app is where install.sh or the person put it.
pub fn has_app(home_dir: &Path) -> bool {
    home_dir.join("Applications/Jaynes.app").is_dir() || Path::new("/Applications/Jaynes.app").is_dir()
}

pub fn update(home: &Path) -> Result<Vec<String>, String> {
    let mut said = Vec::new();
    let manifest = manifest();
    // a program updated from source, as 0.1.0 updated itself, came without the app: the release brings it
    let home_dir = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let app_missing = target() == "macos-aarch64" && !has_app(&home_dir);
    match release(&manifest, &target(), env!("CARGO_PKG_VERSION")) {
        Release::Current(v) if !app_missing => said.push(format!("Jaynes {v} is the newest; this one is {}", version())),
        Release::Newer(v) | Release::Current(v) => {
            // the release's own installer, as the person ran it: the app and the program together
            let status = Command::new("sh")
                .args(["-c", &format!("curl -fsSL '{}' | sh", installer())])
                .env("JAYNES_RELEASES", releases())
                .status()
                .map_err(|e| format!("the installer could not run: {e}"))?;
            if !status.success() {
                return Err(format!("Jaynes {v} could not be installed"));
            }
            said.push(format!("Jaynes {v} installed"));
        }
        // no build for this machine: cargo builds the newest source
        Release::None => {
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
        }
    }
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

    /// Runs the installer's part that checks where the download was built (between its markers in site/install.sh)
    /// on `file` of `version`, with only `bin` on the PATH: whether it went on, and what it said.
    fn provenance(bin: &Path, file: &Path, version: &str) -> (bool, String) {
        let script = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../site/install.sh")).expect("install.sh");
        let part = script.split("# --- provenance: begin").nth(1).and_then(|s| s.split("# --- provenance: end").next()).expect("the provenance's part");
        assert!(
            script.contains("[ \"$(sha256 \"$tmp/jaynes.tar.gz\")\" = \"$want\" ] || fail \"the download is damaged (checksum mismatch); try again\"\nprovenance \"$tmp/jaynes.tar.gz\" \"$version\"\n"),
            "the installer checks the provenance of what it downloaded, right after its checksum"
        );
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("set -eu\nsay() {{ printf '%s\\n' \"$*\"; }}\nfail() {{ say \"jaynes: $*\" >&2; exit 1; }}\n{part}\nprovenance \"$1\" \"$2\""))
            .arg("sh")
            .arg(file)
            .arg(version)
            .env("PATH", bin)
            .env_remove("JAYNES_REPO")
            .output()
            .expect("sh");
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    #[test]
    fn the_installer_checks_where_the_download_was_built_with_gh_and_says_when_it_could_not() {
        let base = std::env::temp_dir().join(format!("jaynes-provenance-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let file = base.join("jaynes.tar.gz");
        std::fs::create_dir_all(&base).expect("dir");
        std::fs::write(&file, "the release").expect("write");
        // a stand-in for GitHub's gh: whether it is logged in, what attestation verify says, and the arguments it got
        let gh = |name: &str, logged_in: bool, verified: bool| -> PathBuf {
            let bin = base.join(name);
            std::fs::create_dir_all(&bin).expect("dir");
            let script = format!(
                "#!/bin/sh\ncase \"$1 $2\" in\n  \"auth status\") exit {};;\n  \"attestation verify\") printf '%s\\n' \"$*\" > {}/args; {};;\nesac\nexit 2\n",
                if logged_in { 0 } else { 1 },
                bin.display(),
                if verified { "exit 0" } else { "echo 'X Verification failed'; echo 'no attestation matched the policy' >&2; exit 1" },
            );
            std::fs::write(bin.join("gh"), script).expect("write");
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join("gh"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
            bin
        };
        // no gh: the checksum is all that was checked, and the installer says so and goes on
        let none = base.join("none");
        std::fs::create_dir_all(&none).expect("dir");
        let (ok, said) = provenance(&none, &file, "1.2.3");
        assert!(ok && said.contains("provenance was not checked") && said.contains("gh is not installed"), "{said}");
        // gh without a login: the same, saying how to log in
        let (ok, said) = provenance(&gh("out", false, true), &file, "1.2.3");
        assert!(ok && said.contains("provenance was not checked") && said.contains("gh auth login"), "{said}");
        // gh logged in, the attestation holds: said, after checking the repository, its release workflow, the tag of the
        // version and GitHub's own runners
        let bin = gh("holds", true, true);
        let (ok, said) = provenance(&bin, &file, "1.2.3");
        assert!(ok && said.contains("provenance was checked") && said.contains("v1.2.3"), "{said}");
        let args = std::fs::read_to_string(bin.join("args")).expect("verify was asked");
        for want in [
            format!("attestation verify {}", file.display()),
            "--repo alk0x1/jaynes".to_string(),
            "--signer-workflow alk0x1/jaynes/.github/workflows/release.yml".to_string(),
            "--source-ref refs/tags/v1.2.3".to_string(),
            "--deny-self-hosted-runners".to_string(),
        ] {
            assert!(args.contains(&want), "{want} not in {args}");
        }
        // gh logged in, the attestation does not hold: the install stops, saying why in gh's words
        let (ok, said) = provenance(&gh("fails", true, false), &file, "1.2.3");
        assert!(!ok && said.contains("provenance does not hold") && said.contains("no attestation matched the policy"), "{said}");
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_guide_starts_a_run_with_the_plugin_s_tool_before_the_command() {
        // the tool runs Jaynes outside the agent's own sandbox, where its box opens; the command, from a sandboxed
        // shell, does not
        let tool = GUIDE.find("mcp__plugin_jaynes_jaynes__start").expect("the guide names the tool");
        let command = GUIDE.find("jaynes start --role").expect("and the command, for when there is no plugin");
        assert!(tool < command, "the tool comes first");
        assert!(GUIDE.contains("Only when the tool is not there"), "{GUIDE}");
    }

    #[test]
    fn the_version_names_the_commit_it_is_built_from() {
        let v = version();
        assert!(v.starts_with(&format!("jaynes {} (commit ", env!("CARGO_PKG_VERSION"))), "{v}");
        assert!(v.contains(jaynes_core::ENGINE), "{v}");
        // built from a git checkout it knows its commit; from a source without git, it says it does not
        assert!(commit_of(&v).is_some() || BUILD_COMMIT == "unknown", "{v}");
        assert_eq!(commit_of("jaynes 0.2.0 (commit 3bf0783abc+changes, engine e)").as_deref(), Some("3bf0783abc"));
        assert_eq!(commit_of("jaynes 0.2.0 (commit unknown, engine e)"), None);
    }

    #[test]
    fn the_version_said_is_read_back_as_the_installed_one() {
        assert_eq!(version_of(&version()).as_deref(), Some(env!("CARGO_PKG_VERSION")));
        assert_eq!(version_of("jaynes 0.2.4 (commit 3bf0783abc, engine e)").as_deref(), Some("0.2.4"));
        // an older program that said something else, or nothing, names no version
        for said in ["", "jaynes", "jaynes the engine", "jaynes 0.2 (commit a)", "jaynes 0.2.x (commit a)", "jaynes-shell 0.2.4"] {
            assert_eq!(version_of(said), None, "{said}");
        }
    }

    #[test]
    fn a_release_is_installed_only_when_it_is_newer_and_built_for_this_machine() {
        let manifest = r#"{"version":"0.2.0","targets":{"macos-aarch64":{"url":"https://x/jaynes-macos-aarch64.tar.gz","sha256":"ab"}}}"#;
        assert_eq!(release(manifest, "macos-aarch64", "0.1.9"), Release::Newer("0.2.0".into()));
        assert_eq!(release(manifest, "macos-aarch64", "0.2.0"), Release::Current("0.2.0".into()));
        assert_eq!(release(manifest, "macos-aarch64", "0.10.0"), Release::Current("0.2.0".into()), "a build from source ahead of the release stays");
        assert_eq!(release(manifest, "linux-x86_64", "0.1.0"), Release::None, "no build for this machine: cargo builds it");
        assert_eq!(release("not json", "macos-aarch64", "0.1.0"), Release::None);
    }

    #[test]
    fn a_name_goes_to_the_index_and_the_official_roles_old_place_moves_there() {
        assert_eq!(by_name("fix-bug").as_deref(), Some("fix-bug"));
        assert_eq!(by_name("index:small-change").as_deref(), Some("small-change"));
        assert_eq!(by_name(&format!("{SOURCE}#engine/roles/fix-bug")).as_deref(), Some("fix-bug"));
        assert_eq!(by_name("https://example.com/r#roles/x"), None);
        assert_eq!(by_name("."), None);
        assert_eq!(by_name("src"), None, "a folder that exists is a folder");
    }

    #[test]
    fn an_update_that_drops_or_changes_a_promise_is_told_by_its_words() {
        let base = std::env::temp_dir().join(format!("jaynes-promises-{}", std::process::id()));
        let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roles/small-change");
        let (old, new) = (base.join("old"), base.join("new"));
        let _gone = std::fs::remove_dir_all(&base);
        copy_dir(&here, &old).expect("copy");
        copy_dir(&here, &new).expect("copy");
        assert!(lost_promises(&old, &new).is_empty());
        let text = std::fs::read_to_string(new.join("role.toml")).expect("read");
        std::fs::write(new.join("role.toml"), text.replace("never = \"deliver\"\nunless = \"check suite-passes of change passed\"", "never = \"deliver\"\nunless = \"step runner done\"")).expect("write");
        assert_eq!(lost_promises(&old, &new), ["\"Nothing is delivered before the project's suite passed on the change\""]);
    }

    #[test]
    fn an_update_whose_step_or_check_reaches_a_new_local_port_asks_for_more() {
        let base = std::env::temp_dir().join(format!("jaynes-ports-more-{}", std::process::id()));
        let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roles/small-change");
        let (old, new) = (base.join("old"), base.join("new"));
        let _gone = std::fs::remove_dir_all(&base);
        copy_dir(&here, &old).expect("copy");
        copy_dir(&here, &new).expect("copy");
        let reach = |dir: &Path| {
            let role = crate::toml_role::TomlRole::load(dir).unwrap_or_else(|e| panic!("{}", e.join("\n")));
            reach_of(&role.manifest, dir)
        };
        assert!(reach(&new).difference(&reach(&old)).next().is_none());
        // a check of the new version names a local port
        let text = std::fs::read_to_string(new.join("role.toml")).expect("read");
        assert!(text.contains("\"suite-passes\""), "{text}");
        std::fs::write(new.join("role.toml"), text.replacen("\"suite-passes\"", "{ use = \"suite-passes\", local_ports = [5432] }", 1)).expect("write");
        let more: Vec<String> = reach(&new).difference(&reach(&old)).cloned().collect();
        assert_eq!(more, ["change: its check suite-passes, the local port 5432"]);
        // a step of the new version names one
        std::fs::create_dir_all(new.join("steps")).expect("dir");
        std::fs::write(new.join("steps/ping.sh"), "").expect("write");
        let text = std::fs::read_to_string(new.join("role.toml")).expect("read");
        std::fs::write(new.join("role.toml"), format!("{text}\n[[step]]\nname = \"ping\"\nby = \"role\"\nscript = \"steps/ping.sh\"\nneeds = [\"change\"]\nreach = {{ local_ports = [8125] }}\neffect = \"none\"\n")).expect("write");
        let more: Vec<String> = reach(&new).difference(&reach(&old)).cloned().collect();
        assert!(more.contains(&"ping: the local port 8125".to_string()), "{more:?}");
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_role_installed_from_git_is_pinned_and_an_update_that_asks_for_more_waits_for_the_person() {
        let base = std::env::temp_dir().join(format!("jaynes-install-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let (repo, home) = (base.join("repo"), base.join("home"));
        // a role in Python: one in TOML is served by the jaynes program, which a unit test is not
        std::fs::create_dir_all(repo.join("r")).expect("dir");
        std::fs::write(repo.join("r/role.py"), concat!(
            "from jaynes_role import Role\n",
            "role = Role(\"small-change\", version=\"1.0.0\")\n",
            "role.step(\"change\", executor=\"agent\", reach={\"project\": \"write\", \"write\": [\"**\"], \"commands\": True},\n",
            "          instructions=\"Make the change.\")\n",
            "if __name__ == \"__main__\":\n    role.serve()\n",
        )).expect("write");
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
