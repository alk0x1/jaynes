//! The role's process: one call is one start of the role program, one request line in, one reply line out.
//! A role that does not start, does not answer in time, or answers what does not read, is Jaynes's fault, and
//! never spends an attempt of the agent.

use crate::protocol::{Body, Call, Reply, Request, MAJOR};
use jaynes_core::Digest;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static CALLS: AtomicU64 = AtomicU64::new(0);

/// The files of the library a role runs with: the Python library, the box, and the menu of a role in TOML.
pub const LIBRARY_FILES: [&str; 3] = ["jaynes_role.py", "jaynes_box.py", "jaynes_menu.py"];

/// The program that serves roles written in TOML: `JAYNES_EXE`, or this one.
fn this_program() -> Result<PathBuf, String> {
    match std::env::var("JAYNES_EXE") {
        Ok(p) if !p.is_empty() => Ok(PathBuf::from(p)),
        _ => std::env::current_exe().map_err(|e| format!("Jaynes cannot find its own program: {e}")),
    }
}

/// A role, as a folder with a `role.toml` (served by this program), a `role.py` (run with Python) or an
/// executable `role`.
#[derive(Clone, Debug)]
pub struct RoleProgram {
    pub dir: PathBuf,
    /// The folder of `jaynes_role.py`, put on the role's Python path.
    pub library: PathBuf,
    pub timeout: Duration,
    /// Where to note a call's process group while it runs (a check may run the project's suite), so a process
    /// that takes the run after a crash stops it.
    pub pidfile: Option<PathBuf>,
}

type Shared = std::sync::Arc<std::sync::Mutex<Vec<u8>>>;

/// Read a pipe on a thread into a buffer the caller keeps, so it can take what came without waiting for the end.
fn pump(mut from: impl Read + Send + 'static) -> (std::thread::JoinHandle<()>, Shared) {
    let into: Shared = Default::default();
    let kept = into.clone();
    let reader = std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = from.read(&mut buf) {
            if n == 0 {
                break;
            }
            if let Ok(mut t) = kept.lock() {
                t.extend_from_slice(&buf[..n]);
            }
        }
    });
    (reader, into)
}

impl RoleProgram {
    pub fn new(dir: &Path, library: &Path) -> RoleProgram {
        let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        RoleProgram { dir: real(dir), library: real(library), timeout: Duration::from_secs(900), pidfile: None }
    }

    fn command(&self) -> Result<Command, String> {
        let exe = self.dir.join("role");
        let py = self.dir.join("role.py");
        let mut c = if self.dir.join(crate::toml_role::FILE).is_file() {
            let mut c = Command::new(this_program()?);
            c.arg("role-serve").arg(&self.dir).arg("--library").arg(&self.library);
            c
        } else if exe.is_file() {
            Command::new(exe)
        } else if py.is_file() {
            let python = std::env::var("JAYNES_PYTHON").unwrap_or_else(|_| "python3".to_string());
            let mut c = Command::new(python);
            c.arg(py);
            c
        } else {
            return Err(format!("{} has neither role.toml, role.py nor an executable role", self.dir.display()));
        };
        let path = match std::env::var("PYTHONPATH") {
            Ok(p) if !p.is_empty() => format!("{}:{p}", self.library.display()),
            _ => self.library.display().to_string(),
        };
        c.current_dir(&self.dir).env("PYTHONPATH", path).env("PYTHONDONTWRITEBYTECODE", "1");
        // where its commands in the box note their groups, beside the call's own note, so a halt stops them too
        if let Some(f) = &self.pidfile {
            c.env("JAYNES_BOX_GROUPS", f.with_file_name(crate::drive::BOX_GROUPS));
        }
        Ok(c)
    }

    /// Make one call. An error is a sentence for the record: the role could not answer.
    pub fn call(&self, call: Call) -> Result<Body, String> {
        let id = format!("{}-{}", std::process::id(), CALLS.fetch_add(1, Ordering::Relaxed));
        let request = Request { jaynes: MAJOR, id: id.clone(), call };
        let line = serde_json::to_string(&request).map_err(|e| e.to_string())? + "\n";
        use std::os::unix::process::CommandExt;
        let mut child = self
            .command()?
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("the role program did not start: {e}"))?;
        if let Some(f) = &self.pidfile {
            let _noted = std::fs::write(f, child.id().to_string());
        }
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(line.as_bytes()).map_err(|e| format!("the role program did not take the request: {e}"))?;
        }
        let stdout = child.stdout.take().ok_or("no output pipe")?;
        let stderr = child.stderr.take().ok_or("no error pipe")?;
        // read into buffers kept outside the readers: what a check leaves running may hold the pipes open
        let (reader, out_text) = pump(stdout);
        let (errors, err_text) = pump(stderr);
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() > self.timeout => {
                    // the whole group: a check's suite runs in the role's children
                    let _killed = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).status();
                    let _waited = child.wait();
                    if let Some(f) = &self.pidfile {
                        let _gone = std::fs::remove_file(f);
                    }
                    return Err(format!("the role program did not answer in {} s", self.timeout.as_secs()));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => return Err(format!("the role program could not be watched: {e}")),
            }
        }
        if let Some(f) = &self.pidfile {
            let _gone = std::fs::remove_file(f);
        }
        // what the call left running in its group ends with it; then read for a moment more, at most
        let group = |sig: &str| Command::new("kill").args([sig, "--", &format!("-{}", child.id())]).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
        if group("-0") {
            let _term = group("-TERM");
            std::thread::sleep(Duration::from_millis(300));
            let _kill = group("-KILL");
        }
        let until = Instant::now() + Duration::from_secs(3);
        while !(reader.is_finished() && errors.is_finished()) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        let text = |t: &Shared| t.lock().map(|t| String::from_utf8_lossy(&t).into_owned()).unwrap_or_default();
        let (out, err) = (text(&out_text), text(&err_text));
        let last = out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
        let reply: Reply = serde_json::from_str(last).map_err(|e| {
            let tail: String = err.chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect();
            format!("the role program's reply does not read ({e}): {} {tail}", last.chars().take(200).collect::<String>())
        })?;
        if reply.jaynes != MAJOR {
            return Err(format!("the role program answered in protocol {}, not {MAJOR}", reply.jaynes));
        }
        if reply.id != id {
            return Err("the role program answered another request".to_string());
        }
        if let Body::Error { message } = &reply.body {
            return Err(format!("the role program could not answer: {message}"));
        }
        Ok(reply.body)
    }

    /// The fingerprint of the role's content, and of the library it runs with: every file, by path.
    pub fn fingerprint(&self) -> Result<Digest, String> {
        let mut files: Vec<(String, Digest)> = Vec::new();
        collect(&self.dir, &self.dir, &mut files)?;
        for name in LIBRARY_FILES {
            if let Ok(bytes) = std::fs::read(self.library.join(name)) {
                files.push((format!("<library>/{name}"), Digest::of_bytes(&bytes)));
            }
        }
        files.sort();
        Ok(Digest::of_json(&files))
    }

    /// A copy of the role and its library under `into`, so a run stays on the version it started with.
    /// Copy the role where the run keeps it, and the fingerprint of that copy: what the run records is what it runs,
    /// whatever happens to the role's own folder meanwhile.
    pub fn pinned(&self, into: &Path) -> Result<(RoleProgram, Digest), String> {
        self.pin_then_fingerprint(into, || {})
    }

    /// `pinned`, with `between` run after the copy and before its fingerprint, where a change of the role's folder
    /// would fall.
    fn pin_then_fingerprint(&self, into: &Path, between: impl FnOnce()) -> Result<(RoleProgram, Digest), String> {
        let role = self.pin(into)?;
        between();
        let fingerprint = role.fingerprint()?;
        Ok((role, fingerprint))
    }

    pub fn pin(&self, into: &Path) -> Result<RoleProgram, String> {
        let dir = into.join("role");
        let lib = into.join("library");
        if !dir.exists() {
            copy_dir(&self.dir, &dir)?;
            std::fs::create_dir_all(&lib).map_err(|e| e.to_string())?;
            for name in LIBRARY_FILES {
                let from = self.library.join(name);
                if from.is_file() {
                    std::fs::copy(&from, lib.join(name)).map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(RoleProgram { dir, library: lib, timeout: self.timeout, pidfile: None })
    }
}

/// The fingerprint of a role's folder alone, as an index lists it: every file by path, without the library it runs
/// with or what Jaynes keeps beside an installed role.
pub fn content_fingerprint(dir: &Path) -> Result<Digest, String> {
    let mut files: Vec<(String, Digest)> = Vec::new();
    collect(dir, dir, &mut files)?;
    files.retain(|(p, _)| p != ".jaynes-origin");
    files.sort();
    Ok(Digest::of_json(&files))
}

fn skip(name: &str) -> bool {
    name == "__pycache__" || name == ".git" || name.ends_with(".pyc") || name == ".DS_Store"
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, Digest)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if skip(&name) {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            collect(root, &p, out)?;
        } else if let Ok(bytes) = std::fs::read(&p) {
            let rel = p.strip_prefix(root).unwrap_or(&p).display().to_string();
            out.push((rel, Digest::of_bytes(&bytes)));
        }
    }
    Ok(())
}

pub fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("cannot make {}: {e}", to.display()))?;
    for e in std::fs::read_dir(from).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if skip(&name) {
            continue;
        }
        let (src, dst) = (e.path(), to.join(&name));
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| format!("cannot copy {}: {e}", src.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_run_records_the_fingerprint_of_the_role_it_runs_and_an_unchanged_role_keeps_its_own() {
        let base = std::env::temp_dir().join(format!("jaynes-pin-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let (dir, lib) = (base.join("role"), base.join("lib"));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::create_dir_all(&lib).expect("lib");
        std::fs::write(dir.join("role.toml"), "role = \"a\"\n").expect("write");
        let given = RoleProgram::new(&dir, &lib);
        // unchanged: the copy's fingerprint is the role's own, so the records kept under it still count
        let (_, same) = given.pinned(&base.join("run1")).expect("pinned");
        assert_eq!(same, given.fingerprint().expect("fp"));
        // changed between the copy and the fingerprint: what is recorded is what was copied, which is what runs
        let (pinned, recorded) = given
            .pin_then_fingerprint(&base.join("run2"), || std::fs::write(dir.join("role.toml"), "role = \"b\"\n").expect("write"))
            .expect("pinned");
        assert_eq!(recorded, pinned.fingerprint().expect("fp"));
        assert_eq!(recorded, same, "the run runs the role as it was copied");
        assert_ne!(recorded, given.fingerprint().expect("fp"));
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_call_ends_even_when_what_the_role_left_running_holds_its_output() {
        let dir = std::env::temp_dir().join(format!("jaynes-role-left-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("role"), "#!/bin/sh\nsleep 30 &\npython3 -c 'import os,time; os.setsid(); time.sleep(30)' &\nread line\necho '{}'\n").expect("write");
        std::fs::set_permissions(dir.join("role"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let started = Instant::now();
        let said = RoleProgram::new(&dir, &dir).call(Call::Describe {});
        assert!(said.is_err(), "{said:?}");
        assert!(started.elapsed() < Duration::from_secs(15), "the call waited {:?}", started.elapsed());
    }
}
