//! Runs the person moved to Finished. A run started on an engine that is no longer on this Mac cannot go on: no
//! answer, Halt or Abandon reaches it, since one engine carries a run and this one never writes in another's record.
//! The person moves it out of what waits for them; this engine keeps that in a dated list of its own, in its home and
//! outside every run's folder, and the run's record stays as it was. There is no undo.

use jaynes_core::run::Run;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The list, in the engine's home.
pub const FILE: &str = "moved-to-finished.json";

/// A run the person moved, when, and the app's signature over the move when the app sent it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Moved {
    pub run: String,
    /// The engine that started it, as its record says.
    pub engine: String,
    /// When it was moved, in seconds.
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// Every run moved so far, the first move of each; none when nothing was moved.
pub fn read(home: &Path) -> Result<Vec<Moved>, String> {
    match std::fs::read_to_string(home.join(FILE)) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| format!("cannot read {FILE}: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(format!("cannot read {FILE}: {e}")),
    }
}

/// When run `run` was moved, if it was.
pub fn when(moved: &[Moved], run: &str) -> Option<u64> {
    moved.iter().find(|m| m.run == run).map(|m| m.at)
}

/// Moves `runs` to Finished at `at`, all or none: a run this engine or a kept one still carries on is refused, and
/// so is every other in the same move. A run already moved keeps its first date. Says how many were moved now.
pub fn mark(home: &Path, runs: &[&Run], signature: Option<&str>, at: u64) -> Result<usize, String> {
    if runs.is_empty() {
        return Err("which runs?".into());
    }
    if let Some(r) = runs.iter().find(|r| !crate::stop::left_behind(home, r, jaynes_core::ENGINE)) {
        return Err(if r.finished() {
            format!("run {} is finished already", r.id)
        } else {
            format!("run {} goes on with this Jaynes: answer, halt or abandon it instead", r.id)
        });
    }
    let mut list = read(home)?;
    let mut now = 0;
    for r in runs {
        if when(&list, &r.id).is_none() {
            list.push(Moved { run: r.id.clone(), engine: r.engine.clone(), at, signature: signature.map(str::to_string) });
            now += 1;
        }
    }
    if now > 0 {
        std::fs::create_dir_all(home).map_err(|e| e.to_string())?;
        let tmp = home.join(format!("{FILE}.{}", std::process::id()));
        let body = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, body).map_err(|e| format!("cannot write {FILE}: {e}"))?;
        std::fs::rename(&tmp, home.join(FILE)).map_err(|e| format!("cannot put {FILE} in place: {e}"))?;
    }
    Ok(now)
}

/// What a run moved to Finished says where its button was: "You moved it to Finished on 22/11."
pub fn moved_on(at: u64) -> String {
    format!("You moved it to Finished on {}.", day(at))
}

/// The day and month of a moment on this machine's clock: "22/11".
fn day(ts: u64) -> String {
    let flags: [&str; 2] = if cfg!(target_os = "macos") { ["-r", ""] } else { ["-d", "@"] };
    let said = std::process::Command::new("date").args([flags[0].to_string(), format!("{}{ts}", flags[1]), "+%d/%m".into()]).output();
    match said {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => {
            // the civil date in UTC, from the days since 1970 (Howard Hinnant's days_from_civil, inverted)
            let z = (ts / 86_400) as i64 + 719_468;
            let doe = z.rem_euclid(146_097);
            let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let d = doy - (153 * mp + 2) / 5 + 1;
            let m = if mp < 10 { mp + 3 } else { mp - 9 };
            format!("{d:02}/{m:02}")
        }
    }
}
