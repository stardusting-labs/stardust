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
use stardust_core::http::{HttpOrbitClient, Revoke};
use stardust_core::i18n::Language;
use stardust_core::integrations::{IntegrationRecord, SecretStore};
use stardust_core::outbound::{prepare_posts, DeliveryNote, DeliveryWindow};
use stardust_core::pet::{pet_pose, tray_glyph, ObservedCheck};
use stardust_core::policy::{calm_notice_in, present, Notice};
use stardust_core::position::ScreenPoint;
use stardust_core::route::Router;
use stardust_core::schedule::{parse_timezone, FireSource};
use stardust_core::session::{current_access, stored_from_response, Access, Refresher};
use stardust_core::signin_window::{SessionRead, SignInEnd};
use stardust_core::text::{menu_text, test_send_target};
use stardust_core::token::{StoredToken, TokenStore};
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
        Command::SignOut { config, demo } => {
            // An unreadable or missing config file signs out as before.
            let loaded = load_config(&config_path(config)).ok().flatten();
            if demo_sign_out_refused(demo, loaded.as_ref()) {
                return Err("demo mode does not sign out".to_string());
            }
            crate::signin::sign_out(loaded.as_ref())
        }
        Command::Feature { config, action } => {
            let path = config_path(config);
            let mut loaded = load_or_create(&path)?;
            if let FeatureAction::Set { feature, on } = action {
                loaded.set_feature(feature, on);
                save_config(&path, &loaded).map_err(|err| err.to_string())?;
            }
            for feature in Feature::ALL {
                let state = if loaded.feature(feature) { "on" } else { "off" };
                println!("{} {state}", feature.name());
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

/// `--demo` makes no use of a sign-in, so under the launch sign-in window flag
/// it must not delete a real one or ask ORBIT to revoke it. With the flag off
/// the command stays as it was.
fn demo_sign_out_refused(demo: bool, config: Option<&Config>) -> bool {
    demo && config.is_some_and(|config| config.features().launch_sign_in_window)
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
    live.set_demo(demo);
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
                let sign_in_on_timer = config
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .features()
                    .sign_in_on_timer();
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
                    if sign_in_on_timer && demo {
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
                        UiMsg::SignIn(update) if sign_in_on_timer => print_sign_in(&update),
                        UiMsg::SignIn(_) | UiMsg::Menu(_) | UiMsg::Pet => {}
                    }
                }
            }
        }
    }
    while windows_unavailable && !quit.load(Ordering::SeqCst) {
        match inbox.wait_timeout(Duration::from_secs(45)) {
            Some(UiMsg::Menu(id)) => {
                if let Some(request) = timer_request(&config, &live, &id) {
                    request.queue(&live);
                    wake.notify_one();
                    refresh_tray(&config, &live, &tray);
                    continue;
                }
                let effect = {
                    let mut loaded = config.lock().unwrap_or_else(|err| err.into_inner());
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
                let on_timer = config
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .features()
                    .sign_in_on_timer();
                if on_timer {
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
                finish_sign_out_on_quit(&backend, &config, &tokens, &live);
                break;
            }
            run_requests(&backend, &config, &tokens, &live, &inbox, &mut flow);
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
                publish(&inbox, &tokens, &mut router, signal);
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
                step_flow(&backend, &tokens, &live, &inbox, &mut flow, Instant::now());
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
            publish(&inbox, &tokens, &mut router, signal);
        }
    });
}

/// Hand a check to the tray and the windows, then make its sink posts, so
/// the bubble does not wait for the network.
fn publish(inbox: &Inbox, tokens: &KeychainTokenStore, router: &mut Router, signal: CheckSignal) {
    inbox.push(UiMsg::Pet);
    if signal.publish {
        inbox.push(UiMsg::Checked(signal.presentation));
    }
    if let Some(posts) = signal.posts {
        deliver::deliver(&posts.notes);
        routing::route_remote(router, &posts.routing(tokens), &HttpsTransport::new());
    }
}

fn next_failures(current: u32, soften: bool) -> u32 {
    if soften {
        current.saturating_add(1)
    } else {
        0
    }
}

/// Auto sign-in starts by itself only on a first run: the welcome window
/// was never closed. After that it starts from a click on "Sign in". With the
/// launch sign-in window on, that window is the only thing that starts one,
/// because the window ignores a flow it did not number.
fn first_run_connect(config: &Mutex<Config>) -> bool {
    let loaded = config.lock().unwrap_or_else(|err| err.into_inner());
    let features = loaded.features();
    features.auto_sign_in && !features.launch_sign_in_window && !loaded.onboarded()
}

/// A tray click the check timer carries out with the newer features on.
enum TimerRequest {
    Connect,
    SignOut,
    Test(String),
}

impl TimerRequest {
    fn queue(self, live: &LivePet) {
        match self {
            Self::Connect => live.request_connect(),
            Self::SignOut => {
                if let Err(err) = crate::signin::begin_sign_out(&KeychainTokenStore::new(), live) {
                    eprintln!("stardust: could not sign out: {err}");
                }
            }
            Self::Test(id) => live.request_test(&id),
        }
    }
}

fn timer_request(config: &Mutex<Config>, live: &LivePet, id: &str) -> Option<TimerRequest> {
    timer_request_with(config, live, id, &crate::signin::session)
}

/// `read_session` looks in the keychain. The launch sign-in window flag always
/// looks on a click, so a sign-in made in a terminal is noticed rather than
/// started again. Tests pass their own.
fn timer_request_with(
    config: &Mutex<Config>,
    live: &LivePet,
    id: &str,
    read_session: &dyn Fn() -> SessionRead,
) -> Option<TimerRequest> {
    let features = config
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .features();
    if features.routing {
        if let Some(target) = test_send_target(id) {
            return Some(TimerRequest::Test(target.to_string()));
        }
    }
    if features.launch_sign_in_window {
        if live.demo() {
            return None;
        }
        return match id {
            "sign-in" => match read_session() {
                SessionRead::Usable { until } => {
                    live.note_session(until);
                    live.set_signed_in(true);
                    if live.observed() == ObservedCheck::SignedOut {
                        live.set_observed(ObservedCheck::Waiting);
                    }
                    None
                }
                SessionRead::Missing => {
                    live.set_signed_in(false);
                    Some(TimerRequest::Connect)
                }
                SessionRead::Unreadable => {
                    eprintln!(
                        "stardust: could not read the system keychain; unlock it and try again"
                    );
                    None
                }
            },
            "sign-out" => Some(TimerRequest::SignOut),
            _ => None,
        };
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
fn run_requests<S: TokenStore + SecretStore>(
    backend: &Backend,
    config: &Mutex<Config>,
    tokens: &S,
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
    let requests = live.take_flow_requests();
    // Only the launch sign-in window asks for this. The code stays valid on
    // ORBIT until it expires, because no route cancels it; this computer just
    // stops polling.
    if requests.cancel {
        let dropped = flow.take().is_some();
        live.set_phase(FlowPhase::Idle);
        if dropped {
            inbox.push(UiMsg::Pet);
        }
    }
    if let Some(attempt) = requests.connect {
        if flow.is_none() {
            if let Backend::Live(client) = backend {
                start_flow(client, config, live, inbox, flow, attempt);
            }
        }
    }
    let tests = live.take_tests();
    if !tests.is_empty() {
        send_tests(config, tokens, inbox, &tests);
    }
}

/// Quit can land while a long check holds up the timer, right after Sign out.
/// The tray already said signed out, so finish it: otherwise the refresh token
/// is never revoked, and a record that check saved would sign the next launch
/// back in. Only under the launch sign-in window flag; the other flags keep
/// what they do today.
fn finish_sign_out_on_quit(
    backend: &Backend,
    config: &Mutex<Config>,
    tokens: &dyn TokenStore,
    live: &LivePet,
) {
    let window_flag = config
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .features()
        .launch_sign_in_window;
    if !window_flag {
        return;
    }
    if let Backend::Live(client) = backend {
        if let Some(ticket) = live.take_sign_out() {
            finish_sign_out(client, tokens, ticket.refresh_token());
        }
    }
}

/// The settings window already deleted the keychain record. A check that was
/// running then may have saved a rotated one, so delete again and revoke
/// every refresh token seen. A server without the revoke route answers 404,
/// which is fine.
fn finish_sign_out(client: &HttpOrbitClient, tokens: &dyn TokenStore, refresh: Option<&str>) {
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
    attempt: u64,
) {
    let (server_base, for_window) = {
        let loaded = config.lock().unwrap_or_else(|err| err.into_inner());
        (
            server_base_of(&loaded).unwrap_or_else(|_| DEFAULT_SERVER_BASE.to_string()),
            loaded.features().launch_sign_in_window,
        )
    };
    match DeviceFlow::start(client, &server_base, Instant::now) {
        // No browser opens by itself: a window appearing at login should not
        // also launch one. The Open row is a click away.
        Ok(started) if for_window => {
            let started = started.for_window(attempt);
            live.set_phase(FlowPhase::Waiting {
                user_code: started.user_code().to_string(),
            });
            inbox.push(UiMsg::Pet);
            inbox.push(UiMsg::SignIn(SignInUpdate::Issued {
                attempt,
                code: started.issued(),
                lifetime: started.remaining(Instant::now()),
            }));
            *flow = Some(started);
        }
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
            inbox.push(UiMsg::SignIn(if for_window {
                SignInUpdate::Ended {
                    attempt,
                    end: SignInEnd::from_error(&err),
                }
            } else {
                SignInUpdate::Failed
            }));
        }
    }
}

/// One device-token poll, when it is due.
fn step_flow(
    backend: &Backend,
    tokens: &dyn TokenStore,
    live: &LivePet,
    inbox: &Inbox,
    flow: &mut Option<DeviceFlow>,
    now: Instant,
) {
    let Backend::Live(client) = backend else {
        *flow = None;
        return;
    };
    let Some(current) = flow.as_mut() else {
        return;
    };
    let user_code = current.user_code().to_string();
    let attempt = current.window_attempt();
    let step = current.step(client, now, entropy());
    let phase = match step {
        FlowStep::Wait(_) => return,
        FlowStep::Pending(_) => FlowPhase::Waiting { user_code },
        FlowStep::Offline(_) => FlowPhase::Offline { user_code },
        FlowStep::Approved(token) => {
            *flow = None;
            live.set_phase(FlowPhase::Idle);
            let refresh = token.refresh_token.clone();
            let stored = stored_from_response(token, Utc::now());
            let until = stored.as_ref().ok().map(StoredToken::usable_until);
            let (saved, end) = match stored {
                Ok(stored) => (
                    tokens.store(&stored).map_err(|err| err.to_string()),
                    SignInEnd::KeychainFailed,
                ),
                Err(err) => (Err(err.to_string()), SignInEnd::from_error(&err)),
            };
            match saved {
                Ok(()) => {
                    if let Some(until) = until {
                        live.note_session(until);
                    }
                    if attempt.is_some() {
                        // A new attempt asked for while this poll was out has
                        // no purpose now. This thread is also the one that
                        // takes requests, so it cannot race a flow starting.
                        live.request_cancel();
                    }
                    live.set_signed_in(true);
                    live.set_observed(ObservedCheck::Waiting);
                    inbox.push(UiMsg::Pet);
                    inbox.push(UiMsg::SignIn(SignInUpdate::Done));
                }
                Err(err) => {
                    eprintln!("stardust: sign-in: {err}");
                    if let (Some(_), Some(refresh)) = (attempt, refresh.as_deref()) {
                        // This token is about to be dropped, so nothing could
                        // revoke it later.
                        let _ = client.revoke_refresh_token(refresh);
                    }
                    inbox.push(UiMsg::Pet);
                    inbox.push(UiMsg::SignIn(ended_update(attempt, end)));
                }
            }
            return;
        }
        FlowStep::Ended(err) => {
            *flow = None;
            eprintln!("stardust: sign-in: {err}");
            live.set_phase(FlowPhase::Idle);
            inbox.push(UiMsg::Pet);
            inbox.push(UiMsg::SignIn(ended_update(
                attempt,
                SignInEnd::from_error(&err),
            )));
            return;
        }
    };
    if live.phase() != phase {
        live.set_phase(phase);
        inbox.push(UiMsg::Pet);
    }
}

/// How a sign-in that stopped is reported: with its reason to the window that
/// numbered it, and as before to anything else.
fn ended_update(attempt: Option<u64>, end: SignInEnd) -> SignInUpdate {
    match attempt {
        Some(attempt) => SignInUpdate::Ended { attempt, end },
        None => SignInUpdate::Failed,
    }
}

fn send_tests(config: &Mutex<Config>, tokens: &dyn SecretStore, inbox: &Inbox, ids: &[String]) {
    let (records, server_base, language, routing_on) = {
        let loaded = config.lock().unwrap_or_else(|err| err.into_inner());
        (
            loaded.integrations().to_vec(),
            server_base_of(&loaded).unwrap_or_else(|_| DEFAULT_SERVER_BASE.to_string()),
            loaded.effective_language(),
            loaded.features().routing,
        )
    };
    if !routing_on {
        return;
    }
    for id in ids {
        let result = routing::send_test(
            &records,
            tokens,
            id,
            &server_base,
            Utc::now(),
            &HttpsTransport::new(),
        );
        match &result {
            Ok(host) => eprintln!("stardust: test sent to {id} ({host})"),
            Err(err) => eprintln!("stardust: test to {id} did not go through: {err}"),
        }
        inbox.push(UiMsg::Checked(routing::test_presentation(
            id, &result, language,
        )));
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
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_check<S: TokenStore + SecretStore>(
    client: &Backend,
    config: &Mutex<Config>,
    path: &Path,
    tokens: &S,
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
                live.note_session(token.usable_until());
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
            live.note_session(token.usable_until());
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
            let (records, routing_on) = {
                let mut loaded = config.lock().unwrap_or_else(|err| err.into_inner());
                note_anchor(loaded.state_mut(), kind);
                note_success(loaded.state_mut(), now, None);
                if let Err(err) = save_config(path, &loaded) {
                    eprintln!("stardust: could not save config: {err}");
                }
                (loaded.integrations().to_vec(), loaded.features().routing)
            };
            live.set_observed(ObservedCheck::Succeeded {
                item_count,
                high_severity,
            });
            live.set_signed_in(true);
            let local = now.with_timezone(&schedule.timezone());
            let quiet = schedule.is_quiet_at(local);
            let manual = matches!(kind, CheckKind::Manual);
            let summary_only: Vec<IntegrationRecord>;
            let summary_records: &[IntegrationRecord] = if routing_on {
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
                    interval_minutes: schedule.interval_minutes().unwrap_or(1440),
                    server_base: &server_base,
                },
            );
            let mut presentation = outcome.presentation;
            let mut posts = None;
            if !routing_on {
                deliver::deliver(&notes);
            } else {
                let routed_at = Instant::now();
                let reached = routing::route_local(
                    router,
                    &CheckRouting {
                        records: &records,
                        secrets: tokens,
                        items: &outcome.items,
                        server_base: &server_base,
                        quiet,
                        now: routed_at,
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
                posts = Some(PendingPosts {
                    notes,
                    records,
                    items: outcome.items,
                    server_base: server_base.clone(),
                    quiet,
                    now: routed_at,
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
    let (language, window_flag) = {
        let mut loaded = config.lock().unwrap_or_else(|err| err.into_inner());
        note_anchor(loaded.state_mut(), kind);
        if let Err(err) = save_config(path, &loaded) {
            eprintln!("stardust: could not save config: {err}");
        }
        (
            loaded.effective_language(),
            loaded.features().launch_sign_in_window,
        )
    };
    // The bubble that says to sign in stays away from a sign-in in progress.
    let already_signing_in = window_flag && notice == Notice::SignedOut && live.sign_in_active();
    live.set_observed(observed);
    if notice == Notice::SignedOut {
        live.set_signed_in(false);
    }
    let manual = matches!(kind, CheckKind::Manual);
    CheckSignal {
        presentation: calm_notice_in(notice, server_base, language),
        publish: (manual || previous != observed) && !already_signing_in,
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
        SignInUpdate::Issued { code, .. } => {
            // No browser opens by itself with this flow, so the address is
            // the way there.
            println!(
                "Enter this code on the ORBIT account page: {}",
                code.user_code
            );
            match &code.address {
                Some(address) => println!("https://{address}"),
                None => println!(
                    "Open the ORBIT account page in your browser and enter the code there."
                ),
            }
        }
        SignInUpdate::Ended { end, .. } => {
            println!(
                "{}. Choose Sign in to try again.",
                end.message(Language::default())
            );
        }
    }
}

fn print_lines(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        demo_sign_out_refused, finish_sign_out_on_quit, first_run_connect, plan_next, run_check,
        run_requests, soft_signal, start_flow, step_flow, timer_request, timer_request_with,
        Backend, TimerRequest,
    };
    use crate::inbox::{Inbox, SignInUpdate, UiMsg};
    use crate::live::LivePet;
    use crate::mock_orbit::{serve_forever, Script};
    use crate::testmock::MockOrbit;
    use chrono::{DateTime, Utc};
    use stardust_core::api::{PATH_DEVICE_CODE, PATH_DEVICE_REVOKE, PATH_DEVICE_TOKEN};
    use stardust_core::check::{CheckKind, BACKOFF_CAP};
    use stardust_core::config::{parse_config, Config, Feature};
    use stardust_core::connect::{DeviceFlow, FlowPhase};
    use stardust_core::http::HttpOrbitClient;
    use stardust_core::integrations::{SecretError, SecretStore};
    use stardust_core::pet::ObservedCheck;
    use stardust_core::policy::Notice;
    use stardust_core::signin_window::{SessionRead, SignInEnd};
    use stardust_core::token::{MemoryTokenStore, StoredToken, TokenError, TokenStore};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const DEVICE_CODE: &str = "abababababababababababababababababababababababababababababababab";
    const PENDING: &str = r#"{"error":"authorization_pending"}"#;

    /// Stands in for the keychain, so no test here touches the real one.
    struct TestStore {
        tokens: MemoryTokenStore,
        refuse_store: bool,
    }

    impl TestStore {
        fn new() -> Self {
            Self {
                tokens: MemoryTokenStore::new(),
                refuse_store: false,
            }
        }
    }

    impl TokenStore for TestStore {
        fn store(&self, token: &StoredToken) -> Result<(), TokenError> {
            if self.refuse_store {
                return Err(TokenError::Keychain);
            }
            self.tokens.store(token)
        }
        fn load(&self, now: DateTime<Utc>) -> Result<Option<StoredToken>, TokenError> {
            self.tokens.load(now)
        }
        fn clear(&self) -> Result<(), TokenError> {
            self.tokens.clear()
        }
    }

    impl SecretStore for TestStore {
        fn put(&self, _: &str, _: &str) -> Result<(), SecretError> {
            Ok(())
        }
        fn get(&self, _: &str) -> Result<Option<String>, SecretError> {
            Ok(None)
        }
        fn delete(&self, _: &str) -> Result<(), SecretError> {
            Ok(())
        }
    }

    fn code_body() -> String {
        format!(
            r#"{{"device_code":"{DEVICE_CODE}","user_code":"ABCD-EFGH","verification_uri":"https://helloorbit.app/rust-preview/pqc/account","verification_uri_complete":"https://helloorbit.app/rust-preview/pqc/account?user_code=ABCD-EFGH","expires_in":600,"interval":1}}"#
        )
    }

    fn token_body(refresh: Option<&str>) -> String {
        let stamp = Utc::now().timestamp_millis() + 300_000;
        let refresh = refresh.map_or(String::new(), |token| {
            format!(r#","refresh_token":"{token}""#)
        });
        format!(
            r#"{{"token":"access-1","access_token":"access-1","token_type":"Bearer","expires_in":300,"expires_at_epoch_ms":{stamp}{refresh}}}"#
        )
    }

    /// How a server answers a device-token poll: the status and the body.
    type PollReply = fn() -> (u16, String);

    /// A server that issues a code and answers each poll with `poll`.
    fn server(poll: PollReply) -> MockOrbit {
        MockOrbit::start(move |path, _| match path {
            PATH_DEVICE_CODE => (200, code_body()),
            PATH_DEVICE_TOKEN => poll(),
            PATH_DEVICE_REVOKE => (200, "{}".to_string()),
            _ => (404, String::new()),
        })
    }

    /// A config on the loopback mock with these features. A loopback base
    /// allows no browser page, so nothing here can open one.
    fn config_on(base: &str, features: &[Feature]) -> Mutex<Config> {
        let mut config = parse_config(&format!(
            r#"{{"character_visible":false,"character_position":{{"x":1,"y":2}},"schedule":{{"interval_minutes":60,"clock_times":[],"weekdays":["mon"],"timezone":"UTC"}},"server_base_url":"{base}"}}"#
        ))
        .unwrap();
        for feature in features {
            config.set_feature(*feature, true);
        }
        Mutex::new(config)
    }

    fn backend(base: &str) -> Backend {
        Backend::Live(HttpOrbitClient::with_timeout(base, Duration::from_secs(2)).unwrap())
    }

    fn client_of(backend: &Backend) -> &HttpOrbitClient {
        match backend {
            Backend::Live(client) => client,
            Backend::Demo(_) => panic!("a live backend"),
        }
    }

    /// Everything waiting in the inbox, sign-in updates only.
    fn updates(inbox: &Inbox) -> Vec<SignInUpdate> {
        std::iter::from_fn(|| inbox.try_pop())
            .filter_map(|message| match message {
                UiMsg::SignIn(update) => Some(update),
                UiMsg::Menu(_) | UiMsg::Checked(_) | UiMsg::Pet => None,
            })
            .collect()
    }

    fn later() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    #[test]
    fn with_the_flag_a_new_sign_in_is_numbered_and_carries_only_what_the_window_may_show() {
        let orbit = server(|| (400, PENDING.to_string()));
        let config = config_on(orbit.base(), &[Feature::LaunchSignInWindow]);
        let backend = backend(orbit.base());
        let (live, inbox, store) = (LivePet::new(1.0), Inbox::new(), TestStore::new());
        let mut flow = None;
        let attempt = live.request_new_sign_in();
        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        assert_eq!(orbit.count(PATH_DEVICE_CODE), 1);
        assert_eq!(flow.as_ref().unwrap().window_attempt(), Some(attempt));
        assert_eq!(
            live.phase(),
            FlowPhase::Waiting {
                user_code: "ABCD-EFGH".to_string()
            }
        );
        let updates = updates(&inbox);
        let [SignInUpdate::Issued {
            attempt: stamped,
            code,
            lifetime,
        }] = updates.as_slice()
        else {
            panic!("expected one issued update");
        };
        assert_eq!(*stamped, attempt);
        assert_eq!(code.user_code, "ABCD-EFGH");
        assert_eq!(code.open_url, None, "no page on a plain-http server");
        assert!(*lifetime > Duration::from_secs(590));
        assert!(!format!("{code:?}").contains(DEVICE_CODE));
    }

    #[test]
    fn with_only_auto_sign_in_the_old_updates_are_sent() {
        let orbit = server(|| (400, r#"{"error":"access_denied"}"#.to_string()));
        let config = config_on(orbit.base(), &[Feature::AutoSignIn]);
        let backend = backend(orbit.base());
        let (live, inbox, store) = (LivePet::new(1.0), Inbox::new(), TestStore::new());
        let mut flow = None;
        live.request_connect();
        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        assert_eq!(flow.as_ref().unwrap().window_attempt(), None);
        assert!(matches!(
            updates(&inbox).as_slice(),
            [SignInUpdate::Code(code)] if code == "ABCD-EFGH"
        ));
        step_flow(&backend, &store, &live, &inbox, &mut flow, later());
        assert!(matches!(updates(&inbox).as_slice(), [SignInUpdate::Failed]));
    }

    #[test]
    fn approval_stores_the_token_and_reports_done() {
        let orbit = server(|| (200, token_body(Some("refresh-1"))));
        let config = config_on(orbit.base(), &[Feature::LaunchSignInWindow]);
        let backend = backend(orbit.base());
        let (live, inbox, store) = (LivePet::new(1.0), Inbox::new(), TestStore::new());
        let mut flow = None;
        live.request_new_sign_in();
        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        updates(&inbox);
        step_flow(&backend, &store, &live, &inbox, &mut flow, later());
        assert!(flow.is_none());
        assert_eq!(live.phase(), FlowPhase::Idle);
        assert_eq!(live.signed_in(), Some(true));
        assert_eq!(live.observed(), ObservedCheck::Waiting);
        let saved = store.load(Utc::now()).unwrap().unwrap();
        assert_eq!(saved.access_token(), "access-1");
        assert_eq!(saved.refresh_token(), Some("refresh-1"));
        assert!(matches!(updates(&inbox).as_slice(), [SignInUpdate::Done]));
        assert_eq!(orbit.count(PATH_DEVICE_REVOKE), 0);
    }

    #[test]
    fn a_keychain_that_will_not_save_reports_it_and_revokes_the_dropped_token() {
        let orbit = server(|| (200, token_body(Some("refresh-1"))));
        let config = config_on(orbit.base(), &[Feature::LaunchSignInWindow]);
        let backend = backend(orbit.base());
        let (live, inbox) = (LivePet::new(1.0), Inbox::new());
        let store = TestStore {
            tokens: MemoryTokenStore::new(),
            refuse_store: true,
        };
        let mut flow = None;
        let attempt = live.request_new_sign_in();
        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        updates(&inbox);
        step_flow(&backend, &store, &live, &inbox, &mut flow, later());
        assert!(flow.is_none());
        assert_eq!(live.phase(), FlowPhase::Idle);
        assert_ne!(live.signed_in(), Some(true));
        assert!(matches!(
            updates(&inbox).as_slice(),
            [SignInUpdate::Ended { attempt: stamped, end: SignInEnd::KeychainFailed }]
                if *stamped == attempt
        ));
        assert_eq!(orbit.count(PATH_DEVICE_REVOKE), 1);
        assert!(orbit.bodies(PATH_DEVICE_REVOKE)[0].contains("refresh-1"));
    }

    #[test]
    fn each_way_a_sign_in_can_end_has_its_own_reason() {
        let cases: [(PollReply, SignInEnd); 3] = [
            (
                || (400, r#"{"error":"access_denied"}"#.to_string()),
                SignInEnd::Denied,
            ),
            (
                || (400, r#"{"error":"expired_token"}"#.to_string()),
                SignInEnd::Expired,
            ),
            (|| (404, String::new()), SignInEnd::Unavailable),
        ];
        for (poll, expected) in cases {
            let orbit = server(poll);
            let config = config_on(orbit.base(), &[Feature::LaunchSignInWindow]);
            let backend = backend(orbit.base());
            let (live, inbox, store) = (LivePet::new(1.0), Inbox::new(), TestStore::new());
            let mut flow = None;
            live.request_new_sign_in();
            run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
            updates(&inbox);
            step_flow(&backend, &store, &live, &inbox, &mut flow, later());
            assert!(flow.is_none(), "{expected:?}");
            assert_eq!(live.phase(), FlowPhase::Idle);
            assert!(
                matches!(
                    updates(&inbox).as_slice(),
                    [SignInUpdate::Ended { end, .. }] if *end == expected
                ),
                "{expected:?}"
            );
        }
    }

    #[test]
    fn a_failed_request_for_a_code_says_why() {
        let off = MockOrbit::start(|_, _| (404, String::new()));
        let config = config_on(off.base(), &[Feature::LaunchSignInWindow]);
        let backend_off = backend(off.base());
        let (live, inbox, store) = (LivePet::new(1.0), Inbox::new(), TestStore::new());
        let mut flow = None;
        live.request_new_sign_in();
        run_requests(&backend_off, &config, &store, &live, &inbox, &mut flow);
        assert!(flow.is_none());
        assert_eq!(live.phase(), FlowPhase::Idle);
        assert!(matches!(
            updates(&inbox).as_slice(),
            [SignInUpdate::Ended {
                end: SignInEnd::Unavailable,
                ..
            }]
        ));

        let closed = backend("http://127.0.0.1:1");
        let config = config_on("http://127.0.0.1:1", &[Feature::LaunchSignInWindow]);
        live.request_new_sign_in();
        run_requests(&closed, &config, &store, &live, &inbox, &mut flow);
        assert!(matches!(
            updates(&inbox).as_slice(),
            [SignInUpdate::Ended {
                end: SignInEnd::Offline,
                ..
            }]
        ));
    }

    #[test]
    fn going_offline_keeps_the_flow_and_recovers() {
        let orbit = server(|| (400, PENDING.to_string()));
        let config = config_on(orbit.base(), &[Feature::LaunchSignInWindow]);
        let live_backend = backend(orbit.base());
        let (live, inbox, store) = (LivePet::new(1.0), Inbox::new(), TestStore::new());
        let mut flow = None;
        live.request_new_sign_in();
        run_requests(&live_backend, &config, &store, &live, &inbox, &mut flow);
        updates(&inbox);
        let closed = backend("http://127.0.0.1:1");
        step_flow(&closed, &store, &live, &inbox, &mut flow, later());
        assert!(flow.is_some(), "still trying");
        let code = "ABCD-EFGH".to_string();
        assert_eq!(
            live.phase(),
            FlowPhase::Offline {
                user_code: code.clone()
            }
        );
        assert!(updates(&inbox).is_empty(), "a phase change, not an ending");
        let long_after = Instant::now() + Duration::from_secs(120);
        step_flow(&live_backend, &store, &live, &inbox, &mut flow, long_after);
        assert_eq!(live.phase(), FlowPhase::Waiting { user_code: code });
    }

    #[test]
    fn cancel_stops_the_polling_and_resets_the_phase() {
        let orbit = server(|| (400, PENDING.to_string()));
        let config = config_on(orbit.base(), &[Feature::LaunchSignInWindow]);
        let backend = backend(orbit.base());
        let (live, inbox, store) = (LivePet::new(1.0), Inbox::new(), TestStore::new());
        let mut flow = None;
        live.request_new_sign_in();
        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        step_flow(&backend, &store, &live, &inbox, &mut flow, later());
        assert_eq!(orbit.count(PATH_DEVICE_TOKEN), 1);
        while inbox.try_pop().is_some() {}

        live.request_cancel();
        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        assert!(flow.is_none());
        assert_eq!(live.phase(), FlowPhase::Idle);
        assert!(
            matches!(inbox.try_pop(), Some(UiMsg::Pet)),
            "so the tray stops showing the old code"
        );
        step_flow(&backend, &store, &live, &inbox, &mut flow, later());
        assert_eq!(orbit.count(PATH_DEVICE_TOKEN), 1, "no further polls");
        assert_eq!(orbit.count(PATH_DEVICE_CODE), 1);
    }

    #[test]
    fn open_then_cancel_asks_for_nothing_and_cancel_then_open_asks_once() {
        let orbit = server(|| (400, PENDING.to_string()));
        let config = config_on(orbit.base(), &[Feature::LaunchSignInWindow]);
        let backend = backend(orbit.base());
        let (live, inbox, store) = (LivePet::new(1.0), Inbox::new(), TestStore::new());
        let mut flow = None;

        live.request_new_sign_in();
        live.request_cancel();
        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        assert_eq!(orbit.count(PATH_DEVICE_CODE), 0);
        assert!(flow.is_none());

        live.request_new_sign_in();
        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        assert_eq!(orbit.count(PATH_DEVICE_CODE), 1);
        let first = flow.as_ref().unwrap().window_attempt().unwrap();

        live.request_cancel();
        let second = live.request_new_sign_in();
        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        assert_eq!(orbit.count(PATH_DEVICE_CODE), 2, "the old code is replaced");
        assert_eq!(flow.as_ref().unwrap().window_attempt(), Some(second));
        assert!(second > first);
    }

    #[test]
    fn try_again_stamps_the_new_attempt_so_the_window_can_ignore_the_old_one() {
        let orbit = server(|| (400, PENDING.to_string()));
        let config = config_on(orbit.base(), &[Feature::LaunchSignInWindow]);
        let backend = backend(orbit.base());
        let (live, inbox, store) = (LivePet::new(1.0), Inbox::new(), TestStore::new());
        let mut flow = None;
        let old = live.request_new_sign_in();
        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        let new = live.request_new_sign_in();
        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        let stamps: Vec<u64> = updates(&inbox)
            .iter()
            .filter_map(|update| match update {
                SignInUpdate::Issued { attempt, .. } => Some(*attempt),
                _ => None,
            })
            .collect();
        assert_eq!(stamps, [old, new]);
    }

    #[test]
    fn a_slow_answer_keeps_the_number_it_was_asked_under() {
        // The request for a code blocks the timer. A newer attempt made during
        // it must not relabel this answer, or the window would show a code
        // that is about to be dropped.
        let orbit = server(|| (400, PENDING.to_string()));
        let config = config_on(orbit.base(), &[Feature::LaunchSignInWindow]);
        let live_backend = backend(orbit.base());
        let (live, inbox) = (LivePet::new(1.0), Inbox::new());
        let mut flow: Option<DeviceFlow> = None;
        let asked = live.request_new_sign_in();
        let requests = live.take_flow_requests();
        live.request_new_sign_in();
        start_flow(
            client_of(&live_backend),
            &config,
            &live,
            &inbox,
            &mut flow,
            requests.connect.unwrap(),
        );
        assert!(matches!(
            updates(&inbox).as_slice(),
            [SignInUpdate::Issued { attempt, .. }] if *attempt == asked
        ));
    }

    #[test]
    fn sign_out_during_a_sign_in_drops_the_flow_and_revokes_the_saved_token() {
        let orbit = server(|| (200, token_body(None)));
        let config = config_on(orbit.base(), &[Feature::LaunchSignInWindow]);
        let backend = backend(orbit.base());
        let (live, inbox, store) = (LivePet::new(1.0), Inbox::new(), TestStore::new());
        let now = Utc::now();
        store
            .store(
                &StoredToken::issue(
                    "access-0",
                    Some("refresh-0".to_string()),
                    now + chrono::TimeDelta::seconds(100),
                    now,
                )
                .unwrap(),
            )
            .unwrap();
        let mut flow = None;
        live.request_new_sign_in();
        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        assert!(flow.is_some());

        crate::signin::begin_sign_out(&store, &live).unwrap();
        assert!(store.load(Utc::now()).unwrap().is_none());
        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        assert!(flow.is_none(), "it could otherwise save a token afterwards");
        assert_eq!(live.phase(), FlowPhase::Idle);
        assert_eq!(live.signed_in(), Some(false));
        assert_eq!(live.observed(), ObservedCheck::SignedOut);
        assert_eq!(orbit.count(PATH_DEVICE_REVOKE), 1);
        assert!(orbit.bodies(PATH_DEVICE_REVOKE)[0].contains("refresh-0"));
        assert_eq!(orbit.count(PATH_DEVICE_TOKEN), 0);
    }

    #[test]
    fn the_sign_in_bubble_stays_away_while_signing_in_and_only_under_the_flag() {
        let path = std::env::temp_dir()
            .join(format!("stardust-test-{}", uuid::Uuid::new_v4()))
            .join("config.json");
        let shown = |features: &[Feature], signing_in: bool, notice: Notice| {
            let config = config_on("http://127.0.0.1:1", features);
            let live = LivePet::new(1.0);
            live.set_sign_in_window_open(signing_in);
            soft_signal(
                &config,
                &path,
                &live,
                &CheckKind::Manual,
                "http://127.0.0.1:1",
                notice,
                false,
            )
            .publish
        };
        let flag = [Feature::LaunchSignInWindow];
        assert!(shown(&flag, false, Notice::SignedOut));
        assert!(!shown(&flag, true, Notice::SignedOut));
        assert!(
            shown(&flag, true, Notice::Unavailable),
            "only the sign-in bubble is held back"
        );
        assert!(shown(&[Feature::AutoSignIn], true, Notice::SignedOut));
        assert!(shown(&[], true, Notice::SignedOut));
    }

    #[test]
    fn with_no_window_the_tray_starts_a_sign_in_after_a_fresh_look_at_the_keychain() {
        let config = Mutex::new(Config::default_for("UTC").unwrap());
        config
            .lock()
            .unwrap()
            .set_feature(Feature::LaunchSignInWindow, true);
        let live = LivePet::new(1.0);
        // The last answer said signed in, and the keychain now has nothing.
        live.set_signed_in(true);
        assert!(matches!(
            timer_request_with(&config, &live, "sign-in", &|| SessionRead::Missing),
            Some(TimerRequest::Connect)
        ));
        assert_eq!(live.signed_in(), Some(false));
        // A sign-in made in a terminal is noticed, not started again.
        live.set_observed(ObservedCheck::SignedOut);
        assert!(timer_request_with(&config, &live, "sign-in", &|| {
            SessionRead::Usable { until: None }
        })
        .is_none());
        assert_eq!(live.signed_in(), Some(true));
        assert_eq!(live.observed(), ObservedCheck::Waiting);
        assert!(
            timer_request_with(&config, &live, "sign-in", &|| { SessionRead::Unreadable })
                .is_none()
        );
        assert!(matches!(
            timer_request_with(&config, &live, "sign-out", &|| SessionRead::Missing),
            Some(TimerRequest::SignOut)
        ));
        assert!(
            timer_request_with(&config, &live, "check-now", &|| SessionRead::Missing).is_none()
        );
        live.set_demo(true);
        for id in ["sign-in", "sign-out"] {
            assert!(
                timer_request_with(&config, &live, id, &|| SessionRead::Missing).is_none(),
                "{id}: --demo has no sign-in to touch"
            );
        }
    }

    #[test]
    fn an_approval_during_a_new_request_starts_nothing_more() {
        let orbit = server(|| (200, token_body(None)));
        let config = config_on(orbit.base(), &[Feature::LaunchSignInWindow]);
        let backend = backend(orbit.base());
        let (live, inbox, store) = (LivePet::new(1.0), Inbox::new(), TestStore::new());
        let mut flow = None;
        live.request_new_sign_in();
        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        assert_eq!(orbit.count(PATH_DEVICE_CODE), 1);
        updates(&inbox);

        // The user cancelled and reopened while a poll was out, so the
        // keychain still looked empty. Then that poll came back approved.
        live.request_new_sign_in();
        step_flow(&backend, &store, &live, &inbox, &mut flow, later());
        assert!(matches!(updates(&inbox).as_slice(), [SignInUpdate::Done]));

        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        assert_eq!(orbit.count(PATH_DEVICE_CODE), 1, "no code nobody needs");
        assert!(flow.is_none(), "no flow left polling with no window");
        assert!(updates(&inbox).is_empty());
        assert_eq!(live.phase(), FlowPhase::Idle);
        assert_eq!(live.signed_in(), Some(true));
    }

    #[test]
    fn with_only_auto_sign_in_an_approval_leaves_a_waiting_request_alone() {
        let orbit = server(|| (200, token_body(None)));
        let config = config_on(orbit.base(), &[Feature::AutoSignIn]);
        let backend = backend(orbit.base());
        let (live, inbox, store) = (LivePet::new(1.0), Inbox::new(), TestStore::new());
        let mut flow = None;
        live.request_connect();
        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        live.request_connect();
        step_flow(&backend, &store, &live, &inbox, &mut flow, later());
        assert!(live.take_flow_requests().connect.is_some());
    }

    #[test]
    fn sign_out_then_quit_still_revokes_and_deletes_under_the_flag_only() {
        for (features, finished) in [
            (&[Feature::LaunchSignInWindow][..], true),
            (&[Feature::AutoSignIn][..], false),
        ] {
            let orbit = server(|| (400, PENDING.to_string()));
            let config = config_on(orbit.base(), features);
            let backend = backend(orbit.base());
            let (live, store) = (LivePet::new(1.0), TestStore::new());
            let now = Utc::now();
            live.queue_sign_out(Some("refresh-0"));
            // A check that was running saved a rotated record after the delete.
            store
                .store(
                    &StoredToken::issue(
                        "access-1",
                        Some("refresh-1".to_string()),
                        now + chrono::TimeDelta::seconds(100),
                        now,
                    )
                    .unwrap(),
                )
                .unwrap();
            finish_sign_out_on_quit(&backend, &config, &store, &live);
            assert_eq!(store.load(now).unwrap().is_none(), finished, "{features:?}");
            let revoked = orbit.bodies(PATH_DEVICE_REVOKE);
            if finished {
                assert_eq!(revoked.len(), 2, "both refresh tokens");
                assert!(revoked[0].contains("refresh-0"));
                assert!(revoked[1].contains("refresh-1"));
                assert!(live.take_sign_out().is_none());
            } else {
                assert!(revoked.is_empty());
                assert!(live.take_sign_out().is_some(), "left as it was");
            }
        }
    }

    #[test]
    fn demo_sign_out_is_refused_only_under_the_flag() {
        let off = config_on("http://127.0.0.1:1", &[]);
        let on = config_on("http://127.0.0.1:1", &[Feature::LaunchSignInWindow]);
        let off = off.lock().unwrap();
        let on = on.lock().unwrap();
        assert!(demo_sign_out_refused(true, Some(&on)));
        assert!(!demo_sign_out_refused(false, Some(&on)));
        assert!(!demo_sign_out_refused(true, Some(&off)));
        assert!(!demo_sign_out_refused(true, None));
    }

    /// `scripts/mock_orbit.rs` on a free loopback port, for the life of the
    /// test process.
    fn script_mock(script: Script) -> String {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || serve_forever(&listener, script));
        base
    }

    #[test]
    fn end_to_end_on_the_mock_approval_saves_the_token_and_the_first_check_uses_it() {
        let base = script_mock(Script::Approve);
        let config = config_on(&base, &[Feature::LaunchSignInWindow]);
        let backend = backend(&base);
        let (live, inbox, store) = (LivePet::new(1.0), Inbox::new(), TestStore::new());
        let path = std::env::temp_dir()
            .join(format!("stardust-test-{}", uuid::Uuid::new_v4()))
            .join("config.json");
        let mut flow = None;
        let attempt = live.request_new_sign_in();
        run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
        assert!(matches!(
            updates(&inbox).as_slice(),
            [SignInUpdate::Issued { attempt: stamped, .. }] if *stamped == attempt
        ));
        step_flow(&backend, &store, &live, &inbox, &mut flow, later());
        assert!(matches!(updates(&inbox).as_slice(), [SignInUpdate::Done]));
        assert_eq!(live.signed_in(), Some(true));

        // Done is what asks the window for a check. This is that check, with
        // the saved token as the bearer, which the mock verifies.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let signal = runtime.block_on(run_check(
            &backend,
            &config,
            &path,
            &store,
            &live,
            &mut stardust_core::route::Router::new(),
            &CheckKind::Manual,
            true,
        ));
        assert!(signal.publish);
        assert!(matches!(
            live.observed(),
            ObservedCheck::Succeeded { item_count: 1, .. }
        ));
        assert_eq!(live.signed_in(), Some(true));
        assert!(
            live.signed_in_at(Utc::now()).unwrap(),
            "the session end was noted, and it is still ahead"
        );
    }

    #[test]
    fn end_to_end_on_the_mock_each_scripted_ending_reaches_the_window() {
        for (script, expected) in [
            (Script::Deny, SignInEnd::Denied),
            (Script::Expire, SignInEnd::Expired),
        ] {
            let base = script_mock(script);
            let config = config_on(&base, &[Feature::LaunchSignInWindow]);
            let backend = backend(&base);
            let (live, inbox, store) = (LivePet::new(1.0), Inbox::new(), TestStore::new());
            let mut flow = None;
            live.request_new_sign_in();
            run_requests(&backend, &config, &store, &live, &inbox, &mut flow);
            updates(&inbox);
            step_flow(&backend, &store, &live, &inbox, &mut flow, later());
            assert!(flow.is_none(), "{expected:?}");
            assert!(store.load(Utc::now()).unwrap().is_none());
            assert!(
                matches!(
                    updates(&inbox).as_slice(),
                    [SignInUpdate::Ended { end, .. }] if *end == expected
                ),
                "{expected:?}"
            );
        }

        let base = script_mock(Script::Off);
        let config = config_on(&base, &[Feature::LaunchSignInWindow]);
        let backend_off = backend(&base);
        let (live, inbox, store) = (LivePet::new(1.0), Inbox::new(), TestStore::new());
        let mut flow = None;
        live.request_new_sign_in();
        run_requests(&backend_off, &config, &store, &live, &inbox, &mut flow);
        assert!(matches!(
            updates(&inbox).as_slice(),
            [SignInUpdate::Ended {
                end: SignInEnd::Unavailable,
                ..
            }]
        ));

        let base = script_mock(Script::Pending);
        let config = config_on(&base, &[Feature::LaunchSignInWindow]);
        let backend_pending = backend(&base);
        live.request_new_sign_in();
        run_requests(&backend_pending, &config, &store, &live, &inbox, &mut flow);
        updates(&inbox);
        step_flow(&backend_pending, &store, &live, &inbox, &mut flow, later());
        assert!(flow.is_some(), "still waiting for approval");
        assert!(updates(&inbox).is_empty());
    }

    #[test]
    fn the_launch_window_is_the_only_thing_that_starts_a_sign_in_with_its_flag_on() {
        let config = Mutex::new(Config::default_for("UTC").unwrap());
        config
            .lock()
            .unwrap()
            .set_feature(Feature::AutoSignIn, true);
        assert!(first_run_connect(&config));
        config
            .lock()
            .unwrap()
            .set_feature(Feature::LaunchSignInWindow, true);
        assert!(!first_run_connect(&config));
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
