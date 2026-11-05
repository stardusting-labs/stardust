//! The one winit event loop for the whole process.
//!
//! winit allows a single `EventLoop` per process (a second `build()` returns
//! `RecreationAttempt`), and on macOS and Windows the tray icon needs that
//! loop running on the main thread to receive clicks. So the loop is built
//! once, the tray is installed from inside it, and it keeps running while no
//! window is open. It ends only on Quit.

use crate::actions::{apply_menu, MenuEffect};
use crate::font::{self, Painter, Pen};
use crate::inbox::{Inbox, SignInUpdate, UiMsg, Wake};
use crate::keychain::KeychainTokenStore;
use crate::live::LivePet;
use crate::sprites;
use crate::tray::{tray_text, Tray};
use chrono::Utc;
use stardust_core::a11y::{
    text_scale, BUBBLE_BACKGROUND, BUBBLE_TEXT, SETTINGS_BACKGROUND, SETTINGS_FOCUS_FILL,
    SETTINGS_FOCUS_TEXT, SETTINGS_MUTED_TEXT, SETTINGS_TEXT,
};
use stardust_core::api::{https_origin, is_allowed_https_url, resolve_server_base, ENV_SERVER_URL};
use stardust_core::config::{save_config, Config};
use stardust_core::connect::FlowPhase;
use stardust_core::i18n::{tr, Language, Msg};
use stardust_core::pet::{
    pet_frame, pet_pose, tray_glyph, ObservedCheck, PetPose, PetScale, PetSkin,
};
use stardust_core::policy::{Presentation, Summary};
use stardust_core::position::{layout_bubble, LayoutRequest, ScreenPoint, ScreenRect, WindowSize};
use stardust_core::settings::{
    Key, Line, LineKind, SettingsAction, SettingsModel, SettingsView, SignInStage,
};
use stardust_core::signin_window::{
    opens_at_launch, LaunchWindow, SessionRead, SignInAction, SignInLine, SignInLineKind,
    SignInModel, Tick,
};
use stardust_core::text::test_send_target;
use std::cell::RefCell;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WinitKey, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

const BUBBLE_COLOR: u32 = BUBBLE_BACKGROUND;
const TEXT_COLOR: u32 = BUBBLE_TEXT;
/// How often the tray's "Next check" line is redrawn while nothing else happens.
const TRAY_REFRESH: Duration = Duration::from_secs(45);
/// Extra space between settings lines, in bitmap pixels before scaling.
const LINE_GAP: u32 = 4;
/// Widest settings line, in bitmap cells.
const SETTINGS_COLUMNS: u32 = 52;

type SbContext = softbuffer::Context<winit::event_loop::OwnedDisplayHandle>;
type SbSurface = softbuffer::Surface<winit::event_loop::OwnedDisplayHandle, Window>;

pub struct Session {
    pub config_path: PathBuf,
    pub config: Arc<Mutex<Config>>,
    pub inbox: Inbox,
    pub manual: Arc<AtomicBool>,
    pub quit: Arc<AtomicBool>,
    pub wake: Arc<tokio::sync::Notify>,
    pub tray: Rc<RefCell<Option<Tray>>>,
    pub live: LivePet,
    /// `--demo`: no sign-in.
    pub demo: bool,
}

pub enum RunError {
    /// No display, or the event loop could not be built. Nothing ran.
    Unavailable(String),
    /// The loop ran and then a window or the tray failed.
    Failed(String),
}

struct SettingsWindow {
    surface: SbSurface,
    model: SettingsModel,
    cursor_y: f64,
}

/// The launch sign-in window. It only draws: polling stays on the check timer.
struct SignInWindow {
    surface: SbSurface,
    model: SignInModel,
    /// Updates from any other attempt are ignored.
    attempt: u64,
    cursor_y: f64,
    /// Made on the first Copy. On Linux the copied text is served from here,
    /// so it stays until this window closes.
    clipboard: Option<arboard::Clipboard>,
    title: String,
    next_tick: Option<Instant>,
}

struct Bubble {
    surface: SbSurface,
    presentation: Presentation,
    deadline: Instant,
}

struct Drag {
    grab_x: f64,
    grab_y: f64,
}

pub struct PopupApp {
    session: Session,
    context: SbContext,
    character: Option<SbSurface>,
    bubble: Option<Bubble>,
    settings: Option<SettingsWindow>,
    sign_in_window: Option<SignInWindow>,
    /// What the keychain held at launch. Read only with the launch sign-in
    /// window on.
    launch_session: Option<SessionRead>,
    /// The first-run settings window waits for the sign-in window to close,
    /// so two windows never compete at launch.
    pending_first_run: bool,
    monitors: Vec<ScreenRect>,
    drag: Option<Drag>,
    failure: Option<String>,
    started: bool,
    painter: Painter,
    shift: bool,
    signed_in: bool,
    sign_in: SignInStage,
    sign_in_running: bool,
    start_at_login: bool,
    next_tray_refresh: Instant,
}

impl Drop for PopupApp {
    fn drop(&mut self) {
        self.session.inbox.clear_proxy();
        // Its windows go with it. A run that carries on with no windows must
        // not keep holding back the notice that says to sign in.
        self.session.live.set_sign_in_window_open(false);
    }
}

/// Build the event loop once and run it until Quit.
pub fn run(session: Session) -> Result<(), RunError> {
    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .map_err(|err| RunError::Unavailable(err.to_string()))?;
    let context = softbuffer::Context::new(event_loop.owned_display_handle())
        .map_err(|err| RunError::Unavailable(err.to_string()))?;
    session.inbox.set_proxy(event_loop.create_proxy());
    let launch_window = session
        .config
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .features()
        .launch_sign_in_window;
    let launch_session = (launch_window && !session.demo).then(crate::signin::session);
    let signed_in = session.demo
        || match launch_session {
            Some(read) => matches!(read, SessionRead::Usable { .. }),
            None => crate::signin::signed_in(),
        };
    session.live.set_signed_in(signed_in);
    if let Some(SessionRead::Usable { until }) = launch_session {
        session.live.note_session(until);
    }
    let mut app = PopupApp {
        session,
        context,
        character: None,
        bubble: None,
        settings: None,
        sign_in_window: None,
        launch_session,
        pending_first_run: false,
        monitors: Vec::new(),
        drag: None,
        failure: None,
        started: false,
        painter: Painter::new(),
        shift: false,
        signed_in,
        sign_in: SignInStage::Idle,
        sign_in_running: false,
        start_at_login: crate::autostart::is_enabled(),
        next_tray_refresh: Instant::now() + TRAY_REFRESH,
    };
    let ran = event_loop.run_app(&mut app).map_err(|err| err.to_string());
    match (ran, app.failure.take()) {
        (Err(err), _) => Err(RunError::Failed(err)),
        (Ok(()), Some(err)) => Err(RunError::Failed(err)),
        (Ok(()), None) => Ok(()),
    }
}

impl PopupApp {
    fn fail(&mut self, event_loop: &ActiveEventLoop, err: String) {
        self.failure = Some(err);
        event_loop.exit();
    }

    fn refresh_monitors(&mut self, event_loop: &ActiveEventLoop) {
        let mut scale = 0.0f64;
        self.monitors = event_loop
            .available_monitors()
            .map(|monitor| {
                let factor = monitor.scale_factor();
                if factor.is_finite() {
                    scale = scale.max(factor);
                }
                let position = monitor.position();
                let size = monitor.size();
                ScreenRect {
                    x: position.x,
                    y: position.y,
                    width: size.width,
                    height: size.height,
                }
            })
            .collect();
        if scale > 0.0 {
            self.session.live.set_scale_factor(scale);
        }
    }

    fn open_window(
        &self,
        event_loop: &ActiveEventLoop,
        origin: ScreenPoint,
        width: u32,
        height: u32,
    ) -> Result<SbSurface, String> {
        let attributes = Window::default_attributes()
            .with_title("Stardust")
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(true)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_active(false)
            .with_position(PhysicalPosition::new(origin.x, origin.y))
            .with_inner_size(PhysicalSize::new(width, height));
        self.surface_for(event_loop, attributes)
    }

    fn surface_for(
        &self,
        event_loop: &ActiveEventLoop,
        attributes: winit::window::WindowAttributes,
    ) -> Result<SbSurface, String> {
        let window = event_loop
            .create_window(attributes)
            .map_err(|err| err.to_string())?;
        let mut surface =
            softbuffer::Surface::new(&self.context, window).map_err(|err| err.to_string())?;
        resize_surface(&mut surface)?;
        Ok(surface)
    }

    fn language(&self) -> Language {
        self.session
            .config
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .effective_language()
    }

    fn install_tray(&mut self) {
        if self.session.tray.borrow().is_some() {
            return;
        }
        let language = self.language();
        let (text, glyph) = {
            let config = self
                .session
                .config
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            let observed = self.session.live.observed();
            let local = Utc::now().with_timezone(&config.schedule().timezone());
            (
                tray_text(&config, &self.session.live, language),
                tray_glyph(pet_pose(observed, config.schedule().is_quiet_at(local))),
            )
        };
        match Tray::install(
            &self.session.inbox,
            &text,
            glyph,
            self.session.live.scale_factor(),
            language,
        ) {
            Ok(tray) => *self.session.tray.borrow_mut() = Some(tray),
            Err(err) => self.failure = Some(format!("tray: {err}")),
        }
    }

    fn settings_view(&self) -> SettingsView {
        let config = self
            .session
            .config
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let host = server_host(&config);
        // The saved record can lapse between checks. Only this flag looks.
        let signed_in = if config.features().launch_sign_in_window {
            self.session
                .live
                .signed_in_at(Utc::now())
                .unwrap_or(self.signed_in)
        } else {
            self.signed_in
        };
        SettingsView {
            signed_in,
            sign_in: self.sign_in.clone(),
            interval_minutes: config.schedule().interval_minutes(),
            start_at_login: self.start_at_login,
            start_at_login_available: crate::autostart::available(),
            character_visible: config.character_visible(),
            skin: config.pet_skin(),
            scale: config.pet_scale(),
            server_host: host,
        }
    }

    fn open_settings(&mut self, event_loop: &ActiveEventLoop, first_run: bool) {
        if let Some(settings) = self.settings.as_ref() {
            settings.surface.window().focus_window();
            settings.surface.window().request_redraw();
            return;
        }
        // Settings opened from the tray while the sign-in window is up takes
        // over the welcome, or `onboarded` would never be set.
        let first_run = first_run || std::mem::take(&mut self.pending_first_run);
        let model = SettingsModel::new(first_run);
        let view = self.settings_view();
        let language = self.language();
        let rows = model.lines(&view, language).len() as u32;
        let scale = text_scale(self.session.live.scale_factor());
        let (width, height) = settings_size(SETTINGS_COLUMNS, rows, scale);
        let monitor = self.monitors.first().copied().unwrap_or(ScreenRect {
            x: 0,
            y: 0,
            width: 1280,
            height: 800,
        });
        let origin = centered(monitor, width, height);
        let attributes = Window::default_attributes()
            .with_title(model.focus_label(&view, language))
            .with_decorations(true)
            .with_resizable(false)
            .with_active(true)
            .with_position(PhysicalPosition::new(origin.x, origin.y))
            .with_inner_size(PhysicalSize::new(width, height));
        match self.surface_for(event_loop, attributes) {
            Ok(surface) => {
                surface.window().request_redraw();
                self.settings = Some(SettingsWindow {
                    surface,
                    model,
                    cursor_y: -1.0,
                });
            }
            Err(err) => self.fail(event_loop, err),
        }
    }

    fn close_settings(&mut self) {
        let Some(settings) = self.settings.take() else {
            return;
        };
        if settings.model.first_run() {
            let mut config = self
                .session
                .config
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            config.set_onboarded(true);
            if let Err(err) = save_config(&self.session.config_path, &config) {
                eprintln!("stardust: could not save config: {err}");
            }
        }
    }

    fn settings_id(&self) -> Option<WindowId> {
        self.settings
            .as_ref()
            .map(|settings| settings.surface.window().id())
    }

    fn redraw_settings(&self) {
        let Some(settings) = self.settings.as_ref() else {
            return;
        };
        let view = self.settings_view();
        settings
            .surface
            .window()
            .set_title(&settings.model.focus_label(&view, self.language()));
        settings.surface.window().request_redraw();
    }

    fn paint_settings(&mut self) {
        let view = self.settings_view();
        let language = self.language();
        let scale = text_scale(self.session.live.scale_factor());
        let Some(settings) = self.settings.as_mut() else {
            return;
        };
        let lines = settings.model.lines(&view, language);
        paint_settings(&mut settings.surface, &self.painter, &lines, scale);
    }

    fn on_settings_key(&mut self, event_loop: &ActiveEventLoop, event: &KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }
        let Some(key) = window_key(&event.logical_key, self.shift) else {
            return;
        };
        let view = self.settings_view();
        let Some(settings) = self.settings.as_mut() else {
            return;
        };
        let action = settings.model.handle_key(key, &view);
        self.apply_settings(event_loop, action);
    }

    fn on_settings_click(&mut self, event_loop: &ActiveEventLoop) {
        let scale = text_scale(self.session.live.scale_factor());
        let view = self.settings_view();
        let Some(settings) = self.settings.as_mut() else {
            return;
        };
        let Some(index) = line_at(settings.cursor_y, scale) else {
            return;
        };
        let action = settings.model.click(index, &view);
        self.apply_settings(event_loop, action);
    }

    fn apply_settings(&mut self, event_loop: &ActiveEventLoop, action: SettingsAction) {
        match action {
            SettingsAction::None => return,
            SettingsAction::Moved => {}
            SettingsAction::Close => {
                self.close_settings();
                return;
            }
            SettingsAction::StartSignIn if self.launch_window_on() => {
                self.sign_in_from_menu(event_loop);
            }
            SettingsAction::StartSignIn => self.start_sign_in(),
            SettingsAction::SignOut if self.launch_window_on() => {
                self.sign_out_under_flag(event_loop);
            }
            SettingsAction::SignOut => {
                let finish_on_timer = self.auto_sign_in() && !self.session.demo;
                // With auto sign-in the check timer finishes the sign-out on
                // its next tick and revokes the refresh token. The keychain
                // copy goes now either way.
                let refresh = finish_on_timer
                    .then(crate::signin::saved_refresh_token)
                    .flatten();
                if let Err(err) = crate::signin::clear_keychain_token() {
                    eprintln!("stardust: could not sign out: {err}");
                }
                if finish_on_timer {
                    self.session.live.queue_sign_out(refresh.as_deref());
                    self.session.wake.notify_one();
                }
                self.signed_in = self.session.demo;
                self.sign_in = SignInStage::Idle;
                self.session.live.set_signed_in(self.session.demo);
                if !self.session.demo {
                    self.session.live.set_observed(ObservedCheck::SignedOut);
                }
                self.refresh_tray();
            }
            SettingsAction::SetInterval(minutes) => {
                self.on_menu(event_loop, &format!("interval-{minutes}"));
            }
            SettingsAction::SetStartAtLogin(enabled) => {
                if let Err(err) = crate::autostart::set_enabled(enabled) {
                    eprintln!("stardust: {err}");
                }
                self.start_at_login = crate::autostart::is_enabled();
            }
            SettingsAction::SetCharacterVisible(visible) => {
                self.update_config(|config| config.set_character_visible(visible));
                self.sync_character(event_loop);
            }
            SettingsAction::SetSkin(skin) => {
                self.on_menu(event_loop, skin.menu_id());
                self.redraw_character();
            }
            SettingsAction::SetScale(scale) => {
                self.on_menu(event_loop, scale.menu_id());
                self.redraw_character();
            }
        }
        self.redraw_settings();
    }

    fn update_config(&self, change: impl FnOnce(&mut Config)) {
        let mut config = self
            .session
            .config
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        change(&mut config);
        if let Err(err) = save_config(&self.session.config_path, &config) {
            eprintln!("stardust: could not save config: {err}");
        }
    }

    fn auto_sign_in(&self) -> bool {
        self.session
            .config
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .features()
            .auto_sign_in
    }

    fn launch_window_on(&self) -> bool {
        self.session
            .config
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .features()
            .launch_sign_in_window
    }

    /// "Sign in..." in the tray, or the settings Account row, with the launch
    /// sign-in window on. Looks in the keychain first, so a sign-in made in a
    /// terminal shows as signed in rather than starting another one.
    fn sign_in_from_menu(&mut self, event_loop: &ActiveEventLoop) {
        if self.session.demo {
            return;
        }
        if let Some(window) = self.sign_in_window.as_ref() {
            window.surface.window().focus_window();
            return;
        }
        match crate::signin::session() {
            SessionRead::Usable { until } => self.found_session(until),
            SessionRead::Missing => self.open_sign_in_window(event_loop, false),
            SessionRead::Unreadable => self.open_sign_in_window(event_loop, true),
        }
    }

    /// The keychain holds a usable record the app did not know about.
    fn found_session(&mut self, until: Option<chrono::DateTime<Utc>>) {
        self.session.live.note_session(until);
        self.session.live.set_signed_in(true);
        self.session.live.set_observed(ObservedCheck::Waiting);
        self.on_sign_in(SignInUpdate::Done);
    }

    fn open_sign_in_window(&mut self, event_loop: &ActiveEventLoop, keychain_unreadable: bool) {
        let now = Instant::now();
        let host = server_host(
            &self
                .session
                .config
                .lock()
                .unwrap_or_else(|err| err.into_inner()),
        );
        let model = if keychain_unreadable {
            SignInModel::keychain_unreadable(host, now)
        } else {
            SignInModel::requesting(host, now)
        };
        let scale = text_scale(self.session.live.scale_factor());
        let (width, height) = settings_size(SETTINGS_COLUMNS, model.max_lines() as u32, scale);
        let monitor = self.monitors.first().copied().unwrap_or(ScreenRect {
            x: 0,
            y: 0,
            width: 1280,
            height: 800,
        });
        let origin = centered(monitor, width, height);
        let title = model.title(self.language());
        let attributes = Window::default_attributes()
            .with_title(title.as_str())
            .with_decorations(true)
            .with_resizable(false)
            .with_active(true)
            .with_position(PhysicalPosition::new(origin.x, origin.y))
            .with_inner_size(PhysicalSize::new(width, height));
        match self.surface_for(event_loop, attributes) {
            Ok(surface) => {
                surface.window().request_redraw();
                let attempt = if keychain_unreadable {
                    0
                } else {
                    let attempt = self.session.live.request_new_sign_in();
                    self.session.wake.notify_one();
                    attempt
                };
                self.session.live.set_sign_in_window_open(true);
                self.sign_in_window = Some(SignInWindow {
                    surface,
                    model,
                    attempt,
                    cursor_y: -1.0,
                    clipboard: None,
                    title,
                    next_tick: None,
                });
            }
            Err(err) => self.fail(event_loop, err),
        }
    }

    /// Closing always stops the polling, because every flow under this flag
    /// belongs to this window and nothing could cancel one left behind. The app
    /// keeps running in the tray.
    fn close_sign_in_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.sign_in_window.take().is_none() {
            return;
        }
        self.session.live.set_sign_in_window_open(false);
        self.session.live.request_cancel();
        self.session.wake.notify_one();
        if std::mem::take(&mut self.pending_first_run) {
            self.open_settings(event_loop, true);
        }
        self.refresh_tray();
    }

    fn sign_in_window_id(&self) -> Option<WindowId> {
        self.sign_in_window
            .as_ref()
            .map(|window| window.surface.window().id())
    }

    /// Redraw only this window. The title changes when the state does, and
    /// the tray is left alone, because rebuilding its menu every second could
    /// disturb one that is open.
    fn redraw_sign_in_window(&mut self) {
        let language = self.language();
        let now = Instant::now();
        let Some(window) = self.sign_in_window.as_mut() else {
            return;
        };
        let title = window.model.title(language);
        if title != window.title {
            window.surface.window().set_title(&title);
            window.title = title;
        }
        window.next_tick = window.model.next_wake(now);
        window.surface.window().request_redraw();
    }

    fn paint_sign_in_window(&mut self) {
        let language = self.language();
        let scale = text_scale(self.session.live.scale_factor());
        let Some(window) = self.sign_in_window.as_mut() else {
            return;
        };
        let lines = window.model.lines(Instant::now(), language);
        paint_sign_in(&mut window.surface, &self.painter, &lines, scale);
    }

    fn on_sign_in_window_key(&mut self, event_loop: &ActiveEventLoop, event: &KeyEvent) {
        if event.state != ElementState::Pressed || event.repeat {
            return;
        }
        let Some(key) = window_key(&event.logical_key, self.shift) else {
            return;
        };
        let Some(window) = self.sign_in_window.as_mut() else {
            return;
        };
        let action = window.model.handle_key(key, Instant::now());
        self.apply_sign_in_window(event_loop, action);
    }

    fn on_sign_in_window_click(&mut self, event_loop: &ActiveEventLoop) {
        let scale = text_scale(self.session.live.scale_factor());
        let Some(window) = self.sign_in_window.as_mut() else {
            return;
        };
        let Some(index) = line_at(window.cursor_y, scale) else {
            return;
        };
        let action = window.model.click(index, Instant::now());
        self.apply_sign_in_window(event_loop, action);
    }

    fn apply_sign_in_window(&mut self, event_loop: &ActiveEventLoop, action: SignInAction) {
        match action {
            SignInAction::None => return,
            SignInAction::Moved => {}
            SignInAction::Open(url) => {
                // Detached, so a browser that stays in the foreground does
                // not hold up the window.
                if let Err(err) = open::that_detached(&url) {
                    eprintln!("stardust: could not open the browser: {err}");
                    if let Some(window) = self.sign_in_window.as_mut() {
                        window.model.note_browser_failed();
                    }
                }
            }
            SignInAction::Copy(user_code) => {
                if let Some(window) = self.sign_in_window.as_mut() {
                    let copied = copy_user_code(&mut window.clipboard, &user_code);
                    window.model.note_copied(copied);
                }
            }
            SignInAction::Close => {
                self.close_sign_in_window(event_loop);
                return;
            }
            SignInAction::Restart => {
                if self.restart_sign_in(event_loop) {
                    return;
                }
            }
        }
        self.redraw_sign_in_window();
    }

    /// Ask for a new code. Returns true when the window closed instead, because
    /// the keychain turned out to hold a usable record.
    fn restart_sign_in(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let reread = self
            .sign_in_window
            .as_ref()
            .is_some_and(|window| window.model.is_keychain_unreadable());
        if reread {
            match crate::signin::session() {
                SessionRead::Usable { until } => {
                    self.found_session(until);
                    self.close_sign_in_window(event_loop);
                    return true;
                }
                SessionRead::Unreadable => return false,
                SessionRead::Missing => {}
            }
        }
        let attempt = self.session.live.request_new_sign_in();
        self.session.wake.notify_one();
        if let Some(window) = self.sign_in_window.as_mut() {
            window.attempt = attempt;
            window.model.start_over(Instant::now());
        }
        false
    }

    /// The window for this attempt, if it is still the one on screen.
    fn sign_in_window_for(&mut self, attempt: u64) -> Option<&mut SignInWindow> {
        self.sign_in_window
            .as_mut()
            .filter(|window| window.attempt == attempt)
    }

    /// A phase change from the check timer: the last poll did or did not
    /// reach ORBIT.
    fn sync_sign_in_phase(&mut self) {
        let (user_code, reachable) = match self.session.live.phase() {
            FlowPhase::Waiting { user_code } => (user_code, true),
            FlowPhase::Offline { user_code } => (user_code, false),
            FlowPhase::Idle => return,
        };
        let changed = self
            .sign_in_window
            .as_mut()
            .is_some_and(|window| window.model.on_connection(&user_code, reachable));
        if changed {
            self.redraw_sign_in_window();
        }
    }

    /// The countdown, and closing after a sign-in. Returns when to wake next.
    fn tick_sign_in_window(&mut self, event_loop: &ActiveEventLoop) -> Option<Instant> {
        let now = Instant::now();
        let window = self.sign_in_window.as_mut()?;
        if window.next_tick.is_some_and(|at| now >= at) {
            match window.model.tick(now) {
                Tick::Close => {
                    self.close_sign_in_window(event_loop);
                    return None;
                }
                Tick::Changed => self.redraw_sign_in_window(),
                Tick::Idle => window.next_tick = window.model.next_wake(now),
            }
        }
        self.sign_in_window.as_ref()?.next_tick
    }

    /// Sign out from the tray or the settings window with the launch sign-in
    /// window on. Immediate, like the settings row has always been. `--demo`
    /// has no sign-in to touch.
    fn sign_out_under_flag(&mut self, event_loop: &ActiveEventLoop) {
        if self.session.demo {
            return;
        }
        match crate::signin::begin_sign_out(&KeychainTokenStore::new(), &self.session.live) {
            Ok(()) => {
                self.session.wake.notify_one();
                self.signed_in = false;
                self.sign_in = SignInStage::Idle;
                self.close_sign_in_window(event_loop);
            }
            Err(err) => {
                eprintln!("stardust: could not sign out: {err}");
                self.show_sign_out_failed(event_loop);
            }
        }
        self.refresh_tray();
    }

    /// No window may be open, so this goes in a bubble.
    fn show_sign_out_failed(&mut self, event_loop: &ActiveEventLoop) {
        let language = self.language();
        self.refresh_monitors(event_loop);
        self.show_bubble(
            event_loop,
            Presentation {
                show_bubble: true,
                show_os_notification: false,
                summary: Summary {
                    lines: vec![
                        tr(language, Msg::SignOutFailed).to_string(),
                        tr(language, Msg::SignOutFailedHint).to_string(),
                    ],
                    details_url: String::new(),
                },
            },
        );
    }

    fn start_sign_in(&mut self) {
        if self.session.demo || self.sign_in_running {
            return;
        }
        if self.auto_sign_in() {
            // The check timer runs the device flow; no sign-in thread.
            if let FlowPhase::Waiting { user_code } | FlowPhase::Offline { user_code } =
                self.session.live.phase()
            {
                self.sign_in = SignInStage::Waiting { user_code };
                return;
            }
            self.sign_in = SignInStage::Idle;
            self.session.live.request_connect();
            self.session.wake.notify_one();
            return;
        }
        self.sign_in_running = true;
        self.sign_in = SignInStage::Idle;
        let config = self
            .session
            .config
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone();
        let inbox = self.session.inbox.clone();
        let spawned = std::thread::Builder::new()
            .name("stardust-sign-in".to_string())
            .spawn(move || {
                let result = crate::signin::sign_in_with(&config, &mut |code, _, _| {
                    inbox.push(UiMsg::SignIn(SignInUpdate::Code(code.to_string())));
                });
                inbox.push(UiMsg::SignIn(match result {
                    Ok(()) => SignInUpdate::Done,
                    Err(err) => {
                        eprintln!("stardust: sign-in: {err}");
                        SignInUpdate::Failed
                    }
                }));
            });
        if spawned.is_err() {
            self.sign_in_running = false;
            self.sign_in = SignInStage::Failed;
        }
    }

    fn on_sign_in(&mut self, update: SignInUpdate) {
        let signed_in_now = matches!(update, SignInUpdate::Done);
        match update {
            SignInUpdate::Code(user_code) => self.sign_in = SignInStage::Waiting { user_code },
            SignInUpdate::Done => {
                self.sign_in_running = false;
                self.sign_in = SignInStage::Idle;
                self.signed_in = true;
                self.session.manual.store(true, Ordering::SeqCst);
                self.session.wake.notify_one();
            }
            SignInUpdate::Failed => {
                self.sign_in_running = false;
                self.sign_in = SignInStage::Failed;
            }
            SignInUpdate::Issued {
                attempt,
                code,
                lifetime,
            } => {
                if let Some(window) = self.sign_in_window_for(attempt) {
                    window.model.on_code(code, lifetime, Instant::now());
                }
            }
            SignInUpdate::Ended { attempt, end } => {
                if let Some(window) = self.sign_in_window_for(attempt) {
                    window.model.on_end(end, Instant::now());
                }
            }
        }
        if signed_in_now {
            // Not numbered: the token is already saved, so whichever attempt
            // this was, the app is signed in.
            if let Some(window) = self.sign_in_window.as_mut() {
                window.model.on_signed_in(Instant::now());
            }
        }
        self.redraw_sign_in_window();
        if self.auto_sign_in() || self.launch_window_on() {
            self.refresh_tray();
        }
        self.redraw_settings();
    }

    fn pet_window_size(&self) -> WindowSize {
        let config = self
            .session
            .config
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        pet_frame(config.pet_skin(), config.pet_scale()).canvas
    }

    fn sync_character(&mut self, event_loop: &ActiveEventLoop) {
        let visible = self
            .session
            .config
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .character_visible();
        if !visible {
            self.character = None;
            self.drag = None;
            return;
        }
        let size = self.pet_window_size();
        if let Some(surface) = self.character.as_ref() {
            let current = surface.window().inner_size();
            if current.width != size.width || current.height != size.height {
                let _ = surface
                    .window()
                    .request_inner_size(PhysicalSize::new(size.width, size.height));
            }
            surface.window().request_redraw();
            return;
        }
        let stored = self
            .session
            .config
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .character_position();
        let layout = layout_bubble(&LayoutRequest {
            character_visible: true,
            stored_position: stored,
            monitors: &self.monitors,
            character_size: size,
            bubble_size: WindowSize {
                width: 1,
                height: 1,
            },
        });
        let Some(origin) = layout.character_origin else {
            return;
        };
        match self.open_window(event_loop, origin, size.width, size.height) {
            Ok(surface) => {
                surface.window().request_redraw();
                self.character = Some(surface);
            }
            Err(err) => self.fail(event_loop, err),
        }
    }

    fn show_bubble(&mut self, event_loop: &ActiveEventLoop, presentation: Presentation) {
        if !presentation.show_bubble {
            self.bubble = None;
            return;
        }
        let (width, height) = font::text_size(&presentation.summary.lines);
        let (visible, stored) = {
            let config = self
                .session
                .config
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            (config.character_visible(), config.character_position())
        };
        let layout = layout_bubble(&LayoutRequest {
            character_visible: visible,
            stored_position: stored,
            monitors: &self.monitors,
            character_size: self.pet_window_size(),
            bubble_size: WindowSize { width, height },
        });
        match self.open_window(event_loop, layout.origin, width, height) {
            Ok(surface) => {
                surface.window().request_redraw();
                self.bubble = Some(Bubble {
                    surface,
                    presentation,
                    deadline: Instant::now() + stardust_core::policy::BUBBLE_VISIBLE_FOR,
                });
            }
            Err(err) => self.fail(event_loop, err),
        }
    }

    fn dismiss_expired(&mut self) {
        if self
            .bubble
            .as_ref()
            .is_some_and(|bubble| Instant::now() >= bubble.deadline)
        {
            self.bubble = None;
        }
    }

    fn drain(&mut self, event_loop: &ActiveEventLoop) {
        while self.failure.is_none() {
            let Some(message) = self.session.inbox.try_pop() else {
                break;
            };
            match message {
                UiMsg::Menu(id) => self.on_menu(event_loop, &id),
                UiMsg::Checked(presentation) => {
                    self.refresh_monitors(event_loop);
                    self.sync_character(event_loop);
                    self.show_bubble(event_loop, presentation);
                    self.redraw_character();
                    self.refresh_tray();
                }
                UiMsg::Pet => {
                    if self.session.live.observed() == ObservedCheck::SignedOut {
                        self.signed_in = false;
                    }
                    self.sync_character(event_loop);
                    self.redraw_character();
                    self.refresh_tray();
                    self.sync_sign_in_phase();
                }
                UiMsg::SignIn(update) => self.on_sign_in(update),
            }
        }
    }

    fn on_menu(&mut self, event_loop: &ActiveEventLoop, id: &str) {
        match id {
            "settings" => {
                self.open_settings(event_loop, false);
                return;
            }
            "sign-in" if self.launch_window_on() => {
                self.sign_in_from_menu(event_loop);
                return;
            }
            "sign-out" if self.launch_window_on() => {
                self.sign_out_under_flag(event_loop);
                self.redraw_settings();
                return;
            }
            "sign-in" => {
                self.open_settings(event_loop, false);
                if !self.signed_in {
                    self.start_sign_in();
                }
                self.redraw_settings();
                return;
            }
            other => {
                if let Some(target) = test_send_target(other) {
                    let routing = self
                        .session
                        .config
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .features()
                        .routing;
                    if routing {
                        // Posted from the check timer, like every alert.
                        self.session.live.request_test(target);
                        self.session.wake.notify_one();
                    }
                    return;
                }
            }
        }
        let now = Utc::now();
        let effect = {
            let mut config = self
                .session
                .config
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            match apply_menu(&mut config, id, now) {
                Ok(effect) => {
                    if matches!(effect, MenuEffect::Refresh) {
                        if let Err(err) = save_config(&self.session.config_path, &config) {
                            eprintln!("stardust: could not save config: {err}");
                        }
                    }
                    effect
                }
                Err(err) => {
                    eprintln!("stardust: could not apply menu: {err}");
                    MenuEffect::None
                }
            }
        };
        match effect {
            MenuEffect::None => {}
            MenuEffect::Quit => {
                self.session.quit.store(true, Ordering::SeqCst);
                self.session.wake.notify_one();
                event_loop.exit();
            }
            MenuEffect::CheckNow => {
                self.session.manual.store(true, Ordering::SeqCst);
                self.session.wake.notify_one();
            }
            MenuEffect::Refresh => {
                self.refresh_monitors(event_loop);
                self.sync_character(event_loop);
                self.refresh_tray();
                self.redraw_settings();
            }
        }
    }

    fn refresh_tray(&self) {
        let observed = self.session.live.observed();
        let scale = self.session.live.scale_factor();
        let language = self.language();
        let (text, glyph) = {
            let config = self
                .session
                .config
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            let local = Utc::now().with_timezone(&config.schedule().timezone());
            let pose = pet_pose(observed, config.schedule().is_quiet_at(local));
            (
                tray_text(&config, &self.session.live, language),
                tray_glyph(pose),
            )
        };
        if let Some(tray) = self.session.tray.borrow().as_ref() {
            if let Err(err) = tray.apply(&text, glyph, scale, language) {
                eprintln!("stardust: could not update the tray menu: {err}");
            }
        }
    }

    fn redraw_character(&self) {
        if let Some(surface) = self.character.as_ref() {
            surface.window().request_redraw();
        }
    }

    fn pet_paint(&self) -> (PetSkin, PetPose, PetScale) {
        let observed = self.session.live.observed();
        let config = self
            .session
            .config
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let local = Utc::now().with_timezone(&config.schedule().timezone());
        let pose = pet_pose(observed, config.schedule().is_quiet_at(local));
        (config.pet_skin(), pose, config.pet_scale())
    }

    fn save_character_position(&self, point: ScreenPoint) {
        let mut config = self
            .session
            .config
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        config.set_character_position(point);
        if let Err(err) = save_config(&self.session.config_path, &config) {
            eprintln!("stardust: could not save config: {err}");
        }
    }

    fn character_id(&self) -> Option<WindowId> {
        self.character.as_ref().map(|surface| surface.window().id())
    }

    fn bubble_id(&self) -> Option<WindowId> {
        self.bubble
            .as_ref()
            .map(|bubble| bubble.surface.window().id())
    }
}

impl ApplicationHandler<Wake> for PopupApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.refresh_monitors(event_loop);
        if !self.started {
            self.started = true;
            self.install_tray();
            if self.failure.is_some() {
                event_loop.exit();
                return;
            }
            let onboarded = self
                .session
                .config
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .onboarded();
            let session = self.launch_session.take().unwrap_or(SessionRead::Missing);
            match opens_at_launch(self.launch_window_on(), self.session.demo, session) {
                LaunchWindow::None => {
                    if !onboarded {
                        self.open_settings(event_loop, true);
                    }
                }
                LaunchWindow::SignIn => {
                    self.pending_first_run = !onboarded;
                    self.open_sign_in_window(event_loop, false);
                }
                LaunchWindow::KeychainUnreadable => {
                    self.pending_first_run = !onboarded;
                    self.open_sign_in_window(event_loop, true);
                }
            }
        }
        self.sync_character(event_loop);
        self.drain(event_loop);
        self.refresh_tray();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: Wake) {
        self.drain(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.sign_in_window_id() == Some(id) {
            match event {
                // Closing the window stops polling. The app stays in the tray.
                WindowEvent::CloseRequested => self.close_sign_in_window(event_loop),
                WindowEvent::RedrawRequested => self.paint_sign_in_window(),
                WindowEvent::Resized(_) => self.redraw_sign_in_window(),
                WindowEvent::ModifiersChanged(modifiers) => {
                    self.shift = modifiers.state().shift_key();
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    self.on_sign_in_window_key(event_loop, &event);
                }
                WindowEvent::CursorMoved { position, .. } => {
                    if let Some(window) = self.sign_in_window.as_mut() {
                        window.cursor_y = position.y;
                    }
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => self.on_sign_in_window_click(event_loop),
                _ => {}
            }
            return;
        }
        if self.settings_id() == Some(id) {
            match event {
                WindowEvent::CloseRequested => self.close_settings(),
                WindowEvent::RedrawRequested => self.paint_settings(),
                WindowEvent::Resized(_) => self.redraw_settings(),
                WindowEvent::ModifiersChanged(modifiers) => {
                    self.shift = modifiers.state().shift_key();
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    self.on_settings_key(event_loop, &event);
                }
                WindowEvent::CursorMoved { position, .. } => {
                    if let Some(settings) = self.settings.as_mut() {
                        settings.cursor_y = position.y;
                    }
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => self.on_settings_click(event_loop),
                _ => {}
            }
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                if self.character_id() == Some(id) {
                    let mut config = self
                        .session
                        .config
                        .lock()
                        .unwrap_or_else(|err| err.into_inner());
                    config.set_character_visible(false);
                    if let Err(err) = save_config(&self.session.config_path, &config) {
                        eprintln!("stardust: could not save config: {err}");
                    }
                    drop(config);
                    self.character = None;
                    self.refresh_tray();
                }
                if self.bubble_id() == Some(id) {
                    self.bubble = None;
                }
            }
            WindowEvent::RedrawRequested => {
                if self.character_id() == Some(id) {
                    let (skin, pose, scale) = self.pet_paint();
                    if let Some(surface) = self.character.as_mut() {
                        paint_character(surface, skin, pose, scale);
                    }
                }
                if self.bubble_id() == Some(id) {
                    if let Some(bubble) = self.bubble.as_mut() {
                        paint_bubble(&mut bubble.surface, &bubble.presentation, &self.painter);
                    }
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                if self.bubble_id() == Some(id) && state == ElementState::Pressed {
                    if let Some(bubble) = self.bubble.as_ref() {
                        let config = self
                            .session
                            .config
                            .lock()
                            .unwrap_or_else(|err| err.into_inner());
                        open_details(&config, &bubble.presentation.summary.details_url);
                    }
                    self.bubble = None;
                }
                if self.character_id() == Some(id) {
                    match state {
                        ElementState::Pressed => {
                            self.drag = Some(Drag {
                                grab_x: f64::NAN,
                                grab_y: f64::NAN,
                            });
                        }
                        ElementState::Released => {
                            if let Some(surface) = self.character.as_ref() {
                                if let Ok(position) = surface.window().outer_position() {
                                    self.save_character_position(ScreenPoint {
                                        x: position.x,
                                        y: position.y,
                                    });
                                }
                            }
                            self.drag = None;
                        }
                    }
                }
            }
            WindowEvent::Resized(_) => {
                if self.character_id() == Some(id) || self.bubble_id() == Some(id) {
                    if self.character_id() == Some(id) {
                        self.redraw_character();
                    }
                    if self.bubble_id() == Some(id) {
                        if let Some(bubble) = self.bubble.as_ref() {
                            bubble.surface.window().request_redraw();
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } if self.character_id() == Some(id) => {
                let Some(drag) = self.drag.as_mut() else {
                    return;
                };
                if drag.grab_x.is_nan() {
                    drag.grab_x = position.x;
                    drag.grab_y = position.y;
                    return;
                }
                let Some(surface) = self.character.as_ref() else {
                    return;
                };
                if let Ok(outer) = surface.window().outer_position() {
                    let x = outer.x + position.x as i32 - drag.grab_x as i32;
                    let y = outer.y + position.y as i32 - drag.grab_y as i32;
                    surface
                        .window()
                        .set_outer_position(PhysicalPosition::new(x, y));
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.drain(event_loop);
        if self.failure.is_some() || self.session.quit.load(Ordering::SeqCst) {
            event_loop.exit();
            return;
        }
        self.dismiss_expired();
        let sign_in_wake = self.tick_sign_in_window(event_loop);
        let visible = self
            .session
            .config
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .character_visible();
        if !visible {
            self.character = None;
        }
        let now = Instant::now();
        if now >= self.next_tray_refresh {
            self.refresh_tray();
            self.next_tray_refresh = now + TRAY_REFRESH;
        }
        let wake_at = match &self.bubble {
            Some(bubble) => bubble.deadline.min(self.next_tray_refresh),
            None => self.next_tray_refresh,
        };
        // Sooner only while the sign-in window has a clock on screen.
        let wake_at = sign_in_wake.map_or(wake_at, |at| at.min(wake_at));
        event_loop.set_control_flow(ControlFlow::WaitUntil(wake_at));
    }
}

/// The ORBIT host, without a scheme.
fn server_host(config: &Config) -> String {
    let server = resolve_server_base(
        config.server_base_url(),
        std::env::var(ENV_SERVER_URL).ok().as_deref(),
    )
    .unwrap_or_else(|_| stardust_core::api::DEFAULT_SERVER_BASE.to_string());
    https_origin(&server)
        .unwrap_or(server)
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_string()
}

/// The keys the settings and sign-in windows act on.
fn window_key(key: &WinitKey, shift: bool) -> Option<Key> {
    Some(match key {
        WinitKey::Named(NamedKey::ArrowUp) => Key::Up,
        WinitKey::Named(NamedKey::ArrowDown) => Key::Down,
        WinitKey::Named(NamedKey::ArrowLeft) => Key::Left,
        WinitKey::Named(NamedKey::ArrowRight) => Key::Right,
        WinitKey::Named(NamedKey::Tab) if shift => Key::BackTab,
        WinitKey::Named(NamedKey::Tab) => Key::Tab,
        WinitKey::Named(NamedKey::Enter | NamedKey::Space) => Key::Activate,
        WinitKey::Named(NamedKey::Escape) => Key::Escape,
        WinitKey::Named(NamedKey::Home) => Key::Home,
        WinitKey::Named(NamedKey::End) => Key::End,
        _ => return None,
    })
}

/// Only the user code goes on the clipboard. A clipboard that cannot be reached
/// (a Wayland session with no X11 bridge, for one) is reported in the window.
fn copy_user_code(clipboard: &mut Option<arboard::Clipboard>, user_code: &str) -> bool {
    if clipboard.is_none() {
        match arboard::Clipboard::new() {
            Ok(opened) => *clipboard = Some(opened),
            Err(err) => {
                eprintln!("stardust: could not reach the clipboard: {err}");
                return false;
            }
        }
    }
    let Some(opened) = clipboard.as_mut() else {
        return false;
    };
    match opened.set_text(user_code) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("stardust: could not copy the code: {err}");
            false
        }
    }
}

fn resize_surface(surface: &mut SbSurface) -> Result<(), String> {
    let size = surface.window().inner_size();
    let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
    else {
        return Ok(());
    };
    surface.resize(width, height).map_err(|err| err.to_string())
}

fn paint_character(surface: &mut SbSurface, skin: PetSkin, pose: PetPose, scale: PetScale) {
    if resize_surface(surface).is_err() {
        return;
    }
    let Ok(mut buffer) = surface.buffer_mut() else {
        return;
    };
    let frame = sprites::render_pet(skin, pose, scale);
    let width = buffer.width().get();
    let height = buffer.height().get();
    for pixel in buffer.iter_mut() {
        *pixel = 0;
    }
    let copy_w = frame.width.min(width);
    let copy_h = frame.height.min(height);
    for y in 0..copy_h {
        for x in 0..copy_w {
            let index = ((y * frame.width + x) * 4) as usize;
            buffer[(y * width + x) as usize] = pack_xrgb([
                frame.pixels[index],
                frame.pixels[index + 1],
                frame.pixels[index + 2],
                frame.pixels[index + 3],
            ]);
        }
    }
    let _ = buffer.present();
}

fn pack_xrgb(px: [u8; 4]) -> u32 {
    if px[3] < 128 {
        0
    } else {
        (u32::from(px[0]) << 16) | (u32::from(px[1]) << 8) | u32::from(px[2])
    }
}

fn paint_bubble(surface: &mut SbSurface, presentation: &Presentation, painter: &Painter) {
    if resize_surface(surface).is_err() {
        return;
    }
    let Ok(mut buffer) = surface.buffer_mut() else {
        return;
    };
    let width = buffer.width().get();
    let height = buffer.height().get();
    for pixel in buffer.iter_mut() {
        *pixel = BUBBLE_COLOR;
    }
    let radius = 8u32.min(width / 4).min(height / 4);
    for y in 0..height {
        for x in 0..width {
            if corner_outside(x, y, width, height, radius) {
                buffer[(y * width + x) as usize] = 0;
            }
        }
    }
    for (row, line) in presentation.summary.lines.iter().take(6).enumerate() {
        let pen = Pen {
            x: font::PAD as i32,
            y: (font::PAD + row as u32 * font::CELL_H * font::SCALE) as i32,
            color: TEXT_COLOR,
            scale: font::SCALE,
        };
        painter.draw(&mut buffer, width, line, pen, font::MAX_COLUMNS);
    }
    let _ = buffer.present();
}

fn corner_outside(x: u32, y: u32, width: u32, height: u32, radius: u32) -> bool {
    let (cx, cy) = if x < radius && y < radius {
        (radius, radius)
    } else if x >= width.saturating_sub(radius) && y < radius {
        (width - radius, radius)
    } else if x < radius && y >= height.saturating_sub(radius) {
        (radius, height - radius)
    } else if x >= width.saturating_sub(radius) && y >= height.saturating_sub(radius) {
        (width - radius, height - radius)
    } else {
        return false;
    };
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    dx * dx + dy * dy > (radius as i32) * (radius as i32)
}

fn open_details(config: &stardust_core::config::Config, url: &str) {
    let Ok(base) = resolve_server_base(
        config.server_base_url(),
        std::env::var(ENV_SERVER_URL).ok().as_deref(),
    ) else {
        return;
    };
    if is_allowed_https_url(&base, url) {
        if let Err(err) = open::that(url) {
            eprintln!("stardust: could not open the workspace: {err}");
        }
    }
}

/// Settings window size for `columns` cells by `rows` lines.
fn settings_size(columns: u32, rows: u32, scale: u32) -> (u32, u32) {
    let pad = font::PAD * scale / 2;
    (
        pad * 2 + columns * font::CELL_W * scale,
        pad * 2 + rows * line_height(scale),
    )
}

fn line_height(scale: u32) -> u32 {
    (font::CELL_H + LINE_GAP) * scale
}

/// Which settings line is under a cursor `y`, in window pixels.
fn line_at(y: f64, scale: u32) -> Option<usize> {
    let pad = f64::from(font::PAD * scale / 2);
    if !y.is_finite() || y < pad {
        return None;
    }
    Some(((y - pad) / f64::from(line_height(scale))) as usize)
}

fn centered(monitor: ScreenRect, width: u32, height: u32) -> ScreenPoint {
    let x = monitor.x + (i64::from(monitor.width) - i64::from(width)).max(0) as i32 / 2;
    let y = monitor.y + (i64::from(monitor.height) - i64::from(height)).max(0) as i32 / 2;
    ScreenPoint { x, y }
}

fn paint_settings(surface: &mut SbSurface, painter: &Painter, lines: &[Line], scale: u32) {
    if resize_surface(surface).is_err() {
        return;
    }
    let Ok(mut buffer) = surface.buffer_mut() else {
        return;
    };
    let width = buffer.width().get();
    let height = buffer.height().get();
    for pixel in buffer.iter_mut() {
        *pixel = SETTINGS_BACKGROUND;
    }
    let pad = font::PAD * scale / 2;
    for (index, line) in lines.iter().enumerate() {
        let top = pad + index as u32 * line_height(scale);
        let color = match (line.kind, line.focused) {
            (_, true) => {
                let fill_top = top.saturating_sub(LINE_GAP * scale / 2);
                let fill_bottom = (fill_top + line_height(scale)).min(height);
                for y in fill_top..fill_bottom {
                    for x in pad / 2..width.saturating_sub(pad / 2) {
                        buffer[(y * width + x) as usize] = SETTINGS_FOCUS_FILL;
                    }
                }
                SETTINGS_FOCUS_TEXT
            }
            (LineKind::Info | LineKind::Hint, false) => SETTINGS_MUTED_TEXT,
            (LineKind::Title | LineKind::Row(_), false) => SETTINGS_TEXT,
        };
        let pen = Pen {
            x: pad as i32,
            y: top as i32,
            color,
            scale,
        };
        painter.draw(&mut buffer, width, &line.text, pen, SETTINGS_COLUMNS);
    }
    let _ = buffer.present();
}

fn paint_sign_in(surface: &mut SbSurface, painter: &Painter, lines: &[SignInLine], scale: u32) {
    if resize_surface(surface).is_err() {
        return;
    }
    let Ok(mut buffer) = surface.buffer_mut() else {
        return;
    };
    let width = buffer.width().get();
    let height = buffer.height().get();
    draw_sign_in(&mut buffer, width, height, painter, lines, scale);
    let _ = buffer.present();
}

/// Draw the sign-in window. Same layout as the settings window, so
/// [`line_at`] maps a click to a line, with the code drawn at twice the scale
/// across its own line and the empty one below it.
fn draw_sign_in(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    painter: &Painter,
    lines: &[SignInLine],
    scale: u32,
) {
    buffer.fill(SETTINGS_BACKGROUND);
    let pad = font::PAD * scale / 2;
    for (index, line) in lines.iter().enumerate() {
        let top = pad + index as u32 * line_height(scale);
        let mut color = match line.kind {
            SignInLineKind::Blank | SignInLineKind::CodeGap => continue,
            SignInLineKind::Code => {
                let large = scale * 2;
                let text_width = font::text_columns(&line.text) * font::CELL_W * large;
                let pen = Pen {
                    x: (width.saturating_sub(text_width) / 2) as i32,
                    y: top as i32,
                    color: SETTINGS_TEXT,
                    scale: large,
                };
                painter.draw_extended(buffer, width, &line.text, pen, SETTINGS_COLUMNS / 2);
                continue;
            }
            SignInLineKind::Hint => SETTINGS_MUTED_TEXT,
            SignInLineKind::Title | SignInLineKind::Info | SignInLineKind::Row(_) => SETTINGS_TEXT,
        };
        if line.focused {
            let fill_top = top.saturating_sub(LINE_GAP * scale / 2);
            let fill_bottom = (fill_top + line_height(scale)).min(height);
            for y in fill_top..fill_bottom {
                for x in pad / 2..width.saturating_sub(pad / 2) {
                    buffer[(y * width + x) as usize] = SETTINGS_FOCUS_FILL;
                }
            }
            color = SETTINGS_FOCUS_TEXT;
        }
        let pen = Pen {
            x: pad as i32,
            y: top as i32,
            color,
            scale,
        };
        painter.draw_extended(buffer, width, &line.text, pen, SETTINGS_COLUMNS);
    }
}

#[cfg(test)]
mod tests {
    use super::{centered, draw_sign_in, line_at, settings_size, LINE_GAP, SETTINGS_COLUMNS};
    use crate::font::{Painter, CELL_H, PAD};
    use stardust_core::a11y::{
        SETTINGS_BACKGROUND, SETTINGS_FOCUS_FILL, SETTINGS_FOCUS_TEXT, SETTINGS_TEXT,
    };
    use stardust_core::connect::IssuedCode;
    use stardust_core::i18n::Language;
    use stardust_core::position::{ScreenPoint, ScreenRect};
    use stardust_core::signin_window::{
        SignInEnd, SignInLineKind, SignInModel, SignInRow, COLUMNS,
    };
    use std::time::{Duration, Instant};

    fn issued() -> IssuedCode {
        IssuedCode {
            user_code: "ABCD-EFGH".to_string(),
            address: Some("helloorbit.app/rust-preview/pqc/account".to_string()),
            open_url: Some(
                "https://helloorbit.app/rust-preview/pqc/account?user_code=ABCD-EFGH".to_string(),
            ),
        }
    }

    fn sign_in_pixels(model: &SignInModel, now: Instant, scale: u32) -> (Vec<u32>, u32, u32) {
        let (width, height) = settings_size(SETTINGS_COLUMNS, model.max_lines() as u32, scale);
        let mut buffer = vec![0_u32; (width * height) as usize];
        let lines = model.lines(now, Language::English);
        draw_sign_in(&mut buffer, width, height, &Painter::new(), &lines, scale);
        (buffer, width, height)
    }

    #[test]
    fn the_sign_in_window_fits_the_columns_the_model_writes_for() {
        assert_eq!(COLUMNS as u32, SETTINGS_COLUMNS);
    }

    #[test]
    fn the_sign_in_window_draws_the_code_large_and_marks_the_focused_row() {
        let t0 = Instant::now();
        let mut model = SignInModel::requesting("helloorbit.app", t0);
        model.on_code(issued(), Duration::from_secs(582), t0);
        let scale = 2;
        let (pixels, width, height) = sign_in_pixels(&model, t0, scale);
        assert!(pixels.contains(&SETTINGS_BACKGROUND));
        assert!(pixels.contains(&SETTINGS_TEXT));
        assert!(pixels.contains(&SETTINGS_FOCUS_FILL));
        assert!(pixels.contains(&SETTINGS_FOCUS_TEXT));

        let lines = model.lines(t0, Language::English);
        let step = (CELL_H + LINE_GAP) * scale;
        let top_of = |index: usize| PAD * scale / 2 + index as u32 * step;
        let row_has_ink =
            |y: u32, color: u32| (0..width).any(|x| pixels[(y * width + x) as usize] == color);

        let code_at = lines
            .iter()
            .position(|line| line.kind == SignInLineKind::Code)
            .unwrap();
        assert_eq!(lines[code_at + 1].kind, SignInLineKind::CodeGap);
        let tallest = (top_of(code_at)..top_of(code_at + 2))
            .filter(|y| row_has_ink(*y, SETTINGS_TEXT))
            .count() as u32;
        assert!(
            tallest > CELL_H * scale,
            "{tallest} px is taller than one line of text"
        );

        let focused_at = lines.iter().position(|line| line.focused).unwrap();
        assert_eq!(lines[focused_at].kind, SignInLineKind::Row(SignInRow::Open));
        assert!(row_has_ink(top_of(focused_at), SETTINGS_FOCUS_FILL));
        let last_line = top_of(lines.len() - 1) + step;
        assert!(last_line <= height, "{last_line} fits in {height}");
    }

    #[test]
    fn a_click_on_a_drawn_row_maps_back_to_that_row() {
        let t0 = Instant::now();
        let after = t0 + Duration::from_secs(1);
        let mut model = SignInModel::requesting("helloorbit.app", t0);
        model.on_code(issued(), Duration::from_secs(582), t0);
        let scale = 3;
        let lines = model.lines(after, Language::English);
        let cancel_at = lines
            .iter()
            .position(|line| line.kind == SignInLineKind::Row(SignInRow::Cancel))
            .unwrap();
        let y = f64::from(PAD * scale / 2 + cancel_at as u32 * (CELL_H + LINE_GAP) * scale) + 1.0;
        assert_eq!(line_at(y, scale), Some(cancel_at));
        assert_eq!(
            model.click(cancel_at, after),
            stardust_core::signin_window::SignInAction::Close
        );
    }

    /// Writes what each state looks like, to look at by eye. Nothing else in
    /// the test suite can see the drawing. Run it with
    /// `cargo test preview_sign_in_window -- --ignored`.
    #[test]
    #[ignore = "writes PNG files for a manual look"]
    fn preview_sign_in_window() {
        let dir = std::env::temp_dir().join("stardust-sign-in-preview");
        std::fs::create_dir_all(&dir).unwrap();
        let t0 = Instant::now();
        let mut waiting = SignInModel::requesting("helloorbit.app", t0);
        waiting.on_code(issued(), Duration::from_secs(582), t0);
        let mut reconnecting = waiting.clone();
        reconnecting.on_connection("ABCD-EFGH", false);
        let mut denied = waiting.clone();
        denied.on_end(SignInEnd::Denied, t0);
        let mut keychain_failed = waiting.clone();
        keychain_failed.on_end(SignInEnd::KeychainFailed, t0);
        let mut signed_in = waiting.clone();
        signed_in.on_signed_in(t0);
        let states = [
            ("requesting", SignInModel::requesting("helloorbit.app", t0)),
            ("waiting", waiting),
            ("reconnecting", reconnecting),
            ("denied", denied),
            ("keychain-failed", keychain_failed),
            ("signed-in", signed_in),
        ];
        for (name, model) in states {
            let (pixels, width, height) = sign_in_pixels(&model, t0, 2);
            let rgba: Vec<u8> = pixels
                .iter()
                .flat_map(|pixel| {
                    let [_, red, green, blue] = pixel.to_be_bytes();
                    [red, green, blue, 255]
                })
                .collect();
            let file = std::fs::File::create(dir.join(format!("{name}.png"))).unwrap();
            let mut encoder = png::Encoder::new(file, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&rgba).unwrap();
        }
    }

    /// This is why the app builds one loop for the whole process. winit
    /// checks its once-per-process guard before it touches the display, so
    /// this holds on a machine without one. Test threads are not the main
    /// thread, so the X11 `any_thread` switch is used; that switch exists on
    /// Linux and the BSDs only.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_second_event_loop_is_refused_by_winit() {
        use winit::platform::x11::EventLoopBuilderExtX11;
        let first = winit::event_loop::EventLoop::<()>::with_user_event()
            .with_any_thread(true)
            .build();
        let second = winit::event_loop::EventLoop::<()>::with_user_event()
            .with_any_thread(true)
            .build();
        assert!(matches!(
            second,
            Err(winit::error::EventLoopError::RecreationAttempt)
        ));
        drop(first);
    }

    #[test]
    fn settings_lines_map_back_from_the_cursor() {
        let scale = 2;
        let (width, height) = settings_size(52, 12, scale);
        assert!(width > 0 && height > 0);
        let pad = f64::from(PAD * scale / 2);
        let step = f64::from((CELL_H + LINE_GAP) * scale);
        assert_eq!(line_at(pad - 1.0, scale), None);
        assert_eq!(line_at(pad, scale), Some(0));
        assert_eq!(line_at(pad + step * 3.0 + 1.0, scale), Some(3));
        assert_eq!(line_at(f64::NAN, scale), None);
    }

    #[test]
    fn settings_window_is_centered_and_stays_on_a_small_monitor() {
        let monitor = ScreenRect {
            x: 100,
            y: 50,
            width: 1000,
            height: 600,
        };
        assert_eq!(centered(monitor, 400, 200), ScreenPoint { x: 400, y: 250 });
        assert_eq!(centered(monitor, 2000, 900), ScreenPoint { x: 100, y: 50 });
    }
}
