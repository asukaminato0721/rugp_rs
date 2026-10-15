//! Scalar reconstruction of CRip008's paired-row YUV movie profile.
use super::FrameIndex;
use crate::{binary::Bits, image::Image};
use anyhow::{Context, Result, ensure};

struct Input<'a> {
    bits: Bits<'a>,
    position: usize,
    data: &'a [u8],
}
impl<'a> Input<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            bits: Bits::new(data),
            position: 0,
            data,
        }
    }
    fn read(&mut self, count: usize) -> Result<u32> {
        let value = self
            .bits
            .read(count)
            .context("truncated paired-row video stream")?;
        self.position += count;
        Ok(value)
    }
    fn bit(&mut self) -> Result<bool> {
        Ok(self.read(1)? != 0)
    }
    fn unsigned(&mut self) -> Result<usize> {
        if !self.bit()? {
            return Ok(0);
        }
        let mut value = 1usize;
        for _ in 0..24 {
            value = (value << 1) | self.read(1)? as usize;
            if !self.bit()? {
                return Ok(value - 1);
            }
        }
        anyhow::bail!("oversized video run")
    }
    fn signed(&mut self) -> Result<i32> {
        if !self.bit()? {
            return Ok(0);
        }
        let negative = self.bit()?;
        let mut value = 1;
        for _ in 0..6 {
            if !self.bit()? {
                return Ok(if negative { -value } else { value });
            }
            value = (value << 1) | self.read(1)? as i32;
        }
        Ok(if negative { -value } else { value })
    }
    fn luma(&mut self, difference: i32) -> Result<i32> {
        ensure!(
            (-256..=255).contains(&difference),
            "invalid video luma predictor"
        );
        let code = if self.bit()? {
            4
        } else if self.bit()? {
            if self.bit()? { 3 } else { 5 }
        } else if self.bit()? {
            if self.bit()? { 2 } else { 6 }
        } else if self.bit()? {
            if self.bit()? { 7 } else { 1 }
        } else if self.bit()? {
            0
        } else {
            // Five zero bits followed by an absolute signed luma offset.
            return Ok(self.read(7)? as i32 * 4 - 256);
        };
        let factor = f64::from(difference) * (40.0 / (f64::from(difference.abs()) + 25.0) + 0.64);
        Ok(
            ((f64::from(code - 4) * factor + f64::from(difference * 2)) * 0.25 + 0.5).floor()
                as i32,
        )
    }
    fn finish(&self) -> Result<()> {
        ensure!(
            self.data.len() * 8 - self.position <= 31,
            "excess paired-row video tail at bit {} / {}",
            self.position,
            self.data.len() * 8
        );
        for i in self.position..self.data.len() * 8 {
            ensure!(
                self.data[i / 8] >> (i % 8) & 1 == 0,
                "nonzero paired-row video padding"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Default)]
struct Yuv {
    y: i32,
    cb: i32,
    cr: i32,
}
impl Yuv {
    fn average(self, other: Self) -> Self {
        Self {
            y: (self.y + other.y) >> 1,
            cb: (self.cb + other.cb) >> 1,
            cr: (self.cr + other.cr) >> 1,
        }
    }
    fn rgba(self) -> [u8; 4] {
        let b = self.y + ((self.cb * 29032) >> 16);
        let g = self.y + ((self.cb * -5630) >> 16) + ((self.cr * -11702) >> 16);
        let r = self.y + ((self.cr * 22970) >> 16);
        [
            r.clamp(0, 255) as u8,
            g.clamp(0, 255) as u8,
            b.clamp(0, 255) as u8,
            255,
        ]
    }
}

pub fn decode_keyframe(width: u16, height: u16, frame: &FrameIndex, data: &[u8]) -> Result<Image> {
    ensure!(
        frame.is_keyframe(),
        "temporal delta frame requires a previous frame"
    );
    decode(width, height, frame, data, None)
}

/// Decodes a frame using only its payload and the preceding RGBA image. Spatial
/// YUV predictors reset for every frame; temporal frames carry RGB residuals.
pub fn decode_frame(
    width: u16,
    height: u16,
    frame: &FrameIndex,
    data: &[u8],
    previous: Option<&Image>,
) -> Result<Image> {
    let previous = if frame.is_keyframe() {
        None
    } else {
        let image = previous.context("temporal delta frame requires a previous frame")?;
        ensure!(
            image.width == u32::from(width)
                && image.height == u32::from(height)
                && image.rgba.len() == usize::from(width) * usize::from(height) * 4,
            "previous video frame dimensions mismatch"
        );
        Some(image.rgba.as_slice())
    };
    decode(width, height, frame, data, previous)
}

fn decode(
    width: u16,
    height: u16,
    frame: &FrameIndex,
    data: &[u8],
    previous: Option<&[u8]>,
) -> Result<Image> {
    ensure!(
        frame.kind <= 1,
        "unsupported CRsi frame kind {}",
        frame.kind
    );
    ensure!(
        frame.codec_id() == 1 && frame.codec & 0x200 == 0,
        "unsupported CRsi movie codec {:#x}",
        frame.codec
    );
    ensure!(
        width > 0
            && height > 0
            && width.is_multiple_of(2)
            && height.is_multiple_of(2)
            && u64::from(width) * u64::from(height) <= 16_777_216,
        "invalid paired-row video dimensions"
    );
    ensure!(
        data.len() == frame.size as usize,
        "video payload length mismatch"
    );
    let [b, g, r] = frame.channel_depths();
    ensure!(
        [b, g, r] == [6, 7, 6],
        "unsupported movie channel precision"
    );
    let threshold = i32::from(frame.auxiliary.context("missing video coding threshold")?) - 1;
    let threshold = if threshold < 0 { 32767 } else { threshold };
    let mut input = Input::new(data);
    let width = usize::from(width);
    let height = usize::from(height);
    let mut rgba = vec![0; width * height * 4];
    let temporal = previous.is_some();
    let mut above = vec![Yuv::default(); width / 2];
    for row in (0..height).step_by(2) {
        let mut left = Yuv {
            y: 128,
            cb: 0,
            cr: 0,
        };
        let mut top_left = Yuv::default();
        let mut col = 0usize;
        while col < width / 2 {
            let mut repeated_pixel = [0; 4];
            let count = input.unsigned()? + 1;
            ensure!(count <= width / 2 - col, "video literal run exceeds row");
            for _ in 0..count {
                let upper = above[col];
                let left_before = left;
                let copied_above = input.bit()?;
                let uniform = if copied_above {
                    left = upper;
                    input.bit()?
                } else {
                    left = left.average(upper);
                    let delta = input.signed()? * (1 << (7 - g));
                    if temporal {
                        // The delta profile quantizes its predictor before adding
                        // chroma, rather than saturating the two luma increments.
                        left.y = (left.y + delta * 2) & 0xfffe;
                        left.cb = (left.cb as i16 & !15) as i32;
                        left.cr = (left.cr as i16 & !15) as i32;
                    } else {
                        left.y = (left.y + delta).clamp(0, 255);
                        left.y = (left.y + delta).clamp(0, 255);
                    }
                    left.cr = (left.cr + input.signed()? * (1 << (10 - b))).clamp(-32768, 32767);
                    left.cb = (left.cb + input.signed()? * (1 << (10 - r))).clamp(-32768, 32767);
                    input.bit()?
                };
                let copy_pixel = temporal && uniform;
                let uniform = uniform || (temporal && input.bit()?);
                let (tl, tr, bl, br) = if uniform {
                    (left, left, left, left)
                } else {
                    let mut tr = left.average(upper);
                    if !copied_above {
                        tr.y = (upper.y + input.luma(left.y - upper.y)?).clamp(0, 255);
                    }
                    let mut bl = left.average(left_before);
                    bl.y = left_before.y + input.luma(left.y - left_before.y)?;
                    let mut tl = tr.average(top_left);
                    tl.y = top_left.y + input.luma(tr.y - top_left.y)?;
                    (tl, tr, bl, left)
                };
                let x = col * 2;
                repeated_pixel = tr.rgba();
                for (dx, dy, value) in [(0, 0, tl), (1, 0, tr), (0, 1, bl), (1, 1, br)] {
                    let at = ((row + dy) * width + x + dx) * 4;
                    let prior_at = if copy_pixel {
                        (row * width + x + 1) * 4
                    } else {
                        at
                    };
                    write_pixel(
                        &mut rgba[at..at + 4],
                        value.rgba(),
                        previous.map(|p| &p[prior_at..prior_at + 4]),
                    );
                }
                top_left = tr;
                above[col] = left;
                col += 1;
            }
            if col == width / 2 {
                break;
            }
            let run = input.unsigned()?;
            ensure!(
                run < width / 2 - col,
                "video repeated run {run} exceeds row {row}, block {col}, bit {}",
                input.position
            );
            if !temporal && run > 0 && run as i32 >= threshold {
                let code = input.read(6)? as usize;
                left.y = (left.y + [0, 1, 2, 4][code & 3]).clamp(0, 255);
                left.cb = (left.cb + [-12, -4, 0, 4][code >> 4]).clamp(-32768, 32767);
                left.cr = (left.cr + [-12, -4, 0, 4][code >> 2 & 3]).clamp(-32768, 32767);
                repeated_pixel = left.rgba();
            }
            for _ in 0..=run {
                for dy in 0..2 {
                    for dx in 0..2 {
                        let at = ((row + dy) * width + col * 2 + dx) * 4;
                        write_pixel(
                            &mut rgba[at..at + 4],
                            repeated_pixel,
                            previous.map(|p| &p[at..at + 4]),
                        );
                    }
                }
                above[col] = left;
                col += 1;
            }
            top_left = left;
        }
    }
    input.finish()?;
    Ok(Image {
        canvas_width: width as u32,
        canvas_height: height as u32,
        width: width as u32,
        height: height as u32,
        x: 0,
        y: 0,
        display_scale: 1.0,
        rgba,
    })
}

fn write_pixel(target: &mut [u8], residual: [u8; 4], previous: Option<&[u8]>) {
    if let Some(previous) = previous {
        for channel in 0..3 {
            target[channel] = (i32::from(previous[channel])
                + 2 * (i32::from(residual[channel]) - 128))
                .clamp(0, 255) as u8;
        }
        target[3] = 255;
    } else {
        target.copy_from_slice(&residual);
    }
}
