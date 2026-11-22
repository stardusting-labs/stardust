//! Which alert goes to which sink, and only once.
//!
//! Used only when routing is on. Each check hands the router the alerts it
//! saw. Alert ids are kept in memory in a bounded list, least recently seen
//! first out, with one bit per sink that already has the alert. A sink gets
//! an alert once, and again if the same id comes back with a higher
//! severity. A send that failed in a way worth retrying, a quiet-hours or
//! do-not-disturb hold, and a rate-limit wait leave the bit clear, so a later
//! check that still lists the alert sends it then. The list is in memory
//! only, so after a restart every listed alert counts as new.

use crate::api::Severity;
use crate::integrations::IntegrationRecord;
use crate::sink::{Alert, Sink, SinkError, SinkKind};
use std::collections::VecDeque;
use std::time::Instant;

/// Alert ids remembered at once. One check that lists more keeps them all
/// until the next check.
pub const SEEN_CAPACITY: usize = 256;
/// Messages an hour for a sink that does not set its own limit.
pub const DEFAULT_MAX_PER_HOUR: u32 = 30;
/// Most messages one remote sink sends from a single check. The rest wait
/// for the next check. [`RouteContext::post_until`] bounds the time as well.
pub const MAX_PER_CHECK: usize = 20;
const MAX_SLOTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SinkRule {
    pub min_severity: Severity,
    /// `None` means no hourly limit.
    pub max_per_hour: Option<u32>,
    /// A high-severity alert still goes out during quiet hours.
    pub quiet_high_bypass: bool,
    /// The bubble and the OS notification on this computer. They stay quiet
    /// while the OS do-not-disturb switch is on, for every severity.
    pub local: bool,
}

impl SinkRule {
    /// Same as the bubble without routing: every item, and a high one
    /// during quiet hours.
    pub const BUBBLE: Self = Self {
        min_severity: Severity::Low,
        max_per_hour: None,
        quiet_high_bypass: true,
        local: true,
    };
    /// Same as the OS notification without routing: high items only.
    pub const NOTIFICATION: Self = Self {
        min_severity: Severity::High,
        max_per_hour: None,
        quiet_high_bypass: true,
        local: true,
    };

    /// The rule for an integration, or `None` for a kind that is not a sink.
    pub fn for_record(record: &IntegrationRecord) -> Option<Self> {
        Some(Self::for_sink(record, SinkKind::of(record.kind)?))
    }

    /// The rule for an integration routed as `kind`.
    pub fn for_sink(record: &IntegrationRecord, kind: SinkKind) -> Self {
        Self {
            min_severity: record
                .min_severity
                .unwrap_or_else(|| kind.default_min_severity()),
            max_per_hour: Some(record.max_per_hour.unwrap_or(DEFAULT_MAX_PER_HOUR)),
            quiet_high_bypass: record.quiet_high_bypass.unwrap_or(true),
            local: false,
        }
    }
}

pub struct Route<'a> {
    pub id: &'a str,
    pub rule: SinkRule,
    pub sink: &'a dyn Sink,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteContext {
    /// Inside the schedule's quiet hours, in its local time.
    pub quiet: bool,
    /// The OS do-not-disturb switch looked on.
    pub dnd: bool,
    pub now: Instant,
    /// No remote post starts after this, read from the clock, so a slow
    /// destination cannot hold up the check timer for long.
    pub post_until: Instant,
}

/// What happened to each alert at one sink during one check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub delivered: u32,
    /// Below the sink's minimum severity. Not sent later either, unless the
    /// alert comes back with a higher severity.
    pub filtered: u32,
    /// Quiet hours. Sent from a later check that still lists it.
    pub held: u32,
    /// Hourly limit, per-check cap, or time budget. Sent from a later check.
    pub limited: u32,
    /// Do-not-disturb on a local sink. Shown from a later check that still
    /// lists it, never during do-not-disturb.
    pub suppressed: u32,
    pub failed: u32,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RouteReport {
    pub tallies: Vec<(String, Tally)>,
    /// The first error each sink returned in this check.
    pub errors: Vec<(String, SinkError)>,
}

impl RouteReport {
    pub fn tally(&self, id: &str) -> Tally {
        self.tallies
            .iter()
            .find(|(name, _)| name == id)
            .map(|(_, tally)| *tally)
            .unwrap_or_default()
    }
}

struct Seen {
    id: String,
    /// Bit per sink slot: delivered, filtered, or refused for good.
    done: u64,
    /// The highest severity seen under this id.
    rank: u8,
}

struct Bucket {
    id: String,
    per_hour: u32,
    tokens: f64,
    last: Instant,
}

/// Routing state kept between checks on the check timer.
#[derive(Default)]
pub struct Router {
    seen: VecDeque<Seen>,
    slots: Vec<String>,
    buckets: Vec<Bucket>,
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    /// Alert ids remembered now.
    pub fn remembered(&self) -> usize {
        self.seen.len()
    }

    /// Whether the route `id` has an alert in `alerts` it has not taken yet.
    /// Lets the caller skip keychain reads and the do-not-disturb probe when
    /// nothing is new.
    pub fn wants(&self, id: &str, alerts: &[Alert]) -> bool {
        self.wants_any(id, alerts, |_| true)
    }

    /// [`Router::wants`] counting only the alerts `deliverable` accepts, so a
    /// caller can skip a sink whose new alerts would only be held or
    /// filtered this check.
    pub fn wants_any(
        &self,
        id: &str,
        alerts: &[Alert],
        deliverable: impl Fn(&Alert) -> bool,
    ) -> bool {
        let bit = self.bit(id);
        alerts
            .iter()
            .filter(|alert| deliverable(alert))
            .any(|alert| {
                let Some(bit) = bit else {
                    return true;
                };
                self.seen
                    .iter()
                    .find(|seen| seen.id == alert.id)
                    .is_none_or(|seen| seen.done & bit == 0 || rank(alert.severity) > seen.rank)
            })
    }

    /// Whether `rule` would send `alert` now rather than filter or hold it.
    /// Do-not-disturb and the limits are left out.
    pub fn sends_now(rule: &SinkRule, alert: &Alert, quiet: bool) -> bool {
        rank(alert.severity) >= rank(rule.min_severity)
            && (!quiet || (alert.is_high() && rule.quiet_high_bypass))
    }

    /// Hand each alert to each sink that has not had it. High-severity
    /// alerts go first so the hourly limits favour them.
    pub fn route(
        &mut self,
        alerts: &[Alert],
        routes: &[Route<'_>],
        context: &RouteContext,
    ) -> RouteReport {
        let mut report = RouteReport {
            tallies: routes
                .iter()
                .map(|route| (route.id.to_string(), Tally::default()))
                .collect(),
            errors: Vec::new(),
        };
        self.reserve(routes);
        let bits: Vec<u64> = routes
            .iter()
            .map(|route| self.bit(route.id).unwrap_or(0))
            .collect();
        let capacity = SEEN_CAPACITY.max(alerts.len());
        let mut sent = vec![0_usize; routes.len()];
        let mut broken = vec![false; routes.len()];
        let mut order: Vec<&Alert> = alerts.iter().collect();
        order.sort_by_key(|alert| std::cmp::Reverse(rank(alert.severity)));
        for alert in order {
            self.touch(&alert.id, rank(alert.severity), capacity);
            for (index, route) in routes.iter().enumerate() {
                let bit = bits[index];
                if self.seen[0].done & bit != 0 {
                    continue;
                }
                let tally = &mut report.tallies[index].1;
                if rank(alert.severity) < rank(route.rule.min_severity) {
                    self.seen[0].done |= bit;
                    tally.filtered += 1;
                    continue;
                }
                if route.rule.local && context.dnd {
                    tally.suppressed += 1;
                    continue;
                }
                if context.quiet && !(alert.is_high() && route.rule.quiet_high_bypass) {
                    tally.held += 1;
                    continue;
                }
                // Network posts stop at the cap or the time budget so the
                // check timer is not held up. The local sinks post nothing.
                if broken[index]
                    || (!route.rule.local
                        && (sent[index] >= MAX_PER_CHECK || Instant::now() >= context.post_until))
                {
                    tally.limited += 1;
                    continue;
                }
                if let Some(per_hour) = route.rule.max_per_hour {
                    if !take(&mut self.buckets, route.id, per_hour, context.now) {
                        tally.limited += 1;
                        continue;
                    }
                }
                sent[index] += 1;
                match route.sink.deliver(alert) {
                    Ok(()) => {
                        self.seen[0].done |= bit;
                        tally.delivered += 1;
                    }
                    Err(err) => {
                        tally.failed += 1;
                        if err.retry() {
                            broken[index] = true;
                            refund(&mut self.buckets, route.id);
                        } else if err.gone() {
                            // Every other alert would fail the same way. Try
                            // this one again next check, in case it is fixed.
                            broken[index] = true;
                            refund(&mut self.buckets, route.id);
                        } else {
                            self.seen[0].done |= bit;
                        }
                        if !report.errors.iter().any(|(id, _)| id == route.id) {
                            report.errors.push((route.id.to_string(), err));
                        }
                    }
                }
            }
        }
        report
    }

    /// Move `id` to the front, adding it when new and dropping the least
    /// recently seen ids past `capacity`. A higher severity than before
    /// makes the alert new to every sink again.
    fn touch(&mut self, id: &str, rank: u8, capacity: usize) {
        let mut entry = match self.seen.iter().position(|seen| seen.id == id) {
            Some(at) => self.seen.remove(at),
            None => None,
        }
        .unwrap_or(Seen {
            id: id.to_string(),
            done: 0,
            rank,
        });
        if rank > entry.rank {
            entry.rank = rank;
            entry.done = 0;
        }
        self.seen.push_front(entry);
        self.seen.truncate(capacity);
    }

    /// Give every route a slot before any bit is read. When they would not
    /// fit, sink ids changed many times: start over rather than share a bit
    /// between two sinks.
    fn reserve(&mut self, routes: &[Route<'_>]) {
        let missing = routes
            .iter()
            .filter(|route| self.bit(route.id).is_none())
            .count();
        if self.slots.len() + missing > MAX_SLOTS {
            self.slots.clear();
            self.seen.clear();
        }
        for route in routes {
            if self.bit(route.id).is_none() && self.slots.len() < MAX_SLOTS {
                self.slots.push(route.id.to_string());
            }
        }
    }

    fn bit(&self, id: &str) -> Option<u64> {
        self.slots
            .iter()
            .position(|slot| slot == id)
            .map(|at| 1_u64 << at)
    }
}

fn rank(severity: Severity) -> u8 {
    match severity {
        Severity::Low => 0,
        Severity::Medium => 1,
        Severity::High => 2,
    }
}

/// Token bucket: `per_hour` messages to start with, refilled evenly over the
/// hour.
fn take(buckets: &mut Vec<Bucket>, id: &str, per_hour: u32, now: Instant) -> bool {
    let per_hour = per_hour.max(1);
    let full = f64::from(per_hour);
    let at = match buckets.iter().position(|bucket| bucket.id == id) {
        Some(at) => at,
        None => {
            buckets.push(Bucket {
                id: id.to_string(),
                per_hour,
                tokens: full,
                last: now,
            });
            buckets.len() - 1
        }
    };
    let bucket = &mut buckets[at];
    if bucket.per_hour != per_hour {
        bucket.per_hour = per_hour;
        bucket.tokens = bucket.tokens.min(full);
    }
    let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
    bucket.tokens = (bucket.tokens + elapsed * full / 3_600.0).min(full);
    if now > bucket.last {
        bucket.last = now;
    }
    if bucket.tokens >= 1.0 {
        bucket.tokens -= 1.0;
        true
    } else {
        false
    }
}

/// Give back the message a busy destination did not take.
fn refund(buckets: &mut [Bucket], id: &str) {
    if let Some(bucket) = buckets.iter_mut().find(|bucket| bucket.id == id) {
        bucket.tokens = (bucket.tokens + 1.0).min(f64::from(bucket.per_hour));
    }
}

#[cfg(test)]
mod tests {
    use super::{Route, RouteContext, Router, SinkRule, MAX_PER_CHECK, SEEN_CAPACITY};
    use crate::api::{Severity, DEFAULT_SERVER_BASE};
    use crate::integrations::{IntegrationKind, IntegrationRecord};
    use crate::sink::{Alert, Sink, SinkError, SinkKind, WebhookSink};
    use crate::testserver::{LocalTransport, Reply, TestServer};
    use chrono::{TimeZone, Utc};
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const HOOK: &str = "https://hooks.slack.com/services/T000/B000/example";
    const HOOK_PATH: &str = "/services/T000/B000/example";

    fn alert(id: &str, severity: Severity) -> Alert {
        Alert {
            id: id.to_string(),
            severity,
            title: format!("Example item {id}"),
            created_at: Utc.with_ymd_and_hms(2026, 10, 26, 0, 0, 0).unwrap(),
            link: Some("https://helloorbit.app/workspace".to_string()),
        }
    }

    fn rule(min: Severity, per_hour: Option<u32>, bypass: bool) -> SinkRule {
        SinkRule {
            min_severity: min,
            max_per_hour: per_hour,
            quiet_high_bypass: bypass,
            local: false,
        }
    }

    fn open(now: Instant) -> RouteContext {
        RouteContext {
            quiet: false,
            dnd: false,
            now,
            post_until: Instant::now() + Duration::from_secs(3_600),
        }
    }

    #[derive(Default)]
    struct Collect(RefCell<Vec<String>>);

    impl Sink for Collect {
        fn deliver(&self, alert: &Alert) -> Result<(), SinkError> {
            self.0.borrow_mut().push(alert.id.clone());
            Ok(())
        }
    }

    #[test]
    fn an_alert_reaches_each_sink_once_and_the_seen_list_stays_bounded() {
        let server = TestServer::start(|_| Reply::json(200, "ok"));
        let transport = LocalTransport::to(&server);
        let sink = WebhookSink::new(
            "ops",
            SinkKind::Slack,
            Some(HOOK),
            None,
            DEFAULT_SERVER_BASE,
            &transport,
        )
        .unwrap();
        let bubble = Collect::default();
        let routes = [
            Route {
                id: "ops",
                rule: rule(Severity::Low, None, true),
                sink: &sink,
            },
            Route {
                id: "bubble",
                rule: SinkRule::BUBBLE,
                sink: &bubble,
            },
        ];
        let mut router = Router::new();
        let now = Instant::now();
        let first = [alert("a", Severity::Medium), alert("b", Severity::Low)];
        let report = router.route(&first, &routes, &open(now));
        assert_eq!(report.tally("ops").delivered, 2);
        assert_eq!(report.tally("bubble").delivered, 2);
        assert_eq!(server.count(HOOK_PATH), 2);
        let again = router.route(&first, &routes, &open(now));
        assert_eq!(again.tally("ops").delivered, 0);
        assert_eq!(server.count(HOOK_PATH), 2);
        let more = [
            alert("a", Severity::Medium),
            alert("b", Severity::Low),
            alert("c", Severity::High),
        ];
        let report = router.route(&more, &routes, &open(now));
        assert_eq!(report.tally("ops").delivered, 1);
        assert_eq!(server.count(HOOK_PATH), 3);
        assert_eq!(bubble.0.borrow().as_slice(), ["a", "b", "c"]);

        let mut quiet_router = Router::new();
        let many: Vec<Alert> = (0..SEEN_CAPACITY + 44)
            .map(|index| alert(&format!("n{index}"), Severity::Low))
            .collect();
        let local = Collect::default();
        let only_local = [Route {
            id: "bubble",
            rule: SinkRule::BUBBLE,
            sink: &local,
        }];
        quiet_router.route(&many, &only_local, &open(now));
        assert_eq!(
            quiet_router.remembered(),
            SEEN_CAPACITY + 44,
            "one response is kept whole"
        );
        assert_eq!(local.0.borrow().len(), SEEN_CAPACITY + 44);
        quiet_router.route(&many[SEEN_CAPACITY + 43..], &only_local, &open(now));
        assert_eq!(quiet_router.remembered(), SEEN_CAPACITY);
        assert_eq!(local.0.borrow().len(), SEEN_CAPACITY + 44);
        quiet_router.route(&many[..1], &only_local, &open(now));
        assert_eq!(
            local.0.borrow().len(),
            SEEN_CAPACITY + 45,
            "the oldest id was dropped, so it counts as new"
        );
    }

    #[test]
    fn the_hourly_limit_and_the_per_check_cap_hold_the_rest_for_later() {
        let server = TestServer::start(|_| Reply::json(200, "ok"));
        let transport = LocalTransport::to(&server);
        let sink = WebhookSink::new(
            "ops",
            SinkKind::Slack,
            Some(HOOK),
            None,
            DEFAULT_SERVER_BASE,
            &transport,
        )
        .unwrap();
        let routes = [Route {
            id: "ops",
            rule: rule(Severity::Low, Some(2), true),
            sink: &sink,
        }];
        let mut router = Router::new();
        let start = Instant::now();
        let four: Vec<Alert> = ["a", "b", "c", "d"]
            .iter()
            .map(|id| alert(id, Severity::Low))
            .collect();
        let report = router.route(&four, &routes, &open(start));
        assert_eq!(report.tally("ops").delivered, 2);
        assert_eq!(report.tally("ops").limited, 2);
        let report = router.route(&four, &routes, &open(start + Duration::from_secs(60)));
        assert_eq!(report.tally("ops").delivered, 0);
        assert_eq!(report.tally("ops").limited, 2);
        let report = router.route(&four, &routes, &open(start + Duration::from_secs(1_860)));
        assert_eq!(
            report.tally("ops").delivered,
            1,
            "one message refills each 30 minutes"
        );
        let report = router.route(&four, &routes, &open(start + Duration::from_secs(9_000)));
        assert_eq!(report.tally("ops").delivered, 1);
        assert_eq!(server.count(HOOK_PATH), 4);

        let roomy = [Route {
            id: "ops",
            rule: rule(Severity::Low, Some(600), true),
            sink: &sink,
        }];
        let mut fresh = Router::new();
        let many: Vec<Alert> = (0..MAX_PER_CHECK + 3)
            .map(|index| alert(&format!("e{index}"), Severity::Low))
            .collect();
        let report = fresh.route(&many, &roomy, &open(start));
        assert_eq!(report.tally("ops").delivered, MAX_PER_CHECK as u32);
        let report = fresh.route(&many, &roomy, &open(start));
        assert_eq!(report.tally("ops").delivered, 3);

        let mut late = Router::new();
        let spent = RouteContext {
            post_until: Instant::now(),
            ..open(start)
        };
        let report = late.route(&many[..2], &roomy, &spent);
        assert_eq!(report.tally("ops").delivered, 0, "the time budget is spent");
        assert_eq!(report.tally("ops").limited, 2);
        let report = late.route(&many[..2], &roomy, &open(start));
        assert_eq!(report.tally("ops").delivered, 2, "and they go next time");
    }

    #[test]
    fn quiet_hours_hold_ordinary_alerts_and_let_high_ones_through_when_allowed() {
        let server = TestServer::start(|_| Reply::json(200, "ok"));
        let transport = LocalTransport::to(&server);
        let sink = WebhookSink::new(
            "ops",
            SinkKind::Slack,
            Some(HOOK),
            None,
            DEFAULT_SERVER_BASE,
            &transport,
        )
        .unwrap();
        let strict = WebhookSink::new(
            "strict",
            SinkKind::Slack,
            Some("https://hooks.slack.com/services/T000/B000/strict"),
            None,
            DEFAULT_SERVER_BASE,
            &transport,
        )
        .unwrap();
        let routes = [
            Route {
                id: "ops",
                rule: rule(Severity::Low, None, true),
                sink: &sink,
            },
            Route {
                id: "strict",
                rule: rule(Severity::Low, None, false),
                sink: &strict,
            },
        ];
        let mut router = Router::new();
        let now = Instant::now();
        let quiet = RouteContext {
            quiet: true,
            ..open(now)
        };
        let alerts = [alert("m", Severity::Medium), alert("h", Severity::High)];
        let report = router.route(&alerts, &routes, &quiet);
        assert_eq!(report.tally("ops").delivered, 1);
        assert_eq!(report.tally("ops").held, 1);
        assert_eq!(report.tally("strict").delivered, 0);
        assert_eq!(report.tally("strict").held, 2);
        assert_eq!(server.count(HOOK_PATH), 1);
        assert!(server.requests()[0].body.contains("high-severity"));
        let report = router.route(&alerts, &routes, &open(now));
        assert_eq!(
            report.tally("ops").delivered,
            1,
            "the held medium alert goes now"
        );
        assert_eq!(report.tally("strict").delivered, 2);
        assert_eq!(server.count(HOOK_PATH), 2);
        assert_eq!(server.count("/services/T000/B000/strict"), 2);
    }

    #[test]
    fn minimum_severity_filters_until_severity_rises_and_dnd_holds_only_local_sinks() {
        let server = TestServer::start(|_| Reply::json(202, "{}"));
        let transport = LocalTransport::to(&server);
        let pager = WebhookSink::new(
            "pd",
            SinkKind::PagerDuty,
            None,
            Some("exampleroutingkey000000000000000"),
            DEFAULT_SERVER_BASE,
            &transport,
        )
        .unwrap();
        let mut record = IntegrationRecord::new(IntegrationKind::PagerDuty, "pd").unwrap();
        record.enabled = true;
        let pager_rule = SinkRule::for_record(&record).unwrap();
        assert_eq!(pager_rule.min_severity, Severity::High);
        assert_eq!(pager_rule.max_per_hour, Some(super::DEFAULT_MAX_PER_HOUR));
        assert!(SinkRule::for_record(
            &IntegrationRecord::new(IntegrationKind::GenericJson, "json").unwrap()
        )
        .is_none());
        let bubble = Collect::default();
        let notice = Collect::default();
        let routes = [
            Route {
                id: "pd",
                rule: pager_rule,
                sink: &pager,
            },
            Route {
                id: "bubble",
                rule: SinkRule::BUBBLE,
                sink: &bubble,
            },
            Route {
                id: "notification",
                rule: SinkRule::NOTIFICATION,
                sink: &notice,
            },
        ];
        let mut router = Router::new();
        let now = Instant::now();
        let dnd = RouteContext {
            dnd: true,
            ..open(now)
        };
        let alerts = [alert("m", Severity::Medium), alert("h", Severity::High)];
        assert!(router.wants("bubble", &alerts));
        let report = router.route(&alerts, &routes, &dnd);
        assert_eq!(report.tally("pd").delivered, 1);
        assert_eq!(report.tally("pd").filtered, 1);
        assert_eq!(report.tally("bubble").suppressed, 2);
        assert_eq!(report.tally("notification").suppressed, 1);
        assert_eq!(report.tally("notification").filtered, 1);
        assert!(bubble.0.borrow().is_empty());
        assert!(notice.0.borrow().is_empty());
        assert_eq!(server.count("/v2/enqueue"), 1);
        assert!(!router.wants("pd", &alerts));
        assert!(router.wants("bubble", &alerts), "held, not dropped");
        let report = router.route(&alerts, &routes, &dnd);
        assert_eq!(report.tally("bubble").suppressed, 2, "never during it");
        let report = router.route(&alerts, &routes, &open(now));
        assert_eq!(report.tally("pd").delivered, 0);
        assert_eq!(
            bubble.0.borrow().as_slice(),
            ["h", "m"],
            "shown once do-not-disturb ends, high first"
        );
        assert_eq!(notice.0.borrow().as_slice(), ["h"]);
        assert!(!router.wants("bubble", &alerts));
        assert!(!router.wants("notification", &alerts));
        let fresh = [alert("q", Severity::Medium)];
        assert!(router.wants("pd", &fresh), "unseen");
        assert!(
            !router.wants_any("pd", &fresh, |a| Router::sends_now(&pager_rule, a, false)),
            "but below the minimum, so no keychain read is needed"
        );
        let strict = rule(Severity::Low, None, false);
        let night = [alert("z", Severity::High)];
        assert!(!router.wants_any("pd", &night, |a| Router::sends_now(&strict, a, true)));
        assert!(router.wants_any("pd", &night, |a| Router::sends_now(&pager_rule, a, true)));
        let rose = [alert("m", Severity::High)];
        assert!(router.wants("pd", &rose), "a higher severity is news");
        let report = router.route(&rose, &routes, &open(now));
        assert_eq!(report.tally("pd").delivered, 1);
        assert_eq!(server.count("/v2/enqueue"), 2);
        assert_eq!(notice.0.borrow().as_slice(), ["h", "m"]);
        let fell = [alert("m", Severity::Low)];
        assert!(!router.wants("pd", &fell), "a lower one is not");
        let report = router.route(&fell, &routes, &open(now));
        assert_eq!(report.tally("bubble").delivered, 0);
    }

    #[test]
    fn one_large_check_keeps_every_id_and_slots_never_share_a_bit() {
        let local = Collect::default();
        let routes = [Route {
            id: "bubble",
            rule: SinkRule::BUBBLE,
            sink: &local,
        }];
        let mut router = Router::new();
        let now = Instant::now();
        let many: Vec<Alert> = (0..SEEN_CAPACITY + 50)
            .map(|index| alert(&format!("x{index}"), Severity::High))
            .collect();
        router.route(&many, &routes, &open(now));
        assert_eq!(local.0.borrow().len(), SEEN_CAPACITY + 50);
        router.route(&many, &routes, &open(now));
        assert_eq!(
            local.0.borrow().len(),
            SEEN_CAPACITY + 50,
            "no repeats from one response that lists more than the usual capacity"
        );

        let ids: Vec<String> = (0..super::MAX_SLOTS + 1)
            .map(|index| format!("sink-{index}"))
            .collect();
        let sinks: Vec<Collect> = ids.iter().map(|_| Collect::default()).collect();
        let mut router = Router::new();
        let first = [alert("a", Severity::Low)];
        for (id, sink) in ids.iter().zip(&sinks).take(super::MAX_SLOTS) {
            let one = [Route {
                id,
                rule: SinkRule::BUBBLE,
                sink,
            }];
            router.route(&first, &one, &open(now));
        }
        let both = [
            Route {
                id: &ids[0],
                rule: SinkRule::BUBBLE,
                sink: &sinks[0],
            },
            Route {
                id: &ids[super::MAX_SLOTS],
                rule: SinkRule::BUBBLE,
                sink: &sinks[super::MAX_SLOTS],
            },
        ];
        let report = router.route(&first, &both, &open(now));
        assert_eq!(
            report.tally(&ids[super::MAX_SLOTS]).delivered,
            1,
            "a new sink past the slot limit still gets the alert"
        );
        assert_eq!(report.tally(&ids[0]).delivered, 1, "after a clean start");
    }

    #[test]
    fn a_busy_destination_is_retried_later_and_a_refusal_is_not() {
        let calls = Arc::new(AtomicU32::new(0));
        let server = {
            let calls = Arc::clone(&calls);
            TestServer::start(move |request| match request.path() {
                "/services/T000/B000/busy" => {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        Reply::json(503, "busy")
                    } else {
                        Reply::json(200, "ok")
                    }
                }
                _ => Reply::json(400, "invalid_payload"),
            })
        };
        let transport = LocalTransport::to(&server);
        let busy = WebhookSink::new(
            "busy",
            SinkKind::Slack,
            Some("https://hooks.slack.com/services/T000/B000/busy"),
            None,
            DEFAULT_SERVER_BASE,
            &transport,
        )
        .unwrap();
        let refusing = WebhookSink::new(
            "refusing",
            SinkKind::Slack,
            Some("https://hooks.slack.com/services/T000/B000/refusing"),
            None,
            DEFAULT_SERVER_BASE,
            &transport,
        )
        .unwrap();
        let routes = [
            Route {
                id: "busy",
                rule: rule(Severity::Low, None, true),
                sink: &busy,
            },
            Route {
                id: "refusing",
                rule: rule(Severity::Low, None, true),
                sink: &refusing,
            },
        ];
        let mut router = Router::new();
        let now = Instant::now();
        let alerts = [alert("a", Severity::High), alert("b", Severity::High)];
        let report = router.route(&alerts, &routes, &open(now));
        assert_eq!(report.tally("busy").failed, 1);
        assert_eq!(
            report.tally("busy").limited,
            1,
            "after one failure the sink waits for the next check"
        );
        assert_eq!(report.tally("refusing").failed, 2);
        assert_eq!(
            report.errors,
            vec![
                ("busy".to_string(), SinkError::Busy(503)),
                ("refusing".to_string(), SinkError::Rejected(400)),
            ]
        );
        let report = router.route(&alerts, &routes, &open(now));
        assert_eq!(report.tally("busy").delivered, 2);
        assert_eq!(report.tally("refusing").failed, 0);
        assert_eq!(server.count("/services/T000/B000/refusing"), 2);
        assert_eq!(server.count("/services/T000/B000/busy"), 3);
    }

    #[test]
    fn a_gone_webhook_costs_one_post_a_check_and_a_busy_one_keeps_its_hourly_quota() {
        let fixed = Arc::new(AtomicU32::new(0));
        let server = {
            let fixed = Arc::clone(&fixed);
            TestServer::start(move |request| match request.path() {
                "/services/T000/B000/gone" if fixed.load(Ordering::SeqCst) == 0 => {
                    Reply::json(404, "no_service")
                }
                "/services/T000/B000/busy" if fixed.load(Ordering::SeqCst) == 0 => {
                    Reply::json(429, "rate_limited")
                }
                _ => Reply::json(200, "ok"),
            })
        };
        let transport = LocalTransport::to(&server);
        let gone = WebhookSink::new(
            "gone",
            SinkKind::Slack,
            Some("https://hooks.slack.com/services/T000/B000/gone"),
            None,
            DEFAULT_SERVER_BASE,
            &transport,
        )
        .unwrap();
        let busy = WebhookSink::new(
            "busy",
            SinkKind::Slack,
            Some("https://hooks.slack.com/services/T000/B000/busy"),
            None,
            DEFAULT_SERVER_BASE,
            &transport,
        )
        .unwrap();
        let routes = [
            Route {
                id: "gone",
                rule: rule(Severity::Low, Some(2), true),
                sink: &gone,
            },
            Route {
                id: "busy",
                rule: rule(Severity::Low, Some(2), true),
                sink: &busy,
            },
        ];
        let mut router = Router::new();
        let now = Instant::now();
        let alerts: Vec<Alert> = ["a", "b", "c"]
            .iter()
            .map(|id| alert(id, Severity::Medium))
            .collect();
        for _ in 0..3 {
            let report = router.route(&alerts, &routes, &open(now));
            assert_eq!(report.tally("gone").failed, 1);
            assert_eq!(report.tally("gone").limited, 2);
            assert_eq!(report.tally("busy").failed, 1);
        }
        assert_eq!(server.count("/services/T000/B000/gone"), 3);
        fixed.store(1, Ordering::SeqCst);
        let report = router.route(&alerts, &routes, &open(now));
        assert_eq!(
            report.tally("gone").delivered,
            2,
            "nothing was thrown away, and the failed posts did not use up its hourly limit"
        );
        assert_eq!(report.tally("gone").limited, 1);
        assert_eq!(
            report.tally("busy").delivered,
            2,
            "the busy answers did not use up the hourly limit"
        );
    }
}
