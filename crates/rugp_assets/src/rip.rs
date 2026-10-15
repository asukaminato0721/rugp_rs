//! CRip007 bitmap decoding; see GARbro attribution in THIRD_PARTY.md.
use crate::{image::Image, object::ObjectReader, rip_quant::QUANT};
use anyhow::{Context, Result, ensure};

struct Msb<'a> {
    data: &'a [u8],
    position: usize,
}
impl Msb<'_> {
    fn bit(&mut self) -> Result<bool> {
        ensure!(
            self.position < self.data.len() * 8,
            "truncated RIP bitmap bitstream"
        );
        let value = self.data[self.position / 8] >> (7 - self.position % 8) & 1;
        self.position += 1;
        Ok(value != 0)
    }
    fn unsigned(&mut self) -> Result<usize> {
        let mut n = 1usize;
        for _ in 0..24 {
            if !self.bit()? {
                return Ok(n);
            }
            n = (n << 1) | self.bit()? as usize;
        }
        anyhow::bail!("RIP bitmap integer exceeds limit")
    }
    fn signed(&mut self) -> Result<i32> {
        let negative = self.bit()?;
        let n = self.unsigned()? as i32;
        Ok(if negative { -n } else { n })
    }
    fn chroma(&mut self, q: usize) -> Result<i32> {
        if !self.bit()? {
            return Ok(0);
        }
        let negative = self.bit()?;
        let index = self.unsigned()?;
        let value = *QUANT
            .get(q)
            .and_then(|row| row.get(index))
            .context("invalid RIP quantization index")?;
        Ok(if negative { -value } else { value })
    }
    fn optional_signed(&mut self) -> Result<i32> {
        if self.bit()? { self.signed() } else { Ok(0) }
    }
}

pub fn decode_rip007(data: &[u8], schema: u16, shift: u32) -> Result<Image> {
    decode_rip(data, schema, shift, false)
}

pub fn decode_rip008(data: &[u8], schema: u16, shift: u32) -> Result<Image> {
    decode_rip(data, schema, shift, true)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RipHeader {
    pub width: u16,
    pub height: u16,
    pub x: u16,
    pub y: u16,
    pub crop_width: u16,
    pub crop_height: u16,
    pub flags: u32,
    pub info: [u8; 7],
    pub data_size: u32,
    pub modern: bool,
    pub image_properties: Option<Box<crate::object::ObjectRef>>,
}
impl RipHeader {
    pub fn read(r: &mut ObjectReader<'_>, schema: u16, shift: u32, modern: bool) -> Result<Self> {
        r.input.u32()?;
        let width = r.input.u16()?;
        let height = r.input.u16()?;
        let x = r.input.u16()?;
        let y = r.input.u16()?;
        let crop_width = r.input.u16()?;
        let crop_height = r.input.u16()?;
        let flags = r.input.u32()?;
        ensure!(
            [2, 3].contains(&(flags & 0xff)),
            "unsupported RIP bitmap kind {}",
            flags & 0xff
        );
        let info = if modern {
            let flags = r.input.u8()?;
            let depths = r.input.bytes(3)?;
            [0, flags, 0, flags, depths[0], depths[1], depths[2]]
        } else {
            r.input.bytes(7)?.try_into()?
        };
        let image_properties = if schema >= 2 {
            Some(Box::new(r.reference(shift)?))
        } else {
            None
        };
        let data_size = r.input.u32()?;
        r.input.u32()?;
        Ok(Self {
            width,
            height,
            x,
            y,
            crop_width,
            crop_height,
            flags,
            info,
            data_size,
            modern,
            image_properties,
        })
    }
}
fn decode_rip(data: &[u8], schema: u16, shift: u32, modern: bool) -> Result<Image> {
    let mut r = ObjectReader::new(data);
    r.input.skip(3)?;
    r.scrambled = true;
    r.pad(3)?;
    let header = RipHeader::read(&mut r, schema, shift, modern)?;
    let size = header.data_size as usize;
    let data = r.input.bytes(size)?;
    decode_rip_pixels(&header, data)
}
pub fn decode_rip_pixels(header: &RipHeader, data: &[u8]) -> Result<Image> {
    ensure!(
        data.len() == header.data_size as usize,
        "RIP compressed length mismatch"
    );
    let width = header.width as usize;
    let height = header.height as usize;
    let x = header.x as usize;
    let y = header.y as usize;
    let crop_width = header.crop_width as usize;
    let crop_height = header.crop_height as usize;
    let flags = header.flags;
    let info = header.info;
    let modern = header.modern;
    ensure!(
        width > 0 && height > 0 && width * height <= 16_777_216,
        "invalid RIP dimensions {width}x{height}"
    );
    ensure!(
        info[0] < 6 && info[4..7].iter().all(|d| (1..=8).contains(d)),
        "invalid RIP compression parameters {info:?}"
    );
    let mut bits = Msb { data, position: 0 };
    let q = info[0] as usize;
    let b_shift = 8 - info[4] as u32;
    let g_shift = 16 - info[5] as u32;
    let r_shift = 24 - info[6] as u32;
    let baseline = (255u32 >> info[4]) | ((255u32 >> info[5]) << 8) | ((255u32 >> info[6]) << 16);
    let mut pixels = vec![0u32; width * height];
    let alpha = flags & 0xff == 3;
    let alpha_max = if modern { 32 } else { 31 };
    if alpha {
        ensure!(
            x + crop_width <= width && y + crop_height <= height,
            "RIP crop outside canvas"
        );
        let mut line = vec![0u32; crop_width];
        for row in 0..crop_height {
            let mut a = 0i32;
            let mut rgb = 0u32;
            let mut run = 0usize;
            let mut repeat = true;
            let mut alpha_run = 0usize;
            let mut col = 0;
            let mut green_acc = 0i32;
            while col < crop_width {
                if alpha_run == 0 {
                    if bits.bit()? {
                        a += bits.signed()?;
                    }
                    ensure!(
                        (0..=alpha_max).contains(&a),
                        "invalid RIP alpha predictor {a}"
                    );
                    if a == 0 || a == alpha_max {
                        alpha_run = bits.unsigned()?;
                    }
                }
                if a == 0 {
                    ensure!(
                        alpha_run <= crop_width - col,
                        "transparent RIP run exceeds row"
                    );
                    col += alpha_run;
                    alpha_run = 0;
                    continue;
                }
                if a == alpha_max {
                    alpha_run -= 1;
                }
                if run == 0 {
                    run = bits.unsigned()?;
                    repeat = !repeat;
                    green_acc = 0;
                }
                run -= 1;
                if !repeat {
                    if bits.bit()? {
                        rgb = line[col];
                        green_acc = 0;
                    } else {
                        let raw_g = bits.optional_signed()?;
                        let b_inc = if modern {
                            bits.optional_signed()?
                        } else {
                            bits.chroma(q)?
                        };
                        let r_inc = if modern {
                            bits.optional_signed()?
                        } else {
                            bits.chroma(q)?
                        };
                        let g = if modern && info[1] & 1 != 0 {
                            let current = (rgb >> g_shift) & (255u32 >> (g_shift - 8));
                            raw_g
                                + green_acc.clamp(
                                    -(current as i32),
                                    ((255u32 >> (g_shift - 8)) - current) as i32,
                                )
                        } else {
                            raw_g
                        };
                        green_acc = g;
                        let b_max = 255u32 >> b_shift;
                        let previous_b = (rgb >> b_shift) & b_max;
                        let b = g.clamp(-(previous_b as i32), (b_max - previous_b) as i32) + b_inc;
                        let red_max = 255u32 >> (r_shift - 16);
                        let red = (rgb >> r_shift) & red_max;
                        let red = g.clamp(-(red as i32), (red_max - red) as i32) + r_inc;
                        rgb = rgb
                            .wrapping_add((b as u32).wrapping_shl(b_shift))
                            .wrapping_add((red as u32).wrapping_shl(r_shift))
                            .wrapping_add((g as u32).wrapping_shl(g_shift));
                    }
                }
                line[col] = rgb;
                let opacity = if a == alpha_max {
                    255
                } else if modern {
                    ((a as u32 * 4 + 3) * 255 + 64) / 128
                } else {
                    a as u32 * 8
                };
                pixels[(row + y) * width + x + col] = rgb.wrapping_add(baseline) | opacity << 24;
                col += 1;
            }
        }
    } else {
        let baseline = baseline | 0xff000000;
        for row in 0..height {
            let mut rgb = 0u32;
            let mut predicted = 0u32;
            let mut col = 0;
            let mut green_acc = 0i32;
            while col < width {
                let count = bits.unsigned()?;
                ensure!(count <= width - col, "RIP delta run exceeds row");
                if modern {
                    green_acc = 0;
                }
                for _ in 0..count {
                    if bits.bit()? {
                        ensure!(row > 0 || modern, "RIP predictor refers before first row");
                        rgb = if row == 0 {
                            0
                        } else {
                            pixels[(row - 1) * width + col]
                        };
                        if rgb != 0 {
                            rgb = rgb.wrapping_sub(baseline);
                        }
                        predicted = rgb;
                        green_acc = 0;
                    } else {
                        let raw_g = bits.optional_signed()?;
                        let b_inc = if modern {
                            bits.optional_signed()?
                        } else {
                            bits.chroma(q)?
                        };
                        let r_inc = if modern {
                            bits.optional_signed()?
                        } else {
                            bits.chroma(q)?
                        };
                        let g = if modern && info[1] & 1 != 0 {
                            let current = (predicted >> g_shift) & (255u32 >> (g_shift - 8));
                            raw_g
                                + green_acc.clamp(
                                    -(current as i32),
                                    ((255u32 >> (g_shift - 8)) - current) as i32,
                                )
                        } else {
                            raw_g
                        };
                        green_acc = g;
                        let common = if info[4..7] == [6, 7, 6] {
                            g >> 1
                        } else if modern && info[4..7] == [6, 8, 6] {
                            g >> 2
                        } else {
                            g
                        };
                        let b_max = 255u32 >> b_shift;
                        let red_max = 255u32 >> (r_shift - 16);
                        let previous_b = (predicted >> b_shift) & b_max;
                        let previous_r = (predicted >> r_shift) & red_max;
                        let b = if if modern {
                            info[3] & 2 != 0
                        } else {
                            info[3] != 0
                        } {
                            common.clamp(-(previous_b as i32), (b_max - previous_b) as i32)
                        } else {
                            common
                        } + b_inc;
                        let red = if if modern {
                            info[3] & 2 != 0
                        } else {
                            info[3] != 0
                        } {
                            common.clamp(-(previous_r as i32), (red_max - previous_r) as i32)
                        } else {
                            common
                        } + r_inc;
                        predicted = predicted
                            .wrapping_add((b as u32).wrapping_shl(b_shift))
                            .wrapping_add((red as u32).wrapping_shl(r_shift))
                            .wrapping_add((g as u32).wrapping_shl(g_shift));
                        rgb = predicted;
                    }
                    if rgb != 0 {
                        rgb = rgb.wrapping_add(baseline);
                    }
                    pixels[row * width + col] = rgb;
                    col += 1;
                }
                if col < width {
                    let count = bits.unsigned()?;
                    ensure!(count <= width - col, "RIP repeated run exceeds row");
                    pixels[row * width + col..row * width + col + count].fill(rgb);
                    col += count;
                }
            }
        }
    }
    let rgba = pixels
        .into_iter()
        .flat_map(|p| {
            [
                (p >> 16) as u8,
                if modern { p as u8 } else { (p >> 8) as u8 },
                if modern { (p >> 8) as u8 } else { p as u8 },
                if alpha { (p >> 24) as u8 } else { 255 },
            ]
        })
        .collect();
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
