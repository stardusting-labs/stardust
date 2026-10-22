use chrono::{DateTime, Utc};
use keyring::Entry;
use stardust_core::integrations::{SecretError, SecretStore};
use stardust_core::token::{StoredToken, TokenError, TokenStore};
use std::fmt::Display;

pub struct KeychainTokenStore;

impl KeychainTokenStore {
    pub fn new() -> Self {
        Self
    }
}

impl TokenStore for KeychainTokenStore {
    fn store(&self, token: &StoredToken) -> Result<(), TokenError> {
        let entry = open().map_err(|_| TokenError::Keychain)?;
        entry
            .set_password(&token.to_json())
            .map_err(|_| TokenError::Keychain)
    }

    fn load(&self, now: DateTime<Utc>) -> Result<Option<StoredToken>, TokenError> {
        let entry = match open() {
            Ok(entry) => entry,
            Err(err) if missing(&err) => return Ok(None),
            Err(_) => return Err(TokenError::Keychain),
        };
        match entry.get_password() {
            Ok(secret) => match StoredToken::from_json(&secret, now)? {
                Some(token) => Ok(Some(token)),
                None => {
                    let _ = entry.delete_credential();
                    Ok(None)
                }
            },
            Err(err) if missing(&err) => Ok(None),
            Err(_) => Err(TokenError::Keychain),
        }
    }

    fn clear(&self) -> Result<(), TokenError> {
        let entry = match open() {
            Ok(entry) => entry,
            Err(err) if missing(&err) => return Ok(()),
            Err(_) => return Err(TokenError::Keychain),
        };
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(err) if missing(&err) => Ok(()),
            Err(_) => Err(TokenError::Keychain),
        }
    }
}

fn open() -> Result<Entry, keyring::Error> {
    Entry::new("stardust", "orbit-access-token")
}

impl SecretStore for KeychainTokenStore {
    fn put(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        if secret.is_empty() {
            return Err(SecretError::Empty);
        }
        let entry = Entry::new("stardust", account).map_err(|_| SecretError::Keychain)?;
        entry
            .set_password(secret)
            .map_err(|_| SecretError::Keychain)
    }

    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        let entry = match Entry::new("stardust", account) {
            Ok(entry) => entry,
            Err(err) if missing(&err) => return Ok(None),
            Err(_) => return Err(SecretError::Keychain),
        };
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(err) if missing(&err) => Ok(None),
            Err(_) => Err(SecretError::Keychain),
        }
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        let entry = match Entry::new("stardust", account) {
            Ok(entry) => entry,
            Err(err) if missing(&err) => return Ok(()),
            Err(_) => return Err(SecretError::Keychain),
        };
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(err) if missing(&err) => Ok(()),
            Err(_) => Err(SecretError::Keychain),
        }
    }
}

fn missing(err: &impl Display) -> bool {
    let text = err.to_string().to_ascii_lowercase();
    text.contains("no entry")
        || text.contains("not found")
        || text.contains("no such")
        || text.contains("no matching credential")
        || text.contains("no default store")
}
