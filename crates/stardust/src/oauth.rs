//! Optional Slack MCP sign-in.
//!
//! A loopback listener exists only while this command runs. The verifier and
//! the resulting tokens stay in the keychain. The integration stays off.

use crate::deliver::https_post;
use stardust_core::integrations::{secret_account, SecretSlot, SecretStore};
use stardust_core::mcp::{
    parse_slack_token, pkce_challenge, slack_authorize_url, slack_token_form, SLACK_TOKEN_URL,
};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};
use uuid::Uuid;

const PREFERRED_PORT: u16 = 47321;
const WAIT: Duration = Duration::from_secs(180);

pub fn slack_sign_in(secrets: &dyn SecretStore, id: &str, client_id: &str) -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", PREFERRED_PORT))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
        .map_err(|_| "could not listen for the Slack redirect".to_string())?;
    let port = listener
        .local_addr()
        .map_err(|_| "could not listen for the Slack redirect".to_string())?
        .port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let state = Uuid::new_v4().simple().to_string();
    let account = secret_account(id, SecretSlot::PkceVerifier);
    secrets
        .put(&account, &verifier)
        .map_err(|_| "could not store the sign-in verifier".to_string())?;
    let challenge = pkce_challenge(&verifier);
    let url = slack_authorize_url(client_id, &redirect, &state, &challenge);
    println!("Waiting for Slack to redirect to {redirect}");
    println!("{url}");
    let _ = open::that(&url);
    let result = wait_for_code(&listener, &state);
    let _ = secrets.delete(&account);
    let code = result?;
    let form = slack_token_form(client_id, &code, &verifier, &redirect);
    let reply = https_post(
        SLACK_TOKEN_URL,
        "application/x-www-form-urlencoded",
        &[],
        form.as_bytes(),
    )
    .map_err(|_| "Slack token request failed".to_string())?;
    if reply.status != 200 {
        return Err("Slack token response was refused".to_string());
    }
    let text = String::from_utf8(reply.body)
        .map_err(|_| "Slack token response could not be read".to_string())?;
    let access = parse_slack_token(&text).map_err(|err| match err {
        stardust_core::mcp::McpError::Refused(code) => {
            format!("Slack token response was refused ({code})")
        }
        stardust_core::mcp::McpError::Response => {
            "Slack token response could not be read".to_string()
        }
    })?;
    secrets
        .put(
            &secret_account(id, SecretSlot::AccessToken),
            &access.access_token,
        )
        .map_err(|_| "could not store the access token".to_string())?;
    if let Some(refresh) = access.refresh_token {
        secrets
            .put(&secret_account(id, SecretSlot::RefreshToken), &refresh)
            .map_err(|_| "could not store the refresh token".to_string())?;
    }
    println!("Slack sign-in saved for {id}. The integration stays off until you enable it.");
    Ok(())
}

fn wait_for_code(listener: &TcpListener, expected_state: &str) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|_| "could not listen for the Slack redirect".to_string())?;
    let deadline = Instant::now() + WAIT;
    let (mut stream, _) = loop {
        if Instant::now() >= deadline {
            return Err("sign-in timed out".to_string());
        }
        match listener.accept() {
            Ok(pair) => break pair,
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(_) => return Err("could not listen for the Slack redirect".to_string()),
        }
    };
    stream
        .set_nonblocking(false)
        .map_err(|_| "could not read the Slack redirect".to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|_| "could not read the Slack redirect".to_string())?;
    let request = read_request(&mut stream)?;
    let query =
        query_string(&request).ok_or_else(|| "could not read the Slack redirect".to_string())?;
    let code = query_value(query, "code");
    let state = query_value(query, "state");
    let ok = state.as_deref() == Some(expected_state) && code.as_deref().is_some_and(valid_code);
    let body = if ok {
        "Stardust saved the Slack sign-in. You can close this window.\n"
    } else {
        "Stardust could not complete the Slack sign-in. You can close this window.\n"
    };
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    if ok {
        Ok(code.unwrap_or_default())
    } else {
        Err("Slack redirect did not match this sign-in".to_string())
    }
}

fn read_request(stream: &mut impl Read) -> Result<String, String> {
    let mut collected = Vec::new();
    let mut buf = [0_u8; 1024];
    loop {
        if collected.windows(4).any(|window| window == b"\r\n\r\n") || collected.len() >= 8192 {
            break;
        }
        let read = stream
            .read(&mut buf)
            .map_err(|_| "could not read the Slack redirect".to_string())?;
        if read == 0 {
            break;
        }
        collected.extend_from_slice(&buf[..read]);
    }
    String::from_utf8(collected).map_err(|_| "could not read the Slack redirect".to_string())
}

fn query_string(request: &str) -> Option<&str> {
    let line = request.lines().next()?;
    let path = line.split_whitespace().nth(1)?;
    path.split_once('?').map(|(_, query)| query)
}

fn query_value(query: &str, name: &str) -> Option<String> {
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        if key == name {
            return Some(value.to_string());
        }
    }
    None
}

fn valid_code(code: &str) -> bool {
    (1..=512).contains(&code.len())
        && code.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/')
        })
}
