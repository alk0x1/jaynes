//! "Open the web page ↗": one click, in the status bar or in Command-K, opens the page in the person's browser with its
//! key, starting it first when it does not run. The page is a process of the engine's own, started from the installed
//! `jaynes`, so it outlives the app. The app never reads the key: the engine puts it in the link it opens, and says why
//! without it when it cannot. And the app's version, as the person reads it.

use std::path::Path;

/// The command's words, in the status bar and in Command-K.
pub const LABEL: &str = "Open the web page ↗";
/// Its words while the engine starts the page and opens it.
pub const OPENING: &str = "Opening the web page…";
/// What the notice over the status bar says first when it did not open.
pub const DID_NOT_OPEN: &str = "The web page did not open";

/// The app's version as the status bar and Settings show it.
pub fn version() -> String {
    format!("Jaynes {}", env!("CARGO_PKG_VERSION"))
}

/// The page of `home` at `port`, started from `exe` when it does not run and its link opened in the browser when
/// `browser`, by `ensure`; why not, on one line, after which page did not open.
pub fn open_with(home: &Path, port: u16, exe: &Path, browser: bool, ensure: impl FnOnce(&Path, u16, &Path, bool) -> Result<String, String>) -> Result<(), String> {
    ensure(home, port, exe, browser).map(|_address| ()).map_err(|why| {
        let why = why.split_whitespace().collect::<Vec<_>>().join(" ");
        if why.is_empty() { DID_NOT_OPEN.to_string() } else { format!("{DID_NOT_OPEN}: {why}") }
    })
}

/// A stop's live version: the page a role kept of a direction, `page` by its name, opened by `ensure_at` through the
/// page of `home` at `port` (started from `exe` when it does not run) with its key, at the page's own path for it;
/// why not, on one line.
pub fn live_with(home: &Path, port: u16, exe: &Path, page: &str, ensure_at: impl FnOnce(&Path, u16, &Path, &str) -> Result<String, String>) -> Result<(), String> {
    ensure_at(home, port, exe, &format!("/page/{page}")).map(|_address| ()).map_err(|why| one_line(&why))
}

/// What a click on "Open live version" does: the page of the engine home the app reads, from the installed `jaynes`,
/// as "Open the web page ↗" does; the key goes in the link the engine opens, never through the app.
pub fn live(page: &str) -> Result<(), String> {
    live_with(&jaynes_shell::home(), page_port()?, &crate::answer::engine_program(), page, jaynes_shell::install::ensure_page_at)
}

/// The port the page is asked on: the engine's own (install::PORT), or, in a build for tests only, the one
/// JAYNES_APP_PAGE_PORT names, so a test never reaches the page of the person who runs it.
fn page_port() -> Result<u16, String> {
    port_from(crate::testing::var("JAYNES_APP_PAGE_PORT"))
}

/// The engine's port when nothing is named; the named one, or why it is none, so a test that named a wrong one opens
/// nothing rather than the person's page.
fn port_from(named: Option<std::ffi::OsString>) -> Result<u16, String> {
    match named {
        None => Ok(jaynes_shell::install::PORT),
        Some(v) => v.to_str().and_then(|v| v.parse::<u16>().ok()).filter(|p| *p != 0).ok_or_else(|| format!("{DID_NOT_OPEN}: JAYNES_APP_PAGE_PORT is {v:?}, not a port")),
    }
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
    open_with(&jaynes_shell::home(), page_port()?, &crate::answer::engine_program(), browser, jaynes_shell::install::ensure_page_on)
}

#[cfg(test)]
mod tests {
    use super::{live_with, open_with, page_port, port_from, version};

    #[test]
    fn a_test_build_asks_the_page_on_the_port_it_is_given_and_a_build_for_people_always_on_the_engines() {
        use std::ffi::OsString;
        assert_eq!(port_from(None), Ok(jaynes_shell::install::PORT));
        assert_eq!(port_from(Some(OsString::from("51234"))), Ok(51234));
        // a name that is no port opens nothing, never the person's page in its place
        for bad in ["", "0", "x", "70000", "8750 "] {
            assert!(port_from(Some(OsString::from(bad))).is_err_and(|why| why.contains("not a port")), "{bad:?}");
        }
        // SAFETY: only page_port reads this variable, and no other test here sets it.
        unsafe { std::env::set_var("JAYNES_APP_PAGE_PORT", "51235") };
        let port = page_port();
        unsafe { std::env::remove_var("JAYNES_APP_PAGE_PORT") };
        assert_eq!(port, Ok(if crate::testing::BUILT_FOR_TESTS { 51235 } else { jaynes_shell::install::PORT }));
    }

    #[test]
    fn the_live_version_opens_through_the_page_at_its_own_path_with_its_key() {
        let asked = std::cell::RefCell::new(None);
        let page = "ab12.html";
        let said = live_with(Path::new("/h/.jaynes/engine"), 8750, Path::new("/h/.local/bin/jaynes"), page, |home, port, exe, path| {
            *asked.borrow_mut() = Some((home.to_path_buf(), port, exe.to_path_buf(), path.to_string()));
            Ok(format!("http://localhost:{port}{path}"))
        });
        assert_eq!(said, Ok(()));
        assert_eq!(asked.into_inner(), Some((PathBuf::from("/h/.jaynes/engine"), 8750, PathBuf::from("/h/.local/bin/jaynes"), "/page/ab12.html".into())));
        // why it did not open, on one line, as the line beside the button shows it
        let said = live_with(Path::new("/h"), 8750, Path::new("/j"), page, |_, _, _, _| Err("the page did not start:\n  see page.log".into()));
        assert_eq!(said, Err("The page did not start: see page.log".into()));
    }
    use std::path::{Path, PathBuf};

    // Alan, 20/11: "seria bom o app e o site mostrarem a versão atual instalada em algum lugar" (the app shows it in
    // Settings and in the status bar; the site at its foot, engine/crates/shell/tests/page_open.rs)
    #[test]
    fn asked_the_app_shows_the_version_installed_its_own_from_its_cargo_toml() {
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
        // the notice says which page did not open, then why
        assert_eq!(said, Err("The web page did not open: another program answers on [::1]:8750, where the browser may go".into()));
        assert_eq!(super::LABEL, "Open the web page ↗");
        assert_eq!(super::OPENING, "Opening the web page…");
    }

    /// A port the system gives on 127.0.0.1 that is free on [::1] too, where the page is also asked.
    fn quiet_port() -> u16 {
        loop {
            let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
            if std::net::TcpListener::bind(("::1", port)).is_ok() {
                return port;
            }
        }
    }

    #[test]
    fn the_engine_that_cannot_start_the_page_says_why_on_one_line_and_never_the_key() {
        let home = std::env::temp_dir().join(format!("jaynes-open-page-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        // a port nothing answers on, and a program that ends at once in place of `jaynes page`; no browser is asked. The
        // port is free on both of the page's addresses when the system gives it, but another program on this machine
        // (other tests start pages on port 0) may take it before the page is asked: then it is not this test's case,
        // and another port is tried (it failed once in four full runs so)
        let mut said = Err(String::new());
        for _ in 0..5 {
            let port = quiet_port();
            said = open_with(&home, port, Path::new("/usr/bin/false"), false, jaynes_shell::install::ensure_page_on);
            if !said.as_ref().is_err_and(|why| why.contains("answers on")) {
                break;
            }
        }
        let key = std::fs::read_to_string(home.join("page.key")).unwrap_or_default();
        std::fs::remove_dir_all(&home).unwrap();
        let why = said.expect_err("a page that did not start");
        assert!(why.contains("did not start"), "{why}");
        assert!(!why.contains('\n'), "{why}");
        assert!(key.trim().len() >= 16, "the key is made in the test's home");
        assert!(!why.contains(key.trim()), "the key is in what the person reads: {why}");
    }
}
