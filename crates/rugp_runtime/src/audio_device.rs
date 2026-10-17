//! Kira output adapter, separated from resource decoding and game scheduling.
use crate::{
    audio::{AudioResources, SoundData},
    audio_envelope::GainEnvelope,
    audio_spatial::StereoGains,
    audio_state::snapshot::PlaybackResume,
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
        PlaybackPosition, PlaybackState,
        static_sound::{StaticSoundData, StaticSoundHandle, StaticSoundSettings},
    },
    track::{TrackBuilder, TrackHandle},
    tween::Tween,
};
use rugp_assets::{RioArchive, sound::Sample};
use std::{collections::BTreeMap, sync::Arc};

pub use crate::audio_state::Bus;
struct Playing {
    source: u64,
    bus: Bus,
    handle: StaticSoundHandle,
    gain_scale: f64,
}
struct PlaybackStart {
    position: usize,
    envelope: GainEnvelope,
    gain_scale: f64,
    stop_after_ms: Option<u32>,
    stereo: StereoGains,
}
/// BGM gain while a voice plays when ducking is on. The retail setting
/// (CSoundManEx bSmallBgmInPlayVoice) exists; its native factor is not ported,
/// so this is a -6 dB approximation.
const DUCKED_BGM: f64 = 0.5;
const DUCK_TWEEN_MS: u64 = 300;

pub struct Mixer<B: Backend> {
    manager: AudioManager<B>,
    tracks: [TrackHandle; 3],
    playing: BTreeMap<u64, Playing>,
    next: u64,
    bgm_level: f64,
    duck_bgm: bool,
    ducked: bool,
}
impl<B: Backend> Mixer<B> {
    pub fn new(mut manager: AudioManager<B>) -> Result<Self> {
        let mut track = || {
            manager
                .add_sub_track(TrackBuilder::new())
                .map_err(|e| anyhow::anyhow!("create audio channel: {e}"))
        };
        let tracks = [track()?, track()?, track()?];
        Ok(Self {
            manager,
            tracks,
            playing: BTreeMap::new(),
            next: 1,
            bgm_level: 1.0,
            duck_bgm: false,
            ducked: false,
        })
    }
    /// 音声再生中にＢＧＭの音量を小さくする.
    pub fn set_bgm_ducking(&mut self, enabled: bool) {
        self.duck_bgm = enabled;
        self.update_ducking();
    }
    /// Follow voice start/finish; hosts call this once per frame.
    pub fn update_ducking(&mut self) {
        let voice = self
            .playing
            .values()
            .any(|p| p.bus == Bus::Voice && p.handle.state() != PlaybackState::Stopped);
        let ducked = self.duck_bgm && voice;
        if ducked != self.ducked {
            self.ducked = ducked;
            self.apply_bgm_level(Tween {
                duration: std::time::Duration::from_millis(DUCK_TWEEN_MS),
                ..Tween::default()
            });
        }
    }
    fn apply_bgm_level(&mut self, tween: Tween) {
        let scale = if self.ducked { DUCKED_BGM } else { 1.0 };
        self.tracks[0].set_volume(Volume::Amplitude(self.bgm_level * scale), tween);
    }
    /// Host levels use separate mixer tracks. Script gain envelopes and fades
    /// continue without replacing or restarting active sounds.
    pub fn set_output_levels(&mut self, levels: [u8; 4]) -> Result<()> {
        ensure!(levels.iter().all(|&v| v <= 100), "invalid audio level");
        self.manager.main_track().set_volume(
            Volume::Amplitude(f64::from(levels[0]) / 100.0),
            Tween::default(),
        );
        for (track, level) in self.tracks.iter_mut().zip(&levels[1..]).skip(1) {
            track.set_volume(
                Volume::Amplitude(f64::from(*level) / 100.0),
                Tween::default(),
            );
        }
        self.bgm_level = f64::from(levels[1]) / 100.0;
        self.apply_bgm_level(Tween::default());
        Ok(())
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
        self.play_with_spatial(source, bus, data, looped, envelope, StereoGains::UNITY)
    }
    pub fn play_with_spatial(
        &mut self,
        source: u64,
        bus: Bus,
        data: &SoundData,
        looped: bool,
        envelope: GainEnvelope,
        stereo: StereoGains,
    ) -> Result<u64> {
        self.play_config(
            source,
            bus,
            data,
            looped,
            PlaybackStart {
                position: 0,
                envelope,
                gain_scale: 1.0,
                stop_after_ms: None,
                stereo,
            },
        )
    }
    /// Resume at a PCM frame offset. Loop phase uses the decoded frame count,
    /// not rounded duration milliseconds; an expired one-shot is not replayed.
    pub fn resume_playback(
        &mut self,
        playback: &PlaybackResume,
        data: &SoundData,
        looped: bool,
    ) -> Result<Option<u64>> {
        playback.validate()?;
        let Some(position) = resume_frame(data, looped, playback.elapsed_ms)? else {
            return Ok(None);
        };
        // Preserve current audibility and the remaining stop deadline. Kira's
        // stop tween is decibel-based; restarting its remaining segment is a
        // presentation approximation, not an extension of the VM wait.
        let gain_scale = playback.fade_stop.map_or(1.0, |fade| {
            Volume::Decibels(
                Volume::MIN_DECIBELS * f64::from(fade.elapsed_ms) / f64::from(fade.duration_ms),
            )
            .as_amplitude()
        });
        self.play_config(
            playback
                .source
                .offset
                .context("resumed sound offset missing")?,
            playback.bus,
            data,
            looped,
            PlaybackStart {
                position,
                envelope: playback.envelope,
                gain_scale,
                stop_after_ms: playback.fade_stop.map(|f| f.remaining_ms),
                stereo: playback.stereo,
            },
        )
        .map(Some)
    }
    fn play_config(
        &mut self,
        source: u64,
        bus: Bus,
        data: &SoundData,
        looped: bool,
        start: PlaybackStart,
    ) -> Result<u64> {
        validate_data(data)?;
        start.stereo.validate()?;
        let ticket = self.next;
        let next = self.next.checked_add(1).context("sound ticket overflow")?;
        self.playing
            .retain(|_, v| v.handle.state() != PlaybackState::Stopped);
        if bus != Bus::Effect {
            self.stop_bus(bus);
        }
        let mut frames = Vec::with_capacity(data.pcm.len() / data.channels as usize);
        for samples in data.pcm.chunks(data.channels as usize) {
            let left = samples[0] as f32 / 32768.0;
            let right = samples.get(1).map_or(left, |v| *v as f32 / 32768.0);
            frames.push(Frame::new(
                left * start.stereo.left,
                right * start.stereo.right,
            ));
        }
        let settings = if looped {
            StaticSoundSettings::default().loop_region(..)
        } else {
            StaticSoundSettings::default()
        };
        let envelope = start.envelope;
        let settings = settings
            .output_destination(
                &self.tracks[match bus {
                    Bus::Bgm => 0,
                    Bus::Voice => 1,
                    Bus::Effect => 2,
                }],
            )
            .start_position(PlaybackPosition::Samples(start.position))
            .volume(Volume::Amplitude(
                envelope.start_amplitude() * start.gain_scale,
            ));
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
                Volume::Amplitude(envelope.end_amplitude() * start.gain_scale),
                Tween {
                    duration: std::time::Duration::from_millis(u64::from(envelope.duration_ms())),
                    easing: kira::tween::Easing::Linear,
                    ..Tween::default()
                },
            );
        }
        if let Some(duration) = start.stop_after_ms {
            handle.stop(Tween {
                duration: std::time::Duration::from_millis(u64::from(duration)),
                easing: kira::tween::Easing::Linear,
                ..Tween::default()
            });
        }
        self.next = next;
        self.playing.insert(
            ticket,
            Playing {
                source,
                bus,
                handle,
                gain_scale: start.gain_scale,
            },
        );
        self.update_ducking();
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
    pub fn fade_stop_source(&mut self, source: u64, duration_ms: u16) {
        for p in self.playing.values_mut().filter(|p| p.source == source) {
            p.handle.stop(Tween {
                duration: std::time::Duration::from_millis(u64::from(duration_ms)),
                easing: kira::tween::Easing::Linear,
                ..Tween::default()
            });
        }
    }
    pub fn set_source_gain(&mut self, source: u64, envelope: GainEnvelope) {
        let amplitude = if envelope.duration_ms() == 0 {
            envelope.start_amplitude()
        } else {
            envelope.end_amplitude()
        };
        for playing in self.playing.values_mut().filter(|p| p.source == source) {
            // Playback-first approximation: ramp from the current device gain
            // instead of resetting to the packet's initial amplitude.
            playing.handle.set_volume(
                Volume::Amplitude(amplitude * playing.gain_scale),
                Tween {
                    duration: std::time::Duration::from_millis(u64::from(envelope.duration_ms())),
                    easing: kira::tween::Easing::Linear,
                    ..Tween::default()
                },
            );
        }
    }
}
fn validate_data(data: &SoundData) -> Result<()> {
    ensure!(
        data.sample_rate > 0
            && [1, 2].contains(&data.channels)
            && data.pcm.len().is_multiple_of(usize::from(data.channels)),
        "unsupported output sample rate/channel layout"
    );
    Ok(())
}
fn resume_frame(data: &SoundData, looped: bool, elapsed_ms: u64) -> Result<Option<usize>> {
    validate_data(data)?;
    let frames = data.pcm.len() / usize::from(data.channels);
    ensure!(
        frames > 0 || !looped,
        "cannot resume an empty looping sound"
    );
    let position = u128::from(elapsed_ms) * u128::from(data.sample_rate) / 1000;
    Ok(if looped {
        Some((position % frames as u128) as usize)
    } else if position >= frames as u128 {
        None
    } else {
        Some(position as usize)
    })
}
/// AGES-ACS page (Vm60 CSoundManEx nAcsOn/nAcsMode/bAcsAtVoice/bAcsAtEffect).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AcsSettings {
    /// 0 無効, 1 2ch ヘッドフォン, 2 5.1ch スピーカー.
    pub output: u8,
    /// Dialog order of 詳細設定: 0 2D, 1 2D (回折・骨導音), 2 擬似バイノーラル, 3 3D.
    pub model: u8,
    pub voice: bool,
    pub effect: bool,
}
impl Default for AcsSettings {
    /// Retail defaults: nAcsOn 1, nAcsMode 2, applied to voice and effects.
    fn default() -> Self {
        Self {
            output: 1,
            model: 2,
            voice: true,
            effect: true,
        }
    }
}
impl AcsSettings {
    /// Portable stereo placement: no binaural filters or room model; models
    /// with diffraction or bone conduction let the far ear hear the source.
    pub fn place(self, bus: Bus, stereo: StereoGains) -> StereoGains {
        let applies = match bus {
            Bus::Voice => self.voice,
            Bus::Effect => self.effect,
            Bus::Bgm => false,
        };
        if self.output == 0 || !applies {
            return StereoGains::UNITY;
        }
        let crossfeed = match self.model {
            0 => 0.0,
            1 => 0.2,
            3 => 0.25,
            _ => 0.3,
        };
        StereoGains {
            left: stereo.left + (stereo.right - stereo.left).max(0.0) * crossfeed,
            right: stereo.right + (stereo.left - stereo.right).max(0.0) * crossfeed,
        }
    }
}

pub struct DeviceAudio {
    mixer: Mixer<DefaultBackend>,
    resources: AudioResources,
    archive: Arc<RioArchive>,
    acs: AcsSettings,
}
impl DeviceAudio {
    pub fn open(archive: Arc<RioArchive>) -> Result<Self> {
        let manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())
            .map_err(|e| anyhow::anyhow!("open audio device: {e}"))?;
        let resources = AudioResources::new(Arc::clone(&archive))?;
        Ok(Self {
            mixer: Mixer::new(manager)?,
            resources,
            archive,
            acs: AcsSettings::default(),
        })
    }
    pub fn set_acs(&mut self, acs: AcsSettings) {
        self.acs = acs;
    }
    pub fn set_bgm_ducking(&mut self, enabled: bool) {
        self.mixer.set_bgm_ducking(enabled);
    }
    /// Per-frame housekeeping such as BGM ducking.
    pub fn update(&mut self) {
        self.mixer.update_ducking();
    }
    pub fn request(&mut self, request: SoundRequest) -> Result<Option<u64>> {
        if request.null_source_is_noop() {
            return Ok(None);
        }
        let envelope = if [2, 4].contains(&request.kind) {
            Some(request.gain_envelope()?)
        } else {
            None
        };
        let Some(ref source) = request.source else {
            match request.global_stop_bus()? {
                Some(bus) => self.mixer.stop_bus(bus),
                None => self.mixer.stop_all(),
            };
            return Ok(None);
        };
        let offset = source.offset.context("sound source offset missing")?;
        if request.kind == 3 {
            self.mixer.stop_source(offset);
            return Ok(None);
        }
        if request.kind == 5 {
            self.mixer.fade_stop_source(offset, request.parameters[2]);
            return Ok(None);
        }
        if request.kind == 6 {
            self.mixer.set_source_gain(offset, request.gain_envelope()?);
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
        let bus = match class.name.as_str() {
            "CPmBgm" => Bus::Bgm,
            "COptimizedObs" => Bus::Voice,
            _ => Bus::Effect,
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
        let data = self.resources.load(source)?;
        let ticket = self.mixer.play_with_spatial(
            offset,
            bus,
            &data,
            looped,
            envelope.context("playback envelope missing")?,
            self.acs.place(bus, request.stereo),
        )?;
        // Every playback needs completion reporting, including asynchronous
        // sounds that a later CVmSync may query. VM waiting is runtime-owned.
        Ok(Some(ticket))
    }
    pub fn finished(&self, ticket: u64) -> bool {
        self.mixer.finished(ticket)
    }
    pub fn set_output_levels(&mut self, levels: [u8; 4]) -> Result<()> {
        self.mixer.set_output_levels(levels)
    }
    /// A coordinated game restore must stop the old output before rebuilding
    /// its runtime/device ticket mapping. Decoded resource caches may survive.
    pub fn reset(&mut self) {
        self.mixer.stop_all();
    }
    pub fn resume(&mut self, playback: &PlaybackResume) -> Result<Option<u64>> {
        playback.validate()?;
        let looped = self.resources.looped(&playback.source)?;
        let data = self.resources.load(&playback.source)?;
        let mut playback = playback.clone();
        playback.stereo = self.acs.place(playback.bus, playback.stereo);
        self.mixer.resume_playback(&playback, &data, looped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kira::manager::backend::mock::{MockBackend, MockBackendSettings};
    fn continuation(bus: Bus, elapsed_ms: u64, envelope: GainEnvelope) -> PlaybackResume {
        use rugp_assets::object::{ClassId, ObjectRef};
        let name = match bus {
            Bus::Bgm => "CPmBgm",
            Bus::Voice => "COptimizedObs",
            Bus::Effect => "CPmEffect",
        };
        PlaybackResume {
            ticket: 19,
            source: ObjectRef {
                class: Some(ClassId {
                    name: name.into(),
                    schema: 1,
                }),
                cache_index: None,
                flags: 0xc108,
                offset: Some(7),
                size: Some(7),
                owner: None,
                selector: 0,
                inline: None,
            },
            bus,
            elapsed_ms,
            envelope,
            fade_stop: None,
            stereo: Default::default(),
        }
    }
    fn mock_mixer(rate: u32) -> Result<Mixer<MockBackend>> {
        let manager = AudioManager::<MockBackend>::new(AudioManagerSettings {
            backend_settings: MockBackendSettings { sample_rate: rate },
            ..Default::default()
        })
        .map_err(|_| anyhow::anyhow!("mock init"))?;
        Mixer::new(manager)
    }
    #[test]
    fn settings_mix_buses_without_restarting_or_replacing_script_gain() -> Result<()> {
        let data = SoundData {
            sample_rate: 1000,
            channels: 1,
            pcm: vec![8192; 32],
        };
        for (bus, channel) in [(Bus::Bgm, 1), (Bus::Voice, 2), (Bus::Effect, 3)] {
            let mut mixer = mock_mixer(1000)?;
            let ticket = mixer.play_with_envelope(
                7,
                bus,
                &data,
                true,
                GainEnvelope::from_native([2048, 2048, 0])?,
            )?;
            mixer.manager.backend_mut().on_start_processing();
            let mut baseline = 0.0f32;
            for _ in 0..1000 {
                baseline = mixer.manager.backend_mut().process().left;
            }
            let next = mixer.next;
            let mut levels = [100; 4];
            levels[0] = 50;
            levels[channel] = 40;
            mixer.set_output_levels(levels)?;
            mixer.manager.backend_mut().on_start_processing();
            let mut reduced = 0.0;
            for _ in 0..1000 {
                reduced = mixer.manager.backend_mut().process().left;
            }
            assert!((reduced / baseline - 0.2).abs() < 0.01, "bus {bus:?}");
            assert_eq!(mixer.next, next);
            assert!(!mixer.finished(ticket));
            mixer.set_output_levels([100; 4])?;
            mixer.manager.backend_mut().on_start_processing();
            for _ in 0..1000 {
                reduced = mixer.manager.backend_mut().process().left;
            }
            assert!((reduced / baseline - 1.0).abs() < 0.01);
            assert!(mixer.set_output_levels([101; 4]).is_err());
        }
        Ok(())
    }
    #[test]
    fn spatial_channels_survive_gain_motion_and_resumed_pcm_without_touching_the_source()
    -> Result<()> {
        let data = SoundData {
            sample_rate: 1000,
            channels: 1,
            pcm: vec![16384; 1000],
        };
        let stereo = StereoGains {
            left: 0.75,
            right: 0.25,
        };
        for resumed in [false, true] {
            let mut mixer = mock_mixer(1000)?;
            if resumed {
                let mut playback = continuation(Bus::Effect, 200, GainEnvelope::UNITY);
                playback.stereo = stereo;
                mixer.resume_playback(&playback, &data, false)?.unwrap();
            } else {
                mixer.play_with_spatial(
                    7,
                    Bus::Effect,
                    &data,
                    false,
                    GainEnvelope::UNITY,
                    stereo,
                )?;
            }
            mixer.manager.backend_mut().on_start_processing();
            for _ in 0..100 {
                let _ = mixer.manager.backend_mut().process();
            }
            let frame = mixer.manager.backend_mut().process();
            assert!(frame.left > 0.05 && frame.right > 0.01);
            assert!((frame.left / frame.right - 3.0).abs() < 0.001);
            mixer.set_source_gain(7, GainEnvelope::from_native([4096, 2048, 100])?);
            mixer.manager.backend_mut().on_start_processing();
            for _ in 0..150 {
                let _ = mixer.manager.backend_mut().process();
            }
            let faded = mixer.manager.backend_mut().process();
            assert!((faded.left / faded.right - 3.0).abs() < 0.001);
            assert!((faded.left / frame.left - 0.5).abs() < 0.02);
        }
        assert!(data.pcm.iter().all(|s| *s == 16384));
        Ok(())
    }
    #[test]
    fn resumed_pcm_plays_the_tail_not_the_beginning_and_finishes_at_its_tail() -> Result<()> {
        for looped in [false, true] {
            let mut mixer = mock_mixer(1000)?;
            let data = SoundData {
                sample_rate: 1000,
                channels: 1,
                pcm: (0..1000)
                    .map(|i| if i < 500 { 8192 } else { -8192 })
                    .collect(),
            };
            let playback = continuation(
                Bus::Effect,
                if looped { 1500 } else { 500 },
                GainEnvelope::UNITY,
            );
            let ticket = mixer.resume_playback(&playback, &data, looped)?.unwrap();
            mixer.manager.backend_mut().on_start_processing();
            let samples = (0..400)
                .map(|_| mixer.manager.backend_mut().process().left)
                .collect::<Vec<_>>();
            assert!(samples[10..400].iter().all(|s| *s < -0.1));
            assert!(!mixer.finished(ticket));
            for _ in 0..200 {
                let _ = mixer.manager.backend_mut().process();
            }
            assert_eq!(mixer.finished(ticket), !looped);
        }
        Ok(())
    }
    #[test]
    fn resumed_gain_continues_from_current_amplitude_and_only_runs_remaining_motion() -> Result<()>
    {
        fn render(envelope: GainEnvelope) -> Result<Vec<f32>> {
            let mut mixer = mock_mixer(1000)?;
            let data = SoundData {
                sample_rate: 1000,
                channels: 1,
                pcm: vec![8192; 32],
            };
            mixer
                .resume_playback(&continuation(Bus::Bgm, 512, envelope), &data, true)?
                .unwrap();
            mixer.manager.backend_mut().on_start_processing();
            Ok((0..700)
                .map(|_| mixer.manager.backend_mut().process().left)
                .collect())
        }
        let unity = render(GainEnvelope::UNITY)?;
        let resumed = render(GainEnvelope::from_native([2048, 4096, 500])?)?;
        for at in [10, 100, 250, 450] {
            let expected = 0.5 + at as f32 / 1000.0;
            assert!((resumed[at] / unity[at] - expected).abs() < 0.03);
        }
        assert!((resumed[600] / unity[600] - 1.0).abs() < 0.001);
        Ok(())
    }
    #[test]
    fn resumed_fade_stop_keeps_attenuation_and_finishes_on_remaining_deadline() -> Result<()> {
        use crate::audio_state::snapshot::FadeResume;
        let data = SoundData {
            sample_rate: 1000,
            channels: 1,
            pcm: vec![16384; 32],
        };
        let mut mixer = mock_mixer(1000)?;
        let mut playback = continuation(Bus::Bgm, 1200, GainEnvelope::UNITY);
        playback.fade_stop = Some(FadeResume {
            elapsed_ms: 200,
            duration_ms: 1000,
            remaining_ms: 800,
        });
        let ticket = mixer.resume_playback(&playback, &data, true)?.unwrap();
        mixer.manager.backend_mut().on_start_processing();
        let samples = (0..500)
            .map(|_| mixer.manager.backend_mut().process().left)
            .collect::<Vec<_>>();
        assert!(samples[10] > 0.01 && samples[10] < 0.15);
        assert!(samples[10] > samples[300]);
        assert!(!mixer.finished(ticket));
        for _ in 0..400 {
            let _ = mixer.manager.backend_mut().process();
        }
        assert!(mixer.finished(ticket));
        Ok(())
    }
    #[test]
    fn pcm_resume_position_uses_exact_frames_for_stereo_fractional_loops_and_large_epochs()
    -> Result<()> {
        let data = SoundData {
            sample_rate: 3000,
            channels: 2,
            pcm: vec![0; 2002],
        }; // 1001 frames, NOT 334ms loops
        assert_eq!(resume_frame(&data, true, 334)?, Some(1));
        assert_eq!(resume_frame(&data, false, 333)?, Some(999));
        assert_eq!(resume_frame(&data, false, 334)?, None);
        assert_eq!(
            resume_frame(&data, true, u64::MAX)?,
            Some(((u128::from(u64::MAX) * 3) % 1001) as usize)
        );
        let data = SoundData {
            sample_rate: 44100,
            channels: 1,
            pcm: vec![0; 44100],
        };
        assert_eq!(resume_frame(&data, false, 1)?, Some(44));
        assert_eq!(resume_frame(&data, false, 999)?, Some(44055));
        assert_eq!(resume_frame(&data, false, 1000)?, None);
        assert_eq!(resume_frame(&data, true, 1001)?, Some(44));
        Ok(())
    }
    #[test]
    fn expired_or_invalid_resumes_never_allocate_a_device_ticket_or_replace_a_bus() -> Result<()> {
        use crate::audio_state::snapshot::FadeResume;
        let mut mixer = mock_mixer(1000)?;
        let data = SoundData {
            sample_rate: 1000,
            channels: 1,
            pcm: vec![8192; 100],
        };
        let original = mixer.play(7, Bus::Voice, &data, true)?;
        let next = mixer.next;
        let mut playback = continuation(Bus::Voice, 100, GainEnvelope::UNITY);
        assert!(mixer.resume_playback(&playback, &data, false)?.is_none());
        assert_eq!(mixer.next, next);
        assert!(!mixer.finished(original));
        playback.elapsed_ms = 0;
        playback.fade_stop = Some(FadeResume {
            elapsed_ms: 100,
            duration_ms: 100,
            remaining_ms: 1,
        });
        assert!(mixer.resume_playback(&playback, &data, false).is_err());
        assert_eq!(mixer.next, next);
        assert!(!mixer.finished(original));
        playback.fade_stop = None;
        let invalid = SoundData {
            sample_rate: 0,
            channels: 1,
            pcm: vec![0; 1],
        };
        assert!(mixer.resume_playback(&playback, &invalid, false).is_err());
        mixer.next = u64::MAX;
        assert!(mixer.resume_playback(&playback, &data, false).is_err());
        assert!(!mixer.finished(original));
        Ok(())
    }
    #[test]
    fn gain_motion_keeps_the_same_looping_instance_and_does_not_stop_it() -> Result<()> {
        let manager = AudioManager::<MockBackend>::new(AudioManagerSettings {
            backend_settings: MockBackendSettings { sample_rate: 1000 },
            ..Default::default()
        })
        .map_err(|_| anyhow::anyhow!("mock init"))?;
        let mut mixer = Mixer::new(manager)?;
        let data = SoundData {
            sample_rate: 1000,
            channels: 1,
            pcm: vec![8192; 32],
        };
        let ticket = mixer.play(7, Bus::Bgm, &data, true)?;
        mixer.manager.backend_mut().on_start_processing();
        let mut baseline = 0f32;
        for _ in 0..100 {
            baseline = baseline.max(mixer.manager.backend_mut().process().left.abs());
        }
        let next_ticket = mixer.next;
        mixer.set_source_gain(7, GainEnvelope::from_native([4096, 2048, 200])?);
        mixer.manager.backend_mut().on_start_processing();
        let mut samples = Vec::new();
        for _ in 0..500 {
            samples.push(mixer.manager.backend_mut().process().left);
        }
        assert!(samples[50] > samples[150]);
        assert!((samples[400] / baseline - 0.5).abs() < 0.03);
        assert_eq!(mixer.next, next_ticket);
        assert!(!mixer.finished(ticket));
        mixer.set_source_gain(7, GainEnvelope::from_native([0, 0, 0])?);
        mixer.manager.backend_mut().on_start_processing();
        for _ in 0..100 {
            let _ = mixer.manager.backend_mut().process();
        }
        assert!(!mixer.finished(ticket)); // silence is not completion
        Ok(())
    }
    #[test]
    fn fade_stop_does_not_restart_looping_audio_and_finishes_after_the_ramp() -> Result<()> {
        let manager = AudioManager::<MockBackend>::new(AudioManagerSettings {
            backend_settings: MockBackendSettings { sample_rate: 1000 },
            ..Default::default()
        })
        .map_err(|_| anyhow::anyhow!("mock init"))?;
        let mut mixer = Mixer::new(manager)?;
        let data = SoundData {
            sample_rate: 1000,
            channels: 1,
            pcm: vec![16384; 1000],
        };
        let ticket = mixer.play(7, Bus::Bgm, &data, true)?;
        mixer.manager.backend_mut().on_start_processing();
        for _ in 0..100 {
            let _ = mixer.manager.backend_mut().process();
        }
        mixer.fade_stop_source(7, 400);
        mixer.manager.backend_mut().on_start_processing();
        let mut samples = Vec::new();
        for _ in 0..800 {
            samples.push(mixer.manager.backend_mut().process().left);
        }
        assert!(samples[100].abs() > samples[300].abs());
        // Kira's stop tween interpolates decibels, not linear amplitude.
        assert!(samples[100].abs() > 0.01);
        assert!(samples[799].abs() < 0.001);
        assert!(mixer.finished(ticket));
        Ok(())
    }
    #[test]
    fn mock_output_and_stop_are_real_audio_commands() -> Result<()> {
        let settings = AudioManagerSettings::<MockBackend> {
            backend_settings: MockBackendSettings { sample_rate: 44100 },
            ..Default::default()
        };
        let manager =
            AudioManager::<MockBackend>::new(settings).map_err(|_| anyhow::anyhow!("mock init"))?;
        let mut mixer = Mixer::new(manager)?;
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
        let mut mixer = Mixer::new(manager)?;
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
            let mut mixer = Mixer::new(manager)?;
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
