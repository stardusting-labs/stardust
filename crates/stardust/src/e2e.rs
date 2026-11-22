//! The whole chain, against local servers only: device sign-in, a check of
//! the desktop routes, the router, and the Slack, Teams, and JSON sinks.
//!
//! [`LocalTransport`] keeps the path of each saved destination and posts it
//! to a loopback server, so the vendor host names in these URLs are only
//! text. Nothing here can reach them.

use crate::app::apply_outcomes;
use crate::inbox::{Inbox, UiMsg};
use crate::integrations_cmd::{save_url, settle_consent};
use crate::live::LivePet;
use crate::routing::{route_consented, route_local, CheckRouting};
use chrono::Utc;
use stardust_core::api::{
    AlertItem, DEFAULT_SERVER_BASE, PATH_DESKTOP_ALERTS, PATH_DESKTOP_STATUS, PATH_DEVICE_CODE,
    PATH_DEVICE_TOKEN,
};
use stardust_core::check::{perform_check, CheckKind};
use stardust_core::config::{load_config, save_config, Config, Feature};
use stardust_core::connect::{DeviceFlow, FlowStep};
use stardust_core::delivery::Outcome;
use stardust_core::http::HttpOrbitClient;
use stardust_core::integrations::{IntegrationKind, MemorySecretStore, SecretStore};
use stardust_core::outbound::verify;
use stardust_core::route::Router;
use stardust_core::session::{current_access, stored_from_response, Access};
use stardust_core::testserver::{LocalTransport, Reply, Request, TestServer};
use stardust_core::token::{MemoryTokenStore, TokenStore};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ACCESS_TOKEN: &str = "e2e-access-token-value";
const REFRESH_TOKEN: &str = "e2e-refresh-token-value";
const DEVICE_CODE: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
const USER_CODE: &str = "ABCD-EFGH";
const ORGANIZATION: &str = "11111111-1111-4111-8111-111111111111";
const SIGNING_SECRET: &str = "e2e-signing-secret-value";

const SLACK_PATH: &str = "/services/T000/B000/e2eslackpath";
const TEAMS_PATH: &str = "/workflows/e2eteamspath/triggers/manual/paths/invoke";
const JSON_PATH: &str = "/inbound/e2ejsonpath";

static NEXT: AtomicU64 = AtomicU64::new(0);

/// An alert whose id does not appear in its title, so a test can tell which
/// sinks carry the id.
fn alert_json(name: &str, severity: &str) -> String {
    let immediate = severity == "high";
    format!(
        r#"{{"id":"orbit-id-{name}","severity":"{severity}","title":"Example item {name}","created_at":"2026-10-26T00:00:00Z","workspace_path":"/rust-preview/pqc/studio/app/findings","immediate":{immediate}}}"#
    )
}

/// ORBIT's device sign-in and desktop routes. The desktop routes answer only
/// to the token the sign-in gave out.
fn orbit(alerts: Arc<Mutex<Vec<String>>>) -> TestServer {
    TestServer::start(move |request| {
        let signed_in =
            request.header("authorization") == Some(format!("Bearer {ACCESS_TOKEN}").as_str());
        match request.path() {
            PATH_DEVICE_CODE => Reply::json(
                200,
                &format!(
                    r#"{{"device_code":"{DEVICE_CODE}","user_code":"{USER_CODE}","verification_uri":"https://helloorbit.app/rust-preview/pqc/account","verification_uri_complete":"https://helloorbit.app/rust-preview/pqc/account?user_code={USER_CODE}","expires_in":600,"interval":1}}"#
                ),
            ),
            PATH_DEVICE_TOKEN if request.body.contains(DEVICE_CODE) => {
                let expires = Utc::now().timestamp_millis() + 300_000;
                Reply::json(
                    200,
                    &format!(
                        r#"{{"token":"{ACCESS_TOKEN}","access_token":"{ACCESS_TOKEN}","token_type":"Bearer","expires_in":300,"expires_at_epoch_ms":{expires},"refresh_token":"{REFRESH_TOKEN}"}}"#
                    ),
                )
            }
            PATH_DESKTOP_STATUS | PATH_DESKTOP_ALERTS if !signed_in => {
                Reply::json(401, r#"{"error":"invalid_token"}"#)
            }
            PATH_DESKTOP_STATUS => Reply::json(
                200,
                &format!(
                    r#"{{"organization_id":"{ORGANIZATION}","observation_count":1,"alert_count":1,"high_severity_count":0,"immediate_severity":"high","schedule_owner":"client","workspace_path":"/rust-preview/pqc/studio/app/findings","as_of_unix":1761436800}}"#
                ),
            ),
            PATH_DESKTOP_ALERTS => {
                let listed = alerts.lock().unwrap().join(",");
                Reply::json(200, &format!(r#"{{"alerts":[{listed}]}}"#))
            }
            _ => Reply::empty(404),
        }
    })
}

/// One signed-in computer: the ORBIT client, its token store, and its config
/// with the panel on and each integration accepted the way a person would.
struct Computer {
    client: HttpOrbitClient,
    base: String,
    tokens: MemoryTokenStore,
    config: Mutex<Config>,
    secrets: MemorySecretStore,
    router: Router,
    path: PathBuf,
    live: LivePet,
    inbox: Inbox,
}

impl Drop for Computer {
    fn drop(&mut self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn sign_in(orbit: &TestServer) -> Computer {
    let base = orbit.base();
    let client = HttpOrbitClient::with_timeout(&base, Duration::from_secs(2)).unwrap();
    let mut flow = DeviceFlow::start(&client, &base, Instant::now).unwrap();
    let due = Instant::now() + flow.interval();
    let FlowStep::Approved(response) = flow.step(&client, due, 0) else {
        panic!("the device flow did not finish");
    };
    let tokens = MemoryTokenStore::new();
    tokens
        .store(&stored_from_response(response, Utc::now()).unwrap())
        .unwrap();

    let mut config = Config::default_for("UTC").unwrap();
    config.set_feature(Feature::Routing, true);
    config.set_feature(Feature::IntegrationsPanel, true);
    let dir = std::env::temp_dir().join(format!(
        "stardust-e2e-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    Computer {
        client,
        base,
        tokens,
        config: Mutex::new(config),
        secrets: MemorySecretStore::new(),
        router: Router::new(),
        path: dir.join("config.json"),
        live: LivePet::new(1.0),
        inbox: Inbox::new(),
    }
}

impl Computer {
    /// `integration add`, `url`, and `enable`, answering yes.
    fn connect(&self, kind: IntegrationKind, id: &str, url: &str, secret: Option<&str>) {
        let mut config = self.config.lock().unwrap();
        config.add_integration(kind, id).unwrap();
        save_url(&mut config, &self.secrets, id, url, secret).unwrap();
        let mut said = Vec::new();
        settle_consent(&mut config, id, false, &b"yes\n"[..], &mut said).unwrap();
        config.set_integration_enabled(id, true).unwrap();
        save_config(&self.path, &config).unwrap();
        let said = String::from_utf8(said).unwrap();
        assert!(said.contains("Nothing else."), "{said}");
    }

    fn alerts(&self) -> Vec<AlertItem> {
        let Access::Ready { token, .. } =
            current_access(&self.tokens, Some(&self.client), Utc::now(), false)
        else {
            panic!("not signed in");
        };
        let schedule = self.config.lock().unwrap().schedule().clone();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime
            .block_on(perform_check(
                &self.client,
                token.access_token(),
                Utc::now(),
                &schedule,
                &CheckKind::Manual,
                &self.base,
            ))
            .unwrap()
            .items
    }

    /// One check the way the app runs it. The links use the default ORBIT
    /// address as text, because a sink takes only https links.
    fn check(&mut self, transport: &LocalTransport) -> (Vec<usize>, Vec<(String, Outcome)>) {
        let items = self.alerts();
        let records = self.config.lock().unwrap().integrations().to_vec();
        let check = CheckRouting {
            records: &records,
            secrets: &self.secrets,
            items: &items,
            server_base: DEFAULT_SERVER_BASE,
            quiet: false,
            now: Instant::now(),
            interval_minutes: 60,
        };
        let local = route_local(&mut self.router, &check, &|| false);
        let outcomes = route_consented(&mut self.router, &check, transport);
        apply_outcomes(&self.config, &self.path, &self.live, &self.inbox, &outcomes);
        (local.shown, outcomes)
    }
}

fn outcome(outcomes: &[(String, Outcome)], id: &str) -> Option<Outcome> {
    outcomes
        .iter()
        .find(|(name, _)| name == id)
        .map(|(_, outcome)| *outcome)
}

fn bodies_for<'a>(requests: &'a [Request], name: &str) -> Vec<&'a Request> {
    let title = format!("Example item {name}");
    requests
        .iter()
        .filter(|request| request.body.contains(&title))
        .collect()
}

#[test]
fn sign_in_alerts_router_and_sinks_work_together_without_leaking_anything() {
    let listed = Arc::new(Mutex::new(vec![
        alert_json("a1", "high"),
        alert_json("a2", "medium"),
    ]));
    let orbit = orbit(Arc::clone(&listed));
    let mut computer = sign_in(&orbit);
    let teams_calls = Arc::new(AtomicU32::new(0));
    let sinks = {
        let teams_calls = Arc::clone(&teams_calls);
        TestServer::start(move |request| match request.path() {
            TEAMS_PATH if teams_calls.fetch_add(1, Ordering::SeqCst) == 0 => Reply::empty(500),
            TEAMS_PATH => Reply::empty(202),
            SLACK_PATH => Reply::json(200, "ok"),
            JSON_PATH => Reply::empty(204),
            _ => Reply::empty(400),
        })
    };
    let transport = LocalTransport::to(&sinks);
    computer.connect(
        IntegrationKind::SlackIncoming,
        "ops",
        &format!("https://hooks.slack.com{SLACK_PATH}"),
        None,
    );
    computer.connect(
        IntegrationKind::TeamsIncoming,
        "team",
        &format!(
            "https://prod-00.westus.logic.azure.com:443{TEAMS_PATH}?api-version=1&sig=e2eteamssig"
        ),
        None,
    );
    computer.connect(
        IntegrationKind::GenericJson,
        "json",
        &format!("https://hooks.example{JSON_PATH}"),
        Some(SIGNING_SECRET),
    );
    computer
        .config
        .lock()
        .unwrap()
        .add_integration(IntegrationKind::OrbitMcp, "orbit")
        .unwrap();
    computer
        .config
        .lock()
        .unwrap()
        .set_integration_enabled("orbit", true)
        .unwrap();

    // First check: one message per alert, except that Teams answers 500.
    let (shown, outcomes) = computer.check(&transport);
    assert_eq!(shown, [0, 1], "the bubble takes both once");
    assert_eq!(outcome(&outcomes, "ops"), Some(Outcome::Delivered));
    assert_eq!(outcome(&outcomes, "json"), Some(Outcome::Delivered));
    assert_eq!(outcome(&outcomes, "team"), Some(Outcome::RetryPending));
    assert_eq!(outcome(&outcomes, "orbit"), None, "MCP is never routed");
    assert_eq!(sinks.count(SLACK_PATH), 2);
    assert_eq!(sinks.count(JSON_PATH), 2);
    assert_eq!(
        sinks.count(TEAMS_PATH),
        1,
        "after a 500 the rest wait for the next check"
    );
    assert_eq!(
        computer.live.delivery().get("team"),
        Some(Outcome::RetryPending)
    );

    // Second check: Teams catches up, and nobody else gets a repeat.
    let (shown, outcomes) = computer.check(&transport);
    assert!(shown.is_empty());
    assert_eq!(outcome(&outcomes, "team"), Some(Outcome::Delivered));
    assert_eq!(sinks.count(TEAMS_PATH), 3);
    assert_eq!(sinks.count(SLACK_PATH), 2);
    assert_eq!(sinks.count(JSON_PATH), 2);
    assert_eq!(
        computer.live.delivery().get("ops"),
        Some(Outcome::Delivered),
        "nothing new does not hide the last result"
    );

    // Third check of the same list: nothing moves.
    computer.check(&transport);
    assert_eq!(sinks.count(SLACK_PATH), 2);
    assert_eq!(sinks.count(TEAMS_PATH), 3);
    assert_eq!(sinks.count(JSON_PATH), 2);

    // A new alert goes once to each.
    listed.lock().unwrap().push(alert_json("a3", "low"));
    computer.check(&transport);
    assert_eq!(sinks.count(SLACK_PATH), 3);
    assert_eq!(sinks.count(TEAMS_PATH), 4);
    assert_eq!(sinks.count(JSON_PATH), 3);

    let requests = sinks.requests();
    let by_path = |path: &str| -> Vec<Request> {
        requests
            .iter()
            .filter(|request| request.path() == path)
            .cloned()
            .collect()
    };
    let slack = by_path(SLACK_PATH);
    let teams = by_path(TEAMS_PATH);
    let json = by_path(JSON_PATH);
    for alert in ["a1", "a2", "a3"] {
        assert_eq!(bodies_for(&slack, alert).len(), 1, "slack {alert}");
        assert_eq!(bodies_for(&json, alert).len(), 1, "json {alert}");
    }
    assert_eq!(bodies_for(&teams, "a1").len(), 2, "the first went twice");
    assert_eq!(bodies_for(&teams[1..], "a1").len(), 1, "and was taken once");
    assert_eq!(bodies_for(&teams, "a2").len(), 1);
    assert_eq!(bodies_for(&teams, "a3").len(), 1);

    // Nothing but what consent stated went out.
    let secrets = [
        ACCESS_TOKEN,
        REFRESH_TOKEN,
        DEVICE_CODE,
        USER_CODE,
        ORGANIZATION,
        SIGNING_SECRET,
        "e2eslackpath",
        "e2eteamspath",
        "e2eteamssig",
        "e2ejsonpath",
        "hooks.slack.com",
        "logic.azure.com",
        "hooks.example",
    ];
    for request in &requests {
        for secret in secrets {
            assert!(!request.body.contains(secret), "{secret}: {}", request.body);
            for (name, value) in &request.headers {
                assert!(!value.contains(secret), "{secret} in {name}");
            }
        }
    }
    for request in slack.iter().chain(&teams) {
        assert!(!request.body.contains("orbit-id-"), "{}", request.body);
        assert!(!request.body.contains("tenant"), "{}", request.body);
        assert!(
            request.body.contains("https://helloorbit.app/rust-preview"),
            "{}",
            request.body
        );
    }

    // Every JSON post is signed, and the signature checks out.
    let now = Utc::now().timestamp();
    for request in &json {
        let signature = request.header("orbit-signature").unwrap();
        assert!(verify(
            SIGNING_SECRET,
            signature,
            request.body.as_bytes(),
            now
        ));
        assert_eq!(request.header("orbit-event"), Some("finding.created"));
        assert_eq!(
            request.header("orbit-delivery"),
            request.json()["event_id"].as_str()
        );
        assert!(request.body.contains("orbit-id-"), "{}", request.body);
        assert_eq!(request.json()["check_interval_minutes"], 60);
    }

    // Keychain items stay out of the file on disk.
    let file = std::fs::read_to_string(&computer.path).unwrap();
    for secret in [SIGNING_SECRET, "e2eslackpath", "e2eteamssig", "e2ejsonpath"] {
        assert!(!file.contains(secret), "{secret}");
    }
    assert_eq!(
        computer
            .secrets
            .get(&stardust_core::integrations::secret_account(
                "json",
                stardust_core::integrations::SecretSlot::SigningSecret
            ))
            .unwrap()
            .as_deref(),
        Some(SIGNING_SECRET)
    );
}

#[test]
fn a_destination_that_is_gone_is_switched_off_saved_and_announced_once() {
    let listed = Arc::new(Mutex::new(vec![alert_json("a1", "medium")]));
    let orbit = orbit(Arc::clone(&listed));
    let mut computer = sign_in(&orbit);
    let sinks = TestServer::start(|request| match request.path() {
        JSON_PATH => Reply::empty(410),
        SLACK_PATH => Reply::json(404, "no_service"),
        _ => Reply::json(401, "invalid_token"),
    });
    let transport = LocalTransport::to(&sinks);
    computer.connect(
        IntegrationKind::GenericJson,
        "json",
        &format!("https://hooks.example{JSON_PATH}"),
        Some(SIGNING_SECRET),
    );
    computer.connect(
        IntegrationKind::SlackIncoming,
        "ops",
        &format!("https://hooks.slack.com{SLACK_PATH}"),
        None,
    );
    computer.connect(
        IntegrationKind::SlackIncoming,
        "locked",
        "https://hooks.slack.com/services/T000/B000/locked",
        None,
    );
    while computer.inbox.try_pop().is_some() {}

    let (_, outcomes) = computer.check(&transport);
    assert_eq!(outcome(&outcomes, "json"), Some(Outcome::GoneDisabled));
    assert_eq!(outcome(&outcomes, "ops"), Some(Outcome::GoneDisabled));
    assert_eq!(
        outcome(&outcomes, "locked"),
        Some(Outcome::Refused),
        "a 401 can be fixed at the other end, so it stays on"
    );

    let mut lines = Vec::new();
    while let Some(message) = computer.inbox.try_pop() {
        if let UiMsg::Checked(presentation) = message {
            assert!(presentation.show_bubble);
            assert!(!presentation.show_os_notification);
            assert!(presentation.summary.details_url.is_empty());
            lines.extend(presentation.summary.lines);
        }
    }
    assert_eq!(
        lines,
        [
            "json: Turned off: destination gone",
            "ops: Turned off: destination gone"
        ]
    );

    let saved = load_config(&computer.path).unwrap().unwrap();
    let on_disk = |id: &str| {
        saved
            .integrations()
            .iter()
            .find(|record| record.id == id)
            .map(|record| record.enabled)
    };
    assert_eq!(on_disk("json"), Some(false));
    assert_eq!(on_disk("ops"), Some(false));
    assert_eq!(on_disk("locked"), Some(true));

    let before = (sinks.count(JSON_PATH), sinks.count(SLACK_PATH));
    let (_, outcomes) = computer.check(&transport);
    assert_eq!((sinks.count(JSON_PATH), sinks.count(SLACK_PATH)), before);
    assert_eq!(outcome(&outcomes, "json"), None);
    assert_eq!(
        computer.live.delivery().get("json"),
        Some(Outcome::GoneDisabled),
        "the note stays for the panel"
    );
    let mut bubbles = 0;
    while let Some(message) = computer.inbox.try_pop() {
        if matches!(message, UiMsg::Checked(_)) {
            bubbles += 1;
        }
    }
    assert_eq!(bubbles, 0, "announced once");
}

#[test]
fn turning_the_panel_on_stops_an_integration_until_it_is_accepted() {
    let listed = Arc::new(Mutex::new(vec![alert_json("a1", "high")]));
    let orbit = orbit(Arc::clone(&listed));
    let mut computer = sign_in(&orbit);
    let sinks = TestServer::start(|_| Reply::json(200, "ok"));
    let transport = LocalTransport::to(&sinks);
    {
        // An integration from before the panel: on, with no recorded yes.
        let mut config = computer.config.lock().unwrap();
        config
            .add_integration(IntegrationKind::SlackIncoming, "ops")
            .unwrap();
        save_url(
            &mut config,
            &computer.secrets,
            "ops",
            &format!("https://hooks.slack.com{SLACK_PATH}"),
            None,
        )
        .unwrap();
        config.set_integration_enabled("ops", true).unwrap();
        save_config(&computer.path, &config).unwrap();
    }
    let (_, outcomes) = computer.check(&transport);
    assert_eq!(outcome(&outcomes, "ops"), Some(Outcome::NeedsConsent));
    assert_eq!(sinks.requests().len(), 0, "nothing leaves without a yes");

    {
        let mut config = computer.config.lock().unwrap();
        config.accept_integration_consent("ops").unwrap();
        save_config(&computer.path, &config).unwrap();
    }
    let (_, outcomes) = computer.check(&transport);
    assert_eq!(outcome(&outcomes, "ops"), Some(Outcome::Delivered));
    assert_eq!(sinks.count(SLACK_PATH), 1, "the alert was kept for this");
}
