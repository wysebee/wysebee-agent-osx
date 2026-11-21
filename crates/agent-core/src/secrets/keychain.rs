//! The login keychain: one generic password, service [`KEYCHAIN_SERVICE`], account
//! [`super::keychain_account`]. **Not exercised by the tests** (they must not touch the real
//! one), so this file is the only one outside the 100% line rule (`scripts/check.sh`).
//! Keep it this small.

use super::{SecretStore, KEYCHAIN_SERVICE};
use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};

/// `errSecItemNotFound`.
const NOT_FOUND: i32 = -25300;

pub struct Keychain {
    pub account: String,
}

impl SecretStore for Keychain {
    fn get(&self) -> Result<Option<String>, String> {
        match get_generic_password(KEYCHAIN_SERVICE, &self.account) {
            Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| "The Keychain item could not be read.".to_string()),
            Err(e) if e.code() == NOT_FOUND => Ok(None),
            Err(e) => Err(format!("The Keychain could not be read: {e}")),
        }
    }

    fn set(&self, secret: &str) -> Result<(), String> {
        set_generic_password(KEYCHAIN_SERVICE, &self.account, secret.as_bytes()).map_err(|e| format!("The Keychain could not be written: {e}"))
    }

    fn delete(&self) -> Result<(), String> {
        match delete_generic_password(KEYCHAIN_SERVICE, &self.account) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == NOT_FOUND => Ok(()),
            Err(e) => Err(format!("The Keychain could not be changed: {e}")),
        }
    }
}
