//! A proof only the person can give, over one input of theirs, checked by the core itself.
//!
//! The person's device holds two keys made by its secure chip in one gesture: one that signs only with their
//! fingerprint (biometry) and one that signs with their finger or their password (presence); a device with no
//! fingerprint reader holds only the second. Their public halves are recorded when a run starts. An input that comes
//! with a proof counts as the person's own, present at the device, only when the proof holds for one of those keys
//! over `input_message`: the run (by the fingerprint of how it started, so it is this run and no other with the same
//! id), a nonce used once, and the input as the core reads it. The gesture proves the person was present; it does not
//! prove what they read, which is the device's to show.

use crate::digest::Digest;
use crate::io::Input;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Which of the person's keys signed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum KeyKind {
    /// Signs only with the person's fingerprint.
    Biometry,
    /// Signs with the person's finger or their password, at the device.
    Presence,
}

/// One of the person's keys, as it was paired: a P-256 public key, in SEC1 form, in hex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PersonKey {
    pub kind: KeyKind,
    pub public: String,
}

/// What an answer must come with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Requires {
    /// The person's word, from any of their channels: the page, the terminal, the app.
    Word,
    /// A proof of either of the person's keys.
    Presence,
    /// A proof of the key that signs only with their fingerprint.
    Biometry,
}

impl Requires {
    /// Whether a proof by a key of `kind` meets this.
    pub fn met_by(self, kind: Option<KeyKind>) -> bool {
        match self {
            Requires::Word => true,
            Requires::Presence => kind.is_some(),
            Requires::Biometry => kind == Some(KeyKind::Biometry),
        }
    }
}

/// A proof over one input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Proof {
    /// An ECDSA P-256 signature, in DER, in hex, over the SHA-256 of `input_message`, made by the person's key of `key`.
    Device { key: KeyKind, signature: String },
}

/// The bytes a proof covers: this run, a nonce used once, and the input as the core reads it.
pub fn input_message(run: &str, started: &Digest, nonce: &str, input: &Input) -> Vec<u8> {
    crate::digest::canonical(&serde_json::json!({
        "jaynes": "input/1",
        "run": run,
        "started": started,
        "nonce": nonce,
        "input": input,
    }))
    .into_bytes()
}

/// Whether `proof` holds over `message` for one of `keys` of the kind it names: the kind of key that signed.
pub fn holds(keys: &[PersonKey], message: &[u8], proof: &Proof) -> Result<KeyKind, String> {
    let Proof::Device { key, signature } = proof;
    let paired = keys.iter().find(|k| k.kind == *key).ok_or_else(|| format!("no {} key of yours is paired with this run", word(*key)))?;
    let public = VerifyingKey::from_sec1_bytes(&strict_hex(&paired.public).ok_or("the paired key is not hex")?)
        .map_err(|_| "the paired key is not a P-256 key".to_string())?;
    let der = strict_hex(signature).ok_or("the signature is not hex")?;
    let signature = Signature::from_der(&der).map_err(|_| "the signature is not a DER ECDSA signature".to_string())?;
    // a signature and its mirror (r, n - s) are both valid; the device may give either
    let signature = signature.normalize_s().unwrap_or(signature);
    public.verify(message, &signature).map_err(|_| "the signature does not hold for this input".to_string())?;
    Ok(*key)
}

/// The key's kind in words.
pub fn word(k: KeyKind) -> &'static str {
    match k {
        KeyKind::Biometry => "fingerprint",
        KeyKind::Presence => "device",
    }
}

/// Lowercase or uppercase hex digits and nothing else, in pairs.
pub fn strict_hex(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Signer;
    use p256::ecdsa::SigningKey;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32].into()).expect("key")
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn paired(kind: KeyKind, k: &SigningKey) -> PersonKey {
        PersonKey { kind, public: hex(k.verifying_key().to_encoded_point(false).as_bytes()) }
    }

    fn sign(k: &SigningKey, kind: KeyKind, message: &[u8]) -> Proof {
        let s: Signature = k.sign(message);
        Proof::Device { key: kind, signature: hex(s.to_der().as_bytes()) }
    }

    #[test]
    fn a_proof_holds_only_for_its_key_and_its_message() {
        let (bio, any) = (key(3), key(4));
        let keys = vec![paired(KeyKind::Biometry, &bio), paired(KeyKind::Presence, &any)];
        let m = input_message("r", &Digest("s".into()), "n1", &Input::Tick);
        assert_eq!(holds(&keys, &m, &sign(&bio, KeyKind::Biometry, &m)), Ok(KeyKind::Biometry));
        assert_eq!(holds(&keys, &m, &sign(&any, KeyKind::Presence, &m)), Ok(KeyKind::Presence));
        // signed by one key and said to be the other
        assert!(holds(&keys, &m, &sign(&any, KeyKind::Biometry, &m)).is_err());
        // another run with the same id, another nonce: another message
        let other = input_message("r", &Digest("t".into()), "n1", &Input::Tick);
        assert!(holds(&keys, &other, &sign(&any, KeyKind::Presence, &m)).is_err());
        let again = input_message("r", &Digest("s".into()), "n2", &Input::Tick);
        assert!(holds(&keys, &again, &sign(&any, KeyKind::Presence, &m)).is_err());
        // only the presence key paired: a biometry proof cannot hold
        assert!(holds(&keys[1..], &m, &sign(&bio, KeyKind::Biometry, &m)).is_err());
    }

    #[test]
    fn hex_is_read_strictly() {
        assert_eq!(strict_hex("0aFf"), Some(vec![0x0a, 0xff]));
        for bad in ["", "0", "+a", "0x", "zz", "0a "] {
            assert_eq!(strict_hex(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn what_each_requirement_takes() {
        assert!(Requires::Word.met_by(None));
        assert!(!Requires::Presence.met_by(None) && Requires::Presence.met_by(Some(KeyKind::Presence)) && Requires::Presence.met_by(Some(KeyKind::Biometry)));
        assert!(Requires::Biometry.met_by(Some(KeyKind::Biometry)) && !Requires::Biometry.met_by(Some(KeyKind::Presence)));
    }
}
