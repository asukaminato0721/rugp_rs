//! Kira output adapter, separated from resource decoding and game scheduling.
use crate::{
    audio::{AudioResources, SoundData},
    audio_envelope::GainEnvelope,
    game::SoundRequest,
};
use anyhow::{Context, Result, ensure};
use kira::{
    Frame, Volume,
    manager::{
        AudioManager, AudioManagerSettings,
        backend::{Backend, DefaultBackend},
    },
    sound::{
        PlaybackState,
        static_sound::{StaticSoundData, StaticSoundHandle, StaticSoundSettings},
    },
    tween::Tween,
};
use rugp_assets::{RioArchive, sound::Sample};
use std::{collections::BTreeMap, sync::Arc};

pub use crate::audio_state::Bus;
struct Playing {
    source: u64,
    bus: Bus,
    handle: StaticSoundHandle,
}
pub struct Mixer<B: Backend> {
    manager: AudioManager<B>,
    playing: BTreeMap<u64, Playing>,
    next: u64,
}
impl<B: Backend> Mixer<B> {
    pub fn new(manager: AudioManager<B>) -> Self {
        Self {
            manager,
            playing: BTreeMap::new(),
            next: 1,
        }
    }
    pub fn play(&mut self, source: u64, bus: Bus, data: &SoundData, looped: bool) -> Result<u64> {
        self.play_with_envelope(source, bus, data, looped, GainEnvelope::UNITY)
    }
    pub fn play_with_envelope(
        &mut self,
        source: u64,
        bus: Bus,
        data: &SoundData,
        looped: bool,
        envelope: GainEnvelope,
    ) -> Result<u64> {
        ensure!(
            [1, 2].contains(&data.channels)
                && data.pcm.len().is_multiple_of(data.channels as usize),
            "unsupported output channel layout"
        );
        self.playing
            .retain(|_, v| v.handle.state() != PlaybackState::Stopped);
        if bus != Bus::Effect {
            self.stop_bus(bus);
        }
        let mut frames = Vec::with_capacity(data.pcm.len() / data.channels as usize);
        for samples in data.pcm.chunks(data.channels as usize) {
            let left = samples[0] as f32 / 32768.0;
            let right = samples.get(1).map_or(left, |v| *v as f32 / 32768.0);
            frames.push(Frame::new(left, right));
        }
        let settings = if looped {
            StaticSoundSettings::default().loop_region(..)
        } else {
            StaticSoundSettings::default()
        };
        let settings = settings.volume(Volume::Amplitude(envelope.start_amplitude()));
        let sound = StaticSoundData {
            sample_rate: data.sample_rate,
            frames: frames.into(),
            settings,
            slice: None,
        };
        let mut handle = self
            .manager
            .play(sound)
            .map_err(|e| anyhow::anyhow!("queue audio: {e}"))?;
        if envelope.duration_ms() > 0 {
            handle.set_volume(
                Volume::Amplitude(envelope.end_amplitude()),
                Tween {
                    duration: std::time::Duration::from_millis(u64::from(envelope.duration_ms())),
                    easing: kira::tween::Easing::Linear,
                    ..Tween::default()
                },
            );
        }
        let ticket = self.next;
        self.next = self.next.checked_add(1).context("sound ticket overflow")?;
        self.playing.insert(
            ticket,
            Playing {
                source,
                bus,
                handle,
            },
        );
        Ok(ticket)
    }
    pub fn finished(&self, ticket: u64) -> bool {
        self.playing
            .get(&ticket)
            .is_none_or(|p| p.handle.state() == PlaybackState::Stopped)
    }
    pub fn stop_all(&mut self) {
        for (_, mut playing) in std::mem::take(&mut self.playing) {
            playing.handle.stop(Tween::default());
        }
    }
    pub fn stop_bus(&mut self, bus: Bus) {
        self.playing.retain(|_, p| {
            if p.bus == bus {
                p.handle.stop(Tween::default());
                false
            } else {
                true
            }
        });
    }
    pub fn stop_source(&mut self, source: u64) {
        self.playing.retain(|_, p| {
            if p.source == source {
                p.handle.stop(Tween::default());
                false
            } else {
                true
            }
        });
    }
}
pub struct DeviceAudio {
    mixer: Mixer<DefaultBackend>,
    resources: AudioResources,
    archive: Arc<RioArchive>,
}
impl DeviceAudio {
    pub fn open(archive: Arc<RioArchive>) -> Result<Self> {
        let manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())
            .map_err(|e| anyhow::anyhow!("open audio device: {e}"))?;
        let resources = AudioResources::new(Arc::clone(&archive))?;
        Ok(Self {
            mixer: Mixer::new(manager),
            resources,
            archive,
        })
    }
    pub fn request(&mut self, request: SoundRequest) -> Result<Option<u64>> {
        let envelope = if [2, 4].contains(&request.kind) {
            Some(request.gain_envelope()?)
        } else {
            None
        };
        let Some(source) = request.source else {
            match request.kind {
                255 => self.mixer.stop_all(),
                254 => self.mixer.stop_bus(Bus::Effect),
                other => anyhow::bail!("unimplemented global sound operation {other}"),
            };
            return Ok(None);
        };
        let offset = source.offset.context("sound source offset missing")?;
        if request.kind == 3 {
            self.mixer.stop_source(offset);
            return Ok(None);
        }
        ensure!(
            [2, 4].contains(&request.kind),
            "unimplemented sound mode {} flags {:#x}",
            request.kind,
            request.flags
        );
        let class = source
            .class
            .as_ref()
            .context("sound source class missing")?;
        let bus = if class.name == "CPmBgm" {
            Bus::Bgm
        } else {
            Bus::Effect
        };
        let looped = if ["CPmBgm", "CPmEffect"].contains(&class.name.as_str()) {
            let bytes = self
                .archive
                .read_at(offset, source.size.context("sound size missing")? as usize)?;
            Sample::parse(&bytes, self.archive.address_shift)?.mode & 1 != 0
        } else {
            false
        };
        ensure!(
            !looped || request.flags & 1 == 0,
            "cannot wait for a looping sound"
        );
        let data = self.resources.load(&source)?;
        let ticket = self.mixer.play_with_envelope(
            offset,
            bus,
            &data,
            looped,
            envelope.context("playback envelope missing")?,
        )?;
        // Every playback needs completion reporting, including asynchronous
        // sounds that a later CVmSync may query. VM waiting is runtime-owned.
        Ok(Some(ticket))
    }
    pub fn finished(&self, ticket: u64) -> bool {
        self.mixer.finished(ticket)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kira::manager::backend::mock::{MockBackend, MockBackendSettings};
    #[test]
    fn mock_output_and_stop_are_real_audio_commands() -> Result<()> {
        let settings = AudioManagerSettings::<MockBackend> {
            backend_settings: MockBackendSettings { sample_rate: 44100 },
            ..Default::default()
        };
        let manager =
            AudioManager::<MockBackend>::new(settings).map_err(|_| anyhow::anyhow!("mock init"))?;
        let mut mixer = Mixer::new(manager);
        let data = SoundData {
            sample_rate: 44100,
            channels: 1,
            pcm: vec![16384; 44100],
        };
        let ticket = mixer.play(1, Bus::Voice, &data, false)?;
        assert!(!mixer.finished(ticket));
        mixer.manager.backend_mut().on_start_processing();
        let mut peak = 0.0f32;
        for _ in 0..256 {
            let f = mixer.manager.backend_mut().process();
            peak = peak.max(f.left.abs()).max(f.right.abs());
        }
        assert!(peak > 0.1);
        mixer.stop_all();
        assert!(mixer.finished(ticket));
        Ok(())
    }
    #[test]
    fn natural_completion_reports_async_playbacks_but_not_looping_audio() -> Result<()> {
        let manager = AudioManager::<MockBackend>::new(AudioManagerSettings {
            backend_settings: MockBackendSettings { sample_rate: 44100 },
            ..Default::default()
        })
        .map_err(|_| anyhow::anyhow!("mock init"))?;
        let mut mixer = Mixer::new(manager);
        let data = SoundData {
            sample_rate: 44100,
            channels: 1,
            pcm: vec![8192; 441],
        };
        let short = mixer.play_with_envelope(
            7,
            Bus::Effect,
            &data,
            false,
            GainEnvelope::from_native([0, 4096, 1000])?,
        )?;
        let looping = mixer.play(8, Bus::Effect, &data, true)?;
        mixer.manager.backend_mut().on_start_processing();
        for _ in 0..8192 {
            let _ = mixer.manager.backend_mut().process();
        }
        assert!(mixer.finished(short));
        assert!(!mixer.finished(looping));
        mixer.stop_source(8);
        assert!(mixer.finished(looping));
        Ok(())
    }
    #[test]
    fn linear_amplitude_envelopes_survive_pcm_loops_and_zero_duration_uses_start() -> Result<()> {
        fn render(envelope: GainEnvelope, looped: bool) -> Result<Vec<f32>> {
            let manager = AudioManager::<MockBackend>::new(AudioManagerSettings {
                backend_settings: MockBackendSettings { sample_rate: 1000 },
                ..Default::default()
            })
            .map_err(|_| anyhow::anyhow!("mock init"))?;
            let mut mixer = Mixer::new(manager);
            let data = SoundData {
                sample_rate: 1000,
                channels: 1,
                pcm: vec![8192; if looped { 32 } else { 1000 }],
            };
            let id = mixer.play_with_envelope(1, Bus::Effect, &data, looped, envelope)?;
            mixer.manager.backend_mut().on_start_processing();
            let frames = (0..250)
                .map(|_| mixer.manager.backend_mut().process().left)
                .collect();
            assert!(!mixer.finished(id)); // ramp reaching zero does not stop playback
            Ok(frames)
        }
        let unity = render(GainEnvelope::UNITY, true)?;
        let rising = render(GainEnvelope::from_native([0, 4096, 100])?, true)?;
        let offset = render(GainEnvelope::from_native([1024, 3072, 100])?, false)?;
        let zero = render(GainEnvelope::from_native([1024, 4096, 0])?, true)?;
        let falling = render(GainEnvelope::from_native([4096, 0, 100])?, false)?;
        for at in [10, 25, 50, 75] {
            let expected = at as f32 / 100.0;
            assert!(
                (rising[at] / unity[at] - expected).abs() < 0.03,
                "sample {at}: {}",
                rising[at] / unity[at]
            );
            assert!((offset[at] / unity[at] - (0.25 + expected * 0.5)).abs() < 0.03);
            assert!((falling[at] / unity[at] - (1.0 - expected)).abs() < 0.03);
        }
        for at in [110, 150, 200] {
            assert!((rising[at] / unity[at] - 1.0).abs() < 0.001);
            assert!((zero[at] / unity[at] - 0.25).abs() < 0.001);
            assert!(falling[at].abs() < 0.001);
        }
        Ok(())
    }
}
