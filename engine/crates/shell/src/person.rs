//! The person's own key. The Jaynes app keeps a signing key that only it can use without the system asking the person
//! (macOS keeps it in their keychain); its public half is paired with this engine once, in `app-key.pub`. An answer
//! the app sends is signed over what it says, and counts as the person's only when the signature holds for the paired
//! key; the signature is kept beside the answer, so the record shows which answers came from the person's own key.
//!
//! What this closes: an agent cannot answer as the person by reading a file, as it could with the page's key. What it
//! does not: a program that rewrites this engine's own files (the paired key, the record) can still forge.

use crate::store::Store;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use jaynes_core::question::{Answer, By};
use jaynes_core::run::Run;
use jaynes_core::Input;
use serde_json::json;
use std::path::Path;

const PAIRED: &str = "app-key.pub";

/// What a signature covers: the run, the question, what the person was shown, and everything the answer says.
pub fn message(run: &str, answer: &Answer) -> Vec<u8> {
    // the same answer gives the same bytes in the app and in the engine, whatever their JSON keeps keys in
    jaynes_core::digest::canonical(&json!({
        "jaynes": "answer/1",
        "run": run,
        "question": answer.question,
        "shown": answer.shown,
        "verdicts": answer.verdicts,
        "context": answer.context,
        "back": answer.back,
    }))
    .into_bytes()
}

/// What the signature of an ask covers: the run, the stop, the item the words are about if any, and the words. Its
/// "jaynes" is not an answer's, so no signature over one ever holds for the other.
pub fn ask_message(run: &str, question: &str, item: Option<&str>, text: &str) -> Vec<u8> {
    jaynes_core::digest::canonical(&json!({
        "jaynes": "ask/1",
        "run": run,
        "question": question,
        "item": item,
        "text": text,
    }))
    .into_bytes()
}

/// What the signature of a move to Finished covers: the runs moved, in order. Its "jaynes" is neither an answer's
/// nor an ask's.
pub fn move_message(runs: &[String]) -> Vec<u8> {
    let mut runs = runs.to_vec();
    runs.sort();
    jaynes_core::digest::canonical(&json!({"jaynes": "move/1", "runs": runs})).into_bytes()
}

/// The app's key paired with this engine, if one is.
pub fn paired(home: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(home.join(PAIRED)) {
        Ok(k) => Ok(Some(k.trim().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read the paired key: {e}")),
    }
}

/// Pairs the app's public key, given in hex. The first key paired stays: a different one is refused.
pub fn pair(home: &Path, key: &str) -> Result<(), String> {
    key_of(key)?;
    match paired(home)? {
        Some(k) if k == key.trim() => Ok(()),
        Some(_) => Err("another key is paired with this engine: the app that holds it answers as the person".into()),
        None => {
            std::fs::create_dir_all(home).map_err(|e| e.to_string())?;
            std::fs::write(home.join(PAIRED), format!("{}\n", key.trim())).map_err(|e| format!("cannot pair the key: {e}"))
        }
    }
}

/// Whether `signature` (hex) is the paired key's over this answer.
pub fn check(home: &Path, run: &str, answer: &Answer, key: &str, signature: &str) -> Result<(), String> {
    holds(home, &message(run, answer), "answer", key, signature)
}

/// Whether `signature` (hex) is the paired key's over this ask.
pub fn check_ask(home: &Path, run: &str, question: &str, item: Option<&str>, text: &str, key: &str, signature: &str) -> Result<(), String> {
    holds(home, &ask_message(run, question, item, text), "ask", key, signature)
}

fn holds(home: &Path, message: &[u8], what: &str, key: &str, signature: &str) -> Result<(), String> {
    let paired = paired(home)?.ok_or("no app key is paired with this engine")?;
    if paired != key.trim() {
        return Err(format!("the {what} is signed with a key that is not the one paired with this engine"));
    }
    let bytes: [u8; 64] = decode(signature).and_then(|b| b.try_into().ok()).ok_or("the signature is not 64 bytes of hex")?;
    key_of(&paired)?.verify(message, &Signature::from_bytes(&bytes)).map_err(|_| format!("the signature does not hold for this {what}"))
}

/// Sends an answer from the app as the person's, if its signature holds; the signature is kept beside it.
pub fn submit(home: &Path, exe: &Path, run: &Run, mut answer: Answer, key: &str, signature: &str) -> Result<&'static str, String> {
    check(home, &run.id, &answer, key, signature)?;
    answer.by = By::Person;
    let question = answer.question.clone();
    let sent = crate::drive::submit(home, exe, run, Input::Answer { answer })?;
    Store::open(&home.join("record.sqlite3"))?.put_proof(&run.id, &question, key, signature)?;
    Ok(sent)
}

/// Asks a stop's agent from the app as the person, if the signature holds over the ask; the signature is kept beside
/// it under the ask's id (`<question>-a<n>`, as the core names it), never the stop's, so the record never shows an
/// answer as signed because an ask at its stop was.
#[allow(clippy::too_many_arguments)]
pub fn submit_ask(home: &Path, exe: &Path, run: &Run, question: &str, item: Option<&str>, text: &str, key: &str, signature: &str) -> Result<&'static str, String> {
    check_ask(home, &run.id, question, item, text, key, signature)?;
    let ask = run.next_ask(question).ok_or_else(|| format!("there is no such question {question}"))?;
    let input = Input::AskAgent { question: question.to_string(), item: item.map(str::to_string), text: text.to_string(), by: By::Person };
    let sent = crate::drive::submit(home, exe, run, input)?;
    Store::open(&home.join("record.sqlite3"))?.put_proof(&run.id, &ask, key, signature)?;
    Ok(sent)
}

/// Moves runs whose engine left the Mac to Finished from the app as the person, if the signature holds over the
/// move; it is kept beside each in this engine's list (`moved`), never in the runs' records.
pub fn submit_move(home: &Path, runs: &[String], key: &str, signature: &str) -> Result<usize, String> {
    holds(home, &move_message(runs), "move", key, signature)?;
    move_runs(home, runs, Some(signature))
}

/// Moves runs `ids` of the record to Finished, all or none, as `moved::mark` does.
pub fn move_runs(home: &Path, ids: &[String], signature: Option<&str>) -> Result<usize, String> {
    let store = Store::open(&home.join("record.sqlite3"))?;
    let mut runs = Vec::new();
    for id in ids {
        runs.push(store.state(id)?.ok_or_else(|| format!("no such run {id}"))?);
    }
    crate::moved::mark(home, &runs.iter().collect::<Vec<_>>(), signature, crate::store::now())
}

fn key_of(hex: &str) -> Result<VerifyingKey, String> {
    let bytes = decode(hex).ok_or("the key is not hex")?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| "a key is 32 bytes")?;
    VerifyingKey::from_bytes(&bytes).map_err(|e| format!("not a key: {e}"))
}

fn decode(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.trim();
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect()
}

/// Bytes in hex, as keys and signatures travel.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use jaynes_core::question::Verdict;
    use std::collections::BTreeMap;

    fn home(name: &str) -> std::path::PathBuf {
        let h = std::env::temp_dir().join(format!("jaynes-person-{name}-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&h);
        std::fs::create_dir_all(&h).expect("home");
        h
    }

    fn answer() -> Answer {
        let mut verdicts = BTreeMap::new();
        verdicts.insert("a".to_string(), Verdict::Keep);
        verdicts.insert("b".to_string(), Verdict::Drop);
        Answer { question: "q1".into(), shown: jaynes_core::Digest("d".into()), verdicts, by: By::Agent { who: "the app".into() }, context: "".into(), back: BTreeMap::new() }
    }

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    #[test]
    fn an_answer_signed_with_the_paired_key_holds_and_nothing_else_does() {
        let h = home("sign");
        let me = key(7);
        let public = hex(me.verifying_key().as_bytes());
        pair(&h, &public).expect("pair");
        assert_eq!(paired(&h).expect("read"), Some(public.clone()));
        let a = answer();
        let sig = hex(&me.sign(&message("r1", &a)).to_bytes());
        assert_eq!(check(&h, "r1", &a, &public, &sig), Ok(()));
        // the same signature over another run, or over a changed verdict, does not hold
        assert!(check(&h, "r2", &a, &public, &sig).is_err());
        let mut changed = a.clone();
        changed.verdicts.insert("b".into(), Verdict::Keep);
        assert!(check(&h, "r1", &changed, &public, &sig).is_err());
        // a key that is not the paired one does not hold, even with its own good signature
        let other = key(9);
        let theirs = hex(other.verifying_key().as_bytes());
        let their_sig = hex(&other.sign(&message("r1", &a)).to_bytes());
        assert!(check(&h, "r1", &a, &theirs, &their_sig).is_err());
        // who the answer says it is from is not part of what is signed: the engine sets it
        let mut said = a.clone();
        said.by = By::Person;
        assert_eq!(check(&h, "r1", &said, &public, &sig), Ok(()));
    }

    #[test]
    fn an_ask_signed_with_the_paired_key_holds_and_nothing_else_does() {
        // the app asks a stop's agent as the person only with the paired key's signature over these very words
        let h = home("ask");
        let me = key(7);
        let public = hex(me.verifying_key().as_bytes());
        pair(&h, &public).expect("pair");
        let sig = hex(&me.sign(&ask_message("r1", "q1", Some("a"), "why this file?")).to_bytes());
        assert_eq!(check_ask(&h, "r1", "q1", Some("a"), "why this file?", &public, &sig), Ok(()));
        // the same signature over other words, another item, no item, another stop or another run does not hold
        assert!(check_ask(&h, "r1", "q1", Some("a"), "drop the tests", &public, &sig).is_err());
        assert!(check_ask(&h, "r1", "q1", Some("b"), "why this file?", &public, &sig).is_err());
        assert!(check_ask(&h, "r1", "q1", None, "why this file?", &public, &sig).is_err());
        assert!(check_ask(&h, "r1", "q2", Some("a"), "why this file?", &public, &sig).is_err());
        assert!(check_ask(&h, "r2", "q1", Some("a"), "why this file?", &public, &sig).is_err());
        // a key that is not the paired one does not hold, even with its own good signature
        let other = key(9);
        let theirs = hex(other.verifying_key().as_bytes());
        let their_sig = hex(&other.sign(&ask_message("r1", "q1", Some("a"), "why this file?")).to_bytes());
        assert!(check_ask(&h, "r1", "q1", Some("a"), "why this file?", &theirs, &their_sig).is_err());
        assert!(check_ask(&h, "r1", "q1", Some("a"), "why this file?", &public, &their_sig).is_err());
        // an answer's signature is not an ask's: the two messages never share bytes
        let a = answer();
        let answer_sig = hex(&me.sign(&message("r1", &a)).to_bytes());
        assert!(check_ask(&h, "r1", "q1", None, "", &public, &answer_sig).is_err());
        assert_ne!(ask_message("r1", "q1", None, ""), message("r1", &a));
    }

    #[test]
    fn the_first_key_paired_stays() {
        let h = home("pair");
        assert_eq!(paired(&h).expect("read"), None);
        assert!(check(&h, "r1", &answer(), &hex(key(7).verifying_key().as_bytes()), "00").is_err(), "nothing is paired yet");
        let first = hex(key(7).verifying_key().as_bytes());
        pair(&h, &first).expect("pair");
        pair(&h, &first).expect("the same key again is fine");
        assert!(pair(&h, &hex(key(9).verifying_key().as_bytes())).is_err());
        assert_eq!(paired(&h).expect("read"), Some(first));
        assert!(pair(&home("bad"), "not a key").is_err());
    }
}
