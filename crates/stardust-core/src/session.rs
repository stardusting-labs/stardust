//! Access-token lifecycle between sign-in and each check.
//!
//! A check needs a live access token. When the stored one has expired and a
//! refresh token is kept, [`current_access`] trades it once for a new access
//! token. When there is nothing to refresh, or the server refuses the refresh
//! token, the record is cleared and the user is asked to sign in again.

use crate::api::{DeviceTokenResponse, OrbitError};
use crate::token::{StoredToken, TokenStore, MAX_ACCESS_TOKEN_LIFETIME};
use chrono::{DateTime, TimeDelta, Utc};

/// Something that can trade a refresh token for a new access token.
pub trait Refresher {
    fn refresh(&self, refresh_token: &str) -> Result<DeviceTokenResponse, OrbitError>;
}

impl Refresher for crate::http::HttpOrbitClient {
    fn refresh(&self, refresh_token: &str) -> Result<DeviceTokenResponse, OrbitError> {
        self.refresh_access_token(refresh_token)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// A live access token. `refreshed` is true when it came from a refresh
    /// in this call.
    Ready { token: StoredToken, refreshed: bool },
    /// No usable record. The user needs to sign in.
    SignedOut,
    /// The refresh request did not reach the server or was not readable. The
    /// record is kept for the next try.
    Unavailable(OrbitError),
}

/// Build the keychain record from a device-token or refresh response.
pub fn stored_from_response(
    response: DeviceTokenResponse,
    now: DateTime<Utc>,
) -> Result<StoredToken, OrbitError> {
    let expires_at = access_expiry(now, response.expires_in, response.expires_at_epoch_ms)?;
    let refresh_expiry = response.refresh_expires_in.map(|seconds| {
        let seconds = i64::try_from(seconds).unwrap_or(i64::MAX);
        now.checked_add_signed(TimeDelta::seconds(seconds))
            .unwrap_or(DateTime::<Utc>::MAX_UTC)
    });
    StoredToken::issue(
        response.access_token,
        response.refresh_token,
        expires_at,
        now,
    )
    .map(|token| token.with_refresh_expiry(refresh_expiry, now))
    .map_err(|_| OrbitError::Response)
}

/// The earlier of `expires_in` and `expires_at_epoch_ms`, capped at 5 minutes.
pub fn access_expiry(
    now: DateTime<Utc>,
    expires_in: u64,
    expires_at_epoch_ms: u64,
) -> Result<DateTime<Utc>, OrbitError> {
    let cap = now + MAX_ACCESS_TOKEN_LIFETIME;
    let from_ttl = (expires_in > 0).then(|| {
        let seconds = i64::try_from(expires_in).unwrap_or(i64::MAX);
        now.checked_add_signed(TimeDelta::seconds(seconds))
            .unwrap_or(cap)
    });
    let from_ms = (expires_at_epoch_ms > 0)
        .then(|| {
            i64::try_from(expires_at_epoch_ms)
                .ok()
                .and_then(DateTime::from_timestamp_millis)
        })
        .flatten();
    let chosen = match (from_ttl, from_ms) {
        (Some(ttl), Some(stamp)) => ttl.min(stamp),
        (Some(ttl), None) => ttl,
        (None, Some(stamp)) => stamp,
        (None, None) => return Err(OrbitError::Expired),
    };
    if chosen <= now {
        return Err(OrbitError::Expired);
    }
    Ok(chosen.min(cap))
}

/// Return a live access token, refreshing an expired one when possible.
///
/// `force_refresh` is used after the server answered 401 to a token that
/// looked live here.
pub fn current_access(
    store: &dyn TokenStore,
    refresher: Option<&dyn Refresher>,
    now: DateTime<Utc>,
    force_refresh: bool,
) -> Access {
    let stored = match store.load(now) {
        Ok(Some(token)) => token,
        Ok(None) | Err(_) => return Access::SignedOut,
    };
    if !force_refresh && !stored.is_expired(now) {
        return Access::Ready {
            token: stored,
            refreshed: false,
        };
    }
    let (Some(refresher), Some(refresh_token)) = (refresher, stored.refresh_token()) else {
        let _ = store.clear();
        return Access::SignedOut;
    };
    if !stored.can_refresh(now) {
        let _ = store.clear();
        return Access::SignedOut;
    }
    match refresher.refresh(refresh_token) {
        Ok(response) => match stored_from_response(response, now) {
            Ok(next) => {
                let next = next.keep_refresh_from(&stored);
                if store.store(&next).is_err() {
                    return Access::Unavailable(OrbitError::Refused);
                }
                Access::Ready {
                    token: next,
                    refreshed: true,
                }
            }
            Err(err) => Access::Unavailable(err),
        },
        Err(OrbitError::Unauthorized | OrbitError::Expired | OrbitError::Denied) => {
            let _ = store.clear();
            Access::SignedOut
        }
        Err(err) => Access::Unavailable(err),
    }
}

#[cfg(test)]
mod tests {
    use super::{access_expiry, current_access, stored_from_response, Access, Refresher};
    use crate::api::{DeviceTokenResponse, OrbitError};
    use crate::token::{MemoryTokenStore, StoredToken, TokenStore};
    use chrono::{TimeDelta, Utc};
    use std::cell::RefCell;

    struct FakeRefresher {
        answer: Result<DeviceTokenResponse, OrbitError>,
        seen: RefCell<Vec<String>>,
    }

    impl Refresher for FakeRefresher {
        fn refresh(&self, refresh_token: &str) -> Result<DeviceTokenResponse, OrbitError> {
            self.seen.borrow_mut().push(refresh_token.to_string());
            self.answer.clone()
        }
    }

    fn response(access: &str, refresh: Option<&str>) -> DeviceTokenResponse {
        DeviceTokenResponse {
            token: access.to_string(),
            access_token: access.to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 300,
            expires_at_epoch_ms: 0,
            refresh_token: refresh.map(str::to_string),
            refresh_expires_in: Some(3_600),
        }
    }

    fn fake(answer: Result<DeviceTokenResponse, OrbitError>) -> FakeRefresher {
        FakeRefresher {
            answer,
            seen: RefCell::new(Vec::new()),
        }
    }

    #[test]
    fn a_live_token_is_used_without_a_refresh() {
        let now = Utc::now();
        let store = MemoryTokenStore::new();
        store
            .store(&stored_from_response(response("access-1", Some("refresh-1")), now).unwrap())
            .unwrap();
        let refresher = fake(Err(OrbitError::Unreachable));
        let got = current_access(
            &store,
            Some(&refresher),
            now + TimeDelta::seconds(60),
            false,
        );
        assert!(
            matches!(got, Access::Ready { refreshed: false, ref token } if token.access_token() == "access-1")
        );
        assert!(refresher.seen.borrow().is_empty());
    }

    #[test]
    fn an_expired_token_is_refreshed_and_stored_without_a_new_sign_in() {
        let now = Utc::now();
        let store = MemoryTokenStore::new();
        store
            .store(&stored_from_response(response("access-1", Some("refresh-1")), now).unwrap())
            .unwrap();
        let later = now + TimeDelta::minutes(20);
        let refresher = fake(Ok(response("access-2", None)));
        let got = current_access(&store, Some(&refresher), later, false);
        let Access::Ready { token, refreshed } = got else {
            panic!("expected a refreshed token");
        };
        assert!(refreshed);
        assert_eq!(token.access_token(), "access-2");
        assert_eq!(token.refresh_token(), Some("refresh-1"));
        assert_eq!(refresher.seen.borrow().as_slice(), ["refresh-1"]);
        let kept = store.load(later).unwrap().unwrap();
        assert_eq!(kept.access_token(), "access-2");
        let again = current_access(
            &store,
            Some(&refresher),
            later + TimeDelta::seconds(30),
            false,
        );
        assert!(matches!(
            again,
            Access::Ready {
                refreshed: false,
                ..
            }
        ));
        assert_eq!(refresher.seen.borrow().len(), 1);
    }

    #[test]
    fn a_401_forces_one_refresh_even_for_a_live_token() {
        let now = Utc::now();
        let store = MemoryTokenStore::new();
        store
            .store(&stored_from_response(response("access-1", Some("refresh-1")), now).unwrap())
            .unwrap();
        let refresher = fake(Ok(response("access-2", Some("refresh-2"))));
        let got = current_access(&store, Some(&refresher), now, true);
        assert!(matches!(got, Access::Ready { refreshed: true, ref token }
            if token.access_token() == "access-2" && token.refresh_token() == Some("refresh-2")));
    }

    #[test]
    fn a_refused_refresh_token_signs_out_and_a_network_error_keeps_the_record() {
        let now = Utc::now();
        let later = now + TimeDelta::minutes(20);
        let store = MemoryTokenStore::new();
        let record = stored_from_response(response("access-1", Some("refresh-1")), now).unwrap();
        store.store(&record).unwrap();
        let offline = fake(Err(OrbitError::Unreachable));
        assert_eq!(
            current_access(&store, Some(&offline), later, false),
            Access::Unavailable(OrbitError::Unreachable)
        );
        assert!(store.load(later).unwrap().is_some());
        let refused = fake(Err(OrbitError::Unauthorized));
        assert_eq!(
            current_access(&store, Some(&refused), later, false),
            Access::SignedOut
        );
        assert!(store.load(later).unwrap().is_none());
    }

    #[test]
    fn without_a_refresh_token_the_old_five_minute_behaviour_stays() {
        let now = Utc::now();
        let store = MemoryTokenStore::new();
        store
            .store(&stored_from_response(response("access-1", None), now).unwrap())
            .unwrap();
        let refresher = fake(Ok(response("access-2", None)));
        assert_eq!(
            current_access(
                &store,
                Some(&refresher),
                now + TimeDelta::seconds(301),
                false
            ),
            Access::SignedOut
        );
        assert!(refresher.seen.borrow().is_empty());
        let demo_store = MemoryTokenStore::new();
        demo_store
            .store(
                &StoredToken::issue(
                    "access-1",
                    Some("refresh-1".into()),
                    now + TimeDelta::seconds(10),
                    now,
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            current_access(&demo_store, None, now + TimeDelta::seconds(11), false),
            Access::SignedOut
        );
    }

    #[test]
    fn access_expiry_takes_the_earlier_value_and_caps_at_five_minutes() {
        let now = chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        assert_eq!(
            access_expiry(now, 120, 0).unwrap(),
            now + TimeDelta::seconds(120)
        );
        assert_eq!(
            access_expiry(now, 3_600, 0).unwrap(),
            now + TimeDelta::seconds(300)
        );
        assert_eq!(
            access_expiry(now, 200, 1_800_000_060_000).unwrap(),
            now + TimeDelta::seconds(60)
        );
        assert_eq!(access_expiry(now, 0, 0).unwrap_err(), OrbitError::Expired);
        assert_eq!(
            access_expiry(now, 0, 1_799_999_999_000).unwrap_err(),
            OrbitError::Expired
        );
    }
}
