//! Resource resolution and PCM data; device playback belongs to a platform backend.
use anyhow::{Context, Result, ensure};
use rugp_assets::{
    RioArchive,
    object::ObjectRef,
    oor::{Oor, SetupHeaders, VorbisDecoder},
    sound::Sample,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};
pub struct SoundData {
    pub sample_rate: u32,
    pub channels: u8,
    pub pcm: Vec<i16>,
}
pub struct AudioResources {
    archive: Arc<RioArchive>,
    headers: SetupHeaders,
    cache: BTreeMap<u64, Arc<SoundData>>,
    ages: VecDeque<u64>,
    bytes: usize,
}
impl AudioResources {
    pub fn new(archive: Arc<RioArchive>) -> Result<Self> {
        let root = archive.volumes[0]
            .path
            .parent()
            .context("game root missing")?;
        let plugin = std::fs::read(root.join("Plugins/riooor.rpo"))?;
        Ok(Self {
            archive,
            headers: SetupHeaders::from_plugin(&plugin),
            cache: BTreeMap::new(),
            ages: VecDeque::new(),
            bytes: 0,
        })
    }
    pub fn load(&mut self, reference: &ObjectRef) -> Result<Arc<SoundData>> {
        self.load_at_depth(reference, 0)
    }
    pub fn looped(&self, reference: &ObjectRef) -> Result<bool> {
        if !reference
            .class
            .as_ref()
            .is_some_and(|c| ["CPmBgm", "CPmEffect"].contains(&c.name.as_str()))
        {
            return Ok(false);
        }
        let bytes = self.archive.read_at(
            reference.offset.context("sound offset missing")?,
            reference.size.context("sound size missing")? as usize,
        )?;
        Ok(Sample::parse(&bytes, self.archive.address_shift)?.mode & 1 != 0)
    }
    fn load_at_depth(&mut self, reference: &ObjectRef, depth: usize) -> Result<Arc<SoundData>> {
        ensure!(depth < 8, "cyclic or over-nested sound source");
        let offset = reference
            .offset
            .context("sound reference has no stored extent")?;
        if let Some(sound) = self.cache.get(&offset) {
            self.ages.retain(|v| *v != offset);
            self.ages.push_back(offset);
            return Ok(Arc::clone(sound));
        }
        let class = reference.class.as_ref().context("sound class missing")?;
        let bytes = self.archive.read_at(
            offset,
            reference.size.context("sound size missing")? as usize,
        )?;
        if class.name == "CPmBgm" || class.name == "CPmEffect" {
            let sample = Sample::parse(&bytes, self.archive.address_shift)?;
            return self.load_at_depth(&sample.source, depth + 1);
        }
        ensure!(
            class.name == "COptimizedObs",
            "unsupported sound source {}",
            class.name
        );
        let mut decoder = VorbisDecoder::new(Oor::parse(&bytes)?, &self.headers)?;
        let mut pcm = Vec::new();
        while let Some(samples) = decoder.next_samples()? {
            ensure!(
                pcm.len() + samples.len() <= 256 * 1024 * 1024,
                "decoded audio exceeds memory limit"
            );
            pcm.extend(samples);
        }
        let sound = Arc::new(SoundData {
            sample_rate: decoder.info.sample_rate,
            channels: decoder.info.channels,
            pcm,
        });
        while self.bytes + sound.pcm.len() * 2 > 64 * 1024 * 1024 {
            let Some(key) = self.ages.pop_front() else {
                break;
            };
            if let Some(old) = self.cache.remove(&key) {
                self.bytes -= old.pcm.len() * 2;
            }
        }
        self.bytes += sound.pcm.len() * 2;
        self.cache.insert(offset, Arc::clone(&sound));
        self.ages.push_back(offset);
        Ok(sound)
    }
}

/// Muting removes output, not playback state or timing. Even asynchronous
/// one-shots get a deadline so a later CVmSync query/wait observes their tail.
pub fn mute_requests(
    game: &mut crate::game::Game,
    resources: &mut Option<AudioResources>,
) -> Result<()> {
    for request in game.take_sounds() {
        if let Some(source) = &request.source
            && [2, 4].contains(&request.kind)
        {
            // Volume motion does not change PCM duration, even when it reaches
            // zero. Muting still validates the same playback packet.
            request.gain_envelope()?;
            if resources.is_none() {
                *resources = Some(AudioResources::new(Arc::clone(&game.archive))?);
            }
            let resources = resources.as_mut().unwrap();
            if resources.looped(source)? {
                ensure!(request.flags & 1 == 0, "cannot wait for a looping sound");
                continue;
            }
            let data = resources.load(source)?;
            let frames = data.pcm.len() as u64 / data.channels as u64;
            let ms = (frames * 1000).div_ceil(data.sample_rate as u64);
            game.time_audio_ticket(
                request
                    .ticket
                    .context("play request has no runtime ticket")?,
                ms.try_into()
                    .context("audio duration exceeds runtime clock")?,
            );
        }
    }
    Ok(())
}
