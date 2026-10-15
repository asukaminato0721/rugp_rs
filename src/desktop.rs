//! Desktop presentation host; all game/render state stays in rugp_runtime.
use anyhow::{Context, Result, bail};
use rugp_runtime::{Game, RunState};
use std::{
    num::NonZeroU32,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

pub fn run(game: Game, mute: bool, quit_after_ms: Option<u32>) -> Result<()> {
    let event_loop = EventLoop::new().context("create desktop event loop")?;
    let mut app = App {
        game,
        window: None,
        surface: None,
        last: Instant::now(),
        elapsed: 0,
        error: None,
        mute,
        quit_after_ms,
        muted_audio: None,
        #[cfg(feature = "audio")]
        audio: None,
        #[cfg(feature = "audio")]
        audio_wait: None,
    };
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
    #[cfg(feature = "audio")]
    audio: Option<rugp_runtime::audio_device::DeviceAudio>,
    #[cfg(feature = "audio")]
    audio_wait: Option<u64>,
}
impl App {
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
                if let Some(ticket) = self.audio.as_mut().unwrap().request(request)? {
                    self.audio_wait = Some(ticket);
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
        let width = (size.width as u64)
            .min(size.height as u64 * gw as u64 / gh as u64)
            .max(1) as usize;
        let height = (width as u64 * gh as u64 / gw as u64).max(1) as usize;
        let left = (size.width as usize - width) / 2;
        let top = (size.height as usize - height) / 2;
        let frame = self.game.frame();
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
        #[cfg(feature = "audio")]
        {
            self.audio = None;
            self.audio_wait = None;
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
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = now.duration_since(self.last).as_millis().min(100) as u32;
                self.last = now;
                if self.error.is_none() {
                    #[cfg(feature = "audio")]
                    if let Some(ticket) = self.audio_wait
                        && self.audio.as_ref().is_some_and(|a| a.finished(ticket))
                    {
                        self.audio_wait = None;
                        if let Err(e) = self.game.finish_audio() {
                            self.fail(e);
                        }
                    }
                    match self.game.step(dt) {
                        Ok(RunState::Ended) => event_loop.exit(),
                        Ok(_) => {}
                        Err(e) => self.fail(e),
                    }
                    if let Err(e) = self.sounds() {
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
