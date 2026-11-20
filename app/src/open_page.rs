//! "Open the page": one click, in the status bar or in Command-K, opens the page in the person's browser with its
//! key, starting it first when it does not run. The page is a process of the engine's own, started from the installed
//! `jaynes`, so it outlives the app. The app never reads the key: the engine puts it in the link it opens, and says why
//! without it when it cannot. And the app's version, as the person reads it.

use std::path::Path;

/// The command's words, in the status bar and in Command-K.
pub const LABEL: &str = "Open the page";
/// Its words while the engine starts the page and opens it.
pub const OPENING: &str = "Opening the page…";

/// The app's version as the status bar and Settings show it.
pub fn version() -> String {
    format!("Jaynes {}", env!("CARGO_PKG_VERSION"))
}

/// The page of `home` at `port`, started from `exe` when it does not run and its link opened in the browser when
/// `browser`, by `ensure`; why not, on one line.
pub fn open_with(home: &Path, port: u16, exe: &Path, browser: bool, ensure: impl FnOnce(&Path, u16, &Path, bool) -> Result<String, String>) -> Result<(), String> {
    ensure(home, port, exe, browser).map(|_address| ()).map_err(|why| one_line(&why))
}

/// `why` on one line, as a sentence: its lines and runs of spaces as one space, its first letter a capital.
fn one_line(why: &str) -> String {
    let joined = why.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut letters = joined.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => "The page did not open".into(),
    }
}

/// What a click does: the page of the engine home the app reads, started from the installed `jaynes` (the program the
/// app answers and halts with, answer.rs), its link opened in the person's browser unless JAYNES_NO_BROWSER says not
/// to, as the engine's own start reads it.
pub fn open() -> Result<(), String> {
    let browser = std::env::var_os("JAYNES_NO_BROWSER").is_none();
    open_with(&jaynes_shell::home(), jaynes_shell::install::PORT, &crate::answer::engine_program(), browser, jaynes_shell::install::ensure_page_on)
}

#[cfg(test)]
mod tests {
    use super::{open_with, version};
    use std::path::{Path, PathBuf};

    #[test]
    fn the_version_shown_is_the_apps_own_from_its_cargo_toml() {
        let manifest = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).expect("Cargo.toml");
        let package = manifest.split("[package]").nth(1).expect("[package]");
        let line = package.lines().find(|l| l.trim_start().starts_with("version")).expect("its version");
        let number = line.split('"').nth(1).expect("a quoted version");
        assert_eq!(version(), format!("Jaynes {number}"));
    }

    #[test]
    fn a_click_asks_the_engine_for_the_page_of_this_home_from_the_installed_jaynes() {
        let asked = std::cell::RefCell::new(None);
        let said = open_with(Path::new("/h/.jaynes/engine"), jaynes_shell::install::PORT, Path::new("/h/.local/bin/jaynes"), true, |home, port, exe, browser| {
            *asked.borrow_mut() = Some((home.to_path_buf(), port, exe.to_path_buf(), browser));
            Ok("http://localhost:8750/".into())
        });
        assert_eq!(said, Ok(()));
        assert_eq!(asked.into_inner(), Some((PathBuf::from("/h/.jaynes/engine"), 8750, PathBuf::from("/h/.local/bin/jaynes"), true)));
        // why the engine could not, on one line, as the line over the status bar shows it
        let said = open_with(Path::new("/h"), 8750, Path::new("/j"), true, |_, _, _, _| Err("another program answers on [::1]:8750,\n  where the browser may go".into()));
        assert_eq!(said, Err("Another program answers on [::1]:8750, where the browser may go".into()));
    }

    #[test]
    fn the_engine_that_cannot_start_the_page_says_why_on_one_line_and_never_the_key() {
        let home = std::env::temp_dir().join(format!("jaynes-open-page-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        // a port nothing answers on, and a program that ends at once in place of `jaynes page`; no browser is asked
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let said = open_with(&home, port, Path::new("/usr/bin/false"), false, jaynes_shell::install::ensure_page_on);
        let key = std::fs::read_to_string(home.join("page.key")).unwrap_or_default();
        std::fs::remove_dir_all(&home).unwrap();
        let why = said.expect_err("a page that did not start");
        assert!(why.contains("did not start"), "{why}");
        assert!(!why.contains('\n'), "{why}");
        assert!(key.trim().len() >= 16, "the key is made in the test's home");
        assert!(!why.contains(key.trim()), "the key is in what the person reads: {why}");
    }
}
