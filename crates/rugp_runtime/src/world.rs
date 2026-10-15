//! Axis-aligned world image planes. Native zoom is a focal length, not a FOV.
//!
//! Camera porting evidence (static inspection, never native execution):
//! Vm60 1003ffc0 receives OM_SetWorldCamera; 1004009b..100400b9
//! shift script roll/pitch/bank by four. 1003bd90 negates roll before
//! 100683f0 builds its signed Q12 matrix. Its radians coefficient at
//! 100904d0 is 2*pi/65536, hence 4096 script units per revolution.
//! 10046c70's mode-0 branch (10046cb2) takes start+delta, ignoring
//! interpolation progress. Do not infer linear motion from duration alone.
//! CBg2d has its own placement metadata; treating its inline bitmap as an
//! ordinary rotated image is not a faithful implementation of that resource.
use crate::render::Texture;
use anyhow::{Result, ensure};

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub focal_length: u16,
}
impl Default for Camera {
    fn default() -> Self {
        Self { focal_length: 720 }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Plane {
    origin: [f64; 2],
    size: [f64; 2],
    depth: f64,
}
impl Plane {
    pub fn bitmap(texture: &Texture) -> Result<Self> {
        let scale = f64::from(texture.display_scale);
        ensure!(
            scale.is_finite() && scale > 0.0,
            "invalid world bitmap scale"
        );
        // Vm60's plain-image placement floors the scaled origin and ceils its
        // dimensions independently. Style 0x200 defaults zero depth to 720.
        let size = [
            f64::from(texture.width) * scale,
            f64::from(texture.height) * scale,
        ];
        ensure!(
            size.iter().all(|n| *n <= 32767.0),
            "world bitmap exceeds native vertex range"
        );
        Ok(Self {
            origin: [(size[0] * 0.5).floor(), (size[1] * 0.5).floor()],
            size: [size[0].ceil(), size[1].ceil()],
            depth: 720.0,
        })
    }
    pub fn rect(
        &self,
        camera: &Camera,
        viewport: (i32, i32, u32, u32),
    ) -> Result<(i32, i32, u32, u32)> {
        ensure!(
            camera.focal_length > 0 && self.depth > 0.0,
            "invalid camera/plane depth"
        );
        let ratio = f64::from(camera.focal_length) / self.depth;
        let (x, y, w, h) = viewport;
        let left = f64::from(x) + f64::from(w) * 0.5 - self.origin[0] * ratio;
        let top = f64::from(y) + f64::from(h) * 0.5 - self.origin[1] * ratio;
        let width = self.size[0] * ratio;
        let height = self.size[1] * ratio;
        ensure!(
            [left, top, width, height]
                .iter()
                .all(|v| v.is_finite() && v.abs() <= i32::MAX as f64),
            "projected image outside coordinate range"
        );
        Ok((
            left.round() as i32,
            top.round() as i32,
            width.round() as u32,
            height.round() as u32,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bitmap_scale_and_focal_length_are_independent() -> Result<()> {
        let mut texture = Texture {
            width: 1024,
            height: 600,
            display_scale: 0.5,
            rgba: Vec::new(),
        };
        let plane = Plane::bitmap(&texture)?;
        let viewport = (0, 0, 1024, 600);
        assert_eq!(
            plane.rect(&Camera::default(), viewport)?,
            (256, 150, 512, 300)
        );
        assert_eq!(
            plane.rect(&Camera { focal_length: 1440 }, viewport)?,
            viewport
        );
        texture.display_scale = 1.0;
        let plane = Plane::bitmap(&texture)?;
        assert_eq!(plane.rect(&Camera::default(), viewport)?, viewport);
        assert_eq!(
            plane.rect(&Camera { focal_length: 1440 }, viewport)?,
            (-512, -300, 2048, 1200)
        );
        texture.display_scale = f32::NAN;
        assert!(Plane::bitmap(&texture).is_err());
        texture.display_scale = f32::INFINITY;
        assert!(Plane::bitmap(&texture).is_err());
        Ok(())
    }
}
