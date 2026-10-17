//! Portable outline-font rasterization; never loads the original GDI DLLs.
use crate::render::Framebuffer;
use ab_glyph::{Font, FontArc, FontVec, PxScale, ScaleFont, point};
use anyhow::{Context, Result, bail, ensure};
use std::{collections::BTreeMap, path::Path, sync::Arc};

const FONT_LIMIT: usize = 64 * 1024 * 1024;
const GLYPH_LIMIT: usize = 8 * 1024 * 1024;

pub struct Glyph {
    pub advance: i32,
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub alpha: Vec<u8>,
    /// Black shadow/outline coverage with the same extent as `alpha`;
    /// empty for an unstyled face.
    pub edge: Vec<u8>,
}
impl Glyph {
    /// Draw at a pen position, clipped to `[left, top, right, bottom)`:
    /// the black edge first, then the face in `color` over it.
    pub fn paint(&self, frame: &mut Framebuffer, x: i32, y: i32, clip: [i32; 4], color: [u8; 3]) {
        for (mask, rgb) in [(&self.edge, [0; 3]), (&self.alpha, color)] {
            if mask.is_empty() {
                continue;
            }
            for gy in 0..self.height as i32 {
                let dy = y + self.top + gy;
                if dy < clip[1] || dy >= clip[3] {
                    continue;
                }
                for gx in 0..self.width as i32 {
                    let dx = x + self.left + gx;
                    if dx >= clip[0] && dx < clip[2] {
                        let a = mask[(gy * self.width as i32 + gx) as usize];
                        frame.pixel(dx, dy, [rgb[0], rgb[1], rgb[2], a]);
                    }
                }
            }
        }
    }
}

/// Message text decoration. Vm60 10003608 stores the フォント page as
/// dwMainFontStyle bits: 1 太字, 4 影, 8 縁取り, 0x1008 縁取り（強）. The
/// antialias flag is the `%$-A` suffix of the font descriptor (UnivUI).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TextStyle {
    pub antialias: bool,
    /// Emboldening on top of the face weight.
    pub bold: bool,
    pub shadow: bool,
    /// 0 none, 1 縁取り, 2 縁取り（強）.
    pub outline: u8,
}
impl Default for TextStyle {
    fn default() -> Self {
        Self {
            antialias: true,
            bold: false,
            shadow: false,
            outline: 0,
        }
    }
}
impl TextStyle {
    pub fn from_native(bits: u32, antialias: bool) -> Self {
        Self {
            antialias,
            bold: bits & 1 != 0,
            shadow: bits & 4 != 0,
            outline: if bits & 0x1008 == 0x1008 {
                2
            } else {
                u8::from(bits & 8 != 0)
            },
        }
    }
    /// Pixels a styled glyph may extend beyond its face coverage.
    fn padding(self) -> u32 {
        u32::from(self.outline).max(if self.shadow { 2 } else { 0 }) + u32::from(self.bold)
    }
}

/// Installed families that can render Japanese text, sorted by name.
pub fn japanese_families() -> Vec<String> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut names: Vec<String> = db
        .faces()
        .filter(|face| {
            db.with_face_data(face.id, |data, index| {
                ab_glyph::FontRef::try_from_slice_and_index(data, index)
                    .is_ok_and(|font| ['あ', '漢'].iter().all(|&c| font.glyph_id(c).0 != 0))
            })
            .unwrap_or(false)
        })
        .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
        .collect();
    names.sort();
    names.dedup();
    names
}

pub struct TextFont {
    font: FontArc,
    index: u32,
    scale: PxScale,
    pub source: String,
    pub size: u16,
    style: TextStyle,
    /// A bold face was requested but only a regular one exists; widen like
    /// GDI's simulated bold.
    synthetic_bold: bool,
    /// The requested face is proportional (ＭＳ Ｐゴシック) but the
    /// substitute is not: narrow wide glyphs to their ink like GDI would.
    proportional: bool,
    cache: BTreeMap<char, Arc<Glyph>>,
    cache_bytes: usize,
}
impl TextFont {
    pub fn from_bytes(bytes: Vec<u8>, index: u32, size: u16, source: String) -> Result<Self> {
        ensure!(bytes.len() <= FONT_LIMIT, "font exceeds 64 MiB limit");
        ensure!((4..=128).contains(&size), "invalid font pixel size");
        let font = FontArc::new(
            FontVec::try_from_vec_and_index(bytes, index)
                .map_err(|_| anyhow::anyhow!("invalid outline font or face index"))?,
        );
        let scale = em_scale(&font, size);
        Ok(Self {
            font,
            index,
            scale,
            source,
            size,
            style: TextStyle::default(),
            synthetic_bold: false,
            proportional: false,
            cache: BTreeMap::new(),
            cache_bytes: 0,
        })
    }
    pub fn from_file(path: &Path, index: u32, size: u16) -> Result<Self> {
        use std::io::Read;
        let file =
            std::fs::File::open(path).with_context(|| format!("open font {}", path.display()))?;
        ensure!(
            file.metadata()?.len() <= FONT_LIMIT as u64,
            "oversized font"
        );
        let mut bytes = Vec::new();
        file.take(FONT_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
        Self::from_bytes(bytes, index, size, path.display().to_string())
    }
    /// Prefer the configured retail face, then known Japanese system faces.
    /// Fontconfig is parsed as data by fontdb on Linux; no native engine or
    /// font-discovery subprocess is executed. Missing fonts are an error.
    pub fn system(family: &str, size: u16) -> Result<Self> {
        Self::system_weighted(family, size, true)
    }
    /// As [`Self::system`]; `bold` is the descriptor's `%$-B` face weight.
    pub fn system_weighted(family: &str, size: u16, bold: bool) -> Result<Self> {
        use fontdb::{Database, Family, Query, Weight};
        let mut db = Database::new();
        db.load_system_fonts();
        for name in [
            family,
            "MS PGothic",
            "MS Gothic",
            "Yu Gothic",
            "Meiryo",
            "Noto Sans CJK JP",
            "Noto Sans JP",
            "IPA PGothic",
            "IPAGothic",
            "Hiragino Kaku Gothic ProN",
        ] {
            let Some(id) = db.query(&Query {
                families: &[Family::Name(name)],
                weight: if bold { Weight::BOLD } else { Weight::NORMAL },
                ..Query::default()
            }) else {
                continue;
            };
            let synthetic_bold = bold && db.face(id).is_some_and(|face| face.weight.0 < 600);
            let loaded = db.with_face_data(id, |data, index| -> Result<Self> {
                ensure!(data.len() <= FONT_LIMIT, "oversized system font");
                let mut font = Self::from_bytes(data.to_vec(), index, size, name.to_owned())?;
                font.synthetic_bold = synthetic_bold;
                font.proportional = is_proportional(family) && !is_proportional(name);
                Ok(font)
            });
            if let Some(Ok(font)) = loaded
                && ['あ', '漢'].into_iter().all(|c| font.supports(c))
            {
                if name != family {
                    eprintln!(
                        "Font {family:?} unavailable; using {name:?} (rasterization/metrics may differ)."
                    );
                }
                return Ok(font);
            }
        }
        bail!("no Japanese outline font found; provide --font with a Japanese TTF/OTF/TTC file")
    }
    /// The outline file bytes and face index, for hosts with their own text stack.
    pub fn face(&self) -> (&[u8], u32) {
        (self.font.font_data(), self.index)
    }
    pub fn style(&self) -> TextStyle {
        self.style
    }
    pub fn set_style(&mut self, style: TextStyle) {
        ensure_style(style);
        if style != self.style {
            self.style = style;
            self.cache.clear();
            self.cache_bytes = 0;
        }
    }
    pub fn supports(&self, ch: char) -> bool {
        self.font.glyph_id(ch).0 != 0
    }
    /// Share the loaded outline face, but keep an independent size/glyph
    /// cache. The copy is unstyled (credits and host UI draw plain glyphs).
    pub fn resized(&self, size: u16) -> Result<Self> {
        ensure!((4..=128).contains(&size), "invalid font pixel size");
        Ok(Self {
            font: self.font.clone(),
            index: self.index,
            scale: em_scale(&self.font, size),
            source: self.source.clone(),
            size,
            style: TextStyle::default(),
            synthetic_bold: false,
            proportional: false,
            cache: BTreeMap::new(),
            cache_bytes: 0,
        })
    }
    pub fn glyph(&mut self, ch: char) -> Result<Arc<Glyph>> {
        if let Some(glyph) = self.cache.get(&ch) {
            return Ok(Arc::clone(glyph));
        }
        let id = self.font.glyph_id(ch);
        ensure!(
            id.0 != 0,
            "selected font has no glyph for U+{:04X}",
            ch as u32
        );
        let scaled = self.font.as_scaled(self.scale);
        let mut advance = scaled.h_advance(id).round() as i32;
        let baseline = scaled.ascent().round();
        let outlined = self
            .font
            .outline_glyph(id.with_scale_and_position(self.scale, point(0.0, baseline)));
        let glyph = if let Some(outline) = outlined {
            let bounds = outline.px_bounds();
            let width = bounds.width() as u32;
            let height = bounds.height() as u32;
            ensure!(width <= 512 && height <= 512, "oversized glyph bitmap");
            let mut alpha = vec![0; width as usize * height as usize];
            outline.draw(|x, y, v| {
                alpha[(y * width + x) as usize] = (v * 255.0).round().clamp(0.0, 255.0) as u8;
            });
            let mut left = bounds.min.x as i32;
            if self.proportional && advance >= i32::from(self.size) * 9 / 10 {
                let pad = (i32::from(self.size) / 24).max(1);
                advance = width as i32 + 2 * pad;
                left = pad;
            }
            styled(
                self.synthetic_bold,
                Glyph {
                    advance,
                    left,
                    top: bounds.min.y as i32,
                    width,
                    height,
                    alpha,
                    edge: Vec::new(),
                },
                self.style,
            )
        } else {
            Glyph {
                advance,
                left: 0,
                top: 0,
                width: 0,
                height: 0,
                alpha: Vec::new(),
                edge: Vec::new(),
            }
        };
        if self.cache_bytes + glyph.alpha.len() > GLYPH_LIMIT {
            self.cache.clear();
            self.cache_bytes = 0;
        }
        self.cache_bytes += glyph.alpha.len() + glyph.edge.len();
        let glyph = Arc::new(glyph);
        self.cache.insert(ch, Arc::clone(&glyph));
        Ok(glyph)
    }
}

/// MS PGothic-style names: "ＭＳ Ｐゴシック", "MS PGothic", "IPA PGothic".
fn is_proportional(family: &str) -> bool {
    family.contains('Ｐ')
        || family.contains(" P")
        || family.contains("PGothic")
        || family.contains("PMincho")
}

/// GDI sizes the retail face (MS PGothic, whose ascent+descent is one em)
/// by its em, while ab_glyph scales ascent-descent. Scale by em so every
/// substitute face draws the same glyph size.
fn em_scale(font: &FontArc, size: u16) -> PxScale {
    let em = font.units_per_em().unwrap_or(1000.0);
    PxScale::from(f32::from(size) * font.height_unscaled() / em)
}

fn ensure_style(style: TextStyle) {
    debug_assert!(style.outline <= 2, "invalid outline level");
}

/// Apply decoration to face coverage: binarize without antialiasing, widen
/// for emboldening, then derive the black outline (square dilation) and the
/// down-right shadow from the final face.
fn styled(synthetic_bold: bool, glyph: Glyph, style: TextStyle) -> Glyph {
    let widen = i32::from(style.bold) + i32::from(synthetic_bold);
    if style == TextStyle::default() && widen == 0 {
        return glyph;
    }
    let pad = style.padding() as i32 + i32::from(synthetic_bold);
    let (w, h) = (glyph.width as i32 + 2 * pad, glyph.height as i32 + 2 * pad);
    let mut face = vec![0u8; (w * h) as usize];
    let source = |x: i32, y: i32| -> u8 {
        let (x, y) = (x - pad, y - pad);
        if x < 0 || y < 0 || x >= glyph.width as i32 || y >= glyph.height as i32 {
            0
        } else {
            glyph.alpha[(y * glyph.width as i32 + x) as usize]
        }
    };
    for y in 0..h {
        for x in 0..w {
            let mut a = source(x, y);
            for shift in 1..=widen {
                a = a.max(source(x - shift, y));
            }
            if !style.antialias {
                a = if a >= 128 { 255 } else { 0 };
            }
            face[(y * w + x) as usize] = a;
        }
    }
    let at = |x: i32, y: i32| -> u8 {
        if x < 0 || y < 0 || x >= w || y >= h {
            0
        } else {
            face[(y * w + x) as usize]
        }
    };
    let radius = i32::from(style.outline);
    let mut edge = vec![0u8; face.len()];
    for y in 0..h {
        for x in 0..w {
            let mut a = 0;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    a = a.max(at(x + dx, y + dy));
                }
            }
            if style.shadow {
                a = a.max(at(x - 2, y - 2));
            }
            // GDI draws the outline as offset solid text; antialiased
            // coverage alone leaves it translucent.
            edge[(y * w + x) as usize] = a.saturating_mul(2);
        }
    }
    Glyph {
        advance: glyph.advance + widen,
        left: glyph.left - pad,
        top: glyph.top - pad,
        width: w as u32,
        height: h as u32,
        alpha: face,
        edge,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn styles_decode_native_bits_and_decorate_coverage() {
        let strong = TextStyle::from_native(0x1008, true);
        assert_eq!(
            (strong.outline, strong.bold, strong.shadow),
            (2, false, false)
        );
        assert_eq!(TextStyle::from_native(8, true).outline, 1);
        assert!(TextStyle::from_native(5, true).bold && TextStyle::from_native(5, true).shadow);
        let dot = Glyph {
            advance: 3,
            left: 1,
            top: 1,
            width: 1,
            height: 1,
            alpha: vec![200],
            edge: Vec::new(),
        };
        let outlined = styled(false, dot, strong);
        assert_eq!((outlined.width, outlined.left, outlined.top), (5, -1, -1));
        assert_eq!(outlined.alpha.iter().filter(|a| **a > 0).count(), 1);
        assert!(outlined.edge.iter().all(|a| *a == 255));
        let mut frame = Framebuffer::transparent(4, 4).unwrap();
        outlined.paint(&mut frame, 0, 0, [0, 0, 4, 4], [255; 3]);
        let pixel = |x: usize, y: usize| &frame.rgba[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
        assert_eq!(pixel(0, 0), [0, 0, 0, 255]); // outline only
        assert!(pixel(1, 1)[0] > 150); // white face over its own outline
    }
    #[test]
    fn invalid_font_and_size_are_rejected() {
        assert!(TextFont::from_bytes(vec![0; 64], 0, 24, "test".into()).is_err());
        assert!(TextFont::from_bytes(vec![], 0, 0, "test".into()).is_err());
    }
    #[test]
    #[ignore = "requires an installed Japanese outline font"]
    fn japanese_system_font_produces_cached_coverage() -> Result<()> {
        let mut font = TextFont::system("ＭＳ Ｐゴシック", 24)?;
        let glyph = font.glyph('あ')?;
        assert!(glyph.advance > 0 && glyph.width > 0 && glyph.height > 0);
        assert!(glyph.alpha.iter().any(|a| *a > 0));
        assert!(Arc::ptr_eq(&glyph, &font.glyph('あ')?));
        assert_eq!(font.glyph(' ')?.width, 0);
        assert!(font.glyph('\u{10ffff}').is_err());
        Ok(())
    }
}
