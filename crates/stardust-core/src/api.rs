//! Proposed ORBIT desktop API.
//!
//! These endpoints are not available on ORBIT yet. [`MockOrbitClient`] is the
//! only backend this preview runs. The placeholder origin is
//! `https://helloorbit.app`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

pub const PLACEHOLDER_ORIGIN: &str = "https://helloorbit.app";
pub const PATH_DEVICE_AUTHORIZE: &str = "/api/pqc/v1/device/authorize";
pub const PATH_DEVICE_TOKEN: &str = "/api/pqc/v1/device/token";
pub const PATH_DESKTOP_STATUS: &str = "/api/pqc/v1/desktop/status";
pub const PATH_DESKTOP_ALERTS: &str = "/api/pqc/v1/desktop/alerts";
pub const PATH_DEVICE_SESSION: &str = "/api/pqc/v1/device/sessions";
pub const PROPOSED_SCOPE: &str = "alerts:read status:read";
pub const PROPOSED_CLIENT_ID: &str = "stardust-preview";
pub const DEVICE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OrbitError {
    #[error("proposed endpoint is not available yet: {path}")]
    ProposedEndpoint { path: &'static str },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn is_high_or_above(self) -> bool {
        matches!(self, Self::High | Self::Critical)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopStatus {
    pub last_scan_time: Option<DateTime<Utc>>,
    pub counts_by_severity: BTreeMap<Severity, u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertItem {
    pub id: String,
    pub severity: Severity,
    pub title: String,
    pub first_seen: DateTime<Utc>,
    pub link: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertsPage {
    pub items: Vec<AlertItem>,
    pub next_cursor: Option<String>,
}

/// Proposed `POST /api/pqc/v1/device/authorize` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceAuthorizeRequest {
    pub client_id: String,
    pub scope: String,
}

/// Proposed device-authorization response. Field names follow RFC 8628.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceAuthorizeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// Proposed `POST /api/pqc/v1/device/token` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceTokenRequest {
    pub grant_type: String,
    pub device_code: String,
    pub client_id: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub scope: String,
}

impl fmt::Debug for DeviceTokenResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceTokenResponse")
            .field("access_token", &"<redacted>")
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("scope", &self.scope)
            .finish()
    }
}

/// Proposed `DELETE /api/pqc/v1/device/sessions/{id}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeSessionRequest {
    pub session_id: String,
}

pub trait OrbitClient: Send + Sync {
    fn desktop_status(&self) -> impl Future<Output = Result<DesktopStatus, OrbitError>> + Send;
    fn desktop_alerts<'a>(
        &'a self,
        since: Option<&'a str>,
    ) -> impl Future<Output = Result<AlertsPage, OrbitError>> + Send;
    fn device_authorize<'a>(
        &'a self,
        request: &'a DeviceAuthorizeRequest,
    ) -> impl Future<Output = Result<DeviceAuthorizeResponse, OrbitError>> + Send;
    fn device_token<'a>(
        &'a self,
        request: &'a DeviceTokenRequest,
    ) -> impl Future<Output = Result<DeviceTokenResponse, OrbitError>> + Send;
    fn revoke_session<'a>(
        &'a self,
        request: &'a RevokeSessionRequest,
    ) -> impl Future<Output = Result<(), OrbitError>> + Send;
}

#[derive(Debug)]
pub struct MockOrbitClient {
    status: DesktopStatus,
    first_page: AlertsPage,
    alerts_calls: AtomicUsize,
    status_calls: AtomicUsize,
    authorize_calls: AtomicUsize,
    token_calls: AtomicUsize,
    revoke_calls: AtomicUsize,
    last_since: Mutex<Option<String>>,
}

impl MockOrbitClient {
    pub fn example() -> Self {
        let seen = DateTime::parse_from_rfc3339("2026-10-21T00:00:00Z")
            .expect("fixed example timestamp")
            .with_timezone(&Utc);
        let mut counts = BTreeMap::new();
        counts.insert(Severity::Medium, 1);
        Self {
            status: DesktopStatus {
                last_scan_time: Some(seen),
                counts_by_severity: counts,
            },
            first_page: AlertsPage {
                items: vec![AlertItem {
                    id: "example-1".to_string(),
                    severity: Severity::Medium,
                    title: "Example connection finding".to_string(),
                    first_seen: seen,
                    link: format!("{PLACEHOLDER_ORIGIN}/workspace/example"),
                }],
                next_cursor: Some("mock-1".to_string()),
            },
            alerts_calls: AtomicUsize::new(0),
            status_calls: AtomicUsize::new(0),
            authorize_calls: AtomicUsize::new(0),
            token_calls: AtomicUsize::new(0),
            revoke_calls: AtomicUsize::new(0),
            last_since: Mutex::new(None),
        }
    }

    pub fn alerts_calls(&self) -> usize {
        self.alerts_calls.load(Ordering::SeqCst)
    }

    pub fn status_calls(&self) -> usize {
        self.status_calls.load(Ordering::SeqCst)
    }
}

impl OrbitClient for MockOrbitClient {
    async fn desktop_status(&self) -> Result<DesktopStatus, OrbitError> {
        self.status_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.status.clone())
    }

    async fn desktop_alerts(&self, since: Option<&str>) -> Result<AlertsPage, OrbitError> {
        self.alerts_calls.fetch_add(1, Ordering::SeqCst);
        *self
            .last_since
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = since.map(str::to_string);
        if since == self.first_page.next_cursor.as_deref() {
            return Ok(AlertsPage {
                items: Vec::new(),
                next_cursor: self.first_page.next_cursor.clone(),
            });
        }
        Ok(self.first_page.clone())
    }

    async fn device_authorize(
        &self,
        _request: &DeviceAuthorizeRequest,
    ) -> Result<DeviceAuthorizeResponse, OrbitError> {
        self.authorize_calls.fetch_add(1, Ordering::SeqCst);
        Err(OrbitError::ProposedEndpoint {
            path: PATH_DEVICE_AUTHORIZE,
        })
    }

    async fn device_token(
        &self,
        _request: &DeviceTokenRequest,
    ) -> Result<DeviceTokenResponse, OrbitError> {
        self.token_calls.fetch_add(1, Ordering::SeqCst);
        Err(OrbitError::ProposedEndpoint {
            path: PATH_DEVICE_TOKEN,
        })
    }

    async fn revoke_session(&self, _request: &RevokeSessionRequest) -> Result<(), OrbitError> {
        self.revoke_calls.fetch_add(1, Ordering::SeqCst);
        Err(OrbitError::ProposedEndpoint {
            path: PATH_DEVICE_SESSION,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DeviceAuthorizeRequest, DeviceTokenRequest, DeviceTokenResponse, MockOrbitClient,
        OrbitClient, OrbitError, RevokeSessionRequest, Severity, DEVICE_GRANT_TYPE,
        PATH_DESKTOP_ALERTS, PATH_DESKTOP_STATUS, PATH_DEVICE_AUTHORIZE, PATH_DEVICE_SESSION,
        PATH_DEVICE_TOKEN, PLACEHOLDER_ORIGIN, PROPOSED_CLIENT_ID, PROPOSED_SCOPE,
    };

    #[tokio::test(flavor = "current_thread")]
    async fn mock_returns_one_page_then_nothing_new() {
        let client = MockOrbitClient::example();
        let page = client.desktop_alerts(None).await.unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].severity, Severity::Medium);
        assert!(page.items[0]
            .link
            .starts_with(&format!("{PLACEHOLDER_ORIGIN}/")));
        let next = page.next_cursor.clone();
        let again = client.desktop_alerts(next.as_deref()).await.unwrap();
        assert!(again.items.is_empty());
        assert_eq!(client.alerts_calls(), 2);

        let status = client.desktop_status().await.unwrap();
        assert_eq!(status.counts_by_severity.get(&Severity::Medium), Some(&1));
        assert_eq!(client.status_calls(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn device_flow_is_proposed_and_not_implemented() {
        let client = MockOrbitClient::example();
        let authorize = client
            .device_authorize(&DeviceAuthorizeRequest {
                client_id: PROPOSED_CLIENT_ID.to_string(),
                scope: PROPOSED_SCOPE.to_string(),
            })
            .await;
        assert_eq!(
            authorize.unwrap_err(),
            OrbitError::ProposedEndpoint {
                path: PATH_DEVICE_AUTHORIZE
            }
        );
        let token = client
            .device_token(&DeviceTokenRequest {
                grant_type: DEVICE_GRANT_TYPE.to_string(),
                device_code: "not-issued".to_string(),
                client_id: PROPOSED_CLIENT_ID.to_string(),
            })
            .await;
        assert_eq!(
            token.unwrap_err(),
            OrbitError::ProposedEndpoint {
                path: PATH_DEVICE_TOKEN
            }
        );
        let revoke = client
            .revoke_session(&RevokeSessionRequest {
                session_id: "not-issued".to_string(),
            })
            .await;
        assert_eq!(
            revoke.unwrap_err(),
            OrbitError::ProposedEndpoint {
                path: PATH_DEVICE_SESSION
            }
        );
        assert_eq!(PATH_DESKTOP_STATUS, "/api/pqc/v1/desktop/status");
        assert_eq!(PATH_DESKTOP_ALERTS, "/api/pqc/v1/desktop/alerts");
    }

    #[test]
    fn token_response_debug_redacts_secrets() {
        let response = DeviceTokenResponse {
            access_token: "secret-access".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 900,
            refresh_token: Some("secret-refresh".to_string()),
            scope: PROPOSED_SCOPE.to_string(),
        };
        let rendered = format!("{response:?}");
        assert!(!rendered.contains("secret-access"));
        assert!(!rendered.contains("secret-refresh"));
        assert!(!Severity::Medium.is_high_or_above());
        assert!(Severity::High.is_high_or_above());
        assert!(Severity::Critical.is_high_or_above());
    }
}
