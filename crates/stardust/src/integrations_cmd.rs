//! Settings commands for the integration catalog.
//!
//! Secrets are written to the keychain and are not printed.

use crate::deliver::{https_post, HttpReply};
use crate::keychain::KeychainTokenStore;
use crate::oauth::slack_sign_in;
use stardust_core::config::{load_config, save_config, Config};
use stardust_core::integrations::{secret_account, IntegrationKind, SecretSlot, SecretStore};
use stardust_core::mcp::{
    initialize_request, server_name, tool_names, tools_list_request, SLACK_MCP_URL,
};
use stardust_core::outbound::public_https;
use stardust_core::schedule::parse_timezone;
use std::path::Path;

use crate::cli::IntegrationAction;

pub fn apply(path: &Path, action: IntegrationAction) -> Result<(), String> {
    let mut config = load_or_create(path)?;
    let secrets = KeychainTokenStore::new();
    match action {
        IntegrationAction::List => {
            list(&config);
            return Ok(());
        }
        IntegrationAction::Add { kind, id } => {
            let parsed = IntegrationKind::parse(&kind)
                .ok_or_else(|| "unknown integration kind".to_string())?;
            config
                .add_integration(parsed, &id)
                .map_err(|err| err.to_string())?;
            save_config(path, &config).map_err(|err| err.to_string())?;
            println!("added {} {id} (off)", parsed.as_str());
        }
        IntegrationAction::Enable { id } => set_enabled(path, &mut config, &id, true)?,
        IntegrationAction::Disable { id } => set_enabled(path, &mut config, &id, false)?,
        IntegrationAction::Url {
            id,
            url,
            signing_secret,
        } => save_url(
            path,
            &mut config,
            &secrets,
            &id,
            &url,
            signing_secret.as_deref(),
        )?,
        IntegrationAction::Token { id, token } => save_token(&config, &secrets, &id, &token)?,
        IntegrationAction::Oauth { id, client_id } => {
            let kind = kind_of(&config, &id)?;
            if kind != IntegrationKind::SlackMcp {
                return Err("oauth is for the Slack MCP integration".to_string());
            }
            config
                .set_integration_client_id(&id, &client_id)
                .map_err(|err| err.to_string())?;
            save_config(path, &config).map_err(|err| err.to_string())?;
            slack_sign_in(&secrets, &id, &client_id)?;
        }
        IntegrationAction::Connect { id } => connect(&config, &secrets, &id)?,
    }
    Ok(())
}

fn list(config: &Config) {
    if config.integrations().is_empty() {
        println!("Integrations off");
        return;
    }
    for record in config.integrations() {
        let state = if record.enabled { "on" } else { "off" };
        let host = record.host.as_deref().unwrap_or("—");
        println!("{} {} {state} {host}", record.id, record.kind.as_str());
    }
}

fn set_enabled(path: &Path, config: &mut Config, id: &str, enabled: bool) -> Result<(), String> {
    config
        .set_integration_enabled(id, enabled)
        .map_err(|err| err.to_string())?;
    save_config(path, config).map_err(|err| err.to_string())?;
    let state = if enabled { "on" } else { "off" };
    println!("{id} {state}");
    Ok(())
}

fn save_url(
    path: &Path,
    config: &mut Config,
    secrets: &KeychainTokenStore,
    id: &str,
    url: &str,
    signing_secret: Option<&str>,
) -> Result<(), String> {
    let checked = public_https(url).map_err(|_| "the endpoint must be an https URL".to_string())?;
    let kind = kind_of(config, id)?;
    match kind {
        IntegrationKind::SlackIncoming
        | IntegrationKind::TeamsIncoming
        | IntegrationKind::GenericJson => {
            secrets
                .put(&secret_account(id, SecretSlot::Url), &checked.url)
                .map_err(|_| "could not store the destination".to_string())?;
            if let Some(secret) = signing_secret {
                if secret.is_empty() {
                    return Err("signing secret is empty".to_string());
                }
                secrets
                    .put(&secret_account(id, SecretSlot::SigningSecret), secret)
                    .map_err(|_| "could not store the signing secret".to_string())?;
            }
            config
                .set_integration_host(id, &checked.host)
                .map_err(|err| err.to_string())?;
        }
        IntegrationKind::OrbitMcp => {
            if signing_secret.is_some() {
                return Err("ORBIT MCP does not take a signing secret".to_string());
            }
            config
                .set_integration_endpoint(id, &checked.url)
                .map_err(|_| "the endpoint must be an https URL".to_string())?;
        }
        IntegrationKind::SlackMcp => {
            return Err("Slack MCP uses its hosted endpoint".to_string());
        }
    }
    save_config(path, config).map_err(|err| err.to_string())?;
    println!("saved destination for {id} ({})", checked.host);
    Ok(())
}

fn save_token(
    config: &Config,
    secrets: &KeychainTokenStore,
    id: &str,
    token: &str,
) -> Result<(), String> {
    let kind = kind_of(config, id)?;
    match kind {
        IntegrationKind::OrbitMcp | IntegrationKind::SlackMcp => {}
        IntegrationKind::SlackIncoming
        | IntegrationKind::TeamsIncoming
        | IntegrationKind::GenericJson => {
            return Err("a webhook integration stores a signing secret with url".to_string());
        }
    }
    if token.is_empty() {
        return Err("token is empty".to_string());
    }
    secrets
        .put(&secret_account(id, SecretSlot::AccessToken), token)
        .map_err(|_| "could not store the access token".to_string())?;
    println!("saved token for {id}");
    Ok(())
}

fn connect(config: &Config, secrets: &KeychainTokenStore, id: &str) -> Result<(), String> {
    let kind = kind_of(config, id)?;
    let url = match kind {
        IntegrationKind::OrbitMcp => config
            .integrations()
            .iter()
            .find(|record| record.id == id)
            .and_then(|record| record.endpoint.clone())
            .ok_or_else(|| "missing endpoint".to_string())?,
        IntegrationKind::SlackMcp => SLACK_MCP_URL.to_string(),
        IntegrationKind::SlackIncoming
        | IntegrationKind::TeamsIncoming
        | IntegrationKind::GenericJson => {
            return Err("connect is for an MCP integration".to_string());
        }
    };
    let checked =
        public_https(&url).map_err(|_| "the endpoint must be an https URL".to_string())?;
    let token = secrets
        .get(&secret_account(id, SecretSlot::AccessToken))
        .map_err(|_| "could not read the access token".to_string())?
        .ok_or_else(|| "missing token".to_string())?;
    let headers = vec![
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("Accept".to_string(), "application/json".to_string()),
        ("MCP-Protocol-Version".to_string(), "2025-06-18".to_string()),
    ];
    let init = post_mcp(&url, &headers, &initialize_request(1))?;
    let name = server_name(&init).map_err(|_| "MCP response could not be read".to_string())?;
    let listed = post_mcp(&url, &headers, &tools_list_request(2))?;
    let names = tool_names(&listed).map_err(|_| "MCP response could not be read".to_string())?;
    println!("{} server {name}", checked.host);
    println!("{} tools", names.len());
    for tool in names.into_iter().take(16) {
        if tool.len() <= 64
            && !tool.contains(&token)
            && tool.chars().all(|ch| ch.is_ascii_graphic() && ch != ' ')
        {
            println!("  {tool}");
        }
    }
    Ok(())
}

fn post_mcp(url: &str, headers: &[(String, String)], body: &str) -> Result<String, String> {
    let HttpReply { status, body } = https_post(url, "application/json", headers, body.as_bytes())
        .map_err(|_| "MCP request failed".to_string())?;
    if status != 200 {
        return Err("MCP request failed".to_string());
    }
    String::from_utf8(body).map_err(|_| "MCP response could not be read".to_string())
}

fn kind_of(config: &Config, id: &str) -> Result<IntegrationKind, String> {
    config
        .integrations()
        .iter()
        .find(|record| record.id == id)
        .map(|record| record.kind)
        .ok_or_else(|| format!("unknown integration {id}"))
}

fn load_or_create(path: &Path) -> Result<Config, String> {
    match load_config(path).map_err(|err| err.to_string())? {
        Some(config) => Ok(config),
        None => {
            let config = Config::default_for(&initial_timezone()).map_err(|err| err.to_string())?;
            save_config(path, &config).map_err(|err| err.to_string())?;
            Ok(config)
        }
    }
}

fn initial_timezone() -> String {
    match iana_time_zone::get_timezone() {
        Ok(name) => match parse_timezone(&name) {
            Ok(timezone) => timezone.name().to_string(),
            Err(_) => "UTC".to_string(),
        },
        Err(_) => "UTC".to_string(),
    }
}
