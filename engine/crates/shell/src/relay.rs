//! How a command in the Linux box reaches the ports of this machine it may reach (`jaynes box --port N`): the box
//! runs in a network of its own, which has only its loopback, and two small programs carry each connection across.
//!
//! - The helper (`jaynes box-relay out`) starts before the box makes its namespaces, so it stays in this machine's
//!   network. The ports it may connect to are given on its command line then, and never come from inside: asked
//!   for one of them, it connects to 127.0.0.1 (then ::1) at that port and hands the connected socket back; asked
//!   for anything else, it hands nothing back and opens nothing.
//! - The forwarder (`jaynes box-relay in`) starts in the box's network, listens on 127.0.0.1 and ::1 at each of those
//!   ports there, and for each connection asks the helper for its socket and copies the bytes both ways.
//!
//! The only channel between them is a pair of sockets the box makes before its namespaces, passed to each as its
//! standard input: nothing of it is a path on disk the box reaches, and the command holds no end of it. Both end
//! with the box's command, which each watches (a pidfd of the process that started it), and they are in its
//! process group, which a halt stops.

use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer, SendAncillaryMessage, SendFlags};
use std::io::{IoSlice, IoSliceMut, Write};
use std::mem::MaybeUninit;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Whether the helper connects for `port`: only one of the ports it was started with.
pub fn may_reach(ports: &[u16], port: u16) -> bool {
    ports.contains(&port)
}

/// The relay's end: it watches the process that started it and ends as soon as that one does. A parent death
/// signal would not reach it: the box's command is in a Landlock domain that signals no process outside it, the
/// relay among them, and the kernel sends that signal as the dying parent. It refuses to run once that one is gone.
fn tie_to(parent: u32) -> Result<(), String> {
    use rustix::event::{poll, PollFd, PollFlags};
    let pid = i32::try_from(parent).ok().and_then(rustix::process::Pid::from_raw).ok_or("the relay needs the box that starts it")?;
    let watched = rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()).map_err(|e| format!("the relay could not watch the box: {e}"))?;
    if rustix::process::getppid() != Some(pid) {
        return Err("the box that started the relay is gone".into());
    }
    // a process's pidfd is readable once it ends
    std::thread::spawn(move || {
        let mut fds = [PollFd::new(&watched, PollFlags::IN)];
        while poll(&mut fds, None).is_err() {}
        std::process::exit(0);
    });
    Ok(())
}

/// The helper, in this machine's network: answers each request on its standard input with a socket connected to
/// one of `ports` on this machine's loopback, or with nothing.
pub fn helper(ports: &[u16], parent: u32) -> Result<(), String> {
    tie_to(parent)?;
    let stdin = std::io::stdin();
    let channel = stdin.as_fd();
    loop {
        let mut asked = [0u8; 2];
        let (n, _) = rustix::net::recv(channel, &mut asked[..], RecvFlags::empty()).map_err(|e| format!("the relay's channel failed: {e}"))?;
        if n == 0 {
            return Ok(());
        }
        let port = u16::from_be_bytes(asked);
        let stream = if n == asked.len() && may_reach(ports, port) { connect_here(port) } else { None };
        let fd: Option<OwnedFd> = stream.map(OwnedFd::from);
        send(channel, fd.as_ref().map(AsFd::as_fd))?;
    }
}

/// A connection to `port` on this machine's own loopback: 127.0.0.1, then ::1.
fn connect_here(port: u16) -> Option<TcpStream> {
    let wait = Duration::from_secs(5);
    TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), wait)
        .or_else(|_| TcpStream::connect_timeout(&SocketAddr::from((Ipv6Addr::LOCALHOST, port)), wait))
        .ok()
}

/// One message: a byte, 1 with the socket `fd` or 0 with none.
fn send(channel: BorrowedFd<'_>, fd: Option<BorrowedFd<'_>>) -> Result<(), String> {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut extra = SendAncillaryBuffer::new(&mut space);
    let fds: Vec<BorrowedFd<'_>> = fd.into_iter().collect();
    if !fds.is_empty() && !extra.push(SendAncillaryMessage::ScmRights(&fds)) {
        return Err("the relay could not hand a socket on".into());
    }
    let byte = [u8::from(!fds.is_empty())];
    rustix::net::sendmsg(channel, &[IoSlice::new(&byte)], &mut extra, SendFlags::empty()).map_err(|e| format!("the relay's channel failed: {e}"))?;
    Ok(())
}

/// Ask the helper on `channel` for a socket connected to `port` on this machine; None when it gave none.
pub fn ask(channel: BorrowedFd<'_>, port: u16) -> Result<Option<OwnedFd>, String> {
    let failed = |e: rustix::io::Errno| format!("the relay's channel failed: {e}");
    rustix::net::send(channel, &port.to_be_bytes(), SendFlags::empty()).map_err(failed)?;
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut extra = RecvAncillaryBuffer::new(&mut space);
    let mut byte = [0u8; 1];
    rustix::net::recvmsg(channel, &mut [IoSliceMut::new(&mut byte)], &mut extra, RecvFlags::CMSG_CLOEXEC).map_err(failed)?;
    let mut got = None;
    for message in extra.drain() {
        if let RecvAncillaryMessage::ScmRights(mut fds) = message {
            got = got.or_else(|| fds.next());
        }
    }
    Ok(got)
}

/// The forwarder, in the box's network: listens at each of `ports` on its loopback, says "ready" on standard output
/// once it does, and carries each connection to the helper's socket for that port.
pub fn forwarder(ports: &[u16], parent: u32) -> Result<(), String> {
    tie_to(parent)?;
    let channel: OwnedFd = rustix::io::dup(std::io::stdin().as_fd()).map_err(|e| format!("the relay's channel failed: {e}"))?;
    let channel = Arc::new(Mutex::new(channel));
    let mut listening = Vec::new();
    for &port in ports {
        let four = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|e| format!("the box could not listen at port {port} of its own network: {e}"))?;
        listening.push((port, four));
        // its ::1 as well, where the box's loopback has one
        if let Ok(six) = TcpListener::bind((Ipv6Addr::LOCALHOST, port)) {
            listening.push((port, six));
        }
    }
    let mut out = std::io::stdout();
    writeln!(out, "ready").and_then(|()| out.flush()).map_err(|e| format!("the relay could not say it is ready: {e}"))?;
    let threads: Vec<_> = listening
        .into_iter()
        .map(|(port, listener)| {
            let channel = Arc::clone(&channel);
            std::thread::spawn(move || {
                for inside in listener.incoming().flatten() {
                    let channel = Arc::clone(&channel);
                    std::thread::spawn(move || carry(inside, port, &channel));
                }
            })
        })
        .collect();
    for t in threads {
        let _joined = t.join();
    }
    Ok(())
}

/// Carry one connection from inside the box to the helper's socket for `port`, both ways, until either ends.
fn carry(inside: TcpStream, port: u16, channel: &Mutex<OwnedFd>) {
    let got = match channel.lock() {
        Ok(c) => ask(c.as_fd(), port),
        Err(_) => return,
    };
    let Ok(Some(fd)) = got else { return };
    let outside = TcpStream::from(fd);
    let (Ok(mut a_in), Ok(mut b_out)) = (inside.try_clone(), outside.try_clone()) else { return };
    let (mut a_out, mut b_in) = (outside, inside);
    let up = std::thread::spawn(move || {
        let _copied = std::io::copy(&mut a_in, &mut a_out);
        let _done = a_out.shutdown(std::net::Shutdown::Write);
    });
    let _copied = std::io::copy(&mut b_out, &mut b_in);
    let _done = b_in.shutdown(std::net::Shutdown::Write);
    let _joined = up.join();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helper_reaches_only_the_ports_it_was_started_with() {
        assert!(may_reach(&[4000, 5000], 4000));
        assert!(!may_reach(&[4000, 5000], 8750), "another port, the page's among them");
        assert!(!may_reach(&[], 4000));
    }
}
