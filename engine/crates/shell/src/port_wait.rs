//! What a run waits for while another run on this machine holds a port its box needs (T10, 23/11): the box writes
//! each wait into the run's folder (jaynes_box.py `_port_turns`) and keeps it locked while it waits, so a box that
//! died leaves nothing that reads as a wait. The page, the app and `jaynes show` say it in the run's line, and no time
//! limit counts it: neither the turn's deadline nor a role's call.

use std::path::{Path, PathBuf};

/// The run's folder of waits, under its folder in the engine's home.
pub const FOLDER: &str = "port-waits";

/// Where the waits of the run whose folder is `run_dir` are.
pub fn folder(run_dir: &Path) -> PathBuf {
    run_dir.join(FOLDER)
}

/// What the run whose folder is `run_dir` waits for now, in the box's words ("Waiting for port 5432: another run
/// holds it while its commands run (estoque · Fix login)"); none when nothing waits.
pub fn live(run_dir: &Path) -> Option<String> {
    let mut found: Vec<(u64, String)> = std::fs::read_dir(folder(run_dir))
        .ok()?
        .flatten()
        .filter(|f| f.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|f| {
            let file = std::fs::File::open(f.path()).ok()?;
            // a wait whose box still keeps it locked: one the lock is free on was left by a box that died
            match file.try_lock_shared() {
                Ok(()) => return None,
                Err(std::fs::TryLockError::WouldBlock) => {}
                Err(std::fs::TryLockError::Error(_)) => return None,
            }
            let said: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(f.path()).ok()?).ok()?;
            Some((said["since"].as_u64().unwrap_or(0), said["said"].as_str()?.chars().take(300).collect()))
        })
        .collect();
    found.sort();
    found.into_iter().next().map(|(_, said)| said)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn a_wait_reads_while_its_box_keeps_it_and_never_after() {
        let dir = std::env::temp_dir().join(format!("jaynes-port-wait-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&dir);
        assert_eq!(live(&dir), None);
        std::fs::create_dir_all(folder(&dir)).expect("folder");
        let path = folder(&dir).join("a.json");
        let mut kept = std::fs::File::create(&path).expect("wait");
        kept.lock().expect("locked");
        let said = "Waiting for port 5432: another run holds it while its commands run (estoque · Fix login)";
        kept.write_all(serde_json::json!({"said": said, "since": 1}).to_string().as_bytes()).expect("written");
        // on another open of the file, as the page's process reads it
        assert_eq!(live(&dir).as_deref(), Some(said));
        // the box died: the lock went with it, the file stays
        drop(kept);
        assert_eq!(live(&dir), None);
        let _gone = std::fs::remove_dir_all(&dir);
    }
}
