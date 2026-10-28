//! One scheduled or user-requested check.
//!
//! The alerts endpoint is called once. The next poll stays on the schedule
//! even when the check fails or a high-severity item is present.

use crate::api::{AlertItem, AlertsPage, OrbitClient, OrbitError};
use crate::policy::{present, Presentation};
use crate::schedule::{FireSource, Schedule};
use chrono::{DateTime, Utc};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckKind {
    Manual,
    Scheduled {
        slot: DateTime<Utc>,
        source: FireSource,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckOutcome {
    pub presentation: Presentation,
    pub next_cursor: Option<String>,
    pub items: Vec<AlertItem>,
}

pub async fn perform_check<C: OrbitClient + Sync>(
    client: &C,
    cursor: Option<&str>,
    now_utc: DateTime<Utc>,
    schedule: &Schedule,
    kind: &CheckKind,
) -> Result<CheckOutcome, OrbitError> {
    let page = client.desktop_alerts(cursor).await?;
    let next_cursor = page.next_cursor.clone();
    let items = deduped(page);
    let local = now_utc.with_timezone(&schedule.timezone());
    let quiet = schedule.is_quiet_at(local);
    let force_bubble = matches!(kind, CheckKind::Manual);
    let presentation = present(&items, quiet, local, force_bubble);
    Ok(CheckOutcome {
        presentation,
        next_cursor,
        items,
    })
}

fn deduped(page: AlertsPage) -> Vec<AlertItem> {
    let mut seen = BTreeSet::new();
    page.items
        .into_iter()
        .filter(|item| !item.id.is_empty() && seen.insert(item.id.clone()))
        .collect()
}

pub fn note_anchor(state: &mut crate::config::CheckState, kind: &CheckKind) {
    if let CheckKind::Scheduled { slot, source } = kind {
        if source.updates_interval_anchor() {
            state.interval_anchor = Some(*slot);
        }
    }
}

pub fn note_success(
    state: &mut crate::config::CheckState,
    now: DateTime<Utc>,
    cursor: Option<String>,
) {
    state.last_checked = Some(now);
    state.alerts_cursor = cursor;
}

#[cfg(test)]
mod tests {
    use super::{note_anchor, note_success, perform_check, CheckKind};
    use crate::api::{
        AlertItem, AlertsPage, DesktopStatus, DeviceAuthorizeRequest, DeviceAuthorizeResponse,
        DeviceTokenRequest, DeviceTokenResponse, OrbitClient, OrbitError, RevokeSessionRequest,
        Severity,
    };
    use crate::config::CheckState;
    use crate::schedule::{parse_quiet, parse_weekdays, FireSource, Schedule};
    use chrono::{TimeZone, Utc};
    use chrono_tz::UTC;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Script {
        pages: std::sync::Mutex<Vec<AlertsPage>>,
        alerts_calls: AtomicUsize,
    }

    impl OrbitClient for Script {
        async fn desktop_status(&self) -> Result<DesktopStatus, OrbitError> {
            Ok(DesktopStatus {
                last_scan_time: None,
                counts_by_severity: BTreeMap::new(),
            })
        }

        async fn desktop_alerts(&self, _since: Option<&str>) -> Result<AlertsPage, OrbitError> {
            self.alerts_calls.fetch_add(1, Ordering::SeqCst);
            self.pages
                .lock()
                .unwrap()
                .pop()
                .ok_or(OrbitError::ProposedEndpoint { path: "/unused" })
        }

        async fn device_authorize(
            &self,
            _: &DeviceAuthorizeRequest,
        ) -> Result<DeviceAuthorizeResponse, OrbitError> {
            Err(OrbitError::ProposedEndpoint { path: "/unused" })
        }

        async fn device_token(
            &self,
            _: &DeviceTokenRequest,
        ) -> Result<DeviceTokenResponse, OrbitError> {
            Err(OrbitError::ProposedEndpoint { path: "/unused" })
        }

        async fn revoke_session(&self, _: &RevokeSessionRequest) -> Result<(), OrbitError> {
            Err(OrbitError::ProposedEndpoint { path: "/unused" })
        }
    }

    fn schedule(quiet: bool) -> Schedule {
        Schedule::try_new(
            Some(60),
            Vec::new(),
            parse_weekdays("all").unwrap(),
            quiet.then(|| parse_quiet("00:00-23:59").unwrap()),
            UTC,
        )
        .unwrap()
    }

    fn high_page() -> AlertsPage {
        AlertsPage {
            items: vec![AlertItem {
                id: "h1".to_string(),
                severity: Severity::High,
                title: "Example high connection".to_string(),
                first_seen: Utc.with_ymd_and_hms(2026, 10, 21, 14, 0, 0).unwrap(),
                link: "https://helloorbit.app/workspace/high".to_string(),
            }],
            next_cursor: Some("cursor-2".to_string()),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn scheduled_check_calls_alerts_once_and_shows_high_severity_now() {
        let client = Script {
            pages: std::sync::Mutex::new(vec![high_page()]),
            alerts_calls: AtomicUsize::new(0),
        };
        let now = Utc.with_ymd_and_hms(2026, 10, 21, 14, 5, 0).unwrap();
        let outcome = perform_check(
            &client,
            None,
            now,
            &schedule(true),
            &CheckKind::Scheduled {
                slot: now,
                source: FireSource::Interval,
            },
        )
        .await
        .unwrap();
        assert_eq!(client.alerts_calls.load(Ordering::SeqCst), 1);
        assert!(outcome.presentation.show_bubble);
        assert!(outcome.presentation.show_os_notification);
        assert_eq!(outcome.items.len(), 1);
        assert_eq!(outcome.next_cursor.as_deref(), Some("cursor-2"));
    }

    #[test]
    fn anchor_moves_only_for_a_scheduled_interval() {
        let slot = Utc.with_ymd_and_hms(2026, 10, 21, 11, 0, 0).unwrap();
        let mut state = CheckState::default();
        note_anchor(
            &mut state,
            &CheckKind::Scheduled {
                slot,
                source: FireSource::Clock,
            },
        );
        note_success(&mut state, slot, Some("cursor".to_string()));
        assert_eq!(state.interval_anchor, None);
        assert_eq!(state.alerts_cursor.as_deref(), Some("cursor"));

        note_anchor(&mut state, &CheckKind::Manual);
        assert_eq!(state.interval_anchor, None);

        note_anchor(
            &mut state,
            &CheckKind::Scheduled {
                slot,
                source: FireSource::Interval,
            },
        );
        assert_eq!(state.interval_anchor, Some(slot));
    }
}
