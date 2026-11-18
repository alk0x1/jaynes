//! `jaynes uninstall`: take away what the installer put on this machine, and say what stays in each repository.
//!
//! It shows everything first and removes only on the person's yes (a terminal's answer, or `--yes`). It removes
//! nothing while something of Jaynes still works: the app open, or a run's agent alive. It stops this engine's page,
//! then removes the Claude Code plugin first (it is what turns the guard on, which would otherwise call a program that
//! is gone), then the app, a herdr-core only when Jaynes installed it and it is unchanged and not running, the data
//! folder only when asked apart, and the program last. A repository is the person's: what runs left there (their
//! `jaynes/*` branches, `refs/jaynes/*`) is listed with the command that removes it, never removed. What the
//! system keeps of the app (its permissions) is said as a line for the person to run, never run.
//!
//! What runs is read from /proc on Linux and asked of lsof elsewhere. Where neither can say it (no /proc, and no lsof),
//! nothing is removed and the person reads why: never "nothing runs" by not knowing.

use crate::install;
use crate::store::Store;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where things are, read from the environment as the rest of Jaynes reads it.
pub struct Places {
    pub home_dir: PathBuf,
    /// The data folder, `$JAYNES_HOME` or `~/.jaynes`, where the guard's settings and the engine's folder live.
    pub data: PathBuf,
    pub engine: PathBuf,
    pub install_root: PathBuf,
    pub apps: Vec<PathBuf>,
    /// The app's folder name in those places: Jaynes.app, or another name a test gives so that it never makes a
    /// folder macOS takes for an app.
    pub app_name: String,
    pub page_port: u16,
}

impl Places {
    pub fn from_env() -> Result<Places, String> {
        let home_dir = PathBuf::from(std::env::var("HOME").map_err(|e| e.to_string())?);
        let data = std::env::var("JAYNES_HOME").map(PathBuf::from).unwrap_or_else(|_| home_dir.join(".jaynes"));
        let apps = match std::env::var("JAYNES_APP_DIRS") {
            Ok(v) => std::env::split_paths(&v).collect(),
            Err(_) => vec![PathBuf::from("/Applications"), home_dir.join("Applications")],
        };
        let page_port = std::env::var("JAYNES_PAGE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(install::PORT);
        let app_name = std::env::var("JAYNES_APP_NAME").unwrap_or_else(|_| "Jaynes.app".into());
        Ok(Places { engine: crate::home(), install_root: install::install_root()?, data, home_dir, apps, app_name, page_port })
    }
}

/// How the program was installed: by cargo, which keeps a list of what it installed, or as a release's file.
#[derive(Debug, PartialEq)]
pub enum Program {
    Cargo { root: PathBuf, file: PathBuf },
    File(PathBuf),
    None,
}

/// What becomes of herdr-core, which keeps the app's terminals and may be the person's own.
#[derive(Debug, PartialEq)]
pub enum Herdr {
    /// Jaynes installed it, it is unchanged and nothing runs it: it goes.
    Goes(PathBuf),
    /// It stays, and why, with what the person may run later.
    Stays(PathBuf, String),
    None,
}

/// A repository runs worked on, and what they left in it.
pub struct Left {
    pub project: PathBuf,
    pub refs: Vec<String>,
}

pub struct Plan {
    /// Why nothing may be removed now, each with what to do.
    pub blockers: Vec<String>,
    pub page: Option<Vec<u32>>,
    pub claude: bool,
    pub apps: Vec<PathBuf>,
    pub bundle_ids: Vec<String>,
    pub program: Program,
    pub leftovers: Vec<PathBuf>,
    pub herdr: Herdr,
    /// The data folders (the data folder, and the engine's when it is elsewhere), with their parts' sizes.
    pub data: Vec<(PathBuf, Vec<(String, u64)>)>,
    pub repositories: Vec<Left>,
}

fn size(path: &Path) -> u64 {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::read_dir(path).map(|d| d.flatten().map(|e| size(&e.path())).sum()).unwrap_or(0),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

fn human(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} bytes"),
    }
}

fn sha256(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect())
}

/// /proc, where Linux says what runs; None where there is none to read.
fn proc_root() -> Option<&'static Path> {
    let proc = Path::new("/proc");
    (cfg!(target_os = "linux") && proc.join("self/exe").exists()).then_some(proc)
}

/// The process ids lsof prints with `args`; Err when lsof could not run, which is not knowing, never "none".
fn lsof(args: &[&std::ffi::OsStr]) -> Result<Vec<u32>, String> {
    let out = Command::new("lsof").args(args).stderr(Stdio::null()).output().map_err(|e| format!("lsof could not run ({e})"))?;
    Ok(String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect())
}

/// The process ids in `proc`, each with its folder there.
fn pids_in(proc: &Path) -> Result<Vec<(u32, PathBuf)>, String> {
    let entries = std::fs::read_dir(proc).map_err(|e| format!("{} could not be read ({e})", proc.display()))?;
    let mut pids: Vec<(u32, PathBuf)> = entries.flatten().filter_map(|e| Some((e.file_name().to_str()?.parse().ok()?, e.path()))).collect();
    pids.sort_unstable();
    Ok(pids)
}

/// The processes /proc says run `file` as their program: those whose exe is that very file, by its device and inode,
/// whatever path started it. A process whose exe this user may not read (another user's) is passed over, as lsof
/// does for whoever is not root.
fn running_in(proc: &Path, file: &Path) -> Result<Vec<u32>, String> {
    use std::os::unix::fs::MetadataExt;
    let want = std::fs::metadata(file).map_err(|e| format!("{} could not be read ({e})", file.display()))?;
    Ok(pids_in(proc)?
        .into_iter()
        .filter(|(_, dir)| std::fs::metadata(dir.join("exe")).is_ok_and(|m| m.dev() == want.dev() && m.ino() == want.ino()))
        .map(|(pid, _)| pid)
        .collect())
}

/// The processes /proc says listen on TCP `port`: the LISTEN sockets of net/tcp and net/tcp6 on that port, and the
/// processes holding one of them open.
fn listening_in(proc: &Path, port: u16) -> Result<Vec<u32>, String> {
    let mut sockets = std::collections::HashSet::new();
    let mut tables = 0;
    for table in ["net/tcp", "net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(proc.join(table)) else { continue };
        tables += 1;
        for line in text.lines().skip(1) {
            // sl, local address as HEX:PORT, remote address, state (0A is LISTEN), ..., inode as the tenth field
            let f: Vec<&str> = line.split_whitespace().collect();
            let on = f.get(1).and_then(|a| a.rsplit(':').next()).and_then(|p| u16::from_str_radix(p, 16).ok());
            if f.get(3) == Some(&"0A") && on == Some(port)
                && let Some(inode) = f.get(9)
            {
                sockets.insert(format!("socket:[{inode}]"));
            }
        }
    }
    if tables == 0 {
        return Err(format!("{} has no TCP table to read", proc.display()));
    }
    Ok(pids_in(proc)?
        .into_iter()
        .filter(|(_, dir)| {
            std::fs::read_dir(dir.join("fd")).is_ok_and(|fds| {
                fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|to| to.to_str().is_some_and(|t| sockets.contains(t))))
            })
        })
        .map(|(pid, _)| pid)
        .collect())
}

/// The processes that run `file` as their program, found by the file itself, not by a name that may lie: in /proc on
/// Linux, by lsof elsewhere. Err when neither can tell.
fn running(file: &Path) -> Result<Vec<u32>, String> {
    if !file.exists() {
        return Ok(Vec::new());
    }
    match proc_root() {
        Some(proc) => running_in(proc, file),
        None => lsof(&["-t".as_ref(), "--".as_ref(), file.as_os_str()]).map_err(|e| format!("there is no /proc here, and {e}")),
    }
}

/// The processes that listen on TCP `port`, in /proc on Linux, by lsof elsewhere. Err when neither can tell.
fn listening(port: u16) -> Result<Vec<u32>, String> {
    match proc_root() {
        Some(proc) => listening_in(proc, port),
        None => {
            let on = format!("-iTCP:{port}");
            lsof(&["-t".as_ref(), "-nP".as_ref(), on.as_ref(), "-sTCP:LISTEN".as_ref()]).map_err(|e| format!("there is no /proc here, and {e}"))
        }
    }
}

/// Nothing may be removed when what runs cannot be told: the blocker that says why and what to do, once.
fn cannot_tell(blockers: &mut Vec<String>, why: &str) {
    let said = format!("Jaynes cannot tell what of it still runs on this machine ({why}), so it removes nothing: install lsof (on Debian or Ubuntu: sudo apt install lsof), then uninstall again.");
    if !blockers.contains(&said) {
        blockers.push(said);
    }
}

/// Send `sig` to `who` (a pid, or -PGID for a group) with the kill program: whether it was taken. Err when kill could
/// not run, which is not knowing, never "it is gone".
fn signal(sig: &str, who: &str) -> Result<bool, String> {
    Command::new("kill").args([sig, "--", who]).stderr(Stdio::null()).status().map(|s| s.success()).map_err(|e| format!("kill could not run ({e})"))
}

/// Whether the process group a pidfile names is alive; Err when that cannot be told.
fn group_alive(pidfile: &Path) -> Result<bool, String> {
    let Some(pgid) = std::fs::read_to_string(pidfile).ok().and_then(|t| t.trim().parse::<u32>().ok()) else { return Ok(false) };
    signal("-0", &format!("-{pgid}"))
}

fn plist_value(plist: &Path, key: &str) -> Option<String> {
    let out = Command::new("plutil").args(["-extract", key, "raw", "-o", "-"]).arg(plist).stderr(Stdio::null()).output().ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !v.is_empty()).then_some(v)
}

/// What uninstalling would do now, found without changing anything.
pub fn plan(at: &Places) -> Plan {
    plan_with(at, &|file| running(file).map(|pids| !pids.is_empty()), &listening)
}

/// The plan, with `runs` telling whether a program file is running and `listens` which processes listen on a port:
/// /proc or lsof in use, stand-ins in the tests. Err from either is not knowing, and nothing is removed.
pub fn plan_with(at: &Places, runs: &dyn Fn(&Path) -> Result<bool, String>, listens: &dyn Fn(u16) -> Result<Vec<u32>, String>) -> Plan {
    let mut blockers = Vec::new();
    // the app, and what of it the system keeps
    let apps: Vec<PathBuf> = at.apps.iter().map(|d| d.join(&at.app_name)).filter(|a| a.is_dir()).collect();
    let mut bundle_ids = Vec::new();
    for app in &apps {
        let info = app.join("Contents/Info.plist");
        let exe = plist_value(&info, "CFBundleExecutable").unwrap_or_else(|| "Jaynes".into());
        match runs(&app.join("Contents/MacOS").join(exe)) {
            Ok(true) => blockers.push(format!("The Jaynes app is open ({}). Quit the Jaynes app, then uninstall again.", app.display())),
            Ok(false) => {}
            Err(why) => cannot_tell(&mut blockers, &why),
        }
        for plist in std::iter::once(info).chain(nested_plists(app)) {
            if let Some(id) = plist_value(&plist, "CFBundleIdentifier")
                && !bundle_ids.contains(&id)
            {
                bundle_ids.push(id);
            }
        }
    }
    // runs whose agent or role program is alive, whatever the record says
    if let Ok(dirs) = std::fs::read_dir(at.engine.join("runs")) {
        let mut busy: Vec<String> = Vec::new();
        for d in dirs.flatten() {
            for f in ["agent.pid", "role.pid"] {
                match group_alive(&d.path().join(f)) {
                    Ok(true) => busy.push(d.file_name().to_string_lossy().to_string()),
                    Ok(false) => {}
                    Err(why) => cannot_tell(&mut blockers, &why),
                }
            }
        }
        busy.sort();
        busy.dedup();
        for run in busy {
            blockers.push(format!("Run {run} is still working. Halt it first: jaynes halt {run}"));
        }
    }
    // this engine's page: the one that proves it holds this engine's key, never another program on the port
    let key = std::fs::read_to_string(at.engine.join("page.key")).map(|k| k.trim().to_string()).unwrap_or_default();
    let page = if !key.is_empty() && install::page_is_ours(at.page_port, &key) {
        match listens(at.page_port) {
            // its processes found, and the kill program there to stop them and to tell that they stopped
            Ok(pids) if !pids.is_empty() => match pids.first().map(|p| signal("-0", &p.to_string())) {
                Some(Err(why)) => {
                    cannot_tell(&mut blockers, &why);
                    None
                }
                _ => Some(pids),
            },
            // it answers as this engine's page, yet no process is seen listening: not knowing which to stop
            Ok(_) => {
                cannot_tell(&mut blockers, &format!("the page answers on port {}, and no process is seen listening there", at.page_port));
                None
            }
            Err(why) => {
                cannot_tell(&mut blockers, &why);
                None
            }
        }
    } else {
        None
    };
    // the program
    let bin = at.install_root.join("bin");
    let file = bin.join("jaynes");
    let crates = std::fs::read_to_string(at.install_root.join(".crates.toml")).unwrap_or_default();
    let program = if crates.lines().any(|l| l.trim_start().starts_with("\"jaynes-shell ")) {
        Program::Cargo { root: at.install_root.clone(), file }
    } else if file.exists() {
        Program::File(file)
    } else {
        Program::None
    };
    let leftovers = [bin.join("jaynes.new")].into_iter().filter(|p| p.exists()).collect();
    // herdr-core, by the installer's note (one note a file, in installed/), read before anything may delete its folder
    let (herdr, untold) = herdr_of(&at.engine.join("installed/herdr-core.json"), &bin.join("herdr-core"), runs);
    if let Some(why) = untold {
        cannot_tell(&mut blockers, &why);
    }
    // the data
    let mut data = Vec::new();
    if at.data.is_dir() {
        data.push((at.data.clone(), parts(&at.data, &at.engine)));
    }
    if at.engine.is_dir() && !at.engine.starts_with(&at.data) {
        data.push((at.engine.clone(), parts(&at.engine, &at.engine)));
    }
    Plan {
        blockers,
        page,
        claude: on_path("claude"),
        apps,
        bundle_ids,
        program,
        leftovers,
        herdr,
        data,
        repositories: repositories(&at.engine),
    }
}

/// Whether a program of that name is on the PATH, found without running it.
fn on_path(name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| std::fs::metadata(d.join(name)).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)))
        .unwrap_or(false)
}

fn nested_plists(app: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for place in ["Contents/Helpers", "Contents/Library/LoginItems", "Contents/Resources"] {
        if let Ok(d) = std::fs::read_dir(app.join(place)) {
            found.extend(d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "app")).map(|p| p.join("Contents/Info.plist")));
        }
    }
    found
}

/// What becomes of herdr-core, and why whether it runs could not be told, when it could not.
fn herdr_of(marker: &Path, default: &Path, runs: &dyn Fn(&Path) -> Result<bool, String>) -> (Herdr, Option<String>) {
    let said: serde_json::Value = std::fs::read_to_string(marker).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let path = said["path"].as_str().map(PathBuf::from);
    let Some(path) = path else {
        return (if default.exists() {
            Herdr::Stays(default.to_path_buf(), "Jaynes did not install it (no installer's note says so), so it may be yours or Herdr's".into())
        } else {
            Herdr::None
        }, None);
    };
    if !path.exists() {
        return (Herdr::None, None);
    }
    if sha256(&path).as_deref() != said["sha256"].as_str() {
        return (Herdr::Stays(path, "it changed since Jaynes installed it (updated by Herdr, or replaced), so it is no longer Jaynes's to remove".into()), None);
    }
    match runs(&path) {
        Ok(true) => {
            let p = path.display().to_string();
            (Herdr::Stays(path, format!("it is running and holds your live terminals. When your sessions can stop: hc server stop && rm {p}")), None)
        }
        Ok(false) => (Herdr::Goes(path), None),
        Err(why) => (Herdr::Stays(path, "Jaynes cannot tell whether it runs".into()), Some(why)),
    }
}

fn parts(folder: &Path, engine: &Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut add = |label: String, path: PathBuf| {
        let s = size(&path);
        if path.exists() {
            out.push((label, s));
        }
    };
    if engine.starts_with(folder) {
        add("runs (each run's copy of its project)".into(), engine.join("runs"));
        add("the record of every run (record.sqlite3)".into(), engine.join("record.sqlite3"));
        add("the roles and their proofs".into(), engine.join("roles"));
    }
    // what the earlier Python engine left at the data folder's top
    for f in ["jaynes.sqlite3", "jaynes.sqlite3-wal", "jaynes.sqlite3-shm", "backups", "objects"] {
        add(format!("the earlier engine's {f}"), folder.join(f));
    }
    add("everything".into(), folder.to_path_buf());
    out
}

fn repositories(engine: &Path) -> Vec<Left> {
    let Ok(store) = Store::open(&engine.join("record.sqlite3")) else { return Vec::new() };
    let mut projects: Vec<PathBuf> = store
        .runs()
        .unwrap_or_default()
        .iter()
        .filter_map(|r| store.setup(r).ok())
        .filter_map(|s| s["project"].as_str().map(PathBuf::from))
        .collect();
    projects.sort();
    projects.dedup();
    projects
        .into_iter()
        .filter(|p| p.join(".git").exists())
        .filter_map(|project| {
            let refs = crate::project::left_by_runs(&project);
            (!refs.is_empty()).then_some(Left { project, refs })
        })
        .collect()
}

/// What the plan says, for the person to read before saying yes.
pub fn describe(plan: &Plan) -> Vec<String> {
    let mut said = vec!["Uninstalling Jaynes removes:".to_string()];
    if plan.page.is_some() {
        said.push("- the page, which is running: it is stopped first".into());
    }
    said.push(if plan.claude {
        "- the Claude Code plugin (skill, guard, MCP server), with Claude Code's own commands: claude plugin uninstall jaynes@jaynes, claude plugin marketplace remove jaynes".into()
    } else {
        "- the Claude Code plugin: Claude Code is not on the PATH, so remove it there with /plugin (jaynes@jaynes)".into()
    });
    for app in &plan.apps {
        said.push(format!("- the app, {}", app.display()));
    }
    match &plan.herdr {
        Herdr::Goes(p) => said.push(format!("- herdr-core, {}, which Jaynes installed", p.display())),
        Herdr::Stays(p, why) => said.push(format!("- not herdr-core, {}: {why}", p.display())),
        Herdr::None => {}
    }
    match &plan.program {
        Program::Cargo { root, file } => said.push(format!("- the program, {}, with cargo uninstall jaynes-shell --root {}", file.display(), root.display())),
        Program::File(f) => said.push(format!("- the program, {}", f.display())),
        Program::None => {}
    }
    for f in &plan.leftovers {
        said.push(format!("- {}, left by an install that did not finish", f.display()));
    }
    for (folder, parts) in &plan.data {
        let all = parts.last().map(|(_, s)| *s).unwrap_or(0);
        said.push(format!("Asked apart, since it holds your history: the data folder {} ({}):", folder.display(), human(all)));
        for (label, s) in &parts[..parts.len().saturating_sub(1)] {
            said.push(format!("    {label}: {}", human(*s)));
        }
    }
    if !plan.repositories.is_empty() {
        said.push("What stays in your repositories, which uninstalling never touches (remove it yourself if you want):".into());
        for left in &plan.repositories {
            let p = left.project.display();
            let (branches, refs): (Vec<&String>, Vec<&String>) = left.refs.iter().partition(|r| r.starts_with("refs/heads/"));
            if !branches.is_empty() {
                let names: Vec<&str> = branches.iter().map(|b| b.trim_start_matches("refs/heads/")).collect();
                said.push(format!("    {p}: branches {}. To remove: git -C {p} branch -D {}", names.join(", "), names.join(" ")));
            }
            for r in refs {
                said.push(format!("    {p}: {r}. To remove: git -C {p} update-ref -d {r}"));
            }
        }
        said.push("    The commits runs merged stay in your history, with Jaynes as their author.".into());
    }
    said
}

/// What stays with the system or was never changed, said once everything went.
pub fn after(plan: &Plan) -> Vec<String> {
    let mut said = Vec::new();
    for id in &plan.bundle_ids {
        said.push(format!("macOS keeps the permissions it gave {id} (Accessibility and others). To clear them: tccutil reset Accessibility {id}, or System Settings › Privacy & Security."));
    }
    said.push("The installer never changed your shell's PATH or profile, so there is nothing to undo there.".into());
    said
}

/// Do the plan: what was done, each as a line. `delete_data` decides the data folder.
pub fn apply(plan: &Plan, delete_data: bool) -> Vec<String> {
    let mut done = Vec::new();
    if let Some(pids) = &plan.page {
        // not knowing whether it lives is not knowing it stopped
        let alive = |pid: &u32| signal("-0", &pid.to_string()).unwrap_or(true);
        for pid in pids {
            let _stopped = signal("-TERM", &pid.to_string());
        }
        for _ in 0..30 {
            if !pids.iter().any(alive) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        done.push(if pids.iter().any(alive) { "the page did not stop; it ends when this machine restarts".into() } else { "the page stopped".into() });
    }
    if plan.claude {
        for args in [["plugin", "uninstall", "jaynes@jaynes"].as_slice(), ["plugin", "marketplace", "remove", "jaynes"].as_slice()] {
            match Command::new("claude").args(args).output() {
                Ok(o) if o.status.success() => done.push(format!("claude {}", args.join(" "))),
                Ok(o) => done.push(format!("claude {} did not work: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim())),
                Err(e) => done.push(format!("claude {} did not run: {e}", args.join(" "))),
            }
        }
    }
    for app in &plan.apps {
        done.push(match std::fs::remove_dir_all(app) {
            Ok(()) => format!("removed {}", app.display()),
            Err(e) => format!("{} was not removed: {e}", app.display()),
        });
    }
    match &plan.herdr {
        Herdr::Goes(p) => done.push(match std::fs::remove_file(p) {
            Ok(()) => format!("removed {}", p.display()),
            Err(e) => format!("{} was not removed: {e}", p.display()),
        }),
        Herdr::Stays(p, why) => done.push(format!("kept {}: {why}", p.display())),
        Herdr::None => {}
    }
    for (folder, _) in &plan.data {
        if delete_data {
            done.push(match std::fs::remove_dir_all(folder) {
                Ok(()) => format!("removed {}", folder.display()),
                Err(e) => format!("{} was not removed: {e}", folder.display()),
            });
        } else {
            done.push(format!("kept {}", folder.display()));
        }
    }
    for f in &plan.leftovers {
        let _gone = std::fs::remove_file(f);
    }
    // the program last: it is the one doing all this
    match &plan.program {
        Program::Cargo { root, file } => {
            let ran = Command::new("cargo").args(["uninstall", "jaynes-shell", "--root"]).arg(root).output();
            done.push(match ran {
                Ok(o) if o.status.success() => format!("cargo uninstall jaynes-shell --root {}", root.display()),
                _ => match std::fs::remove_file(file) {
                    Ok(()) => format!("removed {} (cargo could not, so its list in {} still names it)", file.display(), root.join(".crates.toml").display()),
                    Err(e) => format!("{} was not removed: {e}", file.display()),
                },
            });
        }
        Program::File(f) => done.push(match std::fs::remove_file(f) {
            Ok(()) => format!("removed {}", f.display()),
            Err(e) => format!("{} was not removed: {e}", f.display()),
        }),
        Program::None => {}
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app open, asked of a stand-in: no test runs a program from inside an app's folder, which macOS would
    /// register and try to open.
    #[test]
    fn nothing_is_removed_while_the_app_runs() {
        let base = std::env::temp_dir().join(format!("jaynes-uninstall-unit-{}", std::process::id()));
        let app = base.join("apps/Jaynes-stand-in");
        std::fs::create_dir_all(app.join("Contents")).expect("dir");
        let at = Places {
            home_dir: base.clone(), data: base.join(".jaynes"), engine: base.join(".jaynes/engine"), install_root: base.join(".local"),
            apps: vec![base.join("apps")], app_name: "Jaynes-stand-in".into(), page_port: 1,
        };
        let none = |_: u16| Ok(Vec::new());
        let open = |file: &Path| Ok(file.starts_with(&app));
        let blocked = plan_with(&at, &open, &none);
        assert!(blocked.blockers.iter().any(|b| b.contains("Quit the Jaynes app")), "{:?}", blocked.blockers);
        let closed = plan_with(&at, &|_| Ok(false), &none);
        assert!(closed.blockers.is_empty() && closed.apps == [app.clone()], "{:?}", closed.blockers);
        // not knowing whether it is open is not knowing it is closed: nothing is removed, and the person reads why
        let untold = plan_with(&at, &|_| Err("there is no /proc here, and lsof could not run (not found)".into()), &none);
        assert!(untold.blockers.len() == 1 && untold.blockers[0].contains("cannot tell") && untold.blockers[0].contains("install lsof"), "{:?}", untold.blockers);
        let _gone = std::fs::remove_dir_all(&base);
    }

    /// /proc as Linux lays it out, written by hand: a process's exe is a link to its program, its fd a link to what it
    /// holds open, a socket as socket:[INODE], and net/tcp and net/tcp6 list sockets with their state and inode.
    #[test]
    fn proc_says_what_runs_a_file_and_what_listens_on_a_port() {
        use std::os::unix::fs::symlink;
        let base = std::env::temp_dir().join(format!("jaynes-uninstall-proc-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let proc = base.join("proc");
        let herdr = base.join("bin/herdr-core");
        let other = base.join("bin/other");
        std::fs::create_dir_all(base.join("bin")).expect("dir");
        std::fs::write(&herdr, "herdr").expect("write");
        std::fs::write(&other, "other").expect("write");
        let pid = |n: u32| {
            let d = proc.join(n.to_string());
            std::fs::create_dir_all(d.join("fd")).expect("dir");
            d
        };
        symlink(&herdr, pid(100).join("exe")).expect("link");
        symlink(&other, pid(200).join("exe")).expect("link");
        symlink(base.join("bin/../bin/herdr-core"), pid(300).join("exe")).expect("link");
        pid(400); // a process whose exe cannot be read
        std::fs::create_dir_all(proc.join("self")).expect("dir");
        assert_eq!(running_in(&proc, &herdr), Ok(vec![100, 300]), "by the file itself, whatever path reaches it");
        assert_eq!(running_in(&proc, &base.join("bin/gone")).map_err(|e| e.contains("could not be read")), Err(true));
        // 8750 is 222E: listening on IPv4 by 100 and on IPv6 by 300; 200 holds a socket on that port that does not listen
        std::fs::create_dir_all(proc.join("net")).expect("dir");
        let head = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";
        std::fs::write(proc.join("net/tcp"), format!("{head}   0: 0100007F:222E 00000000:0000 0A 00000000:00000000 00:00000000 00000000   501        0 777 1 0 100 0 0 10 0\n   1: 0100007F:222E 0100007F:D431 01 00000000:00000000 00:00000000 00000000   501        0 778 1 0 20 4 30 10 -1\n   2: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000   501        0 779 1 0 100 0 0 10 0\n")).expect("write");
        std::fs::write(proc.join("net/tcp6"), format!("{head}   0: 00000000000000000000000001000000:222E 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000   501        0 780 1 0 100 0 0 10 0\n")).expect("write");
        symlink("socket:[777]", pid(100).join("fd/3")).expect("link");
        symlink("socket:[778]", pid(200).join("fd/4")).expect("link");
        symlink("socket:[779]", pid(200).join("fd/5")).expect("link");
        symlink("socket:[780]", pid(300).join("fd/6")).expect("link");
        symlink("pipe:[1]", pid(400).join("fd/1")).expect("link");
        assert_eq!(listening_in(&proc, 8750), Ok(vec![100, 300]));
        assert_eq!(listening_in(&proc, 8080), Ok(vec![200]));
        assert_eq!(listening_in(&proc, 9), Ok(vec![]));
        // a /proc with no TCP table cannot say what listens
        std::fs::remove_dir_all(proc.join("net")).expect("rm");
        assert!(listening_in(&proc, 8750).is_err());
        let _gone = std::fs::remove_dir_all(&base);
    }
}
