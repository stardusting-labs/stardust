//! Outbound posts in the shape ORBIT's webhook draft uses.
//!
//! A high-severity item is posted from the check that observed it, including
//! during quiet hours. Other results wait for a check outside quiet hours.
//! This module does not open a socket.

use crate::api::{
    https_origin, workspace_url, AlertItem, Severity, DEFAULT_SERVER_BASE, PLACEHOLDER_ORIGIN,
};
use crate::integrations::{
    secret_account, IntegrationKind, IntegrationRecord, SecretSlot, SecretStore,
};
use crate::policy::details_url;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fmt;
use std::net::IpAddr;
use uuid::Uuid;

const PREVIEW_TENANT: &str = "00000000-0000-4000-8000-000000000001";
const PREVIEW_WORKSPACE: &str = "00000000-0000-4000-8000-000000000002";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UrlError {
    #[error("destination must be an https URL")]
    Url,
}

/// A destination that passed [`public_https`]. Webhook URLs carry their
/// credential in the path or query, so Debug output shows the host only.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedUrl {
    pub url: String,
    pub host: String,
}

impl fmt::Debug for CheckedUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CheckedUrl")
            .field("url", &"<redacted>")
            .field("host", &self.host)
            .finish()
    }
}

pub fn public_https(value: &str) -> Result<CheckedUrl, UrlError> {
    let trimmed = value.trim();
    let rest = trimmed.strip_prefix("https://").ok_or(UrlError::Url)?;
    if rest.is_empty() || trimmed.contains([' ', '\\', '@', '#', '?', '%']) {
        return Err(UrlError::Url);
    }
    let (host_port, path) = match rest.split_once('/') {
        Some((host, path)) => (host, path),
        None => (rest, ""),
    };
    if host_port.is_empty() || path.contains("//") {
        return Err(UrlError::Url);
    }
    let host = host_without_port(host_port).ok_or(UrlError::Url)?;
    if host_blocked(&host) {
        return Err(UrlError::Url);
    }
    Ok(CheckedUrl {
        url: trimmed.to_string(),
        host,
    })
}

/// [`public_https`] that also accepts a query string, for webhook URLs that
/// carry their signature there (Teams Workflows). The part before `?`
/// passes the same checks. The query may hold percent escapes and nothing
/// that changes the host, such as `@`, `#`, `\`, or a space.
pub fn public_https_with_query(value: &str) -> Result<CheckedUrl, UrlError> {
    let trimmed = value.trim();
    let Some((base, query)) = trimmed.split_once('?') else {
        return public_https(trimmed);
    };
    let checked = public_https(base)?;
    if !base.trim_start_matches("https://").contains('/')
        || query.is_empty()
        || !query.bytes().all(|byte| byte.is_ascii_graphic())
        || query.contains(['?', '#', '\\', '@'])
        || !percent_escapes_are_whole(query)
    {
        return Err(UrlError::Url);
    }
    Ok(CheckedUrl {
        url: trimmed.to_string(),
        host: checked.host,
    })
}

fn percent_escapes_are_whole(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let pair = bytes.get(index + 1..index + 3);
            if !pair.is_some_and(|pair| pair.iter().all(u8::is_ascii_hexdigit)) {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    true
}

pub fn ip_is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_unspecified()
                || ip.is_documentation()
                || octets[0] == 0
                || (octets[0] == 100 && (octets[1] & 0xc0) == 64)
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unicast_link_local()
                || ip.is_unique_local()
                || matches!(ip.to_ipv4_mapped(), Some(mapped) if ip_is_blocked(IpAddr::V4(mapped)))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendPlan {
    None,
    Summary,
    Immediate,
    Both,
}

pub fn send_plan(enabled: bool, quiet: bool, high: bool, manual: bool) -> SendPlan {
    if !enabled {
        return SendPlan::None;
    }
    match (high, manual || !quiet) {
        (false, false) => SendPlan::None,
        (true, false) => SendPlan::Immediate,
        (false, true) => SendPlan::Summary,
        (true, true) => SendPlan::Both,
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PreparedPost {
    pub integration_id: String,
    pub host: String,
    pub url: String,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
}

impl fmt::Debug for PreparedPost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedPost")
            .field("integration_id", &self.integration_id)
            .field("host", &self.host)
            .field("url", &"<redacted>")
            .field("body_len", &self.body.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryNote {
    Ready(PreparedPost),
    Missing { id: String, what: &'static str },
    Refused { id: String },
}

pub struct DeliveryWindow<'a> {
    pub quiet: bool,
    pub manual: bool,
    pub now: DateTime<Utc>,
    pub interval_minutes: u32,
    pub server_base: &'a str,
}

pub fn prepare_posts(
    records: &[IntegrationRecord],
    secrets: &dyn SecretStore,
    items: &[AlertItem],
    window: &DeliveryWindow<'_>,
) -> Vec<DeliveryNote> {
    let mut notes = Vec::new();
    for record in records {
        if !record.enabled || !record.kind.posts_webhook() {
            continue;
        }
        match one_destination(record, secrets, items, window) {
            Ok(posts) => notes.extend(posts.into_iter().map(DeliveryNote::Ready)),
            Err(note) => notes.push(note),
        }
    }
    notes
}

fn one_destination(
    record: &IntegrationRecord,
    secrets: &dyn SecretStore,
    items: &[AlertItem],
    window: &DeliveryWindow<'_>,
) -> Result<Vec<PreparedPost>, DeliveryNote> {
    let url_secret = secrets
        .get(&secret_account(&record.id, SecretSlot::Url))
        .map_err(|_| missing(&record.id, "url"))?
        .ok_or_else(|| missing(&record.id, "url"))?;
    let checked = public_https(&url_secret).map_err(|_| DeliveryNote::Refused {
        id: record.id.clone(),
    })?;
    let signing = secrets
        .get(&secret_account(&record.id, SecretSlot::SigningSecret))
        .map_err(|_| missing(&record.id, "signing secret"))?;
    if record.kind == IntegrationKind::GenericJson && signing.as_deref().unwrap_or("").is_empty() {
        return Err(missing(&record.id, "signing secret"));
    }
    let high = items.iter().any(AlertItem::shows_immediately);
    let plan = send_plan(true, window.quiet, high, window.manual);
    let mut posts = Vec::new();
    if matches!(plan, SendPlan::Immediate | SendPlan::Both) {
        for item in items.iter().filter(|item| item.shows_immediately()) {
            posts.push(render_post(
                record,
                &checked,
                signing.as_deref(),
                &immediate_body(item, window),
                "finding.created",
            )?);
        }
    }
    if matches!(plan, SendPlan::Summary | SendPlan::Both) {
        posts.push(render_post(
            record,
            &checked,
            signing.as_deref(),
            &summary_body(items, window),
            "status.summary",
        )?);
    }
    Ok(posts)
}

fn render_post(
    record: &IntegrationRecord,
    checked: &CheckedUrl,
    signing: Option<&str>,
    value: &Value,
    event_type: &str,
) -> Result<PreparedPost, DeliveryNote> {
    let body = serde_json::to_vec(value).map_err(|_| DeliveryNote::Refused {
        id: record.id.clone(),
    })?;
    let delivery = value["event_id"].as_str().unwrap_or("0");
    let mut headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Orbit-Event".to_string(), event_type.to_string()),
        ("Orbit-Delivery".to_string(), delivery.to_string()),
    ];
    if let Some(secret) = signing.filter(|secret| !secret.is_empty()) {
        let stamp = Utc::now().timestamp();
        headers.push(("Orbit-Signature".to_string(), sign(secret, stamp, &body)));
    }
    Ok(PreparedPost {
        integration_id: record.id.clone(),
        host: checked.host.clone(),
        url: checked.url.clone(),
        body,
        headers,
    })
}

fn missing(id: &str, what: &'static str) -> DeliveryNote {
    DeliveryNote::Missing {
        id: id.to_string(),
        what,
    }
}

fn immediate_body(item: &AlertItem, window: &DeliveryWindow<'_>) -> Value {
    let link = workspace_link(window.server_base, &item.workspace_path);
    json!({
        "schema_version": "1",
        "event_id": Uuid::new_v4().to_string(),
        "event_type": "finding.created",
        "occurred_at": stamp(window.now),
        "tenant_id": PREVIEW_TENANT,
        "workspace_id": PREVIEW_WORKSPACE,
        "severity": "high",
        "subject_id": clip_id(&item.id),
        "summary": clip_text(&item.title),
        "last_checked_at": stamp(window.now),
        "check_interval_minutes": window.interval_minutes.clamp(1, 10_080),
        "workspace_url": link,
    })
}

fn summary_body(items: &[AlertItem], window: &DeliveryWindow<'_>) -> Value {
    let high = items.iter().filter(|item| item.shows_immediately()).count();
    let summary = if items.is_empty() {
        "No new or changed items".to_string()
    } else if high > 0 {
        format!("{high} high-severity items")
    } else {
        format!("{} new or changed items", items.len())
    };
    let attention: Vec<Value> = items
        .iter()
        .take(10)
        .map(|item| {
            json!({
                "id": clip_id(&item.id),
                "severity": wire_severity(item.severity),
                "summary": clip_text(&item.title),
                "last_checked_at": stamp(item.created_at),
                "workspace_url": workspace_link(window.server_base, &item.workspace_path),
            })
        })
        .collect();
    json!({
        "schema_version": "1",
        "event_id": Uuid::new_v4().to_string(),
        "event_type": "status.summary",
        "occurred_at": stamp(window.now),
        "tenant_id": PREVIEW_TENANT,
        "workspace_id": PREVIEW_WORKSPACE,
        "summary": summary,
        "last_checked_at": stamp(window.now),
        "check_interval_minutes": window.interval_minutes.clamp(1, 10_080),
        "workspace_url": details_url(items, window.server_base, ""),
        "attention_count": high,
        "finding_count": items.len(),
        "check_count": 1,
        "attention_items": attention,
    })
}

pub fn render_destination(kind: IntegrationKind, generic: &Value) -> Value {
    match kind {
        IntegrationKind::GenericJson
        | IntegrationKind::OrbitMcp
        | IntegrationKind::SlackMcp
        | IntegrationKind::DiscordWebhook
        | IntegrationKind::PagerDuty => generic.clone(),
        IntegrationKind::SlackIncoming => slack_body(generic),
        IntegrationKind::TeamsIncoming => teams_body(generic),
    }
}

fn slack_body(generic: &Value) -> Value {
    let summary = generic["summary"].as_str().unwrap_or("");
    let cadence = cadence_line(generic);
    let link = generic["workspace_url"]
        .as_str()
        .unwrap_or(PLACEHOLDER_ORIGIN);
    json!({
        "text": cadence,
        "blocks": [
            { "type": "header", "text": { "type": "plain_text", "text": "ORBIT workspace update", "emoji": false } },
            { "type": "section", "text": { "type": "mrkdwn", "text": slack_escape(summary) } },
            { "type": "context", "elements": [{ "type": "mrkdwn", "text": slack_escape(&cadence) }] },
            { "type": "section", "text": { "type": "mrkdwn", "text": format!("<{link}|Open workspace>") } }
        ]
    })
}

fn teams_body(generic: &Value) -> Value {
    let summary = generic["summary"].as_str().unwrap_or("");
    let cadence = cadence_line(generic);
    let link = generic["workspace_url"]
        .as_str()
        .unwrap_or(PLACEHOLDER_ORIGIN);
    json!({
        "type": "message",
        "attachments": [{
            "contentType": "application/vnd.microsoft.card.adaptive",
            "content": {
                "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
                "type": "AdaptiveCard",
                "version": "1.4",
                "body": [
                    { "type": "TextBlock", "text": "ORBIT workspace update", "wrap": true, "weight": "Bolder" },
                    { "type": "TextBlock", "text": summary, "wrap": true },
                    { "type": "TextBlock", "text": cadence, "wrap": true }
                ],
                "actions": [{ "type": "Action.OpenUrl", "title": "Open workspace", "url": link }]
            }
        }]
    })
}

fn cadence_line(generic: &Value) -> String {
    let minutes = generic["check_interval_minutes"].as_u64().unwrap_or(1);
    let last = generic["last_checked_at"].as_str().unwrap_or("");
    format!("Checked every {minutes} minutes. Last checked time: {last}.")
}

pub fn sign(secret: &str, timestamp_unix: i64, body: &[u8]) -> String {
    let digest = hmac_sha256(secret.as_bytes(), &signed_bytes(timestamp_unix, body));
    format!("t={timestamp_unix},v1={}", encode_hex(&digest))
}

pub fn verify(secret: &str, header: &str, body: &[u8], now_unix: i64) -> bool {
    let Some((timestamp, received)) = parse_header(header) else {
        return false;
    };
    if now_unix.abs_diff(timestamp) > 300 {
        return false;
    }
    let expected = hmac_sha256(secret.as_bytes(), &signed_bytes(timestamp, body));
    constant_time_eq(&expected, &received)
}

fn signed_bytes(timestamp_unix: i64, body: &[u8]) -> Vec<u8> {
    let prefix = timestamp_unix.to_string();
    let mut signed = Vec::with_capacity(prefix.len() + 1 + body.len());
    signed.extend_from_slice(prefix.as_bytes());
    signed.push(b'.');
    signed.extend_from_slice(body);
    signed
}

fn parse_header(header: &str) -> Option<(i64, [u8; 32])> {
    let mut timestamp = None;
    let mut signature = None;
    for part in header.split(',') {
        let (name, value) = part.split_once('=')?;
        match name.trim() {
            "t" => {
                if timestamp
                    .replace(value.trim().parse::<i64>().ok()?)
                    .is_some()
                {
                    return None;
                }
            }
            "v1" => {
                let decoded = decode_hex(value.trim())?;
                let bytes: [u8; 32] = decoded.try_into().ok()?;
                if signature.replace(bytes).is_some() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some((timestamp?, signature?))
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut key_block = [0_u8; BLOCK];
    if key.len() > BLOCK {
        let hashed = Sha256::digest(key);
        key_block[..hashed.len()].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36_u8; BLOCK];
    let mut opad = [0x5c_u8; BLOCK];
    for index in 0..BLOCK {
        ipad[index] ^= key_block[index];
        opad[index] ^= key_block[index];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    let digest = outer.finalize();
    let mut out = [0_u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    let mut diff = 0_u8;
    for index in 0..32 {
        diff |= left[index] ^ right[index];
    }
    diff == 0
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || !text.len().is_multiple_of(2) {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(text.len() / 2);
    let mut index = 0;
    while index < bytes.len() {
        let high = hex_value(bytes[index])?;
        let low = hex_value(bytes[index + 1])?;
        out.push((high << 4) | low);
        index += 2;
    }
    Some(out)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn host_without_port(host_port: &str) -> Option<String> {
    if let Some(inner) = host_port.strip_prefix('[') {
        let (host, _) = inner.split_once(']')?;
        return Some(host.to_ascii_lowercase());
    }
    let host = host_port.split(':').next()?;
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

fn host_blocked(host: &str) -> bool {
    if host == "localhost"
        || host == "localhost.localdomain"
        || host == "metadata.google.internal"
        || host.ends_with(".local")
        || host.ends_with(".internal")
    {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(ip_is_blocked)
}

fn wire_severity(severity: Severity) -> &'static str {
    match severity {
        Severity::Low => "low",
        Severity::Medium => "medium",
        Severity::High => "high",
    }
}

fn stamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn clip_text(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(280)
        .collect();
    if cleaned.is_empty() {
        "ORBIT workspace update".to_string()
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

fn workspace_link(server_base: &str, path: &str) -> String {
    workspace_url(server_base, path).unwrap_or_else(|| {
        https_origin(server_base).unwrap_or_else(|| DEFAULT_SERVER_BASE.to_string())
    })
}

fn slack_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::{
        ip_is_blocked, prepare_posts, public_https, render_destination, send_plan, sign, verify,
        DeliveryNote, DeliveryWindow, SendPlan,
    };
    use crate::api::{AlertItem, Severity};
    use crate::copy::find_disallowed_copy;
    use crate::integrations::{
        secret_account, IntegrationKind, IntegrationRecord, MemorySecretStore, SecretSlot,
        SecretStore,
    };
    use chrono::{TimeZone, Utc};
    use serde_json::Value;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn item(severity: Severity) -> AlertItem {
        AlertItem {
            id: "finding-1".to_string(),
            severity,
            title: "Example connection finding".to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 10, 22, 12, 0, 0).unwrap(),
            workspace_path: "/workspace/example".to_string(),
            immediate: severity == Severity::High,
        }
    }

    #[test]
    fn signature_matches_the_rfc_vector_and_a_round_trip() {
        let digest = super::hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            super::encode_hex(&digest),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        let secret = "example-signing-secret";
        let body = br#"{"schema_version":"1"}"#;
        let header = sign(secret, 1_700_000_000, body);
        assert!(verify(secret, &header, body, 1_700_000_100));
        assert!(!verify(secret, &header, b"{}", 1_700_000_100));
        assert!(!header.contains(secret));
    }

    #[test]
    fn quiet_hours_hold_ordinary_results_and_still_post_high_severity() {
        assert_eq!(send_plan(true, true, false, false), SendPlan::None);
        assert_eq!(send_plan(true, true, true, false), SendPlan::Immediate);
        assert_eq!(send_plan(false, false, true, true), SendPlan::None);
        let mut record = IntegrationRecord::new(IntegrationKind::GenericJson, "ops").unwrap();
        record.enabled = true;
        let secrets = MemorySecretStore::new();
        secrets
            .put(
                &secret_account("ops", SecretSlot::Url),
                "https://hooks.example/inbound",
            )
            .unwrap();
        secrets
            .put(
                &secret_account("ops", SecretSlot::SigningSecret),
                "example-signing-secret",
            )
            .unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 22, 23, 0, 0).unwrap();
        let window = DeliveryWindow {
            quiet: true,
            manual: false,
            now,
            interval_minutes: 45,
            server_base: crate::api::DEFAULT_SERVER_BASE,
        };
        let held = prepare_posts(
            std::slice::from_ref(&record),
            &secrets,
            &[item(Severity::Medium)],
            &window,
        );
        assert!(held.is_empty());
        let sent = prepare_posts(
            std::slice::from_ref(&record),
            &secrets,
            &[item(Severity::High)],
            &window,
        );
        assert_eq!(sent.len(), 1);
        let DeliveryNote::Ready(post) = &sent[0] else {
            panic!("ready");
        };
        let value: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(value["event_type"], "finding.created");
        assert_eq!(value["severity"], "high");
        assert!(post
            .headers
            .iter()
            .any(|(name, _)| name == "Orbit-Signature"));
        let rendered = format!("{post:?}");
        assert!(rendered.contains("hooks.example"));
        assert!(!rendered.contains("/inbound"));
        assert!(!rendered.contains("example-signing-secret"));
        let slack = render_destination(IntegrationKind::SlackIncoming, &value);
        let teams = render_destination(IntegrationKind::TeamsIncoming, &value);
        assert!(slack["blocks"].is_array());
        assert_eq!(teams["attachments"][0]["content"]["type"], "AdaptiveCard");
        for body in [slack.to_string(), teams.to_string(), value.to_string()] {
            assert_eq!(find_disallowed_copy(&body), None, "{body}");
        }
    }

    #[test]
    fn destinations_reject_local_and_metadata_addresses() {
        assert!(public_https("http://hooks.example/hook").is_err());
        assert!(public_https("https://user:pass@hooks.example/hook").is_err());
        assert!(public_https("https://localhost/hook").is_err());
        assert!(public_https("https://metadata.google.internal/computeMetadata/v1").is_err());
        assert!(public_https("https://127.0.0.1/hook").is_err());
        assert!(public_https("https://10.1.2.3/hook").is_err());
        assert!(public_https("https://169.254.169.254/latest").is_err());
        assert!(ip_is_blocked(std::net::IpAddr::V4(Ipv4Addr::new(
            192, 168, 1, 1
        ))));
        assert!(ip_is_blocked(std::net::IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(public_https("https://hooks.example/hook?token=1").is_err());
        assert_eq!(
            public_https("https://hooks.example/inbound").unwrap().host,
            "hooks.example"
        );
    }

    #[test]
    fn a_query_is_accepted_only_by_the_query_variant_and_only_after_a_path() {
        use super::public_https_with_query;
        let workflow = "https://prod-00.westus.logic.azure.com:443/workflows/abc/triggers/manual/paths/invoke?api-version=2016-06-01&sp=%2Ftriggers%2Fmanual%2Frun&sv=1.0&sig=AbC-123_x";
        let checked = public_https_with_query(workflow).unwrap();
        assert_eq!(checked.host, "prod-00.westus.logic.azure.com");
        assert_eq!(checked.url, workflow);
        assert!(public_https(workflow).is_err());
        for refused in [
            "http://prod-00.westus.logic.azure.com/workflows/a?sig=1",
            "https://prod-00.westus.logic.azure.com?sig=1",
            "https://prod-00.westus.logic.azure.com/workflows/a?",
            "https://prod-00.westus.logic.azure.com/workflows/a?sig=1#frag",
            "https://prod-00.westus.logic.azure.com/workflows/a?sig=1@evil.example",
            "https://prod-00.westus.logic.azure.com/workflows/a?sig=%zz",
            "https://prod-00.westus.logic.azure.com/workflows/a?sig=%2",
            "https://prod-00.westus.logic.azure.com/workflows/a?sig=a b",
            "https://user@prod-00.westus.logic.azure.com/workflows/a?sig=1",
            "https://127.0.0.1/workflows/a?sig=1",
            "https://localhost/workflows/a?sig=1",
        ] {
            assert!(public_https_with_query(refused).is_err(), "{refused}");
        }
        assert_eq!(
            public_https_with_query("https://hooks.example/inbound")
                .unwrap()
                .url,
            "https://hooks.example/inbound"
        );
    }
}
