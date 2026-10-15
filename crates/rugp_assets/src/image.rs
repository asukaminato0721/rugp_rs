//! rUGP bitmap decoding. All surfaces use straight-alpha RGBA8.
use crate::{binary::Bits, object::ObjectReader};
use anyhow::{Context, Result, ensure};

#[derive(Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub x: u16,
    pub y: u16,
    pub rgba: Vec<u8>,
}

#[derive(Debug)]
pub struct R6Header {
    pub version: u32,
    pub canvas_width: u16,
    pub canvas_height: u16,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub flags: u32,
    pub depth: u32,
    pub data_size: u32,
}

impl R6Header {
    pub fn read(r: &mut ObjectReader<'_>, shift: u32) -> Result<Self> {
        let version = r.input.u32()?;
        let canvas_width = r.input.u16()?;
        let canvas_height = r.input.u16()?;
        let x = r.input.u16()?;
        let y = r.input.u16()?;
        let width = r.input.u16()?;
        let height = r.input.u16()?;
        let flags = r.input.u32()?;
        let depth = r.input.u32()?;
        r.input.skip(3)?;
        r.reference(shift)?;
        let data_size = r.input.u32()?;
        r.input.skip(8)?;
        ensure!(
            width > 0 && height > 0 && width as usize * height as usize <= 16_777_216,
            "invalid bitmap dimensions {width}x{height}"
        );
        Ok(Self {
            version,
            canvas_width,
            canvas_height,
            x,
            y,
            width,
            height,
            flags,
            depth,
            data_size,
        })
    }
}

struct Packed<'a>(Bits<'a>);
impl Packed<'_> {
    fn bit(&mut self) -> Result<bool> {
        Ok(self.0.read(1).context("truncated bitmap bitstream")? != 0)
    }
    fn unsigned(&mut self) -> Result<u32> {
        if !self.bit()? {
            return Ok(0);
        }
        let mut n = 1u32;
        for _ in 0..30 {
            n = (n << 1) | self.bit()? as u32;
            if !self.bit()? {
                return Ok(n - 1);
            }
        }
        anyhow::bail!("bitmap run length exceeds limit")
    }
    fn signed(&mut self) -> Result<i32> {
        if !self.bit()? {
            return Ok(0);
        }
        let negative = self.bit()?;
        let mut n = 1i32;
        for _ in 0..6 {
            if !self.bit()? {
                break;
            }
            n = (n << 1) | self.bit()? as i32;
        }
        Ok(if negative { -n } else { n })
    }
}

/// Decodes a Cr6Ti extent, including its three-byte RIO object prefix.
pub fn decode_r6ti(data: &[u8], shift: u32) -> Result<Image> {
    let mut r = ObjectReader::new(data);
    r.input.skip(3)?;
    r.scrambled = true;
    r.pad(3)?;
    let header = R6Header::read(&mut r, shift)?;
    let compressed = r.input.bytes(header.data_size as usize)?;
    let mut bits = Packed(Bits::new(compressed));
    let width = header.width as usize;
    let height = header.height as usize;
    let mut pixels = vec![[0u8; 4]; width * height];
    let mut previous = vec![[0u8; 4]; width];
    match header.flags & 0xff {
        2 => {
            let predictive = header.depth & 4 != 0;
            let depths = [
                (header.depth >> 8) & 255,
                (header.depth >> 16) & 255,
                (header.depth >> 24) & 255,
            ];
            if !predictive {
                ensure!(
                    depths.iter().all(|d| (1..=8).contains(d)),
                    "invalid channel depths {depths:?}"
                );
            }
            for y in 0..height {
                let mut rgb = [0i32; 3];
                let mut delta = 0;
                let mut count = -1i64;
                let mut x = 0;
                while x < width {
                    if count < 0 {
                        count = bits.unsigned()? as i64;
                    }
                    if bits.bit()? {
                        rgb = [
                            previous[x][0] as i32,
                            previous[x][1] as i32,
                            previous[x][2] as i32,
                        ];
                        delta = 0;
                    } else if predictive {
                        let prediction = (rgb[1] + delta * 2).clamp(0, 255) - rgb[1];
                        delta = (prediction / 2 + bits.signed()?).clamp(-128, 127);
                        for channel in &mut rgb {
                            *channel = (*channel + delta * 2).clamp(0, 255);
                        }
                        rgb[0] = (rgb[0] + bits.signed()? * 2).clamp(0, 255);
                        rgb[2] = (rgb[2] + bits.signed()? * 2).clamp(0, 255);
                    } else {
                        let common = bits.signed()? << (8 - depths[1]);
                        for channel in &mut rgb {
                            *channel = (*channel + common).clamp(0, 255);
                        }
                        rgb[0] = (rgb[0] + (bits.signed()? << (8 - depths[0]))).clamp(0, 255);
                        rgb[2] = (rgb[2] + (bits.signed()? << (8 - depths[2]))).clamp(0, 255);
                        for (channel, depth) in rgb.iter_mut().zip(depths) {
                            *channel &= 0x100 - (1 << (8 - depth));
                        }
                    }
                    let pixel = [rgb[0] as u8, rgb[1] as u8, rgb[2] as u8, 255];
                    pixels[y * width + x] = pixel;
                    previous[x] = pixel;
                    x += 1;
                    count -= 1;
                    if count < 0 && x < width {
                        let length = (bits.unsigned()? as usize + 1).min(width - x);
                        pixels[y * width + x..y * width + x + length].fill(pixel);
                        previous[x..x + length].fill(pixel);
                        x += length;
                        delta = 0;
                    }
                }
            }
        }
        3 => {
            for y in 0..height {
                let mut rgb = [0i32; 3];
                let mut alpha = 0i32;
                let mut delta = 0i32;
                let mut alpha_run = 0i64;
                let mut color_run = 0i64;
                let mut repeat = true;
                let mut x = 0;
                while x < width {
                    alpha_run -= 1;
                    if alpha_run < 0 {
                        alpha += bits.signed()?;
                        ensure!((0..=32).contains(&alpha), "invalid alpha predictor {alpha}");
                        if alpha == 0 {
                            let length = (bits.unsigned()? as usize + 1).min(width - x);
                            previous[x..x + length].fill([0; 4]);
                            x += length;
                            continue;
                        }
                        if alpha == 32 {
                            alpha_run = bits.unsigned()? as i64;
                        }
                    }
                    color_run -= 1;
                    if color_run < 0 {
                        repeat = !repeat;
                        color_run = bits.unsigned()? as i64;
                        delta = 0;
                    }
                    let a = (alpha * 8).clamp(0, 255) as u8;
                    if repeat {
                        let length = if alpha_run < color_run {
                            1
                        } else {
                            if color_run > 0 {
                                alpha_run -= color_run;
                            }
                            let length = color_run as usize + 1;
                            color_run = 0;
                            length.min(width - x)
                        };
                        let pixel = [rgb[0] as u8, rgb[1] as u8, rgb[2] as u8, a];
                        pixels[y * width + x..y * width + x + length].fill(pixel);
                        previous[x..x + length].fill(pixel);
                        x += length;
                    } else {
                        if bits.bit()? {
                            rgb = [
                                previous[x][0] as i32,
                                previous[x][1] as i32,
                                previous[x][2] as i32,
                            ];
                            delta = 0;
                        } else {
                            let prediction =
                                ((rgb[1] + delta * 2).clamp(0, 255) - rgb[1]) as i8 as i32;
                            delta = (prediction / 2 + bits.signed()?).clamp(-128, 127);
                            rgb[0] = (rgb[0] + delta * 2 + bits.signed()? * 2).clamp(0, 255) & 0xfe;
                            rgb[1] = (rgb[1] + delta * 2).clamp(0, 255) & 0xfe;
                            rgb[2] = (rgb[2] + delta * 2 + bits.signed()? * 2).clamp(0, 255) & 0xfe;
                        }
                        let pixel = [rgb[0] as u8, rgb[1] as u8, rgb[2] as u8, a];
                        pixels[y * width + x] = pixel;
                        previous[x] = pixel;
                        x += 1;
                    }
                }
            }
        }
        kind => anyhow::bail!("unsupported Cr6Ti bitmap kind {kind}"),
    }
    Ok(Image {
        width: header.width as u32,
        height: header.height as u32,
        x: header.x,
        y: header.y,
        // Predictor variables follow the original engine's B,G,R order.
        rgba: pixels
            .into_iter()
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect(),
    })
}
