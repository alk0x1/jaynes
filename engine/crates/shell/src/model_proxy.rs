//! How an agent program in Jaynes's box reaches its model, and nothing else: a proxy outside the box, in the engine,
//! for the length of one turn. The box lets the program reach only the proxy's port (on macOS, one local port of the
//! box's list; on Linux, when its box comes, one more port the relay carries); the program is told it in
//! HTTPS_PROXY.
//!
//! The host is the agent's adapter's, written in the code (Codex: api.openai.com), never the agent's nor the role's.
//! The proxy takes one CONNECT a connection, to that host and port 443 exactly: no address by number, no other name,
//! no other port, no plain HTTP. It resolves the name itself and refuses an address that is local, private or
//! link-local. After its 200 it reads the first message of TLS and requires its server name to be the host; only
//! then does it reach the host, and from there it copies bytes both ways and never reads them. What it let through
//! and what it refused, with why, is the turn's tally.
//!
//! A test build (`--features test-model-host`) sends the host to the address JAYNES_TEST_MODEL_AT names, a server
//! of the test's; a build for people refuses to start a proxy with that variable set.

use std::io::{Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);
/// The most a request's head may hold, and the most refusals a turn keeps word for word.
const HEAD: usize = 8192;
const KEPT: usize = 50;

/// What a turn's proxy let through and refused.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Tally {
    pub reached: u32,
    pub refused: Vec<String>,
    /// Refusals past the ones kept word for word.
    pub more: u32,
}

impl Tally {
    fn refuse(&mut self, why: String) {
        if self.refused.len() < KEPT {
            self.refused.push(why);
        } else {
            self.more += 1;
        }
    }
}

/// Where a name is: the system's resolver, or a test's.
pub type Resolve = Arc<dyn Fn(&str, u16) -> Vec<SocketAddr> + Send + Sync>;

pub struct Proxy {
    pub port: u16,
    host: String,
    stop: Arc<AtomicBool>,
    tally: Arc<Mutex<Tally>>,
    accepting: Option<std::thread::JoinHandle<()>>,
}

/// The proxy for `host`, as this build makes it: the system's resolver, and only public addresses; in a test build,
/// the address JAYNES_TEST_MODEL_AT names when it is set.
pub fn start(host: &str) -> Result<Proxy, String> {
    let test_at = std::env::var("JAYNES_TEST_MODEL_AT").ok();
    if cfg!(feature = "test-model-host") {
        if let Some(at) = test_at {
            let at: SocketAddr = at.parse().map_err(|e| format!("JAYNES_TEST_MODEL_AT is no address: {e}"))?;
            return start_with(host, Arc::new(move |_, _| vec![at]), true);
        }
    } else if test_at.is_some() {
        return Err("JAYNES_TEST_MODEL_AT is read only by a test build of Jaynes (--features test-model-host): this build refuses to run an agent with it set, since it would send the agent's model elsewhere".into());
    }
    start_with(host, Arc::new(|name, port| (name, port).to_socket_addrs().map(Iterator::collect).unwrap_or_default()), false)
}

/// The proxy for `host` with `resolve`; `local` lets it reach an address of this machine, which only a test asks.
pub fn start_with(host: &str, resolve: Resolve, local: bool) -> Result<Proxy, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("Jaynes could not open its proxy for the model: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let tally = Arc::new(Mutex::new(Tally::default()));
    let (h, s, t) = (host.to_string(), stop.clone(), tally.clone());
    let accepting = std::thread::spawn(move || {
        for conn in listener.incoming() {
            if s.load(Ordering::SeqCst) {
                break;
            }
            let Ok(conn) = conn else { continue };
            let (h, r, t) = (h.clone(), resolve.clone(), t.clone());
            std::thread::spawn(move || serve(conn, &h, &r, local, &t));
        }
    });
    Ok(Proxy { port, host: host.to_string(), stop, tally, accepting: Some(accepting) })
}

impl Proxy {
    /// The address the agent is told, in HTTPS_PROXY.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn host(&self) -> String {
        format!("{}:443", self.host)
    }

    /// Stop taking connections, and what was let through and refused.
    pub fn finish(mut self) -> Tally {
        self.close();
        self.tally.lock().map(|t| t.clone()).unwrap_or_default()
    }

    fn close(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // the accepting thread waits in accept: a connection of its own wakes it to see the stop
        let _woke = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], self.port)), Duration::from_secs(1));
        if let Some(t) = self.accepting.take() {
            let _joined = t.join();
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        if self.accepting.is_some() {
            self.close();
        }
    }
}

/// What a client sent as its request's head, up to the empty line.
fn head(conn: &mut TcpStream) -> Result<String, String> {
    let mut got = Vec::new();
    let mut byte = [0u8; 1];
    while !got.ends_with(b"\r\n\r\n") {
        if got.len() >= HEAD {
            return Err("a request longer than a request's head".into());
        }
        match conn.read(&mut byte) {
            Ok(1) => got.push(byte[0]),
            _ => return Err("a request that never ended".into()),
        }
    }
    Ok(String::from_utf8_lossy(&got).to_string())
}

/// Text from the agent, shown safe and short: what it named, never more than a line of it.
fn shown(s: &str) -> String {
    s.chars().take(100).map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { '?' }).collect()
}

/// Whether the request line asks for `host`:443 and nothing else; why not, when it does not.
pub fn asks_for(line: &str, host: &str) -> Result<(), String> {
    let mut parts = line.split(' ');
    let (method, target, version) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return Err(format!("a request Jaynes does not read: {}", shown(line)));
    }
    if method != "CONNECT" {
        return Err(format!("{} is not CONNECT: only a tunnel to {host} passes", shown(method)));
    }
    let Some((name, port)) = target.rsplit_once(':') else { return Err(format!("{} names no port", shown(target))) };
    let name = name.trim_start_matches('[').trim_end_matches(']');
    if name.parse::<IpAddr>().is_ok() {
        return Err(format!("{}: an address by number, never a name", shown(target)));
    }
    if !name.eq_ignore_ascii_case(host) {
        return Err(format!("{}: another name than {host}", shown(target)));
    }
    if port != "443" {
        return Err(format!("{}: another port than 443", shown(target)));
    }
    Ok(())
}

/// Whether an address is on the internet: not this machine's, nor a private, shared, link-local or reserved one.
pub fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let [b0, b1, ..] = a.octets();
            !(a.is_loopback() || a.is_private() || a.is_link_local() || a.is_unspecified() || a.is_multicast() || a.is_broadcast()
                || a.is_documentation() || b0 == 0 || (b0 == 100 && (64..128).contains(&b1)) || b0 >= 240)
        }
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return public(IpAddr::V4(v4));
            }
            let s = a.segments();
            let v4 = |hi: u16, lo: u16| IpAddr::V4(std::net::Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo)));
            // an IPv4 carried inside: NAT64's well-known prefix (64:ff9b::/96) and 6to4 (2002::/16) are judged by it;
            // NAT64's local prefix (64:ff9b:1::/48) and Teredo (2001::/32) hide it, and are refused
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                return public(v4(s[6], s[7]));
            }
            if s[0] == 0x2002 {
                return public(v4(s[1], s[2]));
            }
            if (s[0] == 0x64 && s[1] == 0xff9b && s[2] == 1) || (s[0] == 0x2001 && s[1] == 0) {
                return false;
            }
            !(a.is_loopback() || a.is_unspecified() || a.is_multicast() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80)
        }
    }
}

/// The server name of a TLS ClientHello's first record, or why there is none.
pub fn server_name(record: &[u8]) -> Result<String, String> {
    let none = || "a TLS hello that names no server".to_string();
    let take = |at: usize, n: usize| record.get(at..at + n).ok_or_else(none);
    let u16_at = |at: usize| take(at, 2).map(|b| usize::from(b[0]) << 8 | usize::from(b[1]));
    if record.first() != Some(&0x16) || take(5, 1)?[0] != 0x01 {
        return Err("not TLS after the tunnel opened".into());
    }
    // record header 5, handshake header 4, version 2, random 32
    let mut at = 5 + 4 + 2 + 32;
    at += 1 + usize::from(take(at, 1)?[0]); // session id
    at += 2 + u16_at(at)?; // cipher suites
    at += 1 + usize::from(take(at, 1)?[0]); // compression methods
    let end = at + 2 + u16_at(at)?;
    at += 2;
    while at + 4 <= end {
        let (kind, len) = (u16_at(at)?, u16_at(at + 2)?);
        if kind == 0 {
            // server_name: list length 2, then entries of type 1, length 2, name
            let mut e = at + 4 + 2;
            while e + 3 <= at + 4 + len {
                let (t, n) = (take(e, 1)?[0], u16_at(e + 1)?);
                if t == 0 {
                    return Ok(String::from_utf8_lossy(take(e + 3, n)?).to_string());
                }
                e += 3 + n;
            }
            return Err(none());
        }
        at += 4 + len;
    }
    Err(none())
}

/// One record of TLS, as the client sent it.
fn record(conn: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut header = [0u8; 5];
    conn.read_exact(&mut header).map_err(|_| "no TLS after the tunnel opened".to_string())?;
    let len = usize::from(header[3]) << 8 | usize::from(header[4]);
    if header[0] != 0x16 || len > 16 * 1024 {
        return Err("not TLS after the tunnel opened".into());
    }
    let mut body = vec![0u8; len];
    conn.read_exact(&mut body).map_err(|_| "a TLS hello cut short".to_string())?;
    Ok([header.to_vec(), body].concat())
}

fn serve(mut conn: TcpStream, host: &str, resolve: &Resolve, local: bool, tally: &Mutex<Tally>) {
    let refuse = |why: String| {
        if let Ok(mut t) = tally.lock() {
            t.refuse(why);
        }
    };
    let _set = conn.set_read_timeout(Some(WAIT));
    let asked = head(&mut conn).and_then(|h| {
        let line = h.lines().next().unwrap_or("").to_string();
        asks_for(&line, host)
    });
    if let Err(why) = asked {
        let _said = conn.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        refuse(why);
        return;
    }
    if conn.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").is_err() {
        return;
    }
    // the server name the agent's TLS asks for, before anything reaches the host
    let hello = match record(&mut conn).and_then(|r| server_name(&r).map(|name| (r, name))) {
        Ok((r, name)) if name.eq_ignore_ascii_case(host) => r,
        Ok((_, name)) => return refuse(format!("a TLS name other than {host}: {}", shown(&name))),
        Err(why) => return refuse(why),
    };
    let addrs: Vec<SocketAddr> = resolve(host, 443).into_iter().filter(|a| local || public(a.ip())).collect();
    if addrs.is_empty() {
        return refuse(format!("{host} resolved to no address on the internet"));
    }
    let Some(mut up) = addrs.iter().find_map(|a| TcpStream::connect_timeout(a, WAIT).ok()) else {
        return refuse(format!("{host} could not be reached"));
    };
    if up.write_all(&hello).is_err() {
        return;
    }
    if let Ok(mut t) = tally.lock() {
        t.reached += 1;
    }
    let _set = conn.set_read_timeout(None);
    let (Ok(mut back), Ok(mut down)) = (up.try_clone(), conn.try_clone()) else { return };
    let upward = std::thread::spawn(move || {
        let _copied = std::io::copy(&mut conn, &mut up);
        let _closed = up.shutdown(Shutdown::Write);
    });
    let _copied = std::io::copy(&mut back, &mut down);
    let _closed = down.shutdown(Shutdown::Write);
    let _joined = upward.join();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ClientHello's first record as a client writes it, naming `name` (or no name).
    fn hello(name: Option<&str>) -> Vec<u8> {
        let mut ext = Vec::new();
        if let Some(n) = name {
            let n = n.as_bytes();
            let entry = [&[0u8][..], &(n.len() as u16).to_be_bytes(), n].concat();
            let list = [&(entry.len() as u16).to_be_bytes()[..], &entry].concat();
            ext.extend([&0u16.to_be_bytes()[..], &(list.len() as u16).to_be_bytes(), &list].concat());
        }
        ext.extend([0x00, 0x0b, 0x00, 0x02, 0x01, 0x00]); // ec_point_formats, after it
        let body = [&[0x03, 0x03][..], &[7u8; 32], &[0], &[0x00, 0x02, 0x13, 0x01], &[0x01, 0x00], &(ext.len() as u16).to_be_bytes(), &ext].concat();
        let hs = [&[0x01][..], &(body.len() as u32).to_be_bytes()[1..], &body].concat();
        [&[0x16, 0x03, 0x01][..], &(hs.len() as u16).to_be_bytes(), &hs].concat()
    }

    #[test]
    fn a_request_line_passes_only_as_connect_to_the_host_s_443() {
        let host = "api.openai.com";
        assert!(asks_for("CONNECT api.openai.com:443 HTTP/1.1", host).is_ok());
        assert!(asks_for("CONNECT API.OpenAI.com:443 HTTP/1.1", host).is_ok(), "a name's capitals are the same name");
        for (line, why) in [
            ("GET http://api.openai.com/ HTTP/1.1", "is not CONNECT"),
            ("CONNECT example.com:443 HTTP/1.1", "another name"),
            ("CONNECT api.openai.com.evil.example:443 HTTP/1.1", "another name"),
            ("CONNECT api.openai.com.:443 HTTP/1.1", "another name"),
            ("CONNECT 104.18.7.192:443 HTTP/1.1", "by number"),
            ("CONNECT [2606:4700::6812:7c0]:443 HTTP/1.1", "by number"),
            ("CONNECT api.openai.com:8443 HTTP/1.1", "another port"),
            ("CONNECT api.openai.com HTTP/1.1", "names no port"),
            ("CONNECT api.openai.com:443 HTTP/1.1 extra", "does not read"),
        ] {
            let said = asks_for(line, host).expect_err(line);
            assert!(said.contains(why), "{line}: {said}");
        }
    }

    #[test]
    fn only_an_address_on_the_internet_is_public() {
        for ip in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "::1", "fe80::1", "fd00::1", "::ffff:127.0.0.1", "::ffff:10.0.0.1", "224.0.0.1", "255.255.255.255",
                   "64:ff9b::a00:1", "64:ff9b::7f00:1", "2002:a00:1::1", "2002:7f00:1::", "64:ff9b:1::6812:7c0", "2001:0:4136:e378::1"] {
            assert!(!public(ip.parse().expect("ip")), "{ip}");
        }
        for ip in ["104.18.7.192", "2606:4700::6812:7c0", "::ffff:104.18.7.192", "64:ff9b::6812:7c0", "2002:6812:7c0::1"] {
            assert!(public(ip.parse().expect("ip")), "{ip}");
        }
    }

    #[test]
    fn the_server_name_is_read_from_a_tls_hello() {
        assert_eq!(server_name(&hello(Some("api.openai.com"))), Ok("api.openai.com".to_string()));
        assert!(server_name(&hello(None)).is_err_and(|e| e.contains("names no server")));
        assert!(server_name(b"GET / HTTP/1.1\r\n").is_err_and(|e| e.contains("not TLS")));
        let mut cut = hello(Some("api.openai.com"));
        cut.truncate(60);
        assert!(server_name(&cut).is_err());
    }

    /// The proxy in this process, its host sent to a server of the test's that notes what reaches it.
    fn proxy_to_a_server(local: bool) -> (Proxy, Arc<Mutex<Vec<Vec<u8>>>>) {
        let server = TcpListener::bind("127.0.0.1:0").expect("listen");
        let at = server.local_addr().expect("addr");
        let heard = Arc::new(Mutex::new(Vec::new()));
        let kept = heard.clone();
        std::thread::spawn(move || {
            for mut c in server.incoming().flatten() {
                let _set = c.set_read_timeout(Some(Duration::from_secs(2)));
                let mut got = vec![0u8; 4096];
                let n = c.read(&mut got).unwrap_or(0);
                got.truncate(n);
                let _back = c.write_all(b"from the host");
                if let Ok(mut h) = kept.lock() {
                    h.push(got);
                }
            }
        });
        (start_with("api.openai.com", Arc::new(move |_, _| vec![at]), local).expect("proxy"), heard)
    }

    fn ask(port: u16, request: &str, then: Option<Vec<u8>>) -> String {
        let mut c = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let _set = c.set_read_timeout(Some(Duration::from_secs(5)));
        c.write_all(request.as_bytes()).expect("write");
        let mut first = vec![0u8; 512];
        let n = c.read(&mut first).unwrap_or(0);
        let mut said = String::from_utf8_lossy(&first[..n]).to_string();
        if let Some(bytes) = then {
            let _sent = c.write_all(&bytes);
            let mut rest = Vec::new();
            let _read = c.read_to_end(&mut rest);
            said.push_str(&String::from_utf8_lossy(&rest));
        }
        said
    }

    #[test]
    fn the_host_is_reached_once_its_tls_names_it_and_nothing_else_is() {
        let (proxy, heard) = proxy_to_a_server(true);
        let p = proxy.port;
        let connect = "CONNECT api.openai.com:443 HTTP/1.1\r\nHost: api.openai.com:443\r\n\r\n";
        let said = ask(p, connect, Some(hello(Some("api.openai.com"))));
        assert!(said.starts_with("HTTP/1.1 200") && said.ends_with("from the host"), "{said}");
        // refused before the tunnel: 403
        for req in ["GET http://api.openai.com/ HTTP/1.1\r\n\r\n", "CONNECT example.com:443 HTTP/1.1\r\n\r\n", "CONNECT 127.0.0.1:443 HTTP/1.1\r\n\r\n", "CONNECT api.openai.com:80 HTTP/1.1\r\n\r\n"] {
            assert!(ask(p, req, None).starts_with("HTTP/1.1 403"), "{req}");
        }
        // refused in the tunnel: it closes and the host hears nothing
        for then in [hello(Some("elsewhere.example")), hello(None), b"GET / HTTP/1.1\r\n\r\n".to_vec()] {
            let said = ask(p, connect, Some(then));
            assert!(said.starts_with("HTTP/1.1 200") && !said.contains("from the host"), "{said}");
        }
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(heard.lock().expect("heard").len(), 1, "only the hello naming the host reached it");
        let tally = proxy.finish();
        assert_eq!(tally.reached, 1);
        assert_eq!(tally.refused.len(), 7, "{:?}", tally.refused);
        assert!(tally.refused.iter().any(|r| r.contains("elsewhere.example")), "{:?}", tally.refused);
    }

    #[test]
    fn a_host_that_resolves_to_this_machine_or_a_private_address_is_not_reached() {
        let (proxy, heard) = proxy_to_a_server(false);
        let said = ask(proxy.port, "CONNECT api.openai.com:443 HTTP/1.1\r\n\r\n", Some(hello(Some("api.openai.com"))));
        assert!(!said.contains("from the host"), "{said}");
        std::thread::sleep(Duration::from_millis(200));
        assert!(heard.lock().expect("heard").is_empty());
        let tally = proxy.finish();
        assert!(tally.reached == 0 && tally.refused.iter().any(|r| r.contains("no address on the internet")), "{tally:?}");
    }

    #[test]
    fn a_turn_keeps_fifty_refusals_word_for_word_and_counts_the_rest() {
        let mut t = Tally::default();
        for k in 0..60 {
            t.refuse(format!("r{k}"));
        }
        assert_eq!((t.refused.len(), t.more), (50, 10));
    }

    #[test]
    fn what_the_agent_named_is_shown_safe_and_short() {
        assert_eq!(shown("a\x1b[31mb\nc"), "a?[31mb?c");
        assert_eq!(shown(&"x".repeat(500)).len(), 100);
    }
}
