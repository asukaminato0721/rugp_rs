//! Portable outline-font rasterization; never loads the original GDI DLLs.
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
}

pub struct TextFont {
    font: FontArc,
    scale: PxScale,
    pub source: String,
    pub size: u16,
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
        let scale = PxScale::from(f32::from(size));
        Ok(Self {
            font,
            scale,
            source,
            size,
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
                weight: Weight::BOLD,
                ..Query::default()
            }) else {
                continue;
            };
            let loaded = db.with_face_data(id, |data, index| -> Result<Self> {
                ensure!(data.len() <= FONT_LIMIT, "oversized system font");
                Self::from_bytes(data.to_vec(), index, size, name.to_owned())
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
    pub fn supports(&self, ch: char) -> bool {
        self.font.glyph_id(ch).0 != 0
    }
    /// Share the loaded outline face, but keep an independent size/glyph cache.
    pub fn resized(&self, size: u16) -> Result<Self> {
        ensure!((4..=128).contains(&size), "invalid font pixel size");
        Ok(Self {
            font: self.font.clone(),
            scale: PxScale::from(f32::from(size)),
            source: self.source.clone(),
            size,
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
        let advance = scaled.h_advance(id).round() as i32;
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
            Glyph {
                advance,
                left: bounds.min.x as i32,
                top: bounds.min.y as i32,
                width,
                height,
                alpha,
            }
        } else {
            Glyph {
                advance,
                left: 0,
                top: 0,
                width: 0,
                height: 0,
                alpha: Vec::new(),
            }
        };
        if self.cache_bytes + glyph.alpha.len() > GLYPH_LIMIT {
            self.cache.clear();
            self.cache_bytes = 0;
        }
        self.cache_bytes += glyph.alpha.len();
        let glyph = Arc::new(glyph);
        self.cache.insert(ch, Arc::clone(&glyph));
        Ok(glyph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
