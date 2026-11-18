//! Where the person's device keys are paired: a Unix socket in the engine's home (`person.sock`, open to this user
//! only), served by the engine's lasting process (the page's). A change of the keys is taken only from the Jaynes app
//! the installation recorded: the system says which program is at the other end of the connection (its audit token,
//! which names that very process and no later one with its number), and the engine takes the change only when that
//! program's code is the app's exact build (its cdhash is in `app.cdhash`, written by the installation and by
//! `jaynes update`), signed with the hardened runtime, not open to a debugger, and with the app's identifier. Nothing
//! else pairs: not `jaynes pair` from a terminal, not an agent, not another program with the same name.
//!
//! What this does not hold against: a program that rewrites `app.cdhash` or the record itself, which are files of the
//! engine's home; and the first pairing, which the app makes with its own Touch ID.

use crate::store::Store;
use jaynes_core::pairing::Change;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

/// The Jaynes app's signing identifier.
pub const APP_ID: &str = "dev.jaynes.app";
/// The file of the app's builds the installation recorded, one cdhash in hex per line.
pub const PINNED: &str = "app.cdhash";
/// The socket's name in the engine's home.
pub const SOCKET: &str = "person.sock";

/// The system's requirement that a program is one of the app's builds the installation recorded: its identifier and
/// the exact hash of its code directory, which also covers how it was signed (the hardened runtime, no debugger
/// allowed: the installation records only builds signed so). None when no build is recorded.
pub fn requirement(home: &Path) -> Result<String, String> {
    let pinned = std::fs::read_to_string(home.join(PINNED)).map_err(|_| "no Jaynes app is recorded by this installation".to_string())?;
    let hashes: Vec<String> = pinned
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| {
            if l.len() == 40 && l.bytes().all(|b| b.is_ascii_hexdigit()) { Ok(format!("cdhash H\"{}\"", l.to_lowercase())) } else { Err(format!("{PINNED} holds a line that is not a cdhash: {l}")) }
        })
        .collect::<Result<_, _>>()?;
    if hashes.is_empty() {
        return Err("no Jaynes app is recorded by this installation".into());
    }
    Ok(format!("identifier \"{APP_ID}\" and ({})", hashes.join(" or ")))
}

/// Serves pairing on the socket of `home` until the process ends. Each connection sends one line, {"pair": Change},
/// and reads one, {"recorded": true} or {"refused": why}.
pub fn serve(home: &Path) -> Result<(), String> {
    let path = home.join(SOCKET);
    // the page may be the first of the engine to run on this home
    std::fs::create_dir_all(home).map_err(|e| format!("cannot make {}: {e}", home.display()))?;
    let _gone = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    let home = home.to_path_buf();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let reply = answer(&home, &conn);
            let mut conn = conn;
            let _sent = writeln!(conn, "{reply}");
        }
    });
    Ok(())
}

fn answer(home: &Path, conn: &UnixStream) -> Value {
    match take(home, conn) {
        Ok(()) => json!({"recorded": true}),
        Err(why) => json!({"refused": why}),
    }
}

fn take(home: &Path, conn: &UnixStream) -> Result<(), String> {
    let _timed = conn.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    // what was sent is read first, so a refusal reaches the sender instead of a broken pipe
    let mut line = String::new();
    BufReader::new(conn.take(64 * 1024)).read_line(&mut line).map_err(|e| e.to_string())?;
    let requirement = requirement(home)?;
    let caller = peer(conn, &requirement)?;
    let v: Value = serde_json::from_str(&line).map_err(|e| format!("not JSON: {e}"))?;
    let mut change: Change = serde_json::from_value(v["pair"].clone()).map_err(|e| format!("not a change of keys: {e}"))?;
    change.came_from("app", &caller);
    Store::open(&home.join("record.sqlite3"))?.change_keys(&change)
}

/// Sends a change of the keys on the socket of `home`, as the app does: what `jaynes pair --socket` runs.
pub fn send(home: &Path, change: &Value) -> Result<(), String> {
    let mut conn = UnixStream::connect(home.join(SOCKET)).map_err(|e| format!("the engine's page is not running ({}): {e}", home.join(SOCKET).display()))?;
    writeln!(conn, "{}", json!({"pair": change})).map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(conn).read_line(&mut reply).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&reply).map_err(|e| format!("the engine answered {reply:?}: {e}"))?;
    match v["refused"].as_str() {
        Some(why) => Err(format!("refused: {why}")),
        None => Ok(()),
    }
}

/// The socket of `home`.
pub fn path(home: &Path) -> PathBuf {
    home.join(SOCKET)
}

#[cfg(not(target_os = "macos"))]
fn peer(_conn: &UnixStream, _requirement: &str) -> Result<String, String> {
    Err("only the Jaynes app pairs a device, and there is none for this system yet".into())
}

/// Whether the process at the other end of `conn` runs a program that meets `requirement`, as the system checks it
/// (its signature holds, statically and while it runs): what it is, for the record.
#[cfg(target_os = "macos")]
fn peer(conn: &UnixStream, requirement: &str) -> Result<String, String> {
    use core_foundation::base::TCFType;
    use core_foundation::data::CFData;
    use security_framework::os::macos::code_signing::{Flags, GuestAttributes, SecCode, SecRequirement};
    let token = jaynes_peer::audit_token(conn).ok_or("the system did not say who is asking")?;
    let data = CFData::from_buffer(&token);
    let mut attrs = GuestAttributes::new();
    attrs.set_audit_token(data.as_concrete_TypeRef());
    let code = SecCode::copy_guest_with_attribues(None, &attrs, Flags::NONE).map_err(|e| format!("the system did not say what program is asking: {e}"))?;
    let wanted: SecRequirement = requirement.parse().map_err(|e| format!("the recorded app reads as no requirement: {e}"))?;
    code.check_validity(Flags::NONE, &wanted).map_err(|_| "the program asking is not the build of the Jaynes app this installation recorded".to_string())?;
    let path = code.path(Flags::NONE).ok().and_then(|u| u.to_path()).map(|p| p.display().to_string()).unwrap_or_default();
    Ok(format!("{APP_ID} {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_requirement_names_the_recorded_builds_and_nothing_else() {
        let home = std::env::temp_dir().join(format!("jaynes-person-socket-{}", std::process::id()));
        std::fs::create_dir_all(&home).expect("home");
        assert!(requirement(&home).is_err(), "no app recorded");
        std::fs::write(home.join(PINNED), format!("{}\n\n{}\n", "AB".repeat(20), "cd".repeat(20))).expect("pin");
        assert_eq!(requirement(&home).expect("req"), format!("identifier \"dev.jaynes.app\" and (cdhash H\"{}\" or cdhash H\"{}\")", "ab".repeat(20), "cd".repeat(20)));
        std::fs::write(home.join(PINNED), "ab\" or anchor apple\n").expect("pin");
        assert!(requirement(&home).is_err(), "a line that is not a cdhash never enters the requirement");
        let _gone = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_socket_opens_on_a_home_not_made_yet_and_answers() {
        // the page may be the first thing the engine runs on a home: it makes the folder, and the socket answers
        let base = std::env::temp_dir().join(format!("jaynes-person-socket-new-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&base);
        let home = base.join("home");
        serve(&home).expect("the socket opens");
        let said = send(&home, &json!({"pairing": "paired", "keys": [], "nonce": "n"}));
        assert!(said.as_ref().is_err_and(|e| e.contains("no Jaynes app is recorded")), "{said:?}");
        let _gone = std::fs::remove_dir_all(&base);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_program_that_is_not_the_recorded_build_is_refused() {
        let (a, _b) = UnixStream::pair().expect("pair");
        let req = format!("identifier \"{APP_ID}\" and cdhash H\"{}\"", "ab".repeat(20));
        assert!(peer(&a, &req).is_err_and(|e| e.contains("not the build")), "this test program is not the app");
    }
}
