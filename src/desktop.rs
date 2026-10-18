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
mod egui_window;
mod history;
mod menu_bar;
mod menu_panel;
mod native_store;
mod paint;
mod preferences;
mod read_log;
mod save_store;
mod save_window;
use config_window::{Action as ConfigAction, ConfigWindow, FontChoices, Page as ConfigPage};
use menu_panel::{Action as MenuAction, MenuPanel, Page, Toggle};
use preferences::{FontPreferences, Preferences};
use save_window::{Action as BrowserAction, Browser, Mode as BrowserMode, SaveWindow};

pub type Reopen = Box<dyn Fn() -> Result<Game>>;
pub fn run(
    mut game: Game,
    reopen: Reopen,
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
    native_saves.import_progress(&mut game);
    let mut app = App::new(game, quit_after_ms, native_saves);
    app.read_log = read_log;
    app.reopen = Some(reopen);
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
    panel_frame: Option<rugp_runtime::render::Framebuffer>,
    panel_font: Option<rugp_runtime::font::TextFont>,
    /// The 9pt system menu font of the retail popup.
    menu_font: Option<rugp_runtime::font::TextFont>,
    panel_audio: Vec<rugp_runtime::audio_state::snapshot::PlaybackResume>,
    /// The browser's last chosen file.
    slot: u32,
    native_saves: native_store::NativeStore,
    saves: save_store::SaveStore,
    browser: Option<SaveWindow>,
    /// Creating a window needs the event loop; see `config_requested`.
    browser_requested: Option<Browser>,
    menu: Option<MenuPanel>,
    bar: menu_bar::MenuBar,
    /// Reopens the game for ゲームの一番最初に戻る.
    reopen: Option<Reopen>,
    /// Alt went down with no other key since; its release opens the menu bar.
    alt_tap: bool,
    config: Option<ConfigWindow>,
    /// Creating a window needs the event loop, so opening is deferred to `about_to_wait`.
    config_requested: bool,
    /// The property page a 設定 menu item opens.
    config_page: ConfigPage,
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
        let saves = save_store::SaveStore::new(game.user_data_dir());
        Self {
            saves,
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
            panel_frame: None,
            panel_font: None,
            menu_font: None,
            panel_audio: Vec::new(),
            slot: 1,
            native_saves,
            browser: None,
            browser_requested: None,
            menu: None,
            bar: menu_bar::MenuBar::default(),
            reopen: None,
            alt_tap: false,
            config: None,
            config_requested: false,
            config_page: ConfigPage::Screen,
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
    /// Game pixels plus, in a window, the menu bar above them.
    fn canvas_size(&self) -> (u32, u32) {
        let (width, height) = self.game.size();
        (width, height + bar_offset(self.preferences.fullscreen))
    }
    fn viewport(&self) -> Option<Viewport> {
        let size = self.window.as_ref()?.surface_size();
        let (width, height) = self.canvas_size();
        Viewport::fit(size.width, size.height, width, height)
    }
    fn canvas_point(&self, x: f64, y: f64) -> Option<(i32, i32)> {
        self.viewport()?.point(x, y)
    }
    /// Logical game coordinates, or (-1, -1) outside the game.
    fn point(&self, x: f64, y: f64) -> (i32, i32) {
        let offset = bar_offset(self.preferences.fullscreen) as i32;
        match self.canvas_point(x, y) {
            Some((x, y)) if y >= offset => (x, y - offset),
            _ => (-1, -1),
        }
    }
    fn bar_visible(&self) -> bool {
        !self.preferences.fullscreen || self.bar.reveal || self.bar_open().is_some()
    }
    /// The menu-bar title whose dropdown is showing.
    fn bar_open(&self) -> Option<usize> {
        match self.menu.as_ref()?.page {
            Page::Bar(index) => Some(index),
            _ => None,
        }
    }
    /// The bar title under a window position, when the bar is showing.
    fn bar_hit(&self, x: f64, y: f64) -> Option<Option<usize>> {
        let (x, y) = self.canvas_point(x, y)?;
        (self.bar_visible() && y < menu_bar::HEIGHT).then(|| self.bar.hit(x, y))
    }
    fn pointer_moved(&mut self, x: f64, y: f64) -> Result<()> {
        // Fullscreen hides the bar until the pointer touches the top edge.
        if self.preferences.fullscreen {
            match self.canvas_point(x, y) {
                Some((_, y)) if y <= 1 => self.bar.reveal = true,
                Some((_, y)) if y < menu_bar::HEIGHT => {}
                _ => self.bar.reveal = false,
            }
        }
        self.bar.hover = self.bar_hit(x, y).flatten();
        // Win32 follows the pointer from one open dropdown to another title.
        if let (Some(open), Some(hover)) = (self.bar_open(), self.bar.hover)
            && open != hover
            && hover != menu_bar::QUIT
        {
            return self.open_bar(hover);
        }
        let point = self.point(x, y);
        if let Some(menu) = &mut self.menu {
            menu.pointer(point.0, point.1);
            self.refresh_panel()
        } else if !self.modal_open() {
            self.move_pointer(x, y)
        } else {
            Ok(())
        }
    }
    /// A left press or release on the menu bar, if it is one.
    fn bar_button(&mut self, pressed: bool) -> Option<Result<()>> {
        let (x, y) = self.cursor?;
        let hit = self.bar_hit(x, y);
        if pressed {
            let index = hit??;
            self.bar.pressed = true;
            if self.config_open() || self.browser_open() {
                if let Some(config) = &self.config {
                    config.focus();
                }
                if let Some(browser) = &self.browser {
                    browser.focus();
                }
                return Some(Ok(()));
            }
            if let Some(menu) = &mut self.menu {
                menu.cancel_press();
            }
            return Some(match index {
                menu_bar::QUIT => Ok(()),
                index if self.bar_open() == Some(index) => self.close_panel(),
                index => self.open_bar(index),
            });
        }
        if !std::mem::take(&mut self.bar.pressed) {
            return None;
        }
        Some(
            if hit.flatten() == Some(menu_bar::QUIT) && !self.config_open() && !self.browser_open()
            {
                self.open_menu(Page::Quit, None)
            } else {
                Ok(())
            },
        )
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
        } else if let Some(menu) = &mut self.menu {
            self.panel_frame = Some(menu.render(
                self.game.frame(),
                self.game.size(),
                self.menu_font.as_mut().context("menu font missing")?,
                self.game.title(),
            )?);
        }
        Ok(())
    }
    fn modal_open(&self) -> bool {
        self.browser_open()
            || self.menu.is_some()
            || self.config_open()
            || self.text_hidden
            || self.history.opened
    }
    fn browser_open(&self) -> bool {
        self.browser.is_some() || self.browser_requested.is_some()
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
        self.close_browser();
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
        self.close_browser();
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
        let Some(viewport) = self.viewport() else {
            return Ok(());
        };
        let offset = bar_offset(self.preferences.fullscreen) as i32;
        let (wx, wy) = viewport.window_point(x, y + offset);
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
        let height = height + bar_offset(preferences.fullscreen);
        let scale = u32::from(preferences.window_scale);
        PhysicalSize::new(width * scale / 100, height * scale / 100)
    }
    fn ensure_menu_font(&mut self) -> Result<()> {
        if self.panel_font.is_none() {
            self.panel_font = Some(self.game.interface_font(20)?);
        }
        if self.menu_font.is_none() {
            self.menu_font = Some(self.panel_font.as_ref().unwrap().resized(12)?);
        }
        Ok(())
    }
    fn pause_game(&mut self) -> Result<()> {
        self.controls.stop();
        self.right_pressed = false;
        if self.panel_font.is_none() {
            self.panel_font = Some(self.game.interface_font(20)?);
        }
        self.ensure_menu_font()?;
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
    /// Popups open at the cursor, or at `anchor` when given.
    fn open_menu(&mut self, page: Page, anchor: Option<(i32, i32)>) -> Result<()> {
        let anchor = anchor.unwrap_or_else(|| {
            let (width, height) = self.game.size();
            match self.cursor.map(|(x, y)| self.point(x, y)) {
                Some((x, y)) if x >= 0 && y >= 0 => (x, y),
                _ => (width as i32 / 2, height as i32 / 2),
            }
        });
        let state = menu_panel::State {
            read_skip: self.preferences.read_skip,
            auto: self.controls.automatic(),
            save: self.game.can_save(),
            load: true,
            restart: self.reopen.is_some(),
            fullscreen: self.preferences.fullscreen,
            scale: self.preferences.window_scale,
            sound: [
                self.preferences.channels[0],
                self.preferences.channels[2],
                self.preferences.channels[1],
            ],
            mouse_follow: self.preferences.mouse_follow,
        };
        self.pause_game()?;
        self.close_browser();
        self.text_hidden = false;
        self.history.close();
        self.close_config();
        self.menu = Some(MenuPanel::new(page, anchor, state));
        self.refresh_panel()
    }
    /// ゲームの一番最初に戻る: reopen the game, keeping the session's settings.
    fn restart(&mut self) -> Result<()> {
        let Some(reopen) = &self.reopen else {
            return self.close_panel();
        };
        let mut game = reopen()?;
        self.native_saves.import_progress(&mut game);
        self.panel_audio.clear();
        self.close_panel()?;
        self.game = game;
        if let Some(audio) = &mut self.audio {
            audio.reset();
        }
        self.audio_tickets.clear();
        self.audio_resources = None;
        self.shown_page = None;
        self.history = history::History::default();
        self.controls.stop();
        self.apply_preferences(self.preferences.clone())?;
        self.last = Instant::now();
        Ok(())
    }
    fn open_config(&mut self) -> Result<()> {
        self.pause_game()?;
        self.close_browser();
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
            self.config_page,
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
            return self.open_menu(Page::Context, None);
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
            MenuAction::Save => self.open_browser(BrowserMode::Save),
            MenuAction::Load => self.open_browser(BrowserMode::Load),
            MenuAction::RestartPrompt => self.open_menu(Page::Restart, None),
            MenuAction::Restart => self.restart(),
            MenuAction::MenuBar => self.open_bar(0),
            MenuAction::BarStep(step) => {
                let count = menu_bar::TITLES.len() as isize;
                let mut next = self.bar_open().unwrap_or(0) as isize;
                loop {
                    next = (next + step).rem_euclid(count);
                    if next as usize != menu_bar::QUIT {
                        break;
                    }
                }
                self.open_bar(next as usize)
            }
            MenuAction::Fullscreen(fullscreen) => self.change_preferences(|p| {
                p.fullscreen = fullscreen;
            }),
            MenuAction::Scale(scale) => self.change_preferences(|p| {
                p.fullscreen = false;
                p.window_scale = scale;
            }),
            MenuAction::Toggle(Toggle::ReadSkip) => {
                self.close_panel()?;
                self.set_read_skip(!self.preferences.read_skip)
            }
            MenuAction::Toggle(toggle) => self.change_preferences(|p| match toggle {
                Toggle::Bgm => p.channels[0] ^= true,
                Toggle::Voice => p.channels[1] ^= true,
                Toggle::Effects => p.channels[2] ^= true,
                Toggle::MouseFollow => p.mouse_follow ^= true,
                Toggle::ReadSkip => {}
            }),
            MenuAction::Config(page) => {
                self.config_page = page;
                if let Some(config) = &mut self.config {
                    config.show_page(page);
                }
                self.open_config()
            }
            MenuAction::Help(simple) => {
                self.close_panel()?;
                let root = self.game.archive.volumes[0]
                    .path
                    .parent()
                    .context("game root missing")?;
                let document = root.join(if simple { "Manual.txt" } else { "index.htm" });
                if let Err(error) = open_document(&document) {
                    eprintln!("Help: {error:#}");
                }
                Ok(())
            }
            MenuAction::About => self.open_menu(Page::About, None),
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
    /// Drop a menu-bar menu under its title.
    fn open_bar(&mut self, index: usize) -> Result<()> {
        let top = if self.preferences.fullscreen {
            menu_bar::HEIGHT
        } else {
            0
        };
        self.open_menu(Page::Bar(index), Some((self.bar.left(index), top)))
    }
    /// A menu-bar switch: persist it like a settings change.
    fn change_preferences(&mut self, change: impl FnOnce(&mut Preferences)) -> Result<()> {
        self.close_panel()?;
        let mut preferences = self.preferences.clone();
        change(&mut preferences);
        if let Err(error) = preferences.save(self.game.user_data_dir()) {
            eprintln!("Settings: {error:#}");
        }
        self.apply_preferences(preferences)
    }
    fn open_browser(&mut self, mode: BrowserMode) -> Result<()> {
        if mode == BrowserMode::Save && !self.game.can_save() {
            return Ok(());
        }
        if let Some(browser) = &self.browser
            && browser.mode() == mode
        {
            browser.focus();
            return Ok(());
        }
        let entries = self.saves.list(&self.native_saves)?;
        self.remember_dialogue();
        let comment = save_store::default_comment(&self.game, self.history.last_page());
        self.pause_game()?;
        self.close_config();
        self.close_browser();
        self.menu = None;
        self.text_hidden = false;
        self.history.close();
        self.panel_frame = None;
        self.browser_requested = Some(Browser::new(
            mode,
            entries,
            self.slot,
            self.preferences.system.double_click_lists,
            comment,
        ));
        Ok(())
    }
    fn create_browser(&mut self, event_loop: &dyn ActiveEventLoop) -> Result<()> {
        let Some(browser) = self.browser_requested.take() else {
            return Ok(());
        };
        let font = self
            .panel_font
            .as_ref()
            .context("save browser font missing")?;
        self.browser = Some(SaveWindow::open(event_loop, font.face(), browser)?);
        Ok(())
    }
    fn close_browser(&mut self) {
        self.browser = None;
        self.browser_requested = None;
    }
    fn browser_action(&mut self, action: BrowserAction) -> Result<()> {
        let result = match &action {
            BrowserAction::None => return Ok(()),
            BrowserAction::Close => return self.close_panel(),
            BrowserAction::Save { slot, comment } => self.write_save(*slot, comment.clone()),
            BrowserAction::Load { slot, kind } => self.load_save(*slot, *kind),
            BrowserAction::Comment { slot, comment } => self.saves.set_comment(*slot, comment),
            BrowserAction::Important { slot, important } => {
                self.saves.set_important(*slot, *important)
            }
            BrowserAction::Delete(slot) => self.saves.delete(*slot),
        };
        if let Err(error) = result {
            eprintln!("Save browser: {error:#}");
            let message = match action {
                BrowserAction::Save { .. } => {
                    format!("セーブに失敗しました。\n{error}")
                }
                BrowserAction::Load { .. } => {
                    format!("ロードに失敗しました。現在のゲームは変更されていません。\n{error}")
                }
                _ => format!("セーブファイルを正しく開けませんでした。\n{error}"),
            };
            if let Some(browser) = &mut self.browser {
                browser.browser().show_message(message);
            }
            self.last = Instant::now();
            return Ok(());
        }
        match action {
            BrowserAction::Save { slot, .. } | BrowserAction::Load { slot, .. } => {
                self.slot = slot;
                eprintln!("File {slot}: done");
                if matches!(action, BrowserAction::Load { .. }) {
                    self.apply_preferences(self.preferences.clone())?;
                    self.history.forget_cursor();
                    self.shown_page = None;
                }
                self.close_panel()
            }
            _ => {
                let entries = self.saves.list(&self.native_saves)?;
                if let Some(browser) = &mut self.browser {
                    browser.browser().set_entries(entries);
                }
                Ok(())
            }
        }
    }
    fn write_save(&mut self, slot: u32, comment: String) -> Result<()> {
        anyhow::ensure!(self.game.can_save(), "saving is not available now");
        let snapshot = self.game.snapshot()?;
        let header = save_store::header(&self.game, comment);
        self.saves.save(slot, &header, &snapshot)
    }
    fn load_save(&mut self, slot: u32, kind: save_store::Kind) -> Result<()> {
        let resources = self
            .audio_resources
            .as_mut()
            .context("load audio resources missing")?;
        self.panel_audio = match kind {
            save_store::Kind::Port => {
                let snapshot = self.saves.load(slot)?;
                self.game.restore_snapshot(&snapshot, resources)?
            }
            save_store::Kind::Original => self.game.import_uuc(
                &self.native_saves.path(slot),
                &self.native_saves.choices,
                resources,
            )?,
        };
        Ok(())
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
        self.close_browser();
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
    /// The game or modal frame with the menu bar above or over it.
    fn compose(&mut self) -> Result<Option<Vec<u8>>> {
        if !self.bar_visible() {
            return Ok(None);
        }
        self.ensure_menu_font()?;
        let (width, _) = self.game.size();
        let open = self.bar_open();
        let strip = self
            .bar
            .render(self.menu_font.as_mut().unwrap(), width, open)?;
        let frame = self
            .panel_frame
            .as_ref()
            .map_or_else(|| self.game.frame(), |f| f.rgba.as_slice());
        let mut canvas = Vec::with_capacity(strip.rgba.len() + frame.len());
        if bar_offset(self.preferences.fullscreen) > 0 {
            canvas.extend_from_slice(&strip.rgba);
            canvas.extend_from_slice(frame);
        } else {
            canvas.extend_from_slice(frame);
            canvas[..strip.rgba.len()].copy_from_slice(&strip.rgba);
        }
        Ok(Some(canvas))
    }
    fn present(&mut self) -> Result<()> {
        if self.window.is_none() {
            return Ok(());
        }
        let canvas = self.compose()?;
        let (gw, gh) = self.canvas_size();
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
        let viewport = Viewport::fit(size.width, size.height, gw, gh)
            .context("invalid presentation viewport")?;
        let (width, height, left, top) = (
            viewport.width as usize,
            viewport.height as usize,
            viewport.left as usize,
            viewport.top as usize,
        );
        let frame = canvas.as_deref().unwrap_or_else(|| {
            self.panel_frame
                .as_ref()
                .map_or_else(|| self.game.frame(), |f| f.rgba.as_slice())
        });
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
        self.alt_tap = false;
        self.bar.hover = None;
        self.bar.pressed = false;
        self.bar.reveal = false;
        self.controls.stop();
        self.right_pressed = false;
        self.history.cancel_press();
        if let Some(menu) = &mut self.menu {
            menu.cancel_press();
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
/// Rows the window's menu bar takes above the game; fullscreen overlays it.
fn bar_offset(fullscreen: bool) -> u32 {
    if fullscreen {
        0
    } else {
        menu_bar::HEIGHT as u32
    }
}

/// ヘルプの参照: hand a disc document to the desktop's viewer.
fn open_document(path: &std::path::Path) -> Result<()> {
    anyhow::ensure!(path.is_file(), "{} not found", path.display());
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    let mut child = command.arg(path).spawn().context("start document viewer")?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

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
        if let Some(browser) = &mut self.browser
            && browser.id() == id
        {
            let action = if event == WindowEvent::RedrawRequested {
                browser.redraw()
            } else {
                Ok(browser.event(&event))
            };
            if let Err(error) = action.and_then(|action| self.browser_action(action)) {
                self.fail(error);
            }
            return;
        }
        if self.window.as_ref().is_none_or(|w| w.id() != id) {
            return;
        }
        if matches!(event, WindowEvent::PointerButton { .. }) {
            self.alt_tap = false;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::PointerMoved { position, .. } if self.error.is_none() => {
                self.cursor = Some((position.x, position.y));
                if let Err(error) = self.pointer_moved(position.x, position.y) {
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
                let result = if let Some(result) = self.bar_button(state == ElementState::Pressed) {
                    result
                } else if self.text_hidden {
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
                } else if self.menu.is_some() {
                    let (x, y) = self.cursor.map_or((-1, -1), |(x, y)| self.point(x, y));
                    let action =
                        self.menu
                            .as_mut()
                            .unwrap()
                            .button(x, y, state == ElementState::Pressed);
                    self.menu_action(action)
                } else if self.config_open() || self.browser_open() {
                    if let Some(config) = &self.config {
                        config.focus();
                    }
                    if let Some(browser) = &self.browser {
                        browser.focus();
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
                    PhysicalKey::Code(KeyCode::AltLeft | KeyCode::AltRight)
                ) =>
            {
                // A lone Alt tap shows the retail menu bar, here a popup at its place.
                if event.state == ElementState::Pressed {
                    self.alt_tap |= !event.repeat;
                } else if std::mem::take(&mut self.alt_tap)
                    && self.error.is_none()
                    && !self.modal_open()
                    && let Err(error) = self.open_bar(0)
                {
                    self.fail(error);
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if matches!(
                    event.physical_key,
                    PhysicalKey::Code(KeyCode::ControlLeft | KeyCode::ControlRight)
                ) =>
            {
                self.alt_tap = false;
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
                self.alt_tap = false;
                let result = match event.physical_key {
                    PhysicalKey::Code(KeyCode::F5) => self.open_browser(BrowserMode::Save),
                    PhysicalKey::Code(KeyCode::F9) => self.open_browser(BrowserMode::Load),
                    PhysicalKey::Code(KeyCode::F2) => {
                        if self.config_open() {
                            self.close_panel()
                        } else {
                            self.open_config()
                        }
                    }
                    PhysicalKey::Code(KeyCode::Escape) if self.config_open() => self.close_panel(),
                    _ if self.config_open() || self.browser_open() => Ok(()),
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
                    PhysicalKey::Code(KeyCode::Escape) => {
                        if self.game.video().is_some_and(|v| !v.finished()) {
                            self.game.skip_video().map(|_| ())
                        } else if self.controls.reading_skip() {
                            self.set_read_skip(false)
                        } else {
                            self.open_menu(Page::Context, None)
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
                        && let Err(e) = self.open_browser(BrowserMode::Load)
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
        if self.browser_requested.is_some()
            && self.error.is_none()
            && let Err(error) = self.create_browser(event_loop)
        {
            eprintln!("Save browser: {error:#}");
            if let Err(error) = self.close_panel() {
                self.fail(error);
            }
        }
        if let Some(config) = &self.config {
            config.wake(Instant::now());
        }
        if let Some(browser) = &self.browser {
            browser.wake(Instant::now());
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
    fn retail_story_pages_save_and_load_back() -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let directory = std::env::temp_dir().join(format!(
            "rugp-story-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let game = Game::open_persistent(root.as_ref(), &directory)?;
        let originals = Some(directory.join("Vmreg"));
        let native_saves = native_store::NativeStore::detect(&game, originals, vec![])?;
        let mut app = App::new(game, None, native_saves);
        app.apply_preferences(Preferences::default())?;
        let mut audio = None;
        let mut saved = Vec::new();
        // New Game, then save every few pages of the prologue.
        for page in 0..20_000 {
            if saved.len() == 6 {
                break;
            }
            if app.game.waiting_for_selection() {
                app.game.navigate_selection(1)?;
                app.game.activate_selection()?;
            } else if app.game.dialogue_waits_for_input() {
                if page % 7 == 0 {
                    let slot = saved.len() as u32 + 1;
                    let state = (app.game.vm.cursor().cloned(), app.game.frame().to_vec());
                    app.open_browser(BrowserMode::Save)?;
                    app.browser_action(BrowserAction::Save {
                        slot,
                        comment: format!("{slot}"),
                    })?;
                    assert!(!app.browser_open(), "save {slot} failed");
                    saved.push((slot, state));
                }
                app.game.activate_selection()?;
            } else if app.game.waiting_for_input() {
                app.game.activate_selection()?;
            }
            app.game.skip_video()?;
            app.game.step(100)?;
            rugp_runtime::audio::mute_requests(&mut app.game, &mut audio)?;
        }
        assert_eq!(saved.len(), 6);
        for (slot, (cursor, frame)) in saved {
            app.open_browser(BrowserMode::Load)?;
            app.browser_action(BrowserAction::Load {
                slot,
                kind: save_store::Kind::Port,
            })?;
            assert!(!app.browser_open(), "load {slot} failed");
            assert_eq!(app.game.vm.cursor(), cursor.as_ref());
            assert!(app.game.frame() == frame.as_slice());
            // A loaded page plays on.
            for _ in 0..30 {
                app.game.activate_selection()?;
                app.game.step(100)?;
                rugp_runtime::audio::mute_requests(&mut app.game, &mut audio)?;
            }
            assert_ne!(app.game.vm.cursor(), cursor.as_ref());
        }
        drop(app);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }
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
        let action = app.menu.as_mut().unwrap().key(KeyCode::KeyM);
        app.menu_action(action)?;
        assert!(app.menu.as_ref().is_some_and(|m| m.page == Page::Bar(0)));
        let action = app.menu.as_mut().unwrap().key(KeyCode::ArrowRight);
        app.menu_action(action)?;
        assert!(app.menu.as_ref().is_some_and(|m| m.page == Page::Bar(1)));
        let action = app.menu.as_mut().unwrap().key(KeyCode::KeyX);
        app.menu_action(action)?;
        assert_eq!(app.config_page, ConfigPage::Text);
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
            // The window's canvas: menu bar above the game and its popup.
            let canvas = app.compose()?.context("menu bar hidden")?;
            let (width, height) = app.canvas_size();
            let mut encoder = png::Encoder::new(std::fs::File::create(path)?, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header()?.write_image_data(&canvas)?;
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
        app.open_browser(BrowserMode::Load)?;
        assert!(app.browser_open());
        app.browser_action(BrowserAction::Load {
            slot: 1,
            kind: save_store::Kind::Original,
        })?;
        assert!(app.browser_open());
        assert_eq!(app.game.vm.cursor(), cursor.as_ref());
        assert_eq!(app.game.frame(), pixels);
        app.close_panel()?;
        assert!(!app.browser_open());
        // Reach the title selection, save it, start a game, then load it back.
        let mut audio = None;
        for _ in 0..4000 {
            if app.game.waiting_for_selection() {
                break;
            }
            app.game.skip_video()?;
            app.game.step(100)?;
            rugp_runtime::audio::mute_requests(&mut app.game, &mut audio)?;
        }
        assert!(app.game.waiting_for_selection());
        let saved_at = app.game.vm.cursor().cloned();
        let saved_frame = app.game.frame().to_vec();
        app.open_browser(BrowserMode::Save)?;
        app.browser_action(BrowserAction::Save {
            slot: 2,
            comment: "テスト".into(),
        })?;
        assert!(!app.browser_open());
        let entries = app.saves.list(&app.native_saves)?;
        let saved = entries
            .iter()
            .find(|e| e.slot == 2)
            .context("save missing")?;
        assert_eq!(
            (saved.kind, saved.comment.as_str(), saved.broken),
            (save_store::Kind::Port, "テスト", false)
        );
        assert!(entries.iter().any(|e| e.slot == 1 && e.broken));
        assert!(!saves.join("vmr00002.uuc").exists());
        assert_eq!(
            std::fs::read(saves.join("vmr00001.uuc"))?,
            b"damaged native save"
        );
        app.game.navigate_selection(1)?;
        for _ in 0..3 {
            app.game.activate_selection()?;
            for _ in 0..50 {
                app.game.step(100)?;
                rugp_runtime::audio::mute_requests(&mut app.game, &mut audio)?;
            }
        }
        assert_ne!(app.game.vm.cursor(), saved_at.as_ref());
        app.open_browser(BrowserMode::Load)?;
        app.browser_action(BrowserAction::Load {
            slot: 2,
            kind: save_store::Kind::Port,
        })?;
        assert!(!app.browser_open());
        assert_eq!(app.game.vm.cursor(), saved_at.as_ref());
        assert_eq!(app.game.frame(), saved_frame);
        drop(app);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }
}
