//! ORBIT device-code sign-in.
//!
//! The user code is printed. The device code and the access token are not.
//! The token is stored in the OS keychain.

use crate::keychain::KeychainTokenStore;
use chrono::{DateTime, TimeDelta, Utc};
use stardust_core::api::{
    https_origin, resolve_server_base, verification_url, OrbitError, ENV_SERVER_URL,
};
use stardust_core::config::Config;
use stardust_core::http::HttpOrbitClient;
use stardust_core::token::{StoredToken, TokenStore, MAX_ACCESS_TOKEN_LIFETIME};

pub fn sign_in(config: &Config) -> Result<(), String> {
    let base = resolve_server_base(
        config.server_base_url(),
        std::env::var(ENV_SERVER_URL).ok().as_deref(),
    )
    .map_err(|err| err.to_string())?;
    let client = HttpOrbitClient::new(&base).map_err(|err| err.to_string())?;
    let started = client.start_device_code().map_err(|err| err.to_string())?;
    println!(
        "Enter this code on the ORBIT account page: {}",
        started.user_code
    );
    if let Some(url) = verification_url(&base, &started) {
        println!("{url}");
        let _ = open::that(&url);
    } else if let Some(origin) = https_origin(&base) {
        println!("Open {origin} and enter the code there.");
    } else {
        println!("Open the ORBIT account page in your browser and enter the code there.");
    }
    let approved = client
        .wait_for_device_token(&started.device_code, started.expires_in, started.interval)
        .map_err(|err| err.to_string())?;
    let now = Utc::now();
    let expires_at = expiry(now, approved.expires_in, approved.expires_at_epoch_ms)?;
    let stored = StoredToken::issue(approved.access_token, None, expires_at, now)
        .map_err(|err| err.to_string())?;
    KeychainTokenStore::new()
        .store(&stored)
        .map_err(|err| err.to_string())?;
    println!("Signed in. The access token is in the system keychain.");
    Ok(())
}

pub fn sign_out() -> Result<(), String> {
    KeychainTokenStore::new()
        .clear()
        .map_err(|err| err.to_string())?;
    println!("Signed out. The access token was removed from the system keychain.");
    Ok(())
}

fn expiry(
    now: DateTime<Utc>,
    expires_in: u64,
    expires_at_epoch_ms: u64,
) -> Result<DateTime<Utc>, String> {
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
        (None, None) => return Err(OrbitError::Expired.to_string()),
    };
    if chosen <= now {
        return Err(OrbitError::Expired.to_string());
    }
    Ok(chosen.min(cap))
}
