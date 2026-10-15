//! Static world image planes and native simplified-view projection.
//!
//! Camera porting evidence (static inspection, never native execution):
//! Vm60 1003ffc0 receives OM_SetWorldCamera; 1004009b..100400b9
//! shift script roll/pitch/bank by four. 1003bd90 negates roll before
//! 100683f0 builds its signed Q12 matrix. Its radians coefficient at
//! 100904d0 is 2*pi/65536, hence 4096 script units per revolution.
//! 10046c70's mode-0 branch (10046cb2) takes start+delta, ignoring
//! interpolation progress and the low-four-bit easing power/direction flags.
//! Those flags only enter the mode-4/8/12 branch at 10046d3a; style 1
//! does not change mode-0 timing. Do not infer linear motion from duration alone.
//! For that remaining branch, low bits 0..2 select strength n (zero means
//! two), applying n-1 successive squarings; bit 3 selects acceleration
//! instead of mirrored decay. Each squaring is ((q >> 2) * q) >> 14, not a float
//! power or a single full-precision Q16 multiplication. Start/delta slots
//! are +844/+868 relative to the world state. 10045031..10045078 passes
//! the per-window progress to 10046c70 before updating the visible camera.
//! CParseBgBox resource loading does not restore save-state camera flags;
//! the constructor's default mode is zero. Vm60 1003c2e5 then uses an
//! axis-aligned projection with linear roll/pitch pan, *not* 100683f0's
//! trigonometric matrix. Full-matrix modes require separate implementation.
use crate::render::Texture;
use anyhow::{Context, Result, ensure};

/// Per-slot native 0x10000 flags. This suppresses a cached background plane
/// in the interleaved world-sprite pass, not in the plain background pass.
/// Vm60 10048500 handles OM_SetVarData IDs 128 (set) and 129 (clear);
/// 1003bb94/1003bc7b skip drawing the flagged slot between world sprites.
/// The fast no-sprite path at 1003bfe7 does not inspect this flag.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorldLayerFlags {
    suppressed_in_sprite_pass: u8,
}
impl WorldLayerFlags {
    /// Native layer zero is deliberately not configurable by this message.
    /// Invalid integer slots are ignored, including negative values.
    pub fn set_sprite_pass_suppressed(&mut self, slot: i32, suppressed: bool) {
        if !(1..8).contains(&slot) {
            return;
        }
        let bit = 1u8 << slot;
        if suppressed {
            self.suppressed_in_sprite_pass |= bit;
        } else {
            self.suppressed_in_sprite_pass &= !bit;
        }
    }
    pub fn draws_in_sprite_pass(&self, slot: u8) -> bool {
        slot < 8 && self.suppressed_in_sprite_pass & (1 << slot) == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Camera {
    pub focal_length: u16,
    /// Script roll/pitch/bank shifted left by four, as in the native state.
    /// Simplified projection uses roll/pitch only (bank has no effect).
    pub angles_q4: [i32; 3],
}
impl Default for Camera {
    fn default() -> Self {
        Self {
            focal_length: 720,
            angles_q4: [0; 3],
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Plane {
    origin: [i32; 2],
    size: [i32; 2],
    depth: i32,
    pitch_bias: i16,
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
            origin: [
                (size[0] * 0.5).floor() as i32,
                (size[1] * 0.5).floor() as i32,
            ],
            size: [size[0].ceil() as i32, size[1].ceil() as i32],
            depth: 720,
            pitch_bias: 0,
        })
    }
    pub fn background(bg: &rugp_assets::background::Background) -> Result<Self> {
        ensure!(
            bg.planes.len() == 1,
            "background requires extended/multiplane world geometry"
        );
        Self::background_scaled(bg, 0.0)
    }
    pub fn scene_background(scene: &rugp_assets::background::SceneBackground) -> Result<Self> {
        Self::background_scaled(&scene.background, scene.scale)
    }
    fn background_scaled(bg: &rugp_assets::background::Background, scale: f32) -> Result<Self> {
        let p = bg.placement();
        ensure!(
            p.mode == 0 && bg.flags == 0,
            "background requires extended/multiplane world geometry"
        );
        ensure!(scale.is_finite(), "invalid background extension scale");
        ensure!(
            p.scale_q16 > 0 && p.depth > 0,
            "invalid background scale/depth"
        );
        let scaled = |n: i64| -> Result<i32> {
            let product = n * i64::from(p.scale_q16);
            ensure!(
                i32::try_from(product).is_ok(),
                "background scale exceeds native multiplication range"
            );
            let value = (product >> 16) as i32;
            Ok(if scale > 0.0 {
                let scaled = (f64::from(value) * f64::from(scale)).trunc();
                ensure!(
                    (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&scaled),
                    "background extension scale exceeds native integer range"
                );
                scaled as i32
            } else {
                value
            })
        };
        let origin = [
            scaled(i64::from(p.origin[0]))?,
            scaled(i64::from(p.origin[1]))?,
        ];
        let size = [scaled(i64::from(bg.width))?, scaled(i64::from(bg.height))?];
        for axis in 0..2 {
            ensure!(
                size[axis] > 0
                    && i16::try_from(-i64::from(origin[axis])).is_ok()
                    && i16::try_from(i64::from(size[axis]) - i64::from(origin[axis])).is_ok(),
                "background exceeds native vertex range"
            );
        }
        ensure!(
            i16::try_from(p.depth).is_ok(),
            "background exceeds native depth range"
        );
        Ok(Self {
            origin,
            size,
            depth: p.depth,
            pitch_bias: p.pitch_bias,
        })
    }
    /// Vm60 1003e6fe..1003e800: anchor offsets are measured from half the
    /// unscaled background width and its unscaled Y origin. The native
    /// coefficient is 0.0016 (not Q16); global and optional per-layer float
    /// scales multiply it. Each edge truncates independently before i16 storage.
    pub fn background_layer(
        scene: &rugp_assets::background::SceneBackground,
        layer: &rugp_assets::background::BackgroundLayer,
        texture: &Texture,
    ) -> Result<Self> {
        let depth = layer.position[2];
        ensure!(
            depth > 0 && i16::try_from(depth).is_ok(),
            "invalid background layer depth"
        );
        ensure!(
            scene.scale.is_finite() && layer.scale.is_finite(),
            "invalid background layer scale"
        );
        let global_scale = if scene.scale > 0.0 {
            f64::from(scene.scale)
        } else {
            1.0
        };
        let mut factor = f64::from(depth) * global_scale * 0.0015999999999999999;
        if layer.scale > 0.0 {
            factor *= f64::from(layer.scale);
        }
        let base = [
            f64::from(layer.anchor[0]) - f64::from(scene.background.width) * 0.5,
            f64::from(layer.anchor[1]) - f64::from(scene.background.placement().origin[1]),
        ];
        let dimensions = [texture.width, texture.height];
        let mut origin = [0; 2];
        let mut size = [0; 2];
        for axis in 0..2 {
            let edge = |coordinate: f64| -> Result<i32> {
                let value = (coordinate * factor + f64::from(layer.position[axis])).trunc();
                ensure!(
                    (f64::from(i16::MIN)..=f64::from(i16::MAX)).contains(&value),
                    "background layer exceeds native vertex range"
                );
                Ok(value as i32)
            };
            let left = edge(base[axis])?;
            let right = edge(base[axis] + f64::from(dimensions[axis]))?;
            ensure!(right > left, "degenerate background layer");
            origin[axis] = -left;
            size[axis] = right - left;
        }
        Ok(Self {
            origin,
            size,
            depth,
            pitch_bias: scene.background.placement().pitch_bias,
        })
    }
    /// Default native world mode: pan terms are linear in the internal Q4
    /// angles. See 1003c388..1003c410: angle*depth*8 is subtracted from the
    /// Q16 vertex before focal/depth projection, not interpreted as degrees.
    pub fn quad(&self, camera: &Camera, viewport: (i32, i32, u32, u32)) -> Result<[[f64; 3]; 4]> {
        ensure!(
            camera.focal_length > 0 && self.depth > 0,
            "invalid camera/plane depth"
        );
        let center = [
            f64::from(viewport.0) + f64::from(viewport.2 / 2),
            f64::from(viewport.1) + f64::from(viewport.3 / 2),
        ];
        let angles = [
            camera.angles_q4[0],
            camera.angles_q4[1]
                .checked_add(i32::from(self.pitch_bias))
                .context("camera pitch overflow")?,
        ];
        let project = |coordinate: i32, angle: i32| -> Result<f64> {
            // Native KERNEL32.MulDiv rounds the signed Q16 projection to
            // nearest (ties away from zero), rather than truncating vertices
            // to whole pixels or using an unquantized floating-point ratio.
            let value =
                i64::from(coordinate) * 65536 - i64::from(angle) * i64::from(self.depth) * 8;
            ensure!(
                i32::try_from(value).is_ok(),
                "camera projection exceeds native numerator range"
            );
            let product = value * i64::from(camera.focal_length);
            let magnitude = (product.abs() + i64::from(self.depth) / 2) / i64::from(self.depth);
            let result = magnitude * product.signum();
            ensure!(
                i32::try_from(result).is_ok(),
                "camera projection exceeds native screen range"
            );
            Ok(result as f64 / 65536.0)
        };
        let left = center[0] + project(-self.origin[0], angles[0])?;
        let top = center[1] + project(-self.origin[1], angles[1])?;
        let right = center[0] + project(self.size[0] - self.origin[0], angles[0])?;
        let bottom = center[1] + project(self.size[1] - self.origin[1], angles[1])?;
        ensure!(
            [left, top, right, bottom]
                .iter()
                .all(|v| v.is_finite() && v.abs() <= i32::MAX as f64),
            "projected image outside coordinate range"
        );
        Ok([
            [left, top, f64::from(self.depth)],
            [right, top, f64::from(self.depth)],
            [right, bottom, f64::from(self.depth)],
            [left, bottom, f64::from(self.depth)],
        ])
    }
    pub fn rect(
        &self,
        camera: &Camera,
        viewport: (i32, i32, u32, u32),
    ) -> Result<(i32, i32, u32, u32)> {
        ensure!(
            camera.focal_length > 0 && self.depth > 0,
            "invalid camera/plane depth"
        );
        let quad = self.quad(camera, viewport)?;
        let [left, top, _] = quad[0];
        let width = quad[1][0] - left;
        let height = quad[3][1] - top;
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
    fn layer_edges_truncate_independently_and_ignore_texture_display_scale() -> Result<()> {
        use rugp_assets::background::{Background, BackgroundLayer, SceneBackground};
        let mut metadata = [0; 20];
        metadata[2..6].copy_from_slice(&65536i32.to_le_bytes());
        metadata[8..12].copy_from_slice(&30i32.to_le_bytes());
        metadata[12..16].copy_from_slice(&20i32.to_le_bytes());
        metadata[16..20].copy_from_slice(&1000i32.to_le_bytes());
        let mut scene = SceneBackground {
            background: Background {
                width: 100,
                height: 80,
                display_metadata: metadata,
                flags: 0,
                planes: vec![],
                properties: None,
                trailing_data: vec![],
            },
            color: 0,
            screen: [0; 2],
            range: [0; 2],
            scale: 0.6,
            layers: vec![],
        };
        let mut layer = BackgroundLayer {
            anchor: [10, 5],
            position: [3, -7, 1000],
            extra: [0; 3],
            scale: 0.0,
            image: rugp_assets::object::ObjectReader::new(&[0; 3]).reference(0)?,
            name: String::new(),
            id: 0,
            attributes: [0; 2],
            parameters: [0; 3],
            flags: 0,
        };
        let texture = Texture {
            width: 30,
            height: 20,
            display_scale: 999.0,
            rgba: vec![],
        };
        let plane = Plane::background_layer(&scene, &layer, &texture)?;
        assert_eq!(plane.origin, [35, 21]);
        assert_eq!(plane.size, [29, 19]);
        assert_eq!(plane.depth, 1000);
        layer.scale = 0.5;
        let plane = Plane::background_layer(&scene, &layer, &texture)?;
        assert_eq!(plane.origin, [16, 14]);
        assert_eq!(plane.size, [15, 10]);
        layer.position[2] = 0;
        assert!(Plane::background_layer(&scene, &layer, &texture).is_err());
        layer.position[2] = 32768;
        assert!(Plane::background_layer(&scene, &layer, &texture).is_err());
        layer.position[2] = 1000;
        layer.position[0] = i32::MAX;
        assert!(Plane::background_layer(&scene, &layer, &texture).is_err());
        layer.position[0] = 3;
        layer.scale = f32::NAN;
        assert!(Plane::background_layer(&scene, &layer, &texture).is_err());
        scene.scale = f32::MAX;
        assert!(Plane::scene_background(&scene).is_err());
        // Even a native minimum integer origin must error instead of
        // overflowing during vertex negation in debug or release builds.
        scene.scale = 65536.0;
        scene.background.display_metadata[8..12].copy_from_slice(&(-32768i32).to_le_bytes());
        assert!(Plane::scene_background(&scene).is_err());
        Ok(())
    }
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
            plane.rect(
                &Camera {
                    focal_length: 1440,
                    ..Camera::default()
                },
                viewport
            )?,
            viewport
        );
        texture.display_scale = 1.0;
        let plane = Plane::bitmap(&texture)?;
        assert_eq!(plane.rect(&Camera::default(), viewport)?, viewport);
        assert_eq!(
            plane.rect(
                &Camera {
                    focal_length: 1440,
                    ..Camera::default()
                },
                viewport
            )?,
            (-512, -300, 2048, 1200)
        );
        texture.display_scale = f32::NAN;
        assert!(Plane::bitmap(&texture).is_err());
        texture.display_scale = f32::INFINITY;
        assert!(Plane::bitmap(&texture).is_err());
        Ok(())
    }

    #[test]
    fn native_background_q16_origin_size_and_depth_are_independent() -> Result<()> {
        use rugp_assets::{
            background::{Background, BackgroundPlane},
            object::ObjectReader,
        };
        let mut metadata = Vec::new();
        metadata.extend(0u16.to_le_bytes());
        metadata.extend(98304i32.to_le_bytes()); // 1.5 in Q16
        metadata.extend(0i16.to_le_bytes());
        metadata.extend(3i32.to_le_bytes());
        metadata.extend(2i32.to_le_bytes());
        metadata.extend(90i32.to_le_bytes());
        let mut bg = Background {
            width: 8,
            height: 6,
            display_metadata: metadata.try_into().unwrap(),
            flags: 0,
            planes: vec![BackgroundPlane {
                metadata: [0; 12],
                image: ObjectReader::new(&[0; 3]).reference(0)?,
            }],
            properties: None,
            trailing_data: vec![],
        };
        let camera = Camera {
            focal_length: 90,
            ..Camera::default()
        };
        assert_eq!(bg.placement().origin, [3, 2]);
        assert_eq!(
            Plane::background(&bg)?.quad(&camera, (0, 0, 20, 10))?,
            [
                [6.0, 2.0, 90.0],
                [18.0, 2.0, 90.0],
                [18.0, 11.0, 90.0],
                [6.0, 11.0, 90.0]
            ]
        );
        bg.display_metadata[6..8].copy_from_slice(&16i16.to_le_bytes());
        assert_eq!(bg.placement().pitch_bias, 16);
        assert_eq!(
            Plane::background(&bg)?.quad(&camera, (0, 0, 20, 10))?[0][1],
            1.82421875
        );
        bg.display_metadata[8..12].copy_from_slice(&(-1i32).to_le_bytes());
        assert_eq!(Plane::background(&bg)?.origin, [-2, 3]); // arithmetic Q16 shift
        bg.display_metadata[2..6].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(Plane::background(&bg).is_err());
        bg.display_metadata[2..6].copy_from_slice(&0i32.to_le_bytes());
        assert!(Plane::background(&bg).is_err());
        Ok(())
    }

    #[test]
    fn simple_view_uses_linear_pan_not_degrees_or_a_rotation_matrix() -> Result<()> {
        let texture = Texture {
            width: 1024,
            height: 600,
            display_scale: 1.0,
            rgba: vec![],
        };
        let plane = Plane::bitmap(&texture)?;
        let camera = Camera {
            angles_q4: [16, -32, 1000],
            ..Camera::default()
        };
        let q = plane.quad(&camera, (0, 0, 1024, 600))?;
        assert_eq!(
            q,
            [
                [-1.40625, 2.8125, 720.0],
                [1022.59375, 2.8125, 720.0],
                [1022.59375, 602.8125, 720.0],
                [-1.40625, 602.8125, 720.0]
            ]
        );
        let other_bank = Camera {
            angles_q4: [16, -32, 0],
            ..camera
        };
        assert_eq!(plane.quad(&other_bank, (0, 0, 1024, 600))?, q);
        Ok(())
    }

    #[test]
    fn projection_keeps_native_q16_rounding_and_integer_screen_center() -> Result<()> {
        let plane = Plane {
            origin: [1, 1],
            size: [4, 4],
            depth: 3,
            pitch_bias: 0,
        };
        let camera = Camera {
            focal_length: 1,
            ..Camera::default()
        };
        let q = plane.quad(&camera, (10, 20, 9, 7))?;
        assert_eq!(
            q[0],
            [14.0 - 21845.0 / 65536.0, 23.0 - 21845.0 / 65536.0, 3.0]
        );
        assert_eq!(q[2], [15.0, 24.0, 3.0]);
        assert!(
            plane
                .quad(
                    &Camera {
                        angles_q4: [i32::MAX, 0, 0],
                        ..camera
                    },
                    (0, 0, 9, 7)
                )
                .is_err()
        );
        assert!(
            plane
                .quad(
                    &Camera {
                        focal_length: 0,
                        ..camera
                    },
                    (0, 0, 9, 7)
                )
                .is_err()
        );
        Ok(())
    }
}
