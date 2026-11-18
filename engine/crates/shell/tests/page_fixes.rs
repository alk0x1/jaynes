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

/// A test's folder, taken away when the test ends, even when it panics.
struct Base(PathBuf);

impl std::ops::Deref for Base {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Base {
    fn drop(&mut self) {
        let _gone = std::fs::remove_dir_all(&self.0);
    }
}

fn home(name: &str) -> Base {
    let base = std::env::temp_dir().join(format!("jaynes-page-fixes-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("home")).expect("dir");
    Base(base)
}

/// How long a test waits on a page: past it, the test fails saying what answered, instead of holding the queue.
const PAGE_WAIT: Duration = Duration::from_secs(10);

/// Wait until Jaynes's page answers on `port`: its /api/version, with the record's sequence, and nothing else's.
fn page_up(port: u16) {
    let until = Instant::now() + PAGE_WAIT;
    while Instant::now() < until {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            let said = get(port, "/api/version").unwrap_or_default();
            if said.starts_with("HTTP/1.1 200") && said.contains("\"seq\"") {
                return;
            }
            assert!(said.is_empty(), "something that is not Jaynes's page answers on port {port}: {}", said.chars().take(300).collect::<String>());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("Jaynes's page did not come up on port {port} within {} s", PAGE_WAIT.as_secs());
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
    page_up(port);
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
    ask(port, format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes(), Duration::from_secs(3))
}

/// A port free now on both of the page's addresses, for a page a test starts. Not the system's choice: the system
/// gives a bind on 0 a port of the range it also gives every connection that goes out (49152 and up on macOS), and a
/// connection of another test running at once could take it before the page binds it. A port drawn below that range,
/// at random among 20000, is taken by no connection, and by another test's draw only by chance. A bind alone does not
/// tell it free: the standard library binds with SO_REUSEADDR, which on macOS lets a bind on 127.0.0.1 through where
/// another program listens on every address at that port; so nothing may answer there either.
fn port(_n: u16) -> u16 {
    use std::hash::{BuildHasher, Hasher};
    let answers = |address: &str, p: u16| {
        let at: std::net::SocketAddr = format!("{address}:{p}").parse().expect("address");
        TcpStream::connect_timeout(&at, Duration::from_millis(200)).is_ok()
    };
    for _ in 0..200 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        let p = 20000 + (h.finish() % 20000) as u16;
        let four = std::net::TcpListener::bind(("127.0.0.1", p)).is_ok();
        let six = !std::net::TcpListener::bind(("::1", p)).is_err_and(|e| e.kind() == std::io::ErrorKind::AddrInUse);
        if four && six && !answers("127.0.0.1", p) && !answers("[::1]", p) {
            return p;
        }
    }
    panic!("no free port between 20000 and 40000")
}

/// A listener on 127.0.0.1 at a port the system gives, held from the start: no other test can take its port.
fn held() -> (std::net::TcpListener, u16) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
    let port = l.local_addr().expect("address").port();
    (l, port)
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
    endless.write_all(format!("GET / HTTP/1.1\r\nHost: localhost:{port}\r\nX-Pad: ").as_bytes()).expect("write");
    std::thread::sleep(Duration::from_millis(200));
    // the person is still answered at once: what the page reads to follow the record, and a Halt
    let started = Instant::now();
    let version = get(port, "/api/version").expect("version");
    assert!(version.starts_with("HTTP/1.1 200") && version.contains("\"seq\""), "{version}");
    let body = r#"{"run":"no-such-run","action":"halt"}"#;
    let halt = ask(
        port,
        format!("POST /api/act HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\nContent-Type: application/json\r\nX-Jaynes-Key: k3y\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes(),
        Duration::from_secs(3),
    )
    .expect("halt");
    assert!(halt.starts_with("HTTP/1.1 200") && halt.contains("no such run"), "{halt}");
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    drop(silent);
    // a head larger than any the page sends is refused, and the page goes on
    let huge = format!("GET / HTTP/1.1\r\nHost: localhost:{port}\r\nX-Pad: {}\r\n\r\n", "a".repeat(200 * 1024));
    let refused = ask(port, huge.as_bytes(), Duration::from_secs(3)).unwrap_or_default();
    assert!(!refused.starts_with("HTTP/1.1 200"), "{}", refused.chars().take(200).collect::<String>());
    assert!(get(port, "/api/version").expect("still up").starts_with("HTTP/1.1 200"));
    drop(endless);
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).expect("metadata").permissions().mode() & 0o777
}

#[test]
#[cfg(unix)]
fn the_key_is_the_person_s_alone_and_never_in_the_log() {
    use std::os::unix::fs::PermissionsExt;
    // made by the page: readable by its owner only, and not in what the page says
    let base = home("made");
    let port = port(2);
    let made = page(&base, port, None);
    let file = base.join("home/page.key");
    let key = std::fs::read_to_string(&file).expect("the page made its key");
    assert_eq!(mode(&file), 0o600);
    drop(made);
    let said = std::fs::read_to_string(base.join("said.txt")).expect("said");
    assert!(said.contains(&format!("the page: http://localhost:{port}/")), "{said}");
    assert!(!said.contains(key.trim()), "the key went to the log: {said}");
    // kept by an earlier version, readable by all: tightened when it is read
    let base = home("kept");
    let file = base.join("home/page.key");
    std::fs::write(&file, "0123456789abcdef0123456789abcdef").expect("write");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let _kept = page(&base, port, None);
    assert_eq!(mode(&file), 0o600);
    assert_eq!(std::fs::read_to_string(&file).expect("read"), "0123456789abcdef0123456789abcdef", "the same key goes on");
    let said = std::fs::read_to_string(base.join("said.txt")).expect("said");
    assert!(!said.contains("0123456789abcdef"), "{said}");
}

#[test]
fn a_page_that_cannot_hold_localhost_does_not_start() {
    // another program answers on [::1] at the page's port: it would answer for localhost, the page's origin
    let base = home("taken");
    let port = port(3);
    let Ok(_other) = std::net::TcpListener::bind(("::1", port)) else {
        eprintln!("no IPv6 on this machine: nothing to take");
        return;
    };
    let said_file = base.join("said.txt");
    let mut child = Command::new(BIN)
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", base.join("home")).env("JAYNES_HOME", base.join("jaynes")).env("JAYNES_NO_PAGE", "1").env("JAYNES_NO_UPDATE_CHECK", "1").env("JAYNES_PAGE_KEY", "k3y")
        .stderr(std::fs::File::create(&said_file).expect("said"))
        .spawn()
        .expect("page");
    // it ends at once, saying why; a page that went on would hold the port, so it is stopped after a while
    let until = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break Some(status);
        }
        if Instant::now() > until {
            let _killed = child.kill();
            let _reaped = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let said = std::fs::read_to_string(&said_file).unwrap_or_default();
    assert!(status.is_some_and(|s| !s.success()), "the page started anyway: {said}");
    assert!(said.contains(&format!("another program answers on [::1]:{port}")), "{said}");
}

#[test]
fn past_its_ceiling_the_page_closes_the_oldest_still_speaking_and_serves_the_new_one() {
    let base = home("ceiling");
    let port = port(4);
    let _page = page(&base, port, Some("k3y"));
    // as many silent connections as the page serves at once, on both of its addresses
    let mut silent: Vec<TcpStream> = (0..32).map(|_| TcpStream::connect(("127.0.0.1", port)).expect("connect")).collect();
    silent.extend((0..32).filter_map(|_| TcpStream::connect(("::1", port)).ok()));
    std::thread::sleep(Duration::from_millis(300));
    // the person still halts at once: the oldest connection still speaking its head makes way
    let started = Instant::now();
    let body = r#"{"run":"no-such-run","action":"halt"}"#;
    let halt = ask(
        port,
        format!("POST /api/act HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\nContent-Type: application/json\r\nX-Jaynes-Key: k3y\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes(),
        Duration::from_secs(3),
    )
    .expect("halt");
    assert!(halt.starts_with("HTTP/1.1 200") && halt.contains("no such run"), "{halt}");
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    // and the first silent one was the one closed
    let mut first = silent.remove(0);
    first.set_read_timeout(Some(Duration::from_secs(1))).expect("timeout");
    let mut b = [0u8; 1];
    assert!(matches!(first.read(&mut b), Ok(0)), "the oldest is closed");
}

#[test]
fn a_head_spoken_a_byte_at_a_time_is_cut_off_at_its_deadline() {
    let base = home("slow");
    let port = port(5);
    let _page = page(&base, port, Some("k3y"));
    let mut slow = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    slow.set_read_timeout(Some(Duration::from_millis(100))).expect("timeout");
    let head = format!("GET / HTTP/1.1\r\nHost: localhost:{port}\r\nX-Pad: aaaaaaaaaaaaaaaaaaaa");
    let started = Instant::now();
    let mut closed = None;
    // a byte every 400 ms: each read is quick, the whole head is not
    for b in head.bytes() {
        if slow.write_all(&[b]).is_err() {
            closed = Some(started.elapsed());
            break;
        }
        let mut out = [0u8; 64];
        if let Ok(0) = slow.read(&mut out) {
            closed = Some(started.elapsed());
            break;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    let closed = closed.expect("the page never closed a head that never ended");
    assert!(closed < Duration::from_secs(5), "{closed:?}");
}

const IMPOSTOR: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"seq\":123}";
const OLDER: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 15\r\n\r\n<h1>Jaynes</h1>";

/// A program on the machine that answers anything on `address`, as another program could.
fn impostor(address: &str, port: u16) -> Option<std::net::TcpListener> {
    answering(address, port, IMPOSTOR)
}

/// A program that gives every request the same reply.
fn answering(address: &str, port: u16, reply: &'static str) -> Option<std::net::TcpListener> {
    Some(answer_on(std::net::TcpListener::bind((address, port)).ok()?, reply))
}

/// A program on 127.0.0.1 at a port it holds from the start, giving every request the same reply, and its port.
fn answering_held(reply: &'static str) -> (std::net::TcpListener, u16) {
    let (l, port) = held();
    (answer_on(l, reply), port)
}

fn answer_on(l: std::net::TcpListener, reply: &'static str) -> std::net::TcpListener {
    let mine = l.try_clone().expect("clone");
    std::thread::spawn(move || {
        for s in mine.incoming().flatten() {
            let mut s = s;
            let mut head = [0u8; 2048];
            let _heard = s.read(&mut head);
            let _said = s.write_all(reply.as_bytes());
        }
    });
    l
}

fn stop_pages_on(port: u16) {
    let _stopped = Command::new("pkill").args(["-f", &format!("page --port {port}")]).status();
}

#[test]
fn the_page_is_opened_only_once_it_answers_as_itself_on_both_of_its_addresses() {
    let base = home("ensured");
    std::fs::write(base.join("home/page.key"), "0123456789abcdef0123456789abcdef").expect("key");
    let exe = Path::new(BIN);
    // nothing there: the page starts, proves itself on both addresses, and the link would open
    let port = port(6);
    let log = || std::fs::read_to_string(base.join("home/page.log")).unwrap_or_default();
    let opened = jaynes_shell::install::ensure_page_on(&base.join("home"), port, exe, false);
    assert_eq!(opened, Ok(format!("http://localhost:{port}/")), "the page said: {}", log());
    // and a second time it is found running, as itself
    let again = jaynes_shell::install::ensure_page_on(&base.join("home"), port, exe, false);
    assert_eq!(again, Ok(format!("http://localhost:{port}/")), "the page said: {}", log());
    stop_pages_on(port);
    // another program on ::1, where the browser goes for localhost: the link, key and all, is never opened
    let port = crate::port(7);
    if let Some(_six) = impostor("::1", port) {
        let refused = jaynes_shell::install::ensure_page_on(&base.join("home"), port, exe, false);
        let said = refused.expect_err("opened with another program on ::1");
        assert!(said.contains(&format!("[::1]:{port}")) && said.contains("not opened"), "{said}");
        stop_pages_on(port);
    }
    // another program on 127.0.0.1: the page is not taken for running, and the link is not opened
    let (_four, port) = answering_held(IMPOSTOR);
    let refused = jaynes_shell::install::ensure_page_on(&base.join("home"), port, exe, false);
    let said = refused.expect_err("opened with another program on 127.0.0.1");
    assert!(said.contains(&format!("127.0.0.1:{port}")) && said.contains("not opened"), "{said}");
    stop_pages_on(port);
}

#[test]
fn an_older_page_still_on_one_address_with_another_program_on_the_other_is_never_given_the_link() {
    let base = home("older");
    std::fs::write(base.join("home/page.key"), "0123456789abcdef0123456789abcdef").expect("key");
    // a page of an earlier version knows nothing of the proof: it answers any path with its home screen
    let (_old, port) = answering_held(OLDER);
    let other = impostor("::1", port);
    let refused = jaynes_shell::install::ensure_page_on(&base.join("home"), port, Path::new(BIN), false);
    let said = refused.expect_err("opened with an older page and another program");
    assert!(said.contains("not opened"), "{said}");
    if other.is_some() {
        assert!(said.contains(&format!("[::1]:{port}")), "the program on ::1 is named: {said}");
    }
    stop_pages_on(port);
    // and an older page alone is not taken for this one either, and says what it may be
    let (_old, port) = answering_held(OLDER);
    let alone = jaynes_shell::install::ensure_page_on(&base.join("home"), port, Path::new(BIN), false).expect_err("opened an older page");
    assert!(alone.contains(&format!("127.0.0.1:{port}")) && alone.contains("an older page"), "{alone}");
    stop_pages_on(port);
}

#[test]
fn only_the_page_s_own_name_and_numbers_reach_it_and_a_number_with_an_origin_is_refused() {
    let base = home("names");
    let port = port(9);
    let _page = page(&base, port, Some("k3y"));
    for host in [format!("localhost.evil.example:{port}"), format!("localhost:{port}.evil.example"), format!("localhost:{port}0"), format!("127.0.0.1:{port}0"), "localhost".to_string()] {
        let reply = ask(port, format!("GET /api/version HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes(), Duration::from_secs(3)).expect("reply");
        assert!(reply.starts_with("HTTP/1.1 403"), "{host}: {reply}");
    }
    // a program may speak by number, never a browser: a number with an origin is refused
    let reply = ask(
        port,
        format!("POST /api/whoami HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://localhost:{port}\r\nContent-Type: application/json\r\nX-Jaynes-Key: k3y\r\nContent-Length: 2\r\n\r\n{{}}").as_bytes(),
        Duration::from_secs(3),
    )
    .expect("reply");
    assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
    // a target that is not a path is never sent on
    let reply = ask(port, format!("GET http://evil.example/x HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes(), Duration::from_secs(3)).expect("reply");
    assert!(!reply.starts_with("HTTP/1.1 308") && !reply.contains("evil"), "{reply}");
    // the reading of the person's words answers with nosniff like the rest, the person's and the refusal alike
    let reply = ask(
        port,
        format!("POST /api/understand HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\nContent-Type: application/json\r\nX-Jaynes-Key: k3y\r\nContent-Length: 2\r\n\r\n{{}}").as_bytes(),
        Duration::from_secs(3),
    )
    .expect("reply");
    assert!(reply.starts_with("HTTP/1.1 200") && reply.contains("X-Content-Type-Options: nosniff\r\n"), "{reply}");
    let reply = ask(
        port,
        format!("POST /api/understand HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}").as_bytes(),
        Duration::from_secs(3),
    )
    .expect("reply");
    assert!(reply.starts_with("HTTP/1.1 403") && reply.contains("X-Content-Type-Options: nosniff\r\n"), "{reply}");
}

#[test]
fn the_page_proves_it_holds_the_key_without_saying_it() {
    let base = home("proof");
    let port = port(10);
    let _page = page(&base, port, Some("k3y"));
    let reply = get(port, "/api/knows?nonce=00ff").expect("reply");
    let proof = reply.split("\"proof\":\"").nth(1).and_then(|p| p.split('"').next()).expect("a proof");
    assert_eq!(proof, jaynes_shell::page::proof("k3y", "00ff"));
    assert_ne!(proof, jaynes_shell::page::proof("k3y", "00fe"));
    assert!(!reply.contains("k3y"), "{reply}");
}

#[test]
#[ignore = "holds about a gigabyte while it runs (64 replies of 10 MB, which the system keeps up to 8 MB of each); run by hand: cargo test --test page_fixes -- --ignored"]
fn past_its_ceiling_the_page_closes_the_oldest_even_while_it_takes_its_reply() {
    // a program sends 64 requests, each quick and whole, for a large kept page, and never reads what comes back: each
    // holds its place while its reply waits to be taken, past what the system keeps of a reply for a connection (up to
    // 4 MB sent and 4 MB received on macOS). The person's Halt is still served, at once. The rule itself is tested
    // without the memory in page.rs
    let base = home("readers");
    let name = format!("{}.html", "ab".repeat(32));
    std::fs::create_dir_all(base.join("home/pictures")).expect("dir");
    std::fs::write(base.join("home/pictures").join(&name), "<p>a page an agent wrote</p>".repeat(360_000)).expect("page");
    let port = port(13);
    let _page = page(&base, port, Some("k3y"));
    let readers: Vec<TcpStream> = (0..64)
        .map(|_| {
            let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
            s.write_all(format!("GET /page/{name} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes()).expect("write");
            s
        })
        .collect();
    std::thread::sleep(Duration::from_millis(800));
    let started = Instant::now();
    let body = r#"{"run":"no-such-run","action":"halt"}"#;
    let halt = ask(
        port,
        format!("POST /api/act HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\nContent-Type: application/json\r\nX-Jaynes-Key: k3y\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes(),
        Duration::from_secs(3),
    );
    assert!(halt.as_ref().is_ok_and(|h| h.starts_with("HTTP/1.1 200") && h.contains("no such run")), "{halt:?}");
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    drop(readers);
}

#[test]
fn the_page_proves_itself_only_to_a_nonce_in_hex_and_short() {
    let base = home("nonce");
    let port = port(14);
    let _page = page(&base, port, Some("k3y"));
    let ok = |nonce: &str| get(port, &format!("/api/knows?nonce={nonce}")).expect("reply");
    assert!(ok(&"0f".repeat(32)).starts_with("HTTP/1.1 200"), "64 hex digits are a nonce");
    for bad in ["", "zz", "0x12", "12%20", &"0f".repeat(32).replace("0f0f", "0f0g"), &format!("{}0", "0f".repeat(32))] {
        let reply = ok(bad);
        assert!(reply.starts_with("HTTP/1.1 400") && !reply.contains("proof"), "{bad:?}: {reply}");
    }
    assert!(get(port, "/api/knows").expect("reply").starts_with("HTTP/1.1 400"), "no nonce");
}

#[test]
fn on_port_0_the_page_takes_the_port_it_is_given_on_both_addresses_and_says_it() {
    // tests start pages side by side: each asks for any port and reads the one it got from the page's first line
    let base = home("any-port");
    let said_file = base.join("said.txt");
    let said = std::fs::File::create(&said_file).expect("said");
    let child = Command::new(BIN)
        .args(["page", "--port", "0"])
        .env("JAYNES_ENGINE_HOME", base.join("home")).env("JAYNES_HOME", base.join("jaynes")).env("JAYNES_NO_PAGE", "1").env("JAYNES_NO_UPDATE_CHECK", "1").env("JAYNES_PAGE_KEY", "k3y")
        .stdout(said.try_clone().expect("said"))
        .stderr(said)
        .spawn()
        .map(Page)
        .expect("page");
    let until = Instant::now() + Duration::from_secs(5);
    let port = loop {
        let said = std::fs::read_to_string(&said_file).unwrap_or_default();
        if let Some(p) = said.split("the page: http://localhost:").nth(1).and_then(|r| r.split('/').next())
            && let Ok(p) = p.parse::<u16>()
        {
            break p;
        }
        assert!(Instant::now() < until, "the page never said its port: {said}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_ne!(port, 0, "the page says the port it holds, not the one it was asked for");
    assert!(get(port, "/api/version").expect("answers on 127.0.0.1").starts_with("HTTP/1.1 200"));
    if let Ok(mut six) = TcpStream::connect(("::1", port)) {
        six.set_read_timeout(Some(Duration::from_secs(3))).expect("timeout");
        six.write_all(format!("GET /api/knows?nonce=01 HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes()).expect("write");
        let mut out = String::new();
        let _read = six.read_to_string(&mut out);
        assert!(out.contains(&jaynes_shell::page::proof("k3y", "01")), "another program on [::1]:{port}: {out}");
    }
    // the person is told by the origin of that port: the page holds it as its own
    let body = r#"{"run":"no-such-run","action":"halt"}"#;
    let halt = ask(
        port,
        format!("POST /api/act HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\nContent-Type: application/json\r\nX-Jaynes-Key: k3y\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes(),
        Duration::from_secs(3),
    )
    .expect("halt");
    assert!(halt.starts_with("HTTP/1.1 200") && halt.contains("no such run"), "{halt}");
    drop(child);
}
