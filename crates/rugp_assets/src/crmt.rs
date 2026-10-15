//! CRmt texture containers and CRmti mipmaps.
use crate::{
    image::Image,
    object::{ClassId, ObjectReader, ObjectRef},
};
use anyhow::{Context, Result, ensure};

pub struct MipLevel {
    pub width: u16,
    pub height: u16,
    pub precision: u32,
    pub active_rows: u16,
    pub data: Vec<u8>,
}
pub struct Crmt {
    pub version: u16,
    pub width: u16,
    pub height: u16,
    pub scale: u32,
    pub base_offset: (i32, i32),
    pub image_properties: ObjectRef,
    pub levels: Vec<MipLevel>,
    pub trailing_data: Vec<u8>,
}

impl Crmt {
    pub fn parse(data: &[u8], shift: u32) -> Result<Self> {
        let mut r = ObjectReader::new(data);
        r.input.skip(3)?;
        r.scrambled = true;
        r.pad(3)?;
        // CRmt schema 4 pre-registers these native classes and serializes
        // their schema words before the parent object's own fields.
        for name in ["CRmti", "CRimp", "Cr6Ti"] {
            let schema = r.input.u16()?;
            r.cache_class(ClassId {
                name: name.to_owned(),
                schema,
            });
        }
        let version = r.input.u16()?;
        ensure!(version == 8, "unsupported CRmt parent version {version}");
        let width = r.input.u16()?;
        let height = r.input.u16()?;
        let scale = r.input.u32()?;
        let base_offset = (r.input.u32()? as i32, r.input.u32()? as i32);
        r.input.u8()?;
        r.input.u16()?;
        r.input.u32()?;
        let image_properties = r.reference(shift)?;
        let count = r.input.u8()? as usize;
        ensure!((1..=6).contains(&count), "invalid CRmt mip count {count}");
        let mut levels = Vec::with_capacity(count);
        for _ in 0..count {
            let class = r
                .class()?
                .context("CRmt mip reference has no inline class")?;
            ensure!(
                class.name == "CRmti" && class.schema == 1,
                "unsupported mip class {}#{}",
                class.name,
                class.schema
            );
            let flags = r.input.u16()?;
            ensure!(flags & 0x40 != 0, "CRmt mip is not stored inline");
            r.pad(1)?;
            let width = r.input.u16()?;
            let height = r.input.u16()?;
            let precision = r.input.u32()?;
            let length = r.input.u32()? as usize;
            r.input.u32()?;
            let active_rows = r.input.u16()?;
            ensure!(
                width > 0
                    && height > 0
                    && width as usize * height as usize <= 16_777_216
                    && active_rows <= height,
                "invalid mip dimensions"
            );
            let data = r.input.bytes(length)?.to_vec();
            levels.push(MipLevel {
                width,
                height,
                precision,
                active_rows,
                data,
            });
        }
        // This contains mask, motion and property data, needed by the runtime
        // in addition to the independently decodable texture mip levels.
        let trailing_data = r.input.bytes(r.input.remaining())?.to_vec();
        Ok(Self {
            version,
            width,
            height,
            scale,
            base_offset,
            image_properties,
            levels,
            trailing_data,
        })
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    position: usize,
    lsb: bool,
}
impl BitReader<'_> {
    fn bit(&mut self) -> Result<bool> {
        ensure!(
            self.position < self.data.len() * 8,
            "truncated CRmti bitstream"
        );
        let shift = if self.lsb {
            self.position % 8
        } else {
            7 - self.position % 8
        };
        let bit = self.data[self.position / 8] >> shift & 1;
        self.position += 1;
        Ok(bit != 0)
    }
    fn signed(&mut self, wide: bool) -> Result<i32> {
        if !self.bit()? {
            return Ok(0);
        }
        let negative = self.bit()?;
        let mut n = 1i32;
        let limit = if self.lsb && !wide { 6 } else { 30 };
        for _ in 0..limit {
            if !self.bit()? {
                return Ok(if negative { -n } else { n });
            }
            n = (n << 1) | self.bit()? as i32;
        }
        ensure!(self.lsb && !wide, "CRmti signed integer exceeds limit");
        Ok(if negative { -n } else { n })
    }
    fn unsigned(&mut self) -> Result<usize> {
        if !self.bit()? {
            return Ok(0);
        }
        let mut n = 1usize;
        for _ in 0..30 {
            n = (n << 1) | self.bit()? as usize;
            if !self.bit()? {
                return Ok(n - 1);
            }
        }
        anyhow::bail!("CRmti unsigned integer exceeds limit")
    }
}

impl MipLevel {
    pub fn decode(&self) -> Result<Image> {
        let flags = self.precision & 255;
        let depth = (self.precision >> 8) & 255;
        ensure!(
            [7, 15].contains(&flags)
                && [5, 6, 7].contains(&depth)
                && ((self.precision >> 16) & 255) == depth
                && (self.precision >> 24) == depth,
            "unsupported CRmti precision {:#x}",
            self.precision
        );
        let step = 1i32 << (8 - depth);
        let mask = 255 & !(step - 1);
        let mut bits = BitReader {
            data: &self.data,
            position: 0,
            lsb: flags == 15,
        };
        let width = self.width as usize;
        let height = self.height as usize;
        ensure!(
            width > 0
                && height > 0
                && width * height <= 16_777_216
                && self.active_rows <= self.height,
            "invalid mip dimensions"
        );
        let mut ranges = Vec::with_capacity(height);
        let mut start = 0i32;
        let mut span = 0i32;
        for _ in 0..self.active_rows {
            span = span
                .checked_add(bits.signed(true)?)
                .context("CRmti span overflow")?;
            if span == 0 {
                start = 0;
            } else {
                start = start
                    .checked_add(bits.signed(true)?)
                    .context("CRmti row start overflow")?;
            }
            ensure!(
                start >= 0 && span >= 0 && start as usize + span as usize <= width,
                "CRmti row outside image"
            );
            ranges.push((start as usize, (start + span) as usize));
        }
        ranges.resize(height, (0, 0));
        let mut pixels = vec![[0u8; 4]; width * height];
        let mut previous = vec![[0u8; 3]; width];
        for (y, (start, end)) in ranges.into_iter().enumerate() {
            let mut rgb = [0i32; 3];
            let mut a = 0i32;
            let mut delta = 0i32;
            let mut alpha_run = 0i64;
            let mut color_run = 0i64;
            let mut repeat = true;
            let mut x = start;
            while x < end {
                alpha_run -= 1;
                if alpha_run < 0 {
                    a = a
                        .checked_add(bits.signed(false)?)
                        .context("CRmti alpha overflow")?;
                    ensure!((0..=32).contains(&a), "invalid CRmti alpha {a}");
                    if a == 0 {
                        let count = bits.unsigned()? + 1;
                        ensure!(count <= end - x, "CRmti transparent run exceeds row");
                        x += count;
                        continue;
                    }
                    if a == 32 {
                        alpha_run = bits.unsigned()? as i64;
                    }
                }
                color_run -= 1;
                if color_run < 0 {
                    repeat = !repeat;
                    color_run = bits.unsigned()? as i64;
                    delta = 0;
                }
                let opacity = if a == 32 { 255 } else { a as u8 * 8 };
                let count = if repeat {
                    if alpha_run < color_run {
                        1
                    } else {
                        if color_run > 0 {
                            alpha_run -= color_run;
                        }
                        let count = color_run as usize + 1;
                        color_run = 0;
                        count
                    }
                } else {
                    if bits.bit()? {
                        rgb = previous[x].map(i32::from);
                        delta = 0;
                    } else {
                        let prediction =
                            ((rgb[1] + delta * step).clamp(0, 255) - rgb[1]) as i8 as i32;
                        delta = ((prediction >> (8 - depth)) as i64 + bits.signed(false)? as i64)
                            .clamp(-128, 127) as i32;
                        for channel in &mut rgb {
                            *channel = (*channel + delta * step).clamp(0, 255);
                        }
                        rgb[0] = (rgb[0] as i64 + bits.signed(false)? as i64 * step as i64)
                            .clamp(0, 255) as i32
                            & mask;
                        rgb[2] = (rgb[2] as i64 + bits.signed(false)? as i64 * step as i64)
                            .clamp(0, 255) as i32
                            & mask;
                        rgb[1] &= mask;
                    }
                    1
                };
                ensure!(count <= end - x, "CRmti color run exceeds row");
                let pixel = [rgb[2] as u8, rgb[1] as u8, rgb[0] as u8, opacity];
                pixels[y * width + x..y * width + x + count].fill(pixel);
                previous[x..x + count].fill([rgb[0] as u8, rgb[1] as u8, rgb[2] as u8]);
                x += count;
            }
        }
        // RIO aligns compressed streams to bytes/words. Any remaining bits
        // must be padding; a nonzero tail indicates a decoder/profile mismatch.
        for p in bits.position..self.data.len() * 8 {
            let shift = if bits.lsb { p % 8 } else { 7 - p % 8 };
            ensure!(
                self.data[p / 8] >> shift & 1 == 0,
                "nonzero CRmti bitstream tail at bit {p}"
            );
        }
        Ok(Image {
            canvas_width: width as u32,
            canvas_height: height as u32,
            width: width as u32,
            height: height as u32,
            x: 0,
            y: 0,
            display_scale: 1.0,
            rgba: pixels.into_iter().flatten().collect(),
        })
    }
}
