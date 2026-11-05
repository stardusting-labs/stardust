//! Keychain record for a short-lived access token.
//!
//! The access token from ORBIT device sign-in is stored here. Callers can
//! store a token only when it expires within 5 minutes. Debug output redacts
//! the secret.
//!
//! A refresh token is kept next to it when the server sends one. The record
//! stays in the keychain after the access token expires while the refresh
//! token can still be used, so the app can get a new access token without
//! asking the user to sign in again. The refresh lifetime comes from the
//! server; this client sets no cap of its own. TODO(Unsu): confirm the refresh
//! lifetime policy with the rust-orbit desktop API contract owners.

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Mutex;

pub const MAX_ACCESS_TOKEN_LIFETIME: TimeDelta = TimeDelta::seconds(300);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("token is empty")]
    Empty,
    #[error("token is already expired")]
    AlreadyExpired,
    #[error("token must expire within 5 minutes")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_expires_at: Option<DateTime<Utc>>,
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
                refresh_expires_at: None,
            },
        })
    }

    /// Attach the server's refresh-token expiry. An expiry at or before `now`
    /// drops the refresh token.
    pub fn with_refresh_expiry(
        mut self,
        expires_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Self {
        match expires_at {
            Some(at) if at <= now => {
                self.record.refresh_token = None;
                self.record.refresh_expires_at = None;
            }
            other => self.record.refresh_expires_at = other,
        }
        self
    }

    /// Keep the previous refresh token when the server rotates the access
    /// token without sending a new refresh token.
    pub fn keep_refresh_from(mut self, previous: &StoredToken) -> Self {
        if self.record.refresh_token.is_none() {
            self.record.refresh_token = previous.record.refresh_token.clone();
            self.record.refresh_expires_at = previous.record.refresh_expires_at;
        }
        self
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

    pub fn refresh_expires_at(&self) -> Option<DateTime<Utc>> {
        self.record.refresh_expires_at
    }

    /// A refresh token is present and has not passed its server expiry.
    pub fn can_refresh(&self, now: DateTime<Utc>) -> bool {
        self.record.refresh_token.is_some()
            && self
                .record
                .refresh_expires_at
                .is_none_or(|expires_at| expires_at > now)
    }

    /// The record is still worth keeping: the access token is live, or it can
    /// be refreshed.
    pub fn is_usable(&self, now: DateTime<Utc>) -> bool {
        !self.is_expired(now) || self.can_refresh(now)
    }

    /// When the record stops being usable, as [`StoredToken::is_usable`] sees
    /// it. `None` is no known end: a refresh token with no server expiry. This
    /// is a time, not a secret.
    pub fn usable_until(&self) -> Option<DateTime<Utc>> {
        if self.record.refresh_token.is_none() {
            return Some(self.record.expires_at);
        }
        self.record
            .refresh_expires_at
            .map(|refresh| refresh.max(self.record.expires_at))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.record).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_json(text: &str, now: DateTime<Utc>) -> Result<Option<Self>, TokenError> {
        let record: TokenRecord = serde_json::from_str(text).map_err(|_| TokenError::Corrupt)?;
        if record.access_token.trim().is_empty()
            || matches!(record.refresh_token.as_deref(), Some(""))
        {
            return Err(TokenError::Corrupt);
        }
        let token = Self { record };
        if !token.is_usable(now) {
            return Ok(None);
        }
        Ok(Some(token))
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
            .field("refresh_expires_at", &self.record.refresh_expires_at)
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
        if guard.as_ref().is_some_and(|token| !token.is_usable(now)) {
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
        // Without a refresh token an expired record is dropped. A record with
        // a refresh token is kept; see `a_refreshable_record_outlives_its_access_token`.
        let now = Utc::now();
        let store = MemoryTokenStore::new();
        let token =
            StoredToken::issue("example-access", None, now + TimeDelta::seconds(240), now).unwrap();
        store.store(&token).unwrap();
        let loaded = store.load(now + TimeDelta::seconds(120)).unwrap().unwrap();
        assert_eq!(loaded.access_token(), "example-access");
        assert!(store.load(now + TimeDelta::seconds(240)).unwrap().is_none());
        assert!(store.load(now + TimeDelta::seconds(241)).unwrap().is_none());
    }

    #[test]
    fn a_refreshable_record_outlives_its_access_token() {
        let now = Utc::now();
        let store = MemoryTokenStore::new();
        let token = StoredToken::issue(
            "example-access",
            Some("example-refresh".into()),
            now + TimeDelta::seconds(300),
            now,
        )
        .unwrap()
        .with_refresh_expiry(Some(now + TimeDelta::days(1)), now);
        store.store(&token).unwrap();
        let later = now + TimeDelta::seconds(600);
        let loaded = store.load(later).unwrap().unwrap();
        assert!(loaded.is_expired(later));
        assert!(loaded.can_refresh(later));
        let restored = StoredToken::from_json(&loaded.to_json(), later)
            .unwrap()
            .unwrap();
        assert_eq!(restored, token);
        let past_refresh = now + TimeDelta::days(2);
        assert!(StoredToken::from_json(&token.to_json(), past_refresh)
            .unwrap()
            .is_none());
        assert!(store.load(past_refresh).unwrap().is_none());

        let no_expiry = StoredToken::issue(
            "example-access",
            Some("example-refresh".into()),
            now + TimeDelta::seconds(300),
            now,
        )
        .unwrap();
        assert!(no_expiry.can_refresh(now + TimeDelta::days(30)));
        let dropped = no_expiry.clone().with_refresh_expiry(Some(now), now);
        assert!(!dropped.can_refresh(now));
        assert!(dropped.refresh_token().is_none());

        let rotated =
            StoredToken::issue("example-next", None, later + TimeDelta::seconds(300), later)
                .unwrap()
                .keep_refresh_from(&token);
        assert_eq!(rotated.refresh_token(), Some("example-refresh"));
        assert_eq!(rotated.refresh_expires_at(), token.refresh_expires_at());
    }

    #[test]
    fn an_expired_record_without_a_refresh_token_is_dropped() {
        let now = Utc::now();
        let token =
            StoredToken::issue("example-access", None, now + TimeDelta::seconds(300), now).unwrap();
        assert!(!token.can_refresh(now));
        assert!(
            StoredToken::from_json(&token.to_json(), now + TimeDelta::seconds(300))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            StoredToken::from_json(
                r#"{"access_token":"a","refresh_token":"","expires_at":"2030-01-01T00:00:00Z"}"#,
                now
            )
            .unwrap_err(),
            TokenError::Corrupt
        );
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
            StoredToken::issue("example", None, now + TimeDelta::seconds(301), now).unwrap_err(),
            TokenError::LifetimeTooLong
        );
        assert!(StoredToken::issue("example", None, now + TimeDelta::seconds(300), now).is_ok());
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
