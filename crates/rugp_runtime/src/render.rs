//! Small, deterministic straight-alpha RGBA software compositor.
use anyhow::{Result, ensure};
use rugp_assets::image::Image;

pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub display_scale: f32,
    pub rgba: Vec<u8>,
}
impl Texture {
    pub fn from_image(image: Image) -> Result<Self> {
        ensure!(
            image.canvas_width > 0
                && image.canvas_height > 0
                && image.canvas_width as u64 * image.canvas_height as u64 <= 16_777_216,
            "invalid texture canvas"
        );
        ensure!(
            image.x as u32 + image.width <= image.canvas_width
                && image.y as u32 + image.height <= image.canvas_height,
            "texture crop outside canvas"
        );
        ensure!(
            image.rgba.len() == image.width as usize * image.height as usize * 4,
            "invalid texture pixel length"
        );
        if image.width == image.canvas_width
            && image.height == image.canvas_height
            && image.x == 0
            && image.y == 0
        {
            return Ok(Self {
                width: image.width,
                height: image.height,
                display_scale: image.display_scale,
                rgba: image.rgba,
            });
        }
        let mut rgba = vec![0; image.canvas_width as usize * image.canvas_height as usize * 4];
        for y in 0..image.height as usize {
            let src = y * image.width as usize * 4;
            let dst = ((y + image.y as usize) * image.canvas_width as usize + image.x as usize) * 4;
            rgba[dst..dst + image.width as usize * 4]
                .copy_from_slice(&image.rgba[src..src + image.width as usize * 4]);
        }
        Ok(Self {
            width: image.canvas_width,
            height: image.canvas_height,
            display_scale: image.display_scale,
            rgba,
        })
    }
}
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        ensure!(
            width > 0 && height > 0 && width as u64 * height as u64 <= 16_777_216,
            "invalid frame dimensions"
        );
        let mut frame = Self {
            width,
            height,
            rgba: vec![0; width as usize * height as usize * 4],
        };
        frame.clear([0, 0, 0]);
        Ok(frame)
    }
    pub fn clear(&mut self, color: [u8; 3]) {
        for p in self.rgba.as_chunks_mut::<4>().0 {
            p.copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
    pub fn blit(
        &mut self,
        texture: &Texture,
        rect: (i32, i32, u32, u32),
        opacity: u8,
    ) -> Result<()> {
        let (x, y, w, h) = rect;
        if w == 0 || h == 0 || opacity == 0 {
            return Ok(());
        }
        ensure!(
            texture.width > 0
                && texture.height > 0
                && texture.width as u64 * texture.height as u64 <= 16_777_216
                && texture.rgba.len() == texture.width as usize * texture.height as usize * 4,
            "invalid source texture"
        );
        let left = (x as i64).max(0);
        let top = (y as i64).max(0);
        let right = (x as i64 + w as i64).min(self.width as i64);
        let bottom = (y as i64 + h as i64).min(self.height as i64);
        for dy in top..bottom {
            let sy = (dy - y as i64) as u64 * texture.height as u64 / h as u64;
            for dx in left..right {
                let sx = (dx - x as i64) as u64 * texture.width as u64 / w as u64;
                let si = (sy as usize * texture.width as usize + sx as usize) * 4;
                let di = (dy as usize * self.width as usize + dx as usize) * 4;
                let alpha = (texture.rgba[si + 3] as u32 * opacity as u32 + 127) / 255;
                for c in 0..3 {
                    self.rgba[di + c] = ((texture.rgba[si + c] as u32 * alpha
                        + self.rgba[di + c] as u32 * (255 - alpha)
                        + 127)
                        / 255) as u8;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn crop_alpha_scaling_and_clipping() -> Result<()> {
        let texture = Texture::from_image(Image {
            canvas_width: 2,
            canvas_height: 1,
            width: 1,
            height: 1,
            x: 1,
            y: 0,
            display_scale: 1.0,
            rgba: vec![255, 0, 0, 128],
        })?;
        let mut frame = Framebuffer::new(4, 2)?;
        frame.clear([0, 0, 255]);
        frame.blit(&texture, (0, 0, 4, 2), 255)?;
        assert_eq!(
            &frame.rgba[..16],
            &[
                0, 0, 255, 255, 0, 0, 255, 255, 128, 0, 127, 255, 128, 0, 127, 255
            ]
        );
        frame.clear([0, 0, 255]);
        frame.blit(&texture, (-2, 0, 4, 2), 128)?;
        assert_eq!(&frame.rgba[..8], &[64, 0, 191, 255, 64, 0, 191, 255]);
        assert!(Framebuffer::new(0, 1).is_err());
        Ok(())
    }
}
