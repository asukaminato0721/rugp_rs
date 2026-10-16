//! Archive-backed, clock-driven movie playback; no window or audio dependency.
use anyhow::{Context, Result, ensure};
use rugp_assets::object::snapshot::ReferenceSnapshotWriter;
use rugp_assets::{
    RioArchive,
    crsi::{Crsi, codec::decode_frame},
    image::Image,
    object::ObjectRef,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub struct VideoPlayer {
    archive: Arc<RioArchive>,
    stream: Crsi,
    source: ObjectRef,
    offset: u64,
    rate: u32,
    elapsed: u64,
    current: Option<usize>,
    image: Option<Image>,
    finished: bool,
}
/// Archive-backed reconstruction, not serialized decoded frame buffers. The
/// enclosing game snapshot must also preserve its VM/window/wait association.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoSnapshot {
    version: u32,
    source: u32,
    rate: u32,
    elapsed: u64,
}
impl VideoSnapshot {
    pub fn restore(
        &self,
        archive: Arc<RioArchive>,
        references: &[ObjectRef],
    ) -> Result<VideoPlayer> {
        ensure!(self.version == 1, "unsupported movie snapshot version");
        ensure!(
            (1..=100_000).contains(&self.rate),
            "invalid movie snapshot frame rate"
        );
        let source = references
            .get(self.source as usize)
            .context("movie snapshot source root out of range")?;
        let mut player = VideoPlayer::open(archive, source, self.rate)?;
        player.elapsed = self.elapsed;
        player.finished = self.elapsed >= player.duration_ms();
        let target = clock_frame(self.elapsed, self.rate, player.stream.frames.len());
        if player.current != Some(target) {
            player.decode_to(target)?;
        }
        Ok(player)
    }
}
impl VideoPlayer {
    pub fn open(archive: Arc<RioArchive>, source: &ObjectRef, rate_override: u32) -> Result<Self> {
        ensure!(
            source
                .class
                .as_ref()
                .is_some_and(|c| c.name == "CRsi" && c.schema == 1),
            "unsupported movie source class"
        );
        let offset = source.offset.context("movie has no stored offset")?;
        let stream = Crsi::read(
            &archive,
            offset,
            source.size.context("movie extent missing")?,
        )?;
        ensure!(
            stream.flags == 0
                && stream.modes == [0, 0]
                && stream.unknown == 0
                && stream.parameter == 0,
            "unsupported CRsi stream parameters"
        );
        ensure!(
            stream.frames[0].is_keyframe(),
            "movie does not start with a keyframe"
        );
        let rate = if rate_override == 0 {
            u32::from(stream.rate)
        } else {
            rate_override
        };
        ensure!((1..=100_000).contains(&rate), "invalid movie frame rate");
        let mut player = Self {
            archive,
            stream,
            source: source.clone(),
            offset,
            rate,
            elapsed: 0,
            current: None,
            image: None,
            finished: false,
        };
        player.decode_to(0)?;
        Ok(player)
    }
    pub fn size(&self) -> (u32, u32) {
        (u32::from(self.stream.width), u32::from(self.stream.height))
    }
    pub fn snapshot(&self, references: &mut ReferenceSnapshotWriter) -> Result<VideoSnapshot> {
        Ok(VideoSnapshot {
            version: 1,
            source: references.insert(&self.source)?,
            rate: self.rate,
            elapsed: self.elapsed,
        })
    }
    pub fn frame_index(&self) -> usize {
        self.current.unwrap_or(0)
    }
    pub fn image(&self) -> &Image {
        self.image
            .as_ref()
            .expect("movie initialized with first frame")
    }
    pub fn finished(&self) -> bool {
        self.finished
    }
    pub fn duration_ms(&self) -> u64 {
        (self.stream.frames.len() as u64 * 100_000).div_ceil(u64::from(self.rate))
    }
    /// Complete playback at the final frame. Snapshot restoration uses the
    /// same elapsed time and reconstructs the same delta-frame dependencies.
    pub fn finish(&mut self) -> Result<()> {
        if !self.finished {
            self.decode_to(self.stream.frames.len() - 1)?;
            self.elapsed = self.duration_ms();
            self.finished = true;
        }
        Ok(())
    }
    /// Keeps the final frame visible after completion. Catch-up starts at a
    /// preceding keyframe, but never skips a delta's dependencies.
    pub fn advance(&mut self, dt_ms: u32) -> Result<bool> {
        if self.finished {
            return Ok(false);
        }
        self.elapsed = self.elapsed.saturating_add(u64::from(dt_ms));
        self.finished = self.elapsed >= self.duration_ms();
        let target = clock_frame(self.elapsed, self.rate, self.stream.frames.len());
        if self.current == Some(target) {
            return Ok(false);
        }
        self.decode_to(target)?;
        Ok(true)
    }
    fn decode_to(&mut self, target: usize) -> Result<()> {
        let key = (0..=target)
            .rev()
            .find(|&i| self.stream.frames[i].is_keyframe())
            .context("missing movie keyframe")?;
        let start = self
            .current
            .filter(|&i| i >= key && i < target)
            .map_or(key, |i| i + 1);
        for i in start..=target {
            let frame = &self.stream.frames[i];
            let data = self.stream.frame_data(&self.archive, self.offset, i)?;
            self.image = Some(
                decode_frame(
                    self.stream.width,
                    self.stream.height,
                    frame,
                    &data,
                    self.image.as_ref(),
                )
                .with_context(|| format!("movie {:#x}, frame {i}", self.offset))?,
            );
            self.current = Some(i);
        }
        Ok(())
    }
}
fn clock_frame(elapsed: u64, rate: u32, frames: usize) -> usize {
    (elapsed.saturating_mul(u64::from(rate)) / 100_000).min(frames.saturating_sub(1) as u64)
        as usize
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_frame_clock_has_no_per_frame_rounding_drift() {
        for (ms, index) in [
            (0, 0),
            (33, 0),
            (34, 1),
            (100, 3),
            (1000, 30),
            (60_000, 1800),
            (117_033, 3510),
            (u64::MAX, 3510),
        ] {
            assert_eq!(clock_frame(ms, 3000, 3511), index);
        }
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_movie_snapshot_reconstructs_delta_frames_fractional_boundaries_and_exact_finish()
    -> Result<()> {
        use rugp_assets::object::{ClassId, snapshot::ReferenceSnapshot};
        let root = std::env::var_os("RUGP_TEST_GAME").context("game root")?;
        let archive = Arc::new(RioArchive::open(root)?);
        let source = ObjectRef {
            class: Some(ClassId {
                name: "CRsi".into(),
                schema: 1,
            }),
            cache_index: None,
            flags: 0xc108,
            offset: Some(0xd0ba5f),
            size: Some(234130539),
            owner: None,
            selector: 0,
            inline: None,
        };
        let mut player = VideoPlayer::open(Arc::clone(&archive), &source, 0)?;
        assert_eq!(player.duration_ms(), 117034);
        for target in [0, 33, 34, 67, 12034, 117033, 117034] {
            player.advance((target - player.elapsed) as u32)?;
            let mut writer = ReferenceSnapshotWriter::default();
            let snapshot = player.snapshot(&mut writer)?;
            let wire = serde_json::to_vec(&(snapshot, writer.finish()?))?;
            let (snapshot, references): (VideoSnapshot, ReferenceSnapshot) =
                serde_json::from_slice(&wire)?;
            let roots = references.restore()?;
            let mut restored = snapshot.restore(Arc::clone(&archive), &roots)?;
            assert_eq!(restored.elapsed, player.elapsed);
            assert_eq!(restored.rate, player.rate);
            assert_eq!(restored.frame_index(), player.frame_index());
            assert_eq!(restored.finished(), player.finished());
            assert_eq!(restored.image().rgba, player.image().rgba);
            assert_eq!(restored.advance(1)?, player.advance(1)?);
            assert_eq!(restored.frame_index(), player.frame_index());
            assert_eq!(restored.finished(), player.finished());
            assert_eq!(restored.image().rgba, player.image().rgba);
            let invalid = VideoSnapshot {
                version: 9,
                ..snapshot
            };
            assert!(invalid.restore(Arc::clone(&archive), &roots).is_err());
        }
        assert!(player.finished());
        let final_frame = player.image().rgba.clone();
        assert!(!player.advance(u32::MAX)?);
        assert_eq!(player.image().rgba, final_frame);
        let mut writer = ReferenceSnapshotWriter::default();
        let mut invalid = player.snapshot(&mut writer)?;
        let roots = writer.finish()?.restore()?;
        invalid.rate = 0;
        assert!(invalid.restore(Arc::clone(&archive), &roots).is_err());
        invalid.rate = 3000;
        invalid.source = 999;
        assert!(invalid.restore(archive, &roots).is_err());
        Ok(())
    }
}
