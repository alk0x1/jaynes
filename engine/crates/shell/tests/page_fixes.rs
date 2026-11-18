//! The page stays up and keeps its key to itself: no request from outside ends it, no connection that says nothing
//! holds the others, and the key is neither readable by others on the machine nor written to the page's log.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

/// A page process that dies with its test, even when the test panics.
struct Page(std::process::Child);

impl Drop for Page {
    fn drop(&mut self) {
        let _killed = self.0.kill();
        let _reaped = self.0.wait();
    }
}

fn home(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("jaynes-page-fixes-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("home")).expect("dir");
    base
}

/// The page on `base`, with the key given or, without one, the key it keeps in its home; what it says goes to
/// `said.txt`, as it goes to page.log when Jaynes starts it.
fn page(base: &Path, port: u16, key: Option<&str>) -> Page {
    let said = std::fs::File::create(base.join("said.txt")).expect("said");
    let mut c = Command::new(BIN);
    c.args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", base.join("home")).env("JAYNES_HOME", base.join("jaynes")).env("JAYNES_NO_PAGE", "1").env("JAYNES_NO_UPDATE_CHECK", "1")
        .env_remove("JAYNES_PAGE_KEY")
        .stdout(said.try_clone().expect("said"))
        .stderr(said);
    if let Some(k) = key {
        c.env("JAYNES_PAGE_KEY", k);
    }
    let child = c.spawn().map(Page).expect("page");
    let until = Instant::now() + Duration::from_secs(5);
    while TcpStream::connect(("127.0.0.1", port)).is_err() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    child
}

/// A request and its reply, or what reading it said, within `wait`.
fn ask(port: u16, raw: &[u8], wait: Duration) -> Result<String, String> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(wait)).map_err(|e| e.to_string())?;
    s.write_all(raw).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    match s.read_to_end(&mut out) {
        Ok(_) => Ok(String::from_utf8_lossy(&out).into_owned()),
        Err(e) if !out.is_empty() => Err(format!("{e} after {}", String::from_utf8_lossy(&out))),
        Err(e) => Err(e.to_string()),
    }
}

fn get(port: u16, path: &str) -> Result<String, String> {
    ask(port, format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes(), Duration::from_secs(3))
}

fn port(n: u16) -> u16 {
    22750 + (std::process::id() % 900) as u16 * 3 % 2700 + n
}

#[test]
fn a_request_that_breaks_its_handler_does_not_stop_the_page() {
    let base = home("broken");
    let port = port(0);
    let _page = page(&base, port, Some("k3y"));
    // a name as long as a kept page's (69 bytes), whose 64th byte falls inside a letter of two bytes
    let name = format!("{}éhtml", "a".repeat(63));
    assert_eq!(name.len(), 69);
    let reply = get(port, &format!("/page/{name}")).expect("an answer");
    assert!(reply.starts_with("HTTP/1.1 404"), "{reply}");
    let home = get(port, "/home").expect("the page is still up");
    assert!(home.starts_with("HTTP/1.1 200"), "{home}");
}

#[test]
fn a_connection_that_says_nothing_holds_no_other() {
    let base = home("silent");
    let port = port(1);
    let _page = page(&base, port, Some("k3y"));
    // an agent on the machine opens a connection and says nothing, and another sends a head that never ends
    let silent = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let mut endless = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    endless.write_all(format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Pad: ").as_bytes()).expect("write");
    std::thread::sleep(Duration::from_millis(200));
    // the person is still answered at once: what the page reads to follow the record, and a Halt
    let started = Instant::now();
    let version = get(port, "/api/version").expect("version");
    assert!(version.starts_with("HTTP/1.1 200") && version.contains("\"seq\""), "{version}");
    let body = r#"{"run":"no-such-run","action":"halt"}"#;
    let halt = ask(
        port,
        format!("POST /api/act HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nContent-Type: application/json\r\nCookie: jaynes_key=k3y\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes(),
        Duration::from_secs(3),
    )
    .expect("halt");
    assert!(halt.starts_with("HTTP/1.1 200") && halt.contains("no such run"), "{halt}");
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    drop(silent);
    // a head larger than any the page sends is refused, and the page goes on
    let huge = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Pad: {}\r\n\r\n", "a".repeat(200 * 1024));
    let refused = ask(port, huge.as_bytes(), Duration::from_secs(3)).unwrap_or_default();
    assert!(!refused.starts_with("HTTP/1.1 200"), "{}", refused.chars().take(200).collect::<String>());
    assert!(get(port, "/api/version").expect("still up").starts_with("HTTP/1.1 200"));
    drop(endless);
}
