//! MCP JSON-RPC helpers.
//!
//! ORBIT's outbound webhook draft describes an MCP server as a later addition.
//! This module builds the client requests and stays idle until a saved
//! endpoint and token exist. Slack's hosted MCP is optional and uses OAuth.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const SLACK_MCP_URL: &str = "https://mcp.slack.com/mcp";
pub const SLACK_OAUTH_METADATA: &str =
    "https://mcp.slack.com/.well-known/oauth-authorization-server";
pub const SLACK_AUTHORIZE_URL: &str = "https://slack.com/oauth/v2_user/authorize";
pub const SLACK_TOKEN_URL: &str = "https://slack.com/api/oauth.v2.user.access";
pub const SLACK_ALERT_SCOPE: &str = "chat:write";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpError {
    #[error("MCP response could not be read")]
    Response,
    #[error("Slack token response was refused ({0})")]
    Refused(String),
}

pub fn initialize_request(id: u64) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "stardust", "version": "0.1.0" }
        }
    })
    .to_string()
}

pub fn tools_list_request(id: u64) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/list",
        "params": {}
    })
    .to_string()
}

pub fn server_name(body: &str) -> Result<String, McpError> {
    let value: Value = serde_json::from_str(body).map_err(|_| McpError::Response)?;
    value["result"]["serverInfo"]["name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .ok_or(McpError::Response)
}

pub fn tool_names(body: &str) -> Result<Vec<String>, McpError> {
    let value: Value = serde_json::from_str(body).map_err(|_| McpError::Response)?;
    let tools = value["result"]["tools"]
        .as_array()
        .ok_or(McpError::Response)?;
    let mut names = Vec::new();
    for tool in tools {
        let Some(name) = tool["name"].as_str() else {
            return Err(McpError::Response);
        };
        names.push(name.to_string());
    }
    Ok(names)
}

pub fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64url(&digest)
}

pub fn slack_authorize_url(
    client_id: &str,
    redirect: &str,
    state: &str,
    challenge: &str,
) -> String {
    format!(
        "{SLACK_AUTHORIZE_URL}?client_id={}&scope={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method=S256",
        form_escape(client_id),
        form_escape(SLACK_ALERT_SCOPE),
        form_escape(redirect),
        form_escape(state),
        form_escape(challenge),
    )
}

pub fn slack_token_form(client_id: &str, code: &str, verifier: &str, redirect: &str) -> String {
    format!(
        "client_id={}&code={}&code_verifier={}&redirect_uri={}",
        form_escape(client_id),
        form_escape(code),
        form_escape(verifier),
        form_escape(redirect),
    )
}

pub fn parse_slack_token(body: &str) -> Result<SlackAccess, McpError> {
    let value: Value = serde_json::from_str(body).map_err(|_| McpError::Response)?;
    if value.get("ok") == Some(&Value::Bool(false)) {
        let raw = value["error"].as_str().unwrap_or("refused");
        let code = if raw.len() <= 40
            && raw
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            raw
        } else {
            "refused"
        };
        return Err(McpError::Refused(code.to_string()));
    }
    let token = value["access_token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .ok_or(McpError::Response)?;
    let refresh = value["refresh_token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .map(str::to_string);
    Ok(SlackAccess {
        access_token: token.to_string(),
        refresh_token: refresh,
    })
}

pub struct SlackAccess {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

impl std::fmt::Debug for SlackAccess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SlackAccess")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

fn base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let block = u32::from(bytes[index]) << 16
            | u32::from(bytes[index + 1]) << 8
            | u32::from(bytes[index + 2]);
        out.push(TABLE[((block >> 18) & 63) as usize] as char);
        out.push(TABLE[((block >> 12) & 63) as usize] as char);
        out.push(TABLE[((block >> 6) & 63) as usize] as char);
        out.push(TABLE[(block & 63) as usize] as char);
        index += 3;
    }
    let rest = bytes.len() - index;
    if rest == 1 {
        let block = u32::from(bytes[index]) << 16;
        out.push(TABLE[((block >> 18) & 63) as usize] as char);
        out.push(TABLE[((block >> 12) & 63) as usize] as char);
    } else if rest == 2 {
        let block = u32::from(bytes[index]) << 16 | u32::from(bytes[index + 1]) << 8;
        out.push(TABLE[((block >> 18) & 63) as usize] as char);
        out.push(TABLE[((block >> 12) & 63) as usize] as char);
        out.push(TABLE[((block >> 6) & 63) as usize] as char);
    }
    out
}

fn form_escape(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        initialize_request, parse_slack_token, pkce_challenge, server_name, slack_authorize_url,
        slack_token_form, tool_names, SLACK_MCP_URL,
    };

    #[test]
    fn pkce_and_slack_urls_stay_free_of_a_client_secret() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let url = slack_authorize_url(
            "123.456",
            "http://127.0.0.1:47321/callback",
            "state",
            "challenge",
        );
        assert!(url.starts_with("https://slack.com/oauth/v2_user/authorize?"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(!url.contains("client_secret"));
        let form = slack_token_form(
            "123.456",
            "code",
            "verifier",
            "http://127.0.0.1:47321/callback",
        );
        assert!(form.contains("code_verifier=verifier"));
        assert!(!form.contains("client_secret"));
        assert_eq!(SLACK_MCP_URL, "https://mcp.slack.com/mcp");
    }

    #[test]
    fn initialize_and_token_parsers_redact_secrets() {
        let request = initialize_request(1);
        assert!(request.contains("\"method\":\"initialize\""));
        assert_eq!(
            server_name(r#"{"result":{"serverInfo":{"name":"orbit"}}}"#).unwrap(),
            "orbit"
        );
        assert_eq!(
            tool_names(r#"{"result":{"tools":[{"name":"workspace.status"}]}}"#).unwrap(),
            vec!["workspace.status".to_string()]
        );
        let token = parse_slack_token(
            r#"{"ok":true,"access_token":"access-token-value","refresh_token":"refresh-token-value"}"#,
        )
        .unwrap();
        assert_eq!(token.access_token, "access-token-value");
        let rendered = format!("{token:?}");
        assert!(!rendered.contains("access-token-value"));
        assert!(!rendered.contains("refresh-token-value"));
        assert!(rendered.contains("<redacted>"));
        assert!(parse_slack_token(r#"{"ok":false,"error":"invalid_code"}"#).is_err());
    }
}
