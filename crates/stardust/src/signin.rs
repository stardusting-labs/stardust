//! ORBIT device-code sign-in.
//!
//! The user code is printed or shown in the settings window. The device
//! code, the access token, and any refresh token are not. Tokens are stored
//! in the OS keychain.

use crate::keychain::KeychainTokenStore;
use chrono::Utc;
use stardust_core::api::{https_origin, resolve_server_base, verification_url, ENV_SERVER_URL};
use stardust_core::config::Config;
use stardust_core::http::{HttpOrbitClient, Revoke};
use stardust_core::session::stored_from_response;
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

/// `stardust sign-out`. With auto sign-in on, a saved refresh token is also
/// revoked on ORBIT after the keychain copy is deleted. A server without the
/// revoke route is fine; the local copy is gone either way.
pub fn sign_out(config: Option<&Config>) -> Result<(), String> {
    let revoke = config.filter(|config| config.features().auto_sign_in);
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
