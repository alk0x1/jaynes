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

/// A role, as a folder with a `role.py` (run with Python) or an executable `role`.
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
        let mut c = if exe.is_file() {
            Command::new(exe)
        } else if py.is_file() {
            let python = std::env::var("JAYNES_PYTHON").unwrap_or_else(|_| "python3".to_string());
            let mut c = Command::new(python);
            c.arg(py);
            c
        } else {
            return Err(format!("{} has neither role.py nor an executable role", self.dir.display()));
        };
        let path = match std::env::var("PYTHONPATH") {
            Ok(p) if !p.is_empty() => format!("{}:{p}", self.library.display()),
            _ => self.library.display().to_string(),
        };
        c.current_dir(&self.dir).env("PYTHONPATH", path).env("PYTHONDONTWRITEBYTECODE", "1");
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
                    let _killed = Command::new("kill").args(["-KILL", &format!("-{}", child.id())]).status();
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
        let group = |sig: &str| Command::new("kill").args([sig, &format!("-{}", child.id())]).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
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
        for name in ["jaynes_role.py", "jaynes_box.py"] {
            if let Ok(bytes) = std::fs::read(self.library.join(name)) {
                files.push((format!("<library>/{name}"), Digest::of_bytes(&bytes)));
            }
        }
        files.sort();
        Ok(Digest::of_json(&files))
    }

    /// A copy of the role and its library under `into`, so a run stays on the version it started with.
    pub fn pin(&self, into: &Path) -> Result<RoleProgram, String> {
        let dir = into.join("role");
        let lib = into.join("library");
        if !dir.exists() {
            copy_dir(&self.dir, &dir)?;
            std::fs::create_dir_all(&lib).map_err(|e| e.to_string())?;
            for name in ["jaynes_role.py", "jaynes_box.py"] {
                let from = self.library.join(name);
                if from.is_file() {
                    std::fs::copy(&from, lib.join(name)).map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(RoleProgram { dir, library: lib, timeout: self.timeout, pidfile: None })
    }
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
