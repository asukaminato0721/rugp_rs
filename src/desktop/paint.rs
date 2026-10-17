//! Plain host overlays drawn into the game framebuffer.
use anyhow::Result;
use rugp_runtime::{font::TextFont, render::Framebuffer};

pub(super) fn rect(frame: &mut Framebuffer, bounds: (i32, i32, i32, i32), color: [u8; 4]) {
    for y in bounds.1..bounds.3 {
        for x in bounds.0..bounds.2 {
            frame.pixel(x, y, color);
        }
    }
}
pub(super) fn text(
    frame: &mut Framebuffer,
    font: &mut TextFont,
    mut x: i32,
    y: i32,
    text: &str,
) -> Result<()> {
    for ch in text.chars() {
        let glyph = font.glyph(ch)?;
        if x + glyph.advance > 865 {
            break;
        }
        for gy in 0..glyph.height {
            for gx in 0..glyph.width {
                frame.pixel(
                    x + glyph.left + gx as i32,
                    y + glyph.top + gy as i32,
                    [242, 246, 252, glyph.alpha[(gy * glyph.width + gx) as usize]],
                );
            }
        }
        x += glyph.advance;
    }
    Ok(())
}
