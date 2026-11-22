//! ORBIT device-code sign-in.
//!
//! The user code is printed or shown in the settings window. The device
//! code, the access token, and any refresh token are not. Tokens are stored
//! in the OS keychain.

use crate::keychain::KeychainTokenStore;
use crate::live::LivePet;
use chrono::Utc;
use stardust_core::api::{https_origin, resolve_server_base, verification_url, ENV_SERVER_URL};
use stardust_core::config::Config;
use stardust_core::http::{HttpOrbitClient, Revoke};
use stardust_core::session::stored_from_response;
use stardust_core::signin_window::{read_session, SessionRead};
use stardust_core::token::TokenStore;

/// Terminal sign-in for `stardust sign-in`.
pub fn sign_in(config: &Config) -> Result<(), String> {
    sign_in_with(config, &mut |user_code, url, origin| {
        println!("Enter this code on the ORBIT account page: {user_code}");
        if let Some(url) = url {
            println!("{url}");
        } else if let Some(origin) = origin {
            println!("Open {origin} and enter the code there.");
        } else {
            println!("Open the ORBIT account page in your browser and enter the code there.");
        }
    })?;
    println!("Signed in. The access token is in the system keychain.");
    Ok(())
}

/// Receives the user code, the https verification page when there is one,
/// and the https origin.
pub type OnCode<'a> = dyn FnMut(&str, Option<&str>, Option<&str>) + 'a;

/// Run the device flow. The page is opened in the browser after `on_code`
/// returns.
pub fn sign_in_with(config: &Config, on_code: &mut OnCode<'_>) -> Result<(), String> {
    let base = resolve_server_base(
        config.server_base_url(),
        std::env::var(ENV_SERVER_URL).ok().as_deref(),
    )
    .map_err(|err| err.to_string())?;
    let client = HttpOrbitClient::new(&base).map_err(|err| err.to_string())?;
    let started = client.start_device_code().map_err(|err| err.to_string())?;
    let url = verification_url(&base, &started);
    let origin = https_origin(&base);
    on_code(&started.user_code, url.as_deref(), origin.as_deref());
    if let Some(url) = &url {
        let _ = open::that(url);
    }
    let approved = client
        .wait_for_device_token(&started.device_code, started.expires_in, started.interval)
        .map_err(|err| err.to_string())?;
    let stored = stored_from_response(approved, Utc::now()).map_err(|err| err.to_string())?;
    KeychainTokenStore::new()
        .store(&stored)
        .map_err(|err| err.to_string())?;
    Ok(())
}

/// `stardust sign-out`. With auto sign-in or the launch sign-in window on, a
/// saved refresh token is also revoked on ORBIT after the keychain copy is
/// deleted. A server without the revoke route is fine; the local copy is gone
/// either way.
pub fn sign_out(config: Option<&Config>) -> Result<(), String> {
    let revoke = config.filter(|config| config.features().sign_in_on_timer());
    let refresh = revoke.and_then(|_| saved_refresh_token());
    clear_keychain_token()?;
    println!("Signed out. The access token was removed from the system keychain.");
    if let (Some(config), Some(refresh)) = (revoke, refresh) {
        let revoked = resolve_server_base(
            config.server_base_url(),
            std::env::var(ENV_SERVER_URL).ok().as_deref(),
        )
        .ok()
        .and_then(|base| HttpOrbitClient::new(&base).ok())
        .map(|client| client.revoke_refresh_token(&refresh));
        if revoked == Some(Revoke::Revoked) {
            println!("ORBIT revoked the refresh token.");
        }
    }
    Ok(())
}

/// The refresh token in the saved record, if there is one.
pub fn saved_refresh_token() -> Option<String> {
    KeychainTokenStore::new()
        .load(Utc::now())
        .ok()
        .flatten()
        .and_then(|token| token.refresh_token().map(str::to_string))
}

pub fn clear_keychain_token() -> Result<(), String> {
    KeychainTokenStore::new()
        .clear()
        .map_err(|err| err.to_string())
}

pub fn signed_in() -> bool {
    KeychainTokenStore::new()
        .load(Utc::now())
        .ok()
        .flatten()
        .is_some()
}

/// What the keychain holds. Unlike [`signed_in`] this tells a keychain that
/// could not be read from one with no record.
pub fn session() -> SessionRead {
    read_session(&KeychainTokenStore::new(), Utc::now())
}

/// Sign out with the launch sign-in window on, from the tray or the settings
/// window: delete the keychain copy, then hand the rest to the check timer,
/// which revokes the refresh token and drops any sign-in still running.
///
/// A record that could not be deleted is an error, and nothing else changes,
/// so the app does not claim to be signed out while the keychain holds one.
pub fn begin_sign_out(tokens: &dyn TokenStore, live: &LivePet) -> Result<(), String> {
    let refresh = tokens
        .load(Utc::now())
        .ok()
        .flatten()
        .and_then(|token| token.refresh_token().map(str::to_string));
    tokens.clear().map_err(|err| err.to_string())?;
    live.queue_sign_out(refresh.as_deref());
    live.request_cancel();
    live.note_session(None);
    live.set_signed_in(false);
    live.set_observed(stardust_core::pet::ObservedCheck::SignedOut);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::begin_sign_out;
    use crate::live::LivePet;
    use chrono::{DateTime, TimeDelta, Utc};
    use stardust_core::pet::ObservedCheck;
    use stardust_core::token::{MemoryTokenStore, StoredToken, TokenError, TokenStore};

    struct Locked;

    impl TokenStore for Locked {
        fn store(&self, _: &StoredToken) -> Result<(), TokenError> {
            Err(TokenError::Keychain)
        }
        fn load(&self, _: DateTime<Utc>) -> Result<Option<StoredToken>, TokenError> {
            Err(TokenError::Keychain)
        }
        fn clear(&self) -> Result<(), TokenError> {
            Err(TokenError::Keychain)
        }
    }

    #[test]
    fn sign_out_deletes_the_record_and_queues_the_revoke_and_the_cancel() {
        let now = Utc::now();
        let store = MemoryTokenStore::new();
        let expires = now + TimeDelta::seconds(120);
        store
            .store(&StoredToken::issue("access", Some("refresh-1".into()), expires, now).unwrap())
            .unwrap();
        let live = LivePet::new(1.0);
        live.set_signed_in(true);
        live.request_connect();
        begin_sign_out(&store, &live).unwrap();
        assert_eq!(store.load(now).unwrap(), None);
        assert_eq!(live.signed_in(), Some(false));
        assert_eq!(live.observed(), ObservedCheck::SignedOut);
        let requests = live.take_flow_requests();
        assert!(requests.cancel, "a running sign-in is dropped");
        assert_eq!(requests.connect, None, "and one about to start never does");
        assert_eq!(
            live.take_sign_out().unwrap().refresh_token(),
            Some("refresh-1")
        );
    }

    #[test]
    fn a_keychain_that_will_not_delete_says_so_and_stays_signed_in() {
        let live = LivePet::new(1.0);
        live.set_signed_in(true);
        live.set_observed(ObservedCheck::Waiting);
        assert!(begin_sign_out(&Locked, &live).is_err());
        assert_eq!(live.signed_in(), Some(true));
        assert_eq!(live.observed(), ObservedCheck::Waiting);
        assert!(live.take_sign_out().is_none(), "nothing was queued");
        assert!(!live.take_flow_requests().cancel);
    }
}
