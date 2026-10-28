use chrono::{DateTime, Utc};
use keyring::Entry;
use stardust_core::integrations::{SecretError, SecretStore};
use stardust_core::token::{StoredToken, TokenError, TokenStore};

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

/// No saved item, or no keychain service on this machine.
///
/// Matched on the error variant, not the message, because the message text
/// differs between the macOS, Windows, and Secret Service stores.
fn missing(err: &keyring::Error) -> bool {
    matches!(
        err,
        keyring::Error::NoEntry | keyring::Error::NoDefaultStore
    )
}

#[cfg(test)]
mod tests {
    use super::missing;

    #[test]
    fn only_the_no_entry_and_no_store_variants_count_as_missing() {
        assert!(missing(&keyring::Error::NoEntry));
        assert!(missing(&keyring::Error::NoDefaultStore));
        let platform = keyring::Error::PlatformFailure(Box::new(std::io::Error::other(
            "element not found in the vault",
        )));
        assert!(!missing(&platform));
        assert!(!missing(&keyring::Error::NoStorageAccess(Box::new(
            std::io::Error::other("no such keychain")
        ))));
    }
}
