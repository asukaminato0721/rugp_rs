//! A top-level dialog window drawn by egui on the CPU, as the retail property
//! sheets and the save browser are separate Win32 dialogs.
use super::egui_raster::Raster;
use anyhow::{Context, Result};
use std::{
    num::NonZeroU32,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::{
    cursor::CursorIcon,
    dpi::{LogicalSize, PhysicalPosition},
    event::{
        ButtonSource, ElementState, Ime, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
    },
    event_loop::ActiveEventLoop,
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowAttributes, WindowId, WindowLevel},
};

pub(super) struct EguiWindow {
    window: Arc<dyn Window>,
    surface: softbuffer::Surface<Arc<dyn Window>, Arc<dyn Window>>,
    context: egui::Context,
    raster: Raster,
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
    focused: bool,
    started: Instant,
    repaint: Option<Instant>,
    cursor: egui::CursorIcon,
    ime: bool,
}
impl EguiWindow {
    pub fn open(
        event_loop: &dyn ActiveEventLoop,
        title: &str,
        size: (f64, f64),
        face: (&[u8], u32),
    ) -> Result<Self> {
        let window: Arc<dyn Window> = Arc::from(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title(title)
                        .with_surface_size(LogicalSize::new(size.0, size.1))
                        .with_resizable(false)
                        .with_window_level(WindowLevel::AlwaysOnTop),
                )
                .with_context(|| format!("create {title} window"))?,
        );
        let display = softbuffer::Context::new(Arc::clone(&window))
            .map_err(|e| anyhow::anyhow!("create {title} display context: {e}"))?;
        let surface = softbuffer::Surface::new(&display, Arc::clone(&window))
            .map_err(|e| anyhow::anyhow!("create {title} surface: {e}"))?;
        window.request_redraw();
        Ok(Self {
            window,
            surface,
            context: context(face),
            raster: Raster::default(),
            events: Vec::new(),
            modifiers: egui::Modifiers::default(),
            focused: true,
            started: Instant::now(),
            repaint: None,
            cursor: egui::CursorIcon::Default,
            ime: false,
        })
    }
    pub fn id(&self) -> WindowId {
        self.window.id()
    }
    pub fn focus(&self) {
        self.window.focus_window();
    }
    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }
    /// Serve egui's delayed repaint requests, such as animations and tooltips.
    pub fn wake(&self, now: Instant) {
        if self.repaint.is_some_and(|at| now >= at) {
            self.window.request_redraw();
        }
    }
    /// Translate one window event into egui input; true when the window's
    /// close button was pressed. Redraws go through [`Self::redraw`].
    pub fn event(&mut self, event: &WindowEvent) -> bool {
        let scale = self.window.scale_factor() as f32;
        let point = |p: &PhysicalPosition<f64>| egui::pos2(p.x as f32 / scale, p.y as f32 / scale);
        let input = match event {
            WindowEvent::CloseRequested => return true,
            WindowEvent::Focused(focused) => {
                self.focused = *focused;
                Some(egui::Event::WindowFocused(*focused))
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                let mac = cfg!(target_os = "macos");
                self.modifiers = egui::Modifiers {
                    alt: state.alt_key(),
                    ctrl: state.control_key(),
                    shift: state.shift_key(),
                    mac_cmd: mac && state.meta_key(),
                    command: if mac {
                        state.meta_key()
                    } else {
                        state.control_key()
                    },
                };
                Some(egui::Event::ModifiersChanged(self.modifiers))
            }
            WindowEvent::PointerMoved { position, .. } => {
                Some(egui::Event::PointerMoved(point(position)))
            }
            WindowEvent::PointerLeft { .. } => Some(egui::Event::PointerGone),
            WindowEvent::PointerButton {
                state,
                position,
                button,
                ..
            } => pointer_button(button).map(|button| egui::Event::PointerButton {
                pos: point(position),
                button,
                pressed: *state == ElementState::Pressed,
                modifiers: self.modifiers,
            }),
            WindowEvent::MouseWheel { delta, phase, .. } => {
                let (unit, delta) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        (egui::MouseWheelUnit::Line, egui::vec2(*x, *y))
                    }
                    MouseScrollDelta::PixelDelta(p) => (
                        egui::MouseWheelUnit::Point,
                        egui::vec2(p.x as f32 / scale, p.y as f32 / scale),
                    ),
                    _ => return false,
                };
                Some(egui::Event::MouseWheel {
                    unit,
                    delta,
                    phase: match phase {
                        TouchPhase::Started => egui::TouchPhase::Start,
                        TouchPhase::Moved => egui::TouchPhase::Move,
                        TouchPhase::Ended => egui::TouchPhase::End,
                        TouchPhase::Cancelled => egui::TouchPhase::Cancel,
                    },
                    source: egui::MouseWheelSource::Unknown,
                    modifiers: self.modifiers,
                })
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                if let PhysicalKey::Code(code) = event.physical_key
                    && let Some(key) = key(code)
                {
                    self.events.push(egui::Event::Key {
                        key,
                        physical_key: Some(key),
                        pressed,
                        repeat: event.repeat,
                        modifiers: self.modifiers,
                    });
                }
                event
                    .text
                    .as_ref()
                    .filter(|text| {
                        pressed
                            && !self.modifiers.command
                            && !text.is_empty()
                            && !text.chars().any(char::is_control)
                    })
                    .map(|text| egui::Event::Text(text.to_string()))
            }
            // Comments are Japanese; composition comes through the IME.
            WindowEvent::Ime(Ime::Preedit(text, _)) => {
                Some(egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: text.clone(),
                    active_range_chars: None,
                }))
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                Some(egui::Event::Ime(egui::ImeEvent::Commit(text.clone())))
            }
            WindowEvent::SurfaceResized(_) | WindowEvent::ScaleFactorChanged { .. } => None,
            _ => return false,
        };
        self.events.extend(input);
        self.window.request_redraw();
        false
    }
    /// Lay out and paint one frame with `ui`.
    pub fn redraw(&mut self, mut ui: impl FnMut(&mut egui::Ui)) -> Result<()> {
        let size = self.window.surface_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return Ok(());
        };
        let scale = self.window.scale_factor() as f32;
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size.width as f32, size.height as f32) / scale,
            )),
            time: Some(self.started.elapsed().as_secs_f64()),
            events: std::mem::take(&mut self.events),
            focused: self.focused,
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(scale);
        let context = self.context.clone();
        let mut output = context.run_ui(input, &mut ui);
        self.raster.set_textures(&output.textures_delta);
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);
        self.surface
            .resize(width, height)
            .map_err(|e| anyhow::anyhow!("resize dialog surface: {e}"))?;
        let mut buffer = self
            .surface
            .buffer_mut()
            .map_err(|e| anyhow::anyhow!("acquire dialog buffer: {e}"))?;
        buffer.fill(0);
        self.raster.paint(
            &mut buffer,
            (size.width as usize, size.height as usize),
            output.pixels_per_point,
            &primitives,
        );
        self.window.pre_present_notify();
        buffer
            .present()
            .map_err(|e| anyhow::anyhow!("present dialog window: {e}"))?;
        self.raster.free_textures(&output.textures_delta);
        output.textures_delta.clear();
        self.repaint = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .and_then(|viewport| Instant::now().checked_add(viewport.repaint_delay));
        if self
            .repaint
            .is_some_and(|at| at <= Instant::now() + Duration::from_millis(1))
        {
            self.window.request_redraw();
        }
        let icon = output.platform_output.cursor_icon;
        if icon != self.cursor {
            self.cursor = icon;
            self.window.set_cursor(cursor(icon).into());
        }
        let ime = output.platform_output.ime.is_some();
        if ime != self.ime {
            self.ime = ime;
            #[allow(deprecated)]
            self.window.set_ime_allowed(ime);
        }
        Ok(())
    }
}

pub(super) fn context(face: (&[u8], u32)) -> egui::Context {
    let context = egui::Context::default();
    // egui's bundled faces have no kana or kanji; fall back to the game's face.
    let mut fonts = egui::FontDefinitions::default();
    let mut data = egui::FontData::from_owned(face.0.to_vec());
    data.index = face.1;
    fonts.font_data.insert("game".into(), Arc::new(data));
    for family in fonts.families.values_mut() {
        family.push("game".into());
    }
    context.set_fonts(fonts);
    context.set_theme(egui::Theme::Light);
    context.set_visuals_of(egui::Theme::Light, classic_visuals());
    context
}

/// Windows classic 3D face colours, matching the retail dialogs.
fn classic_visuals() -> egui::Visuals {
    let face = egui::Color32::from_rgb(212, 208, 200);
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = face;
    visuals.window_fill = face;
    visuals.extreme_bg_color = egui::Color32::WHITE;
    visuals.override_text_color = Some(egui::Color32::BLACK);
    visuals.selection.bg_fill = egui::Color32::from_rgb(10, 36, 106);
    visuals.selection.stroke.color = egui::Color32::WHITE;
    visuals
}

fn pointer_button(button: &ButtonSource) -> Option<egui::PointerButton> {
    Some(match button.clone().mouse_button()? {
        MouseButton::Left => egui::PointerButton::Primary,
        MouseButton::Right => egui::PointerButton::Secondary,
        MouseButton::Middle => egui::PointerButton::Middle,
        MouseButton::Back => egui::PointerButton::Extra1,
        MouseButton::Forward => egui::PointerButton::Extra2,
        _ => return None,
    })
}

fn key(code: KeyCode) -> Option<egui::Key> {
    use egui::Key;
    Some(match code {
        KeyCode::Escape => Key::Escape,
        KeyCode::Tab => Key::Tab,
        KeyCode::Enter | KeyCode::NumpadEnter => Key::Enter,
        KeyCode::Space => Key::Space,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::ArrowUp => Key::ArrowUp,
        KeyCode::ArrowDown => Key::ArrowDown,
        KeyCode::ArrowLeft => Key::ArrowLeft,
        KeyCode::ArrowRight => Key::ArrowRight,
        KeyCode::Minus | KeyCode::NumpadSubtract => Key::Minus,
        KeyCode::Digit0 | KeyCode::Numpad0 => Key::Num0,
        KeyCode::Digit1 | KeyCode::Numpad1 => Key::Num1,
        KeyCode::Digit2 | KeyCode::Numpad2 => Key::Num2,
        KeyCode::Digit3 | KeyCode::Numpad3 => Key::Num3,
        KeyCode::Digit4 | KeyCode::Numpad4 => Key::Num4,
        KeyCode::Digit5 | KeyCode::Numpad5 => Key::Num5,
        KeyCode::Digit6 | KeyCode::Numpad6 => Key::Num6,
        KeyCode::Digit7 | KeyCode::Numpad7 => Key::Num7,
        KeyCode::Digit8 | KeyCode::Numpad8 => Key::Num8,
        KeyCode::Digit9 | KeyCode::Numpad9 => Key::Num9,
        KeyCode::KeyA => Key::A,
        KeyCode::KeyC => Key::C,
        KeyCode::KeyD => Key::D,
        KeyCode::KeyN => Key::N,
        KeyCode::KeyP => Key::P,
        KeyCode::KeyV => Key::V,
        KeyCode::KeyX => Key::X,
        KeyCode::KeyY => Key::Y,
        _ => return None,
    })
}

fn cursor(icon: egui::CursorIcon) -> CursorIcon {
    match icon {
        egui::CursorIcon::PointingHand => CursorIcon::Pointer,
        egui::CursorIcon::Text => CursorIcon::Text,
        egui::CursorIcon::ResizeHorizontal => CursorIcon::EwResize,
        egui::CursorIcon::ResizeVertical => CursorIcon::NsResize,
        egui::CursorIcon::Grab => CursorIcon::Grab,
        egui::CursorIcon::Grabbing => CursorIcon::Grabbing,
        egui::CursorIcon::NotAllowed | egui::CursorIcon::NoDrop => CursorIcon::NotAllowed,
        _ => CursorIcon::Default,
    }
}
