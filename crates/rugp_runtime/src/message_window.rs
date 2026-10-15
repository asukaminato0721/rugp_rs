//! Native message-window surfaces plus a portable outline-font presentation.
//! The latter does not yet reproduce the original GDI font cache, speaker
//! colour table, kinsoku rules or all text-layout flags.
use crate::{
    dialogue::TextPart,
    font::TextFont,
    image_manager::ImageManager,
    render::{Framebuffer, Texture},
};
use anyhow::{Context, Result, ensure};
use rugp_assets::{
    RioArchive,
    message::{FrameImage, MessageBox},
    object::ObjectRef,
};
use std::{path::Path, sync::Arc};

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Backdrop {
    pub color: [u8; 3],
    pub source: u16,
    pub destination_shift: u8,
}
impl Backdrop {
    /// rvmm 0x10075470 -> 0x1002f3f0, non-gradient 32-bit path:
    /// min(255, (colour * source + (background << shift)) >> 8).
    /// This is not conventional source-over alpha blending.
    pub fn paint(&self, frame: &mut Framebuffer, rect: (i32, i32, u32, u32)) {
        if self.source == 0 {
            return;
        }
        let (x, y, w, h) = rect;
        for y in i64::from(y).max(0)..(i64::from(y) + i64::from(h)).min(i64::from(frame.height)) {
            for x in i64::from(x).max(0)..(i64::from(x) + i64::from(w)).min(i64::from(frame.width))
            {
                let at = (y as usize * frame.width as usize + x as usize) * 4;
                for c in 0..3 {
                    frame.rgba[at + c] = if self.destination_shift == 0 {
                        self.color[c]
                    } else {
                        ((u32::from(self.color[c]) * u32::from(self.source)
                            + (u32::from(frame.rgba[at + c]) << self.destination_shift))
                            >> 8)
                            .min(255) as u8
                    };
                }
            }
        }
    }
}

pub struct MessageConfig {
    pub family: String,
    pub font_size: u16,
    pub speed_ms: u32,
    pub backdrop: Backdrop,
}
impl MessageConfig {
    pub fn load(root: &Path) -> Result<Self> {
        use std::io::Read;
        let file = std::fs::File::open(root.join("Setup.ini"))
            .context("open message defaults in Setup.ini")?;
        ensure!(file.metadata()?.len() <= 1024 * 1024, "oversized Setup.ini");
        let mut bytes = Vec::new();
        file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 1024 * 1024, "oversized Setup.ini");
        Self::parse(&rugp_assets::binary::decode_text(&bytes))
    }
    fn parse(text: &str) -> Result<Self> {
        // Read only literal installation defaults, not registry/path commands.
        let literal = |key: &str| -> Result<&str> {
            text.lines()
                .find_map(|line| {
                    line.trim_start()
                        .strip_prefix(key)
                        .and_then(|s| s.strip_prefix('='))
                })
                .and_then(|s| s.trim().strip_suffix(';'))
                .context(format!("missing literal message setting {key}"))
        };
        let number = |key: &str| -> Result<i32> {
            literal(key)?
                .strip_prefix('#')
                .context("message setting is not numeric")?
                .parse()
                .context("invalid numeric message setting")
        };
        let font = literal("rvmmUISettings|strMainFont")?
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .context("invalid main font setting")?;
        let (family, descriptor) = font.split_once("%#").context("font size missing")?;
        let font_size: u16 = descriptor
            .split('%')
            .next()
            .unwrap()
            .parse()
            .context("invalid font size")?;
        ensure!((4..=128).contains(&font_size), "invalid font size");
        let color = number("rvmmBoxSettings|nWndBaseColor1")? as u32;
        let source = number("rvmmBoxSettings|nWndBaseRatioSrc")?;
        let destination = number("rvmmBoxSettings|nWndBaseRatioDst")?;
        let speed = number("rvmmUISettings|nTextSpeed")?;
        ensure!(
            (0..=256).contains(&source) && (0..=8).contains(&destination),
            "unsupported window blend coefficients"
        );
        ensure!(
            (0..=3000).contains(&speed),
            "unsupported default text speed"
        );
        ensure!(
            number("rvmmBoxSettings|nWndFrameLevel")? == 0,
            "unsupported window frame setting"
        );
        // Native default is 128; only 1/2 select gradient branches.
        if text.lines().any(|l| {
            l.trim_start()
                .starts_with("rvmmBoxSettings|nWndBaseGradation=")
        }) {
            ensure!(
                ![1, 2].contains(&number("rvmmBoxSettings|nWndBaseGradation")?),
                "gradient message backdrops are not implemented"
            );
        }
        Ok(Self {
            family: family.to_owned(),
            font_size,
            speed_ms: speed as u32,
            backdrop: Backdrop {
                color: [(color >> 16) as u8, (color >> 8) as u8, color as u8],
                source: source as u16,
                destination_shift: destination as u8,
            },
        })
    }
}

pub struct MessageWindow {
    pub data: MessageBox,
    pub border: FrameImage,
    pieces: Vec<Arc<Texture>>,
    pub page: Vec<TextPart>,
}
pub(crate) type SelectorRect = (i32, i32, u32, u32);
impl MessageWindow {
    pub fn load(
        archive: &RioArchive,
        images: &mut ImageManager,
        reference: &ObjectRef,
    ) -> Result<Self> {
        let read = |r: &ObjectRef| -> Result<Vec<u8>> {
            let size = r.size.context("message resource size missing")?;
            ensure!(size <= 16 * 1024 * 1024, "oversized message resource");
            archive.read_at(
                r.offset.context("message resource extent missing")?,
                size as usize,
            )
        };
        let class = reference.class.as_ref().context("missing message class")?;
        ensure!(
            class.name == "CMsgSelBox",
            "unsupported message-window class"
        );
        let data = MessageBox::parse(&read(reference)?, class.schema, archive.address_shift)?;
        ensure!(
            data.children.is_empty() && data.surface.class.is_none(),
            "message child/source surfaces not implemented"
        );
        let class = data
            .frame
            .class
            .as_ref()
            .context("missing message border")?;
        ensure!(
            class.name == "CFrameImage",
            "unsupported message border class"
        );
        let border = FrameImage::parse(&read(&data.frame)?, class.schema, archive.address_shift)?;
        let pieces = border
            .pieces
            .iter()
            .map(|r| images.load(r))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            data,
            border,
            pieces,
            page: vec![],
        })
    }
    /// Approximate choice layout: wrapped rows with explicit hover/press
    /// feedback and matching pointer hit rectangles, independent of GDI.
    pub(crate) fn compose_selectors(
        &self,
        font: &mut TextFont,
        width: u32,
        height: u32,
        entries: &[String],
        hover: Option<usize>,
        pressed: Option<usize>,
    ) -> Result<(Texture, Vec<SelectorRect>)> {
        let mut frame = Framebuffer::transparent(width, height)?;
        paint_border(&self.border, &self.pieces, &mut frame)?;
        let g = &self.border.geometry;
        let left = i32::from(g[12]);
        let top = i32::from(g[13]);
        let right = width as i32 - i32::from(g[14]);
        let bottom = height as i32 - i32::from(g[15]);
        ensure!(
            left >= 0 && top >= 0 && right > left + 12 && bottom > top,
            "invalid text selector insets"
        );
        let line = i32::from(font.size);
        let mut rows = Vec::new();
        let mut total = 0;
        for text in entries {
            let mut glyphs = Vec::new();
            let mut x = left + 6;
            let mut y = 0;
            for ch in text.chars() {
                if ch == '\n' {
                    x = left + 6;
                    y += line;
                    continue;
                }
                let glyph = font.glyph(ch)?;
                ensure!(
                    glyph.advance <= right - left - 12,
                    "selector glyph exceeds row width"
                );
                if x + glyph.advance > right - 6 {
                    x = left + 6;
                    y += line;
                }
                glyphs.push((Arc::clone(&glyph), x, y));
                x += glyph.advance;
            }
            let h = y + line + 8;
            total += h;
            ensure!(
                total <= bottom - top,
                "text selectors exceed portable viewport"
            );
            rows.push((glyphs, h));
        }
        let mut y = top + (bottom - top - total) / 2;
        let mut hits = Vec::new();
        for (index, (glyphs, h)) in rows.into_iter().enumerate() {
            hits.push((left, y, (right - left) as u32, h as u32));
            if hover == Some(index) {
                let color = if pressed == Some(index) {
                    [24, 60, 84, 245]
                } else {
                    [32, 100, 136, 230]
                };
                for dy in y..y + h {
                    for dx in left..right {
                        frame.pixel(dx, dy, color);
                    }
                }
            }
            for (glyph, x, gy) in glyphs {
                for sy in 0..glyph.height {
                    for sx in 0..glyph.width {
                        let dx = x + glyph.left + sx as i32;
                        let dy = y + 4 + gy + glyph.top + sy as i32;
                        if dx >= left && dx < right && dy >= y && dy < y + h {
                            frame.pixel(
                                dx,
                                dy,
                                [255, 255, 255, glyph.alpha[(sy * glyph.width + sx) as usize]],
                            );
                        }
                    }
                }
            }
            y += h;
        }
        Ok((
            Texture {
                width,
                height,
                display_scale: 1.0,
                rgba: frame.rgba,
            },
            hits,
        ))
    }
    pub fn compose(
        &self,
        font: &mut TextFont,
        width: u32,
        height: u32,
        page: &[TextPart],
    ) -> Result<Texture> {
        let mut frame = Framebuffer::transparent(width, height)?;
        paint_border(&self.border, &self.pieces, &mut frame)?;
        let g = &self.border.geometry;
        let left = i32::from(g[12]);
        let top = i32::from(g[13]);
        let right = width as i32 - i32::from(g[14]);
        let bottom = height as i32 - i32::from(g[15]);
        ensure!(
            left >= 0 && top >= 0 && right > left && bottom > top,
            "invalid message content insets"
        );
        let mut x = left;
        let mut y = top;
        // Portable preview metrics: exact native layout/name styles remain
        // separate work, not inferred from CViewBox's saved scroll position.
        let line = i32::from(font.size);
        for part in page {
            let text = match part {
                TextPart::Newline => {
                    x = left;
                    y += line;
                    continue;
                }
                TextPart::Glyph(c) => c.to_string(),
                TextPart::Speaker(name) => {
                    if x != left {
                        y += line;
                    }
                    x = left;
                    format!("{name}\n")
                }
            };
            for ch in text.chars() {
                if ch == '\n' {
                    x = left;
                    y += line;
                    continue;
                }
                let glyph = font.glyph(ch)?;
                if x + glyph.advance > right {
                    x = left;
                    y += line;
                }
                ensure!(
                    y + line <= bottom,
                    "message text exceeds portable viewport; native pagination/layout not implemented"
                );
                for gy in 0..glyph.height {
                    for gx in 0..glyph.width {
                        let dx = x + glyph.left + gx as i32;
                        let dy = y + glyph.top + gy as i32;
                        if dx >= left && dx < right && dy >= top && dy < bottom {
                            frame.pixel(
                                dx,
                                dy,
                                [255, 255, 255, glyph.alpha[(gy * glyph.width + gx) as usize]],
                            );
                        }
                    }
                }
                x += glyph.advance;
            }
        }
        Ok(Texture {
            width,
            height,
            display_scale: 1.0,
            rgba: frame.rgba,
        })
    }
}
fn paint_border(border: &FrameImage, p: &[Arc<Texture>], frame: &mut Framebuffer) -> Result<()> {
    ensure!(p.len() == 8, "message border must have eight pieces");
    let g = border.geometry.map(i32::from);
    let w = frame.width as i32;
    let h = frame.height as i32;
    let pw = |i: usize| p[i].width as i32;
    let ph = |i: usize| p[i].height as i32;
    // rvmm 0x10076b20. Edges precede corners, with asymmetric corner sizes.
    let placements = [
        (4, pw(0) + g[0], g[8], w - pw(0) - pw(1), ph(4)),
        (7, pw(2) + g[4], h - g[11] - ph(7), w - pw(2) - pw(3), ph(7)),
        (5, g[9], ph(0) + g[1], pw(5), h - ph(0) - ph(2)),
        (6, w - g[10] - pw(6), ph(1) + g[3], pw(6), h - ph(1) - ph(3)),
        (3, w - g[6] - pw(3), h - g[7] - ph(3), pw(3), ph(3)),
        (2, g[4], h - g[5] - ph(2), pw(2), ph(2)),
        (1, w - g[2] - pw(1), g[3], pw(1), ph(1)),
        (0, g[0], g[1], pw(0), ph(0)),
    ];
    for (piece, x, y, width, height) in placements {
        ensure!(
            width >= 0 && height >= 0,
            "message window too small for border"
        );
        // Retail edges are invariant along their extended axis. Until the
        // general brush's tiling rules are ported, reject other patterns.
        if piece >= 4 {
            ensure!(
                invariant_edge(&p[piece], piece == 4 || piece == 7),
                "non-invariant border brush is not implemented"
            );
        }
        frame.blit(&p[piece], (x, y, width as u32, height as u32), 255)?;
    }
    Ok(())
}
fn invariant_edge(p: &Texture, horizontal: bool) -> bool {
    (0..p.height).all(|y| {
        (0..p.width).all(|x| {
            let at = (y * p.width + x) as usize * 4;
            let base = if horizontal { y * p.width } else { x } as usize * 4;
            p.rgba[at..at + 4] == p.rgba[base..base + 4]
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn additive_window_background_uses_shift_not_opacity() -> Result<()> {
        let mut frame = Framebuffer::new(3, 1)?;
        frame.clear([200, 120, 40]);
        Backdrop {
            color: [32, 32, 160],
            source: 192,
            destination_shift: 6,
        }
        .paint(&mut frame, (-1, 0, 3, 1));
        assert_eq!(&frame.rgba[..4], &[74, 54, 130, 255]);
        assert_eq!(&frame.rgba[8..], &[200, 120, 40, 255]);
        Backdrop {
            color: [255; 3],
            source: 256,
            destination_shift: 8,
        }
        .paint(&mut frame, (0, 0, 1, 1));
        assert_eq!(&frame.rgba[..4], &[255; 4]);
        Backdrop {
            color: [10; 3],
            source: 1,
            destination_shift: 0,
        }
        .paint(&mut frame, (0, 0, 1, 1));
        assert_eq!(&frame.rgba[..4], &[10, 10, 10, 255]); // native zero-shift branch ignores source weight
        Backdrop {
            color: [255; 3],
            source: 0,
            destination_shift: 1,
        }
        .paint(&mut frame, (0, 0, 1, 1));
        assert_eq!(&frame.rgba[..4], &[10, 10, 10, 255]); // source=0 skips painting
        Ok(())
    }
    #[test]
    fn asymmetric_corners_edges_and_transparent_center() -> Result<()> {
        let mut fixture = vec![0; 20]; // prefix + padding + eight null references
        fixture.extend([0; 32]);
        let mut border = FrameImage::parse(&fixture, 1, 0)?;
        let mut pieces = Vec::new();
        for (i, (width, height)) in [
            (2, 2),
            (3, 3),
            (2, 2),
            (2, 2),
            (2, 1),
            (1, 2),
            (1, 2),
            (2, 1),
        ]
        .into_iter()
        .enumerate()
        {
            pieces.push(Arc::new(Texture {
                width,
                height,
                display_scale: 1.0,
                rgba: [i as u8 + 1, 0, 0, 255].repeat(width as usize * height as usize),
            }));
        }
        let mut frame = Framebuffer::transparent(12, 10)?;
        paint_border(&border, &pieces, &mut frame)?;
        let pixel =
            |x: usize, y: usize| -> &[u8] { &frame.rgba[(y * 12 + x) * 4..(y * 12 + x) * 4 + 4] };
        assert_eq!(pixel(0, 0), &[1, 0, 0, 255]);
        assert_eq!(pixel(11, 2), &[2, 0, 0, 255]);
        assert_eq!(pixel(0, 9), &[3, 0, 0, 255]);
        assert_eq!(pixel(11, 9), &[4, 0, 0, 255]);
        assert_eq!(pixel(5, 0), &[5, 0, 0, 255]);
        assert_eq!(pixel(0, 5), &[6, 0, 0, 255]);
        assert_eq!(pixel(11, 5), &[7, 0, 0, 255]);
        assert_eq!(pixel(5, 9), &[8, 0, 0, 255]);
        assert_eq!(pixel(5, 5), &[0; 4]);
        assert!(paint_border(&border, &pieces, &mut Framebuffer::transparent(1, 1)?).is_err());
        border.geometry[0] = -1;
        let mut clipped = Framebuffer::transparent(12, 10)?;
        paint_border(&border, &pieces, &mut clipped)?;
        assert_eq!(&clipped.rgba[..4], &[1, 0, 0, 255]);
        Arc::get_mut(&mut pieces[4]).unwrap().rgba[4] = 99;
        assert!(paint_border(&border, &pieces, &mut clipped).is_err());
        Ok(())
    }
    #[test]
    fn literal_setup_defaults_are_not_commands() -> Result<()> {
        let text = "rvmmUISettings|strMainFont=\"Example%#24%$-B%$-A\";\nrvmmUISettings|nTextSpeed=#0;\nrvmmBoxSettings|nWndFrameLevel=#0;\nrvmmBoxSettings|nWndBaseRatioSrc=#192;\nrvmmBoxSettings|nWndBaseRatioDst=#6;\nrvmmBoxSettings|nWndBaseColor1=#2105504;";
        let cfg = MessageConfig::parse(text)?;
        assert_eq!(cfg.family, "Example");
        assert_eq!(cfg.font_size, 24);
        assert_eq!(cfg.backdrop.color, [32, 32, 160]);
        assert!(MessageConfig::parse(&text.replace("#192", "<COMMAND>")).is_err());
        assert!(MessageConfig::parse(&text.replace("#6", "#31")).is_err());
        assert!(
            MessageConfig::parse(&format!("{text}\nrvmmBoxSettings|nWndBaseGradation=#1;"))
                .is_err()
        );
        Ok(())
    }
}
