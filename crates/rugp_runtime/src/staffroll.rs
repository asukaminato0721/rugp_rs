//! Portable ending credits: original labels and clock, approximate CSV layout.
//! Native GDI typography, alignment/style columns and decorations are not exact.
use crate::{
    font::{Glyph, TextFont},
    render::{Framebuffer, Texture},
};
use anyhow::{Context, Result, ensure};
use rugp_assets::layout_text::LayoutText;
use std::sync::Arc;

pub(crate) struct StaffRoll {
    pub window: u64,
    width: u32,
    height: u32,
    content_height: i32,
    glyphs: Vec<(Arc<Glyph>, i32, i32)>,
    color: [u8; 3],
    elapsed: u32,
    duration: u32,
}
impl StaffRoll {
    pub fn new(
        window: u64,
        layout: &LayoutText,
        font: &TextFont,
        size: (u32, u32),
        window_width: u32,
        duration: u32,
    ) -> Result<Self> {
        ensure!(
            size.0 > 0 && size.0 <= 4096 && size.1 > 0 && size.1 <= 4096,
            "invalid credits viewport"
        );
        ensure!(duration > 0, "invalid credits duration");
        ensure!(
            window_width >= size.0 && window_width <= 4096,
            "credits viewport exceeds target window"
        );
        ensure!(
            layout.images.iter().all(|r| r.class.is_none()),
            "embedded credits bitmaps are not implemented"
        );
        ensure!(
            layout.background_style == 0 && layout.font_options == 0 && layout.layout_options == 0,
            "unsupported credits layout options"
        );
        let mut fonts = [
            font.resized(u16::try_from(layout.font_sizes[0])?)?,
            font.resized(u16::try_from(layout.font_sizes[1])?)?,
        ];
        let mut glyphs = Vec::new();
        let mut y = 0i32;
        for line in layout.text.lines() {
            if line.is_empty() {
                y += i32::from(fonts[0].size);
                ensure!(y <= 1_000_000, "credits layout exceeds budget");
                continue;
            }
            let mut fields = line.splitn(3, ',');
            let style: usize = fields
                .next()
                .context("credits row style missing")?
                .parse()
                .context("invalid credits row style")?;
            let spacing: i32 = fields
                .next()
                .context("credits row spacing missing")?
                .parse()
                .context("invalid credits row spacing")?;
            let text = fields.next().context("credits row label missing")?;
            ensure!(
                style < fonts.len() && (0..=4096).contains(&spacing),
                "unsupported credits row style/spacing"
            );
            let font = &mut fonts[style];
            let mut row = Vec::new();
            let mut x = 0;
            let mut row_y = y;
            for ch in text.chars() {
                ensure!(
                    glyphs.len() + row.len() < 65536,
                    "credits glyph budget exceeded"
                );
                ensure!(!ch.is_control(), "unsupported credits text control");
                let glyph = font.glyph(ch)?;
                ensure!(
                    glyph.advance >= 0 && glyph.advance <= size.0 as i32,
                    "credits glyph exceeds viewport"
                );
                if x + glyph.advance > size.0 as i32 {
                    let offset = (size.0 as i32 - x) / 2;
                    glyphs.extend(row.drain(..).map(|(g, x)| (g, x + offset, row_y)));
                    x = 0;
                    row_y += i32::from(font.size);
                    ensure!(row_y <= 1_000_000, "credits layout exceeds budget");
                }
                row.push((Arc::clone(&glyph), x));
                x += glyph.advance;
            }
            let offset = (size.0 as i32 - x) / 2;
            glyphs.extend(row.into_iter().map(|(g, x)| (g, x + offset, row_y)));
            y = row_y + spacing.max(i32::from(font.size));
            ensure!(
                glyphs.len() <= 65536 && y <= 1_000_000,
                "credits layout exceeds budget"
            );
        }
        ensure!(!glyphs.is_empty(), "empty credits text");
        let center = ((window_width - size.0) / 2) as i32;
        for (_, x, _) in &mut glyphs {
            *x += center;
        }
        let color = layout.text_style;
        Ok(Self {
            window,
            width: window_width,
            height: size.1,
            content_height: y,
            glyphs,
            color: [(color >> 16) as u8, (color >> 8) as u8, color as u8],
            elapsed: 0,
            duration,
        })
    }
    pub fn advance(&mut self, dt_ms: u32) -> bool {
        let before = self.elapsed;
        self.elapsed = self.elapsed.saturating_add(dt_ms).min(self.duration);
        self.elapsed != before
    }
    pub fn finished(&self) -> bool {
        self.elapsed >= self.duration
    }
    fn top(&self) -> i32 {
        self.height as i32
            - ((u64::from(self.height) + self.content_height as u64) * u64::from(self.elapsed)
                / u64::from(self.duration)) as i32
    }
    pub fn surface(&self) -> Result<Texture> {
        let mut frame = Framebuffer::transparent(self.width, self.height)?;
        let top = self.top();
        for (glyph, x, y) in &self.glyphs {
            let y = y + top + glyph.top;
            if y >= self.height as i32 || y + glyph.height as i32 <= 0 {
                continue;
            }
            for dy in 0..glyph.height {
                for dx in 0..glyph.width {
                    let alpha = glyph.alpha[(dy * glyph.width + dx) as usize];
                    if alpha != 0 {
                        frame.pixel(
                            x + glyph.left + dx as i32,
                            y + dy as i32,
                            [self.color[0], self.color[1], self.color[2], alpha],
                        );
                    }
                }
            }
        }
        Ok(Texture {
            width: frame.width,
            height: frame.height,
            display_scale: 1.0,
            rgba: frame.rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credits_scroll_clock_clips_pixels_and_finishes_at_exact_duration() -> Result<()> {
        let glyph = Arc::new(Glyph {
            advance: 1,
            left: 0,
            top: 0,
            width: 1,
            height: 1,
            alpha: vec![255],
        });
        let mut roll = StaffRoll {
            window: 1,
            width: 2,
            height: 2,
            content_height: 2,
            glyphs: vec![(glyph, 0, 0)],
            color: [10, 20, 30],
            elapsed: 0,
            duration: 400,
        };
        assert!(roll.surface()?.rgba.iter().all(|b| *b == 0));
        assert!(roll.advance(100));
        assert_eq!(roll.top(), 1);
        assert_eq!(&roll.surface()?.rgba[8..12], &[10, 20, 30, 255]);
        roll.advance(100);
        assert_eq!(&roll.surface()?.rgba[..4], &[10, 20, 30, 255]);
        roll.advance(199);
        assert!(!roll.finished());
        roll.advance(1);
        assert!(roll.finished());
        assert!(roll.surface()?.rgba.iter().all(|b| *b == 0));
        assert!(!roll.advance(u32::MAX));
        Ok(())
    }
}
