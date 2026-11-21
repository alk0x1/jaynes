//! Opening the page: the app's "Open the page" and `jaynes open` open it with its key whether it was up or not, a
//! start opens a tab only when no tab with the key asked the page lately, the key is never printed, and the page says
//! its version, shows its icon and tells the person where to open it.
//!
//! Nothing here opens a browser: `open` (and `xdg-open`) on this process's PATH is a stand-in that writes what it was
//! given, and fails for a port the test marks.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use jaynes_shell::install::{ensure_page_at, ensure_page_on, ensure_page_with, Open};

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");
const KEY: &str = "0123456789abcdef0123456789abcdef";

/// The folder every test here shares: the stand-in browser and what it was given. A test's body runs in a child of
/// this test binary whose PATH has the stand-in first from its start, since this crate changes no environment in a
/// running process.
fn shared() -> &'static Path {
    static AT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    AT.get_or_init(|| {
        if let Ok(at) = std::env::var("JAYNES_PAGE_OPEN_SHARED") {
            return PathBuf::from(at);
        }
        let at = std::env::temp_dir().join(format!("jaynes-page-open-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&at);
        std::fs::create_dir_all(at.join("bin")).expect("bin");
        std::fs::create_dir_all(at.join("fail")).expect("fail");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\nport=${{1#http://localhost:}}\nport=${{port%%/*}}\nif [ -e '{fail}/'\"$port\" ]; then echo 'no browser here' >&2; exit 1; fi\nexit 0\n",
            log = at.join("opened.log").display(),
            fail = at.join("fail").display()
        );
        for name in ["open", "xdg-open"] {
            let p = at.join("bin").join(name);
            std::fs::write(&p, &script).expect("stand-in");
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("mode");
        }
        at
    })
}

/// Whether this process runs the body of test `name`: in the child it does; in the test itself, the child is started
/// with the stand-in first on its PATH and a home of its own, and must pass.
fn in_child(name: &str) -> bool {
    if std::env::var("JAYNES_PAGE_OPEN_SHARED").is_ok() {
        return true;
    }
    let at = shared();
    let path = format!("{}:{}", at.join("bin").display(), std::env::var("PATH").unwrap_or_default());
    let found = Command::new("sh").args(["-c", "command -v open"]).env("PATH", &path).output().expect("sh");
    assert_eq!(String::from_utf8_lossy(&found.stdout).trim(), at.join("bin/open").display().to_string(), "the stand-in is not the open on the PATH");
    let out = Command::new(std::env::current_exe().expect("this test"))
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .env("JAYNES_PAGE_OPEN_SHARED", at)
        .env("PATH", path)
        .env("HOME", at.join("person"))
        .env("JAYNES_HOME", at.join("jaynes"))
        .env("JAYNES_NO_UPDATE_CHECK", "1")
        .env_remove("JAYNES_PAGE_KEY")
        .output()
        .expect("the test's child");
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && said.contains("1 passed"), "{said}");
    false
}

/// What the stand-in browser was given for `port`, one link a line.
fn opened(port: u16) -> Vec<String> {
    let log = std::fs::read_to_string(shared().join("opened.log")).unwrap_or_default();
    log.lines().filter(|l| l.contains(&format!("localhost:{port}/"))).map(str::to_string).collect()
}

fn link(port: u16) -> String {
    format!("http://localhost:{port}/#key={KEY}")
}

/// A test's engine home, with the page's key, taken away when the test ends.
struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _gone = std::fs::remove_dir_all(&self.0);
    }
}

fn home(name: &str) -> Home {
    let at = shared().join(name);
    std::fs::create_dir_all(&at).expect("home");
    std::fs::write(at.join("page.key"), KEY).expect("key");
    Home(at)
}

/// The page this test started on `port`, stopped when the test ends: only the process listening there that is this
/// build's page on that port, by its pid.
struct Up(u16);

impl Drop for Up {
    fn drop(&mut self) {
        let Ok(out) = Command::new("/usr/sbin/lsof").args(["-nP", &format!("-iTCP:{}", self.0), "-sTCP:LISTEN", "-t"]).output() else { return };
        for pid in String::from_utf8_lossy(&out.stdout).split_whitespace() {
            let cmd = Command::new("/bin/ps").args(["-o", "command=", "-p", pid]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
            if cmd.starts_with(BIN) && cmd.contains(&format!("page --port {}", self.0)) {
                let _stopped = Command::new("/bin/kill").arg(pid).status();
            }
        }
    }
}

/// A port nothing answers on, on either address.
fn port() -> u16 {
    for _ in 0..200 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let p = l.local_addr().expect("address").port();
        drop(l);
        let six = !std::net::TcpListener::bind(("::1", p)).is_err_and(|e| e.kind() == std::io::ErrorKind::AddrInUse);
        if six {
            return p;
        }
    }
    panic!("no free port")
}

fn ask(port: u16, raw: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout");
    s.write_all(raw.as_bytes()).expect("write");
    let mut out = String::new();
    let _read = s.read_to_string(&mut out);
    out
}

fn get(port: u16, path: &str) -> String {
    ask(port, &format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n"))
}

/// A post to the page, as its script sends it: from its origin, with `key` in the header when given.
fn post(port: u16, path: &str, key: Option<&str>, origin: bool, body: &str) -> String {
    let key = key.map(|k| format!("X-Jaynes-Key: {k}\r\n")).unwrap_or_default();
    let origin = if origin { format!("Origin: http://localhost:{port}\r\n") } else { String::new() };
    ask(
        port,
        &format!("POST {path} HTTP/1.1\r\nHost: localhost:{port}\r\n{origin}{key}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()),
    )
}

/// The page up on `port`, started as a start starts it, with no link opened.
fn up(home: &Home, port: u16) -> Up {
    let up = Up(port);
    assert_eq!(ensure_page_on(&home.0, port, Path::new(BIN), false), Ok(format!("http://localhost:{port}/")));
    up
}

// Alan, 20/11: "ficar rodando open site com a key toda hora é foda": the app's "Open the page" opens it with its key,
// up or not
#[test]
fn asked_no_open_site_with_the_key_every_time_the_app_opens_the_page_with_its_key_up_or_not() {
    if !in_child("asked_no_open_site_with_the_key_every_time_the_app_opens_the_page_with_its_key_up_or_not") {
        return;
    }
    let h = home("app");
    // not up: it starts and opens
    let p = port();
    let _up = Up(p);
    assert_eq!(ensure_page_on(&h.0, p, Path::new(BIN), true), Ok(format!("http://localhost:{p}/")));
    assert_eq!(opened(p), vec![link(p)]);
    // already up: it opens again, as the app's button asks
    let q = port();
    let _upq = up(&h, q);
    assert!(opened(q).is_empty(), "a start with open=false opened {:?}", opened(q));
    assert_eq!(ensure_page_on(&h.0, q, Path::new(BIN), true), Ok(format!("http://localhost:{q}/")));
    assert_eq!(opened(q), vec![link(q)]);
    // and with a tab that has the key open, the button still opens
    post(q, "/api/version", Some(KEY), true, "{}");
    assert_eq!(ensure_page_on(&h.0, q, Path::new(BIN), true), Ok(format!("http://localhost:{q}/")));
    assert_eq!(opened(q).len(), 2);
}

#[test]
fn the_app_opens_a_path_of_the_page_with_its_key_and_refuses_one_that_could_go_elsewhere() {
    if !in_child("the_app_opens_a_path_of_the_page_with_its_key_and_refuses_one_that_could_go_elsewhere") {
        return;
    }
    let h = home("path");
    let p = port();
    let _up = Up(p);
    assert_eq!(ensure_page_at(&h.0, p, Path::new(BIN), "/page/x"), Ok(format!("http://localhost:{p}/page/x")));
    assert_eq!(opened(p), vec![format!("http://localhost:{p}/page/x#key={KEY}")]);
    // up already: opened again
    assert_eq!(ensure_page_at(&h.0, p, Path::new(BIN), "/q/r/s"), Ok(format!("http://localhost:{p}/q/r/s")));
    assert_eq!(opened(p).last(), Some(&format!("http://localhost:{p}/q/r/s#key={KEY}")));
    // a path that is not one of the page's, or would carry a fragment or a query beside the key: refused, nothing opened
    for bad in ["", "page/x", "//evil.example/x", "/a#key=other", "/a?b=c", "/a b", "/a\nb", "@evil.example"] {
        let said = ensure_page_at(&h.0, p, Path::new(BIN), bad).expect_err(bad);
        assert!(!said.contains(KEY), "{bad}: {said}");
    }
    assert_eq!(opened(p).len(), 2, "a refused path opened something");
}

// Alan, 20/11, the same request: a start opens the page with its key, unless a tab with it is open already
#[test]
fn asked_no_open_site_with_the_key_every_time_a_start_opens_the_page_with_it_unless_a_tab_is_open() {
    if !in_child("asked_no_open_site_with_the_key_every_time_a_start_opens_the_page_with_it_unless_a_tab_is_open") {
        return;
    }
    let h = home("start");
    let p = port();
    let _up = up(&h, p);
    // up, and no tab has asked with the key: the start opens one
    assert_eq!(ensure_page_with(&h.0, p, Path::new(BIN), Open::UnlessATabIsOpen), Ok(format!("http://localhost:{p}/")));
    assert_eq!(opened(p), vec![link(p)]);
    // a request without the key, or with it but from no origin (a program, not the page), is no tab
    post(p, "/api/version", None, true, "{}");
    post(p, "/api/version", Some(KEY), false, "{}");
    post(p, "/api/version", Some("not-the-key"), true, "{}");
    assert_eq!(ensure_page_with(&h.0, p, Path::new(BIN), Open::UnlessATabIsOpen), Ok(format!("http://localhost:{p}/")));
    assert_eq!(opened(p).len(), 2, "a request that is no tab counted as one");
    // the page's own poll, with the key: a tab is open, and the next start opens nothing
    let polled = post(p, "/api/version", Some(KEY), true, "{}");
    assert!(polled.contains("\"seq\""), "the poll still says the record's place: {polled}");
    assert_eq!(ensure_page_with(&h.0, p, Path::new(BIN), Open::UnlessATabIsOpen), Ok(format!("http://localhost:{p}/")));
    assert_eq!(opened(p).len(), 2, "a start opened a second tab");
    // whether a tab is open is all that is said, never when
    let knows = get(p, "/api/knows?nonce=00ff");
    assert!(knows.contains("\"tab\":true") && !knows.contains("\"at\""), "{knows}");
}

#[test]
fn when_the_browser_does_not_open_the_link_it_says_why_without_the_key() {
    if !in_child("when_the_browser_does_not_open_the_link_it_says_why_without_the_key") {
        return;
    }
    let h = home("fails");
    let p = port();
    let _up = up(&h, p);
    std::fs::write(shared().join("fail").join(p.to_string()), "").expect("mark");
    let said = ensure_page_on(&h.0, p, Path::new(BIN), true).expect_err("said it opened");
    assert!(said.contains("not opened") && !said.contains(KEY), "{said}");
    assert_eq!(opened(p).len(), 1, "the browser was asked once");
}

#[test]
fn jaynes_open_opens_the_page_with_its_key_and_never_prints_the_key() {
    if !in_child("jaynes_open_opens_the_page_with_its_key_and_never_prints_the_key") {
        return;
    }
    let h = home("cli");
    let p = port();
    let _up = Up(p);
    let open = |extra: &[(&str, &str)]| {
        let mut c = Command::new(BIN);
        c.args(["open", "--port", &p.to_string()]).env("JAYNES_ENGINE_HOME", &h.0).env_remove("JAYNES_NO_BROWSER").env_remove("JAYNES_NO_PAGE");
        for (k, v) in extra {
            c.env(k, v);
        }
        let out = c.output().expect("jaynes open");
        (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
    };
    // not up: started and opened
    let (ok, out, err) = open(&[]);
    assert!(ok, "{out}{err}");
    assert!(!out.contains(KEY) && !err.contains(KEY), "the key was printed: {out}{err}");
    assert_eq!(opened(p), vec![link(p)]);
    // up: opened again
    let (ok, out, err) = open(&[]);
    assert!(ok && !out.contains(KEY) && !err.contains(KEY), "{out}{err}");
    assert_eq!(opened(p).len(), 2);
    // the browser fails: why, on standard error, without the key, and a failing exit
    std::fs::write(shared().join("fail").join(p.to_string()), "").expect("mark");
    let (ok, out, err) = open(&[]);
    assert!(!ok && err.contains("not opened") && !err.contains(KEY) && !out.contains(KEY), "{ok} {out}{err}");
    std::fs::remove_file(shared().join("fail").join(p.to_string())).expect("unmark");
    // told not to open a browser: it says so and fails, asking nothing of the browser
    let (ok, _, err) = open(&[("JAYNES_NO_BROWSER", "1")]);
    assert!(!ok && err.contains("JAYNES_NO_BROWSER") && !err.contains(KEY), "{err}");
    let (ok, _, err) = open(&[("JAYNES_NO_PAGE", "1")]);
    assert!(!ok && err.contains("JAYNES_NO_PAGE") && !err.contains(KEY), "{err}");
    assert_eq!(opened(p).len(), 3, "only the failing browser was asked once more");
}

#[test]
fn what_only_the_person_does_tells_them_to_open_the_page_from_the_app_or_with_jaynes_open() {
    if !in_child("what_only_the_person_does_tells_them_to_open_the_page_from_the_app_or_with_jaynes_open") {
        return;
    }
    let h = home("refusals");
    let p = port();
    let _up = up(&h, p);
    // shown by the page as text: no backtick, which the screen would show raw
    let hint = "Open it from the Jaynes app (Open the page), or run jaynes open";
    for (path, body, start) in [
        ("/api/setup", r#"{"guard":"off"}"#, "only you change the setup: "),
        ("/api/start", r#"{"role":"small-change","project":"/tmp","request":"x"}"#, "only you start a run from the page: "),
        ("/api/reminded", r#"{"due":[]}"#, "only the person is reminded: "),
    ] {
        let said = post(p, path, None, true, body);
        assert!(said.contains(&format!("{start}{hint}")), "{path}: {said}");
        assert!(!said.contains("the page printed") && !said.contains('`'), "{path}: {said}");
    }
    // the page's own notice for a tab without the key says the same, the command as code (tests/key_js.rs)
    let script = get(p, "/key.js");
    assert!(script.contains("Open it from the Jaynes app (Open the page), or run ") && !script.contains("the page printed") && !script.contains("`jaynes open`"), "{script}");
}

// Alan, 20/11: "seria bom o app e o site mostrarem a versão atual instalada em algum lugar"
#[test]
fn asked_the_site_shows_the_version_installed_and_its_own_icon_without_the_key() {
    if !in_child("asked_the_site_shows_the_version_installed_and_its_own_icon_without_the_key") {
        return;
    }
    let h = home("looks");
    let p = port();
    let _up = up(&h, p);
    let page = get(p, "/");
    assert!(page.contains(&format!("Jaynes {}</footer>", env!("CARGO_PKG_VERSION"))), "no version at the foot: {page}");
    assert!(page.contains(r#"<link rel="icon" type="image/svg+xml" href="/favicon-light.svg" media="(prefers-color-scheme: light)">"#), "{page}");
    assert!(page.contains(r#"<link rel="icon" type="image/svg+xml" href="/favicon-dark.svg" media="(prefers-color-scheme: dark)">"#), "{page}");
    // the tab's icon drawn for 16 and 32 px, as approved: each served byte for byte, by its fingerprint
    for (name, approved) in [
        ("light", "8b9cdd6199acf1dc488cac61113a6a59510240b564b2ad53841a105db4eda554"),
        ("dark", "aa741d2e262a04da1871c0fc0069fc1cacd475be909ef993e2bc04bba2e9d1da"),
    ] {
        // no key asked: it is only an icon
        let got = get(p, &format!("/favicon-{name}.svg"));
        assert!(got.starts_with("HTTP/1.1 200"), "{name}: {got}");
        assert!(got.contains("Content-Type: image/svg+xml"), "{name}: {got}");
        let body = got.split("\r\n\r\n").nth(1).unwrap_or_default();
        assert_eq!(jaynes_core::Digest::of_bytes(body.as_bytes()).0, approved, "the {name} icon is not the one approved");
        // a picture and nothing that runs or reaches anything: no script, no link out, no foreign content
        let lower = body.to_ascii_lowercase();
        for never in ["script", "href", "url(", "foreignobject"] {
            assert!(!lower.contains(never), "{name}: {never} in {body}");
        }
    }
    // and nothing else is a file: any other name is not found
    for other in ["/favicon.svg", "/favicon-light.svg/", "/favicon-../page.key", "/favicon-light.svg%00", "/favicon.ico"] {
        let got = get(p, other);
        assert!(got.starts_with("HTTP/1.1 404") || !got.contains("<svg"), "{other}: {got}");
        assert!(!got.contains(KEY), "{other}: {got}");
    }
}

#[test]
fn jaynes_update_refuses_what_it_does_not_know_and_runs_nothing() {
    if !in_child("jaynes_update_refuses_what_it_does_not_know_and_runs_nothing") {
        return;
    }
    // every program update could run is a stand-in here that notes it ran: none may
    let at = shared().join("update");
    std::fs::create_dir_all(at.join("bin")).expect("bin");
    let ran = at.join("ran.log");
    for name in ["curl", "sh", "cargo", "claude", "git"] {
        let p = at.join("bin").join(name);
        std::fs::write(&p, format!("#!/bin/sh\necho {name} >> '{}'\nexit 1\n", ran.display())).expect("stand-in");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("mode");
    }
    for args in [&["update", "--help"][..], &["update", "now"][..], &["update", "-y"][..]] {
        let out = Command::new(BIN)
            .args(args)
            .env("PATH", at.join("bin"))
            .env("JAYNES_ENGINE_HOME", at.join("home"))
            .env("JAYNES_INSTALL_ROOT", at.join("root"))
            .env("JAYNES_RELEASES", "file:///nowhere")
            .env("JAYNES_INSTALLER", "file:///nowhere")
            .output()
            .expect("jaynes update");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?} succeeded");
        assert!(err.contains("usage: jaynes update"), "{args:?}: {err}");
        assert!(!ran.exists(), "{args:?} ran {}", std::fs::read_to_string(&ran).unwrap_or_default());
    }
}
