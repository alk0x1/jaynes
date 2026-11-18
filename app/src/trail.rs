//! The app's own log, kept in a file (~/Library/Logs/Jaynes/app.log, or JAYNES_APP_TRAIL): what it says while it runs
//! and, last, why it ended, so an app that is gone tells how. Opened from the Dock or Finder, nothing sees its stderr.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing_subscriber::fmt::writer::MakeWriterExt as _;

/// How big the log grows before it is kept aside as app.log.1 and a new one starts.
const KEEP_UNDER: u64 = 4 * 1024 * 1024;

/// Why the app ends, said last in its log.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ending {
    /// Command-Q or the menu.
    Quit,
    /// The last window closed.
    LastWindow,
    /// Nothing in the app asked: macOS did.
    Unknown,
}

impl Ending {
    pub fn why(self) -> &'static str {
        match self {
            Ending::Quit => "the person quit (Command-Q or Jaynes › Quit)",
            Ending::LastWindow => "its last window closed (its close button or Command-W), and the app ends with it",
            Ending::Unknown => "macOS asked it to end (Quit from the Dock, logging out or restarting, or another app)",
        }
    }
}

static ENDING: Mutex<Option<Ending>> = Mutex::new(None);

/// Where the log is kept.
pub fn path() -> PathBuf {
    std::env::var_os("JAYNES_APP_TRAIL").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        home.join("Library/Logs/Jaynes/app.log")
    })
}

/// Keeps a log grown past `under` bytes aside as `<log>.1` (the one there before goes); whether it did.
fn rotated(log: &Path, under: u64) -> std::io::Result<bool> {
    match std::fs::metadata(log) {
        Ok(m) if m.len() > under => {
            let aside = PathBuf::from(format!("{}.1", log.display()));
            std::fs::rename(log, aside)?;
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Starts the log: what the app says goes to its stderr and to the file; a panic is written there with where it
/// happened. Call once, first thing.
pub fn start(filter: tracing_subscriber::EnvFilter) {
    let log = path();
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = rotated(&log, KEEP_UNDER);
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&log);
    match file {
        Ok(file) => tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false).with_writer(std::io::stderr.and(Mutex::new(file))).init(),
        Err(e) => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
            tracing::warn!("cannot keep the app's log in {}: {e}", log.display());
        }
    }
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let at = std::backtrace::Backtrace::force_capture();
        tracing::error!("the app panicked: {info}\n{at}");
        before(info);
    }));
    tracing::info!("started: Jaynes {} (pid {})", env!("CARGO_PKG_VERSION"), std::process::id());
}

/// Says why the app is about to end, before it asks to; the last word counts.
pub fn ending(why: Ending) {
    *ENDING.lock().unwrap() = Some(why);
}

/// The main window was closed: the app goes on, in the Dock and the menu bar.
pub fn window_closed() {
    tracing::info!("its window was closed, the app stays (the Dock, the menu bar item or a notice opens it again)");
}

/// The app's last line, as it ends: why, as said by `ending`, else that macOS asked.
pub fn ended() {
    let why = ENDING.lock().unwrap().unwrap_or(Ending::Unknown);
    tracing::info!("ending: {}", why.why());
}

/// A start that cannot go on: said in the log, then the process ends with 1.
pub fn fail(why: &str) -> ! {
    tracing::error!("cannot start: {why}");
    eprintln!("jaynes-app: {why}");
    std::process::exit(1)
}

#[cfg(test)]
mod tests {
    use super::{Ending, rotated};

    #[test]
    fn a_log_grown_too_big_is_kept_aside_once_and_a_new_one_starts() {
        let d = std::env::temp_dir().join(format!("jaynes-trail-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&d).unwrap();
        let log = d.join("app.log");
        std::fs::write(&log, "small").unwrap();
        assert!(!rotated(&log, 100).unwrap());
        std::fs::write(&log, "x".repeat(200)).unwrap();
        assert!(rotated(&log, 100).unwrap());
        assert!(!log.exists());
        assert_eq!(std::fs::read_to_string(d.join("app.log.1")).unwrap().len(), 200);
        assert!(!rotated(&d.join("none.log"), 100).unwrap());
    }

    #[test]
    fn the_last_line_says_why_it_ended() {
        assert_eq!(Ending::Quit.why(), "the person quit (Command-Q or Jaynes › Quit)");
        assert_eq!(Ending::LastWindow.why(), "its last window closed (its close button or Command-W), and the app ends with it");
        assert_eq!(Ending::Unknown.why(), "macOS asked it to end (Quit from the Dock, logging out or restarting, or another app)");
    }
}
