//! The page asks nothing of the internet to draw itself: its fonts come from the page, with their licences, and its
//! content policy names no other host.

use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

/// How long the test waits on the page: past it, it fails saying what it saw, instead of holding the queue.
const PAGE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// A page process that dies with its test, even when the test panics, and takes its folder with it.
struct Page(std::process::Child, std::path::PathBuf);

impl Drop for Page {
    fn drop(&mut self) {
        let _killed = self.0.kill();
        let _reaped = self.0.wait();
        let _gone = std::fs::remove_dir_all(&self.1);
    }
}

fn get(port: u16, path: &str) -> Vec<u8> {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(PAGE_WAIT)).expect("timeout");
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes()).expect("write");
    let mut out = Vec::new();
    let _read = s.read_to_end(&mut out);
    out
}

fn split(reply: &[u8]) -> (String, Vec<u8>) {
    let at = reply.windows(4).position(|w| w == b"\r\n\r\n").expect("a head");
    (String::from_utf8_lossy(&reply[..at]).into_owned(), reply[at + 4..].to_vec())
}

#[test]
fn the_page_draws_itself_with_fonts_of_its_own_and_reaches_no_other_host() {
    let base = std::env::temp_dir().join(format!("jaynes-page-fonts-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("home")).expect("dir");
    // on a port the system gives, which the page says in its first line
    let said_file = base.join("said.txt");
    let said = std::fs::File::create(&said_file).expect("said");
    let child = Command::new(BIN)
        .args(["page", "--port", "0"])
        .env("JAYNES_ENGINE_HOME", base.join("home")).env("JAYNES_HOME", base.join("jaynes")).env("JAYNES_NO_PAGE", "1").env("JAYNES_NO_UPDATE_CHECK", "1").env("JAYNES_PAGE_KEY", "k3y")
        .stdout(said.try_clone().expect("said"))
        .stderr(said)
        .spawn()
        .expect("page");
    let _page = Page(child, base.clone());
    let until = std::time::Instant::now() + PAGE_WAIT;
    let port = loop {
        let said = std::fs::read_to_string(&said_file).unwrap_or_default();
        if let Some(p) = said.split("the page: http://localhost:").nth(1).and_then(|r| r.split('/').next()).and_then(|p| p.parse::<u16>().ok()) {
            break p;
        }
        assert!(std::time::Instant::now() < until, "the page never said its port: {said}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    for path in ["/", "/home", "/new", "/setup"] {
        let (head, body) = split(&get(port, path));
        let body = String::from_utf8_lossy(&body);
        // nothing on the page, and nothing its policy allows, leaves this machine
        assert!(!body.contains("googleapis") && !body.contains("gstatic"), "{path}: {body}");
        // what the browser fetches to draw the page (not a link the person may follow) is the page's own
        for fetched in ["src=\"http", "src='http", "url(http", "@import"] {
            assert!(!body.contains(fetched), "{path}: {fetched} in {body}");
        }
        // a link the browser fetches (the page's icon) is to a path of the page's own, never to another host
        for link in body.split("<link").skip(1) {
            let href = link.split("href=\"").nth(1).and_then(|h| h.split('"').next()).unwrap_or("");
            assert!(href.starts_with('/') && !href.starts_with("//"), "{path}: <link{}", link.split('>').next().unwrap_or(""));
        }
        let policy = head.lines().find_map(|l| l.strip_prefix("Content-Security-Policy: ")).expect("policy");
        assert!(!policy.contains("http"), "{path}: {policy}");
        assert!(policy.contains("font-src 'self'"), "{path}: {policy}");
        assert!(body.contains("@font-face") && body.contains("url(/fonts/InstrumentSans-Variable.ttf)") && body.contains("url(/fonts/IBMPlexMono-Regular.ttf)"), "{path}");
    }
    let fonts = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts");
    for name in ["InstrumentSans-Variable.ttf", "IBMPlexMono-Regular.ttf", "IBMPlexMono-Medium.ttf"] {
        let (head, body) = split(&get(port, &format!("/fonts/{name}")));
        assert!(head.starts_with("HTTP/1.1 200") && head.contains("Content-Type: font/ttf"), "{name}: {head}");
        assert_eq!(body, std::fs::read(fonts.join(name)).expect("font"), "{name}");
        // a font does not change: the browser keeps it
        assert!(head.contains("Cache-Control: public, max-age=31536000, immutable"), "{head}");
    }
    // their licences go with them
    for name in ["OFL-InstrumentSans.txt", "OFL-IBMPlexMono.txt"] {
        let (head, body) = split(&get(port, &format!("/fonts/{name}")));
        assert!(head.starts_with("HTTP/1.1 200") && String::from_utf8_lossy(&body).contains("SIL Open Font License"), "{name}: {head}");
    }
    // nothing else is served from there
    for path in ["/fonts/none.ttf", "/fonts/../page.key", "/fonts/"] {
        let (head, _) = split(&get(port, path));
        assert!(head.starts_with("HTTP/1.1 404"), "{path}: {head}");
    }
}
