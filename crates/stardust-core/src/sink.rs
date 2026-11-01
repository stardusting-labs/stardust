//! One message per alert to Slack, Teams, Discord, or PagerDuty.
//!
//! Rendering, destination checks, and status handling live here. Sockets do
//! not: the app passes a [`Transport`] that pins the resolved public address.
//! A sink is used only when routing is on and its integration is enabled.
//! Destination URLs and routing keys come from the keychain and are redacted
//! in Debug output.

use crate::api::{https_origin, workspace_url, AlertItem, Severity};
use crate::integrations::IntegrationKind;
use crate::outbound::{public_https, public_https_with_query, CheckedUrl};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use std::fmt;

pub const PAGERDUTY_ENQUEUE_US: &str = "https://events.pagerduty.com/v2/enqueue";
pub const PAGERDUTY_ENQUEUE_EU: &str = "https://events.eu.pagerduty.com/v2/enqueue";
/// Id of the fixed message that "Send test" posts.
pub const TEST_ALERT_ID: &str = "stardust-test";

const OPEN_LABEL: &str = "Open in ORBIT";
const SLACK_HOSTS: [&str; 2] = ["hooks.slack.com", "hooks.slack-gov.com"];
const DISCORD_HOSTS: [&str; 4] = [
    "discord.com",
    "discordapp.com",
    "ptb.discord.com",
    "canary.discord.com",
];
const TEAMS_HOST_SUFFIXES: [&str; 2] = [".logic.azure.com", ".powerplatform.com"];

/// One alert as a sink sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alert {
    pub id: String,
    pub severity: Severity,
    pub title: String,
    pub created_at: DateTime<Utc>,
    /// An https page on the configured ORBIT host, or nothing.
    pub link: Option<String>,
}

impl Alert {
    /// The link is the item's workspace page on the configured https host,
    /// then that host's origin. Any other host gives no link.
    pub fn from_item(item: &AlertItem, server_base: &str) -> Self {
        let severity = if item.shows_immediately() {
            Severity::High
        } else {
            item.severity
        };
        Self {
            id: clip_id(&item.id),
            severity,
            title: clip_text(&item.title, 280),
            created_at: item.created_at,
            link: workspace_url(server_base, &item.workspace_path)
                .or_else(|| https_origin(server_base)),
        }
    }

    /// The fixed message "Send test" posts.
    pub fn sample(server_base: &str, now: DateTime<Utc>) -> Self {
        Self {
            id: TEST_ALERT_ID.to_string(),
            severity: Severity::Low,
            title: "Stardust test message. Nothing needs attention.".to_string(),
            created_at: now,
            link: https_origin(server_base),
        }
    }

    pub fn is_high(&self) -> bool {
        self.severity == Severity::High
    }

    fn is_test(&self) -> bool {
        self.id == TEST_ALERT_ID
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SinkError {
    #[error("the destination is not saved")]
    Missing,
    #[error("the destination was refused")]
    Refused,
    #[error("the destination could not be reached")]
    Unreachable,
    #[error("the destination is busy (status {0})")]
    Busy(u16),
    #[error("the destination did not take the message (status {0})")]
    Rejected(u16),
    /// The vendor host resolved to a loopback, private, or other non-public
    /// address, as behind a DNS blocker or a split resolver.
    #[error("the destination resolved to an address that is not public")]
    Blocked,
}

impl SinkError {
    /// Worth sending again from a later check.
    pub fn retry(&self) -> bool {
        matches!(self, Self::Unreachable | Self::Busy(_))
    }

    /// The destination cannot take anything now, whatever the alert: it is
    /// gone or no longer lets Stardust post (401, 403, 404, 410), it
    /// redirects (a captive portal, say), or its name resolves to a private
    /// address. The alert waits for a later check rather than being dropped.
    pub fn gone(&self) -> bool {
        matches!(
            self,
            Self::Rejected(300..=399 | 401 | 403 | 404 | 410) | Self::Blocked
        )
    }
}

/// Somewhere an alert can go.
pub trait Sink {
    fn deliver(&self, alert: &Alert) -> Result<(), SinkError>;
}

/// POSTs a JSON body and returns the HTTP status. An error means the request
/// was not sent or no answer came back.
pub trait Transport {
    fn post_json(&self, url: &CheckedUrl, body: &[u8]) -> Result<u16, SinkError>;
}

/// 2xx is delivered. 408, 425, 429, and 5xx are tried again later. Other
/// statuses, redirects included, mean the destination will not take it.
pub fn status_result(status: u16) -> Result<(), SinkError> {
    match status {
        200..=299 => Ok(()),
        408 | 425 | 429 | 500..=599 => Err(SinkError::Busy(status)),
        _ => Err(SinkError::Rejected(status)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SinkKind {
    Slack,
    Teams,
    Discord,
    PagerDuty,
}

impl SinkKind {
    pub fn of(kind: IntegrationKind) -> Option<Self> {
        match kind {
            IntegrationKind::SlackIncoming => Some(Self::Slack),
            IntegrationKind::TeamsIncoming => Some(Self::Teams),
            IntegrationKind::DiscordWebhook => Some(Self::Discord),
            IntegrationKind::PagerDuty => Some(Self::PagerDuty),
            IntegrationKind::GenericJson
            | IntegrationKind::OrbitMcp
            | IntegrationKind::SlackMcp => None,
        }
    }

    /// PagerDuty pages people, so it starts at high. The rest take all.
    pub fn default_min_severity(self) -> Severity {
        match self {
            Self::PagerDuty => Severity::High,
            Self::Slack | Self::Teams | Self::Discord => Severity::Low,
        }
    }
}

/// A keychain value. Debug output never shows it.
#[derive(Clone, PartialEq, Eq)]
pub struct SinkSecret(String);

impl SinkSecret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SinkSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

/// Check a destination URL for `kind`.
///
/// Slack and Discord must be on their webhook hosts, Teams on a Power
/// Automate Workflows host (the query holds its signature), and PagerDuty
/// one of the two Events API v2 endpoints. The port is 443, written or not.
pub fn check_url(kind: SinkKind, value: &str) -> Result<CheckedUrl, SinkError> {
    let mut checked = match kind {
        SinkKind::Teams => public_https_with_query(value),
        SinkKind::Slack | SinkKind::Discord | SinkKind::PagerDuty => public_https(value),
    }
    .map_err(|_| SinkError::Refused)?;
    if !default_port(&checked.url) {
        return Err(SinkError::Refused);
    }
    if kind == SinkKind::PagerDuty {
        // A written :443 or a trailing slash would also end up in the
        // change-event URL.
        checked.url = checked
            .url
            .replacen(".pagerduty.com:443/", ".pagerduty.com/", 1)
            .trim_end_matches('/')
            .to_string();
    }
    let path = path_of(&checked.url);
    let allowed = match kind {
        SinkKind::Slack => SLACK_HOSTS.contains(&checked.host.as_str()) && path.len() > 1,
        SinkKind::Teams => {
            TEAMS_HOST_SUFFIXES
                .iter()
                .any(|suffix| checked.host.ends_with(suffix))
                && checked.url.contains('?')
        }
        SinkKind::Discord => {
            DISCORD_HOSTS.contains(&checked.host.as_str()) && discord_webhook_path(path)
        }
        SinkKind::PagerDuty => {
            checked.url == PAGERDUTY_ENQUEUE_US || checked.url == PAGERDUTY_ENQUEUE_EU
        }
    };
    if allowed {
        Ok(checked)
    } else {
        Err(SinkError::Refused)
    }
}

/// A PagerDuty integration key: 32 letters and digits.
pub fn check_routing_key(value: &str) -> Result<SinkSecret, SinkError> {
    let key = value.trim();
    if key.len() == 32 && key.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        Ok(SinkSecret(key.to_string()))
    } else {
        Err(SinkError::Refused)
    }
}

/// No port, or `:443`. The vendor hosts are names, so the first `:` in the
/// authority starts the port.
fn default_port(url: &str) -> bool {
    let rest = url.strip_prefix("https://").unwrap_or(url);
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    match authority.split_once(':') {
        None => true,
        Some((_, port)) => port == "443",
    }
}

fn path_of(url: &str) -> &str {
    let rest = url.strip_prefix("https://").unwrap_or(url);
    let rest = rest.split('?').next().unwrap_or("");
    rest.find('/').map(|at| &rest[at..]).unwrap_or("")
}

fn discord_webhook_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/api/") else {
        return false;
    };
    let rest = match rest.split_once('/') {
        Some((version, after))
            if version.len() > 1
                && version.starts_with('v')
                && version[1..].bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            after
        }
        _ => rest,
    };
    let mut parts = rest.split('/');
    parts.next() == Some("webhooks")
        && parts
            .next()
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        && parts.next().is_some_and(|token| {
            !token.is_empty()
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        && parts.next().is_none()
}

/// A Slack, Teams, Discord, or PagerDuty destination.
pub struct WebhookSink<'a> {
    id: String,
    kind: SinkKind,
    url: CheckedUrl,
    routing_key: Option<SinkSecret>,
    source: String,
    transport: &'a dyn Transport,
}

impl fmt::Debug for WebhookSink<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebhookSink")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("host", &self.url.host)
            .field("url", &"<redacted>")
            .field("routing_key", &self.routing_key)
            .finish()
    }
}

impl<'a> WebhookSink<'a> {
    /// `url` is the saved destination. PagerDuty may leave it out to use the
    /// US endpoint and must have a routing key.
    pub fn new(
        id: &str,
        kind: SinkKind,
        url: Option<&str>,
        routing_key: Option<&str>,
        server_base: &str,
        transport: &'a dyn Transport,
    ) -> Result<Self, SinkError> {
        let url = match (kind, url) {
            (_, Some(url)) => check_url(kind, url)?,
            (SinkKind::PagerDuty, None) => check_url(kind, PAGERDUTY_ENQUEUE_US)?,
            (_, None) => return Err(SinkError::Missing),
        };
        let routing_key = match kind {
            SinkKind::PagerDuty => Some(check_routing_key(routing_key.ok_or(SinkError::Missing)?)?),
            SinkKind::Slack | SinkKind::Teams | SinkKind::Discord => None,
        };
        Ok(Self {
            id: id.to_string(),
            kind,
            url,
            routing_key,
            source: source_name(server_base),
            transport,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn host(&self) -> &str {
        &self.url.host
    }

    /// Post the fixed test message. PagerDuty gets a change event, which
    /// shows on the service timeline and does not open an incident.
    pub fn send_test(&self, server_base: &str, now: DateTime<Utc>) -> Result<(), SinkError> {
        let sample = Alert::sample(server_base, now);
        match self.kind {
            SinkKind::PagerDuty => {
                let change = CheckedUrl {
                    url: self
                        .url
                        .url
                        .replacen("/v2/enqueue", "/v2/change/enqueue", 1),
                    host: self.url.host.clone(),
                };
                let body = pagerduty_change(&sample, &self.source, self.key());
                self.post(&change, &body)
            }
            SinkKind::Slack | SinkKind::Teams | SinkKind::Discord => self.deliver(&sample),
        }
    }

    fn key(&self) -> &str {
        self.routing_key
            .as_ref()
            .map(SinkSecret::expose)
            .unwrap_or("")
    }

    fn post(&self, url: &CheckedUrl, body: &Value) -> Result<(), SinkError> {
        let bytes = serde_json::to_vec(body).map_err(|_| SinkError::Refused)?;
        status_result(self.transport.post_json(url, &bytes)?)
    }
}

impl Sink for WebhookSink<'_> {
    fn deliver(&self, alert: &Alert) -> Result<(), SinkError> {
        let body = render(self.kind, alert, &self.source, self.key());
        self.post(&self.url, &body)
    }
}

/// The JSON body for one alert.
pub fn render(kind: SinkKind, alert: &Alert, source: &str, routing_key: &str) -> Value {
    match kind {
        SinkKind::Slack => slack(alert),
        SinkKind::Teams => teams(alert),
        SinkKind::Discord => discord(alert),
        SinkKind::PagerDuty => pagerduty_trigger(alert, source, routing_key),
    }
}

/// Slack incoming webhook with Block Kit. `text` is the notification
/// fallback. Every mrkdwn string is escaped, so a title cannot add a
/// mention or a link.
fn slack(alert: &Alert) -> Value {
    let title = escape_slack(&alert.title);
    let mut blocks = vec![
        json!({ "type": "header", "text": { "type": "plain_text", "text": heading(alert), "emoji": false } }),
        json!({ "type": "section", "text": { "type": "mrkdwn", "text": title } }),
        json!({ "type": "context", "elements": [{ "type": "mrkdwn", "text": format!("Severity {} · seen {}", severity_word(alert.severity), stamp(alert.created_at)) }] }),
    ];
    if let Some(link) = &alert.link {
        blocks.push(json!({
            "type": "actions",
            "elements": [{ "type": "button", "text": { "type": "plain_text", "text": OPEN_LABEL, "emoji": false }, "url": link }]
        }));
    }
    json!({
        "text": format!("{}: {title}", heading(alert)),
        "blocks": blocks,
    })
}

/// Teams Workflows webhook: a message with one Adaptive Card 1.4. A
/// TextBlock reads markdown links, so `](` in a title is split apart.
fn teams(alert: &Alert) -> Value {
    let mut card = json!({
        "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
        "type": "AdaptiveCard",
        "version": "1.4",
        "body": [
            { "type": "TextBlock", "text": heading(alert), "weight": "Bolder", "size": "Medium", "wrap": true },
            { "type": "TextBlock", "text": alert.title.replace("](", "] ("), "wrap": true },
            { "type": "FactSet", "facts": [
                { "title": "Severity", "value": severity_word(alert.severity) },
                { "title": "Seen", "value": stamp(alert.created_at) }
            ] }
        ]
    });
    if let Some(link) = &alert.link {
        card["actions"] = json!([{ "type": "Action.OpenUrl", "title": OPEN_LABEL, "url": link }]);
    }
    json!({
        "type": "message",
        "attachments": [{
            "contentType": "application/vnd.microsoft.card.adaptive",
            "contentUrl": null,
            "content": card
        }]
    })
}

/// Discord webhook with one embed. Mentions are turned off and markdown in
/// the title is escaped.
fn discord(alert: &Alert) -> Value {
    let mut embed = Map::new();
    embed.insert("title".to_string(), json!(heading(alert)));
    embed.insert(
        "description".to_string(),
        json!(escape_discord(&alert.title)),
    );
    if let Some(link) = &alert.link {
        embed.insert("url".to_string(), json!(link));
    }
    embed.insert("color".to_string(), json!(severity_color(alert.severity)));
    embed.insert("timestamp".to_string(), json!(stamp(alert.created_at)));
    embed.insert(
        "fields".to_string(),
        json!([{ "name": "Severity", "value": severity_word(alert.severity), "inline": true }]),
    );
    embed.insert("footer".to_string(), json!({ "text": "Stardust" }));
    json!({
        "username": "Stardust",
        "allowed_mentions": { "parse": [] },
        "embeds": [Value::Object(embed)]
    })
}

/// PagerDuty Events API v2 trigger. The dedup key follows the alert id, so a
/// repeat from another computer joins the same incident.
fn pagerduty_trigger(alert: &Alert, source: &str, routing_key: &str) -> Value {
    let mut body = json!({
        "routing_key": routing_key,
        "event_action": "trigger",
        "dedup_key": format!("orbit-{}", alert.id),
        "payload": {
            "summary": alert.title,
            "source": source,
            "severity": pagerduty_severity(alert.severity),
            "timestamp": stamp(alert.created_at),
            "component": "ORBIT",
            "class": "finding",
            "custom_details": { "alert_id": alert.id, "severity": severity_key(alert.severity) }
        },
        "client": "Stardust"
    });
    if let Some(link) = &alert.link {
        body["client_url"] = json!(link);
        body["links"] = json!([{ "href": link, "text": OPEN_LABEL }]);
    }
    body
}

/// PagerDuty change event for "Send test".
fn pagerduty_change(alert: &Alert, source: &str, routing_key: &str) -> Value {
    let mut body = json!({
        "routing_key": routing_key,
        "payload": {
            "summary": alert.title,
            "timestamp": stamp(alert.created_at),
            "source": source,
            "custom_details": { "sent_by": "Stardust" }
        }
    });
    if let Some(link) = &alert.link {
        body["links"] = json!([{ "href": link, "text": OPEN_LABEL }]);
    }
    body
}

fn heading(alert: &Alert) -> &'static str {
    if alert.is_test() {
        return "Stardust test message";
    }
    match alert.severity {
        Severity::High => "ORBIT high-severity item",
        Severity::Medium => "ORBIT medium-severity item",
        Severity::Low => "ORBIT low-severity item",
    }
}

fn severity_word(severity: Severity) -> &'static str {
    match severity {
        Severity::High => "High",
        Severity::Medium => "Medium",
        Severity::Low => "Low",
    }
}

fn severity_key(severity: Severity) -> &'static str {
    match severity {
        Severity::High => "high",
        Severity::Medium => "medium",
        Severity::Low => "low",
    }
}

fn pagerduty_severity(severity: Severity) -> &'static str {
    match severity {
        Severity::High => "critical",
        Severity::Medium => "warning",
        Severity::Low => "info",
    }
}

fn severity_color(severity: Severity) -> u32 {
    match severity {
        Severity::High => 0xC6_28_28,
        Severity::Medium => 0xF9_A8_25,
        Severity::Low => 0x1E_88_E5,
    }
}

fn source_name(server_base: &str) -> String {
    https_origin(server_base)
        .map(|origin| origin.trim_start_matches("https://").to_string())
        .unwrap_or_else(|| "stardust".to_string())
}

fn stamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn escape_slack(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_discord(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(
            ch,
            '\\' | '*' | '_' | '~' | '`' | '|' | '[' | ']' | '<' | '>' | '#' | '@'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn clip_text(value: &str, max: usize) -> String {
    let cleaned: String = value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(max)
        .collect();
    if cleaned.trim().is_empty() {
        "ORBIT item".to_string()
    } else {
        cleaned
    }
}

fn clip_id(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|ch| !ch.is_control() && !ch.is_whitespace())
        .take(128)
        .collect();
    if cleaned.is_empty() {
        "item".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::{
        check_routing_key, check_url, status_result, Alert, Sink, SinkError, SinkKind, WebhookSink,
        PAGERDUTY_ENQUEUE_EU, PAGERDUTY_ENQUEUE_US,
    };
    use crate::api::{AlertItem, Severity, DEFAULT_SERVER_BASE, WORKSPACE_NEEDS_ATTENTION};
    use crate::copy::find_disallowed_copy;
    use crate::testserver::{LocalTransport, Reply, TestServer};
    use chrono::{TimeZone, Utc};
    use serde_json::Value;

    const ROUTING_KEY: &str = "exampleroutingkey000000000000000";
    const SLACK_URL: &str = "https://hooks.slack.com/services/T000/B000/example";
    const TEAMS_URL: &str = "https://prod-00.westus.logic.azure.com:443/workflows/abc/triggers/manual/paths/invoke?api-version=2016-06-01&sp=%2Ftriggers%2Fmanual%2Frun&sv=1.0&sig=example";
    const DISCORD_URL: &str = "https://discord.com/api/webhooks/123456/example-token_1";

    fn high() -> Alert {
        Alert::from_item(
            &AlertItem {
                id: "mp-41-key-exchange".to_string(),
                severity: Severity::High,
                title:
                    "api.example:443 key-exchange x25519 <!channel> & [docs](https://evil.example)"
                        .to_string(),
                created_at: Utc.with_ymd_and_hms(2026, 10, 26, 0, 0, 0).unwrap(),
                workspace_path: WORKSPACE_NEEDS_ATTENTION.to_string(),
                immediate: true,
            },
            DEFAULT_SERVER_BASE,
        )
    }

    fn golden(name: &str) -> Value {
        let text = match name {
            "slack" => include_str!("../testdata/sinks/slack.json"),
            "teams" => include_str!("../testdata/sinks/teams.json"),
            "discord" => include_str!("../testdata/sinks/discord.json"),
            "pagerduty_trigger" => include_str!("../testdata/sinks/pagerduty_trigger.json"),
            "pagerduty_change" => include_str!("../testdata/sinks/pagerduty_change.json"),
            other => panic!("no golden file {other}"),
        };
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn each_sink_posts_its_golden_json_to_a_local_server() {
        let server = TestServer::start(|request| match request.path() {
            "/services/T000/B000/example" => Reply::json(200, "ok"),
            "/workflows/abc/triggers/manual/paths/invoke" => Reply::empty(202),
            "/api/webhooks/123456/example-token_1" => Reply::empty(204),
            "/v2/enqueue" | "/v2/change/enqueue" => Reply::json(
                202,
                r#"{"status":"success","message":"Event processed","dedup_key":"orbit-mp-41-key-exchange"}"#,
            ),
            _ => Reply::empty(404),
        });
        let transport = LocalTransport::to(&server);
        let alert = high();
        let cases = [
            (SinkKind::Slack, Some(SLACK_URL), None, "slack"),
            (SinkKind::Teams, Some(TEAMS_URL), None, "teams"),
            (SinkKind::Discord, Some(DISCORD_URL), None, "discord"),
            (
                SinkKind::PagerDuty,
                None,
                Some(ROUTING_KEY),
                "pagerduty_trigger",
            ),
        ];
        for (index, (kind, url, key, name)) in cases.into_iter().enumerate() {
            let sink =
                WebhookSink::new("ops", kind, url, key, DEFAULT_SERVER_BASE, &transport).unwrap();
            sink.deliver(&alert).unwrap();
            let requests = server.requests();
            let request = &requests[index];
            assert_eq!(request.method, "POST");
            assert_eq!(request.header("content-type"), Some("application/json"));
            assert_eq!(request.json(), golden(name), "{name}");
            assert_eq!(find_disallowed_copy(&request.body), None, "{name}");
            let debug = format!("{sink:?}");
            assert!(!debug.contains("example-token"), "{debug}");
            assert!(!debug.contains("sig=example"), "{debug}");
            assert!(!debug.contains(ROUTING_KEY), "{debug}");
        }
        assert!(server.requests()[1].target.ends_with("&sig=example"));
        let pagerduty = WebhookSink::new(
            "pd",
            SinkKind::PagerDuty,
            None,
            Some(ROUTING_KEY),
            DEFAULT_SERVER_BASE,
            &transport,
        )
        .unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 31, 9, 0, 0).unwrap();
        pagerduty.send_test(DEFAULT_SERVER_BASE, now).unwrap();
        let last = server.requests().pop().unwrap();
        assert_eq!(last.path(), "/v2/change/enqueue");
        assert_eq!(last.json(), golden("pagerduty_change"));
    }

    #[test]
    fn a_test_message_names_itself_and_links_only_to_the_orbit_origin() {
        let server = TestServer::start(|_| Reply::json(200, "ok"));
        let transport = LocalTransport::to(&server);
        let now = Utc.with_ymd_and_hms(2026, 10, 31, 9, 0, 0).unwrap();
        for (kind, url) in [
            (SinkKind::Slack, SLACK_URL),
            (SinkKind::Teams, TEAMS_URL),
            (SinkKind::Discord, DISCORD_URL),
        ] {
            let sink = WebhookSink::new(
                "ops",
                kind,
                Some(url),
                None,
                DEFAULT_SERVER_BASE,
                &transport,
            )
            .unwrap();
            sink.send_test(DEFAULT_SERVER_BASE, now).unwrap();
            let body = server.requests().pop().unwrap().body;
            assert!(body.contains("Stardust test message"), "{body}");
            assert!(body.contains("https://helloorbit.app"), "{body}");
            assert!(!body.contains("high-severity"), "{body}");
        }
        let local = Alert::sample("http://127.0.0.1:9", now);
        assert_eq!(local.link, None);
    }

    #[test]
    fn statuses_split_into_delivered_retry_and_refused() {
        assert_eq!(status_result(200), Ok(()));
        assert_eq!(status_result(202), Ok(()));
        assert_eq!(status_result(204), Ok(()));
        for busy in [408, 425, 429, 500, 502, 503] {
            let err = status_result(busy).unwrap_err();
            assert!(err.retry(), "{busy}");
        }
        for refused in [301, 400, 401, 403, 404, 410, 413] {
            let err = status_result(refused).unwrap_err();
            assert!(!err.retry(), "{refused}");
        }
        for gone in [301, 302, 307, 401, 403, 404, 410] {
            assert!(status_result(gone).unwrap_err().gone(), "{gone}");
        }
        for bad_payload in [400, 413, 422] {
            assert!(
                !status_result(bad_payload).unwrap_err().gone(),
                "{bad_payload}"
            );
        }
        assert!(SinkError::Blocked.gone());
        assert!(!SinkError::Blocked.retry());
        assert!(!SinkError::Refused.gone());
        assert!(SinkError::Unreachable.retry());
        assert!(!SinkError::Missing.retry());
        let transport = LocalTransport::closed();
        let sink = WebhookSink::new(
            "ops",
            SinkKind::Slack,
            Some(SLACK_URL),
            None,
            DEFAULT_SERVER_BASE,
            &transport,
        )
        .unwrap();
        assert_eq!(sink.deliver(&high()), Err(SinkError::Unreachable));
    }

    #[test]
    fn destinations_stay_on_each_vendor_host() {
        assert!(check_url(SinkKind::Slack, SLACK_URL).is_ok());
        assert!(check_url(
            SinkKind::Slack,
            "https://hooks.slack-gov.com/services/T/B/x"
        )
        .is_ok());
        assert!(check_url(SinkKind::Teams, TEAMS_URL).is_ok());
        assert!(check_url(
            SinkKind::Teams,
            "https://abc.12.environment.api.powerplatform.com:443/powerautomate/automations/direct/workflows/1/triggers/manual/paths/invoke?api-version=1&sig=x"
        )
        .is_ok());
        assert!(check_url(SinkKind::Discord, DISCORD_URL).is_ok());
        assert!(check_url(
            SinkKind::Discord,
            "https://discordapp.com/api/v10/webhooks/1/abc"
        )
        .is_ok());
        assert!(check_url(SinkKind::PagerDuty, PAGERDUTY_ENQUEUE_US).is_ok());
        assert!(check_url(SinkKind::PagerDuty, PAGERDUTY_ENQUEUE_EU).is_ok());
        assert_eq!(
            check_url(
                SinkKind::PagerDuty,
                "https://events.pagerduty.com/v2/enqueue/"
            )
            .unwrap()
            .url,
            PAGERDUTY_ENQUEUE_US,
            "kept without the trailing slash"
        );
        assert_eq!(
            check_url(
                SinkKind::PagerDuty,
                "https://events.eu.pagerduty.com:443/v2/enqueue"
            )
            .unwrap()
            .url,
            PAGERDUTY_ENQUEUE_EU,
            "and without a written default port"
        );
        assert!(check_url(
            SinkKind::Slack,
            "https://hooks.slack.com:443/services/T/B/x"
        )
        .is_ok());
        for (kind, refused) in [
            (SinkKind::Slack, "https://hooks.example/services/T/B/x"),
            (SinkKind::Slack, "http://hooks.slack.com/services/T/B/x"),
            (
                SinkKind::Slack,
                "https://hooks.slack.com/services/T/B/x?x=1",
            ),
            (SinkKind::Slack, "https://hooks.slack.com"),
            (SinkKind::Teams, "https://evil.example/workflows/a?sig=1"),
            (
                SinkKind::Teams,
                "https://prod-00.westus.logic.azure.com/workflows/a",
            ),
            (
                SinkKind::Teams,
                "https://logic.azure.com.evil.example/w?sig=1",
            ),
            (
                SinkKind::Discord,
                "https://discord.com/api/webhooks/abc/token",
            ),
            (
                SinkKind::Discord,
                "https://discord.com/api/webhooks/1/token/extra",
            ),
            (SinkKind::Discord, "https://discord.com/channels/1/2"),
            (
                SinkKind::Discord,
                "https://evil.example/api/webhooks/1/token",
            ),
            (
                SinkKind::PagerDuty,
                "https://events.pagerduty.com/v2/change/enqueue",
            ),
            (SinkKind::PagerDuty, "https://evil.example/v2/enqueue"),
        ] {
            assert_eq!(
                check_url(kind, refused).unwrap_err(),
                SinkError::Refused,
                "{refused}"
            );
        }
        assert!(check_routing_key(ROUTING_KEY).is_ok());
        assert!(check_routing_key("short").is_err());
        assert!(check_routing_key("exampleroutingkey00000000000000!").is_err());
        assert_eq!(
            format!("{:?}", check_routing_key(ROUTING_KEY).unwrap()),
            "<redacted>"
        );
        let transport = LocalTransport::closed();
        assert_eq!(
            WebhookSink::new(
                "pd",
                SinkKind::PagerDuty,
                None,
                None,
                DEFAULT_SERVER_BASE,
                &transport
            )
            .unwrap_err(),
            SinkError::Missing
        );
        assert_eq!(
            WebhookSink::new(
                "ops",
                SinkKind::Slack,
                None,
                None,
                DEFAULT_SERVER_BASE,
                &transport
            )
            .unwrap_err(),
            SinkError::Missing
        );
    }

    #[test]
    fn links_never_leave_the_configured_https_host() {
        let item = AlertItem {
            id: "x".to_string(),
            severity: Severity::Low,
            title: "t".to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 10, 26, 0, 0, 0).unwrap(),
            workspace_path: "https://evil.example/phish".to_string(),
            immediate: false,
        };
        assert_eq!(
            Alert::from_item(&item, DEFAULT_SERVER_BASE).link.as_deref(),
            Some("https://helloorbit.app")
        );
        assert_eq!(Alert::from_item(&item, "http://127.0.0.1:9").link, None);
        assert_eq!(high().severity, Severity::High);
        assert_eq!(
            high().link.as_deref(),
            Some("https://helloorbit.app/rust-preview/pqc/studio/app/control-room/needs-attention")
        );
    }
}
