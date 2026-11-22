//! Integration catalog. Every entry is off until the user enables it.
//!
//! URLs that contain a credential, signing secrets, and access tokens are
//! not fields on these records. Callers keep them in the OS keychain.

use crate::api::Severity;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const MAX_INTEGRATIONS: usize = 16;
/// Upper bound for a per-sink `max_per_hour`.
pub const MAX_PER_HOUR_LIMIT: u32 = 3_600;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationKind {
    SlackIncoming,
    TeamsIncoming,
    GenericJson,
    OrbitMcp,
    SlackMcp,
    DiscordWebhook,
    PagerDuty,
}

impl IntegrationKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "slack" | "slack_incoming" => Some(Self::SlackIncoming),
            "teams" | "teams_incoming" => Some(Self::TeamsIncoming),
            "json" | "generic_json" => Some(Self::GenericJson),
            "orbit-mcp" | "orbit_mcp" => Some(Self::OrbitMcp),
            "slack-mcp" | "slack_mcp" => Some(Self::SlackMcp),
            "discord" | "discord_webhook" => Some(Self::DiscordWebhook),
            "pagerduty" | "pager_duty" => Some(Self::PagerDuty),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SlackIncoming => "slack_incoming",
            Self::TeamsIncoming => "teams_incoming",
            Self::GenericJson => "generic_json",
            Self::OrbitMcp => "orbit_mcp",
            Self::SlackMcp => "slack_mcp",
            Self::DiscordWebhook => "discord_webhook",
            Self::PagerDuty => "pager_duty",
        }
    }

    /// Kinds the check-schedule summary posts go to. Routing turned on
    /// moves Slack and Teams to one message per alert instead.
    pub fn posts_webhook(self) -> bool {
        matches!(
            self,
            Self::SlackIncoming | Self::TeamsIncoming | Self::GenericJson
        )
    }

    /// Kinds that take one message per alert when routing is on.
    pub fn routes_alerts(self) -> bool {
        matches!(
            self,
            Self::SlackIncoming | Self::TeamsIncoming | Self::DiscordWebhook | Self::PagerDuty
        )
    }

    /// Kinds an alert can reach at all. The MCP kinds only list tools when
    /// asked, so no alert ever goes to them.
    pub fn delivers_alerts(self) -> bool {
        !matches!(self, Self::OrbitMcp | Self::SlackMcp)
    }

    /// The name people know this kind by, from the catalog.
    pub fn label(self) -> &'static str {
        catalog()
            .iter()
            .find(|entry| entry.kind == self)
            .map_or("Integration", |entry| entry.name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogEntry {
    pub kind: IntegrationKind,
    pub name: &'static str,
    pub summary: &'static str,
}

pub fn catalog() -> &'static [CatalogEntry] {
    &[
        CatalogEntry {
            kind: IntegrationKind::SlackIncoming,
            name: "Slack",
            summary: "Incoming webhook posts on the check schedule, and for a high-severity item",
        },
        CatalogEntry {
            kind: IntegrationKind::TeamsIncoming,
            name: "Teams",
            summary: "Incoming webhook posts on the check schedule, and for a high-severity item",
        },
        CatalogEntry {
            kind: IntegrationKind::GenericJson,
            name: "JSON webhook",
            summary: "Signed JSON posts on the check schedule, and for a high-severity item",
        },
        CatalogEntry {
            kind: IntegrationKind::OrbitMcp,
            name: "ORBIT MCP",
            summary: "Optional MCP client. Off until an https endpoint is saved",
        },
        CatalogEntry {
            kind: IntegrationKind::SlackMcp,
            name: "Slack MCP",
            summary: "Optional Slack MCP sign-in. Off until a token is saved",
        },
        CatalogEntry {
            kind: IntegrationKind::DiscordWebhook,
            name: "Discord",
            summary: "Webhook embed for each new alert. Needs routing on",
        },
        CatalogEntry {
            kind: IntegrationKind::PagerDuty,
            name: "PagerDuty",
            summary: "Events API v2 trigger for each new alert. Needs routing on",
        },
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretSlot {
    Url,
    SigningSecret,
    AccessToken,
    RefreshToken,
    PkceVerifier,
    /// PagerDuty Events API v2 integration key.
    RoutingKey,
}

impl SecretSlot {
    fn name(self) -> &'static str {
        match self {
            Self::Url => "url",
            Self::SigningSecret => "signing-secret",
            Self::AccessToken => "access-token",
            Self::RefreshToken => "refresh-token",
            Self::PkceVerifier => "pkce-verifier",
            Self::RoutingKey => "routing-key",
        }
    }
}

pub fn secret_account(id: &str, slot: SecretSlot) -> String {
    format!("integration/{id}/{}", slot.name())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationRecord {
    pub id: String,
    pub kind: IntegrationKind,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Routing only: alerts below this severity are not sent here. Unset
    /// means high for PagerDuty and low for the rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_severity: Option<Severity>,
    /// Routing only: at most this many messages an hour. Unset means 30.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_per_hour: Option<u32>,
    /// Routing only: a high-severity alert is still sent during quiet hours.
    /// Unset means yes, the same rule the bubble follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_high_bypass: Option<bool>,
    /// Integrations panel only: the user accepted what this integration
    /// sends, to this host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent: Option<crate::consent::Consent>,
}

impl IntegrationRecord {
    pub fn new(kind: IntegrationKind, id: &str) -> Result<Self, IntegrationError> {
        Ok(Self {
            id: parse_id(id)?,
            kind,
            enabled: false,
            host: None,
            endpoint: None,
            client_id: None,
            min_severity: None,
            max_per_hour: None,
            quiet_high_bypass: None,
            consent: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntegrationError {
    #[error("could not read the integration id")]
    Id,
    #[error("unknown integration kind")]
    Kind,
    #[error("that integration id is already saved")]
    Duplicate,
    #[error("unknown integration {0}")]
    Missing(String),
    #[error("too many integrations")]
    Full,
    #[error("the endpoint must be an https URL")]
    Endpoint,
    #[error("max per hour must be 1 through 3600")]
    Rate,
    #[error("save a destination first with: stardust integration url ID URL")]
    NoDestination,
    #[error("the destination changed; review it again before turning it on")]
    HostChanged,
}

pub fn parse_id(value: &str) -> Result<String, IntegrationError> {
    let id = value.trim();
    if (1..=32).contains(&id.len())
        && id
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        && !id.starts_with('-')
        && !id.ends_with('-')
    {
        Ok(id.to_string())
    } else {
        Err(IntegrationError::Id)
    }
}

pub fn integrations_label(records: &[IntegrationRecord]) -> String {
    let on = records.iter().filter(|record| record.enabled).count();
    if on == 0 {
        "Integrations off".to_string()
    } else if on == 1 {
        "1 integration on".to_string()
    } else {
        format!("{on} integrations on")
    }
}

pub trait SecretStore {
    fn put(&self, account: &str, secret: &str) -> Result<(), SecretError>;
    fn get(&self, account: &str) -> Result<Option<String>, SecretError>;
    fn delete(&self, account: &str) -> Result<(), SecretError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("secret is empty")]
    Empty,
    #[error("keychain request failed")]
    Keychain,
}

impl fmt::Debug for dyn SecretStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretStore")
    }
}

#[derive(Default)]
pub struct MemorySecretStore {
    inner: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecretStore {
    fn put(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        if secret.is_empty() {
            return Err(SecretError::Empty);
        }
        self.inner
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .insert(account.to_string(), secret.to_string());
        Ok(())
    }

    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        Ok(self
            .inner
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .get(account)
            .cloned())
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        self.inner
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        catalog, integrations_label, secret_account, IntegrationKind, IntegrationRecord, SecretSlot,
    };

    #[test]
    fn catalog_entries_start_as_kinds_the_user_turns_on() {
        assert!(catalog()
            .iter()
            .all(|entry| !entry.summary.to_ascii_lowercase().contains("chat")));
        assert_eq!(catalog().len(), 7);
        let record = IntegrationRecord::new(IntegrationKind::SlackIncoming, "ops").unwrap();
        assert!(!record.enabled);
        assert_eq!(integrations_label(&[record]), "Integrations off");
        assert_eq!(
            secret_account("ops", SecretSlot::Url),
            "integration/ops/url"
        );
        assert_eq!(
            secret_account("pd", SecretSlot::RoutingKey),
            "integration/pd/routing-key"
        );
    }

    #[test]
    fn new_sink_kinds_parse_and_stay_out_of_the_summary_posts() {
        assert_eq!(
            IntegrationKind::parse("discord"),
            Some(IntegrationKind::DiscordWebhook)
        );
        assert_eq!(
            IntegrationKind::parse("PagerDuty"),
            Some(IntegrationKind::PagerDuty)
        );
        for kind in [IntegrationKind::DiscordWebhook, IntegrationKind::PagerDuty] {
            assert!(!kind.posts_webhook());
            assert!(kind.routes_alerts());
            assert_eq!(IntegrationKind::parse(kind.as_str()), Some(kind));
        }
        assert!(!IntegrationKind::GenericJson.routes_alerts());
        assert!(IntegrationKind::SlackIncoming.routes_alerts());
        let record = IntegrationRecord::new(IntegrationKind::PagerDuty, "pd").unwrap();
        let text = serde_json::to_string(&record).unwrap();
        assert_eq!(text, r#"{"id":"pd","kind":"pager_duty","enabled":false}"#);
    }
}
