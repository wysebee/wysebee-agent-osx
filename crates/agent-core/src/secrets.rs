//! Where the link's secret is kept: the macOS Keychain, never the state file.
//!
//! The secret is 32 random bytes (base64url) this app makes when pairing starts; Wysebee
//! Cloud keeps only its SHA-256. It is sent as `Authorization: Bearer` to the cloud the
//! link was made with and to nothing else, never shown and never logged.
//!
//! [`SecretStore`] is the seam: the app installs [`Keychain`]; **every test uses
//! [`MemorySecrets`]** and never touches the real Keychain.

use std::sync::Mutex;

/// The one secret this app holds.
pub trait SecretStore: Send + Sync {
    /// The secret; None when there is none. An error is the store failing (a locked
    /// keychain), which is not the same as "none".
    fn get(&self) -> Result<Option<String>, String>;
    fn set(&self, secret: &str) -> Result<(), String>;
    /// Removes it; removing what is not there is fine.
    fn delete(&self) -> Result<(), String>;
}

/// A secret kept in memory: the tests' store.
#[derive(Default)]
pub struct MemorySecrets {
    secret: Mutex<Option<String>>,
    /// When set, every call fails with it (a store that cannot be used).
    failing: Mutex<Option<String>>,
}

impl MemorySecrets {
    pub fn new() -> MemorySecrets {
        MemorySecrets::default()
    }

    /// Make every later call fail with `error`, or work again with None.
    pub fn fail_with(&self, error: Option<&str>) {
        *self.failing.lock().unwrap() = error.map(str::to_string);
    }

    /// What is stored, whatever `fail_with` says.
    pub fn peek(&self) -> Option<String> {
        self.secret.lock().unwrap().clone()
    }

    fn usable(&self) -> Result<(), String> {
        match self.failing.lock().unwrap().clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

impl SecretStore for MemorySecrets {
    fn get(&self) -> Result<Option<String>, String> {
        self.usable()?;
        Ok(self.secret.lock().unwrap().clone())
    }

    fn set(&self, secret: &str) -> Result<(), String> {
        self.usable()?;
        *self.secret.lock().unwrap() = Some(secret.to_string());
        Ok(())
    }

    fn delete(&self) -> Result<(), String> {
        self.usable()?;
        *self.secret.lock().unwrap() = None;
        Ok(())
    }
}

/// The Keychain item's service: the app's bundle identifier.
pub const KEYCHAIN_SERVICE: &str = "com.wysebee.agent.osx";
/// The Keychain item's account for the default state folder (the installed app).
pub const KEYCHAIN_ACCOUNT: &str = "link-secret";

/// The Keychain item's account for a state folder: [`KEYCHAIN_ACCOUNT`] for the default
/// one, else `link-secret <the first 16 hex digits of the folder path's SHA-256>`. The
/// secret belongs with the state file that holds its link, so a copy run with another
/// state folder (`WYSEBEE_AGENT_STATE_DIR`, `scripts/run_dev.sh`) pairs without replacing
/// the installed copy's secret.
pub fn keychain_account(state_dir: &std::path::Path, default_dir: &std::path::Path) -> String {
    if state_dir == default_dir {
        return KEYCHAIN_ACCOUNT.to_string();
    }
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(state_dir.as_os_str().as_encoded_bytes());
    format!("{KEYCHAIN_ACCOUNT} {}", &hex::encode(digest)[..16])
}

/// The login keychain (`secrets/keychain.rs`): the one file the tests never run, since they
/// must not touch the real Keychain; `scripts/check.sh` leaves it out of the 100% rule.
#[cfg(target_os = "macos")]
mod keychain;
#[cfg(target_os = "macos")]
pub use keychain::Keychain;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_default_state_folder_keeps_the_installed_apps_item_and_any_other_has_its_own() {
        let default = Path::new("/Users/a/Library/Application Support/Wysebee Agent");
        assert_eq!(keychain_account(default, default), "link-secret");
        let dev = keychain_account(Path::new("/Users/a/Library/Application Support/Wysebee Agent Dev/dev"), default);
        let local = keychain_account(Path::new("/Users/a/Library/Application Support/Wysebee Agent Dev/local"), default);
        assert!(dev.starts_with("link-secret ") && dev.len() == "link-secret ".len() + 16, "{dev}");
        assert_ne!(dev, local);
        assert_eq!(dev, keychain_account(Path::new("/Users/a/Library/Application Support/Wysebee Agent Dev/dev"), default));
    }

    #[test]
    fn the_memory_store_keeps_one_secret_and_can_be_made_to_fail() {
        let s = MemorySecrets::new();
        assert_eq!(s.get().unwrap(), None);
        s.delete().unwrap();
        s.set("one").unwrap();
        s.set("two").unwrap();
        assert_eq!(s.get().unwrap().as_deref(), Some("two"));
        s.fail_with(Some("locked"));
        assert_eq!(s.get().unwrap_err(), "locked");
        assert_eq!(s.set("x").unwrap_err(), "locked");
        assert_eq!(s.delete().unwrap_err(), "locked");
        assert_eq!(s.peek().as_deref(), Some("two"));
        s.fail_with(None);
        s.delete().unwrap();
        assert_eq!(s.peek(), None);
    }
}
