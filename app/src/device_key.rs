//! The person's device keys (G18): two P-256 keys this Mac's Secure Enclave makes, one that signs only with a finger
//! enrolled now (the fingerprint key, biometry) and one that signs with their finger or their password (the device
//! key, presence); a Mac with no finger enrolled makes the device key alone. Their sealed blocks are files in the
//! engine's home (`device/`, 0600, written by renaming), which no box can read; a block is of use only on this Mac,
//! and signing with it takes the person's gesture, in one dialog for both keys.
//!
//! A build for tests keeps software keys in that folder instead, and only with an engine folder named in
//! JAYNES_ENGINE_HOME and JAYNES_APP_TEST_DEVICE (`touch`, `password`, `no-touch-id` or `cancel`: what the person's
//! gesture would be). A build for people has no software key at all.

use jaynes_core::proof::{KeyKind, PersonKey};
use std::path::{Path, PathBuf};

/// What the person's gesture gave: the device key's signature, and the fingerprint key's when the gesture was a
/// finger. Each in DER, in hex.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    pub presence: String,
    pub biometry: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gesture {
    Signed(Signed),
    /// The person closed the dialog.
    Cancelled,
}

/// This Mac's keys, as kept in their folder.
pub struct DeviceKeys {
    keys: Vec<PersonKey>,
    held: Held,
}

enum Held {
    Enclave { presence: Vec<u8>, biometry: Option<Vec<u8>> },
    #[cfg(feature = "test-driver")]
    Software { presence: p256::ecdsa::SigningKey, biometry: Option<p256::ecdsa::SigningKey>, gesture: String },
}

/// Where this Mac's keys are kept: the engine's home, which the boxes hide.
pub fn dir() -> PathBuf {
    jaynes_shell::home().join("device")
}

const PRESENCE: &str = "presence.key";
const BIOMETRY: &str = "biometry.key";

/// Whether software keys stand in for the enclave: only in a build for tests, and only when the test names both its
/// engine folder and the gesture.
pub fn software() -> bool {
    cfg!(feature = "test-driver") && crate::testing::var("JAYNES_APP_TEST_DEVICE").is_some() && std::env::var_os("JAYNES_ENGINE_HOME").is_some()
}

/// What this Mac can make: none (no Secure Enclave), the device key alone (no finger enrolled), or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Can {
    Nothing,
    DeviceKey,
    Both,
}

pub fn can() -> Can {
    #[cfg(feature = "test-driver")]
    if software() {
        let gesture = crate::testing::var("JAYNES_APP_TEST_DEVICE").unwrap_or_default();
        return if gesture == "no-touch-id" { Can::DeviceKey } else { Can::Both };
    }
    enclave_can()
}

#[cfg(target_os = "macos")]
fn enclave_can() -> Can {
    // SAFETY: a call into the app's own Swift (swift/DeviceKeys.swift) with no argument.
    match unsafe { enclave::jaynes_se_available() } {
        2 => Can::Both,
        1 => Can::DeviceKey,
        _ => Can::Nothing,
    }
}

#[cfg(not(target_os = "macos"))]
fn enclave_can() -> Can {
    Can::Nothing
}

impl DeviceKeys {
    /// The keys kept in `dir`, if this Mac made them.
    pub fn open(dir: &Path) -> Result<Option<DeviceKeys>, String> {
        #[cfg(feature = "test-driver")]
        if software() {
            return software_keys::open(dir);
        }
        let Ok(presence) = std::fs::read(dir.join(PRESENCE)) else { return Ok(None) };
        let biometry = std::fs::read(dir.join(BIOMETRY)).ok();
        let mut keys = vec![PersonKey { kind: KeyKind::Presence, public: hex(&enclave_public(&presence)?) }];
        if let Some(b) = &biometry {
            keys.insert(0, PersonKey { kind: KeyKind::Biometry, public: hex(&enclave_public(b)?) });
        }
        Ok(Some(DeviceKeys { keys, held: Held::Enclave { presence, biometry } }))
    }

    /// Makes this Mac's keys in `dir`: the device key, and the fingerprint key when a finger is enrolled. Keys there
    /// already are never replaced here.
    pub fn make(dir: &Path) -> Result<DeviceKeys, String> {
        if dir.join(PRESENCE).exists() || dir.join("soft-presence.key").exists() {
            return Err(format!("this Mac's keys are made already ({})", dir.display()));
        }
        #[cfg(feature = "test-driver")]
        if software() {
            return software_keys::make(dir);
        }
        let both = match can() {
            Can::Nothing => return Err("this Mac has no Secure Enclave: it cannot keep a key only you can use".into()),
            Can::DeviceKey => false,
            Can::Both => true,
        };
        let presence = enclave_create(false)?;
        let biometry = if both { Some(enclave_create(true)?) } else { None };
        // the fingerprint key first, so a device key on disk always has its companion when there is one
        if let Some(b) = &biometry {
            keep(dir, BIOMETRY, b)?;
        }
        keep(dir, PRESENCE, &presence)?;
        Self::open(dir)?.ok_or_else(|| "the keys made are not where they were kept".into())
    }

    /// The keys as they are paired: their kinds and public halves (SEC1, in hex).
    pub fn keys(&self) -> &[PersonKey] {
        &self.keys
    }

    /// Signs `message` with the person's gesture; `reason` finishes the dialog's "Jaynes is trying to …".
    pub fn sign(&self, message: &[u8], reason: &str) -> Result<Gesture, String> {
        match &self.held {
            Held::Enclave { presence, biometry } => enclave_sign(presence, biometry.as_deref(), message, reason),
            #[cfg(feature = "test-driver")]
            Held::Software { presence, biometry, gesture } => software_keys::sign(presence, biometry.as_ref(), gesture, message),
        }
    }
}

/// Writes a sealed block where only this user reads it, whole or not at all.
fn keep(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    f.write_all(bytes).and_then(|()| f.sync_all()).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, dir.join(name)).map_err(|e| format!("cannot keep {name}: {e}"))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(target_os = "macos")]
mod enclave {
    use std::ffi::c_char;
    // swift/DeviceKeys.swift, built into the app by build.rs
    unsafe extern "C" {
        pub fn jaynes_se_available() -> i32;
        pub fn jaynes_se_create(biometry: i32, block: *mut u8, block_cap: isize, block_len: *mut isize, public: *mut u8, public_cap: isize, public_len: *mut isize, why: *mut u8, why_cap: isize) -> i32;
        pub fn jaynes_se_public(block: *const u8, block_len: isize, public: *mut u8, public_cap: isize, public_len: *mut isize, why: *mut u8, why_cap: isize) -> i32;
        pub fn jaynes_se_sign(
            presence: *const u8,
            presence_len: isize,
            bio: *const u8,
            bio_len: isize,
            message: *const u8,
            message_len: isize,
            reason: *const c_char,
            presence_sig: *mut u8,
            presence_cap: isize,
            presence_sig_len: *mut isize,
            bio_sig: *mut u8,
            bio_cap: isize,
            bio_sig_len: *mut isize,
            why: *mut u8,
            why_cap: isize,
        ) -> i32;
    }

    pub fn why(buf: &[u8]) -> String {
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).into_owned()
    }
}

/// Room for what the enclave gives back: a sealed block is under 600 bytes, a public key 65, a signature 72.
const ROOM: usize = 2048;

#[cfg(target_os = "macos")]
fn enclave_create(biometry: bool) -> Result<Vec<u8>, String> {
    let (mut block, mut public, mut why) = (vec![0u8; ROOM], vec![0u8; 128], vec![0u8; 512]);
    let (mut block_len, mut public_len) = (0isize, 0isize);
    // SAFETY: every pointer is to a buffer of the length given with it, alive for the call.
    let done = unsafe {
        enclave::jaynes_se_create(biometry as i32, block.as_mut_ptr(), ROOM as isize, &mut block_len, public.as_mut_ptr(), 128, &mut public_len, why.as_mut_ptr(), 512)
    };
    if done != 0 {
        return Err(format!("the Secure Enclave made no key: {}", enclave::why(&why)));
    }
    block.truncate(block_len as usize);
    Ok(block)
}

#[cfg(target_os = "macos")]
fn enclave_public(block: &[u8]) -> Result<Vec<u8>, String> {
    let (mut public, mut why) = (vec![0u8; 128], vec![0u8; 512]);
    let mut len = 0isize;
    // SAFETY: as above; `block` is read only.
    let done = unsafe { enclave::jaynes_se_public(block.as_ptr(), block.len() as isize, public.as_mut_ptr(), 128, &mut len, why.as_mut_ptr(), 512) };
    if done != 0 {
        return Err(format!("this Mac's key does not open: {}", enclave::why(&why)));
    }
    public.truncate(len as usize);
    Ok(public)
}

#[cfg(target_os = "macos")]
fn enclave_sign(presence: &[u8], biometry: Option<&[u8]>, message: &[u8], reason: &str) -> Result<Gesture, String> {
    let reason = std::ffi::CString::new(reason.replace('\0', " ")).map_err(|e| e.to_string())?;
    let (mut p, mut b, mut why) = (vec![0u8; 128], vec![0u8; 128], vec![0u8; 512]);
    let (mut p_len, mut b_len) = (0isize, 0isize);
    let (bio, bio_len) = biometry.map_or((std::ptr::null(), 0), |b| (b.as_ptr(), b.len() as isize));
    // SAFETY: every pointer is to a buffer of the length given with it (or null with length 0), alive for the call.
    let done = unsafe {
        enclave::jaynes_se_sign(
            presence.as_ptr(),
            presence.len() as isize,
            bio,
            bio_len,
            message.as_ptr(),
            message.len() as isize,
            reason.as_ptr(),
            p.as_mut_ptr(),
            128,
            &mut p_len,
            b.as_mut_ptr(),
            128,
            &mut b_len,
            why.as_mut_ptr(),
            512,
        )
    };
    match done {
        0 => Ok(Gesture::Signed(Signed { presence: hex(&p[..p_len as usize]), biometry: (b_len > 0).then(|| hex(&b[..b_len as usize])) })),
        2 => Ok(Gesture::Cancelled),
        _ => Err(enclave::why(&why)),
    }
}

#[cfg(not(target_os = "macos"))]
fn enclave_create(_: bool) -> Result<Vec<u8>, String> {
    Err("no Secure Enclave on this system".into())
}
#[cfg(not(target_os = "macos"))]
fn enclave_public(_: &[u8]) -> Result<Vec<u8>, String> {
    Err("no Secure Enclave on this system".into())
}
#[cfg(not(target_os = "macos"))]
fn enclave_sign(_: &[u8], _: Option<&[u8]>, _: &[u8], _: &str) -> Result<Gesture, String> {
    Err("no Secure Enclave on this system".into())
}

/// Software keys made fresh in `dir` for a test, signing as `gesture` would: what tests use, apart from the
/// variables a test build reads.
#[cfg(feature = "test-driver")]
pub fn test_keys(dir: &Path, both: bool, gesture: &str) -> DeviceKeys {
    let _fresh = std::fs::remove_dir_all(dir);
    software_keys::make_with(dir, both, gesture).expect("software keys")
}

/// The software keys a test uses in the enclave's place: seeds in files of the test's engine folder.
#[cfg(feature = "test-driver")]
mod software_keys {
    use super::{DeviceKeys, Gesture, Held, Signed, hex, keep};
    use jaynes_core::proof::{KeyKind, PersonKey};
    use p256::ecdsa::signature::Signer as _;
    use p256::ecdsa::{Signature, SigningKey};
    use std::path::Path;

    fn read(dir: &Path, name: &str) -> Result<Option<SigningKey>, String> {
        match std::fs::read(dir.join(name)) {
            Ok(seed) => SigningKey::from_slice(&seed).map(Some).map_err(|_| format!("{name} is not a key (a software device key, for tests)")),
            Err(_) => Ok(None),
        }
    }

    fn public(k: &SigningKey) -> String {
        hex(k.verifying_key().to_encoded_point(false).as_bytes())
    }

    pub fn open(dir: &Path) -> Result<Option<DeviceKeys>, String> {
        let gesture = crate::testing::var("JAYNES_APP_TEST_DEVICE").map(|g| g.to_string_lossy().into_owned()).unwrap_or_default();
        open_with(dir, &gesture)
    }

    /// The software keys in `dir`, signing as the gesture named would.
    pub fn open_with(dir: &Path, gesture: &str) -> Result<Option<DeviceKeys>, String> {
        let gesture = gesture.to_owned();
        let Some(presence) = read(dir, "soft-presence.key")? else { return Ok(None) };
        let biometry = read(dir, "soft-biometry.key")?;
        let mut keys = vec![PersonKey { kind: KeyKind::Presence, public: public(&presence) }];
        if let Some(b) = &biometry {
            keys.insert(0, PersonKey { kind: KeyKind::Biometry, public: public(b) });
        }
        tracing::info!("a software device key, for tests, stands in for the Secure Enclave in {}", dir.display());
        Ok(Some(DeviceKeys { keys, held: Held::Software { presence, biometry, gesture } }))
    }

    pub fn make(dir: &Path) -> Result<DeviceKeys, String> {
        let gesture = crate::testing::var("JAYNES_APP_TEST_DEVICE").map(|g| g.to_string_lossy().into_owned()).unwrap_or_default();
        make_with(dir, super::can() == super::Can::Both, &gesture)
    }

    /// Software keys made in `dir`: both, or the device key alone as on a Mac with no finger enrolled.
    pub fn make_with(dir: &Path, both: bool, gesture: &str) -> Result<DeviceKeys, String> {
        let fresh = || -> Result<Vec<u8>, String> {
            let mut seed = [0u8; 32];
            use std::io::Read as _;
            std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut seed)).map_err(|e| e.to_string())?;
            SigningKey::from_slice(&seed).map(|_| seed.to_vec()).map_err(|_| "no key from this seed".to_string())
        };
        if both {
            keep(dir, "soft-biometry.key", &fresh()?)?;
        }
        keep(dir, "soft-presence.key", &fresh()?)?;
        open_with(dir, gesture)?.ok_or_else(|| "the software keys made are not where they were kept".into())
    }

    /// As the enclave signs, with the gesture the test names: a finger signs with both, a password with the device key
    /// alone, a cancelled dialog with none.
    pub fn sign(presence: &SigningKey, biometry: Option<&SigningKey>, gesture: &str, message: &[u8]) -> Result<Gesture, String> {
        let der = |k: &SigningKey| {
            let s: Signature = k.sign(message);
            hex(s.to_der().as_bytes())
        };
        Ok(match gesture {
            "cancel" => Gesture::Cancelled,
            "password" => Gesture::Signed(Signed { presence: der(presence), biometry: None }),
            _ => Gesture::Signed(Signed { presence: der(presence), biometry: biometry.map(der) }),
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(not(feature = "test-driver"))]
    fn a_build_for_people_never_takes_a_software_key() {
        use super::software;
        let home = std::env::temp_dir().join(format!("jaynes-device-{}", std::process::id()));
        // SAFETY: this test alone sets these variables, and nothing in this build reads JAYNES_APP_TEST_DEVICE.
        unsafe {
            std::env::set_var("JAYNES_APP_TEST_DEVICE", "touch");
            std::env::set_var("JAYNES_ENGINE_HOME", &home);
        }
        assert!(!software(), "a build for people took a software device key");
    }

    #[test]
    #[cfg(feature = "test-driver")]
    fn a_build_for_tests_makes_signs_and_opens_its_software_keys_by_the_gesture_named() {
        use super::{Gesture, software_keys, test_keys};
        use jaynes_core::proof::{self, KeyKind, Proof};
        let dir = std::env::temp_dir().join(format!("jaynes-device-t-{}", std::process::id()));
        let made = test_keys(&dir, true, "touch");
        assert_eq!(made.keys().iter().map(|k| k.kind).collect::<Vec<_>>(), [KeyKind::Biometry, KeyKind::Presence]);
        // keys there already are never made again over them
        assert!(super::DeviceKeys::make(&dir).err().is_some_and(|e| e.contains("made already")));
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(dir.join("soft-presence.key")).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        let again = software_keys::open_with(&dir, "touch").unwrap().unwrap();
        assert_eq!(again.keys(), made.keys());
        // a finger signs with both keys, each over the message, as the engine checks it
        let Gesture::Signed(s) = again.sign(b"the message", "approve").unwrap() else { panic!("not signed") };
        let bio = Proof::Device { key: KeyKind::Biometry, signature: s.biometry.clone().unwrap() };
        let any = Proof::Device { key: KeyKind::Presence, signature: s.presence.clone() };
        assert_eq!(proof::holds(again.keys(), b"the message", &bio), Ok(KeyKind::Biometry));
        assert_eq!(proof::holds(again.keys(), b"the message", &any), Ok(KeyKind::Presence));
        assert!(proof::holds(again.keys(), b"another", &any).is_err());
        // a password signs with the device key alone, a closed dialog with none
        let by_password = software_keys::open_with(&dir, "password").unwrap().unwrap();
        let Gesture::Signed(p) = by_password.sign(b"m", "approve").unwrap() else { panic!("not signed") };
        assert!(p.biometry.is_none());
        assert_eq!(software_keys::open_with(&dir, "cancel").unwrap().unwrap().sign(b"m", "approve").unwrap(), Gesture::Cancelled);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
