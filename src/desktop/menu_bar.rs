//! The retail window's menu bar (rvmm.dll MENU 5000), drawn above the game in
//! a window. In fullscreen it is hidden until the pointer reaches the top edge
//! or Alt is pressed, as the retail manual describes.
use anyhow::Result;
use rugp_runtime::{font::TextFont, render::Framebuffer};

pub const HEIGHT: i32 = 20;
pub const TITLES: [&str; 4] = ["画面サイズ(&S)", "設定(&C)", "終了(&X)", "ヘルプ(&H)"];
/// 終了 is a command on the bar itself, not a dropdown.
pub const QUIT: usize = 2;
const PADDING: i32 = 6;
const BAR: [u8; 4] = [236, 233, 216, 255];
const HIGHLIGHT: [u8; 4] = [49, 106, 197, 255];

#[derive(Default)]
pub(super) struct MenuBar {
    /// Title under the pointer.
    pub hover: Option<usize>,
    /// Fullscreen: the pointer touched the top edge.
    pub reveal: bool,
    /// A press on a title, swallowed with its release.
    pub pressed: bool,
    /// Title x ranges from the last render.
    spans: Vec<(i32, i32)>,
}
impl MenuBar {
    pub fn hit(&self, x: i32, y: i32) -> Option<usize> {
        if !(0..HEIGHT).contains(&y) {
            return None;
        }
        self.spans
            .iter()
            .position(|&(left, right)| (left..right).contains(&x))
    }
    /// Where a title's dropdown opens.
    pub fn left(&self, index: usize) -> i32 {
        self.spans.get(index).map_or(0, |span| span.0)
    }
    /// `open` is the title whose dropdown is showing.
    pub fn render(
        &mut self,
        font: &mut TextFont,
        width: u32,
        open: Option<usize>,
    ) -> Result<Framebuffer> {
        let mut frame = Framebuffer::new(width, HEIGHT as u32)?;
        for pixel in frame.rgba.as_chunks_mut::<4>().0 {
            *pixel = BAR;
        }
        self.spans.clear();
        let mut x = 0;
        for (index, title) in TITLES.iter().enumerate() {
            let label = title.replace('&', "");
            let mut advance = 0;
            for ch in label.chars() {
                advance += font.glyph(ch)?.advance;
            }
            let span = (x, x + advance + 2 * PADDING);
            let lit = open.or(self.hover) == Some(index);
            if lit {
                for y in 0..HEIGHT {
                    for px in span.0..span.1 {
                        frame.pixel(px, y, HIGHLIGHT);
                    }
                }
            }
            let color = if lit { [255; 3] } else { [0; 3] };
            let mut pen = x + PADDING;
            let clip = [0, 0, width as i32, HEIGHT];
            for ch in label.chars() {
                let glyph = font.glyph(ch)?;
                glyph.paint(&mut frame, pen, (HEIGHT - 12) / 2, clip, color);
                pen += glyph.advance;
            }
            self.spans.push(span);
            x = span.1;
        }
        Ok(frame)
    }
}
