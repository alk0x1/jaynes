//! The record's chain: each entry carries the fingerprint of the one before it and of its own body, so changing,
//! dropping or reordering a past entry shows.

use crate::digest::Digest;

/// The link before the first entry.
pub fn genesis() -> Digest {
    Digest("0".repeat(64))
}

/// The fingerprint of an entry, from the one before it and its body exactly as stored.
pub fn link(previous: &Digest, body: &str) -> Digest {
    let mut bytes = Vec::with_capacity(previous.0.len() + 1 + body.len());
    bytes.extend_from_slice(previous.0.as_bytes());
    bytes.push(b'\n');
    bytes.extend_from_slice(body.as_bytes());
    Digest::of_bytes(&bytes)
}

/// The first entry, counting from 0, whose stored fingerprint does not follow from those before it.
pub fn first_break<'a>(entries: impl IntoIterator<Item = (&'a str, &'a Digest)>) -> Option<usize> {
    let mut previous = genesis();
    for (n, (body, stored)) in entries.into_iter().enumerate() {
        let expected = link(&previous, body);
        if &expected != stored {
            return Some(n);
        }
        previous = expected;
    }
    None
}
