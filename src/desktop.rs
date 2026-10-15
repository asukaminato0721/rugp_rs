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
mod save_panel;
use save_panel::{Action as PanelAction, Mode as PanelMode, Panel};

pub fn run(game: Game, mute: bool, quit_after_ms: Option<u32>) -> Result<()> {
    let event_loop = EventLoop::new().context("create desktop event loop")?;
    let mut app = App::new(game, mute, quit_after_ms);
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
    error: Option<String>,
    mute: bool,
    quit_after_ms: Option<u32>,
    muted_audio: Option<rugp_runtime::audio::AudioResources>,
    cursor: Option<(f64, f64)>,
    panel: Option<Panel>,
    panel_frame: Option<rugp_runtime::render::Framebuffer>,
    panel_font: Option<rugp_runtime::font::TextFont>,
    panel_audio: Vec<rugp_runtime::audio_state::snapshot::PlaybackResume>,
    slot: u8,
    #[cfg(feature = "audio")]
    audio: Option<rugp_runtime::audio_device::DeviceAudio>,
    #[cfg(feature = "audio")]
    audio_tickets: std::collections::BTreeMap<u64, u64>,
}
impl App {
    fn new(game: Game, mute: bool, quit_after_ms: Option<u32>) -> Self {
        Self {
            game,
            mute,
            quit_after_ms,
            window: None,
            surface: None,
            last: Instant::now(),
            elapsed: 0,
            error: None,
            muted_audio: None,
            cursor: None,
            panel: None,
            panel_frame: None,
            panel_font: None,
            panel_audio: Vec::new(),
            slot: 0,
            #[cfg(feature = "audio")]
            audio: None,
            #[cfg(feature = "audio")]
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
        }
        Ok(())
    }
    fn open_panel(&mut self, mode: PanelMode) -> Result<()> {
        if self.game.waiting_for_load() && mode == PanelMode::Save {
            return Ok(());
        }
        let mut occupied = [false; 10];
        for info in self.game.save_slots()? {
            occupied[info.slot as usize] = true;
        }
        if self.panel_font.is_none() {
            self.panel_font = Some(self.game.interface_font(20)?);
        }
        if self.panel.is_none() {
            self.game.pointer_move(-1, -1)?;
            self.game.pointer_button(false)?;
            if self.muted_audio.is_none() {
                self.muted_audio = Some(rugp_runtime::audio::AudioResources::new(Arc::clone(
                    &self.game.archive,
                ))?);
            }
            // Pause device time with the VM. Closing resumes PCM/gain at the
            // unchanged virtual epoch, regardless of time spent in this UI.
            self.panel_audio = self
                .game
                .rebuild_audio_output(self.muted_audio.as_mut().unwrap())?;
            #[cfg(feature = "audio")]
            {
                if let Some(audio) = &mut self.audio {
                    audio.reset();
                }
                self.audio_tickets.clear();
            }
        }
        self.panel = Some(Panel::new(mode, self.slot, occupied));
        self.refresh_panel()
    }
    fn close_panel(&mut self) -> Result<()> {
        self.game.cancel_load_request()?;
        for playback in std::mem::take(&mut self.panel_audio) {
            if self.mute {
                continue;
            }
            #[cfg(feature = "audio")]
            {
                if self.audio.is_none() {
                    self.audio = Some(rugp_runtime::audio_device::DeviceAudio::open(Arc::clone(
                        &self.game.archive,
                    ))?);
                }
                if let Some(ticket) = self.audio.as_mut().unwrap().resume(&playback)? {
                    self.audio_tickets.insert(playback.ticket, ticket);
                } else {
                    self.game.finish_audio_ticket(playback.ticket);
                }
            }
            #[cfg(not(feature = "audio"))]
            {
                let _ = playback;
                bail!("device audio is not enabled in this build; use --features audio, or --mute");
            }
        }
        self.panel = None;
        self.panel_frame = None;
        self.last = Instant::now();
        if let Some((x, y)) = self.cursor {
            self.move_pointer(x, y)?;
        }
        Ok(())
    }
    fn panel_action(&mut self, action: PanelAction) -> Result<()> {
        let result = match action {
            PanelAction::None => return self.refresh_panel(),
            PanelAction::Cancel => return self.close_panel(),
            PanelAction::Save(slot) => {
                self.slot = slot;
                self.game.save_slot(slot)
            }
            PanelAction::Load(slot) => {
                self.slot = slot;
                self.game
                    .load_slot(
                        slot,
                        self.muted_audio
                            .as_mut()
                            .context("slot audio resources missing")?,
                    )
                    .map(|resumes| {
                        self.panel_audio = resumes;
                    })
            }
        };
        if let Err(error) = result {
            eprintln!("Slot {}: {error:#}", self.slot);
            if let Some(panel) = &mut self.panel {
                panel.notice = if panel.mode == PanelMode::Save && !self.game.script_save_enabled()
                {
                    "Saving is disabled here by the game script."
                } else {
                    "Operation failed; current game is unchanged. See terminal."
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
        self.close_panel()
    }
    fn sounds(&mut self) -> Result<()> {
        if self.mute {
            return rugp_runtime::audio::mute_requests(&mut self.game, &mut self.muted_audio);
        }
        for request in self.game.take_sounds() {
            #[cfg(feature = "audio")]
            {
                if self.audio.is_none() {
                    if request.source.is_none() && [254, 255].contains(&request.kind) {
                        continue;
                    }
                    self.audio = Some(rugp_runtime::audio_device::DeviceAudio::open(Arc::clone(
                        &self.game.archive,
                    ))?);
                }
                let runtime_ticket = request.ticket;
                if let Some(ticket) = self.audio.as_mut().unwrap().request(request)? {
                    self.audio_tickets.insert(
                        runtime_ticket.context("play request lacks runtime identity")?,
                        ticket,
                    );
                }
            }
            #[cfg(not(feature = "audio"))]
            if request.source.is_some() {
                bail!("device audio is not enabled in this build; use --features audio, or --mute");
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
        #[cfg(feature = "audio")]
        {
            self.audio = None;
            self.audio_tickets.clear();
        }
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
                if self.panel.is_none()
                    && let Err(error) = self.move_pointer(position.x, position.y)
                {
                    self.fail(error);
                }
            }
            WindowEvent::CursorLeft { .. } | WindowEvent::Focused(false)
                if self.error.is_none() =>
            {
                self.cursor = None;
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
                } else {
                    self.game.pointer_button(state == ElementState::Pressed)
                };
                if let Err(error) = result {
                    self.fail(error);
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
                    PhysicalKey::Code(key) if self.panel.is_some() => {
                        let action = self.panel.as_mut().unwrap().key(key);
                        self.panel_action(action)
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
                let dt = now.duration_since(self.last).as_millis().min(100) as u32;
                self.last = now;
                if self.error.is_none() && self.panel.is_none() {
                    #[cfg(feature = "audio")]
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
                    match self.game.step(dt) {
                        Ok(RunState::Ended) => event_loop.exit(),
                        Ok(_) => {}
                        Err(e) => self.fail(e),
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
                    && self.panel.is_none()
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
    fn retail_desktop_chooser_saves_loads_cancels_and_survives_corrupt_slots_without_click_through()
    -> Result<()> {
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let directory = std::env::temp_dir().join(format!(
            "rugp-desktop-slot-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let mut game = Game::open_persistent(root.as_ref(), &directory)?;
        let mut audio = None;
        let mut title = false;
        for _ in 0..300 {
            game.step(1000)?;
            rugp_runtime::audio::mute_requests(&mut game, &mut audio)?;
            if game.waiting_for_dialogue() {
                break;
            }
            if game.waiting_for_selection() && !title {
                title = true;
                game.navigate_selection(1)?;
                game.activate_selection()?;
            }
        }
        anyhow::ensure!(title && game.waiting_for_dialogue(), "story not reached");
        let mut app = App::new(game, true, None);
        let cursor = app.game.vm.cursor().cloned();
        let pixels = app.game.frame().to_vec();
        let navigation = app.game.navigation.clone();
        app.open_panel(PanelMode::Save)?;
        assert!(app.panel_frame.is_some());
        let action = app.panel.as_mut().unwrap().key(KeyCode::Enter);
        assert_eq!(action, PanelAction::Save(0));
        app.panel_action(action)?;
        assert!(app.panel.is_none());
        assert!(app.error.is_none());
        let slot = app.game.user_data_dir().unwrap().join("slot-0.rugpsave");
        let saved = std::fs::read(&slot)?;
        app.game.navigation = "after-save marker".into();
        app.game.activate_selection()?;
        app.game.step(33)?;
        app.open_panel(PanelMode::Load)?;
        let action = app.panel.as_mut().unwrap().key(KeyCode::Enter);
        app.panel_action(action)?;
        assert!(app.panel.is_none());
        assert_eq!(app.game.vm.cursor(), cursor.as_ref());
        assert!(app.game.frame() == pixels);
        assert_eq!(app.game.navigation, navigation);
        app.game.pointer_button(false)?;
        assert_eq!(app.game.vm.cursor(), cursor.as_ref());
        assert!(app.game.waiting_for_dialogue());
        app.open_panel(PanelMode::Save)?;
        let action = app.panel.as_mut().unwrap().key(KeyCode::Enter);
        assert_eq!(action, PanelAction::None);
        app.panel_action(action)?;
        assert!(std::fs::read(&slot)? == saved);
        app.panel_action(PanelAction::Cancel)?;
        std::fs::write(&slot, b"corrupted test save")?;
        app.open_panel(PanelMode::Load)?;
        app.panel_action(PanelAction::Load(0))?;
        assert!(app.panel.is_some() && app.error.is_none());
        assert!(!app.panel.as_ref().unwrap().notice.is_empty());
        assert_eq!(app.game.vm.cursor(), cursor.as_ref());
        assert!(app.game.frame() == pixels);
        app.panel_action(PanelAction::Cancel)?;
        assert!(app.panel.is_none() && app.error.is_none());
        let game_data = app.game.user_data_dir().unwrap().to_owned();
        drop(app);
        std::fs::remove_file(slot)?;
        for entry in std::fs::read_dir(&game_data)? {
            std::fs::remove_file(entry?.path())?;
        }
        std::fs::remove_dir(game_data)?;
        std::fs::remove_dir(directory)?;
        Ok(())
    }
}
