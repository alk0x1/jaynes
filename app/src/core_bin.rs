//! JAYNES: copied from herdr-app's src/core_bin.rs (Apache-2.0) and changed: the installer, the updater and the CLI
//! link are gone; the app only finds the core the person installed.
//!
//! Finding herdr-core (`hc`) on this machine. The app doesn't carry one: it
//! uses the core people installed.
//!
//! Where it looks, in order: `core` in settings.json; this process's PATH;
//! the person's login shell's PATH (an app opened from the Dock or a
//! launcher doesn't get it); then the usual install places. A candidate
//! counts only if `--version` says `hc …`: some window managers ship an
//! unrelated `hc`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

static BIN: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The core the app runs (`hc` until one is found).
pub fn path() -> PathBuf {
    BIN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone().unwrap_or_else(|| PathBuf::from("hc"))
}

/// Uses this core from now on, and puts its folder on PATH ahead of any
/// other `hc`, so mods' `hc()` and anything the app runs find the same one.
pub fn use_core(bin: PathBuf) {
    let path = std::env::var_os("PATH").unwrap_or_default();
    if let Some(joined) = path_for_core(&path, &bin, &hc_names()) {
        // SAFETY: set once at start, before other threads read the environment.
        unsafe { std::env::set_var("PATH", joined) };
    }
    *BIN.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(bin);
}

/// PATH where `hc` finds `bin`, or None if it already does. The folder goes
/// just before the first entry that may hold another `hc`, so NixOS keeps
/// /run/wrappers/bin ahead of it.
fn path_for_core(path: &std::ffi::OsStr, bin: &Path, hc_names: &[String]) -> Option<std::ffi::OsString> {
    let dir = bin.parent()?;
    let dirs: Vec<PathBuf> = if path.is_empty() { Vec::new() } else { std::env::split_paths(path).collect() };
    // Relative entries (``, `.`) are another folder in each pane: assume an `hc` there.
    // Shells pick different names, so another folder is ours only if every `hc` in it is the core.
    let hcs_in = |d: &Path| hc_names.iter().map(|name| d.join(name)).filter(|hc| hc.is_file()).collect::<Vec<_>>();
    let stop = dirs.iter().position(|d| d.is_relative() || !hcs_in(d).is_empty());
    if let Some(d) = stop.map(|i| &dirs[i])
        && d.is_absolute()
        && (d == dir || bin.canonicalize().is_ok_and(|core| hcs_in(d).iter().all(|hc| hc.canonicalize().is_ok_and(|hc| hc == core))))
    {
        return None;
    }
    let (before, after) = dirs.split_at(stop.unwrap_or(0));
    let others = |d: &&PathBuf| *d != dir;
    let dirs = before.iter().filter(others).map(PathBuf::as_path).chain([dir]).chain(after.iter().filter(others).map(PathBuf::as_path));
    std::env::join_paths(dirs).ok()
}

/// The file names a shell runs for `hc`.
fn hc_names() -> Vec<String> {
    if cfg!(windows) { pathext_names(&std::env::var("PATHEXT").unwrap_or_default()) } else { vec!["hc".into()] }
}

/// Windows shells try each PATHEXT extension, and PowerShell also runs
/// `.ps1` scripts, so an earlier `hc.cmd` or `hc.ps1` wins over our `hc.exe`.
fn pathext_names(pathext: &str) -> Vec<String> {
    let pathext = if pathext.trim().is_empty() { ".COM;.EXE;.BAT;.CMD" } else { pathext };
    let exts = pathext.split(';').map(str::trim).filter(|ext| !ext.is_empty());
    exts.chain([".ps1"]).map(|ext| format!("hc{}", ext.to_ascii_lowercase())).collect()
}

/// Finder and Dock launches don't inherit the person's shell PATH.
#[cfg(target_os = "macos")]
pub fn fill_path() {
    if std::os::unix::process::parent_id() != 1 {
        return;
    }
    let home = home();
    let current = std::env::var_os("PATH").unwrap_or_default();
    if let Some(path) = merged_path(login_shell_path().as_deref(), &usual_dirs(&home)[..3], &current) {
        // SAFETY: called at startup, before other threads read the environment.
        unsafe { std::env::set_var("PATH", path) };
    }
}

#[cfg(any(target_os = "macos", test))]
fn merged_path(login: Option<&std::ffi::OsStr>, usual: &[PathBuf], current: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
    let mut dirs: Vec<PathBuf> = login.map(|path| dirs_on(path).collect()).unwrap_or_default();
    dirs.extend_from_slice(usual);
    dirs.extend(dirs_on(current));
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|dir| seen.insert(dir.clone()));
    std::env::join_paths(dirs).ok()
}

/// `hc X.Y.Z` if this binary is herdr-core and runs.
pub fn version(bin: &Path) -> Option<String> {
    let out = crate::spawn::command(bin).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && text.starts_with("hc ")).then_some(text)
}

/// The first herdr-core found, or None.
pub fn find(configured: Option<&str>) -> Option<PathBuf> {
    let home = home();
    let pick = |candidates: Vec<PathBuf>| candidates.into_iter().filter(|c| c.is_file()).find(|c| version(c).is_some());
    if let Some(found) = pick(configured.map(|c| vec![expand(c, &home)]).unwrap_or_default()) {
        return Some(found);
    }
    // Tests: HERDR_CORE_DIRS replaces every place Herdr would look.
    if let Some(dirs) = std::env::var_os("HERDR_CORE_DIRS") {
        return pick(std::env::split_paths(&dirs).flat_map(|d| names(&d)).collect());
    }
    // The quick places first; the login shell (slow) only if they have none.
    let path = std::env::var_os("PATH").unwrap_or_default();
    let quick: Vec<PathBuf> = dirs_on(&path).chain(usual_dirs(&home)).flat_map(|d| names(&d)).collect();
    pick(quick).or_else(|| pick(dirs_on(&login_shell_path()?).flat_map(|d| names(&d)).collect()))
}

fn expand(p: &str, home: &Path) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(p),
    }
}

fn names(dir: &Path) -> [PathBuf; 2] {
    let exe = std::env::consts::EXE_SUFFIX;
    [dir.join(format!("herdr-core{exe}")), dir.join(format!("hc{exe}"))]
}

fn dirs_on(path: &std::ffi::OsStr) -> impl Iterator<Item = PathBuf> + '_ {
    std::env::split_paths(path)
}

/// Where herdr-core gets installed: its own installer and Herdr 1.x's
/// (~/.local/bin), Homebrew, mise, Nix, cargo.
fn usual_dirs(home: &Path) -> Vec<PathBuf> {
    if cfg!(windows) {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData/Local"));
        return vec![local.join("Programs/herdr-core"), home.join(".cargo/bin")];
    }
    let mut dirs = vec![
        home.join(".local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".local/share/mise/shims"),
        home.join(".nix-profile/bin"),
        PathBuf::from("/run/current-system/sw/bin"),
        home.join(".cargo/bin"),
    ];
    if cfg!(target_os = "macos") {
        dirs.push(home.join(".local/share/herdr-core/current/Contents/MacOS"));
        // Older installers placed the bundle directly in herdr-core.
        dirs.push(home.join(".local/share/herdr-core/Herdr Core.app/Contents/MacOS"));
    }
    dirs
}

/// The PATH the person's login shell sets up, or None if it doesn't answer quickly.
fn login_shell_path() -> Option<std::ffi::OsString> {
    if cfg!(windows) {
        return None;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let mut child = Command::new(shell)
        .args(["-lc", "printf 'herdr-path[%s]herdr-path' \"$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // A slow or interactive shell profile must not hold up the app.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if child.try_wait().ok()?.is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    login_path_answer(&out)
}

/// The PATH inside the probe's frame: a chatty profile prints around it, and a shell that
/// doesn't speak sh (nushell) answers an error, nothing, or `$PATH` literally.
fn login_path_answer(out: &str) -> Option<std::ffi::OsString> {
    let (_, rest) = out.split_once("herdr-path[")?;
    let (path, _) = rest.rsplit_once("]herdr-path")?;
    (!path.is_empty() && path != "$PATH").then(|| path.into())
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}
