//! Popup 設定 property sheet after the retail Vm60.dll dialogs (resources
//! 5505 サウンド, 5506 テキスト表示), drawn by egui into its own window on the CPU.
//! The menu bar's 画面サイズ items get a page, since the port has no menu bar.
//! As in the original, edits stay staged until OK or 適用.
use super::{egui_raster::Raster, preferences::Preferences};
use anyhow::{Context, Result};
use std::{
    num::NonZeroU32,
    ops::RangeInclusive,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::{
    cursor::CursorIcon,
    dpi::{LogicalSize, PhysicalPosition},
    event::{ButtonSource, ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent},
    event_loop::ActiveEventLoop,
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowAttributes, WindowId, WindowLevel},
};

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Action {
    /// OK or 適用 with changes: apply and persist these.
    pub apply: Option<Preferences>,
    pub close: bool,
}
impl Action {
    fn close() -> Self {
        Self {
            apply: None,
            close: true,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Screen,
    Text,
    Sound,
}

pub(super) struct ConfigWindow {
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
    sheet: Sheet,
}
impl ConfigWindow {
    pub fn open(
        event_loop: &dyn ActiveEventLoop,
        settings: Preferences,
        face: (&[u8], u32),
    ) -> Result<Self> {
        let window: Arc<dyn Window> = Arc::from(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title("設定")
                        .with_surface_size(LogicalSize::new(440.0, 372.0))
                        .with_resizable(false)
                        .with_window_level(WindowLevel::AlwaysOnTop),
                )
                .context("create settings window")?,
        );
        let display = softbuffer::Context::new(Arc::clone(&window))
            .map_err(|e| anyhow::anyhow!("create settings display context: {e}"))?;
        let surface = softbuffer::Surface::new(&display, Arc::clone(&window))
            .map_err(|e| anyhow::anyhow!("create settings surface: {e}"))?;
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
            sheet: Sheet::new(settings),
        })
    }
    pub fn id(&self) -> WindowId {
        self.window.id()
    }
    pub fn focus(&self) {
        self.window.focus_window();
    }
    pub fn set_notice(&mut self, notice: String) {
        self.sheet.notice = notice;
        self.window.request_redraw();
    }
    /// Serve egui's delayed repaint requests, such as animations and tooltips.
    pub fn wake(&self, now: Instant) {
        if self.repaint.is_some_and(|at| now >= at) {
            self.window.request_redraw();
        }
    }
    /// Translate one window event into egui input. Redraws go through [`Self::redraw`].
    pub fn event(&mut self, event: &WindowEvent) -> Action {
        let scale = self.window.scale_factor() as f32;
        let point = |p: &PhysicalPosition<f64>| egui::pos2(p.x as f32 / scale, p.y as f32 / scale);
        let input = match event {
            WindowEvent::CloseRequested => return Action::close(),
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
                    _ => return Action::default(),
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
            WindowEvent::SurfaceResized(_) | WindowEvent::ScaleFactorChanged { .. } => None,
            _ => return Action::default(),
        };
        self.events.extend(input);
        self.window.request_redraw();
        Action::default()
    }
    pub fn redraw(&mut self) -> Result<Action> {
        let size = self.window.surface_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return Ok(Action::default());
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
        let mut action = Action::default();
        let mut output = context.run_ui(input, |ui| action = self.sheet.ui(ui));
        self.raster.set_textures(&output.textures_delta);
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);
        self.surface
            .resize(width, height)
            .map_err(|e| anyhow::anyhow!("resize settings surface: {e}"))?;
        let mut buffer = self
            .surface
            .buffer_mut()
            .map_err(|e| anyhow::anyhow!("acquire settings buffer: {e}"))?;
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
            .map_err(|e| anyhow::anyhow!("present settings window: {e}"))?;
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
        Ok(action)
    }
}

fn context(face: (&[u8], u32)) -> egui::Context {
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

struct Sheet {
    page: Page,
    /// What the host last applied; キャンセル returns here.
    applied: Preferences,
    settings: Preferences,
    notice: String,
}
impl Sheet {
    fn new(settings: Preferences) -> Self {
        Self {
            page: Page::Screen,
            applied: settings.clone(),
            settings,
            notice: String::new(),
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui) -> Action {
        let mut action = Action::default();
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                for (page, label) in [
                    (Page::Screen, "画面サイズ"),
                    (Page::Text, "テキスト表示"),
                    (Page::Sound, "サウンド"),
                ] {
                    ui.selectable_value(&mut self.page, page, label);
                }
            });
            ui.separator();
            let settings = &mut self.settings;
            let height = ui.available_height() - 40.0;
            ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                ui.set_min_height(height);
                match self.page {
                    Page::Screen => screen_page(ui, settings),
                    Page::Text => text_page(ui, settings),
                    Page::Sound => sound_page(ui, settings),
                }
            });
            if !self.notice.is_empty() {
                ui.colored_label(ui.visuals().error_fg_color, &self.notice);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
                let size = egui::vec2(75.0, 23.0);
                let changed = self.settings != self.applied;
                if ui
                    .add_enabled(changed, egui::Button::new("適用").min_size(size))
                    .clicked()
                {
                    action.apply = Some(self.settings.clone());
                }
                if ui
                    .add(egui::Button::new("キャンセル").min_size(size))
                    .clicked()
                {
                    action.close = true;
                }
                let editing = ui.ctx().egui_wants_keyboard_input();
                if ui.add(egui::Button::new("OK").min_size(size)).clicked()
                    || (!editing && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    action.apply = changed.then(|| self.settings.clone());
                    action.close = true;
                }
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                action = Action::close();
            }
        });
        if let Some(settings) = &action.apply {
            self.applied = settings.clone();
        }
        action
    }
}

fn group(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.label(title);
    ui.group(|ui| {
        ui.set_min_width(ui.available_width());
        add_contents(ui);
    });
    ui.add_space(6.0);
}

/// Retail trackbars carry end captions rather than numbers.
fn trackbar<T: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    enabled: bool,
    value: &mut T,
    range: RangeInclusive<T>,
    step: f64,
    captions: [&str; 2],
) {
    ui.label(captions[0]);
    ui.add_enabled(
        enabled,
        egui::Slider::new(value, range)
            .step_by(step)
            .show_value(false),
    );
    ui.label(captions[1]);
}

fn screen_page(ui: &mut egui::Ui, settings: &mut Preferences) {
    group(ui, "画面モード", |ui| {
        ui.radio_value(&mut settings.fullscreen, true, "フルスクリーン");
        ui.radio_value(&mut settings.fullscreen, false, "ウインドウ");
    });
    group(ui, "ウインドウサイズ", |ui| {
        ui.add_enabled_ui(!settings.fullscreen, |ui| {
            egui::Grid::new("scale").num_columns(3).show(ui, |ui| {
                for (i, scale) in (50..=250).step_by(25).enumerate() {
                    ui.radio_value(&mut settings.window_scale, scale, format!("{scale}%"));
                    if i % 3 == 2 {
                        ui.end_row();
                    }
                }
            });
        });
    });
}

fn text_page(ui: &mut egui::Ui, settings: &mut Preferences) {
    egui::Grid::new("text")
        .num_columns(4)
        .spacing([8.0, 14.0])
        .show(ui, |ui| {
            ui.label("テキスト速度");
            trackbar(
                ui,
                true,
                &mut settings.text_speed_ms,
                RangeInclusive::new(100, 0),
                5.0,
                ["遅い", "速い"],
            );
            ui.end_row();
            ui.label("自動送り改ページ速度");
            trackbar(
                ui,
                true,
                &mut settings.auto_delay_ms,
                RangeInclusive::new(10000, 250),
                250.0,
                ["遅い", "速い"],
            );
            ui.end_row();
        });
}

fn sound_page(ui: &mut egui::Ui, settings: &mut Preferences) {
    group(ui, "音源別設定", |ui| {
        egui::Grid::new("sound")
            .num_columns(4)
            .spacing([8.0, 10.0])
            .show(ui, |ui| {
                ui.label("全体音量");
                trackbar(
                    ui,
                    true,
                    &mut settings.levels[0],
                    0..=100,
                    1.0,
                    ["最小", "最大"],
                );
                ui.end_row();
                // Retail order: BGM, 効果音, 音声. Levels are master, BGM, voice, effect.
                for (label, channel) in [("BGM", 0), ("効果音", 2), ("音声", 1)] {
                    ui.checkbox(&mut settings.channels[channel], label);
                    let enabled = settings.channels[channel];
                    let level = &mut settings.levels[channel + 1];
                    trackbar(ui, enabled, level, 0..=100, 1.0, ["最小", "最大"]);
                    ui.end_row();
                }
            });
    });
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
        KeyCode::KeyV => Key::V,
        KeyCode::KeyX => Key::X,
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

#[cfg(test)]
mod tests {
    use super::*;
    fn press(context: &egui::Context, sheet: &mut Sheet, key: egui::Key) -> Action {
        let event = |pressed| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let input = egui::RawInput {
            events: vec![event(true), event(false)],
            ..Default::default()
        };
        let mut action = Action::default();
        let mut output = context.run_ui(input, |ui| action = sheet.ui(ui));
        output.textures_delta.clear();
        action
    }
    #[test]
    fn edits_stay_staged_until_ok_and_escape_discards_them() {
        let context = egui::Context::default();
        let mut sheet = Sheet::new(Preferences::default());
        assert_eq!(press(&context, &mut sheet, egui::Key::Enter), Action::close());
        sheet.settings.window_scale = 150;
        assert_eq!(press(&context, &mut sheet, egui::Key::Escape), Action::close());
        let action = press(&context, &mut sheet, egui::Key::Enter);
        assert!(action.close);
        assert_eq!(action.apply.map(|p| p.window_scale), Some(150));
        assert_eq!(sheet.applied.window_scale, 150);
    }
}
