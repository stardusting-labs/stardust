//! Per-alert routing on the check timer, used only with the routing
//! feature on.
//!
//! Each enabled Slack, Teams, Discord, or PagerDuty integration gets one
//! message per new alert, and the bubble and the OS notification on this
//! computer go through the same router so they share its dedupe, quiet-hours,
//! and do-not-disturb rules. The local sinks are decided first, so the bubble
//! does not wait for the network; the posts follow. Destination URLs and
//! routing keys are read from the keychain for the post and never logged;
//! logs name the integration id and the destination host only.

use crate::deliver::{port_of, resolve_checked, PinnedAddrs, ResolveError};
use chrono::{DateTime, Utc};
use stardust_core::api::AlertItem;
use stardust_core::consent::has_valid_consent;
use stardust_core::delivery::{outcome_of, Outcome};
use stardust_core::i18n::{tr, Language, Msg};
use stardust_core::integrations::{secret_account, IntegrationRecord, SecretSlot, SecretStore};
use stardust_core::outbound::CheckedUrl;
use stardust_core::policy::{Presentation, Summary};
use stardust_core::route::{Route, RouteContext, Router, SinkRule};
use stardust_core::sink::{Alert, Sink, SinkError, SinkKind, Transport, WebhookSink};
use std::cell::{Cell, RefCell};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Connect and whole-request timeout for one sink post.
pub const SINK_TIMEOUT: Duration = Duration::from_secs(5);
/// No sink post starts later than this after the posts begin. The rest wait
/// for the next check.
pub const POST_BUDGET: Duration = Duration::from_secs(20);

/// Router ids of the two local sinks. Integration ids cannot hold `:`.
const BUBBLE_ROUTE: &str = "local:bubble";
const NOTIFICATION_ROUTE: &str = "local:notification";

/// A failed name lookup that took this long means the resolver itself is
/// stuck or the network is down, not one bad name.
const SLOW_LOOKUP: Duration = Duration::from_secs(2);

/// HTTPS POST to a checked sink destination. The host is resolved once,
/// every address must be public, the request is pinned to those addresses,
/// redirects are not followed, and [`SINK_TIMEOUT`] applies. Use one per
/// check. The name lookup has no timeout of its own, so a host whose lookup
/// failed is not looked up again in that check, and after a slow failure no
/// host is.
#[derive(Default)]
pub struct HttpsTransport {
    offline: Cell<bool>,
    unresolved: RefCell<Vec<String>>,
}

impl HttpsTransport {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Transport for HttpsTransport {
    fn post_json(&self, url: &CheckedUrl, body: &[u8]) -> Result<u16, SinkError> {
        self.post(url, body, &[])
    }

    fn post_json_with_headers(
        &self,
        url: &CheckedUrl,
        body: &[u8],
        headers: &[(String, String)],
    ) -> Result<u16, SinkError> {
        self.post(url, body, headers)
    }
}

impl HttpsTransport {
    fn post(
        &self,
        url: &CheckedUrl,
        body: &[u8],
        headers: &[(String, String)],
    ) -> Result<u16, SinkError> {
        if self.offline.get() || self.unresolved.borrow().contains(&url.host) {
            return Err(SinkError::Unreachable);
        }
        let started = Instant::now();
        let addrs = resolve_checked(&url.host, port_of(&url.url)).map_err(|err| match err {
            ResolveError::Lookup => {
                if started.elapsed() >= SLOW_LOOKUP {
                    self.offline.set(true);
                } else {
                    self.unresolved.borrow_mut().push(url.host.clone());
                }
                SinkError::Unreachable
            }
            ResolveError::Blocked => SinkError::Blocked,
        })?;
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(SINK_TIMEOUT)
            .timeout(SINK_TIMEOUT)
            .redirects(0)
            .resolver(PinnedAddrs(addrs))
            .build();
        let mut request = agent.post(&url.url).set("Content-Type", "application/json");
        for (name, value) in headers {
            request = request.set(name, value);
        }
        match request.send_bytes(body) {
            Ok(response) => Ok(response.status()),
            Err(ureq::Error::Status(status, _)) => Ok(status),
            Err(ureq::Error::Transport(_)) => Err(SinkError::Unreachable),
        }
    }
}

/// What reached the bubble and the OS notification in this check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalOutcome {
    /// Indexes into the check's items of the alerts the bubble has not
    /// shown before, in item order.
    pub shown: Vec<usize>,
    /// A new high-severity alert reached the notification.
    pub notification: bool,
}

/// One successful check, as routing sees it.
pub struct CheckRouting<'a> {
    pub records: &'a [IntegrationRecord],
    pub secrets: &'a dyn SecretStore,
    pub items: &'a [AlertItem],
    pub server_base: &'a str,
    /// Inside the schedule's quiet hours.
    pub quiet: bool,
    pub now: Instant,
    /// The schedule's interval, for the JSON webhook's field.
    pub interval_minutes: u32,
}

impl CheckRouting<'_> {
    fn alerts(&self) -> Vec<Alert> {
        self.items
            .iter()
            .map(|item| Alert::from_item(item, self.server_base))
            .collect()
    }
}

/// Collects the ids that reached a local sink. Showing them is up to the app.
#[derive(Default)]
struct Collect(RefCell<Vec<String>>);

impl Sink for Collect {
    fn deliver(&self, alert: &Alert) -> Result<(), SinkError> {
        self.0.borrow_mut().push(alert.id.clone());
        Ok(())
    }
}

/// Decide the bubble and the OS notification for one check. Nothing here
/// touches the network.
///
/// `dnd` is called at most once, and only when an item is new to this
/// computer.
pub fn route_local(
    router: &mut Router,
    check: &CheckRouting<'_>,
    dnd: &dyn Fn() -> bool,
) -> LocalOutcome {
    let alerts = check.alerts();
    let shows = |id: &str, rule: &SinkRule| {
        router.wants_any(id, &alerts, |alert| {
            Router::sends_now(rule, alert, check.quiet)
        })
    };
    if !shows(BUBBLE_ROUTE, &SinkRule::BUBBLE)
        && !shows(NOTIFICATION_ROUTE, &SinkRule::NOTIFICATION)
    {
        return LocalOutcome::default();
    }
    let bubble = Collect::default();
    let notification = Collect::default();
    let routes = [
        Route {
            id: BUBBLE_ROUTE,
            rule: SinkRule::BUBBLE,
            sink: &bubble,
        },
        Route {
            id: NOTIFICATION_ROUTE,
            rule: SinkRule::NOTIFICATION,
            sink: &notification,
        },
    ];
    let context = RouteContext {
        quiet: check.quiet,
        dnd: dnd(),
        now: check.now,
        post_until: check.now,
    };
    router.route(&alerts, &routes, &context);
    let reached = bubble.0.into_inner();
    let shown = alerts
        .iter()
        .enumerate()
        .filter(|(at, alert)| {
            reached.contains(&alert.id) && alerts[..*at].iter().all(|before| before.id != alert.id)
        })
        .map(|(at, _)| at)
        .collect();
    LocalOutcome {
        shown,
        notification: !notification.0.into_inner().is_empty(),
    }
}

/// Post one check's new alerts to every enabled sink integration. Only the
/// sinks with something to send now, not just to hold for quiet hours or
/// filter out, read their destination from the keychain.
pub fn route_remote(router: &mut Router, check: &CheckRouting<'_>, transport: &dyn Transport) {
    let alerts = check.alerts();
    let wanted: Vec<IntegrationRecord> = check
        .records
        .iter()
        .filter(|record| record.enabled)
        .filter(|record| {
            SinkRule::for_record(record).is_some_and(|rule| {
                router.wants_any(&record.id, &alerts, |alert| {
                    Router::sends_now(&rule, alert, check.quiet)
                })
            })
        })
        .cloned()
        .collect();
    let sinks = load_sinks(&wanted, check.secrets, check.server_base, transport);
    if sinks.is_empty() {
        return;
    }
    let routes: Vec<Route<'_>> = sinks
        .iter()
        .map(|(rule, sink)| Route {
            id: sink.id(),
            rule: *rule,
            sink,
        })
        .collect();
    let context = RouteContext {
        quiet: check.quiet,
        dnd: false,
        now: check.now,
        post_until: Instant::now() + POST_BUDGET,
    };
    let report = router.route(&alerts, &routes, &context);
    for (id, err) in &report.errors {
        eprintln!("stardust: integration {id}: {err}");
    }
    for (_, sink) in &sinks {
        let delivered = report.tally(sink.id()).delivered;
        if delivered > 0 {
            eprintln!(
                "stardust: sent {delivered} alert(s) to {} ({})",
                sink.id(),
                sink.host()
            );
        }
    }
}

/// [`route_remote`] with the integrations panel on. The JSON webhook goes
/// through the router too, an integration without a valid yes is skipped
/// before any keychain read, and MCP integrations are never routed. Returns
/// what the panel and the tray should show for each integration: only what
/// happened in this check, so one with nothing to report keeps its last line.
pub fn route_consented(
    router: &mut Router,
    check: &CheckRouting<'_>,
    transport: &dyn Transport,
) -> Vec<(String, Outcome)> {
    let alerts = check.alerts();
    let mut outcomes = Vec::new();
    let mut sinks: Vec<(SinkRule, WebhookSink<'_>)> = Vec::new();
    for record in check.records.iter().filter(|record| record.enabled) {
        let Some(kind) = SinkKind::with_panel(record.kind) else {
            continue;
        };
        if !has_valid_consent(record) {
            outcomes.push((record.id.clone(), Outcome::NeedsConsent));
            continue;
        }
        let rule = SinkRule::for_sink(record, kind);
        let sends_now = router.wants_any(&record.id, &alerts, |alert| {
            Router::sends_now(&rule, alert, check.quiet)
        });
        if !sends_now {
            // Nothing to post, so no keychain read. An alert still waiting
            // can only be waiting for the quiet hours to end.
            let held = check.quiet
                && router.wants_any(&record.id, &alerts, |alert| {
                    Router::sends_now(&rule, alert, false)
                });
            let outcome = if held {
                Outcome::HeldByQuietHours
            } else {
                Outcome::NothingWaiting
            };
            outcomes.push((record.id.clone(), outcome));
            continue;
        }
        let built = build_routed(record, kind, check, transport);
        match ready_once(&record.id, built) {
            Some(sink) if consent_covers(record, sink.host()) => sinks.push((rule, sink)),
            Some(_) => outcomes.push((record.id.clone(), Outcome::NeedsConsent)),
            None => outcomes.push((record.id.clone(), Outcome::NotReady)),
        }
    }
    if sinks.is_empty() {
        return outcomes;
    }
    let routes: Vec<Route<'_>> = sinks
        .iter()
        .map(|(rule, sink)| Route {
            id: sink.id(),
            rule: *rule,
            sink,
        })
        .collect();
    let context = RouteContext {
        quiet: check.quiet,
        dnd: false,
        now: check.now,
        post_until: Instant::now() + POST_BUDGET,
    };
    let report = router.route(&alerts, &routes, &context);
    for (id, err) in &report.errors {
        eprintln!("stardust: integration {id}: {err}");
    }
    for (_, sink) in &sinks {
        let tally = report.tally(sink.id());
        if tally.delivered > 0 {
            eprintln!(
                "stardust: sent {} alert(s) to {} ({})",
                tally.delivered,
                sink.id(),
                sink.host()
            );
        }
        let error = report
            .errors
            .iter()
            .find(|(id, _)| id == sink.id())
            .map(|(_, err)| err);
        if let Some(outcome) = outcome_of(tally, error) {
            outcomes.push((sink.id().to_string(), outcome));
        }
    }
    outcomes
}

/// The saved destination is the one the user said yes to. The config and the
/// keychain are written apart, so they could differ.
fn consent_covers(record: &IntegrationRecord, host: &str) -> bool {
    record
        .consent
        .as_ref()
        .is_some_and(|consent| consent.host == host)
}

/// The bubble that says integrations were switched off because their
/// destination is gone. Links nowhere, and shows ids only.
pub fn gone_presentation(ids: &[String], language: Language) -> Presentation {
    Presentation {
        show_bubble: true,
        show_os_notification: false,
        summary: Summary {
            lines: ids
                .iter()
                .map(|id| format!("{id}: {}", tr(language, Msg::OutcomeGoneDisabled)))
                .collect(),
            details_url: String::new(),
        },
    }
}

/// The records the check-summary posts go to. With routing on, Slack and
/// Teams take one message per alert instead, so they are left out here.
pub fn summary_records(records: &[IntegrationRecord], routing: bool) -> Vec<IntegrationRecord> {
    records
        .iter()
        .filter(|record| !routing || !record.kind.routes_alerts())
        .cloned()
        .collect()
}

/// Post the fixed test message to integration `id`, enabled or not.
/// Returns the destination host.
pub fn send_test(
    records: &[IntegrationRecord],
    secrets: &dyn SecretStore,
    id: &str,
    server_base: &str,
    now: DateTime<Utc>,
    transport: &dyn Transport,
) -> Result<String, SinkError> {
    let record = records
        .iter()
        .find(|record| record.id == id)
        .ok_or(SinkError::Missing)?;
    let kind = SinkKind::of(record.kind).ok_or(SinkError::Refused)?;
    let sink = build_sink(record, kind, secrets, server_base, transport)?;
    sink.send_test(server_base, now)?;
    Ok(sink.host().to_string())
}

/// [`send_test`] with the integrations panel on: the JSON webhook can be
/// tested too, and only an integration the user said yes to is posted to.
pub fn send_test_consented(
    check: &CheckRouting<'_>,
    id: &str,
    now: DateTime<Utc>,
    transport: &dyn Transport,
) -> Result<String, SinkError> {
    let record = check
        .records
        .iter()
        .find(|record| record.id == id)
        .ok_or(SinkError::Missing)?;
    let kind = SinkKind::with_panel(record.kind).ok_or(SinkError::Refused)?;
    if !has_valid_consent(record) {
        return Err(SinkError::NeedsConsent);
    }
    let sink = build_routed(record, kind, check, transport)?;
    if !consent_covers(record, sink.host()) {
        return Err(SinkError::NeedsConsent);
    }
    sink.send_test(check.server_base, now)?;
    Ok(sink.host().to_string())
}

/// The bubble that reports a "Send test" result.
pub fn test_presentation(
    id: &str,
    result: &Result<String, SinkError>,
    language: Language,
) -> Presentation {
    let msg = if result.is_ok() {
        Msg::TestSent
    } else {
        Msg::TestFailed
    };
    Presentation {
        show_bubble: true,
        show_os_notification: false,
        summary: Summary {
            lines: vec![format!("{}: {id}", tr(language, msg))],
            details_url: String::new(),
        },
    }
}

/// Integration ids already logged as not ready, so the log says it once
/// rather than every check.
static NOT_READY: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Every enabled sink integration with its destination ready. One that is
/// not ready is logged by id, once until it is ready again, and left out.
fn load_sinks<'a>(
    records: &[IntegrationRecord],
    secrets: &dyn SecretStore,
    server_base: &str,
    transport: &'a dyn Transport,
) -> Vec<(SinkRule, WebhookSink<'a>)> {
    records
        .iter()
        .filter(|record| record.enabled)
        .filter_map(|record| {
            let kind = SinkKind::of(record.kind)?;
            let rule = SinkRule::for_record(record)?;
            let built = build_sink(record, kind, secrets, server_base, transport);
            ready_once(&record.id, built).map(|sink| (rule, sink))
        })
        .collect()
}

/// The sink if it could be built. If not, its id is logged, once until it is
/// ready again.
fn ready_once<'a>(id: &str, built: Result<WebhookSink<'a>, SinkError>) -> Option<WebhookSink<'a>> {
    let mut noted = NOT_READY.lock().unwrap_or_else(|err| err.into_inner());
    match built {
        Ok(sink) => {
            noted.retain(|noted_id| noted_id != id);
            Some(sink)
        }
        Err(err) => {
            if !noted.iter().any(|noted_id| noted_id == id) {
                eprintln!("stardust: integration {id} is not ready: {err}");
                noted.push(id.to_string());
            }
            None
        }
    }
}

/// [`build_sink`], and the signed JSON webhook the panel adds.
fn build_routed<'a>(
    record: &IntegrationRecord,
    kind: SinkKind,
    check: &CheckRouting<'_>,
    transport: &'a dyn Transport,
) -> Result<WebhookSink<'a>, SinkError> {
    if kind != SinkKind::Generic {
        return build_sink(record, kind, check.secrets, check.server_base, transport);
    }
    let url = saved(check.secrets, &record.id, SecretSlot::Url);
    let secret = saved(check.secrets, &record.id, SecretSlot::SigningSecret);
    WebhookSink::signed_json(
        &record.id,
        url.as_deref(),
        secret.as_deref(),
        check.interval_minutes,
        transport,
    )
}

fn build_sink<'a>(
    record: &IntegrationRecord,
    kind: SinkKind,
    secrets: &dyn SecretStore,
    server_base: &str,
    transport: &'a dyn Transport,
) -> Result<WebhookSink<'a>, SinkError> {
    let url = saved(secrets, &record.id, SecretSlot::Url);
    let key = match kind {
        SinkKind::PagerDuty => saved(secrets, &record.id, SecretSlot::RoutingKey),
        SinkKind::Slack | SinkKind::Teams | SinkKind::Discord | SinkKind::Generic => None,
    };
    WebhookSink::new(
        &record.id,
        kind,
        url.as_deref(),
        key.as_deref(),
        server_base,
        transport,
    )
}

fn saved(secrets: &dyn SecretStore, id: &str, slot: SecretSlot) -> Option<String> {
    secrets.get(&secret_account(id, slot)).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::{
        route_local, route_remote, send_test, summary_records, test_presentation, CheckRouting,
        HttpsTransport, LocalOutcome,
    };
    use chrono::{TimeZone, Utc};
    use stardust_core::api::{AlertItem, Severity, DEFAULT_SERVER_BASE};
    use stardust_core::i18n::Language;
    use stardust_core::integrations::{
        secret_account, IntegrationKind, IntegrationRecord, MemorySecretStore, SecretSlot,
        SecretStore,
    };
    use stardust_core::outbound::CheckedUrl;
    use stardust_core::route::Router;
    use stardust_core::sink::{SinkError, Transport};
    use std::cell::{Cell, RefCell};
    use std::time::{Duration, Instant};

    const SLACK: &str = "https://hooks.slack.com/services/T000/B000/example";
    const ROUTING_KEY: &str = "exampleroutingkey000000000000000";

    /// Records each post's URL and body text and answers with a fixed status.
    struct Recorder {
        posts: RefCell<Vec<(String, String)>>,
        status: u16,
    }

    impl Recorder {
        fn answering(status: u16) -> Self {
            Self {
                posts: RefCell::new(Vec::new()),
                status,
            }
        }

        fn urls(&self) -> Vec<String> {
            self.posts
                .borrow()
                .iter()
                .map(|(url, _)| url.clone())
                .collect()
        }
    }

    impl Transport for Recorder {
        fn post_json(&self, url: &CheckedUrl, body: &[u8]) -> Result<u16, SinkError> {
            let body = String::from_utf8_lossy(body).to_string();
            self.posts.borrow_mut().push((url.url.clone(), body));
            Ok(self.status)
        }

        fn post_json_with_headers(
            &self,
            url: &CheckedUrl,
            body: &[u8],
            _headers: &[(String, String)],
        ) -> Result<u16, SinkError> {
            self.post_json(url, body)
        }
    }

    fn record(kind: IntegrationKind, id: &str, enabled: bool) -> IntegrationRecord {
        let mut record = IntegrationRecord::new(kind, id).unwrap();
        record.enabled = enabled;
        record
    }

    fn item(id: &str, severity: Severity) -> AlertItem {
        AlertItem {
            id: id.to_string(),
            severity,
            title: format!("Example item {id}"),
            created_at: Utc.with_ymd_and_hms(2026, 10, 26, 0, 0, 0).unwrap(),
            workspace_path: "/rust-preview/pqc/studio/app/findings".to_string(),
            immediate: severity == Severity::High,
        }
    }

    fn slack_secrets() -> MemorySecretStore {
        let secrets = MemorySecretStore::new();
        secrets
            .put(&secret_account("ops", SecretSlot::Url), SLACK)
            .unwrap();
        secrets
    }

    fn check<'a>(
        records: &'a [IntegrationRecord],
        secrets: &'a dyn SecretStore,
        items: &'a [AlertItem],
    ) -> CheckRouting<'a> {
        CheckRouting {
            records,
            secrets,
            items,
            server_base: DEFAULT_SERVER_BASE,
            quiet: false,
            now: Instant::now(),
            interval_minutes: 60,
        }
    }

    /// One check the way the app runs it: local first, then the posts.
    fn route_check(
        router: &mut Router,
        check: &CheckRouting<'_>,
        dnd: &dyn Fn() -> bool,
        transport: &dyn Transport,
    ) -> LocalOutcome {
        let local = route_local(router, check, dnd);
        route_remote(router, check, transport);
        local
    }

    /// Counts keychain reads.
    struct Counting {
        inner: MemorySecretStore,
        reads: Cell<u32>,
    }

    impl SecretStore for Counting {
        fn put(
            &self,
            account: &str,
            secret: &str,
        ) -> Result<(), stardust_core::integrations::SecretError> {
            self.inner.put(account, secret)
        }
        fn get(
            &self,
            account: &str,
        ) -> Result<Option<String>, stardust_core::integrations::SecretError> {
            self.reads.set(self.reads.get() + 1);
            self.inner.get(account)
        }
        fn delete(&self, account: &str) -> Result<(), stardust_core::integrations::SecretError> {
            self.inner.delete(account)
        }
    }

    #[test]
    fn summary_posts_keep_every_record_until_routing_moves_the_alert_sinks() {
        let records = [
            record(IntegrationKind::SlackIncoming, "slack", true),
            record(IntegrationKind::TeamsIncoming, "teams", true),
            record(IntegrationKind::DiscordWebhook, "discord", true),
            record(IntegrationKind::PagerDuty, "pd", true),
            record(IntegrationKind::GenericJson, "json", true),
        ];
        assert_eq!(summary_records(&records, false), records);
        let kept = summary_records(&records, true);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "json");
    }

    #[test]
    fn each_new_alert_goes_once_to_each_ready_sink_and_to_the_local_ones() {
        let records = [
            record(IntegrationKind::SlackIncoming, "ops", true),
            record(IntegrationKind::SlackIncoming, "off", false),
            record(IntegrationKind::SlackIncoming, "nourl", true),
            record(IntegrationKind::GenericJson, "json", true),
        ];
        let secrets = Counting {
            inner: slack_secrets(),
            reads: Cell::new(0),
        };
        secrets
            .put(&secret_account("off", SecretSlot::Url), SLACK)
            .unwrap();
        let transport = Recorder::answering(200);
        let mut router = Router::new();
        let asked = Cell::new(0);
        let dnd = || {
            asked.set(asked.get() + 1);
            false
        };
        let items = [item("a", Severity::Medium)];
        let reached = route_check(
            &mut router,
            &check(&records, &secrets, &items),
            &dnd,
            &transport,
        );
        assert_eq!(
            reached,
            LocalOutcome {
                shown: vec![0],
                notification: false
            }
        );
        assert_eq!(transport.urls(), [SLACK]);
        assert_eq!(asked.get(), 1);
        let reads = secrets.reads.get();
        assert!(reads > 0);
        let again = route_check(
            &mut router,
            &check(&records, &secrets, &items),
            &dnd,
            &transport,
        );
        assert_eq!(again, LocalOutcome::default(), "an alert is shown once");
        assert_eq!(transport.posts.borrow().len(), 1, "and posted once");
        assert_eq!(asked.get(), 1, "nothing new, no do-not-disturb probe");
        assert_eq!(
            secrets.reads.get(),
            reads + 1,
            "only the sink still missing its URL looks again"
        );
        let none = route_check(
            &mut router,
            &check(&records, &secrets, &[]),
            &dnd,
            &transport,
        );
        assert_eq!(none, LocalOutcome::default());
        assert_eq!(asked.get(), 1, "no items, no do-not-disturb probe");
        let mixed = [item("a", Severity::Medium), item("h", Severity::High)];
        let reached = route_check(
            &mut router,
            &check(&records, &secrets, &mixed),
            &dnd,
            &transport,
        );
        assert_eq!(
            reached,
            LocalOutcome {
                shown: vec![1],
                notification: true
            },
            "only the new item is shown"
        );
        let (_, body) = transport.posts.borrow()[1].clone();
        assert!(body.contains("high-severity"), "{body}");
    }

    #[test]
    fn do_not_disturb_keeps_this_computer_quiet_and_still_posts_to_sinks() {
        let records = [record(IntegrationKind::SlackIncoming, "ops", true)];
        let secrets = slack_secrets();
        let transport = Recorder::answering(200);
        let mut router = Router::new();
        let items = [item("h", Severity::High)];
        let reached = route_check(
            &mut router,
            &check(&records, &secrets, &items),
            &|| true,
            &transport,
        );
        assert_eq!(reached, LocalOutcome::default());
        assert_eq!(transport.posts.borrow().len(), 1);
    }

    #[test]
    fn a_test_message_goes_to_the_named_integration_even_when_it_is_off() {
        let records = [
            record(IntegrationKind::PagerDuty, "pd", false),
            record(IntegrationKind::GenericJson, "json", true),
        ];
        let secrets = MemorySecretStore::new();
        secrets
            .put(&secret_account("pd", SecretSlot::RoutingKey), ROUTING_KEY)
            .unwrap();
        let transport = Recorder::answering(202);
        let now = Utc.with_ymd_and_hms(2026, 10, 31, 9, 0, 0).unwrap();
        let host = send_test(
            &records,
            &secrets,
            "pd",
            DEFAULT_SERVER_BASE,
            now,
            &transport,
        )
        .unwrap();
        assert_eq!(host, "events.pagerduty.com");
        let (url, body) = transport.posts.borrow()[0].clone();
        assert_eq!(url, "https://events.pagerduty.com/v2/change/enqueue");
        assert!(
            body.contains(&format!(r#""routing_key":"{ROUTING_KEY}""#)),
            "{body}"
        );
        assert_eq!(
            send_test(
                &records,
                &secrets,
                "nope",
                DEFAULT_SERVER_BASE,
                now,
                &transport
            ),
            Err(SinkError::Missing)
        );
        assert_eq!(
            send_test(
                &records,
                &secrets,
                "json",
                DEFAULT_SERVER_BASE,
                now,
                &transport
            ),
            Err(SinkError::Refused)
        );
        let refusing = Recorder::answering(404);
        let err = send_test(
            &records,
            &secrets,
            "pd",
            DEFAULT_SERVER_BASE,
            now,
            &refusing,
        )
        .unwrap_err();
        assert!(!err.to_string().contains(ROUTING_KEY));
    }

    #[test]
    fn the_test_result_bubble_names_the_integration_and_links_nowhere() {
        let sent = test_presentation("ops", &Ok("hooks.slack.com".to_string()), Language::English);
        assert!(sent.show_bubble);
        assert!(!sent.show_os_notification);
        assert_eq!(sent.summary.lines, ["Test sent: ops"]);
        assert!(sent.summary.details_url.is_empty());
        let failed = test_presentation("ops", &Err(SinkError::Unreachable), Language::English);
        assert_eq!(failed.summary.lines, ["Test did not go through: ops"]);
    }

    mod panel {
        use super::{check, item, record, Counting, Recorder, SLACK};
        use crate::routing::{
            gone_presentation, route_consented, route_remote, send_test_consented,
        };
        use chrono::{TimeZone, Utc};
        use stardust_core::api::Severity;
        use stardust_core::consent::{Consent, CONSENT_VERSION};
        use stardust_core::delivery::Outcome;
        use stardust_core::i18n::Language;
        use stardust_core::integrations::{
            secret_account, IntegrationKind, IntegrationRecord, MemorySecretStore, SecretSlot,
            SecretStore,
        };
        use stardust_core::route::Router;
        use stardust_core::sink::SinkError;
        use std::cell::Cell;

        const HOOK: &str = "https://hooks.example/inbound";

        fn accepted(kind: IntegrationKind, id: &str, host: &str) -> IntegrationRecord {
            let mut one = record(kind, id, true);
            one.host = Some(host.to_string());
            one.consent = Some(Consent {
                host: host.to_string(),
                version: CONSENT_VERSION,
            });
            one
        }

        fn secrets() -> Counting {
            let inner = MemorySecretStore::new();
            for (id, slot, value) in [
                ("ops", SecretSlot::Url, SLACK),
                ("json", SecretSlot::Url, HOOK),
                ("json", SecretSlot::SigningSecret, "example-signing-secret"),
            ] {
                inner.put(&secret_account(id, slot), value).unwrap();
            }
            Counting {
                inner,
                reads: Cell::new(0),
            }
        }

        #[test]
        fn without_consent_nothing_is_posted_and_the_keychain_is_not_read() {
            let mut slack = accepted(IntegrationKind::SlackIncoming, "ops", "hooks.slack.com");
            slack.consent = None;
            let mut moved = accepted(IntegrationKind::GenericJson, "json", "hooks.example");
            moved.host = Some("hooks.example.net".to_string());
            let records = [slack, moved];
            let secrets = secrets();
            let transport = Recorder::answering(200);
            let items = [item("a", Severity::High)];
            let outcomes = route_consented(
                &mut Router::new(),
                &check(&records, &secrets, &items),
                &transport,
            );
            assert_eq!(
                outcomes,
                [
                    ("ops".to_string(), Outcome::NeedsConsent),
                    ("json".to_string(), Outcome::NeedsConsent),
                ]
            );
            assert!(transport.urls().is_empty());
            assert_eq!(secrets.reads.get(), 0);
        }

        #[test]
        fn the_saved_url_must_be_on_the_host_that_was_accepted() {
            // The file says one host and the keychain holds another, as a
            // failed save could leave them.
            let records = [accepted(
                IntegrationKind::GenericJson,
                "json",
                "other.example",
            )];
            let secrets = secrets();
            let transport = Recorder::answering(200);
            let items = [item("a", Severity::High)];
            let outcomes = route_consented(
                &mut Router::new(),
                &check(&records, &secrets, &items),
                &transport,
            );
            assert_eq!(outcomes, [("json".to_string(), Outcome::NeedsConsent)]);
            assert!(transport.urls().is_empty());
            let now = Utc.with_ymd_and_hms(2026, 10, 31, 9, 0, 0).unwrap();
            assert_eq!(
                send_test_consented(&check(&records, &secrets, &[]), "json", now, &transport),
                Err(SinkError::NeedsConsent)
            );
            assert!(transport.urls().is_empty());
        }

        #[test]
        fn the_router_takes_the_json_webhook_only_with_the_panel_path() {
            let records = [accepted(
                IntegrationKind::GenericJson,
                "json",
                "hooks.example",
            )];
            let secrets = secrets();
            let items = [item("a", Severity::Low)];
            let off = Recorder::answering(200);
            route_remote(&mut Router::new(), &check(&records, &secrets, &items), &off);
            assert!(off.urls().is_empty(), "as before with the panel off");

            let on = Recorder::answering(204);
            let mut router = Router::new();
            let outcomes = route_consented(&mut router, &check(&records, &secrets, &items), &on);
            assert_eq!(outcomes, [("json".to_string(), Outcome::Delivered)]);
            assert_eq!(on.urls(), [HOOK]);
            let (_, body) = on.posts.borrow()[0].clone();
            assert!(body.contains("\"subject_id\":\"a\""), "{body}");
            let reads = secrets.reads.get();
            let outcomes = route_consented(&mut router, &check(&records, &secrets, &items), &on);
            assert_eq!(on.urls().len(), 1, "once");
            assert_eq!(outcomes, [("json".to_string(), Outcome::NothingWaiting)]);
            assert_eq!(secrets.reads.get(), reads, "no keychain read for nothing");
        }

        #[test]
        fn quiet_hours_hold_without_a_keychain_read_and_say_so() {
            let records = [accepted(
                IntegrationKind::SlackIncoming,
                "ops",
                "hooks.slack.com",
            )];
            let secrets = secrets();
            let transport = Recorder::answering(200);
            let items = [item("m", Severity::Medium)];
            let mut router = Router::new();
            let mut quiet = check(&records, &secrets, &items);
            quiet.quiet = true;
            let outcomes = route_consented(&mut router, &quiet, &transport);
            assert_eq!(outcomes, [("ops".to_string(), Outcome::HeldByQuietHours)]);
            assert_eq!(secrets.reads.get(), 0);
            assert!(transport.urls().is_empty());

            let mixed = [item("m", Severity::Medium), item("h", Severity::High)];
            let mut quiet = check(&records, &secrets, &mixed);
            quiet.quiet = true;
            let outcomes = route_consented(&mut router, &quiet, &transport);
            assert_eq!(
                outcomes,
                [("ops".to_string(), Outcome::HeldByQuietHours)],
                "the high one went out and the medium one still waits"
            );
            assert_eq!(transport.urls().len(), 1);

            let outcomes =
                route_consented(&mut router, &check(&records, &secrets, &mixed), &transport);
            assert_eq!(outcomes, [("ops".to_string(), Outcome::Delivered)]);
            assert_eq!(transport.urls().len(), 2);
        }

        #[test]
        fn mcp_integrations_and_disabled_ones_are_not_routed() {
            let mut orbit = record(IntegrationKind::OrbitMcp, "orbit", true);
            orbit.host = Some("helloorbit.app".to_string());
            let slack_mcp = record(IntegrationKind::SlackMcp, "slack", true);
            let mut off = accepted(IntegrationKind::SlackIncoming, "ops", "hooks.slack.com");
            off.enabled = false;
            let records = [orbit, slack_mcp, off];
            let secrets = secrets();
            let transport = Recorder::answering(200);
            let items = [item("a", Severity::High)];
            let outcomes = route_consented(
                &mut Router::new(),
                &check(&records, &secrets, &items),
                &transport,
            );
            assert!(outcomes.is_empty(), "{outcomes:?}");
            assert!(transport.urls().is_empty());
            assert_eq!(secrets.reads.get(), 0);
        }

        #[test]
        fn an_incomplete_destination_is_not_ready_and_failures_are_reported() {
            let mut unsigned = accepted(IntegrationKind::GenericJson, "bare", "hooks.example");
            unsigned.id = "bare".to_string();
            let records = [unsigned];
            let secrets = secrets();
            secrets
                .put(&secret_account("bare", SecretSlot::Url), HOOK)
                .unwrap();
            let transport = Recorder::answering(200);
            let items = [item("a", Severity::High)];
            assert_eq!(
                route_consented(
                    &mut Router::new(),
                    &check(&records, &secrets, &items),
                    &transport
                ),
                [("bare".to_string(), Outcome::NotReady)],
                "never posted unsigned"
            );
            assert!(transport.urls().is_empty());

            let slack = [accepted(
                IntegrationKind::SlackIncoming,
                "ops",
                "hooks.slack.com",
            )];
            for (status, expected) in [
                (500, Outcome::RetryPending),
                (429, Outcome::RetryPending),
                (404, Outcome::GoneDisabled),
                (410, Outcome::GoneDisabled),
                (400, Outcome::Refused),
                (401, Outcome::Refused),
                (403, Outcome::Refused),
                (302, Outcome::Refused),
            ] {
                let answering = Recorder::answering(status);
                assert_eq!(
                    route_consented(
                        &mut Router::new(),
                        &check(&slack, &secrets, &items),
                        &answering
                    ),
                    [("ops".to_string(), expected)],
                    "{status}"
                );
            }
        }

        #[test]
        fn a_test_needs_consent_but_not_the_switch_and_covers_the_json_webhook() {
            let mut off = accepted(IntegrationKind::GenericJson, "json", "hooks.example");
            off.enabled = false;
            let mut unasked = accepted(IntegrationKind::SlackIncoming, "ops", "hooks.slack.com");
            unasked.consent = None;
            let orbit = record(IntegrationKind::OrbitMcp, "orbit", true);
            let records = [off, unasked, orbit];
            let secrets = secrets();
            let transport = Recorder::answering(204);
            let now = Utc.with_ymd_and_hms(2026, 10, 31, 9, 0, 0).unwrap();
            let routing = check(&records, &secrets, &[]);
            assert_eq!(
                send_test_consented(&routing, "json", now, &transport).as_deref(),
                Ok("hooks.example")
            );
            let (_, body) = transport.posts.borrow()[0].clone();
            assert!(body.contains("stardust.test"), "{body}");
            for (id, expected) in [
                ("ops", SinkError::NeedsConsent),
                ("orbit", SinkError::Refused),
                ("nope", SinkError::Missing),
            ] {
                assert_eq!(
                    send_test_consented(&routing, id, now, &transport),
                    Err(expected),
                    "{id}"
                );
            }
            assert_eq!(transport.urls().len(), 1);
        }

        #[test]
        fn the_gone_bubble_shows_ids_and_links_nowhere() {
            let shown = gone_presentation(&["ops".to_string()], Language::English);
            assert!(shown.show_bubble);
            assert!(!shown.show_os_notification);
            assert_eq!(shown.summary.lines, ["ops: Turned off: destination gone"]);
            assert!(shown.summary.details_url.is_empty());
        }
    }

    #[test]
    fn the_https_transport_refuses_a_private_address_before_any_request() {
        for (url, host) in [
            ("https://127.0.0.1/hook", "127.0.0.1"),
            ("https://10.1.2.3:8443/hook?sig=1", "10.1.2.3"),
        ] {
            let checked = CheckedUrl {
                url: url.to_string(),
                host: host.to_string(),
            };
            assert_eq!(
                HttpsTransport::new().post_json(&checked, b"{}"),
                Err(SinkError::Blocked),
                "{url}"
            );
            assert!(!format!("{checked:?}").contains("hook"));
        }
    }

    #[test]
    fn a_signed_post_through_the_https_transport_gets_the_same_address_check() {
        let checked = CheckedUrl {
            url: "https://127.0.0.1/hook".to_string(),
            host: "127.0.0.1".to_string(),
        };
        let headers = [("Orbit-Signature".to_string(), "t=1,v1=00".to_string())];
        assert_eq!(
            HttpsTransport::new().post_json_with_headers(&checked, b"{}", &headers),
            Err(SinkError::Blocked)
        );
    }

    #[test]
    fn a_failed_lookup_is_not_repeated_in_the_same_check() {
        let transport = HttpsTransport::new();
        let unknown = CheckedUrl {
            url: "https://stardust-test.invalid/hook".to_string(),
            host: "stardust-test.invalid".to_string(),
        };
        let started = Instant::now();
        assert_eq!(
            transport.post_json(&unknown, b"{}"),
            Err(SinkError::Unreachable)
        );
        let slow = started.elapsed() >= super::SLOW_LOOKUP;
        let again = Instant::now();
        assert_eq!(
            transport.post_json(&unknown, b"{}"),
            Err(SinkError::Unreachable)
        );
        assert!(
            again.elapsed() < Duration::from_millis(50),
            "no second lookup"
        );
        let private = CheckedUrl {
            url: "https://127.0.0.1/hook".to_string(),
            host: "127.0.0.1".to_string(),
        };
        let expected = if slow {
            // A slow failure means the resolver is stuck: no more lookups.
            Err(SinkError::Unreachable)
        } else {
            // A quick one is one bad name: the other hosts still go.
            Err(SinkError::Blocked)
        };
        assert_eq!(transport.post_json(&private, b"{}"), expected);
        assert_eq!(
            HttpsTransport::new().post_json(&private, b"{}"),
            Err(SinkError::Blocked),
            "a new check starts fresh"
        );
    }
}
