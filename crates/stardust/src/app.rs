use crate::actions::{apply_menu, MenuEffect};
use crate::cli::{self, AutostartAction, CharacterAction, Command};
use crate::deliver;
use crate::inbox::{Inbox, UiMsg};
use crate::keychain::KeychainTokenStore;
use crate::live::LivePet;
use crate::notify;
use crate::tray::Tray;
use crate::window::{self, RunError, Session};
use chrono::{DateTime, Utc};
use stardust_core::api::{
    resolve_server_base, AlertItem, DesktopApi, DesktopStatus, MockOrbitClient, OrbitError,
    ENV_SERVER_URL,
};
use stardust_core::check::{note_anchor, note_success, perform_check, retry_delay, CheckKind};
use stardust_core::config::{default_config_path, load_config, save_config, Config};
use stardust_core::http::HttpOrbitClient;
use stardust_core::outbound::{prepare_posts, DeliveryWindow};
use stardust_core::pet::{pet_pose, tray_glyph, ObservedCheck};
use stardust_core::policy::{calm_notice_in, Notice};
use stardust_core::position::ScreenPoint;
use stardust_core::schedule::{parse_timezone, FireSource};
use stardust_core::session::{current_access, Access, Refresher};
use stardust_core::text::{menu_text, menu_text_in, with_connection_note_in};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

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
        Command::SignOut => crate::signin::sign_out(),
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
        Command::Language { config, language } => {
            let path = config_path(config);
            let mut loaded = load_or_create(&path)?;
            loaded.set_language(language);
            save_config(&path, &loaded).map_err(|err| err.to_string())?;
            println!(
                "language {}",
                match language {
                    Some(language) => language.code(),
                    None => "auto",
                }
            );
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
                windows_unavailable = true;
                eprintln!("stardust: windows are unavailable: {err}");
                inbox.clear_proxy();
                if tray.borrow().is_none() {
                    let loaded = config.lock().unwrap_or_else(|err| err.into_inner());
                    let language = loaded.effective_language();
                    let text = with_connection_note_in(
                        menu_text_in(&loaded, Utc::now(), language),
                        live.observed(),
                        language,
                    );
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
                    if let UiMsg::Checked(presentation) = message {
                        print_lines(&presentation.summary.lines);
                    }
                }
            }
        }
    }
    while windows_unavailable && !quit.load(Ordering::SeqCst) {
        match inbox.wait_timeout(Duration::from_secs(45)) {
            Some(UiMsg::Menu(id)) => {
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
            Some(UiMsg::SignIn(_)) => {}
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
    runtime.block_on(async move {
        let mut failures = 0_u32;
        loop {
            if quit.load(Ordering::SeqCst) {
                break;
            }
            if manual.swap(false, Ordering::SeqCst) {
                let signal = run_check(
                    &backend,
                    &config,
                    &path,
                    &tokens,
                    &live,
                    &CheckKind::Manual,
                    require_token,
                )
                .await;
                failures = next_failures(failures, signal.soften);
                inbox.push(UiMsg::Pet);
                if signal.publish {
                    inbox.push(UiMsg::Checked(signal.presentation));
                }
                continue;
            }
            let planned = plan_next(&config, failures);
            if tokio::time::timeout(planned.delay, wake.notified())
                .await
                .is_ok()
            {
                continue;
            }
            let Some(slot) = planned.slot else {
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
                &kind,
                require_token,
            )
            .await;
            failures = next_failures(failures, signal.soften);
            inbox.push(UiMsg::Pet);
            if signal.publish {
                inbox.push(UiMsg::Checked(signal.presentation));
            }
        }
    });
}

fn next_failures(current: u32, soften: bool) -> u32 {
    if soften {
        current.saturating_add(1)
    } else {
        0
    }
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
    let now = Utc::now();
    let timezone = loaded.schedule().timezone();
    let local_now = now.with_timezone(&timezone);
    let anchor = loaded
        .state()
        .interval_anchor
        .map(|instant| instant.with_timezone(&timezone));
    let Some(fire) = loaded.schedule().next_fire(local_now, anchor) else {
        return Planned {
            delay: retry_delay(Duration::from_secs(60), failures),
            slot: None,
        };
    };
    let at = fire.at.with_timezone(&Utc);
    let delay = retry_delay(
        at.signed_duration_since(now)
            .to_std()
            .unwrap_or(Duration::ZERO),
        failures,
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
}

async fn run_check(
    client: &Backend,
    config: &Mutex<Config>,
    path: &Path,
    tokens: &KeychainTokenStore,
    live: &LivePet,
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
            Access::Ready { token, .. } => Some(token),
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
        if let Access::Ready { token, .. } = current_access(tokens, refresher, Utc::now(), true) {
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
            let records = {
                let mut loaded = config.lock().unwrap_or_else(|err| err.into_inner());
                note_anchor(loaded.state_mut(), kind);
                note_success(loaded.state_mut(), now, None);
                if let Err(err) = save_config(path, &loaded) {
                    eprintln!("stardust: could not save config: {err}");
                }
                loaded.integrations().to_vec()
            };
            live.set_observed(ObservedCheck::Succeeded {
                item_count,
                high_severity,
            });
            let local = now.with_timezone(&schedule.timezone());
            let notes = prepare_posts(
                &records,
                tokens,
                &outcome.items,
                &DeliveryWindow {
                    quiet: schedule.is_quiet_at(local),
                    manual: matches!(kind, CheckKind::Manual),
                    now,
                    interval_minutes: schedule.interval_minutes().unwrap_or(1440),
                    server_base: &server_base,
                },
            );
            deliver::deliver(&notes);
            if outcome.presentation.show_os_notification {
                notify::show(&outcome.presentation);
            }
            CheckSignal {
                presentation: outcome.presentation,
                publish: true,
                soften: false,
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
        note_anchor(loaded.state_mut(), kind);
        if let Err(err) = save_config(path, &loaded) {
            eprintln!("stardust: could not save config: {err}");
        }
        loaded.effective_language()
    };
    live.set_observed(observed);
    let manual = matches!(kind, CheckKind::Manual);
    CheckSignal {
        presentation: calm_notice_in(notice, server_base, language),
        publish: manual || previous != observed,
        soften,
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
            with_connection_note_in(
                menu_text_in(&loaded, Utc::now(), language),
                observed,
                language,
            ),
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

fn print_lines(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}
