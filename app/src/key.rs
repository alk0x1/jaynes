//! The person's own key: an Ed25519 signing key the app makes once and keeps in the person's macOS keychain. macOS
//! lets this app read it without asking; any other program that tries gets a prompt the person sees. Its public half
//! is paired with the engine (`jaynes_shell::person`), which counts an answer as the person's only when this key
//! signed it.

use ed25519_dalek::{Signer, SigningKey};
use std::path::Path;

const SERVICE: &str = "dev.jaynes.app";
const ACCOUNT: &str = "answer-key";

pub struct Key {
    signing: SigningKey,
}

impl Key {
    /// The key in the keychain, made and kept there the first time.
    pub fn load() -> Result<Key, String> {
        use security_framework::passwords::{get_generic_password, set_generic_password};
        // A test keeps its key in a file of its own: the keychain would ask the person about each new build.
        if let Some(file) = std::env::var_os("JAYNES_APP_TEST_KEY") {
            let seed = match std::fs::read(&file) {
                Ok(seed) => seed,
                Err(_) => {
                    let seed = random()?;
                    std::fs::write(&file, &seed).map_err(|e| e.to_string())?;
                    seed
                }
            };
            let seed: [u8; 32] = seed.try_into().map_err(|_| "the test key is not 32 bytes")?;
            return Ok(Key { signing: SigningKey::from_bytes(&seed) });
        }
        let seed = match get_generic_password(SERVICE, ACCOUNT) {
            Ok(bytes) => bytes,
            Err(_) => {
                let seed = random()?;
                set_generic_password(SERVICE, ACCOUNT, &seed).map_err(|e| format!("the keychain did not keep the app's key: {e}"))?;
                seed
            }
        };
        let seed: [u8; 32] = seed.try_into().map_err(|_| "the key in the keychain is not 32 bytes")?;
        Ok(Key { signing: SigningKey::from_bytes(&seed) })
    }

    /// The public half, in hex, as the engine pairs it.
    pub fn public(&self) -> String {
        jaynes_shell::person::hex(self.signing.verifying_key().as_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> String {
        jaynes_shell::person::hex(&self.signing.sign(message).to_bytes())
    }

    /// Pairs this key with the engine at `home`; refused when another key is paired.
    pub fn pair(&self, home: &Path) -> Result<(), String> {
        jaynes_shell::person::pair(home, &self.public())
    }
}

fn random() -> Result<Vec<u8>, String> {
    let mut seed = vec![0u8; 32];
    std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom").map_err(|e| e.to_string())?, &mut seed).map_err(|e| e.to_string())?;
    Ok(seed)
}
