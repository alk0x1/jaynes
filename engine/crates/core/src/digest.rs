//! Fingerprints: the SHA-256 of a value's canonical JSON, written in hex.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::fmt;

/// The fingerprint of some content. Two contents have the same digest only if they are equal.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct Digest(pub String);

impl Digest {
    pub fn of_bytes(bytes: &[u8]) -> Self {
        let sum = Sha256::digest(bytes);
        let mut hex = String::with_capacity(64);
        for b in sum {
            hex.push_str(&format!("{b:02x}"));
        }
        Digest(hex)
    }

    /// The digest of a value's JSON with the keys of every object sorted, so equal values give equal digests
    /// whatever order their fields were built in.
    pub fn of_json<T: Serialize + ?Sized>(value: &T) -> Self {
        let canonical = serde_json::to_value(value).map(|v| v.to_string()).unwrap_or_default();
        Digest::of_bytes(canonical.as_bytes())
    }

    pub fn short(&self) -> &str {
        self.0.get(..12).unwrap_or(&self.0)
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Digest {
    fn from(s: &str) -> Self {
        Digest(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_order_does_not_change_the_digest() {
        let a: serde_json::Value = serde_json::from_str(r#"{"a":1,"b":[1,2]}"#).expect("json");
        let b: serde_json::Value = serde_json::from_str(r#"{"b":[1,2],"a":1}"#).expect("json");
        assert_eq!(Digest::of_json(&a), Digest::of_json(&b));
        assert_ne!(Digest::of_json(&a), Digest::of_json(&serde_json::json!({"a": 2, "b": [1, 2]})));
    }

    #[test]
    fn sha256_of_empty() {
        assert_eq!(Digest::of_bytes(b"").0, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
}
