use crate::actions::{apply_menu, MenuEffect};
use crate::font::{self, draw_lines};
use crate::inbox::{Inbox, UiMsg, Wake};
use crate::live::LivePet;
use crate::sprites;
use crate::tray::Tray;
use chrono::Utc;
use stardust_core::config::{save_config, Config};
use stardust_core::pet::{pet_frame, pet_pose, tray_glyph, PetPose, PetScale, PetSkin};
use stardust_core::policy::Presentation;
use stardust_core::position::{layout_bubble, LayoutRequest, ScreenPoint, ScreenRect, WindowSize};
use stardust_core::text::menu_text;
use std::cell::RefCell;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId, WindowLevel};

const BUBBLE_COLOR: u32 = 0x001C_2430;
const TEXT_COLOR: u32 = 0x00F4_F1EA;

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
    monitors: Vec<ScreenRect>,
    drag: Option<Drag>,
    failure: Option<String>,
}

impl Drop for PopupApp {
    fn drop(&mut self) {
        self.session.inbox.clear_proxy();
    }
}

pub fn run_session(session: Session) -> Result<(), String> {
    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .map_err(|err| err.to_string())?;
    let context = softbuffer::Context::new(event_loop.owned_display_handle())
        .map_err(|err| err.to_string())?;
    session.inbox.set_proxy(event_loop.create_proxy());
    let mut app = PopupApp {
        session,
        context,
        character: None,
        bubble: None,
        monitors: Vec::new(),
        drag: None,
        failure: None,
    };
    let ran = event_loop.run_app(&mut app).map_err(|err| err.to_string());
    match (ran, app.failure.take()) {
        (Err(err), _) => Err(err),
        (Ok(()), Some(err)) => Err(err),
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
        let window = event_loop
            .create_window(attributes)
            .map_err(|err| err.to_string())?;
        let mut surface =
            softbuffer::Surface::new(&self.context, window).map_err(|err| err.to_string())?;
        resize_surface(&mut surface)?;
        Ok(surface)
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
                    self.sync_character(event_loop);
                    self.redraw_character();
                    self.refresh_tray();
                }
            }
        }
    }

    fn on_menu(&mut self, event_loop: &ActiveEventLoop, id: &str) {
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
            }
        }
    }

    fn refresh_tray(&self) {
        let observed = self.session.live.observed();
        let scale = self.session.live.scale_factor();
        let (text, glyph) = {
            let config = self
                .session
                .config
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            let local = Utc::now().with_timezone(&config.schedule().timezone());
            let pose = pet_pose(observed, config.schedule().is_quiet_at(local));
            (menu_text(&config, Utc::now()), tray_glyph(pose))
        };
        if let Some(tray) = self.session.tray.borrow().as_ref() {
            if let Err(err) = tray.apply(&text, glyph, scale) {
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
        self.sync_character(event_loop);
        self.drain(event_loop);
        self.refresh_tray();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: Wake) {
        self.drain(event_loop);
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
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
                        paint_bubble(&mut bubble.surface, &bubble.presentation);
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
                        open_details(&bubble.presentation.summary.details_url);
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
        if self.character.is_none() && self.bubble.is_none() {
            event_loop.exit();
            return;
        }
        if let Some(bubble) = &self.bubble {
            event_loop.set_control_flow(ControlFlow::WaitUntil(bubble.deadline));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
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

pub fn detect_scale_factor() -> f64 {
    struct Probe {
        scale: f64,
    }
    impl Probe {
        fn capture(&mut self, event_loop: &ActiveEventLoop) {
            let mut scale = 0.0f64;
            for monitor in event_loop.available_monitors() {
                let factor = monitor.scale_factor();
                if factor.is_finite() {
                    scale = scale.max(factor);
                }
            }
            if scale > 0.0 {
                self.scale = scale;
            }
            event_loop.exit();
        }
    }
    impl ApplicationHandler for Probe {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            self.capture(event_loop);
        }

        fn window_event(
            &mut self,
            _event_loop: &ActiveEventLoop,
            _id: WindowId,
            _event: WindowEvent,
        ) {
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            self.capture(event_loop);
        }
    }
    let Ok(event_loop) = EventLoop::new() else {
        return 1.0;
    };
    let mut probe = Probe { scale: 1.0 };
    if event_loop.run_app(&mut probe).is_err() {
        return 1.0;
    }
    if probe.scale.is_finite() && probe.scale > 0.0 {
        probe.scale
    } else {
        1.0
    }
}

fn paint_bubble(surface: &mut SbSurface, presentation: &Presentation) {
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
    draw_lines(&mut buffer, width, &presentation.summary.lines, TEXT_COLOR);
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

fn open_details(url: &str) {
    if url.starts_with("https://") || url.starts_with("http://") {
        if let Err(err) = open::that(url) {
            eprintln!("stardust: could not open details: {err}");
        }
    }
}
