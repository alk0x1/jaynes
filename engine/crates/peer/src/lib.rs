//! Who is at the other end of a Unix socket, as the system says: on macOS, the audit token of that process, which
//! names it alone (its pid and the pid's generation), so it cannot be a later process that took its number. This is
//! the engine's one call outside safe Rust, kept here so the rest of the engine forbids unsafe code.

use std::os::unix::net::UnixStream;

/// The audit token of the process at the other end of `conn`, or None where the system does not give one.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub fn audit_token(conn: &UnixStream) -> Option<[u8; 32]> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn getsockopt(fd: i32, level: i32, name: i32, value: *mut std::ffi::c_void, len: *mut u32) -> i32;
    }
    /// SOL_LOCAL and LOCAL_PEERTOKEN, from <sys/un.h>.
    const SOL_LOCAL: i32 = 0;
    const LOCAL_PEERTOKEN: i32 = 0x006;
    let mut token = [0u8; 32];
    let mut len: u32 = 32;
    // SAFETY: the buffer is 32 bytes, its length says so, and the descriptor is the stream's own, open while `conn` is
    let r = unsafe { getsockopt(conn.as_raw_fd(), SOL_LOCAL, LOCAL_PEERTOKEN, token.as_mut_ptr().cast(), &mut len) };
    (r == 0 && len == 32).then_some(token)
}

#[cfg(not(target_os = "macos"))]
pub fn audit_token(_conn: &UnixStream) -> Option<[u8; 32]> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn the_other_end_of_a_socket_has_a_token() {
        let (a, b) = std::os::unix::net::UnixStream::pair().expect("pair");
        let (ta, tb) = (super::audit_token(&a), super::audit_token(&b));
        assert!(ta.is_some() && ta == tb, "both ends are this process");
    }
}
