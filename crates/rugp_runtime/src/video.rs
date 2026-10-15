//! Archive-backed, clock-driven movie playback; no window or audio dependency.
use anyhow::{Context, Result, ensure};
use rugp_assets::{
    RioArchive,
    crsi::{Crsi, codec::decode_frame},
    image::Image,
    object::ObjectRef,
};
use std::sync::Arc;

pub struct VideoPlayer {
    archive: Arc<RioArchive>,
    stream: Crsi,
    offset: u64,
    rate: u32,
    elapsed: u64,
    current: Option<usize>,
    image: Option<Image>,
    finished: bool,
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
}
