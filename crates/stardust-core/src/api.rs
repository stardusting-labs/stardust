//! ORBIT device sign-in and the desktop status and alerts surface.
//!
//! JSON matches `docs/contracts/stardust-desktop-api-v1.md`.
//! `POST /api/pqc/v1/account/device/code` and
//! `POST /api/pqc/v1/account/device/token` are the device flow. A signed-in
//! browser calls `POST /api/pqc/v1/account/device/approve`. This client does not.
//!
//! [`DesktopApi`] is the two bearer GETs. A 404 with an empty body means the
//! feature flag is off. A 401 means the bearer was missing or rejected.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;

pub const DEFAULT_SERVER_BASE: &str = "https://helloorbit.app";
pub const ENV_SERVER_URL: &str = "STARDUST_SERVER_URL";
pub const PATH_DEVICE_CODE: &str = "/api/pqc/v1/account/device/code";
pub const PATH_DEVICE_TOKEN: &str = "/api/pqc/v1/account/device/token";
pub const PATH_DEVICE_APPROVE: &str = "/api/pqc/v1/account/device/approve";
pub const PATH_DESKTOP_STATUS: &str = "/api/pqc/v1/desktop/status";
pub const PATH_DESKTOP_ALERTS: &str = "/api/pqc/v1/desktop/alerts";
pub const DEVICE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// RFC 6749 section 6 refresh grant.
///
/// TODO(Unsu): `docs/contracts/stardust-desktop-api-v1.md` (rust-orbit #782)
/// has no refresh grant yet. This client sends it to the device token route
/// only when the server already returned a `refresh_token`. Confirm the route
/// and field names with the contract owners before release.
pub const REFRESH_GRANT_TYPE: &str = "refresh_token";
pub const PATH_TOKEN_REFRESH: &str = PATH_DEVICE_TOKEN;
/// RFC 7009 style revocation of a refresh token on sign-out.
///
/// TODO(Unsu): not in `docs/contracts/stardust-desktop-api-v1.md` yet
/// (rust-orbit follow-up A). The client calls it only for a saved refresh
/// token, treats a 404 as "not supported", and deletes the local copy
/// either way.
pub const PATH_DEVICE_REVOKE: &str = "/api/pqc/v1/account/device/revoke";
pub const DEVICE_CODE_TTL_SECONDS: u64 = 600;
pub const ACCESS_TOKEN_LIFETIME_SECONDS: u64 = 300;
pub const WORKSPACE_FINDINGS: &str = "/rust-preview/pqc/studio/app/findings";
pub const WORKSPACE_FINDINGS_BY_SEVERITY: &str =
    "/rust-preview/pqc/studio/app/findings/by-severity";
pub const WORKSPACE_NEEDS_ATTENTION: &str =
    "/rust-preview/pqc/studio/app/control-room/needs-attention";

/// Kept so older notes that say "placeholder origin" still point at the default host.
pub const PLACEHOLDER_ORIGIN: &str = DEFAULT_SERVER_BASE;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OrbitError {
    #[error("ORBIT desktop alerts are turned off")]
    FeatureOff,
    #[error("ORBIT sign-in expired")]
    Unauthorized,
    #[error("ORBIT could not be reached")]
    Unreachable,
    #[error("ORBIT response could not be read")]
    Response,
    #[error("ORBIT refused the request")]
    Refused,
    #[error("the sign-in code expired")]
    Expired,
    #[error("ORBIT denied the sign-in")]
    Denied,
    #[error("server address was refused")]
    ServerBase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleOwner {
    Client,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImmediateSeverity {
    High,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopStatus {
    pub organization_id: String,
    pub observation_count: u64,
    pub alert_count: u64,
    pub high_severity_count: u64,
    pub immediate_severity: ImmediateSeverity,
    pub schedule_owner: ScheduleOwner,
    pub workspace_path: String,
    pub as_of_unix: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertItem {
    pub id: String,
    pub severity: Severity,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub workspace_path: String,
    pub immediate: bool,
}

impl AlertItem {
    /// High alerts are marked immediate by the server.
    pub fn shows_immediately(&self) -> bool {
        self.immediate || self.severity == Severity::High
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertsPage {
    pub alerts: Vec<AlertItem>,
}

/// `POST /api/pqc/v1/account/device/code` response.
///
/// `device_code` is a secret. Debug output redacts it.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

impl fmt::Debug for DeviceCodeResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceCodeResponse")
            .field("device_code", &"<redacted>")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

/// `POST /api/pqc/v1/account/device/token` body.
///
/// `device_code` is a secret. Debug output redacts it.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct DeviceTokenRequest {
    pub grant_type: String,
    pub device_code: String,
}

impl fmt::Debug for DeviceTokenRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceTokenRequest")
            .field("grant_type", &self.grant_type)
            .field("device_code", &"<redacted>")
            .finish()
    }
}

/// `POST /api/pqc/v1/account/device/token` refresh body.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct RefreshTokenRequest {
    pub grant_type: String,
    pub refresh_token: String,
}

impl fmt::Debug for RefreshTokenRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RefreshTokenRequest")
            .field("grant_type", &self.grant_type)
            .field("refresh_token", &"<redacted>")
            .finish()
    }
}

/// `POST /api/pqc/v1/account/device/revoke` body, RFC 7009 section 2.1.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct RevokeRequest {
    pub token: String,
    pub token_type_hint: String,
}

impl fmt::Debug for RevokeRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RevokeRequest")
            .field("token", &"<redacted>")
            .field("token_type_hint", &self.token_type_hint)
            .finish()
    }
}

/// Device token and refresh response.
///
/// `refresh_token` and `refresh_expires_in` are optional so a server that
/// starts sending them is still accepted. Other unknown fields are refused.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceTokenResponse {
    pub token: String,
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub expires_at_epoch_ms: u64,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub refresh_expires_in: Option<u64>,
}

impl fmt::Debug for DeviceTokenResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceTokenResponse")
            .field("token", &"<redacted>")
            .field("access_token", &"<redacted>")
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .field("expires_at_epoch_ms", &self.expires_at_epoch_ms)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("refresh_expires_in", &self.refresh_expires_in)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DevicePoll {
    Pending,
    SlowDown { interval: u64 },
    Approved(DeviceTokenResponse),
}

/// Status and alerts. Kept separate from device sign-in.
pub trait DesktopApi: Send + Sync {
    fn desktop_status(
        &self,
        access_token: &str,
    ) -> impl Future<Output = Result<DesktopStatus, OrbitError>> + Send;
    fn desktop_alerts(
        &self,
        access_token: &str,
    ) -> impl Future<Output = Result<AlertsPage, OrbitError>> + Send;
}

/// Sample items for `--demo` and tests. This client does not open a socket.
#[derive(Debug)]
pub struct MockOrbitClient {
    status: DesktopStatus,
    first_page: AlertsPage,
}

impl MockOrbitClient {
    pub fn example() -> Self {
        let seen = DateTime::parse_from_rfc3339("2026-10-21T00:00:00Z")
            .expect("fixed example timestamp")
            .with_timezone(&Utc);
        Self {
            status: DesktopStatus {
                organization_id: "00000000-0000-4000-8000-000000000001".to_string(),
                observation_count: 1,
                alert_count: 1,
                high_severity_count: 0,
                immediate_severity: ImmediateSeverity::High,
                schedule_owner: ScheduleOwner::Client,
                workspace_path: WORKSPACE_FINDINGS.to_string(),
                as_of_unix: seen.timestamp(),
            },
            first_page: AlertsPage {
                alerts: vec![AlertItem {
                    id: "example-1".to_string(),
                    severity: Severity::Medium,
                    title: "Example connection finding".to_string(),
                    created_at: seen,
                    workspace_path: WORKSPACE_FINDINGS_BY_SEVERITY.to_string(),
                    immediate: false,
                }],
            },
        }
    }
}

impl DesktopApi for MockOrbitClient {
    fn desktop_status(
        &self,
        _access_token: &str,
    ) -> impl Future<Output = Result<DesktopStatus, OrbitError>> + Send {
        let status = self.status.clone();
        async move { Ok(status) }
    }

    fn desktop_alerts(
        &self,
        _access_token: &str,
    ) -> impl Future<Output = Result<AlertsPage, OrbitError>> + Send {
        let page = self.first_page.clone();
        async move { Ok(page) }
    }
}

pub fn resolve_server_base(
    configured: Option<&str>,
    env_value: Option<&str>,
) -> Result<String, OrbitError> {
    let chosen = env_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| configured.map(str::trim).filter(|value| !value.is_empty()))
        .unwrap_or(DEFAULT_SERVER_BASE);
    parse_server_base(chosen)
}

pub fn parse_server_base(value: &str) -> Result<String, OrbitError> {
    let trimmed = value.trim();
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .ok_or(OrbitError::ServerBase)?;
    if rest.is_empty()
        || trimmed.contains([' ', '\\', '@', '#', '?'])
        || trimmed.contains("://") && trimmed.matches("://").count() != 1
    {
        return Err(OrbitError::ServerBase);
    }
    let (host_port, path) = match rest.split_once('/') {
        Some((host, path)) => (host, path),
        None => (rest, ""),
    };
    let Some((host, _)) = split_host_port(host_port) else {
        return Err(OrbitError::ServerBase);
    };
    if !path.is_empty() {
        return Err(OrbitError::ServerBase);
    }
    let scheme = if trimmed.starts_with("https://") {
        "https"
    } else {
        // The bearer token rides on every request, so plain http is only
        // accepted for a loopback host (local development and tests).
        if !is_loopback_host(host) {
            return Err(OrbitError::ServerBase);
        }
        "http"
    };
    Ok(format!("{scheme}://{host_port}"))
}

pub fn https_origin(server_base: &str) -> Option<String> {
    let base = parse_server_base(server_base).ok()?;
    base.starts_with("https://").then_some(base)
}

/// Join a workspace path onto the configured https origin.
///
/// Anything that is not a path on that origin is rejected. Full URLs, other
/// hosts, and non-https bases produce no link.
pub fn workspace_url(server_base: &str, workspace_path: &str) -> Option<String> {
    let origin = https_origin(server_base)?;
    let path = safe_workspace_path(workspace_path)?;
    Some(format!("{origin}{path}"))
}

pub fn is_allowed_https_url(server_base: &str, candidate: &str) -> bool {
    let Ok(base) = parse_server_base(server_base) else {
        return false;
    };
    if !base.starts_with("https://") {
        return false;
    }
    let Some(base_host) = base.strip_prefix("https://") else {
        return false;
    };
    let candidate = candidate.trim();
    let Some(rest) = candidate.strip_prefix("https://") else {
        return false;
    };
    if candidate.contains([' ', '\\', '@'])
        || candidate.chars().any(char::is_control)
        || rest.is_empty()
    {
        return false;
    }
    let (host_port, path) = match rest.split_once('/') {
        Some((host, path)) => (host, path),
        None => (rest, ""),
    };
    if path.contains('\\') || host_port.is_empty() {
        return false;
    }
    hosts_match(base_host, host_port)
}

pub fn verification_url(server_base: &str, response: &DeviceCodeResponse) -> Option<String> {
    let candidate = if response.verification_uri_complete.is_empty() {
        response.verification_uri.as_str()
    } else {
        response.verification_uri_complete.as_str()
    }
    .trim();
    is_allowed_https_url(server_base, candidate).then(|| candidate.to_string())
}

pub fn bumped_interval(current: u64, slow_down: bool) -> u64 {
    let current = current.clamp(1, 30);
    if slow_down {
        (current + 5).min(30)
    } else {
        current
    }
}

pub(crate) fn safe_workspace_path(value: &str) -> Option<&str> {
    let path = value.trim();
    if !path.starts_with('/') || path.starts_with("//") || path.contains('\\') {
        return None;
    }
    if path
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace() || ch == '@' || ch == '#')
    {
        return None;
    }
    let (path_part, query) = path
        .split_once('?')
        .map(|(path, query)| (path, Some(query)))
        .unwrap_or((path, None));
    if path_part
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return None;
    }
    let lower = path.to_ascii_lowercase();
    if lower.contains("%2e")
        || lower.contains("%2f")
        || lower.contains("%5c")
        || lower.contains("://")
    {
        return None;
    }
    if query.is_some_and(|query| query.contains(['@', '\\', ' '])) {
        return None;
    }
    Some(path)
}

fn hosts_match(base_host: &str, candidate_host: &str) -> bool {
    let Some((base_name, base_port)) = split_host_port(base_host) else {
        return false;
    };
    let Some((candidate_name, candidate_port)) = split_host_port(candidate_host) else {
        return false;
    };
    if !base_name.eq_ignore_ascii_case(candidate_name) {
        return false;
    }
    port_or_https(base_port) == port_or_https(candidate_port)
}

fn port_or_https(port: Option<u16>) -> u16 {
    port.unwrap_or(443)
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn split_host_port(host_port: &str) -> Option<(&str, Option<u16>)> {
    if host_port.is_empty() {
        return None;
    }
    if let Some(rest) = host_port.strip_prefix('[') {
        let (host, port) = rest.split_once(']')?;
        if host.is_empty() {
            return None;
        }
        let port = if let Some(port) = port.strip_prefix(':') {
            if port.is_empty() {
                return None;
            }
            Some(port.parse().ok()?)
        } else if port.is_empty() {
            None
        } else {
            return None;
        };
        return Some((host, port));
    }
    if host_port.matches(':').count() > 1 {
        return None;
    }
    match host_port.split_once(':') {
        Some((host, port)) if !host.is_empty() && !port.is_empty() => {
            Some((host, Some(port.parse().ok()?)))
        }
        Some(_) => None,
        None => Some((host_port, None)),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::{
        bumped_interval, is_allowed_https_url, parse_server_base, resolve_server_base,
        workspace_url, AlertItem, DesktopApi, DeviceCodeResponse, DeviceTokenResponse,
        ImmediateSeverity, MockOrbitClient, ScheduleOwner, Severity, ACCESS_TOKEN_LIFETIME_SECONDS,
        DEFAULT_SERVER_BASE, DEVICE_CODE_TTL_SECONDS, ENV_SERVER_URL, PATH_DESKTOP_ALERTS,
        PATH_DESKTOP_STATUS, PATH_DEVICE_APPROVE, PATH_DEVICE_CODE, PATH_DEVICE_TOKEN,
        WORKSPACE_FINDINGS, WORKSPACE_FINDINGS_BY_SEVERITY, WORKSPACE_NEEDS_ATTENTION,
    };

    #[tokio::test(flavor = "current_thread")]
    async fn mock_returns_one_page_then_nothing_new() {
        let client = MockOrbitClient::example();
        let page = client.desktop_alerts("").await.unwrap();
        assert_eq!(page.alerts.len(), 1);
        assert_eq!(page.alerts[0].severity, Severity::Medium);
        assert!(!page.alerts[0].immediate);
        assert_eq!(
            page.alerts[0].workspace_path,
            WORKSPACE_FINDINGS_BY_SEVERITY
        );
        assert_eq!(
            workspace_url(DEFAULT_SERVER_BASE, &page.alerts[0].workspace_path).as_deref(),
            Some("https://helloorbit.app/rust-preview/pqc/studio/app/findings/by-severity")
        );
        let again = client.desktop_alerts("").await.unwrap();
        assert_eq!(again.alerts.len(), 1);
        let status = client.desktop_status("").await.unwrap();
        assert_eq!(status.schedule_owner, ScheduleOwner::Client);
        assert_eq!(status.immediate_severity, ImmediateSeverity::High);
        assert_eq!(status.workspace_path, WORKSPACE_FINDINGS);
        assert_eq!(status.observation_count, 1);
    }

    #[test]
    fn routes_match_the_orbit_device_sign_in_paths() {
        assert_eq!(PATH_DEVICE_CODE, "/api/pqc/v1/account/device/code");
        assert_eq!(PATH_DEVICE_TOKEN, "/api/pqc/v1/account/device/token");
        assert_eq!(PATH_DEVICE_APPROVE, "/api/pqc/v1/account/device/approve");
        assert_eq!(PATH_DESKTOP_STATUS, "/api/pqc/v1/desktop/status");
        assert_eq!(PATH_DESKTOP_ALERTS, "/api/pqc/v1/desktop/alerts");
        assert_eq!(ENV_SERVER_URL, "STARDUST_SERVER_URL");
        assert_eq!(DEVICE_CODE_TTL_SECONDS, 600);
        assert_eq!(ACCESS_TOKEN_LIFETIME_SECONDS, 300);
    }

    #[test]
    fn token_response_debug_redacts_secrets() {
        let response = DeviceTokenResponse {
            token: "secret-access".to_string(),
            access_token: "secret-access".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 300,
            expires_at_epoch_ms: 1_700_000_000_000,
            refresh_token: Some("secret-refresh".to_string()),
            refresh_expires_in: Some(86_400),
        };
        let rendered = format!("{response:?}");
        assert!(!rendered.contains("secret-access"));
        assert!(!rendered.contains("secret-refresh"));
        let request = super::RefreshTokenRequest {
            grant_type: super::REFRESH_GRANT_TYPE.to_string(),
            refresh_token: "secret-refresh".to_string(),
        };
        assert!(!format!("{request:?}").contains("secret-refresh"));
        let without: DeviceTokenResponse = serde_json::from_str(
            r#"{"token":"a","access_token":"a","token_type":"Bearer","expires_in":300,"expires_at_epoch_ms":1}"#,
        )
        .unwrap();
        assert_eq!(without.refresh_token, None);
        let with: DeviceTokenResponse = serde_json::from_str(
            r#"{"token":"a","access_token":"a","token_type":"Bearer","expires_in":300,"expires_at_epoch_ms":1,"refresh_token":"r","refresh_expires_in":60}"#,
        )
        .unwrap();
        assert_eq!(with.refresh_token.as_deref(), Some("r"));
        assert_eq!(with.refresh_expires_in, Some(60));
        assert!(serde_json::from_str::<DeviceTokenResponse>(
            r#"{"token":"a","access_token":"a","token_type":"Bearer","expires_in":300,"expires_at_epoch_ms":1,"scope":"x"}"#,
        )
        .is_err());
        let code = DeviceCodeResponse {
            device_code: "secret-device".to_string(),
            user_code: "ABCD-EFGH".to_string(),
            verification_uri: "https://helloorbit.app/account".to_string(),
            verification_uri_complete: "https://helloorbit.app/account?user_code=ABCD-EFGH"
                .to_string(),
            expires_in: 600,
            interval: 5,
        };
        let rendered = format!("{code:?}");
        assert!(!rendered.contains("secret-device"));
        assert!(rendered.contains("ABCD-EFGH"));
        let high = AlertItem {
            id: "h".to_string(),
            severity: Severity::High,
            title: "high".to_string(),
            created_at: chrono::Utc.with_ymd_and_hms(2026, 10, 26, 0, 0, 0).unwrap(),
            workspace_path: WORKSPACE_NEEDS_ATTENTION.to_string(),
            immediate: true,
        };
        assert!(high.shows_immediately());
        assert_eq!(high.severity, Severity::High);
        assert_ne!(Severity::Medium, Severity::High);
    }

    #[test]
    fn server_base_and_workspace_links_stay_on_the_configured_https_host() {
        assert_eq!(
            resolve_server_base(None, None).unwrap(),
            DEFAULT_SERVER_BASE
        );
        assert_eq!(
            resolve_server_base(Some("https://helloorbit.app/"), Some("  ")).unwrap(),
            DEFAULT_SERVER_BASE
        );
        assert_eq!(
            resolve_server_base(Some("https://helloorbit.app"), Some("http://127.0.0.1:9"))
                .unwrap(),
            "http://127.0.0.1:9"
        );
        assert!(parse_server_base("https://user:pass@helloorbit.app").is_err());
        assert!(parse_server_base("https://helloorbit.app/workspace").is_err());
        assert!(parse_server_base("helloorbit.app").is_err());

        assert_eq!(
            workspace_url(DEFAULT_SERVER_BASE, "/workspace/high").as_deref(),
            Some("https://helloorbit.app/workspace/high")
        );
        assert_eq!(workspace_url("http://127.0.0.1:9", "/workspace/high"), None);
        for rejected in [
            "https://helloorbit.app/workspace/high",
            "http://helloorbit.app/workspace/high",
            "//helloorbit.app/workspace/high",
            "/workspace/../secret",
            "/workspace/./high",
            "javascript:open(1)",
            "/workspace/high\\evil",
            "/workspace/%2e%2e/secret",
            "https://evil.example/workspace/high",
        ] {
            assert_eq!(
                workspace_url(DEFAULT_SERVER_BASE, rejected),
                None,
                "{rejected}"
            );
        }
        assert!(is_allowed_https_url(
            DEFAULT_SERVER_BASE,
            "https://helloorbit.app/workspace/high"
        ));
        assert!(is_allowed_https_url(
            DEFAULT_SERVER_BASE,
            "https://HelloOrbit.app:443/account?user_code=ABCD-EFGH"
        ));
        assert!(!is_allowed_https_url(
            DEFAULT_SERVER_BASE,
            "https://evil.example/workspace/high"
        ));
        assert!(!is_allowed_https_url(
            DEFAULT_SERVER_BASE,
            "http://helloorbit.app/workspace/high"
        ));
        assert!(!is_allowed_https_url(
            "http://127.0.0.1:9",
            "https://127.0.0.1/workspace/high"
        ));
        for control in [
            "https://helloorbit.app/account?user_code=AB\tCD",
            "https://helloorbit.app/account\r\nX-Other: 1",
            "https://helloorbit.app/account\u{7f}",
        ] {
            assert!(
                !is_allowed_https_url(DEFAULT_SERVER_BASE, control),
                "{control:?}"
            );
        }
        let padded = super::DeviceCodeResponse {
            device_code: "a".repeat(64),
            user_code: "ABCD-EFGH".to_string(),
            verification_uri: "https://helloorbit.app/account".to_string(),
            verification_uri_complete: " https://helloorbit.app/account?user_code=ABCD-EFGH\n"
                .to_string(),
            expires_in: 600,
            interval: 5,
        };
        assert_eq!(
            super::verification_url(DEFAULT_SERVER_BASE, &padded).as_deref(),
            Some("https://helloorbit.app/account?user_code=ABCD-EFGH"),
            "the page that opens is the trimmed one"
        );
        let request = super::DeviceTokenRequest {
            grant_type: "urn:ietf:params:oauth:grant-type:device_code".to_string(),
            device_code: "b".repeat(64),
        };
        assert!(!format!("{request:?}").contains(&"b".repeat(64)));
        assert_eq!(bumped_interval(5, false), 5);
        assert_eq!(bumped_interval(5, true), 10);
        assert_eq!(bumped_interval(28, true), 30);
        assert_eq!(bumped_interval(0, false), 1);
    }

    #[test]
    fn plain_http_server_base_is_loopback_only() {
        assert!(parse_server_base("http://127.0.0.1:9").is_ok());
        assert!(parse_server_base("http://localhost:8080").is_ok());
        assert!(parse_server_base("http://[::1]:8080").is_ok());
        assert!(parse_server_base("http://helloorbit.app").is_err());
        assert!(parse_server_base("http://10.0.0.5:8080").is_err());
        assert!(parse_server_base("https://helloorbit.app").is_ok());
    }
}
