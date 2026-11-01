//! One scheduled or user-requested check.
//!
//! The alerts endpoint is called once. The next poll stays on the schedule
//! even when the check fails or a high-severity item is present.

use crate::api::{AlertItem, AlertsPage, DesktopApi, OrbitError};
use crate::policy::{present, Presentation};
use crate::schedule::{FireSource, Schedule};
use chrono::{DateTime, Utc};
use std::collections::BTreeSet;
use std::time::Duration;

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
    pub items: Vec<AlertItem>,
    /// The status route's workspace page, the link when no item has one.
    pub workspace_path: String,
}

pub async fn perform_check<C: DesktopApi + Sync>(
    client: &C,
    access_token: &str,
    now_utc: DateTime<Utc>,
    schedule: &Schedule,
    kind: &CheckKind,
    server_base: &str,
) -> Result<CheckOutcome, OrbitError> {
    let status = client.desktop_status(access_token).await?;
    let page = client.desktop_alerts(access_token).await?;
    let items = deduped(page);
    let local = now_utc.with_timezone(&schedule.timezone());
    let quiet = schedule.is_quiet_at(local);
    let force_bubble = matches!(kind, CheckKind::Manual);
    let presentation = present(
        &items,
        quiet,
        local,
        force_bubble,
        server_base,
        &status.workspace_path,
    );
    Ok(CheckOutcome {
        presentation,
        items,
        workspace_path: status.workspace_path,
    })
}

fn deduped(page: AlertsPage) -> Vec<AlertItem> {
    let mut seen = BTreeSet::new();
    page.alerts
        .into_iter()
        .filter(|item| seen.insert(item.id.clone()))
        .collect()
}

pub fn note_anchor(state: &mut crate::config::CheckState, kind: &CheckKind) {
    if let CheckKind::Scheduled { slot, source } = kind {
        if source.updates_interval_anchor() {
            state.interval_anchor = Some(*slot);
        }
    }
}

/// Wait at least the schedule, and longer after a soft failure.
///
/// A zero delay becomes one second so a slot that never moves cannot spin.
pub fn retry_delay(schedule_delay: Duration, failures: u32) -> Duration {
    let backoff = if failures == 0 {
        Duration::ZERO
    } else {
        let shift = failures.saturating_sub(1).min(6);
        Duration::from_secs(30_u64 << shift)
    };
    let delay = schedule_delay.max(backoff);
    if delay.is_zero() {
        Duration::from_secs(1)
    } else {
        delay
    }
}

/// Longest wait [`full_jitter`] picks.
pub const BACKOFF_CAP: Duration = Duration::from_secs(15 * 60);
/// First backoff ceiling. It doubles after each further soft failure.
pub const BACKOFF_BASE: Duration = Duration::from_secs(30);

/// Exponential backoff with full jitter: a wait picked evenly from zero up to
/// `min(BACKOFF_CAP, BACKOFF_BASE * 2^(failures - 1))`.
///
/// `entropy` is any random number; tests pass fixed values.
pub fn full_jitter(failures: u32, entropy: u64) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    let shift = failures.saturating_sub(1).min(16);
    let base = u64::try_from(BACKOFF_BASE.as_millis()).unwrap_or(u64::MAX);
    let cap = u64::try_from(BACKOFF_CAP.as_millis()).unwrap_or(u64::MAX);
    let ceiling = base.saturating_mul(1_u64 << shift).min(cap);
    Duration::from_millis(entropy % (ceiling + 1))
}

/// [`retry_delay`] with [`full_jitter`] in place of the fixed doubling.
///
/// Still waits at least the schedule, so a failure never brings a check
/// forward, and anything under a second becomes one second so nothing
/// spins.
pub fn jittered_retry_delay(schedule_delay: Duration, failures: u32, entropy: u64) -> Duration {
    schedule_delay
        .max(full_jitter(failures, entropy))
        .max(Duration::from_secs(1))
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
    use super::{note_anchor, note_success, perform_check, retry_delay, CheckKind};
    use crate::api::{
        AlertItem, AlertsPage, DesktopApi, DesktopStatus, ImmediateSeverity, OrbitError,
        ScheduleOwner, Severity, DEFAULT_SERVER_BASE, WORKSPACE_FINDINGS,
        WORKSPACE_NEEDS_ATTENTION,
    };
    use crate::config::CheckState;
    use crate::schedule::{parse_quiet, parse_weekdays, FireSource, Schedule};
    use chrono::{TimeZone, Utc};
    use chrono_tz::UTC;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct Script {
        pages: std::sync::Mutex<Vec<AlertsPage>>,
        status: std::sync::Mutex<Result<(), OrbitError>>,
        alerts_calls: AtomicUsize,
        status_calls: AtomicUsize,
    }

    impl DesktopApi for Script {
        fn desktop_status(
            &self,
            _: &str,
        ) -> impl std::future::Future<Output = Result<DesktopStatus, OrbitError>> + Send {
            self.status_calls.fetch_add(1, Ordering::SeqCst);
            let result = self.status.lock().unwrap().clone().map(|()| DesktopStatus {
                organization_id: "00000000-0000-4000-8000-000000000001".to_string(),
                observation_count: 0,
                alert_count: 0,
                high_severity_count: 0,
                immediate_severity: ImmediateSeverity::High,
                schedule_owner: ScheduleOwner::Client,
                workspace_path: WORKSPACE_FINDINGS.to_string(),
                as_of_unix: 1_761_436_800,
            });
            async move { result }
        }

        fn desktop_alerts(
            &self,
            _: &str,
        ) -> impl std::future::Future<Output = Result<AlertsPage, OrbitError>> + Send {
            self.alerts_calls.fetch_add(1, Ordering::SeqCst);
            let page = self.pages.lock().unwrap().pop().ok_or(OrbitError::Response);
            async move { page }
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
            alerts: vec![AlertItem {
                id: "h1".to_string(),
                severity: Severity::High,
                title: "Example high connection".to_string(),
                created_at: Utc.with_ymd_and_hms(2026, 10, 21, 14, 0, 0).unwrap(),
                workspace_path: WORKSPACE_NEEDS_ATTENTION.to_string(),
                immediate: true,
            }],
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn scheduled_check_calls_alerts_once_and_shows_high_severity_now() {
        let client = Script {
            pages: std::sync::Mutex::new(vec![high_page()]),
            status: std::sync::Mutex::new(Ok(())),
            alerts_calls: AtomicUsize::new(0),
            status_calls: AtomicUsize::new(0),
        };
        let now = Utc.with_ymd_and_hms(2026, 10, 21, 14, 5, 0).unwrap();
        let outcome = perform_check(
            &client,
            "example-access",
            now,
            &schedule(true),
            &CheckKind::Scheduled {
                slot: now,
                source: FireSource::Interval,
            },
            DEFAULT_SERVER_BASE,
        )
        .await
        .unwrap();
        assert_eq!(client.status_calls.load(Ordering::SeqCst), 1);
        assert_eq!(client.alerts_calls.load(Ordering::SeqCst), 1);
        assert!(outcome.presentation.show_bubble);
        assert!(outcome.presentation.show_os_notification);
        assert_eq!(outcome.items.len(), 1);
        assert_eq!(
            outcome.presentation.summary.details_url,
            "https://helloorbit.app/rust-preview/pqc/studio/app/control-room/needs-attention"
        );
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

    #[tokio::test(flavor = "current_thread")]
    async fn a_turned_off_server_does_not_ask_for_alerts() {
        let client = Script {
            pages: std::sync::Mutex::new(vec![high_page()]),
            status: std::sync::Mutex::new(Err(OrbitError::FeatureOff)),
            alerts_calls: AtomicUsize::new(0),
            status_calls: AtomicUsize::new(0),
        };
        let now = Utc.with_ymd_and_hms(2026, 10, 21, 14, 5, 0).unwrap();
        let err = perform_check(
            &client,
            "example-access",
            now,
            &schedule(false),
            &CheckKind::Manual,
            DEFAULT_SERVER_BASE,
        )
        .await
        .unwrap_err();
        assert_eq!(err, OrbitError::FeatureOff);
        assert_eq!(client.alerts_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            retry_delay(Duration::from_secs(60), 0),
            Duration::from_secs(60)
        );
        assert_eq!(retry_delay(Duration::ZERO, 0), Duration::from_secs(1));
        assert_eq!(
            retry_delay(Duration::from_secs(60), 1),
            Duration::from_secs(60)
        );
        assert_eq!(
            retry_delay(Duration::from_secs(1), 1),
            Duration::from_secs(30)
        );
        assert_eq!(
            retry_delay(Duration::from_secs(1), 7),
            Duration::from_secs(1920)
        );
    }

    #[test]
    fn full_jitter_stays_under_a_doubling_ceiling_capped_at_fifteen_minutes() {
        use super::{full_jitter, jittered_retry_delay, BACKOFF_CAP};
        assert_eq!(full_jitter(0, u64::MAX), Duration::ZERO);
        assert_eq!(full_jitter(1, 0), Duration::ZERO);
        assert_eq!(full_jitter(1, 30_000), Duration::from_secs(30));
        assert_eq!(full_jitter(1, 30_001), Duration::ZERO);
        assert_eq!(full_jitter(2, 60_000), Duration::from_secs(60));
        assert_eq!(full_jitter(3, 120_000), Duration::from_secs(120));
        assert_eq!(
            full_jitter(5, 480_000),
            Duration::from_secs(480),
            "30 s doubled four times is still under the cap"
        );
        for failures in [6, 10, 32, u32::MAX] {
            assert_eq!(full_jitter(failures, 900_000), BACKOFF_CAP);
            for entropy in [0, 1, 12_345, 899_999, 900_001, u64::MAX] {
                assert!(full_jitter(failures, entropy) <= BACKOFF_CAP);
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for entropy in 0..64_u64 {
            seen.insert(full_jitter(4, entropy.wrapping_mul(0x9e37_79b9_7f4a_7c15)));
        }
        assert!(seen.len() > 32, "picks should spread out");
        assert_eq!(
            jittered_retry_delay(Duration::from_secs(60), 0, 7),
            Duration::from_secs(60)
        );
        assert_eq!(
            jittered_retry_delay(Duration::ZERO, 1, 0),
            Duration::from_secs(1)
        );
        assert_eq!(
            jittered_retry_delay(Duration::ZERO, 1, 5),
            Duration::from_secs(1),
            "a few milliseconds of jitter still wait a second"
        );
        assert_eq!(
            jittered_retry_delay(Duration::from_secs(60), 3, 120_000),
            Duration::from_secs(120)
        );
        assert_eq!(
            jittered_retry_delay(Duration::from_secs(3_600), 9, 900_000),
            Duration::from_secs(3_600)
        );
    }
}
