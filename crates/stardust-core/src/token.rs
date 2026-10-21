//! Keychain record for a short-lived access token.
//!
//! Sign-in is not implemented. Callers can store a token only when it expires
//! within 15 minutes. Debug output redacts the secret.

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Mutex;

pub const MAX_ACCESS_TOKEN_LIFETIME: TimeDelta = TimeDelta::minutes(15);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("token is empty")]
    Empty,
    #[error("token is already expired")]
    AlreadyExpired,
    #[error("token must expire within 15 minutes")]
    LifetimeTooLong,
    #[error("stored token could not be read")]
    Corrupt,
    #[error("keychain request failed")]
    Keychain,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TokenRecord {
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    expires_at: DateTime<Utc>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct StoredToken {
    record: TokenRecord,
}

impl StoredToken {
    pub fn issue(
        access_token: impl Into<String>,
        refresh_token: Option<String>,
        expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Self, TokenError> {
        let access_token = access_token.into();
        if access_token.trim().is_empty() {
            return Err(TokenError::Empty);
        }
        if matches!(refresh_token.as_deref(), Some("")) {
            return Err(TokenError::Empty);
        }
        if expires_at <= now {
            return Err(TokenError::AlreadyExpired);
        }
        if expires_at - now > MAX_ACCESS_TOKEN_LIFETIME {
            return Err(TokenError::LifetimeTooLong);
        }
        Ok(Self {
            record: TokenRecord {
                access_token,
                refresh_token,
                expires_at,
            },
        })
    }

    pub fn access_token(&self) -> &str {
        &self.record.access_token
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.record.refresh_token.as_deref()
    }

    pub fn expires_at(&self) -> DateTime<Utc> {
        self.record.expires_at
    }

    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.record.expires_at <= now
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.record).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_json(text: &str, now: DateTime<Utc>) -> Result<Option<Self>, TokenError> {
        let record: TokenRecord = serde_json::from_str(text).map_err(|_| TokenError::Corrupt)?;
        if record.access_token.trim().is_empty() {
            return Err(TokenError::Corrupt);
        }
        if record.expires_at <= now {
            return Ok(None);
        }
        Ok(Some(Self { record }))
    }
}

impl fmt::Debug for StoredToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredToken")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.record.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at", &self.record.expires_at)
            .finish()
    }
}

pub trait TokenStore: Send + Sync {
    fn store(&self, token: &StoredToken) -> Result<(), TokenError>;
    fn load(&self, now: DateTime<Utc>) -> Result<Option<StoredToken>, TokenError>;
    fn clear(&self) -> Result<(), TokenError>;
}

#[derive(Default)]
pub struct MemoryTokenStore {
    inner: Mutex<Option<StoredToken>>,
}

impl MemoryTokenStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TokenStore for MemoryTokenStore {
    fn store(&self, token: &StoredToken) -> Result<(), TokenError> {
        *self.inner.lock().unwrap_or_else(|err| err.into_inner()) = Some(token.clone());
        Ok(())
    }

    fn load(&self, now: DateTime<Utc>) -> Result<Option<StoredToken>, TokenError> {
        let mut guard = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        if guard.as_ref().is_some_and(|token| token.is_expired(now)) {
            *guard = None;
        }
        Ok(guard.clone())
    }

    fn clear(&self) -> Result<(), TokenError> {
        *self.inner.lock().unwrap_or_else(|err| err.into_inner()) = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{MemoryTokenStore, StoredToken, TokenError, TokenStore};
    use chrono::{TimeDelta, Utc};

    #[test]
    fn stores_loads_and_drops_an_expired_token() {
        let now = Utc::now();
        let store = MemoryTokenStore::new();
        let token = StoredToken::issue(
            "example-access",
            Some("example-refresh".into()),
            now + TimeDelta::minutes(10),
            now,
        )
        .unwrap();
        store.store(&token).unwrap();
        let loaded = store.load(now + TimeDelta::minutes(5)).unwrap().unwrap();
        assert_eq!(loaded.access_token(), "example-access");
        assert!(store.load(now + TimeDelta::minutes(10)).unwrap().is_none());
        assert!(store.load(now + TimeDelta::minutes(11)).unwrap().is_none());
    }

    #[test]
    fn rejects_empty_expired_and_long_lived_tokens() {
        let now = Utc::now();
        assert_eq!(
            StoredToken::issue("  ", None, now + TimeDelta::minutes(5), now).unwrap_err(),
            TokenError::Empty
        );
        assert_eq!(
            StoredToken::issue(
                "example",
                Some(String::new()),
                now + TimeDelta::minutes(5),
                now
            )
            .unwrap_err(),
            TokenError::Empty
        );
        assert_eq!(
            StoredToken::issue("example", None, now, now).unwrap_err(),
            TokenError::AlreadyExpired
        );
        assert_eq!(
            StoredToken::issue("example", None, now + TimeDelta::minutes(16), now).unwrap_err(),
            TokenError::LifetimeTooLong
        );
        let token = StoredToken::issue(
            "super-secret-token",
            Some("super-secret-refresh".into()),
            now + TimeDelta::minutes(5),
            now,
        )
        .unwrap();
        let rendered = format!("{token:?}");
        assert!(!rendered.contains("super-secret-token"));
        assert!(!rendered.contains("super-secret-refresh"));
        let restored = StoredToken::from_json(&token.to_json(), now)
            .unwrap()
            .unwrap();
        assert_eq!(restored, token);
    }
}
