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
use stardust_core::delivery::Outcome;
use stardust_core::i18n::Language;
use stardust_core::pet::{
    pet_frame, pet_pose, tray_glyph, ObservedCheck, PetPose, PetScale, PetSkin,
};
use stardust_core::policy::Presentation;
use stardust_core::position::{layout_bubble, LayoutRequest, ScreenPoint, ScreenRect, WindowSize};
use stardust_core::settings::{
    panel_view, Key, Line, LineKind, SettingsAction, SettingsModel, SettingsView, SignInStage,
    PANEL_LINES,
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
const SETTINGS_COLUMNS: u32 = stardust_core::settings::SETTINGS_COLUMNS as u32;

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
    /// Clicks before this are dropped: a click just changed the page, and the
    /// second click of a double-click would land on the new one.
    clicks_after: Option<Instant>,
}

/// How long after a click changes the page another click is dropped. About
/// the usual double-click interval.
const PAGE_CHANGE_CLICK_GUARD: Duration = Duration::from_millis(500);

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
    let signed_in = session.demo || crate::signin::signed_in();
    session.live.set_signed_in(signed_in);
    let mut app = PopupApp {
        session,
        context,
        character: None,
        bubble: None,
        settings: None,
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
        let server = resolve_server_base(
            config.server_base_url(),
            std::env::var(ENV_SERVER_URL).ok().as_deref(),
        )
        .unwrap_or_else(|_| stardust_core::api::DEFAULT_SERVER_BASE.to_string());
        let host = https_origin(&server)
            .unwrap_or(server)
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_string();
        SettingsView {
            signed_in: self.signed_in,
            sign_in: self.sign_in.clone(),
            interval_minutes: config.schedule().interval_minutes(),
            start_at_login: self.start_at_login,
            start_at_login_available: crate::autostart::available(),
            character_visible: config.character_visible(),
            skin: config.pet_skin(),
            scale: config.pet_scale(),
            server_host: host,
            panel: config.features().integrations_panel.then(|| {
                panel_view(
                    config.integrations(),
                    &self.session.live.delivery(),
                    config.effective_language(),
                )
            }),
        }
    }

    fn open_settings(&mut self, event_loop: &ActiveEventLoop, first_run: bool) {
        self.session
            .config
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .adopt_saved_integrations(&self.session.config_path);
        if let Some(settings) = self.settings.as_ref() {
            settings.surface.window().focus_window();
            settings.surface.window().request_redraw();
            return;
        }
        let model = SettingsModel::new(first_run);
        let view = self.settings_view();
        let language = self.language();
        let mut rows = model.lines(&view, language).len() as u32;
        if view.panel.is_some() {
            // The window is not resized while open, so it starts with room
            // for the tallest integrations page.
            rows = rows.max(PANEL_LINES as u32);
        }
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
                    clicks_after: None,
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
            config.adopt_saved_integrations(&self.session.config_path);
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

    /// The tray's integrations line. With the panel off it is only a label,
    /// so a click cannot arrive here, and if one did it would do nothing.
    fn open_integrations(&mut self, event_loop: &ActiveEventLoop) {
        let panel_on = self
            .session
            .config
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .features()
            .integrations_panel;
        if !panel_on {
            return;
        }
        self.open_settings(event_loop, false);
        if let Some(settings) = self.settings.as_mut() {
            settings.model.open_panel();
        }
        self.redraw_settings();
    }

    /// A new delivery result, for the integrations page if it is showing.
    fn redraw_panel(&self) {
        if self
            .settings
            .as_ref()
            .is_some_and(|settings| settings.model.in_panel())
        {
            self.redraw_settings();
        }
    }

    /// `accepted_host` is the host the consent page showed, when this follows
    /// one. The file is read again first, so if `stardust integration url`
    /// moved the destination since, nothing is recorded or turned on.
    fn set_integration(&mut self, id: &str, on: bool, accepted_host: Option<&str>) {
        self.update_config(|config| {
            let changed = match accepted_host {
                Some(host) => config.accept_integration_consent_for(id, host),
                None => Ok(()),
            }
            .and_then(|()| config.set_integration_enabled(id, on));
            if let Err(err) = changed {
                eprintln!("stardust: could not change integration {id}: {err}");
            }
        });
        if on {
            // A note such as "destination gone" no longer says anything.
            self.session.live.clear_delivery(id);
        }
        self.refresh_tray();
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
        let key = match &event.logical_key {
            WinitKey::Named(NamedKey::ArrowUp) => Key::Up,
            WinitKey::Named(NamedKey::ArrowDown) => Key::Down,
            WinitKey::Named(NamedKey::ArrowLeft) => Key::Left,
            WinitKey::Named(NamedKey::ArrowRight) => Key::Right,
            WinitKey::Named(NamedKey::Tab) if self.shift => Key::BackTab,
            WinitKey::Named(NamedKey::Tab) => Key::Tab,
            WinitKey::Named(NamedKey::Enter | NamedKey::Space) => Key::Activate,
            WinitKey::Named(NamedKey::Escape) => Key::Escape,
            WinitKey::Named(NamedKey::Home) => Key::Home,
            WinitKey::Named(NamedKey::End) => Key::End,
            _ => return,
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
        let now = Instant::now();
        if settings.clicks_after.is_some_and(|after| now < after) {
            return;
        }
        let before = settings.model.page_marker();
        let action = settings.model.click(index, &view);
        if settings.model.page_marker() != before {
            settings.clicks_after = Some(now + PAGE_CHANGE_CLICK_GUARD);
        }
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
            SettingsAction::StartSignIn => self.start_sign_in(),
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
            SettingsAction::SetIntegration { id, on } => self.set_integration(&id, on, None),
            SettingsAction::AcceptConsent { id, host } => {
                self.set_integration(&id, true, Some(&host));
            }
            SettingsAction::SendTest { id } => {
                // Posted from the check timer, like every alert.
                self.session.live.record_delivery(&id, Outcome::TestSending);
                self.session.live.request_test(&id);
                self.session.wake.notify_one();
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
        config.adopt_saved_integrations(&self.session.config_path);
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
        }
        if self.auto_sign_in() {
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
                    self.redraw_panel();
                }
                UiMsg::Pet => {
                    if self.session.live.observed() == ObservedCheck::SignedOut {
                        self.signed_in = false;
                    }
                    self.sync_character(event_loop);
                    self.redraw_character();
                    self.refresh_tray();
                    self.redraw_panel();
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
            "sign-in" => {
                self.open_settings(event_loop, false);
                if !self.signed_in {
                    self.start_sign_in();
                }
                self.redraw_settings();
                return;
            }
            "integrations" => {
                self.open_integrations(event_loop);
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
            config.adopt_saved_integrations(&self.session.config_path);
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
        config.adopt_saved_integrations(&self.session.config_path);
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
            if !onboarded {
                self.open_settings(event_loop, true);
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
                    config.adopt_saved_integrations(&self.session.config_path);
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
        event_loop.set_control_flow(ControlFlow::WaitUntil(wake_at));
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
            (LineKind::Title | LineKind::Row(_) | LineKind::Panel(_), false) => SETTINGS_TEXT,
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

#[cfg(test)]
mod tests {
    use super::{centered, line_at, settings_size, LINE_GAP};
    use crate::font::{CELL_H, PAD};
    use stardust_core::position::{ScreenPoint, ScreenRect};

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
