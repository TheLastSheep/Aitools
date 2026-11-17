//! OS Keychain credential isolation (`SPEC-001-R5`, `INV-SEC-03`).

use std::collections::HashMap;
use std::sync::Mutex;

use crate::{NativeError, Result};

/// Abstraction over OS Keychain credential storage.
pub trait SecretStore: Send + Sync {
    /// Stores `secret` under `account`.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::Secret`] if the underlying keychain rejects or fails the write.
    fn set_secret(&self, account: &str, secret: &str) -> Result<()>;

    /// Retrieves the stored secret for `account`, or `None` if absent.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::Secret`] if querying the underlying keychain fails.
    fn get_secret(&self, account: &str) -> Result<Option<String>>;

    /// Deletes any stored secret for `account`.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::Secret`] if deleting from the underlying keychain fails.
    fn delete_secret(&self, account: &str) -> Result<()>;
}

/// Production OS Keychain store backed by `keyring`.
pub struct KeyringSecretStore {
    service: String,
}

impl KeyringSecretStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
}

impl Default for KeyringSecretStore {
    fn default() -> Self {
        Self::new("aitools.credentials")
    }
}

impl SecretStore for KeyringSecretStore {
    fn set_secret(&self, account: &str, secret: &str) -> Result<()> {
        let entry = keyring::Entry::new(&self.service, account)
            .map_err(|e| NativeError::Secret(e.to_string()))?;
        entry
            .set_password(secret)
            .map_err(|e| NativeError::Secret(e.to_string()))
    }

    fn get_secret(&self, account: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(&self.service, account)
            .map_err(|e| NativeError::Secret(e.to_string()))?;
        match entry.get_password() {
            Ok(pw) => Ok(Some(pw)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(NativeError::Secret(e.to_string())),
        }
    }

    fn delete_secret(&self, account: &str) -> Result<()> {
        let entry = keyring::Entry::new(&self.service, account)
            .map_err(|e| NativeError::Secret(e.to_string()))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(NativeError::Secret(e.to_string())),
        }
    }
}

/// In-memory secret store for CI and unit tests.
#[derive(Default)]
pub struct MemorySecretStore {
    entries: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemorySecretStore {
    fn set_secret(&self, account: &str, secret: &str) -> Result<()> {
        self.entries
            .lock()
            .expect("memory secret store mutex poisoned")
            .insert(account.to_string(), secret.to_string());
        Ok(())
    }

    fn get_secret(&self, account: &str) -> Result<Option<String>> {
        Ok(self
            .entries
            .lock()
            .expect("memory secret store mutex poisoned")
            .get(account)
            .cloned())
    }

    fn delete_secret(&self, account: &str) -> Result<()> {
        self.entries
            .lock()
            .expect("memory secret store mutex poisoned")
            .remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_001_r5_secret_store_isolates_and_deletes_credentials() {
        let store = MemorySecretStore::default();
        store.set_secret("openai.fast", "sk-test-12345").unwrap();
        assert_eq!(
            store.get_secret("openai.fast").unwrap().as_deref(),
            Some("sk-test-12345")
        );
        store.delete_secret("openai.fast").unwrap();
        assert_eq!(store.get_secret("openai.fast").unwrap(), None);
    }
}
