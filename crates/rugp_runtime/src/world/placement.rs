//! Plain-image placement, distinct from immutable bitmap vertices.
use super::*;

impl Plane {
    /// Vm60 1003ea3c..1003ea9a defaults a zero plain-image Z to 720
    /// (style 0x200). Vertices stay centered at Z=0; 1003c338..1003c386
    /// computes vertex - camera + global image XYZ with wrapping i32 math.
    /// Our existing bitmap plane includes that default depth, so only Z's
    /// difference from 720 is retained as its global offset.
    pub fn bitmap_at(texture: &Texture, position: [i32; 3]) -> Result<Self> {
        let mut plane = Self::bitmap(texture)?;
        plane.offset = [
            position[0],
            position[1],
            if position[2] == 0 {
                0
            } else {
                position[2].wrapping_sub(720)
            },
        ];
        Ok(plane)
    }

    pub(super) fn relative_position(&self, camera: &Camera) -> [i32; 3] {
        if self.camera_relative {
            [0; 3]
        } else {
            std::array::from_fn(|axis| camera.position[axis].wrapping_sub(self.offset[axis]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texture() -> Texture {
        Texture {
            width: 8,
            height: 4,
            display_scale: 1.0,
            rgba: vec![255; 8 * 4 * 4],
        }
    }

    #[test]
    fn absolute_bitmap_xyz_is_not_baked_into_bounded_vertices_or_cached_pixels() -> Result<()> {
        let source = texture();
        let baseline = Plane::bitmap(&source)?;
        let viewport = (0, 0, 20, 20);
        let camera = Camera::default();
        for (position, expected) in [
            (
                [0, 0, 0],
                [
                    [6.0, 8.0, 720.0],
                    [14.0, 8.0, 720.0],
                    [14.0, 12.0, 720.0],
                    [6.0, 12.0, 720.0],
                ],
            ),
            (
                [3, -2, 720],
                [
                    [9.0, 6.0, 720.0],
                    [17.0, 6.0, 720.0],
                    [17.0, 10.0, 720.0],
                    [9.0, 10.0, 720.0],
                ],
            ),
            (
                [0, 0, 1440],
                [
                    [8.0, 9.0, 1440.0],
                    [12.0, 9.0, 1440.0],
                    [12.0, 11.0, 1440.0],
                    [8.0, 11.0, 1440.0],
                ],
            ),
        ] {
            let placed = Plane::bitmap_at(&source, position)?;
            placed.validate_snapshot()?;
            assert_eq!(placed.origin, baseline.origin);
            assert_eq!(placed.size, baseline.size);
            assert_eq!(placed.quad(&camera, viewport)?, expected);
            let saved: Plane = serde_json::from_slice(&serde_json::to_vec(&placed)?)?;
            assert_eq!(saved.quad(&camera, viewport)?, expected);
        }
        assert_eq!(source.rgba, vec![255; 8 * 4 * 4]);
        let mut legacy = serde_json::to_value(baseline)?;
        legacy.as_object_mut().unwrap().remove("offset");
        let restored: Plane = serde_json::from_value(legacy)?;
        assert_eq!(
            restored.quad(&camera, viewport)?,
            baseline.quad(&camera, viewport)?
        );
        Ok(())
    }

    #[test]
    fn offsets_keep_native_wrapping_clipping_bounds_and_camera_relative_slots() -> Result<()> {
        let source = texture();
        let viewport = (0, 0, 20, 20);
        let camera = Camera {
            position: [5, -3, 1],
            angles_q4: [16, -32, 48],
            ..Camera::default()
        };
        for position in [[3, -7, 1440], [i32::MAX, i32::MIN, -720]] {
            let placed = Plane::bitmap_at(&source, position)?;
            placed.validate_snapshot()?;
            let baseline = Plane::bitmap(&source)?;
            let local = Camera {
                position: placed.relative_position(&camera),
                ..camera
            };
            assert_eq!(
                placed.quad(&camera, viewport)?,
                baseline.quad(&local, viewport)?
            );
            assert_eq!(
                placed.projected_camera_center_y(&camera, viewport)?,
                baseline.projected_camera_center_y(&local, viewport)?
            );
            let corrected = placed.clamp_camera(camera, viewport, source.width, 10)?;
            let expected = baseline.clamp_camera(local, viewport, source.width, 10)?;
            assert_eq!(corrected.position, camera.position);
            assert_eq!(corrected.angles_q4, expected.angles_q4);
        }
        let mut relative = Plane::bitmap_at(&source, [10, -20, 1440])?;
        relative.camera_relative = true;
        assert_eq!(
            relative.quad(&camera, viewport)?,
            Plane::bitmap(&source)?.quad(
                &Camera {
                    position: [0; 3],
                    ..camera
                },
                viewport
            )?
        );
        assert!(
            Plane::bitmap_at(&source, [0, 0, -720])?
                .quad(&Camera::default(), viewport)?
                .iter()
                .all(|v| v[2] < 0.0)
        );
        Ok(())
    }
}
