//! Native message-window surfaces plus a portable outline-font presentation.
//! The latter does not yet reproduce the original GDI font cache, speaker
//! colour table, kinsoku rules or all text-layout flags.
use crate::{
    dialogue::TextPart,
    font::{Glyph, TextFont},
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

pub mod snapshot;

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

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageConfig {
    pub family: String,
    pub font_size: u16,
    pub speed_ms: u32,
    pub backdrop: Backdrop,
}
impl Default for MessageConfig {
    fn default() -> Self {
        // Installation defaults from the Latest Edition disc. Installed game
        // directories can omit Setup.ini after the installer applies it.
        Self {
            family: "ＭＳ Ｐゴシック".into(),
            font_size: 24,
            speed_ms: 0,
            backdrop: Backdrop {
                color: [32, 32, 160],
                source: 192,
                destination_shift: 6,
            },
        }
    }
}
impl MessageConfig {
    pub fn load(root: &Path) -> Result<Self> {
        use std::io::Read;
        let file = match std::fs::File::open(root.join("Setup.ini")) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error).context("open message defaults in Setup.ini"),
        };
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

/// Message font face and decoration, as the retail フォント page edits them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextSettings {
    /// None keeps the configured retail face (or a `--font` file).
    pub family: Option<String>,
    pub size: u16,
    /// Face weight from the descriptor's `%$-B` suffix.
    pub face_bold: bool,
    pub style: crate::font::TextStyle,
}
impl TextSettings {
    /// Setup.ini rvmmUISettings strMainFont ("family%#size%$-B%$-A") and
    /// dwMainFontStyle; disc defaults when the file is absent.
    pub fn load(root: &Path) -> Result<Self> {
        use std::io::Read;
        let text = match std::fs::File::open(root.join("Setup.ini")) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
                ensure!(bytes.len() <= 1024 * 1024, "oversized Setup.ini");
                rugp_assets::binary::decode_text(&bytes)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error).context("open text defaults in Setup.ini"),
        };
        Ok(Self::parse(&text))
    }
    fn parse(text: &str) -> Self {
        let literal = |key: &str| {
            text.lines().find_map(|line| {
                line.trim_start()
                    .strip_prefix(key)
                    .and_then(|s| s.strip_prefix('='))
                    .and_then(|s| s.trim().strip_suffix(';'))
            })
        };
        let descriptor = literal("rvmmUISettings|strMainFont").unwrap_or("\"%#24%$-B%$-A\"");
        let bits = literal("rvmmUISettings|dwMainFontStyle")
            .and_then(|v| v.strip_prefix('#'))
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0x1008);
        Self {
            family: None,
            size: MessageConfig::default().font_size,
            face_bold: descriptor.contains("%$-B"),
            style: crate::font::TextStyle::from_native(bits, descriptor.contains("%$-A")),
        }
    }
}

pub struct MessageWindow {
    source: ObjectRef,
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
            source: reference.clone(),
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
        if !self.telop() {
            paint_border(&self.border, &self.pieces, &mut frame)?;
        }
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
            rows.push((glyphs, h));
        }
        // Oversized choices (large user fonts) start at the top and clip.
        let mut y = top + (bottom - top - total).max(0) / 2;
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
                glyph.paint(&mut frame, x, y + 4 + gy, [left, y, right, y + h], [255; 3]);
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
    /// The retail 0x311/0x711 layouts (bit 0x100) draw dialogue as telop:
    /// outlined text over the scene, without the frame or backdrop fill
    /// (verified against the 2008 engine under Wine).
    pub fn telop(&self) -> bool {
        self.data.layout & 0x100 != 0
    }
    fn insets(&self, width: u32, height: u32) -> Result<[i32; 4]> {
        let g = &self.border.geometry;
        let inset = [
            i32::from(g[12]),
            i32::from(g[13]),
            width as i32 - i32::from(g[14]),
            height as i32 - i32::from(g[15]),
        ];
        ensure!(
            inset[0] >= 0 && inset[1] >= 0 && inset[2] > inset[0] && inset[3] > inset[1],
            "invalid message content insets"
        );
        Ok(inset)
    }
    pub fn compose(
        &self,
        font: &mut TextFont,
        width: u32,
        height: u32,
        page: &[TextPart],
        style: PageStyle,
    ) -> Result<Texture> {
        let mut frame = Framebuffer::transparent(width, height)?;
        if !self.telop() {
            paint_border(&self.border, &self.pieces, &mut frame)?;
        }
        let [left, top, right, bottom] = self.insets(width, height)?;
        // Wrap into lines; the speaker leads inline as 【name】.
        let mut lines: Vec<Vec<Arc<Glyph>>> = vec![Vec::new()];
        let mut widths = vec![0];
        let mut push = |font: &mut TextFont, ch: char| -> Result<()> {
            if ch == '\n' {
                lines.push(Vec::new());
                widths.push(0);
                return Ok(());
            }
            let glyph = font.glyph(ch)?;
            let last = widths.len() - 1;
            if widths[last] + glyph.advance > right - left && widths[last] > 0 {
                lines.push(Vec::new());
                widths.push(0);
            }
            let last = widths.len() - 1;
            widths[last] += glyph.advance;
            lines[last].push(glyph);
            Ok(())
        };
        for part in page {
            match part {
                TextPart::Newline => push(font, '\n')?,
                TextPart::Glyph(c) => push(font, *c)?,
                TextPart::Speaker(name) => {
                    for ch in format!("【{name}】").chars() {
                        push(font, ch)?;
                    }
                }
            }
        }
        // Retail 24 px text advances 29 px per line; its last line's top is
        // 46 px above the content bottom, and each line is centred.
        let pitch = i32::from(font.size) * 29 / 24;
        let count = lines.len() as i32;
        let first_top = if self.telop() {
            (bottom - pitch - i32::from(font.size) * 17 / 24 - (count - 1) * pitch).max(top)
        } else {
            top
        };
        let clip = [left, top, right, bottom];
        let mut end = (left, first_top);
        for (index, (line, line_width)) in lines.iter().zip(&widths).enumerate() {
            let y = first_top + index as i32 * pitch;
            // A larger user font can outgrow the window; native pagination
            // is not ported, so clip instead of failing.
            if y + pitch > bottom + pitch / 2 {
                break;
            }
            let mut x = if self.telop() {
                left + (right - left - line_width) / 2
            } else {
                left
            };
            for glyph in line {
                glyph.paint(&mut frame, x, y, clip, style.color);
                x += glyph.advance;
            }
            end = (x, y);
        }
        if style.marker {
            paint_marker(&mut frame, end.0 + 4, end.1, i32::from(font.size), clip);
        }
        Ok(Texture {
            width,
            height,
            display_scale: 1.0,
            rgba: frame.rgba,
        })
    }
}

/// The page-wait marker: a white downward triangle with a black edge.
fn paint_marker(frame: &mut Framebuffer, x: i32, y: i32, size: i32, clip: [i32; 4]) {
    let w = (size / 2).max(4);
    let h = (size * 5 / 12).max(3);
    let top = y + size / 3;
    for dy in -1..=h + 1 {
        for dx in -1..=w + 1 {
            let inside = |dx: i32, dy: i32| {
                dy >= 0 && dy <= h && {
                    let half = w * (h - dy) / (2 * h.max(1));
                    (dx - w / 2).abs() <= half
                }
            };
            let (px, py) = (x + dx, top + dy);
            if px < clip[0] || py < clip[1] || px >= clip[2] || py >= clip[3] {
                continue;
            }
            if inside(dx, dy) {
                frame.pixel(px, py, [255, 255, 255, 255]);
            } else if (-1..=1).any(|ox| (-1..=1).any(|oy| inside(dx + ox, dy + oy))) {
                frame.pixel(px, py, [0, 0, 0, 255]);
            }
        }
    }
}

/// Per-page presentation chosen by the host runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageStyle {
    /// Text fill colour (the speaker's セリフ別設定 colour).
    pub color: [u8; 3],
    /// Draw the page-wait marker after the last character.
    pub marker: bool,
}
impl Default for PageStyle {
    fn default() -> Self {
        Self {
            color: [255; 3],
            marker: false,
        }
    }
}

/// CTxtStDlg_arraySerifSetting from the title's CStdb profile, e.g.
/// "_その他/16777215/0//速瀬:水月/11591935/0//": family:given / 0xRRGGBB /
/// flag. `_その他` colours every other speaker and narration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerifColors {
    entries: Vec<(String, String, [u8; 3])>,
    other: [u8; 3],
}
impl Default for SerifColors {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            other: [255; 3],
        }
    }
}
impl SerifColors {
    pub fn parse(text: &str) -> Self {
        let mut colors = Self::default();
        for entry in text.split("//") {
            let mut fields = entry.split('/');
            let (Some(name), Some(value)) = (fields.next(), fields.next()) else {
                continue;
            };
            let Ok(value) = value.trim().parse::<u32>() else {
                continue;
            };
            let rgb = [(value >> 16) as u8, (value >> 8) as u8, value as u8];
            if name == "_その他" {
                colors.other = rgb;
            } else {
                let (family, given) = name.split_once(':').unwrap_or((name, ""));
                colors.entries.push((family.into(), given.into(), rgb));
            }
        }
        colors
    }
    /// The colour of a page whose speaker tag is `speaker` (family or given
    /// name, as the scripts write it).
    pub fn color(&self, speaker: Option<&str>) -> [u8; 3] {
        speaker
            .and_then(|name| {
                self.entries.iter().find(|(family, given, _)| {
                    name == family || name == given || name == format!("{family}{given}")
                })
            })
            .map_or(self.other, |(_, _, rgb)| *rgb)
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
    fn serif_colors_match_family_or_given_names_and_default_others() {
        let colors = SerifColors::parse(
            "_その他/16777215/0//涼宮:遙/16761008/0//速瀬:水月/11591935/0//broken//",
        );
        assert_eq!(colors.color(Some("速瀬")), [176, 224, 255]);
        assert_eq!(colors.color(Some("水月")), [176, 224, 255]);
        assert_eq!(colors.color(Some("遙")), [255, 192, 176]);
        assert_eq!(colors.color(Some("孝之")), [255; 3]);
        assert_eq!(colors.color(None), [255; 3]);
        assert_eq!(SerifColors::parse("").color(Some("速瀬")), [255; 3]);
    }
    #[test]
    fn missing_setup_uses_disc_defaults_but_invalid_existing_setup_fails() -> Result<()> {
        let directory = std::env::temp_dir().join(format!(
            "rugp-message-defaults-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        std::fs::create_dir(&directory)?;
        let config = MessageConfig::load(&directory)?;
        assert_eq!(config.family, "ＭＳ Ｐゴシック");
        assert_eq!(config.font_size, 24);
        assert_eq!(config.speed_ms, 0);
        assert_eq!(config.backdrop.color, [32, 32, 160]);
        assert_eq!(config.backdrop.source, 192);
        assert_eq!(config.backdrop.destination_shift, 6);
        std::fs::write(directory.join("Setup.ini"), b"invalid settings")?;
        assert!(MessageConfig::load(&directory).is_err());
        std::fs::remove_file(directory.join("Setup.ini"))?;
        std::fs::create_dir(directory.join("Setup.ini"))?;
        assert!(MessageConfig::load(&directory).is_err());
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

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
