use crate::actions::{apply_menu, MenuEffect};
use crate::cli::{self, AutostartAction, CharacterAction, Command, FeatureAction};
use crate::deliver;
use crate::inbox::{Inbox, SignInUpdate, UiMsg};
use crate::keychain::KeychainTokenStore;
use crate::live::LivePet;
use crate::notify;
use crate::routing::{self, CheckRouting, HttpsTransport};
use crate::tray::{tray_text, Tray};
use crate::window::{self, RunError, Session};
use chrono::{DateTime, Utc};
use stardust_core::api::{
    resolve_server_base, AlertItem, DesktopApi, DesktopStatus, MockOrbitClient, OrbitError,
    DEFAULT_SERVER_BASE, ENV_SERVER_URL,
};
use stardust_core::check::{
    jittered_retry_delay, note_anchor, note_success, perform_check, retry_delay, CheckKind,
};
use stardust_core::config::{default_config_path, load_config, save_config, Config, Feature};
use stardust_core::connect::{DeviceFlow, FlowPhase, FlowStep};
use stardust_core::consent::{destination_host, needs_consent};
use stardust_core::delivery::{outcome_of_test, Outcome};
use stardust_core::http::{HttpOrbitClient, Revoke};
use stardust_core::integrations::{IntegrationRecord, SecretStore};
use stardust_core::outbound::{prepare_posts, DeliveryNote, DeliveryWindow};
use stardust_core::pet::{pet_pose, tray_glyph, ObservedCheck};
use stardust_core::policy::{calm_notice_in, present, Notice};
use stardust_core::position::ScreenPoint;
use stardust_core::route::Router;
use stardust_core::schedule::{parse_timezone, FireSource};
use stardust_core::session::{current_access, stored_from_response, Access, Refresher};
use stardust_core::text::{menu_text, test_send_target};
use stardust_core::token::TokenStore;
use std::cell::RefCell;
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub fn run(args: impl IntoIterator<Item = impl AsRef<str>>) -> Result<(), String> {
    match cli::parse(args)? {
        Command::Help => {
            print!("{}", cli::usage());
            Ok(())
        }
        Command::Run {
            config,
            headless,
            demo,
        } => run_loop(config_path(config), headless, demo),
        Command::CheckOnce { config, demo } => check_once(&config_path(config), demo),
        Command::SignIn { config } => {
            let path = config_path(config);
            let loaded = load_or_create(&path)?;
            crate::signin::sign_in(&loaded)
        }
        Command::SignOut { config } => {
            // An unreadable or missing config file signs out as before.
            let loaded = load_config(&config_path(config)).ok().flatten();
            crate::signin::sign_out(loaded.as_ref())
        }
        Command::Feature { config, action } => {
            let path = config_path(config);
            let mut loaded = load_or_create(&path)?;
            if let FeatureAction::Set { feature, on } = action {
                loaded.set_feature(feature, on);
                save_config(&path, &loaded).map_err(|err| err.to_string())?;
            }
            for line in feature_lines(&loaded, action) {
                println!("{line}");
            }
            Ok(())
        }
        Command::Autostart(action) => {
            match action {
                AutostartAction::On => {
                    crate::autostart::set_enabled(true).map_err(|err| err.to_string())?
                }
                AutostartAction::Off => {
                    crate::autostart::set_enabled(false).map_err(|err| err.to_string())?
                }
                AutostartAction::Status => {}
            }
            let state = if crate::autostart::is_enabled() {
                "on"
            } else {
                "off"
            };
            println!("start at login {state}");
            Ok(())
        }
        Command::ScheduleShow { config } => {
            let path = config_path(config);
            let loaded = load_or_create(&path)?;
            show_schedule(&path, &loaded);
            Ok(())
        }
        Command::ScheduleSet { config, update } => {
            let path = config_path(config);
            let mut loaded = load_or_create(&path)?;
            loaded
                .apply_schedule_update(&update, Utc::now())
                .map_err(|err| err.to_string())?;
            save_config(&path, &loaded).map_err(|err| err.to_string())?;
            show_schedule(&path, &loaded);
            Ok(())
        }
        Command::Character { config, action } => {
            let path = config_path(config);
            let mut loaded = load_or_create(&path)?;
            match action {
                CharacterAction::Show => loaded.set_character_visible(true),
                CharacterAction::Hide => loaded.set_character_visible(false),
                CharacterAction::Position(x, y) => {
                    loaded.set_character_position(ScreenPoint { x, y })
                }
            }
            save_config(&path, &loaded).map_err(|err| err.to_string())?;
            show_schedule(&path, &loaded);
            Ok(())
        }
        Command::Integration { config, action } => {
            crate::integrations_cmd::apply(&config_path(config), action)
        }
    }
}

/// What `stardust feature ...` prints. The integrations-panel row is left
/// out of the answer to setting another feature while the panel is off, so
/// that answer stays as it was; `feature list` always shows it.
fn feature_lines(config: &Config, action: FeatureAction) -> Vec<String> {
    let panel = config.features().integrations_panel;
    let shows_panel_row = panel
        || match action {
            FeatureAction::List => true,
            FeatureAction::Set { feature, .. } => feature == Feature::IntegrationsPanel,
        };
    let mut lines: Vec<String> = Feature::ALL
        .into_iter()
        .filter(|feature| *feature != Feature::IntegrationsPanel || shows_panel_row)
        .map(|feature| {
            let state = if config.feature(feature) { "on" } else { "off" };
            format!("{} {state}", feature.name())
        })
        .collect();
    if !panel {
        return lines;
    }
    if matches!(
        action,
        FeatureAction::Set {
            feature: Feature::IntegrationsPanel,
            on: true
        }
    ) {
        // A running app keeps the setting it started with and writes it back.
        lines.push("If Stardust is running, quit it and start it again to use this.".to_string());
    }
    // Turning the panel on stops these until accepted. Say so.
    for record in config.integrations().iter().filter(|record| {
        record.enabled && needs_consent(record) && destination_host(record).is_some()
    }) {
        let id = &record.id;
        lines.push(format!(
            "{id} sends nothing until accepted: stardust integration enable {id}"
        ));
    }
    lines
}

fn config_path(config: Option<PathBuf>) -> PathBuf {
    config.unwrap_or_else(default_config_path)
}

fn initial_timezone() -> String {
    match iana_time_zone::get_timezone() {
        Ok(name) => match parse_timezone(&name) {
            Ok(timezone) => timezone.name().to_string(),
            Err(_) => "UTC".to_string(),
        },
        Err(_) => "UTC".to_string(),
    }
}

fn load_or_create(path: &Path) -> Result<Config, String> {
    match load_config(path).map_err(|err| err.to_string())? {
        Some(config) => Ok(config),
        None => {
            let config = Config::default_for(&initial_timezone()).map_err(|err| err.to_string())?;
            save_config(path, &config).map_err(|err| err.to_string())?;
            Ok(config)
        }
    }
}

fn show_schedule(path: &Path, config: &Config) {
    let text = menu_text(config, Utc::now());
    let visibility = if config.character_visible() {
        "shown"
    } else {
        "hidden"
    };
    let position = config.character_position();
    let skin = match config.pet_skin() {
        stardust_core::pet::PetSkin::Astronaut => "astronaut",
        stardust_core::pet::PetSkin::Star => "star",
    };
    println!("{}", path.display());
    println!("character {visibility} at {}, {}", position.x, position.y);
    println!("pet {skin} at {}x", config.pet_scale().factor());
    println!("{}", text.schedule);
    println!("{}", text.integrations);
    println!("{}", text.last_checked);
    println!("{}", text.next_check);
}

fn check_once(path: &Path, demo: bool) -> Result<(), String> {
    let loaded = load_or_create(path)?;
    let base = server_base_of(&loaded)?;
    let backend = open_backend(demo, &base)?;
    let config = Arc::new(Mutex::new(loaded));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|err| err.to_string())?;
    let signal = runtime.block_on(run_check(
        &backend,
        &config,
        path,
        &KeychainTokenStore::new(),
        &LivePet::new(1.0),
        &mut Router::new(),
        &CheckKind::Manual,
        !demo,
    ));
    for line in &signal.presentation.summary.lines {
        println!("{line}");
    }
    if !signal.presentation.summary.details_url.is_empty() {
        println!("{}", signal.presentation.summary.details_url);
    }
    if signal.presentation.show_os_notification {
        notify::show(&signal.presentation);
    }
    if let Some(posts) = signal.posts {
        deliver::deliver(&posts.notes);
        if !posts.items.is_empty() {
            // The alerts already sent are remembered only by a running app,
            // so a one-off check would post every listed alert again.
            eprintln!("stardust: per-alert posts are made by the running app, not by --check-once");
        }
    }
    Ok(())
}

fn run_loop(path: PathBuf, headless: bool, demo: bool) -> Result<(), String> {
    let config = Arc::new(Mutex::new(load_or_create(&path)?));
    let inbox = Inbox::new();
    let manual = Arc::new(AtomicBool::new(false));
    let quit = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(tokio::sync::Notify::new());
    let tray = Rc::new(RefCell::new(None));
    let live = LivePet::new(1.0);
    let scheduler = {
        let config = Arc::clone(&config);
        let manual = Arc::clone(&manual);
        let quit = Arc::clone(&quit);
        let wake = Arc::clone(&wake);
        let inbox = inbox.clone();
        let path = path.clone();
        let live = live.clone();
        let base = {
            let loaded = config.lock().unwrap_or_else(|err| err.into_inner());
            server_base_of(&loaded)?
        };
        let backend = open_backend(demo, &base)?;
        thread::Builder::new()
            .name("stardust-schedule".to_string())
            .spawn(move || {
                scheduler_thread(ScheduleJob {
                    config,
                    path,
                    manual,
                    quit,
                    wake,
                    inbox,
                    live,
                    backend,
                    headless,
                })
            })
            .map_err(|err| err.to_string())?
    };
    let mut windows_unavailable = headless;
    if !headless {
        let session = Session {
            config_path: path.clone(),
            config: Arc::clone(&config),
            inbox: inbox.clone(),
            manual: Arc::clone(&manual),
            quit: Arc::clone(&quit),
            wake: Arc::clone(&wake),
            tray: Rc::clone(&tray),
            live: live.clone(),
            demo,
        };
        match window::run(session) {
            Ok(()) => {}
            Err(RunError::Unavailable(err) | RunError::Failed(err)) => {
                let auto_sign_in = config
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .features()
                    .auto_sign_in;
                windows_unavailable = true;
                eprintln!("stardust: windows are unavailable: {err}");
                inbox.clear_proxy();
                if tray.borrow().is_none() {
                    let loaded = config.lock().unwrap_or_else(|err| err.into_inner());
                    let language = loaded.effective_language();
                    // The window did not get far enough to read the keychain.
                    // Demo mode needs no look; otherwise the connection line
                    // waits for the first check rather than open a keychain
                    // session just for it.
                    if auto_sign_in && demo {
                        live.set_signed_in(true);
                    }
                    let text = tray_text(&loaded, &live, language);
                    let glyph = glyph_for(&loaded, live.observed());
                    drop(loaded);
                    match Tray::install(&inbox, &text, glyph, live.scale_factor(), language) {
                        Ok(installed) => *tray.borrow_mut() = Some(installed),
                        Err(err) => {
                            quit.store(true, Ordering::SeqCst);
                            wake.notify_one();
                            let _ = scheduler.join();
                            return Err(err);
                        }
                    }
                }
                while let Some(message) = inbox.try_pop() {
                    match message {
                        UiMsg::Checked(presentation) => print_lines(&presentation.summary.lines),
                        UiMsg::SignIn(update) if auto_sign_in => print_sign_in(&update),
                        UiMsg::SignIn(_) | UiMsg::Menu(_) | UiMsg::Pet => {}
                    }
                }
            }
        }
    }
    while windows_unavailable && !quit.load(Ordering::SeqCst) {
        match inbox.wait_timeout(Duration::from_secs(45)) {
            Some(UiMsg::Menu(id)) => {
                let panel_on = config
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .features()
                    .integrations_panel;
                if panel_on && id == "integrations" {
                    // Clickable only with the panel on, and it needs a window.
                    println!(
                        "The integrations page needs a window. Use: stardust integration list"
                    );
                    continue;
                }
                if let Some(request) = timer_request(&config, &live, &id) {
                    request.queue(&live);
                    wake.notify_one();
                    refresh_tray(&config, &live, &tray);
                    continue;
                }
                let effect = {
                    let mut loaded = config.lock().unwrap_or_else(|err| err.into_inner());
                    loaded.adopt_saved_integrations(&path);
                    match apply_menu(&mut loaded, &id, Utc::now()) {
                        Ok(MenuEffect::Refresh) => {
                            if let Err(err) = save_config(&path, &loaded) {
                                eprintln!("stardust: could not save config: {err}");
                            }
                            MenuEffect::Refresh
                        }
                        Ok(effect) => effect,
                        Err(err) => {
                            eprintln!("stardust: could not apply menu: {err}");
                            MenuEffect::None
                        }
                    }
                };
                match effect {
                    MenuEffect::None | MenuEffect::Refresh => {}
                    MenuEffect::CheckNow => {
                        manual.store(true, Ordering::SeqCst);
                        wake.notify_one();
                    }
                    MenuEffect::Quit => {
                        quit.store(true, Ordering::SeqCst);
                        wake.notify_one();
                        break;
                    }
                }
                refresh_tray(&config, &live, &tray);
            }
            Some(UiMsg::Checked(presentation)) => {
                print_lines(&presentation.summary.lines);
                refresh_tray(&config, &live, &tray);
            }
            Some(UiMsg::SignIn(update)) => {
                // Only the check-timer sign-in reaches this loop; the
                // settings window, which starts the other one, is not open.
                let auto = config
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .features()
                    .auto_sign_in;
                if auto {
                    print_sign_in(&update);
                    if matches!(update, SignInUpdate::Done) {
                        manual.store(true, Ordering::SeqCst);
                        wake.notify_one();
                    }
                    refresh_tray(&config, &live, &tray);
                }
            }
            Some(UiMsg::Pet) | None => refresh_tray(&config, &live, &tray),
        }
    }
    quit.store(true, Ordering::SeqCst);
    wake.notify_one();
    let _ = scheduler.join();
    Ok(())
}

struct ScheduleJob {
    config: Arc<Mutex<Config>>,
    path: PathBuf,
    manual: Arc<AtomicBool>,
    quit: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
    inbox: Inbox,
    live: LivePet,
    backend: Backend,
    /// `--headless`: no tray and no window, so nothing could show a sign-in.
    headless: bool,
}

fn scheduler_thread(job: ScheduleJob) {
    let ScheduleJob {
        config,
        path,
        manual,
        quit,
        wake,
        inbox,
        live,
        backend,
        headless,
    } = job;
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("stardust: scheduler failed to start: {err}");
            return;
        }
    };
    let tokens = KeychainTokenStore::new();
    let require_token = matches!(backend, Backend::Live(_));
    let mut router = Router::new();
    let mut flow: Option<DeviceFlow> = None;
    if require_token
        && !headless
        && first_run_connect(&config)
        && matches!(tokens.load(Utc::now()), Ok(None))
    {
        live.request_connect();
    }
    runtime.block_on(async move {
        let mut failures = 0_u32;
        // Kept while a device sign-in wakes the loop between checks, so its
        // polls do not push the next check back.
        let mut planned: Option<(Instant, Planned)> = None;
        loop {
            if quit.load(Ordering::SeqCst) {
                break;
            }
            run_requests(&backend, &config, &path, &tokens, &live, &inbox, &mut flow);
            if manual.swap(false, Ordering::SeqCst) {
                let signal = run_check(
                    &backend,
                    &config,
                    &path,
                    &tokens,
                    &live,
                    &mut router,
                    &CheckKind::Manual,
                    require_token,
                )
                .await;
                failures = next_failures(failures, signal.soften);
                publish(&inbox, &tokens, &mut router, signal, &config, &path, &live);
                planned = None;
                continue;
            }
            let now = Instant::now();
            let due = match &planned {
                Some((due, _)) => *due,
                None => {
                    let next = plan_next(&config, failures);
                    let due = now + next.delay;
                    planned = Some((due, next));
                    due
                }
            };
            let mut wait = due.saturating_duration_since(now);
            if let Some(flow) = &flow {
                wait = wait.min(flow.wait(now));
            }
            if tokio::time::timeout(wait, wake.notified()).await.is_ok() {
                planned = None;
                continue;
            }
            if flow.is_some() {
                step_flow(&backend, &tokens, &live, &inbox, &mut flow);
                if Instant::now() < due {
                    continue;
                }
            }
            let Some((_, next)) = planned.take() else {
                continue;
            };
            let Some(slot) = next.slot else {
                continue;
            };
            let kind = CheckKind::Scheduled {
                slot: slot.at,
                source: slot.source,
            };
            let signal = run_check(
                &backend,
                &config,
                &path,
                &tokens,
                &live,
                &mut router,
                &kind,
                require_token,
            )
            .await;
            failures = next_failures(failures, signal.soften);
            publish(&inbox, &tokens, &mut router, signal, &config, &path, &live);
        }
    });
}

/// Hand a check to the tray and the windows, then make its sink posts, so
/// the bubble does not wait for the network.
fn publish(
    inbox: &Inbox,
    tokens: &KeychainTokenStore,
    router: &mut Router,
    signal: CheckSignal,
    config: &Mutex<Config>,
    path: &Path,
    live: &LivePet,
) {
    inbox.push(UiMsg::Pet);
    if signal.publish {
        inbox.push(UiMsg::Checked(signal.presentation));
    }
    if let Some(posts) = signal.posts {
        deliver::deliver(&posts.notes);
        if posts.panel {
            let outcomes =
                routing::route_consented(router, &posts.routing(tokens), &HttpsTransport::new());
            apply_outcomes(config, path, live, inbox, &outcomes);
        } else {
            routing::route_remote(router, &posts.routing(tokens), &HttpsTransport::new());
        }
    }
}

/// With the integrations panel on, keep each integration's result for the
/// panel and the tray. A destination that says it is gone (404 or 410) is
/// switched off and saved, with a bubble so it is not missed.
pub(crate) fn apply_outcomes(
    config: &Mutex<Config>,
    path: &Path,
    live: &LivePet,
    inbox: &Inbox,
    outcomes: &[(String, Outcome)],
) {
    if outcomes.is_empty() {
        return;
    }
    for (id, outcome) in outcomes {
        live.record_delivery(id, *outcome);
    }
    let gone: Vec<String> = outcomes
        .iter()
        .filter(|(_, outcome)| *outcome == Outcome::GoneDisabled)
        .map(|(id, _)| id.clone())
        .collect();
    if !gone.is_empty() {
        let language = {
            let mut loaded = config.lock().unwrap_or_else(|err| err.into_inner());
            loaded.adopt_saved_integrations(path);
            for id in &gone {
                // It may have been removed from the file since this check.
                if loaded.set_integration_enabled(id, false).is_ok() {
                    eprintln!("stardust: integration {id} was turned off: its destination is gone");
                }
            }
            if let Err(err) = save_config(path, &loaded) {
                eprintln!("stardust: could not save config: {err}");
            }
            loaded.effective_language()
        };
        inbox.push(UiMsg::Checked(routing::gone_presentation(&gone, language)));
    }
    inbox.push(UiMsg::Pet);
}

fn next_failures(current: u32, soften: bool) -> u32 {
    if soften {
        current.saturating_add(1)
    } else {
        0
    }
}

/// Auto sign-in starts by itself only on a first run: the welcome window
/// was never closed. After that it starts from a click on "Sign in".
fn first_run_connect(config: &Mutex<Config>) -> bool {
    let loaded = config.lock().unwrap_or_else(|err| err.into_inner());
    loaded.features().auto_sign_in && !loaded.onboarded()
}

/// A tray click the check timer carries out with the newer features on.
enum TimerRequest {
    Connect,
    Test(String),
}

impl TimerRequest {
    fn queue(self, live: &LivePet) {
        match self {
            Self::Connect => live.request_connect(),
            Self::Test(id) => live.request_test(&id),
        }
    }
}

fn timer_request(config: &Mutex<Config>, live: &LivePet, id: &str) -> Option<TimerRequest> {
    let features = config
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .features();
    if features.routing {
        if let Some(target) = test_send_target(id) {
            return Some(TimerRequest::Test(target.to_string()));
        }
    }
    if features.auto_sign_in && id == "sign-in" {
        // Not known yet: look now, on the click, rather than at start.
        let signed_in = live.signed_in().unwrap_or_else(|| {
            let found = crate::signin::signed_in();
            live.set_signed_in(found);
            found
        });
        if !signed_in {
            return Some(TimerRequest::Connect);
        }
    }
    None
}

/// Requests from the tray and the settings window, carried out on the check
/// timer so no other thread does network work.
fn run_requests(
    backend: &Backend,
    config: &Mutex<Config>,
    path: &Path,
    tokens: &KeychainTokenStore,
    live: &LivePet,
    inbox: &Inbox,
    flow: &mut Option<DeviceFlow>,
) {
    if let Some(ticket) = live.take_sign_out() {
        if let Backend::Live(client) = backend {
            finish_sign_out(client, tokens, ticket.refresh_token());
        }
        // A check that was running at sign-out may have marked this
        // computer signed in again.
        live.set_signed_in(false);
        live.set_observed(ObservedCheck::SignedOut);
        inbox.push(UiMsg::Pet);
    }
    if live.take_connect() && flow.is_none() {
        if let Backend::Live(client) = backend {
            start_flow(client, config, live, inbox, flow);
        }
    }
    let tests = live.take_tests();
    if !tests.is_empty() {
        send_tests(config, path, tokens, live, inbox, &tests);
    }
}

/// The settings window already deleted the keychain record. A check that was
/// running then may have saved a rotated one, so delete again and revoke
/// every refresh token seen. A server without the revoke route answers 404,
/// which is fine.
fn finish_sign_out(client: &HttpOrbitClient, tokens: &KeychainTokenStore, refresh: Option<&str>) {
    let rotated = tokens
        .load(Utc::now())
        .ok()
        .flatten()
        .and_then(|token| token.refresh_token().map(str::to_string));
    if let Err(err) = tokens.clear() {
        eprintln!("stardust: could not sign out: {err}");
    }
    let mut presented: Vec<&str> = refresh.into_iter().collect();
    if let Some(rotated) = rotated.as_deref() {
        if !presented.contains(&rotated) {
            presented.push(rotated);
        }
    }
    for token in presented {
        match client.revoke_refresh_token(token) {
            Revoke::Revoked => eprintln!("stardust: ORBIT revoked the refresh token"),
            Revoke::Unsupported => {}
            Revoke::Unreachable => {
                eprintln!("stardust: ORBIT could not be reached to revoke the refresh token");
            }
        }
    }
}

fn start_flow(
    client: &HttpOrbitClient,
    config: &Mutex<Config>,
    live: &LivePet,
    inbox: &Inbox,
    flow: &mut Option<DeviceFlow>,
) {
    let server_base = {
        let loaded = config.lock().unwrap_or_else(|err| err.into_inner());
        server_base_of(&loaded).unwrap_or_else(|_| DEFAULT_SERVER_BASE.to_string())
    };
    match DeviceFlow::start(client, &server_base, Instant::now) {
        Ok(started) => {
            let user_code = started.user_code().to_string();
            live.set_phase(FlowPhase::Waiting {
                user_code: user_code.clone(),
            });
            inbox.push(UiMsg::Pet);
            inbox.push(UiMsg::SignIn(SignInUpdate::Code(user_code)));
            // https on the configured host only. Detached, so a browser that
            // stays in the foreground does not hold up the check timer.
            if let Some(url) = started.verification_url() {
                if let Err(err) = open::that_detached(url) {
                    eprintln!("stardust: could not open the browser: {err}");
                }
            }
            *flow = Some(started);
        }
        Err(err) => {
            eprintln!("stardust: sign-in: {err}");
            live.set_phase(FlowPhase::Idle);
            inbox.push(UiMsg::Pet);
            inbox.push(UiMsg::SignIn(SignInUpdate::Failed));
        }
    }
}

/// One device-token poll, when it is due.
fn step_flow(
    backend: &Backend,
    tokens: &KeychainTokenStore,
    live: &LivePet,
    inbox: &Inbox,
    flow: &mut Option<DeviceFlow>,
) {
    let Backend::Live(client) = backend else {
        *flow = None;
        return;
    };
    let Some(current) = flow.as_mut() else {
        return;
    };
    let user_code = current.user_code().to_string();
    let step = current.step(client, Instant::now(), entropy());
    let phase = match step {
        FlowStep::Wait(_) => return,
        FlowStep::Pending(_) => FlowPhase::Waiting { user_code },
        FlowStep::Offline(_) => FlowPhase::Offline { user_code },
        FlowStep::Approved(token) => {
            *flow = None;
            live.set_phase(FlowPhase::Idle);
            let saved = stored_from_response(token, Utc::now())
                .map_err(|err| err.to_string())
                .and_then(|stored| tokens.store(&stored).map_err(|err| err.to_string()));
            match saved {
                Ok(()) => {
                    live.set_signed_in(true);
                    live.set_observed(ObservedCheck::Waiting);
                    inbox.push(UiMsg::Pet);
                    inbox.push(UiMsg::SignIn(SignInUpdate::Done));
                }
                Err(err) => {
                    eprintln!("stardust: sign-in: {err}");
                    inbox.push(UiMsg::Pet);
                    inbox.push(UiMsg::SignIn(SignInUpdate::Failed));
                }
            }
            return;
        }
        FlowStep::Ended(err) => {
            *flow = None;
            eprintln!("stardust: sign-in: {err}");
            live.set_phase(FlowPhase::Idle);
            inbox.push(UiMsg::Pet);
            inbox.push(UiMsg::SignIn(SignInUpdate::Failed));
            return;
        }
    };
    if live.phase() != phase {
        live.set_phase(phase);
        inbox.push(UiMsg::Pet);
    }
}

fn send_tests(
    config: &Mutex<Config>,
    path: &Path,
    tokens: &KeychainTokenStore,
    live: &LivePet,
    inbox: &Inbox,
    ids: &[String],
) {
    let (records, server_base, language, features, interval_minutes) = {
        let loaded = config.lock().unwrap_or_else(|err| err.into_inner());
        (
            loaded.integrations().to_vec(),
            server_base_of(&loaded).unwrap_or_else(|_| DEFAULT_SERVER_BASE.to_string()),
            loaded.effective_language(),
            loaded.features(),
            loaded.schedule().interval_minutes().unwrap_or(1440),
        )
    };
    if !features.routes_remote() {
        return;
    }
    for id in ids {
        let result = if features.integrations_panel {
            let check = CheckRouting {
                records: &records,
                secrets: tokens,
                items: &[],
                server_base: &server_base,
                quiet: false,
                now: Instant::now(),
                interval_minutes,
            };
            routing::send_test_consented(&check, id, Utc::now(), &HttpsTransport::new())
        } else {
            routing::send_test(
                &records,
                tokens,
                id,
                &server_base,
                Utc::now(),
                &HttpsTransport::new(),
            )
        };
        match &result {
            Ok(host) => eprintln!("stardust: test sent to {id} ({host})"),
            Err(err) => eprintln!("stardust: test to {id} did not go through: {err}"),
        }
        inbox.push(UiMsg::Checked(routing::test_presentation(
            id, &result, language,
        )));
        if features.integrations_panel {
            let outcome = (id.clone(), outcome_of_test(&result));
            apply_outcomes(config, path, live, inbox, &[outcome]);
        }
    }
}

/// Random bits for backoff jitter. std only: every `RandomState` is keyed
/// differently, from a per-thread random seed.
fn entropy() -> u64 {
    std::collections::hash_map::RandomState::new().hash_one(0_u8)
}

struct Planned {
    delay: Duration,
    slot: Option<PlannedSlot>,
}

struct PlannedSlot {
    at: DateTime<Utc>,
    source: FireSource,
}

fn plan_next(config: &Mutex<Config>, failures: u32) -> Planned {
    let loaded = config.lock().unwrap_or_else(|err| err.into_inner());
    let jittered = loaded.features().jittered_backoff;
    let backoff = |delay: Duration| {
        if jittered {
            jittered_retry_delay(delay, failures, entropy())
        } else {
            retry_delay(delay, failures)
        }
    };
    let now = Utc::now();
    let timezone = loaded.schedule().timezone();
    let local_now = now.with_timezone(&timezone);
    let anchor = loaded
        .state()
        .interval_anchor
        .map(|instant| instant.with_timezone(&timezone));
    let Some(fire) = loaded.schedule().next_fire(local_now, anchor) else {
        return Planned {
            delay: backoff(Duration::from_secs(60)),
            slot: None,
        };
    };
    let at = fire.at.with_timezone(&Utc);
    let delay = backoff(
        at.signed_duration_since(now)
            .to_std()
            .unwrap_or(Duration::ZERO),
    );
    Planned {
        delay,
        slot: Some(PlannedSlot {
            at,
            source: fire.source,
        }),
    }
}

struct CheckSignal {
    presentation: stardust_core::policy::Presentation,
    publish: bool,
    soften: bool,
    /// With routing on, the sink posts to make once the check is shown.
    posts: Option<PendingPosts>,
}

/// One check's posts, made after the bubble: the summary posts that still
/// apply with routing on, then one message per new alert.
struct PendingPosts {
    notes: Vec<DeliveryNote>,
    records: Vec<IntegrationRecord>,
    items: Vec<AlertItem>,
    server_base: String,
    quiet: bool,
    now: Instant,
    interval_minutes: u32,
    /// The integrations panel is on, so consent and the signed JSON webhook
    /// apply and results are kept.
    panel: bool,
}

impl PendingPosts {
    fn routing<'a>(&'a self, secrets: &'a dyn SecretStore) -> CheckRouting<'a> {
        CheckRouting {
            records: &self.records,
            secrets,
            items: &self.items,
            server_base: &self.server_base,
            quiet: self.quiet,
            now: self.now,
            interval_minutes: self.interval_minutes,
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_check(
    client: &Backend,
    config: &Mutex<Config>,
    path: &Path,
    tokens: &KeychainTokenStore,
    live: &LivePet,
    router: &mut Router,
    kind: &CheckKind,
    require_token: bool,
) -> CheckSignal {
    let now = Utc::now();
    let (schedule, server_base) = {
        let loaded = config.lock().unwrap_or_else(|err| err.into_inner());
        let server_base = server_base_of(&loaded)
            .unwrap_or_else(|_| stardust_core::api::DEFAULT_SERVER_BASE.to_string());
        (loaded.schedule().clone(), server_base)
    };
    let refresher = client.refresher();
    let token = if require_token {
        match current_access(tokens, refresher, now, false) {
            Access::Ready { token, unsaved, .. } => {
                note_unsaved(unsaved);
                Some(token)
            }
            Access::SignedOut => {
                return soft_signal(
                    config,
                    path,
                    live,
                    kind,
                    &server_base,
                    Notice::SignedOut,
                    false,
                );
            }
            Access::Unavailable(err) => {
                eprintln!("stardust: {err}");
                return soft_signal(
                    config,
                    path,
                    live,
                    kind,
                    &server_base,
                    Notice::Unavailable,
                    true,
                );
            }
        }
    } else {
        None
    };
    let access_token = token
        .as_ref()
        .map(|token| token.access_token())
        .unwrap_or("");
    let mut result = perform_check(client, access_token, now, &schedule, kind, &server_base).await;
    if require_token && matches!(result, Err(OrbitError::Unauthorized)) {
        if let Access::Ready { token, unsaved, .. } =
            current_access(tokens, refresher, Utc::now(), true)
        {
            note_unsaved(unsaved);
            result = perform_check(
                client,
                token.access_token(),
                now,
                &schedule,
                kind,
                &server_base,
            )
            .await;
        }
    }
    match result {
        Ok(outcome) => {
            let item_count = u32::try_from(outcome.items.len()).unwrap_or(u32::MAX);
            let high_severity = outcome.items.iter().any(AlertItem::shows_immediately);
            let (records, routing_on, panel_on) = {
                let mut loaded = config.lock().unwrap_or_else(|err| err.into_inner());
                loaded.adopt_saved_integrations(path);
                note_anchor(loaded.state_mut(), kind);
                note_success(loaded.state_mut(), now, None);
                if let Err(err) = save_config(path, &loaded) {
                    eprintln!("stardust: could not save config: {err}");
                }
                let features = loaded.features();
                (
                    loaded.integrations().to_vec(),
                    features.routing,
                    features.integrations_panel,
                )
            };
            live.set_observed(ObservedCheck::Succeeded {
                item_count,
                high_severity,
            });
            live.set_signed_in(true);
            let local = now.with_timezone(&schedule.timezone());
            let quiet = schedule.is_quiet_at(local);
            let manual = matches!(kind, CheckKind::Manual);
            let interval_minutes = schedule.interval_minutes().unwrap_or(1440);
            let summary_only: Vec<IntegrationRecord>;
            let summary_records: &[IntegrationRecord] = if panel_on {
                // What consent states holds only for the per-alert posts.
                &[]
            } else if routing_on {
                summary_only = routing::summary_records(&records, true);
                &summary_only
            } else {
                &records
            };
            let notes = prepare_posts(
                summary_records,
                tokens,
                &outcome.items,
                &DeliveryWindow {
                    quiet,
                    manual,
                    now,
                    interval_minutes,
                    server_base: &server_base,
                },
            );
            let mut presentation = outcome.presentation;
            let mut posts = None;
            if !routing_on && !panel_on {
                deliver::deliver(&notes);
            } else {
                let routed_at = Instant::now();
                if routing_on {
                    let reached = routing::route_local(
                        router,
                        &CheckRouting {
                            records: &records,
                            secrets: tokens,
                            items: &outcome.items,
                            server_base: &server_base,
                            quiet,
                            now: routed_at,
                            interval_minutes,
                        },
                        &crate::dnd::active,
                    );
                    if !manual {
                        // Only what this computer has not shown yet, outside
                        // quiet hours and do-not-disturb, and summed up again.
                        let fresh: Vec<AlertItem> = reached
                            .shown
                            .iter()
                            .map(|&at| outcome.items[at].clone())
                            .collect();
                        presentation = present(
                            &fresh,
                            quiet,
                            local,
                            false,
                            &server_base,
                            &outcome.workspace_path,
                        );
                        presentation.show_bubble &= !fresh.is_empty();
                    }
                    // A check the user asked for still answers in the bubble.
                    presentation.show_os_notification &= reached.notification;
                }
                posts = Some(PendingPosts {
                    notes,
                    records,
                    items: outcome.items,
                    server_base: server_base.clone(),
                    quiet,
                    now: routed_at,
                    interval_minutes,
                    panel: panel_on,
                });
            }
            if presentation.show_os_notification {
                notify::show(&presentation);
            }
            CheckSignal {
                presentation,
                publish: true,
                soften: false,
                posts,
            }
        }
        Err(err) => match err {
            OrbitError::Unauthorized | OrbitError::Expired => {
                let _ = stardust_core::token::TokenStore::clear(tokens);
                soft_signal(
                    config,
                    path,
                    live,
                    kind,
                    &server_base,
                    Notice::SignedOut,
                    false,
                )
            }
            OrbitError::FeatureOff
            | OrbitError::Unreachable
            | OrbitError::Response
            | OrbitError::Refused
            | OrbitError::Denied
            | OrbitError::ServerBase => {
                eprintln!("stardust: {err}");
                soft_signal(
                    config,
                    path,
                    live,
                    kind,
                    &server_base,
                    Notice::Unavailable,
                    true,
                )
            }
        },
    }
}

fn note_unsaved(unsaved: bool) {
    if unsaved {
        eprintln!(
            "stardust: the keychain did not take the renewed sign-in; the next renewal will ask to sign in again"
        );
    }
}

fn soft_signal(
    config: &Mutex<Config>,
    path: &Path,
    live: &LivePet,
    kind: &CheckKind,
    server_base: &str,
    notice: Notice,
    soften: bool,
) -> CheckSignal {
    let observed = match notice {
        Notice::Unavailable => ObservedCheck::Unavailable,
        Notice::SignedOut => ObservedCheck::SignedOut,
    };
    let previous = live.observed();
    let language = {
        let mut loaded = config.lock().unwrap_or_else(|err| err.into_inner());
        loaded.adopt_saved_integrations(path);
        note_anchor(loaded.state_mut(), kind);
        if let Err(err) = save_config(path, &loaded) {
            eprintln!("stardust: could not save config: {err}");
        }
        loaded.effective_language()
    };
    live.set_observed(observed);
    if notice == Notice::SignedOut {
        live.set_signed_in(false);
    }
    let manual = matches!(kind, CheckKind::Manual);
    CheckSignal {
        presentation: calm_notice_in(notice, server_base, language),
        publish: manual || previous != observed,
        soften,
        posts: None,
    }
}

fn server_base_of(config: &Config) -> Result<String, String> {
    resolve_server_base(
        config.server_base_url(),
        std::env::var(ENV_SERVER_URL).ok().as_deref(),
    )
    .map_err(|err| err.to_string())
}

fn open_backend(demo: bool, base: &str) -> Result<Backend, String> {
    if demo {
        Ok(Backend::Demo(MockOrbitClient::example()))
    } else {
        Ok(Backend::Live(
            HttpOrbitClient::new(base).map_err(|err| err.to_string())?,
        ))
    }
}

enum Backend {
    Demo(MockOrbitClient),
    Live(HttpOrbitClient),
}

impl Backend {
    fn refresher(&self) -> Option<&dyn Refresher> {
        match self {
            Self::Demo(_) => None,
            Self::Live(client) => Some(client),
        }
    }
}

impl DesktopApi for Backend {
    async fn desktop_status(&self, access_token: &str) -> Result<DesktopStatus, OrbitError> {
        match self {
            Self::Demo(client) => client.desktop_status(access_token).await,
            Self::Live(client) => client.desktop_status(access_token).await,
        }
    }

    async fn desktop_alerts(
        &self,
        access_token: &str,
    ) -> Result<stardust_core::api::AlertsPage, OrbitError> {
        match self {
            Self::Demo(client) => client.desktop_alerts(access_token).await,
            Self::Live(client) => client.desktop_alerts(access_token).await,
        }
    }
}

fn glyph_for(config: &Config, observed: ObservedCheck) -> stardust_core::pet::TrayGlyph {
    let local = Utc::now().with_timezone(&config.schedule().timezone());
    tray_glyph(pet_pose(observed, config.schedule().is_quiet_at(local)))
}

fn refresh_tray(config: &Mutex<Config>, live: &LivePet, tray: &RefCell<Option<Tray>>) {
    let observed = live.observed();
    let scale = live.scale_factor();
    let (text, glyph, language) = {
        let loaded = config.lock().unwrap_or_else(|err| err.into_inner());
        let language = loaded.effective_language();
        (
            tray_text(&loaded, live, language),
            glyph_for(&loaded, observed),
            language,
        )
    };
    if let Some(tray) = tray.borrow().as_ref() {
        if let Err(err) = tray.apply(&text, glyph, scale, language) {
            eprintln!("stardust: could not update the tray menu: {err}");
        }
    }
}

/// With no window, the user code goes to the terminal as well as the tray.
fn print_sign_in(update: &SignInUpdate) {
    match update {
        SignInUpdate::Code(user_code) => {
            println!("Enter this code on the ORBIT account page: {user_code}");
        }
        SignInUpdate::Done => println!("Signed in. The access token is in the system keychain."),
        SignInUpdate::Failed => println!("Sign-in did not finish. Choose Sign in to try again."),
    }
}

fn print_lines(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::{first_run_connect, plan_next, timer_request, TimerRequest};
    use crate::live::LivePet;
    use stardust_core::check::BACKOFF_CAP;
    use stardust_core::config::{Config, Feature};
    use std::sync::Mutex;
    use std::time::Duration;

    #[test]
    fn feature_output_adds_the_panel_row_only_to_the_list_while_it_is_off() {
        use super::feature_lines;
        use crate::cli::FeatureAction;
        use stardust_core::integrations::IntegrationKind;
        let mut config = Config::default_for("UTC").unwrap();
        config.set_feature(Feature::Routing, true);
        let set_routing = FeatureAction::Set {
            feature: Feature::Routing,
            on: true,
        };
        assert_eq!(
            feature_lines(&config, set_routing),
            ["auto-sign-in off", "jittered-backoff off", "routing on"],
            "as before the flag existed"
        );
        assert_eq!(
            feature_lines(&config, FeatureAction::List),
            [
                "auto-sign-in off",
                "jittered-backoff off",
                "routing on",
                "integrations-panel off"
            ]
        );
        assert_eq!(
            feature_lines(
                &config,
                FeatureAction::Set {
                    feature: Feature::IntegrationsPanel,
                    on: false
                }
            )
            .len(),
            4
        );

        config
            .add_integration(IntegrationKind::SlackIncoming, "ops")
            .unwrap();
        config
            .set_integration_host("ops", "hooks.slack.com")
            .unwrap();
        config.set_integration_enabled("ops", true).unwrap();
        config
            .add_integration(IntegrationKind::SlackIncoming, "bare")
            .unwrap();
        config.set_integration_enabled("bare", true).unwrap();
        config.set_feature(Feature::IntegrationsPanel, true);
        let lines = feature_lines(
            &config,
            FeatureAction::Set {
                feature: Feature::IntegrationsPanel,
                on: true,
            },
        );
        assert_eq!(lines[3], "integrations-panel on");
        assert_eq!(
            lines[4],
            "If Stardust is running, quit it and start it again to use this."
        );
        assert_eq!(
            lines[5],
            "ops sends nothing until accepted: stardust integration enable ops"
        );
        assert_eq!(
            lines.len(),
            6,
            "one with no destination sends nothing anyway"
        );
        assert_eq!(feature_lines(&config, FeatureAction::List).len(), 5);
    }

    #[test]
    fn tray_clicks_reach_the_check_timer_only_with_their_features_on() {
        let config = Mutex::new(Config::default_for("UTC").unwrap());
        let live = LivePet::new(1.0);
        assert!(timer_request(&config, &live, "test-send:ops").is_none());
        assert!(timer_request(&config, &live, "sign-in").is_none());
        config.lock().unwrap().set_feature(Feature::Routing, true);
        assert!(matches!(
            timer_request(&config, &live, "test-send:ops"),
            Some(TimerRequest::Test(id)) if id == "ops"
        ));
        assert!(timer_request(&config, &live, "test-send:../x").is_none());
        assert!(timer_request(&config, &live, "sign-in").is_none());
        config
            .lock()
            .unwrap()
            .set_feature(Feature::AutoSignIn, true);
        // Known, so the test never reads the real keychain.
        live.set_signed_in(false);
        assert!(matches!(
            timer_request(&config, &live, "sign-in"),
            Some(TimerRequest::Connect)
        ));
        live.set_signed_in(true);
        assert!(timer_request(&config, &live, "sign-in").is_none());
        assert!(timer_request(&config, &live, "check-now").is_none());
    }

    #[test]
    fn auto_sign_in_starts_by_itself_only_before_the_welcome_window_closes() {
        let config = Mutex::new(Config::default_for("UTC").unwrap());
        assert!(!first_run_connect(&config), "off by default");
        config
            .lock()
            .unwrap()
            .set_feature(Feature::AutoSignIn, true);
        assert!(first_run_connect(&config));
        config.lock().unwrap().set_onboarded(true);
        assert!(!first_run_connect(&config));
    }

    #[test]
    fn jitter_changes_only_the_wait_after_failures_and_stays_capped() {
        let config = Mutex::new(Config::default_for("UTC").unwrap());
        let calm = plan_next(&config, 0);
        assert!(calm.delay <= Duration::from_secs(3_600));
        let fixed = plan_next(&config, 4);
        assert!(
            fixed.delay >= Duration::from_secs(240),
            "the old doubling waits 30 s * 2^3 at least"
        );
        config
            .lock()
            .unwrap()
            .set_feature(Feature::JitteredBackoff, true);
        for failures in [1, 4, 12] {
            let jittered = plan_next(&config, failures);
            assert!(jittered.delay >= Duration::from_secs(1));
            assert!(
                jittered.delay <= BACKOFF_CAP.max(calm.delay + Duration::from_secs(1)),
                "{failures}: {:?}",
                jittered.delay
            );
        }
    }
}
