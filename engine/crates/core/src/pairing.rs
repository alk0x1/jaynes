//! Which device keys of the person's this engine knows, as its record holds them: the first pairing takes the keys it
//! is given; replacing them, or unpairing, takes a proof by a key paired now, over what changes. The keys are read only
//! from the record's chain, each change checked again whenever it is read, so a change written into the record without
//! a proof by a key paired then changes nothing, and no other file of the engine's home counts.
//!
//! What this does not hold against: the first pairing takes no proof, so whatever pairs first is paired; and a program
//! that rewrites or replaces the whole record (it is a file of the engine's home) can write any history it likes.

use crate::proof::{self, KeyKind, PersonKey, Proof};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The record's entries about pairing, under this run name, apart from every run.
pub const RECORD: &str = "~pairing";

/// One change of the keys, as the record holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "pairing", rename_all = "snake_case")]
pub enum Change {
    /// The keys from now on: the first ones, or new ones (a finger changed, a new Mac) with a proof by a key paired now.
    Paired {
        keys: Vec<PersonKey>,
        nonce: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        proof: Option<Proof>,
        #[serde(default, flatten)]
        from: Origin,
    },
    /// No key from now on, with a proof by a key paired now.
    Unpaired {
        nonce: String,
        proof: Proof,
        #[serde(default, flatten)]
        from: Origin,
    },
}

/// Where a change came from, as the engine that took it saw it: its channel (the app) and the build that sent it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Origin {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub channel: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub caller: String,
}

impl Change {
    /// Marks where the change came from, over whatever it said of itself.
    pub fn came_from(&mut self, channel: &str, caller: &str) {
        let (Change::Paired { from, .. } | Change::Unpaired { from, .. }) = self;
        *from = Origin { channel: channel.to_string(), caller: caller.to_string() };
    }
}

/// The keys paired, as the changes the record holds leave them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keys {
    pub keys: Vec<PersonKey>,
    nonces: BTreeSet<String>,
}

/// What a proof of a change of the keys covers: the keys paired now, a nonce used once, and the keys after it.
pub fn message(now: &[PersonKey], nonce: &str, after: Option<&[PersonKey]>) -> Vec<u8> {
    crate::digest::canonical(&serde_json::json!({"jaynes": "pairing/1", "now": now, "nonce": nonce, "after": after})).into_bytes()
}

impl Keys {
    /// The keys the changes leave, each checked again as `take` checks it: a change that does not hold changes nothing.
    pub fn fold<'a>(changes: impl IntoIterator<Item = &'a Change>) -> Keys {
        let mut k = Keys::default();
        for c in changes {
            // a change that does not hold is left out, as if it were not there
            if k.take(c).is_err() {
                continue;
            }
        }
        k
    }

    /// Takes a change if it holds.
    pub fn take(&mut self, c: &Change) -> Result<(), String> {
        let (nonce, after, given) = match c {
            Change::Paired { keys, nonce, proof, .. } => (nonce, Some(keys.as_slice()), proof.as_ref()),
            Change::Unpaired { nonce, proof, .. } => (nonce, None, Some(proof)),
        };
        if nonce.is_empty() || nonce.len() > 200 {
            return Err("a nonce is between 1 and 200 characters".into());
        }
        if self.nonces.contains(nonce) {
            return Err("this proof was used already".into());
        }
        if let Some(keys) = after {
            check(keys)?;
        }
        if !self.keys.is_empty() {
            let proof = given.ok_or("a key of yours is paired: replacing it or unpairing it takes a proof by it")?;
            proof::holds(&self.keys, &message(&self.keys, nonce, after), proof).map_err(|e| format!("the proof does not hold: {e}"))?;
        } else if after.is_none() {
            return Err("no key is paired".into());
        }
        self.nonces.insert(nonce.clone());
        self.keys = after.map(<[PersonKey]>::to_vec).unwrap_or_default();
        Ok(())
    }
}

/// Keys that can be paired: one or two, each of its own kind, the device key among them, each a P-256 public key.
pub fn check(keys: &[PersonKey]) -> Result<(), String> {
    if keys.is_empty() {
        return Err("no key given".into());
    }
    let kinds: BTreeSet<KeyKind> = keys.iter().map(|k| k.kind).collect();
    if kinds.len() != keys.len() {
        return Err("two keys of the same kind".into());
    }
    if !kinds.contains(&KeyKind::Presence) {
        return Err("the device key (presence) is missing: every device has one".into());
    }
    let mut seen = Vec::new();
    for k in keys {
        let p = proof::public(k)?;
        // the same key in two forms (compressed or not) is one key: it cannot be both kinds
        if seen.contains(&p) {
            return Err("the same key is paired twice".into());
        }
        seen.push(p);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Signer;
    use p256::ecdsa::{Signature, SigningKey};

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32].into()).expect("key")
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn pk(kind: KeyKind, seed: u8) -> PersonKey {
        PersonKey { kind, public: hex(key(seed).verifying_key().to_encoded_point(false).as_bytes()) }
    }

    fn sign(seed: u8, kind: KeyKind, m: &[u8]) -> Proof {
        let s: Signature = key(seed).sign(m);
        Proof::Device { key: kind, signature: hex(s.to_der().as_bytes()) }
    }

    #[test]
    fn the_first_pairing_takes_its_keys_and_only_a_proof_by_them_changes_them() {
        let first = vec![pk(KeyKind::Biometry, 1), pk(KeyKind::Presence, 2)];
        let mut k = Keys::default();
        k.take(&Change::Paired { keys: first.clone(), nonce: "a".into(), proof: None, from: Default::default() }).expect("first");
        assert_eq!(k.keys, first);
        // another program pairs its own key: refused without a proof by the keys paired now
        let theirs = vec![pk(KeyKind::Presence, 9)];
        assert!(k.take(&Change::Paired { keys: theirs.clone(), nonce: "b".into(), proof: None, from: Default::default() }).is_err());
        assert!(k.take(&Change::Paired { keys: theirs.clone(), nonce: "b".into(), from: Default::default(), proof: Some(sign(9, KeyKind::Presence, &message(&first, "b", Some(&theirs)))) }).is_err());
        assert!(k.take(&Change::Unpaired { from: Default::default(), nonce: "c".into(), proof: sign(9, KeyKind::Presence, &message(&first, "c", None)) }).is_err());
        // a proof over another change
        let again = vec![pk(KeyKind::Biometry, 3), pk(KeyKind::Presence, 2)];
        assert!(k.take(&Change::Paired { keys: theirs.clone(), nonce: "d".into(), from: Default::default(), proof: Some(sign(2, KeyKind::Presence, &message(&first, "d", Some(&again)))) }).is_err());
        assert_eq!(k.keys, first, "nothing changed");
        // a finger changed: the device key proves the new fingerprint key
        k.take(&Change::Paired { keys: again.clone(), nonce: "e".into(), from: Default::default(), proof: Some(sign(2, KeyKind::Presence, &message(&first, "e", Some(&again)))) }).expect("replaced");
        assert_eq!(k.keys, again);
        // unpairing, by a key paired now; the same proof again changes nothing
        let off = Change::Unpaired { from: Default::default(), nonce: "f".into(), proof: sign(3, KeyKind::Biometry, &message(&again, "f", None)) };
        k.take(&off).expect("unpaired");
        assert!(k.keys.is_empty());
        assert!(k.take(&off).is_err());
    }

    #[test]
    fn folding_the_record_checks_every_change_again() {
        let first = vec![pk(KeyKind::Presence, 2)];
        let forged = Change::Unpaired { from: Default::default(), nonce: "x".into(), proof: sign(9, KeyKind::Presence, &message(&first, "x", None)) };
        let k = Keys::fold(&[Change::Paired { keys: first.clone(), nonce: "a".into(), proof: None, from: Default::default() }, forged]);
        assert_eq!(k.keys, first, "a change written in without a proof that holds lowers nothing");
    }

    #[test]
    fn a_nonce_is_never_empty() {
        let mut k = Keys::default();
        assert!(k.take(&Change::Paired { keys: vec![pk(KeyKind::Presence, 2)], nonce: String::new(), proof: None, from: Default::default() }).is_err());
        assert!(k.keys.is_empty());
    }

    #[test]
    fn keys_that_cannot_be_paired() {
        assert!(check(&[]).is_err());
        assert!(check(&[pk(KeyKind::Biometry, 1)]).is_err(), "no device key");
        assert!(check(&[pk(KeyKind::Presence, 1), pk(KeyKind::Presence, 2)]).is_err());
        assert!(check(&[PersonKey { kind: KeyKind::Presence, public: "04ab".into() }]).is_err());
        assert!(check(&[pk(KeyKind::Presence, 1)]).is_ok());
        let compressed = PersonKey { kind: KeyKind::Biometry, public: hex(key(1).verifying_key().to_encoded_point(true).as_bytes()) };
        assert!(check(&[pk(KeyKind::Presence, 1), compressed]).is_err(), "one key paired as both kinds");
        assert!(check(&[pk(KeyKind::Presence, 1), pk(KeyKind::Biometry, 1)]).is_err());
    }
}
