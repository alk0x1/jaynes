//! Pairing this Mac with the engine (G18): the person's device keys (device_key.rs) recorded in the engine's record as
//! theirs, so that what cannot be undone, and leaving the box, take a proof only they can give. The app asks the
//! person's gesture first (Touch ID, or their password), then sends the change over the engine's socket
//! (`person.sock`), which takes it only from this app's recorded build. The keys are read from the record alone: what
//! the app shows of the pairing is what the record holds, every change checked as the engine checks it.

use crate::device_key::{DeviceKeys, Gesture};
use jaynes_core::event::Event;
use jaynes_core::pairing::{self, Change, Keys};
use jaynes_core::proof::{KeyKind, PersonKey, Proof};
use std::path::Path;

/// What the dialog says the app is trying to do ("Jaynes is trying to pair this Mac with Jaynes").
pub const REASON: &str = "pair this Mac with Jaynes";

/// The pairing that stands now, as the record holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paired {
    /// When it was recorded, in seconds since the epoch.
    pub at: u64,
    pub keys: Vec<PersonKey>,
    pub nonce: String,
    /// Where it came from, as the engine saw it: its channel and the build that sent it.
    pub channel: String,
    pub caller: String,
}

impl Paired {
    /// Whether the keys paired are `mine`, whatever their order.
    pub fn is(&self, mine: &[PersonKey]) -> bool {
        self.keys.len() == mine.len() && self.keys.iter().all(|k| mine.contains(k))
    }
}

/// The pairing that stands in the record of `home`.
pub fn standing(home: &Path) -> Result<Option<Paired>, String> {
    let store = jaynes_shell::store::Store::open(&home.join("record.sqlite3"))?;
    let changes = store.history()?.into_iter().filter(|(_, run, _)| run == pairing::RECORD).filter_map(|(at, _, e)| match e {
        Event::KeysChanged { change } => Some((at, change)),
        _ => None,
    });
    Ok(standing_in(changes))
}

/// The pairing the changes leave, each checked as the engine checks it: one that does not hold changes nothing.
fn standing_in(changes: impl IntoIterator<Item = (u64, Change)>) -> Option<Paired> {
    let mut keys = Keys::default();
    let mut now = None;
    for (at, change) in changes {
        if keys.take(&change).is_err() {
            continue;
        }
        now = match change {
            Change::Paired { keys, nonce, from, .. } => Some(Paired { at, keys, nonce, channel: from.channel, caller: from.caller }),
            Change::Unpaired { .. } => None,
        };
    }
    now
}

/// The change that pairs this Mac's keys over the keys paired now (`now`), signed with the person's gesture; none if
/// they closed the dialog. The first pairing takes no proof (the engine's rule); replacing keys takes the device key
/// paired now, so keys of another device cannot be replaced from here.
pub fn change(now: &[PersonKey], keys: &DeviceKeys) -> Result<Option<Change>, String> {
    let mine = keys.keys().to_vec();
    let ours_now = now.iter().any(|k| k.kind == KeyKind::Presence && mine.contains(k));
    if !now.is_empty() && !ours_now {
        return Err("another device's keys are paired with this engine: unpair them from that device first".into());
    }
    let nonce = jaynes_shell::person::nonce()?;
    let message = pairing::message(now, &nonce, Some(&mine));
    let signed = match keys.sign(&message, REASON)? {
        Gesture::Cancelled => return Ok(None),
        Gesture::Signed(s) => s,
    };
    let proof = (!now.is_empty()).then(|| Proof::Device { key: KeyKind::Presence, signature: signed.presence });
    Ok(Some(Change::Paired { keys: mine, nonce, proof, from: Default::default() }))
}

/// What a press on Pair did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pairing {
    /// Recorded by the engine: the pairing's nonce.
    Done(String),
    /// The person closed the dialog: nothing was sent.
    Cancelled,
    /// This Mac's keys are paired already.
    Already,
}

/// Pairs this Mac: makes its keys the first time (in `dir`), asks the person's gesture, and sends the change to the
/// engine of `home`, which records it only from this app's recorded build.
pub fn pair(home: &Path, dir: &Path) -> Result<Pairing, String> {
    let keys = match DeviceKeys::open(dir)? {
        Some(k) => k,
        None => DeviceKeys::make(dir)?,
    };
    let now = standing(home)?;
    if now.as_ref().is_some_and(|p| p.is(keys.keys())) {
        return Ok(Pairing::Already);
    }
    let Some(change) = change(&now.map(|p| p.keys).unwrap_or_default(), &keys)? else { return Ok(Pairing::Cancelled) };
    let Change::Paired { nonce, .. } = &change else { unreachable!("change() only pairs") };
    let nonce = nonce.clone();
    jaynes_shell::person_socket::send(home, &serde_json::to_value(&change).map_err(|e| e.to_string())?)?;
    Ok(Pairing::Done(nonce))
}

#[cfg(all(test, feature = "test-driver"))]
mod tests {
    use super::{change, standing_in};
    use crate::device_key::test_keys;
    use jaynes_core::pairing::{Change, Keys};
    use jaynes_core::proof::KeyKind;

    fn dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("jaynes-pairing-{}-{name}", std::process::id()))
    }

    #[test]
    fn the_first_pairing_takes_the_gesture_and_no_proof_and_the_engine_takes_it() {
        let d = dir("first");
        let keys = test_keys(&d, true, "touch");
        let c = change(&[], &keys).unwrap().unwrap();
        let Change::Paired { keys: sent, proof, .. } = &c else { panic!("{c:?}") };
        assert_eq!(sent.iter().map(|k| k.kind).collect::<Vec<_>>(), [KeyKind::Biometry, KeyKind::Presence]);
        assert!(proof.is_none());
        // the engine's own rule takes it
        let mut engine = Keys::default();
        engine.take(&c).unwrap();
        assert_eq!(engine.keys, keys.keys());
        let p = standing_in([(10, c)]).unwrap();
        assert!(p.is(keys.keys()));
        assert_eq!(p.at, 10);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn what_the_engine_records_is_what_the_app_reads_back_with_when_and_from_where() {
        let home = dir("record");
        let mine = test_keys(&home.join("device"), true, "touch");
        let mut c = change(&[], &mine).unwrap().unwrap();
        // the engine marks where it came from, over what the change said, and records it
        c.came_from("app", "dev.jaynes.app /Applications/Jaynes.app");
        let mut store = jaynes_shell::store::Store::open(&home.join("record.sqlite3")).unwrap();
        store.change_keys(&c).unwrap();
        let p = super::standing(&home).unwrap().unwrap();
        assert!(p.is(mine.keys()));
        assert_eq!((p.channel.as_str(), p.caller.as_str()), ("app", "dev.jaynes.app /Applications/Jaynes.app"));
        assert!(p.at > 0);
        // nothing paired in a record with no pairing
        let empty = dir("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(super::standing(&empty).unwrap(), None);
        for d in [home, empty] {
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn a_closed_dialog_sends_nothing_and_a_mac_without_touch_id_pairs_its_device_key_alone() {
        let d = dir("cancel");
        assert_eq!(change(&[], &test_keys(&d, true, "cancel")).unwrap(), None);
        let d2 = dir("no-touch-id");
        let keys = test_keys(&d2, false, "touch");
        let Some(Change::Paired { keys: sent, .. }) = change(&[], &keys).unwrap() else { panic!() };
        assert_eq!(sent.iter().map(|k| k.kind).collect::<Vec<_>>(), [KeyKind::Presence]);
        for d in [d, d2] {
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn keys_paired_now_are_replaced_only_with_their_own_proof_and_another_devices_not_from_here() {
        let (d, other) = (dir("replace"), dir("other"));
        let mine = test_keys(&d, true, "touch");
        let first = change(&[], &mine).unwrap().unwrap();
        let mut engine = Keys::default();
        engine.take(&first).unwrap();
        // the same Mac pairs again (its keys, a new nonce): signed by its device key, the engine takes it
        let again = change(&engine.keys, &mine).unwrap().unwrap();
        let Change::Paired { proof, .. } = &again else { panic!() };
        assert!(proof.is_some());
        engine.take(&again).unwrap();
        // another device's keys paired: this Mac cannot replace them
        let theirs = test_keys(&other, true, "touch");
        assert!(change(&engine.keys, &theirs).unwrap_err().contains("another device"));
        // and a change written with a wrong proof leaves the pairing as it stood
        let forged = Change::Unpaired { nonce: "n1".into(), proof: jaynes_core::proof::Proof::Device { key: KeyKind::Presence, signature: "3006020101020101".into() }, from: Default::default() };
        let stood = standing_in([(1, first.clone()), (2, forged)]).unwrap();
        assert!(stood.is(mine.keys()));
        for d in [d, other] {
            let _ = std::fs::remove_dir_all(&d);
        }
    }
}
