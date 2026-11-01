//! Settings commands for the integration catalog.
//!
//! Secrets are written to the keychain and are not printed.

use crate::deliver::{https_post, HttpReply};
use crate::keychain::KeychainTokenStore;
use crate::oauth::slack_sign_in;
use crate::routing::{self, HttpsTransport};
use chrono::Utc;
use stardust_core::api::{resolve_server_base, Severity, ENV_SERVER_URL};
use stardust_core::config::{load_config, save_config, Config, RoutingUpdate};
use stardust_core::integrations::{secret_account, IntegrationKind, SecretSlot, SecretStore};
use stardust_core::mcp::{
    initialize_request, server_name, tool_names, tools_list_request, SLACK_MCP_URL,
};
use stardust_core::outbound::{public_https, CheckedUrl};
use stardust_core::route::SinkRule;
use stardust_core::schedule::parse_timezone;
use stardust_core::sink::{check_routing_key, check_url, SinkKind};
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
        } => {
            let url = from_stdin_if_dash(url, "URL")?;
            let signing_secret = signing_secret
                .map(|secret| from_stdin_if_dash(secret, "signing secret"))
                .transpose()?;
            let saved = save_url(&mut config, &secrets, &id, &url, signing_secret.as_deref())?;
            save_config(path, &config).map_err(|err| err.to_string())?;
            println!("saved destination for {id} ({})", saved.host);
            if let Some(note) = saved.note {
                println!("{note}");
            }
        }
        IntegrationAction::Token { id, token } => {
            let token = from_stdin_if_dash(token, "token")?;
            save_token(&config, &secrets, &id, &token)?;
        }
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
        IntegrationAction::Key { id, key } => {
            let key = from_stdin_if_dash(key, "routing key")?;
            save_routing_key(&config, &secrets, &id, &key)?;
            println!("saved routing key for {id}");
        }
        IntegrationAction::Route { id, update } => {
            let line = set_route(&mut config, &id, &update)?;
            save_config(path, &config).map_err(|err| err.to_string())?;
            println!("{line}");
            if !config.features().routing {
                println!("routing is off; turn it on with: stardust feature routing on");
            }
        }
        IntegrationAction::Test { id } => {
            let base = resolve_server_base(
                config.server_base_url(),
                std::env::var(ENV_SERVER_URL).ok().as_deref(),
            )
            .map_err(|err| err.to_string())?;
            let host = routing::send_test(
                config.integrations(),
                &secrets,
                &id,
                &base,
                Utc::now(),
                &HttpsTransport::new(),
            )
            .map_err(|err| format!("test to {id} did not go through: {err}"))?;
            println!("test sent to {id} ({host})");
        }
    }
    Ok(())
}

/// `-` reads the value as one line from standard input, so a secret stays
/// out of the process list and the shell history.
fn from_stdin_if_dash(value: String, what: &str) -> Result<String, String> {
    if value != "-" {
        return Ok(value);
    }
    read_line(std::io::stdin().lock(), what)
}

fn read_line(input: impl std::io::BufRead, what: &str) -> Result<String, String> {
    use std::io::BufRead;
    let mut line = String::new();
    input
        .take(8 * 1024)
        .read_line(&mut line)
        .map_err(|_| format!("could not read the {what}"))?;
    let line = line.trim();
    if line.is_empty() {
        return Err(format!("the {what} is empty"));
    }
    Ok(line.to_string())
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

/// What [`save_url`] stored, for the confirmation line.
struct SavedUrl {
    host: String,
    /// Shown after the confirmation when routing will not use this URL.
    note: Option<&'static str>,
}

/// Store a destination in the keychain and its host in `config`. The caller
/// saves the config file.
fn save_url(
    config: &mut Config,
    secrets: &dyn SecretStore,
    id: &str,
    url: &str,
    signing_secret: Option<&str>,
) -> Result<SavedUrl, String> {
    let routing = config.features().routing;
    let checked = match public_https(url) {
        Ok(checked) => checked,
        // A Teams Workflows URL carries its signature in the query, which
        // only the routing sink accepts.
        Err(_) if routing && kind_of(config, id).ok() == Some(IntegrationKind::TeamsIncoming) => {
            check_url(SinkKind::Teams, url)
                .map_err(|_| "the endpoint must be an https URL".to_string())?
        }
        Err(_) => return Err("the endpoint must be an https URL".to_string()),
    };
    let kind = kind_of(config, id)?;
    let mut note = None;
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
            if routing {
                note = routing_note(kind, &checked);
            }
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
        IntegrationKind::DiscordWebhook | IntegrationKind::PagerDuty => {
            let (sink, refused) = if kind == IntegrationKind::DiscordWebhook {
                (SinkKind::Discord, "the URL must be a Discord webhook URL")
            } else {
                (
                    SinkKind::PagerDuty,
                    "the URL must be the PagerDuty Events API v2 enqueue URL",
                )
            };
            if signing_secret.is_some() {
                return Err("this integration does not take a signing secret".to_string());
            }
            let checked = check_url(sink, &checked.url).map_err(|_| refused.to_string())?;
            secrets
                .put(&secret_account(id, SecretSlot::Url), &checked.url)
                .map_err(|_| "could not store the destination".to_string())?;
            config
                .set_integration_host(id, &checked.host)
                .map_err(|err| err.to_string())?;
            return Ok(SavedUrl {
                host: checked.host,
                note: None,
            });
        }
    }
    Ok(SavedUrl {
        host: checked.host,
        note,
    })
}

/// With routing on, Slack and Teams take one message per alert, and that
/// sink posts only to the vendor's webhook hosts.
fn routing_note(kind: IntegrationKind, checked: &CheckedUrl) -> Option<&'static str> {
    let sink = SinkKind::of(kind)?;
    if check_url(sink, &checked.url).is_ok() {
        return None;
    }
    Some(match sink {
        SinkKind::Teams => "note: routing posts to Teams only through a Workflows webhook URL",
        _ => "note: routing posts to Slack only through a hooks.slack.com URL",
    })
}

fn save_routing_key(
    config: &Config,
    secrets: &dyn SecretStore,
    id: &str,
    key: &str,
) -> Result<(), String> {
    if kind_of(config, id)? != IntegrationKind::PagerDuty {
        return Err("a routing key is for a PagerDuty integration".to_string());
    }
    let key = check_routing_key(key)
        .map_err(|_| "a PagerDuty routing key is 32 letters and digits".to_string())?;
    secrets
        .put(&secret_account(id, SecretSlot::RoutingKey), key.expose())
        .map_err(|_| "could not store the routing key".to_string())
}

/// Change the routing rules of a sink integration. Returns the rules now in
/// effect, defaults included.
fn set_route(config: &mut Config, id: &str, update: &RoutingUpdate) -> Result<String, String> {
    if SinkKind::of(kind_of(config, id)?).is_none() {
        return Err(
            "routing rules are for a Slack, Teams, Discord, or PagerDuty integration".to_string(),
        );
    }
    config
        .set_integration_routing(id, update)
        .map_err(|err| err.to_string())?;
    let rule = config
        .integrations()
        .iter()
        .find(|record| record.id == id)
        .and_then(SinkRule::for_record)
        .ok_or_else(|| format!("unknown integration {id}"))?;
    let severity = match rule.min_severity {
        Severity::Low => "low",
        Severity::Medium => "medium",
        Severity::High => "high",
    };
    let limit = rule
        .max_per_hour
        .map(|limit| format!("at most {limit} an hour"))
        .unwrap_or_else(|| "no hourly limit".to_string());
    let bypass = if rule.quiet_high_bypass { "on" } else { "off" };
    Ok(format!(
        "{id}: min severity {severity}, {limit}, high alerts during quiet hours {bypass}"
    ))
}

fn save_token(
    config: &Config,
    secrets: &dyn SecretStore,
    id: &str,
    token: &str,
) -> Result<(), String> {
    let kind = kind_of(config, id)?;
    match kind {
        IntegrationKind::OrbitMcp | IntegrationKind::SlackMcp => {}
        IntegrationKind::SlackIncoming
        | IntegrationKind::TeamsIncoming
        | IntegrationKind::GenericJson
        | IntegrationKind::DiscordWebhook
        | IntegrationKind::PagerDuty => {
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

fn connect(config: &Config, secrets: &dyn SecretStore, id: &str) -> Result<(), String> {
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
        | IntegrationKind::GenericJson
        | IntegrationKind::DiscordWebhook
        | IntegrationKind::PagerDuty => {
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

#[cfg(test)]
mod tests {
    use super::{save_routing_key, save_url, set_route};
    use stardust_core::api::Severity;
    use stardust_core::config::{Config, Feature, FieldUpdate, RoutingUpdate};
    use stardust_core::integrations::{
        secret_account, IntegrationKind, MemorySecretStore, SecretSlot, SecretStore,
    };

    const DISCORD: &str = "https://discord.com/api/webhooks/123456/example-token_1";
    const TEAMS_WORKFLOW: &str = "https://prod-00.westus.logic.azure.com:443/workflows/abc/triggers/manual/paths/invoke?api-version=2016-06-01&sp=%2Ftriggers%2Fmanual%2Frun&sv=1.0&sig=example";
    const ROUTING_KEY: &str = "exampleroutingkey000000000000000";

    fn config_with(kind: IntegrationKind, id: &str) -> Config {
        let mut config = Config::default_for("UTC").unwrap();
        config.add_integration(kind, id).unwrap();
        config
    }

    fn stored(secrets: &MemorySecretStore, id: &str, slot: SecretSlot) -> Option<String> {
        secrets.get(&secret_account(id, slot)).unwrap()
    }

    #[test]
    fn a_discord_webhook_is_stored_in_the_keychain_and_other_hosts_are_refused() {
        let mut config = config_with(IntegrationKind::DiscordWebhook, "alerts");
        let secrets = MemorySecretStore::new();
        let saved = save_url(&mut config, &secrets, "alerts", DISCORD, None).unwrap();
        assert_eq!(saved.host, "discord.com");
        assert_eq!(
            stored(&secrets, "alerts", SecretSlot::Url).as_deref(),
            Some(DISCORD)
        );
        assert_eq!(
            config.integrations()[0].host.as_deref(),
            Some("discord.com")
        );
        let text = config.to_json().unwrap();
        assert!(!text.contains("example-token"), "{text}");
        for refused in [
            "https://hooks.example/api/webhooks/1/token",
            "https://discord.com/channels/1/2",
            "http://discord.com/api/webhooks/1/token",
        ] {
            let err = save_url(&mut config, &secrets, "alerts", refused, None)
                .err()
                .unwrap();
            assert!(!err.contains("token"), "{err}");
        }
        assert!(save_url(&mut config, &secrets, "alerts", DISCORD, Some("secret")).is_err());
    }

    #[test]
    fn pagerduty_takes_an_events_endpoint_and_a_well_formed_routing_key() {
        let mut config = config_with(IntegrationKind::PagerDuty, "pd");
        let secrets = MemorySecretStore::new();
        let saved = save_url(
            &mut config,
            &secrets,
            "pd",
            "https://events.eu.pagerduty.com/v2/enqueue",
            None,
        )
        .unwrap();
        assert_eq!(saved.host, "events.eu.pagerduty.com");
        assert!(save_url(
            &mut config,
            &secrets,
            "pd",
            "https://hooks.example/v2/enqueue",
            None
        )
        .is_err());
        save_routing_key(&config, &secrets, "pd", ROUTING_KEY).unwrap();
        assert_eq!(
            stored(&secrets, "pd", SecretSlot::RoutingKey).as_deref(),
            Some(ROUTING_KEY)
        );
        let short = save_routing_key(&config, &secrets, "pd", "short-key").unwrap_err();
        assert!(!short.contains("short-key"), "{short}");
        let slack = config_with(IntegrationKind::SlackIncoming, "ops");
        assert!(save_routing_key(&slack, &secrets, "ops", ROUTING_KEY).is_err());
        assert!(!config.to_json().unwrap().contains(ROUTING_KEY));
    }

    #[test]
    fn a_teams_workflows_url_is_accepted_only_with_routing_on() {
        let mut config = config_with(IntegrationKind::TeamsIncoming, "team");
        let secrets = MemorySecretStore::new();
        assert_eq!(
            save_url(&mut config, &secrets, "team", TEAMS_WORKFLOW, None)
                .err()
                .as_deref(),
            Some("the endpoint must be an https URL")
        );
        assert_eq!(stored(&secrets, "team", SecretSlot::Url), None);
        config.set_feature(Feature::Routing, true);
        let saved = save_url(&mut config, &secrets, "team", TEAMS_WORKFLOW, None).unwrap();
        assert_eq!(saved.host, "prod-00.westus.logic.azure.com");
        assert_eq!(saved.note, None);
        assert_eq!(
            stored(&secrets, "team", SecretSlot::Url).as_deref(),
            Some(TEAMS_WORKFLOW)
        );
        let plain = save_url(
            &mut config,
            &secrets,
            "team",
            "https://hooks.example/inbound",
            None,
        )
        .unwrap();
        assert!(plain.note.is_some(), "routing will not use this one");
        let mut json = config_with(IntegrationKind::GenericJson, "json");
        json.set_feature(Feature::Routing, true);
        assert!(save_url(&mut json, &secrets, "json", TEAMS_WORKFLOW, None).is_err());
    }

    #[test]
    fn a_dash_reads_one_secret_line_from_standard_input() {
        use super::read_line;
        assert_eq!(
            read_line(
                &b"exampleroutingkey000000000000000\nnext\n"[..],
                "routing key"
            )
            .unwrap(),
            "exampleroutingkey000000000000000"
        );
        assert_eq!(read_line(&b"  padded \r\n"[..], "URL").unwrap(), "padded");
        assert_eq!(
            read_line(&b"\n"[..], "URL").unwrap_err(),
            "the URL is empty"
        );
        assert!(read_line(&b""[..], "token").is_err());
        let long = vec![b'a'; 20 * 1024];
        assert_eq!(read_line(&long[..], "token").unwrap().len(), 8 * 1024);
        assert_eq!(
            super::from_stdin_if_dash("https://hooks.example/x".to_string(), "URL").unwrap(),
            "https://hooks.example/x"
        );
    }

    #[test]
    fn routing_rules_apply_only_to_sink_integrations_and_show_their_defaults() {
        let mut config = config_with(IntegrationKind::PagerDuty, "pd");
        assert_eq!(
            set_route(&mut config, "pd", &RoutingUpdate::default()).unwrap(),
            "pd: min severity high, at most 30 an hour, high alerts during quiet hours on"
        );
        let line = set_route(
            &mut config,
            "pd",
            &RoutingUpdate {
                min_severity: Some(FieldUpdate::Set(Severity::Medium)),
                max_per_hour: Some(FieldUpdate::Set(4)),
                quiet_high_bypass: Some(FieldUpdate::Set(false)),
            },
        )
        .unwrap();
        assert_eq!(
            line,
            "pd: min severity medium, at most 4 an hour, high alerts during quiet hours off"
        );
        assert!(set_route(
            &mut config,
            "pd",
            &RoutingUpdate {
                max_per_hour: Some(FieldUpdate::Set(0)),
                ..RoutingUpdate::default()
            }
        )
        .is_err());
        assert_eq!(config.integrations()[0].max_per_hour, Some(4));
        let mut json = config_with(IntegrationKind::GenericJson, "json");
        assert!(set_route(&mut json, "json", &RoutingUpdate::default()).is_err());
        assert!(set_route(&mut json, "missing", &RoutingUpdate::default()).is_err());
    }
}
