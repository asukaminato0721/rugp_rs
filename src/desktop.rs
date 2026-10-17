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
    dpi::{PhysicalPosition, PhysicalSize},
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, run_on_demand::EventLoopExtRunOnDemand},
    keyboard::{KeyCode, PhysicalKey},
    monitor::Fullscreen,
    window::{Window, WindowAttributes, WindowId, WindowLevel},
};
mod config_window;
mod controls;
mod egui_raster;
mod history;
mod menu_panel;
mod native_store;
mod preferences;
mod read_log;
mod save_panel;
use config_window::{Action as ConfigAction, ConfigWindow, FontChoices};
use menu_panel::{Action as MenuAction, MenuPanel, Page};
use preferences::{FontPreferences, Preferences};
use save_panel::{Action as PanelAction, Mode as PanelMode, Panel};

pub fn run(
    game: Game,
    quit_after_ms: Option<u32>,
    initial_audio: Vec<rugp_runtime::audio_state::snapshot::PlaybackResume>,
    save_dir: Option<std::path::PathBuf>,
    choices: Vec<u32>,
) -> Result<()> {
    let mut event_loop = EventLoop::new().context("create desktop event loop")?;
    let preferences = Preferences::load(game.user_data_dir())?;
    let read_log = read_log::ReadLog::load(game.user_data_dir())?;
    let native_saves = native_store::NativeStore::detect(&game, save_dir, choices)?;
    eprintln!("Original saves: {}", native_saves.directory.display());
    let mut app = App::new(game, quit_after_ms, native_saves);
    app.read_log = read_log;
    app.apply_preferences(preferences)?;
    app.panel_audio = initial_audio;
    event_loop
        .run_app_on_demand(&mut app)
        .context("run desktop event loop")?;
    if let Some(error) = app.error {
        bail!(error);
    }
    Ok(())
}
struct App {
    game: Game,
    window: Option<Arc<dyn Window>>,
    surface: Option<softbuffer::Surface<Arc<dyn Window>, Arc<dyn Window>>>,
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
    config: Option<ConfigWindow>,
    /// Creating a window needs the event loop, so opening is deferred to `about_to_wait`.
    config_requested: bool,
    preferences: Preferences,
    controls: controls::Controls,
    right_pressed: bool,
    text_hidden: bool,
    history: history::History,
    exit_requested: bool,
    audio: Option<rugp_runtime::audio_device::DeviceAudio>,
    audio_tickets: std::collections::BTreeMap<u64, u64>,
    read_log: read_log::ReadLog,
    /// The waiting page; it counts as read once the player moves past it.
    shown_page: Option<u64>,
    font_families: Option<Vec<String>>,
    /// Fullscreen was dropped because another application became active.
    left_fullscreen: bool,
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
            config: None,
            config_requested: false,
            preferences: Preferences::default(),
            controls: controls::Controls::default(),
            right_pressed: false,
            text_hidden: false,
            history: history::History::default(),
            exit_requested: false,
            audio: None,
            audio_tickets: std::collections::BTreeMap::new(),
            read_log: read_log::ReadLog::default(),
            shown_page: None,
            font_families: None,
            left_fullscreen: false,
        }
    }
    fn point(&self, x: f64, y: f64) -> (i32, i32) {
        let Some(window) = self.window.as_ref() else {
            return (-1, -1);
        };
        let size = window.surface_size();
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
        if self.history.opened {
            self.panel_frame = Some(self.history.render(
                self.game.frame(),
                self.game.size(),
                self.panel_font.as_mut().context("history font missing")?,
            )?);
        } else if let Some(panel) = &self.panel {
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
        self.panel.is_some()
            || self.menu.is_some()
            || self.config_open()
            || self.text_hidden
            || self.history.opened
    }
    fn config_open(&self) -> bool {
        self.config.is_some() || self.config_requested
    }
    fn remember_dialogue(&mut self) {
        if self.game.dialogue_waits_for_input()
            && let (Some(cursor), Some(page)) = (self.game.vm.cursor(), self.game.dialogue_page())
        {
            self.history.record((cursor.scene, cursor.address), page);
        }
    }
    fn open_history(&mut self) -> Result<()> {
        self.remember_dialogue();
        self.pause_game()?;
        self.close_config();
        self.panel = None;
        self.menu = None;
        self.text_hidden = false;
        self.history.open();
        self.refresh_panel()
    }
    fn hide_text(&mut self) -> Result<()> {
        let Some(background) = self.game.dialogue_background()? else {
            return self.close_panel();
        };
        self.pause_game()?;
        self.close_config();
        self.panel = None;
        self.menu = None;
        self.history.close();
        self.text_hidden = true;
        self.panel_frame = Some(background);
        Ok(())
    }
    fn apply_preferences(&mut self, preferences: Preferences) -> Result<()> {
        self.game.set_text_speed(preferences.text_speed_ms)?;
        let text = match &preferences.font {
            Some(font) => font.to_runtime(),
            None => self.game.native_text_settings()?,
        };
        self.game.set_text_settings(text)?;
        if let Some(audio) = &mut self.audio {
            Self::configure_audio(audio, &preferences)?;
        }
        self.history
            .set_capacity(usize::from(preferences.system.history_pages));
        if let Some(window) = &self.window {
            window.set_fullscreen(
                preferences
                    .fullscreen
                    .then(|| Fullscreen::Borderless(window.current_monitor())),
            );
            window.set_window_level(
                if preferences.fullscreen && preferences.system.fullscreen_topmost {
                    WindowLevel::AlwaysOnTop
                } else {
                    WindowLevel::Normal
                },
            );
            // Only an explicit size choice overrides a window the user resized.
            if !preferences.fullscreen
                && (self.preferences.fullscreen
                    || self.preferences.window_scale != preferences.window_scale)
            {
                let _ = window.request_surface_size(self.window_size(&preferences).into());
            }
        }
        self.preferences = preferences;
        Ok(())
    }
    fn configure_audio(
        audio: &mut rugp_runtime::audio_device::DeviceAudio,
        preferences: &Preferences,
    ) -> Result<()> {
        audio.set_output_levels(preferences.output_levels())?;
        audio.set_bgm_ducking(preferences.duck_bgm);
        audio.set_acs(preferences.acs);
        Ok(())
    }
    fn open_audio(&mut self) -> Result<&mut rugp_runtime::audio_device::DeviceAudio> {
        if self.audio.is_none() {
            let mut audio =
                rugp_runtime::audio_device::DeviceAudio::open(Arc::clone(&self.game.archive))?;
            Self::configure_audio(&mut audio, &self.preferences)?;
            self.audio = Some(audio);
        }
        Ok(self.audio.as_mut().unwrap())
    }
    /// 設定 → 読んだ文章は飛ばす. Turning it on starts passing read pages.
    fn set_read_skip(&mut self, enabled: bool) -> Result<()> {
        let mut preferences = self.preferences.clone();
        preferences.read_skip = enabled;
        if let Err(error) = preferences.save(self.game.user_data_dir()) {
            eprintln!("Settings: {error:#}");
        }
        self.preferences = preferences;
        self.controls.set_reading_skip(enabled);
        Ok(())
    }
    /// The waiting page was read before, so read-skip may pass it.
    fn page_read(&self) -> bool {
        self.game
            .dialogue_page_key()
            .is_some_and(|page| self.read_log.contains(page))
    }
    fn track_read_page(&mut self) {
        let page = self.game.dialogue_page_key();
        if page != self.shown_page {
            if let Some(previous) = self.shown_page.take() {
                self.read_log.insert(previous);
            }
            self.shown_page = page;
        }
    }
    /// マウス追尾: put the cursor on a choice when it appears.
    fn follow_selection(&mut self) -> Result<()> {
        let (Some(window), Some((x, y))) = (&self.window, self.game.selection_focus_point()) else {
            return Ok(());
        };
        let size = window.surface_size();
        let (gw, gh) = self.game.size();
        let Some(viewport) = Viewport::fit(size.width, size.height, gw, gh) else {
            return Ok(());
        };
        let (wx, wy) = viewport.window_point(x, y);
        if window
            .set_cursor_position(PhysicalPosition::new(wx, wy).into())
            .is_ok()
        {
            self.cursor = Some((wx, wy));
        }
        Ok(())
    }
    fn font_choices(&mut self) -> Result<FontChoices> {
        let families = self
            .font_families
            .get_or_insert_with(rugp_runtime::font::japanese_families)
            .clone();
        Ok(FontChoices {
            families,
            standard: FontPreferences::from_runtime(&self.game.native_text_settings()?),
        })
    }
    fn window_size(&self, preferences: &Preferences) -> PhysicalSize<u32> {
        let (width, height) = self.game.size();
        let scale = u32::from(preferences.window_scale);
        PhysicalSize::new(width * scale / 100, height * scale / 100)
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
        self.text_hidden = false;
        self.history.close();
        self.close_config();
        self.menu = Some(MenuPanel::new(page));
        self.refresh_panel()
    }
    fn open_config(&mut self) -> Result<()> {
        self.pause_game()?;
        self.panel = None;
        self.menu = None;
        self.text_hidden = false;
        self.history.close();
        self.panel_frame = None;
        match &self.config {
            Some(config) => config.focus(),
            None => self.config_requested = true,
        }
        Ok(())
    }
    fn create_config(&mut self, event_loop: &dyn ActiveEventLoop) -> Result<()> {
        let fonts = self.font_choices()?;
        let font = self.panel_font.as_ref().context("settings font missing")?;
        self.config = Some(ConfigWindow::open(
            event_loop,
            self.preferences.clone(),
            fonts,
            font.face(),
        )?);
        Ok(())
    }
    /// Closing without OK or 適用 discards staged edits, like キャンセル.
    fn close_config(&mut self) {
        self.config_requested = false;
        self.config = None;
    }
    fn config_action(&mut self, action: ConfigAction) -> Result<()> {
        if let Some(settings) = action.apply {
            let saved = settings.save(self.game.user_data_dir());
            if let Err(error) = &saved {
                eprintln!("Settings: {error:#}");
            }
            if let Some(config) = &mut self.config {
                config.set_notice(
                    saved
                        .err()
                        .map(|error| format!("設定を保存できません: {error}"))
                        .unwrap_or_default(),
                );
            }
            self.apply_preferences(settings)?;
        }
        if action.close {
            self.close_panel()?;
        }
        Ok(())
    }
    fn right_button(&mut self, down: bool) -> Result<()> {
        if down {
            self.right_pressed = true;
        } else if std::mem::take(&mut self.right_pressed) {
            if self.modal_open() {
                return self.close_panel();
            }
            // Retail manual: right click cancels 読んだ文章は飛ばす.
            if self.controls.reading_skip() {
                return self.set_read_skip(false);
            }
            return self.open_menu(Page::Context);
        }
        Ok(())
    }
    fn menu_action(&mut self, action: MenuAction) -> Result<()> {
        match action {
            MenuAction::None => self.refresh_panel(),
            MenuAction::Resume => self.close_panel(),
            MenuAction::HideText => self.hide_text(),
            MenuAction::History => self.open_history(),
            MenuAction::Auto => {
                self.close_panel()?;
                self.controls.toggle_auto();
                Ok(())
            }
            MenuAction::Load => self.open_panel(PanelMode::Load),
            MenuAction::Settings => self.open_config(),
            MenuAction::QuitPrompt => self.open_menu(Page::Quit),
            MenuAction::Quit => {
                self.exit_requested = true;
                Ok(())
            }
            MenuAction::Skip => {
                self.close_panel()?;
                self.set_read_skip(!self.preferences.read_skip)
            }
        }
    }
    fn open_panel(&mut self, mode: PanelMode) -> Result<()> {
        if self.game.waiting_for_load() && mode == PanelMode::Save {
            return Ok(());
        }
        let occupied = self.native_saves.slots()?;
        self.pause_game()?;
        self.close_config();
        self.menu = None;
        self.text_hidden = false;
        self.history.close();
        let mut panel = Panel::new(mode, self.slot, occupied);
        panel.double_click = self.preferences.system.double_click_lists;
        self.panel = Some(panel);
        self.refresh_panel()
    }
    fn close_panel(&mut self) -> Result<()> {
        self.game.cancel_load_request()?;
        for playback in std::mem::take(&mut self.panel_audio) {
            if let Some(ticket) = self.open_audio()?.resume(&playback)? {
                self.audio_tickets.insert(playback.ticket, ticket);
            } else {
                self.game.finish_audio_ticket(playback.ticket);
            }
        }
        self.close_config();
        self.panel = None;
        self.menu = None;
        self.text_hidden = false;
        self.history.close();
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
        self.history.forget_cursor();
        self.close_panel()
    }
    fn sounds(&mut self) -> Result<()> {
        for request in self.game.take_sounds() {
            if (self.controls.active() || self.controls.reading_skip())
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
            if self.audio.is_none()
                && request.source.is_none()
                && [254, 255].contains(&request.kind)
            {
                continue;
            }
            let runtime_ticket = request.ticket;
            if let Some(ticket) = self.open_audio()?.request(request)? {
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
        let size = window.surface_size();
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
        let stride = size.width as usize;
        if self.preferences.system.bilinear && (width, height) != (gw as usize, gh as usize) {
            scale_bilinear(
                frame,
                (gw as usize, gh as usize),
                &mut output,
                stride,
                viewport,
            );
        } else {
            for y in 0..height {
                let sy = y * gh as usize / height;
                for x in 0..width {
                    let sx = x * gw as usize / width;
                    let i = (sy * gw as usize + sx) * 4;
                    output[(y + top) * stride + x + left] =
                        (frame[i] as u32) << 16 | (frame[i + 1] as u32) << 8 | frame[i + 2] as u32;
                }
            }
        }
        output
            .present()
            .map_err(|e| anyhow::anyhow!("present frame: {e}"))?;
        Ok(())
    }
    /// Pointer left or focus moved away: drop presses and continuous input.
    fn lose_pointer(&mut self) {
        self.cursor = None;
        self.controls.stop();
        self.right_pressed = false;
        self.history.cancel_press();
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

/// システム: 拡大縮小系のバイリニアフィルタリング. RGBA source into the 0RGB
/// window buffer's viewport, sampling at pixel centres in 8-bit fixed point.
fn scale_bilinear(
    frame: &[u8],
    (gw, gh): (usize, usize),
    output: &mut [u32],
    stride: usize,
    viewport: Viewport,
) {
    let (width, height) = (viewport.width as usize, viewport.height as usize);
    let sample = |position: usize, target: usize, source: usize| {
        let at = ((2 * position + 1) * source * 256 / (2 * target)).saturating_sub(128);
        let low = (at >> 8).min(source - 1);
        (low, (low + 1).min(source - 1), (at & 255) as u32)
    };
    let columns: Vec<_> = (0..width).map(|x| sample(x, width, gw)).collect();
    for y in 0..height {
        let (y0, y1, fy) = sample(y, height, gh);
        let row = (y + viewport.top as usize) * stride + viewport.left as usize;
        for (x, &(x0, x1, fx)) in columns.iter().enumerate() {
            let texel = |sx: usize, sy: usize, c: usize| u32::from(frame[(sy * gw + sx) * 4 + c]);
            let mut pixel = 0;
            for c in 0..3 {
                let top = texel(x0, y0, c) * (256 - fx) + texel(x1, y0, c) * fx;
                let bottom = texel(x0, y1, c) * (256 - fx) + texel(x1, y1, c) * fx;
                pixel = pixel << 8 | (top * (256 - fy) + bottom * fy) >> 16;
            }
            output[row + x] = pixel;
        }
    }
}

impl ApplicationHandler for App {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let result = (|| -> Result<()> {
            let window: Arc<dyn Window> = Arc::from(
                event_loop.create_window(
                    WindowAttributes::default()
                        .with_title(self.game.title())
                        .with_surface_size(self.window_size(&self.preferences)),
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
    fn window_event(&mut self, event_loop: &dyn ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let Some(config) = &mut self.config
            && config.id() == id
        {
            let action = if event == WindowEvent::RedrawRequested {
                config.redraw()
            } else {
                Ok(config.event(&event))
            };
            if let Err(error) = action.and_then(|action| self.config_action(action)) {
                self.fail(error);
            }
            return;
        }
        if self.window.as_ref().is_none_or(|w| w.id() != id) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::PointerMoved { position, .. } if self.error.is_none() => {
                self.cursor = Some((position.x, position.y));
                if let Some(menu) = &mut self.menu {
                    let (width, height) = self.game.size();
                    let size = self.window.as_ref().unwrap().surface_size();
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
            WindowEvent::Focused(focused) if self.error.is_none() && self.config.is_none() => {
                // システム: 他のプロセスがアクティブになった時、フルスクリーンを解除する.
                if let Some(window) = &self.window {
                    if !focused
                        && self.preferences.fullscreen
                        && self.preferences.system.leave_fullscreen_inactive
                    {
                        window.set_fullscreen(None);
                        self.left_fullscreen = true;
                    } else if focused && std::mem::take(&mut self.left_fullscreen) {
                        window
                            .set_fullscreen(Some(Fullscreen::Borderless(window.current_monitor())));
                    }
                }
                if !focused {
                    self.lose_pointer();
                }
            }
            WindowEvent::PointerLeft { .. } | WindowEvent::Focused(false)
                if self.error.is_none() =>
            {
                self.lose_pointer();
            }
            WindowEvent::PointerButton { state, button, .. }
                if self.error.is_none()
                    && button.clone().mouse_button() == Some(MouseButton::Left) =>
            {
                let result = if self.text_hidden {
                    if state == ElementState::Released {
                        self.close_panel()
                    } else {
                        Ok(())
                    }
                } else if self.history.opened {
                    let (x, y) = self.cursor.map_or((-1, -1), |(x, y)| self.point(x, y));
                    if self.history.button(x, y, state == ElementState::Pressed) {
                        self.close_panel()
                    } else {
                        self.refresh_panel()
                    }
                } else if self.panel.is_some() {
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
                } else if self.config_open() {
                    if let Some(config) = &self.config {
                        config.focus();
                    }
                    Ok(())
                } else if self.controls.active()
                    || self.controls.automatic()
                    || self.controls.reading_skip()
                {
                    self.controls.stop();
                    Ok(())
                } else {
                    self.game.pointer_button(state == ElementState::Pressed)
                };
                if let Err(error) = result {
                    self.fail(error);
                }
            }
            WindowEvent::PointerButton { state, button, .. }
                if self.error.is_none()
                    && button.clone().mouse_button() == Some(MouseButton::Right) =>
            {
                if let Err(error) = self.right_button(state == ElementState::Pressed) {
                    self.fail(error);
                }
            }
            WindowEvent::MouseWheel { delta, .. } if self.error.is_none() => {
                let amount = match delta {
                    MouseScrollDelta::LineDelta(_, y) => f64::from(y),
                    MouseScrollDelta::PixelDelta(position) => position.y,
                    _ => 0.0,
                };
                let result = if self.history.opened && amount != 0.0 {
                    self.history.move_entry(if amount > 0.0 { -1 } else { 1 });
                    self.refresh_panel()
                } else if !self.modal_open() && amount > 0.0 {
                    self.open_history()
                } else if !self.modal_open() && amount < 0.0 && self.game.waiting_for_dialogue() {
                    self.controls.stop();
                    self.game.activate_selection()
                } else {
                    Ok(())
                };
                if let Err(error) = result {
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
                        if self.config_open() {
                            self.close_panel()
                        } else {
                            self.open_config()
                        }
                    }
                    PhysicalKey::Code(KeyCode::Escape) if self.config_open() => self.close_panel(),
                    _ if self.config_open() => Ok(()),
                    PhysicalKey::Code(
                        KeyCode::Space | KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Escape,
                    ) if self.text_hidden => self.close_panel(),
                    _ if self.text_hidden => Ok(()),
                    PhysicalKey::Code(key) if self.history.opened => {
                        if self.history.key(key) {
                            self.close_panel()
                        } else {
                            self.refresh_panel()
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
                        } else if self.controls.reading_skip() {
                            self.set_read_skip(false)
                        } else {
                            self.open_menu(Page::Context)
                        }
                    }
                    PhysicalKey::Code(KeyCode::KeyS) => {
                        self.set_read_skip(!self.preferences.read_skip)
                    }
                    PhysicalKey::Code(KeyCode::KeyA) => {
                        self.controls.toggle_auto();
                        Ok(())
                    }
                    PhysicalKey::Code(KeyCode::PageUp) => self.open_history(),
                    PhysicalKey::Code(KeyCode::Space) if self.game.waiting_for_dialogue() => {
                        self.hide_text()
                    }
                    PhysicalKey::Code(KeyCode::ArrowDown | KeyCode::ArrowRight | KeyCode::Tab) => {
                        self.game.navigate_selection(1)
                    }
                    PhysicalKey::Code(KeyCode::ArrowUp | KeyCode::ArrowLeft) => {
                        self.game.navigate_selection(-1)
                    }
                    PhysicalKey::Code(KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space) => {
                        self.controls.stop();
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
                    if let Some(audio) = &mut self.audio {
                        audio.update();
                    }
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
                    if self.controls.auto_ready(
                        self.game.dialogue_waits_for_input(),
                        self.game.dialogue_voice_playing(),
                        dt,
                        self.preferences.auto_delay_ms,
                    ) && let Err(error) = self.game.activate_selection()
                    {
                        self.fail(error);
                    }
                    if self.preferences.read_skip && self.page_read() {
                        self.controls.set_reading_skip(true);
                    }
                    let log = &mut self.read_log;
                    let result = if self.error.is_some() {
                        Ok(RunState::Waiting)
                    } else if self.controls.active() {
                        // Ctrl passes every page; they have been seen.
                        self.game
                            .fast_forward_step_with(dt, |page| {
                                log.insert(page);
                                true
                            })
                            .map(|(state, _)| state)
                    } else if self.preferences.read_skip && self.controls.reading_skip() {
                        match self
                            .game
                            .fast_forward_step_with(dt, |page| log.contains(page))
                        {
                            Ok((state, stopped)) => {
                                if stopped {
                                    self.controls.set_reading_skip(false);
                                }
                                Ok(state)
                            }
                            Err(error) => Err(error),
                        }
                    } else {
                        self.game.step(dt)
                    };
                    match result {
                        Ok(RunState::Ended) => event_loop.exit(),
                        Ok(_) => {}
                        Err(e) => self.fail(e),
                    }
                    self.remember_dialogue();
                    self.track_read_page();
                    if self.game.waiting_for_selection()
                        || self.game.waiting_for_input()
                        || self.game.waiting_for_load()
                    {
                        self.controls.stop();
                    }
                    if !selecting && self.game.waiting_for_selection() {
                        if self.preferences.mouse_follow
                            && let Err(error) = self.follow_selection()
                        {
                            self.fail(error);
                        }
                        if let Some((x, y)) = self.cursor
                            && let Err(error) = self.move_pointer(x, y)
                        {
                            self.fail(error);
                        }
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
            WindowEvent::SurfaceResized(_) => {
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
    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        if std::mem::take(&mut self.config_requested)
            && self.error.is_none()
            && let Err(error) = self.create_config(event_loop)
        {
            eprintln!("Settings window: {error:#}");
            if let Err(error) = self.close_panel() {
                self.fail(error);
            }
        }
        if let Some(config) = &self.config {
            config.wake(Instant::now());
        }
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
        app.controls.key(KeyCode::ControlLeft, true);
        app.right_button(true)?;
        app.right_button(false)?;
        assert!(app.menu.as_ref().is_some_and(|m| m.page == Page::Context));
        assert!(!app.controls.active());
        for _ in 0..6 {
            app.menu.as_mut().unwrap().key(KeyCode::ArrowDown);
        }
        let action = app.menu.as_mut().unwrap().key(KeyCode::Enter);
        app.menu_action(action)?;
        // Settings open as a separate window once the event loop is idle.
        assert!(app.menu.is_none() && app.config_requested && app.modal_open());
        assert!(!app.controls.active());
        app.close_panel()?;
        assert!(!app.config_requested && !app.modal_open());
        // A settings round trip before the first dialogue remains a valid UI state.
        app.game.snapshot_ui(&mut Default::default())?;
        app.right_button(true)?;
        app.right_button(false)?;
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
        app.open_history()?;
        assert!(app.history.opened);
        assert!(!app.controls.active());
        app.close_panel()?;
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
