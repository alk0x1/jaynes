//! The page's key, end to end: it never rides in a cookie, which every server on the machine's name receives whatever its
//! port, and only a request from the page's own origin that carries it counts as the person. What the page shows
//! from agents never runs as a script there, since a script of the page's origin can read the key.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_jaynes");

/// How long a test waits on a page: past it, the test fails saying what answered, instead of holding the queue.
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

/// A port free now, the system's choice, so two runs of these tests at once never ask for the same one.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).expect("a free port")
}

/// Wait until Jaynes's page answers on `port`: its /api/version, with the record's sequence, and nothing else's.
fn page_up(port: u16) {
    let until = std::time::Instant::now() + PAGE_WAIT;
    while std::time::Instant::now() < until {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            let said = http(port, &format!("GET /api/version HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"));
            if said.starts_with("HTTP/1.1 200") && said.contains("\"seq\"") {
                return;
            }
            assert!(said.is_empty(), "something that is not Jaynes's page answers on port {port}: {}", said.chars().take(300).collect::<String>());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("Jaynes's page did not come up on port {port} within {} s", PAGE_WAIT.as_secs());
}

fn http(port: u16, raw: &str) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(PAGE_WAIT)).expect("timeout");
    s.set_write_timeout(Some(PAGE_WAIT)).expect("timeout");
    s.write_all(raw.as_bytes()).expect("write");
    let mut out = String::new();
    let _read = s.read_to_string(&mut out);
    out
}

fn get(port: u16, path: &str, extra: &str) -> String {
    http(port, &format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\n{extra}\r\n"))
}

/// A POST with the headers given, each line ending in \r\n.
fn post(port: u16, path: &str, headers: &str) -> String {
    let body = "{}";
    http(port, &format!("POST {path} HTTP/1.1\r\nHost: localhost:{port}\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\n\r\n{body}", body.len()))
}

fn json(reply: &str) -> serde_json::Value {
    serde_json::from_str(reply.split("\r\n\r\n").nth(1).unwrap_or("")).unwrap_or(serde_json::Value::Null)
}

/// A home with one run whose agent wrote markup into what the person is shown, waiting at its stop, and the page
/// on it. The page's stderr, where it prints its link, goes to `said.txt`.
fn page(name: &str) -> (Page, u16, std::path::PathBuf) {
    let base = std::env::temp_dir().join(format!("jaynes-page-key-{name}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(&project).expect("dir");
    std::fs::write(project.join("greeting.txt"), "hello\n").expect("write");
    std::fs::write(project.join("Makefile"), "test:\n\tgrep -q hi greeting.txt\n").expect("write");
    for args in [vec!["init", "-q", "-b", "main"], vec!["add", "-A"], vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]] {
        assert!(Command::new("git").arg("-C").arg(&project).args(&args).status().expect("git").success(), "git {args:?}");
    }
    let home = base.join("home");
    let role = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/roles/small-change");
    // the Makefile changed raises the risk, so the person is shown the change: the agent writes markup into the file
    // it changes, whose diff the stop shows, and into the name of a new file, with quotes
    let output = r#"printf '{"summary":"hi","files":["greeting.txt"]}' > .jaynes/output.json"#;
    let agent = format!(
        "command:printf 'hi <script>document.title=1</script><img src=x onerror=alert(2)>\\n' > greeting.txt && printf 'x\\n' > 'q\"><img src=y onerror=alert(3)>.txt' && printf 'test:\\n\\ttrue\\n' > Makefile && {output}"
    );
    let started = Command::new(BIN)
        .args(["start", "--run", "marked", "--role", role.to_str().expect("path"), "--project", project.to_str().expect("path"), "--request", "<b>say hi</b>", "--agent", &agent])
        .env("JAYNES_ENGINE_HOME", &home).env("JAYNES_NO_PAGE", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "")
        .output()
        .expect("start");
    assert!(started.status.success(), "{}{}", String::from_utf8_lossy(&started.stdout), String::from_utf8_lossy(&started.stderr));
    let port = free_port();
    let said = std::fs::File::create(base.join("said.txt")).expect("said");
    let child = Command::new(BIN)
        .args(["page", "--port", &port.to_string()])
        .env("JAYNES_ENGINE_HOME", &home).env("JAYNES_HOME", base.join("jaynes")).env("JAYNES_NO_PAGE", "1").env("JAYNES_NO_UPDATE_CHECK", "1").env("JAYNES_PYTHON", "python3").env("PYTHONPATH", "").env("JAYNES_PAGE_KEY", "k3y")
        .stderr(said)
        .spawn()
        .expect("page");
    let page = Page(child, base.clone());
    page_up(port);
    (page, port, base)
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_page_s_key_never_rides_in_a_cookie() {
    let (_page, port, base) = page("cookie");
    // the page says where its key is, never the key, and the way in it names carries it in the fragment, which a
    // browser never sends to any server
    let said = std::fs::read_to_string(base.join("said.txt")).expect("said");
    assert!(said.contains(&format!("the page: http://localhost:{port}/ . Open it with its key: http://localhost:{port}/#key= followed by")), "{said}");
    assert!(!said.contains("k3y"), "{said}");
    // an old link with the key in its query still works once: it goes to the fragment and clears the old cookie
    let old = get(port, "/run/marked?key=k3y", "");
    assert!(old.starts_with("HTTP/1.1 303"), "{old}");
    assert!(old.contains("Location: /#key=k3y\r\n"), "{old}");
    // never to where the address said: a path that starts with // would leave for another host
    let away = get(port, "//evil.example/x?key=k3y", "");
    assert!(away.contains("Location: /#key=k3y\r\n") && !away.contains("evil"), "{away}");
    assert!(old.contains("Set-Cookie: jaynes_key=; Path=/; Max-Age=0"), "{old}");
    // a wrong key in the query is no way in, and sets nothing
    let wrong = get(port, "/?key=nope", "");
    assert!(wrong.starts_with("HTTP/1.1 200") && !wrong.contains("Set-Cookie") && !wrong.contains("#key="), "{wrong}");
    // a browser that still holds the old cookie has it cleared at its next visit
    let held = get(port, "/", "Cookie: jaynes_key=k3y\r\n");
    assert!(held.contains("Set-Cookie: jaynes_key=; Path=/; Max-Age=0"), "{held}");
    // and the page never sets one with the key in it
    for path in ["/", "/home", "/run/marked", "/new", "/setup", "/page.js"] {
        let reply = get(port, path, "");
        assert!(!reply.contains("Set-Cookie"), "{path}: {reply}");
    }
    // the old cookie no longer makes anyone the person
    let r = json(&post(port, "/api/whoami", &format!("Origin: http://localhost:{port}\r\nCookie: jaynes_key=k3y\r\n")));
    assert_eq!(r["person"], false, "{r}");
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn only_a_request_from_the_page_s_own_origin_with_its_key_counts_as_the_person() {
    let (_page, port, _base) = page("origin");
    let ours = format!("Origin: http://localhost:{port}\r\n");
    let key = "X-Jaynes-Key: k3y\r\n";
    assert_eq!(json(&post(port, "/api/whoami", &format!("{ours}{key}")))["person"], true);
    // the page's one origin is localhost: 127.0.0.1 names the same machine but is not the page's origin
    let numbers = format!("Origin: http://127.0.0.1:{port}\r\n");
    assert!(post(port, "/api/whoami", &format!("{numbers}{key}")).starts_with("HTTP/1.1 403"));
    let by_number = |raw: String| http(port, &raw);
    // an old link or bookmark by number goes on to localhost, path and query kept (the browser takes the fragment along)
    let old = by_number(format!("GET /run/marked?x=1 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: jaynes_key=k3y\r\n\r\n"));
    assert!(old.starts_with("HTTP/1.1 308") && old.contains(&format!("Location: http://localhost:{port}/run/marked?x=1\r\n")), "{old}");
    // and the cookie an earlier version kept for that name is cleared there, since cookies are kept by name
    assert!(old.contains("Set-Cookie: jaynes_key=; Path=/; Max-Age=0"), "{old}");
    let away = by_number(format!("GET //evil.example/x HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"));
    assert!(away.contains(&format!("Location: http://localhost:{port}//evil.example/x\r\n")), "never another host: {away}");
    // an agent or the MCP server, which send no origin, may still speak by number, as an agent
    let agent = by_number(format!("POST /api/whoami HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n{key}Content-Length: 2\r\n\r\n{{}}"));
    assert_eq!(json(&agent)["person"], false, "{agent}");
    // the key alone, from a program that sends no origin, is not the person
    assert_eq!(json(&post(port, "/api/whoami", key))["person"], false);
    // a wrong key, or none, is not the person
    assert_eq!(json(&post(port, "/api/whoami", &format!("{ours}X-Jaynes-Key: k3y0\r\n")))["person"], false);
    assert_eq!(json(&post(port, "/api/whoami", &ours))["person"], false);
    // another app on this machine, or any site, is refused outright, key or not
    let alike = format!("http://localhost:{port}.evil.example");
    let numbered = format!("http://127.0.0.1:{port}");
    for other in ["http://127.0.0.1:3000", "http://localhost:5173", "https://example.com", "null", alike.as_str(), numbered.as_str()] {
        let reply = post(port, "/api/whoami", &format!("Origin: {other}\r\n{key}"));
        assert!(reply.starts_with("HTTP/1.1 403"), "{other}: {reply}");
        let reply = post(port, "/api/answer", &format!("Origin: {other}\r\n{key}"));
        assert!(reply.starts_with("HTTP/1.1 403"), "{other}: {reply}");
    }
    // nothing lets another origin read the page's replies
    assert!(!post(port, "/api/whoami", &format!("{ours}{key}")).contains("Access-Control-Allow-Origin"));
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn nothing_an_agent_writes_runs_as_a_script_on_the_page() {
    let (_page, port, _base) = page("script");
    let (_, log) = (0, Command::new(BIN).args(["log", "marked"]).env("JAYNES_ENGINE_HOME", _base.join("home")).output().expect("log"));
    let opened: serde_json::Value = String::from_utf8_lossy(&log.stdout).lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).find(|e| e["type"] == "QuestionOpened").expect("the stop asks");
    let q = opened["question"]["id"].as_str().expect("id");
    for path in ["/".to_string(), "/home".into(), "/run/marked".into(), format!("/q/marked/{q}"), "/new".into(), "/setup".into()] {
        let reply = get(port, &path, "");
        assert!(reply.starts_with("HTTP/1.1 200"), "{path}: {reply}");
        // the policy: scripts only from the page itself, never inline, and nothing fetched from elsewhere
        let policy = reply.lines().find_map(|l| l.strip_prefix("Content-Security-Policy: ")).unwrap_or_else(|| panic!("{path} has no policy"));
        let script = policy.split(';').map(str::trim).find(|d| d.starts_with("script-src")).expect("script-src");
        assert_eq!(script, "script-src 'self'", "{path}: {policy}");
        for d in ["default-src 'none'", "connect-src 'self'", "base-uri 'none'", "frame-ancestors 'none'", "form-action 'self'"] {
            assert!(policy.contains(d), "{path}: {d} missing in {policy}");
        }
        let body = reply.split("\r\n\r\n").nth(1).unwrap_or("");
        // every script tag is the page's own file or data that does not run
        for tag in body.split("<script").skip(1) {
            let head = tag.split('>').next().unwrap_or("");
            assert!(head == " src=\"/key.js\"" || head == " src=\"/page.js\"" || head.contains("type=\"application/json\""), "{path}: <script{head}>");
        }
        // what the agent and the request wrote is shown as text
        assert!(!body.contains("<script>document.title") && !body.contains("<img src=") && !body.contains("<b>say hi</b>") && !body.contains("q\"><img"), "{path}: {body}");
    }
    let stop = get(port, &format!("/q/marked/{q}"), "");
    assert!(stop.contains("hi &lt;script&gt;document.title=1&lt;/script&gt;&lt;img src=x onerror=alert(2)&gt;"), "{stop}");
    // git shows that name quoted, with a backslash before its quote: escaped all the same
    assert!(stop.contains("q\\&quot;&gt;&lt;img src=y onerror=alert(3)&gt;.txt"), "{stop}");
    for script in ["/key.js", "/page.js"] {
        let js = get(port, script, "");
        assert!(js.starts_with("HTTP/1.1 200") && js.contains("Content-Type: text/javascript; charset=utf-8"), "{js}");
    }
    // nothing is taken for what it is not: an agent's page is never a script
    for path in ["/", "/key.js", "/page.js", "/page/none.html", "/api/version"] {
        assert!(get(port, path, "").contains("X-Content-Type-Options: nosniff\r\n"), "{path}");
    }
    assert!(post(port, "/api/whoami", "Origin: http://127.0.0.1:1\r\n").contains("X-Content-Type-Options: nosniff\r\n"));
}

#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "needs the box for project commands, which Linux does not have yet (slice 2)")]
fn the_page_is_reached_by_localhost_on_both_of_its_addresses() {
    let (_page, port, _base) = page("both");
    for address in ["127.0.0.1", "::1"] {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect((address, port)).unwrap_or_else(|e| panic!("{address}: {e}"));
        s.write_all(format!("GET /api/version HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes()).expect("write");
        let mut out = String::new();
        let _read = s.read_to_string(&mut out);
        assert!(out.starts_with("HTTP/1.1 200"), "{address}: {out}");
    }
}
