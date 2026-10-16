//! Desktop presentation host; all game/render state stays in rugp_runtime.
use anyhow::{Context, Result, bail};
use rugp_runtime::input::Viewport;
use rugp_runtime::{Game, RunState};
use std::{
    num::NonZeroU32,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};
mod controls;
mod menu_panel;
mod native_store;
mod preferences;
mod save_panel;
use menu_panel::{Action as MenuAction, MenuPanel, Page};
use preferences::Preferences;
use save_panel::{Action as PanelAction, Mode as PanelMode, Panel};

pub fn run(
    game: Game,
    quit_after_ms: Option<u32>,
    initial_audio: Vec<rugp_runtime::audio_state::snapshot::PlaybackResume>,
    save_dir: Option<std::path::PathBuf>,
    choices: Vec<u32>,
) -> Result<()> {
    let event_loop = EventLoop::new().context("create desktop event loop")?;
    let preferences = Preferences::load(game.user_data_dir())?;
    let native_saves = native_store::NativeStore::detect(&game, save_dir, choices)?;
    eprintln!("Original saves: {}", native_saves.directory.display());
    let mut app = App::new(game, quit_after_ms, native_saves);
    app.apply_preferences(preferences)?;
    app.panel_audio = initial_audio;
    event_loop
        .run_app(&mut app)
        .context("run desktop event loop")?;
    if let Some(error) = app.error {
        bail!(error);
    }
    Ok(())
}
struct App {
    game: Game,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    last: Instant,
    elapsed: u32,
    tick_remainder: Duration,
    error: Option<String>,
    quit_after_ms: Option<u32>,
    audio_resources: Option<rugp_runtime::audio::AudioResources>,
    cursor: Option<(f64, f64)>,
    panel: Option<Panel>,
    panel_frame: Option<rugp_runtime::render::Framebuffer>,
    panel_font: Option<rugp_runtime::font::TextFont>,
    panel_audio: Vec<rugp_runtime::audio_state::snapshot::PlaybackResume>,
    slot: usize,
    native_saves: native_store::NativeStore,
    menu: Option<MenuPanel>,
    preferences: Preferences,
    controls: controls::Controls,
    right_pressed: bool,
    exit_requested: bool,
    audio: Option<rugp_runtime::audio_device::DeviceAudio>,
    audio_tickets: std::collections::BTreeMap<u64, u64>,
}
impl App {
    fn new(
        game: Game,
        quit_after_ms: Option<u32>,
        native_saves: native_store::NativeStore,
    ) -> Self {
        Self {
            game,
            quit_after_ms,
            window: None,
            surface: None,
            last: Instant::now(),
            elapsed: 0,
            tick_remainder: Duration::ZERO,
            error: None,
            audio_resources: None,
            cursor: None,
            panel: None,
            panel_frame: None,
            panel_font: None,
            panel_audio: Vec::new(),
            slot: 0,
            native_saves,
            menu: None,
            preferences: Preferences::default(),
            controls: controls::Controls::default(),
            right_pressed: false,
            exit_requested: false,
            audio: None,
            audio_tickets: std::collections::BTreeMap::new(),
        }
    }
    fn point(&self, x: f64, y: f64) -> (i32, i32) {
        let Some(window) = self.window.as_ref() else {
            return (-1, -1);
        };
        let size = window.inner_size();
        let (gw, gh) = self.game.size();
        Viewport::fit(size.width, size.height, gw, gh)
            .and_then(|v| v.point(x, y))
            .unwrap_or((-1, -1))
    }
    fn move_pointer(&mut self, x: f64, y: f64) -> Result<()> {
        let (x, y) = self.point(x, y);
        self.game.pointer_move(x, y)
    }
    fn refresh_panel(&mut self) -> Result<()> {
        if let Some(panel) = &self.panel {
            self.panel_frame = Some(panel.render(
                self.game.frame(),
                self.game.size(),
                self.panel_font.as_mut().context("slot UI font missing")?,
            )?);
        } else if let Some(menu) = &self.menu {
            self.panel_frame = Some(menu.render(
                self.game.frame(),
                self.game.size(),
                self.panel_font.as_mut().context("menu font missing")?,
            )?);
        }
        Ok(())
    }
    fn modal_open(&self) -> bool {
        self.panel.is_some() || self.menu.is_some()
    }
    fn apply_preferences(&mut self, preferences: Preferences) -> Result<()> {
        self.game.set_text_speed(preferences.text_speed_ms)?;
        if let Some(audio) = &mut self.audio {
            audio.set_output_levels(preferences.levels)?;
        }
        if let Some(window) = &self.window {
            window.set_fullscreen(
                preferences
                    .fullscreen
                    .then(|| winit::window::Fullscreen::Borderless(window.current_monitor())),
            );
        }
        self.preferences = preferences;
        Ok(())
    }
    fn pause_game(&mut self) -> Result<()> {
        self.controls.stop();
        self.right_pressed = false;
        if self.panel_font.is_none() {
            self.panel_font = Some(self.game.interface_font(20)?);
        }
        if !self.modal_open() {
            self.game.pointer_move(-1, -1)?;
            self.game.pointer_button(false)?;
            if self.audio_resources.is_none() {
                self.audio_resources = Some(rugp_runtime::audio::AudioResources::new(Arc::clone(
                    &self.game.archive,
                ))?);
            }
            self.panel_audio = self
                .game
                .rebuild_audio_output(self.audio_resources.as_mut().unwrap())?;
            if let Some(audio) = &mut self.audio {
                audio.reset();
            }
            self.audio_tickets.clear();
        }
        Ok(())
    }
    fn open_menu(&mut self, page: Page) -> Result<()> {
        self.pause_game()?;
        self.panel = None;
        self.menu = Some(MenuPanel::new(page, self.preferences.clone()));
        self.refresh_panel()
    }
    fn right_button(&mut self, down: bool) -> Result<()> {
        if down {
            self.right_pressed = true;
        } else if std::mem::take(&mut self.right_pressed) {
            if self.modal_open() {
                return self.close_panel();
            }
            return self.open_menu(Page::Context);
        }
        Ok(())
    }
    fn menu_action(&mut self, action: MenuAction) -> Result<()> {
        match action {
            MenuAction::None => self.refresh_panel(),
            MenuAction::Resume => self.close_panel(),
            MenuAction::Save => self.open_panel(PanelMode::Save),
            MenuAction::Load => self.open_panel(PanelMode::Load),
            MenuAction::Settings => self.open_menu(Page::Settings),
            MenuAction::QuitPrompt => self.open_menu(Page::Quit),
            MenuAction::Quit => {
                self.exit_requested = true;
                Ok(())
            }
            MenuAction::Skip => {
                self.close_panel()?;
                self.controls.toggle();
                Ok(())
            }
            MenuAction::Change(preferences) => {
                if let Err(error) = preferences.save(self.game.user_data_dir()) {
                    self.menu.as_mut().unwrap().notice = format!("設定を保存できません: {error}");
                } else {
                    self.menu.as_mut().unwrap().notice.clear();
                }
                self.apply_preferences(preferences)?;
                self.refresh_panel()
            }
        }
    }
    fn open_panel(&mut self, mode: PanelMode) -> Result<()> {
        if self.game.waiting_for_load() && mode == PanelMode::Save {
            return Ok(());
        }
        let occupied = self.native_saves.slots()?;
        self.pause_game()?;
        self.menu = None;
        self.panel = Some(Panel::new(mode, self.slot, occupied));
        self.refresh_panel()
    }
    fn close_panel(&mut self) -> Result<()> {
        self.game.cancel_load_request()?;
        for playback in std::mem::take(&mut self.panel_audio) {
            if self.audio.is_none() {
                self.audio = Some(rugp_runtime::audio_device::DeviceAudio::open(Arc::clone(
                    &self.game.archive,
                ))?);
                self.audio
                    .as_mut()
                    .unwrap()
                    .set_output_levels(self.preferences.levels)?;
            }
            if let Some(ticket) = self.audio.as_mut().unwrap().resume(&playback)? {
                self.audio_tickets.insert(playback.ticket, ticket);
            } else {
                self.game.finish_audio_ticket(playback.ticket);
            }
        }
        self.panel = None;
        self.menu = None;
        self.controls.stop();
        self.panel_frame = None;
        self.last = Instant::now();
        if let Some((x, y)) = self.cursor {
            self.move_pointer(x, y)?;
        }
        Ok(())
    }
    fn panel_action(&mut self, action: PanelAction) -> Result<()> {
        if let Some(panel) = &self.panel {
            self.slot = panel.slot;
        }
        let result = match action {
            PanelAction::None => return self.refresh_panel(),
            PanelAction::Cancel => return self.close_panel(),
            PanelAction::Save(_) => Err(anyhow::anyhow!("native UUC writing is not implemented")),
            PanelAction::Load(slot) => self
                .game
                .import_uuc(
                    &self.native_saves.path(slot),
                    &self.native_saves.choices,
                    self.audio_resources
                        .as_mut()
                        .context("load audio resources missing")?,
                )
                .map(|resumes| {
                    self.panel_audio = resumes;
                }),
        };
        if let Err(error) = result {
            eprintln!("Slot {}: {error:#}", self.slot);
            if let Some(panel) = &mut self.panel {
                panel.notice = if panel.mode == PanelMode::Save {
                    "原版セーブの書き込みは未対応です。"
                } else {
                    "ロードできません。現在のゲームは変更されていません。詳細は端末。"
                }
                .into();
            }
            self.last = Instant::now();
            return self.refresh_panel();
        }
        eprintln!(
            "Slot {}: {}",
            self.slot,
            if matches!(action, PanelAction::Save(_)) {
                "saved"
            } else {
                "loaded"
            }
        );
        self.apply_preferences(self.preferences.clone())?;
        self.close_panel()
    }
    fn sounds(&mut self) -> Result<()> {
        for request in self.game.take_sounds() {
            if self.controls.active()
                && [2, 4].contains(&request.kind)
                && let (Some(source), Some(ticket)) = (&request.source, request.ticket)
            {
                if self.audio_resources.is_none() {
                    self.audio_resources = Some(rugp_runtime::audio::AudioResources::new(
                        Arc::clone(&self.game.archive),
                    )?);
                }
                if let Some(duration) = self
                    .audio_resources
                    .as_mut()
                    .unwrap()
                    .muted_duration(source)?
                {
                    self.game.time_audio_ticket(ticket, duration);
                }
            }
            if self.audio.is_none() {
                if request.source.is_none() && [254, 255].contains(&request.kind) {
                    continue;
                }
                self.audio = Some(rugp_runtime::audio_device::DeviceAudio::open(Arc::clone(
                    &self.game.archive,
                ))?);
                self.audio
                    .as_mut()
                    .unwrap()
                    .set_output_levels(self.preferences.levels)?;
            }
            let runtime_ticket = request.ticket;
            if let Some(ticket) = self.audio.as_mut().unwrap().request(request)? {
                self.audio_tickets.insert(
                    runtime_ticket.context("play request lacks runtime identity")?,
                    ticket,
                );
            }
        }
        Ok(())
    }
    fn present(&mut self) -> Result<()> {
        let Some(window) = &self.window else {
            return Ok(());
        };
        let size = window.inner_size();
        let (Some(sw), Some(sh)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return Ok(());
        };
        let surface = self
            .surface
            .as_mut()
            .context("software window surface missing")?;
        surface
            .resize(sw, sh)
            .map_err(|e| anyhow::anyhow!("resize surface: {e}"))?;
        let mut output = surface
            .buffer_mut()
            .map_err(|e| anyhow::anyhow!("acquire window buffer: {e}"))?;
        output.fill(0);
        let (gw, gh) = self.game.size();
        let viewport = Viewport::fit(size.width, size.height, gw, gh)
            .context("invalid presentation viewport")?;
        let (width, height, left, top) = (
            viewport.width as usize,
            viewport.height as usize,
            viewport.left as usize,
            viewport.top as usize,
        );
        let frame = self
            .panel_frame
            .as_ref()
            .map_or_else(|| self.game.frame(), |f| f.rgba.as_slice());
        for y in 0..height {
            let sy = y * gh as usize / height;
            for x in 0..width {
                let sx = x * gw as usize / width;
                let i = (sy * gw as usize + sx) * 4;
                output[(y + top) * size.width as usize + x + left] =
                    (frame[i] as u32) << 16 | (frame[i + 1] as u32) << 8 | frame[i + 2] as u32;
            }
        }
        output
            .present()
            .map_err(|e| anyhow::anyhow!("present frame: {e}"))?;
        Ok(())
    }
    fn fail(&mut self, error: anyhow::Error) {
        // A failing VM tick may already have queued playback. Do not start it
        // after dropping the output device, or reopen a device on the error UI.
        self.game.take_sounds();
        self.audio = None;
        self.audio_tickets.clear();
        let message = format!("{error:#}");
        eprintln!("{message}");
        if let Some(window) = &self.window {
            window.set_title(&format!("rUGP — {message}"));
        }
        self.error = Some(message);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let result = (|| -> Result<()> {
            let (width, height) = self.game.size();
            let window = Arc::new(
                event_loop.create_window(
                    Window::default_attributes()
                        .with_title(self.game.title())
                        .with_inner_size(PhysicalSize::new(width, height)),
                )?,
            );
            let context = softbuffer::Context::new(Arc::clone(&window))
                .map_err(|e| anyhow::anyhow!("create display context: {e}"))?;
            let surface = softbuffer::Surface::new(&context, Arc::clone(&window))
                .map_err(|e| anyhow::anyhow!("create software surface: {e}"))?;
            self.window = Some(window);
            self.surface = Some(surface);
            self.apply_preferences(self.preferences.clone())?;
            if !self.panel_audio.is_empty() {
                self.close_panel()?;
            }
            self.last = Instant::now();
            Ok(())
        })();
        if let Err(error) = result {
            self.fail(error);
            event_loop.exit();
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.window.as_ref().is_none_or(|w| w.id() != id) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::CursorMoved { position, .. } if self.error.is_none() => {
                self.cursor = Some((position.x, position.y));
                if let Some(menu) = &mut self.menu {
                    let (width, height) = self.game.size();
                    let size = self.window.as_ref().unwrap().inner_size();
                    let (x, y) = Viewport::fit(size.width, size.height, width, height)
                        .and_then(|v| v.point(position.x, position.y))
                        .unwrap_or((-1, -1));
                    menu.pointer(x, y);
                    if let Err(error) = self.refresh_panel() {
                        self.fail(error);
                    }
                } else if !self.modal_open()
                    && let Err(error) = self.move_pointer(position.x, position.y)
                {
                    self.fail(error);
                }
            }
            WindowEvent::CursorLeft { .. } | WindowEvent::Focused(false)
                if self.error.is_none() =>
            {
                self.cursor = None;
                self.controls.stop();
                self.right_pressed = false;
                if let Some(menu) = &mut self.menu {
                    menu.cancel_press();
                    return;
                }
                if let Some(panel) = &mut self.panel {
                    panel.cancel_press();
                    return;
                }
                if let Err(error) = self
                    .game
                    .pointer_move(-1, -1)
                    .and_then(|()| self.game.pointer_button(false))
                {
                    self.fail(error);
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } if self.error.is_none() => {
                let result = if self.panel.is_some() {
                    let (x, y) = self.cursor.map_or((-1, -1), |(x, y)| self.point(x, y));
                    let action =
                        self.panel
                            .as_mut()
                            .unwrap()
                            .button(x, y, state == ElementState::Pressed);
                    self.panel_action(action)
                } else if self.menu.is_some() {
                    let (x, y) = self.cursor.map_or((-1, -1), |(x, y)| self.point(x, y));
                    let action =
                        self.menu
                            .as_mut()
                            .unwrap()
                            .button(x, y, state == ElementState::Pressed);
                    self.menu_action(action)
                } else if self.controls.active() {
                    self.controls.stop();
                    Ok(())
                } else {
                    self.game.pointer_button(state == ElementState::Pressed)
                };
                if let Err(error) = result {
                    self.fail(error);
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Right,
                ..
            } if self.error.is_none() => {
                if let Err(error) = self.right_button(state == ElementState::Pressed) {
                    self.fail(error);
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if matches!(
                    event.physical_key,
                    PhysicalKey::Code(KeyCode::ControlLeft | KeyCode::ControlRight)
                ) =>
            {
                if let PhysicalKey::Code(key) = event.physical_key {
                    let down = event.state == ElementState::Pressed
                        && !self.modal_open()
                        && self.error.is_none();
                    self.controls.key(key, down);
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if self.error.is_none()
                    && event.state == ElementState::Pressed
                    && !event.repeat =>
            {
                let result = match event.physical_key {
                    PhysicalKey::Code(KeyCode::F5) => self.open_panel(PanelMode::Save),
                    PhysicalKey::Code(KeyCode::F9) => self.open_panel(PanelMode::Load),
                    PhysicalKey::Code(KeyCode::F2) => {
                        if self.menu.as_ref().is_some_and(|m| m.page == Page::Settings) {
                            self.close_panel()
                        } else {
                            self.open_menu(Page::Settings)
                        }
                    }
                    PhysicalKey::Code(key) if self.menu.is_some() => {
                        let action = self.menu.as_mut().unwrap().key(key);
                        self.menu_action(action)
                    }
                    PhysicalKey::Code(key) if self.panel.is_some() => {
                        let action = self.panel.as_mut().unwrap().key(key);
                        self.panel_action(action)
                    }
                    PhysicalKey::Code(KeyCode::Escape) => {
                        if self.game.video().is_some_and(|v| !v.finished()) {
                            self.game.skip_video().map(|_| ())
                        } else {
                            self.open_menu(Page::Context)
                        }
                    }
                    PhysicalKey::Code(KeyCode::KeyS) => {
                        self.controls.toggle();
                        Ok(())
                    }
                    PhysicalKey::Code(KeyCode::ArrowDown | KeyCode::ArrowRight | KeyCode::Tab) => {
                        self.game.navigate_selection(1)
                    }
                    PhysicalKey::Code(KeyCode::ArrowUp | KeyCode::ArrowLeft) => {
                        self.game.navigate_selection(-1)
                    }
                    PhysicalKey::Code(KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space) => {
                        self.game.activate_selection()
                    }
                    _ => Ok(()),
                };
                if let Err(error) = result {
                    self.fail(error);
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let (dt, remainder) =
                    controls::elapsed_ms(now.duration_since(self.last) + self.tick_remainder);
                self.tick_remainder = remainder;
                self.last = now;
                if self.error.is_none() && !self.modal_open() {
                    if let Some(audio) = &self.audio {
                        self.audio_tickets.retain(|&runtime, &mut ticket| {
                            if audio.finished(ticket) {
                                self.game.finish_audio_ticket(runtime);
                                false
                            } else {
                                true
                            }
                        });
                    }
                    let selecting = self.game.waiting_for_selection();
                    if selecting || self.game.waiting_for_input() || self.game.waiting_for_load() {
                        self.controls.stop();
                    }
                    let result = if self.controls.active() {
                        self.game.fast_forward_step(dt)
                    } else {
                        self.game.step(dt)
                    };
                    match result {
                        Ok(RunState::Ended) => event_loop.exit(),
                        Ok(_) => {}
                        Err(e) => self.fail(e),
                    }
                    if self.game.waiting_for_selection()
                        || self.game.waiting_for_input()
                        || self.game.waiting_for_load()
                    {
                        self.controls.stop();
                    }
                    if !selecting
                        && self.game.waiting_for_selection()
                        && let Some((x, y)) = self.cursor
                        && let Err(error) = self.move_pointer(x, y)
                    {
                        self.fail(error);
                    }
                    if self.error.is_none()
                        && let Err(e) = self.sounds()
                    {
                        self.fail(e);
                    }
                    if self.error.is_none()
                        && self.game.waiting_for_load()
                        && let Err(e) = self.open_panel(PanelMode::Load)
                    {
                        self.fail(e);
                    }
                }
                if self.exit_requested {
                    event_loop.exit();
                }
                if let Err(e) = self.present() {
                    self.fail(e);
                    event_loop.exit();
                }
                self.elapsed = self.elapsed.saturating_add(dt);
                if self
                    .quit_after_ms
                    .is_some_and(|limit| self.elapsed >= limit)
                {
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(_) => {
                if self.error.is_none()
                    && !self.modal_open()
                    && let Some((x, y)) = self.cursor
                    && let Err(error) = self.move_pointer(x, y)
                {
                    self.fail(error);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let deadline = self.last + Duration::from_millis(16);
        event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        if Instant::now() >= deadline
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "set RUGP_TEST_GAME; requires a Japanese outline font"]
    fn retail_settings_and_context_menu_pause_preserve_input_and_handle_invalid_native_loads()
    -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let directory = std::env::temp_dir().join(format!(
            "rugp-menu-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let game = Game::open_persistent(root.as_ref(), &directory)?;
        let saves = directory.join("Vmreg");
        std::fs::create_dir(&saves)?;
        std::fs::write(saves.join("vmr00001.uuc"), b"damaged native save")?;
        let native_saves = native_store::NativeStore::detect(&game, Some(saves.clone()), vec![])?;
        let mut app = App::new(game, None, native_saves);
        app.apply_preferences(Preferences::default())?;
        let cursor = app.game.vm.cursor().cloned();
        let pixels = app.game.frame().to_vec();
        app.right_button(false)?;
        assert!(!app.modal_open());
        app.controls.toggle();
        app.right_button(true)?;
        app.right_button(false)?;
        assert!(app.menu.as_ref().is_some_and(|m| m.page == Page::Context));
        assert!(!app.controls.active());
        for _ in 0..3 {
            app.menu.as_mut().unwrap().key(KeyCode::ArrowDown);
        }
        let action = app.menu.as_mut().unwrap().key(KeyCode::Enter);
        app.menu_action(action)?;
        assert!(app.menu.as_ref().is_some_and(|m| m.page == Page::Settings));
        let action = app.menu.as_mut().unwrap().key(KeyCode::ArrowRight);
        app.menu_action(action)?;
        assert_eq!(app.preferences.text_speed_ms, 5);
        assert_eq!(
            Preferences::load(app.game.user_data_dir())?,
            app.preferences
        );
        // A setting made before the first dialogue also remains a valid UI state.
        app.game.snapshot_ui(&mut Default::default())?;
        if let Some(path) = std::env::var_os("RUGP_TEST_MENU_PNG") {
            let frame = app.panel_frame.as_ref().context("menu frame missing")?;
            let mut encoder =
                png::Encoder::new(std::fs::File::create(path)?, frame.width, frame.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header()?.write_image_data(&frame.rgba)?;
        }
        app.right_button(true)?;
        app.right_button(false)?;
        assert!(!app.modal_open());
        assert_eq!(app.game.vm.cursor(), cursor.as_ref());
        assert_eq!(app.game.frame(), pixels);
        app.game.pointer_button(false)?;
        assert_eq!(app.game.vm.cursor(), cursor.as_ref());
        app.open_panel(PanelMode::Load)?;
        app.panel_action(PanelAction::Load(1))?;
        assert!(app.panel.is_some());
        assert!(!app.panel.as_ref().unwrap().notice.is_empty());
        assert_eq!(app.game.vm.cursor(), cursor.as_ref());
        assert_eq!(app.game.frame(), pixels);
        app.close_panel()?;
        app.open_panel(PanelMode::Save)?;
        app.panel_action(PanelAction::Save(2))?;
        assert!(!saves.join("vmr00002.uuc").exists());
        assert!(app.panel.is_some());
        assert_eq!(
            std::fs::read(saves.join("vmr00001.uuc"))?,
            b"damaged native save"
        );
        assert_eq!(app.game.vm.cursor(), cursor.as_ref());
        drop(app);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }
}
