//! Kira output adapter, separated from resource decoding and game scheduling.
use crate::{
    audio::{AudioResources, SoundData},
    game::SoundRequest,
};
use anyhow::{Context, Result, ensure};
use kira::{
    Frame,
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    Bgm,
    Effect,
    Voice,
}
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
        let sound = StaticSoundData {
            sample_rate: data.sample_rate,
            frames: frames.into(),
            settings,
            slice: None,
        };
        let handle = self
            .manager
            .play(sound)
            .map_err(|e| anyhow::anyhow!("queue audio: {e}"))?;
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
            request.kind == 2 && request.flags & !1 == 0,
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
        let ticket = self.mixer.play(offset, bus, &data, looped)?;
        Ok(if request.flags & 1 != 0 {
            Some(ticket)
        } else {
            None
        })
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
}
