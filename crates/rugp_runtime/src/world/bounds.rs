//! Default angular boundary correction, Vm60 100406d9..10040b94.
//!
//! This is deliberately not a projected-rectangle coverage test. Native
//! correction uses Q13 edges, the first image's pixel width, two successive
//! clamps, quantized sine corrections and a cached projected Y center.
use super::{Camera, Plane};
use anyhow::{Result, ensure};

const RADIANS: f64 = 9.587379924285279e-05; // native 100904d0

// MSVCRT _ftol (78001bf1) truncates to signed i64 with FISTP and returns
// EDX:EAX; this caller only uses EAX. Invalid/overflow produces i64::MIN,
// whose low word is zero, not Rust's saturating float-to-i32 cast.
fn ftol(value: f64) -> i32 {
    if (-9223372036854775808.0..9223372036854775808.0).contains(&value) {
        value as i64 as i32
    } else {
        0
    }
}
fn ordered_clamp(value: i32, low: i32, high: i32) -> i32 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}
pub(crate) fn mul_div(value: i32, multiplier: i32, divisor: i32) -> i32 {
    if divisor == 0 {
        return -1;
    }
    let product = i64::from(value) * i64::from(multiplier);
    let magnitude = (product.abs() + i64::from(divisor).abs() / 2) / i64::from(divisor).abs();
    i32::try_from(magnitude * product.signum() * i64::from(divisor).signum()).unwrap_or(-1)
}

impl Plane {
    /// Default projection flags, including the native global image XYZ offset.
    /// `cached_center_y` is slot +56, *not* this candidate camera's center.
    pub fn clamp_camera(
        &self,
        mut camera: Camera,
        viewport: (i32, i32, u32, u32),
        first_image_width: u32,
        cached_center_y: i16,
    ) -> Result<Camera> {
        ensure!(
            camera.focal_length <= 32767,
            "invalid signed camera focal length"
        );
        let position = self.relative_position(&camera);
        let depth = self.depth.wrapping_sub(position[2]);
        // Native 1004071b..100407ac uses signed MulDiv/fdiv without a
        // positive-depth guard. At zero, masked x87 invalids reach _ftol's
        // zero low word; the helper below preserves that conversion.
        let left = self.origin[0].wrapping_neg().wrapping_sub(position[0]);
        let top = self.origin[1].wrapping_neg().wrapping_sub(position[1]);
        let right = left.wrapping_add(self.size[0]);
        let bottom = top.wrapping_add(self.size[1]);
        // The window and bitmap widths are loaded with MOVSX from i16.
        let width = i32::from(viewport.2 as i16);
        let height = i32::from(viewport.3 as i16);
        let center_x = viewport.0.wrapping_add(width / 2) as i16;
        let center_y = viewport.1.wrapping_add(height / 2) as i16;
        let focal = i32::from(camera.focal_length);
        let pixel_width = i32::from(first_image_width as i16);
        let ratio = f64::from(pixel_width) / f64::from(mul_div(self.size[0], focal, depth));
        let inverse_scale = 1.0 / (ratio * 65536.0);
        let inverse_depth = 1.0 / f64::from(depth);
        let inverse_focal = 1.0 / f64::from(focal);
        let q13 = |v: i32| f64::from(v.wrapping_shl(13));
        let l = q13(left) * inverse_depth;
        let r = q13(right) * inverse_depth;
        let t = q13(top) * inverse_depth;
        let b = q13(bottom) * inverse_depth;
        let sx = q13(i32::from(center_x));
        let sy = q13(i32::from(center_y));
        let sx_right = q13(i32::from(center_x).wrapping_sub(width));
        let sy_bottom = q13(i32::from(center_y).wrapping_sub(height));
        let roll_low = (sx * inverse_focal + l) * 0.86;
        let roll_high = (sx_right * inverse_focal + r) * 0.86;
        let pitch_low = (sy * inverse_focal + t) * 0.9;
        let pitch_high = (sy_bottom * inverse_focal + b) * 0.9;
        let roll = ordered_clamp(camera.angles_q4[0], ftol(roll_low), ftol(roll_high));
        let pitch = ordered_clamp(camera.angles_q4[1], ftol(pitch_low), ftol(pitch_high));
        let half_width = width / 2;
        let half_height = height / 2;
        let half_width_squared = f64::from(half_width.wrapping_mul(half_width) / 2);
        let half_height_squared = f64::from(half_height.wrapping_mul(half_height) / 2);
        let sine =
            |angle: f64, coefficient: f64| ftol((angle * RADIANS).sin() * ratio * coefficient);
        let correction_x = f64::from(
            sine(f64::from(pitch), 573440.0)
                .wrapping_abs()
                .wrapping_mul(half_height),
        ) * f64::from(half_width)
            * inverse_scale;
        let low_roll_correction =
            f64::from(sine(roll_low, 983040.0)) * half_width_squared * inverse_scale;
        let final_roll_low =
            ftol((sx - low_roll_correction + correction_x) * inverse_focal + l).max(-16384);
        // The native symmetric shortcuts reuse the lower sine correction,
        // avoiding a second conversion (which can differ at integer edges).
        let high_roll_correction = if right == left.wrapping_neg() {
            -low_roll_correction
        } else {
            f64::from(sine(roll_high, 983040.0)) * half_width_squared * inverse_scale
        };
        let final_roll_high =
            ftol((sx_right - high_roll_correction - correction_x) * inverse_focal + r);
        let correction_y = ftol(
            f64::from(
                sine(f64::from(roll), -683.0)
                    .wrapping_mul(half_width)
                    .wrapping_shl(13)
                    .wrapping_abs(),
            ) * inverse_scale,
        );
        // 600 is a literal native reference height, not viewport height.
        let bottom_inset = f64::from(
            600i32
                .wrapping_sub(i32::from(cached_center_y))
                .wrapping_abs()
                .wrapping_mul(correction_y)
                >> 3,
        );
        let top_inset = f64::from(
            i32::from(cached_center_y)
                .wrapping_abs()
                .wrapping_mul(correction_y)
                >> 3,
        );
        let low_pitch_correction =
            f64::from(sine(pitch_low, 983040.0)) * half_height_squared * inverse_scale;
        let high_pitch_correction = if top == bottom.wrapping_neg() {
            -low_pitch_correction
        } else {
            f64::from(sine(pitch_high, 983040.0)) * half_height_squared * inverse_scale
        };
        let final_pitch_low = ftol((sy - low_pitch_correction + top_inset) * inverse_focal + t);
        let final_pitch_high =
            ftol((sy_bottom - high_pitch_correction - bottom_inset) * inverse_focal + b);
        camera.angles_q4[0] = ordered_clamp(roll, final_roll_low, final_roll_high);
        camera.angles_q4[1] = ordered_clamp(pitch, final_pitch_low, final_pitch_high);
        Ok(camera)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_integer_helpers_keep_low_word_invalids_ties_and_ordered_empty_ranges() {
        for (value, expected) in [
            (1.9, 1),
            (-1.9, -1),
            (4294967313.0, 17),
            (-4294967297.0, -1),
            (f64::NAN, 0),
            (f64::INFINITY, 0),
            (f64::NEG_INFINITY, 0),
            (9223372036854775808.0, 0),
        ] {
            assert_eq!(ftol(value), expected);
        }
        for (a, b, c, out) in [
            (3, 1, 2, 2),
            (-3, 1, 2, -2),
            (3, 1, -2, -2),
            (-3, -1, -2, -2),
            (i32::MAX, 2, 1, -1),
            (0, 0, 0, -1),
        ] {
            assert_eq!(mul_div(a, b, c), out);
        }
        assert_eq!(ordered_clamp(0, 10, -10), 10);
        assert_eq!(ordered_clamp(20, 10, -10), -10);
    }
    fn chapter_bounds() -> Plane {
        Plane {
            origin: [6720, 2208],
            size: [13440, 4416],
            depth: 5000,
            pitch_bias: 0,
            camera_relative: false,
            offset: [0; 3],
        }
    }
    #[test]
    fn two_stage_correction_uses_image_width_and_previous_projection_center() -> Result<()> {
        // Metadata-only vectors, independently evaluated from the native
        // Q13/sine expression. No game pixels, names or dialogue.
        let bounds = chapter_bounds();
        for (position, angles, previous, expected) in [
            ([0; 3], [-5840, -1200, 9], 598, [-5840, -1200, 9]),
            ([0; 3], [50000, 50000, 9], 598, [7700, 2171, 9]),
            ([0; 3], [-50000, -50000, 9], 598, [-7700, -1269, 9]),
            ([575, -1400, -8], [-5840, -1200, 9], 0, [-5840, -105, 9]),
            ([575, -1400, -8], [-5840, -1200, 9], 100, [-5840, 28, 9]),
            ([575, -1400, -8], [-5840, -1200, 9], 300, [-5840, 311, 9]),
            ([575, -1400, -8], [-5840, -1200, 9], 598, [-5840, 734, 9]),
            ([575, -1400, -8], [-5840, -1200, 9], 1000, [-5840, 1304, 9]),
            ([575, -1400, -8], [-5840, -1200, 9], -500, [-5840, 595, 9]),
            ([575, -1400, -8], [-50000, -50000, 9], 598, [-8494, 1051, 9]),
            (
                [-575, 1400, 80],
                [-50000, -50000, 9],
                598,
                [-7031, -3670, 9],
            ),
        ] {
            let camera = Camera {
                focal_length: 2040,
                position,
                angles_q4: angles,
            };
            let prepared = bounds.clamp_camera(camera, (0, 0, 1024, 600), 2800, previous)?;
            assert_eq!(
                prepared.angles_q4, expected,
                "{position:?}/{angles:?}/{previous}"
            );
            assert_eq!(prepared.position, position);
            assert_eq!(prepared.focal_length, camera.focal_length);
        }
        let camera = Camera {
            focal_length: 2040,
            position: [575, -1400, -8],
            angles_q4: [-5840, -1200, 9],
        };
        assert_ne!(
            bounds.clamp_camera(camera, (0, 0, 1024, 600), 2800, 598)?,
            bounds.clamp_camera(camera, (0, 0, 1024, 600), 1024, 598)?
        );
        let zero = bounds.clamp_camera(
            Camera {
                focal_length: 0,
                ..camera
            },
            (0, 0, 1024, 600),
            2800,
            598,
        )?;
        assert_eq!(zero.angles_q4, [0, 0, 9]);
        for z in [5000, 5001, 10000] {
            let candidate = Camera {
                position: [0, 0, z],
                ..camera
            };
            let adjusted = bounds.clamp_camera(candidate, (0, 0, 1024, 600), 2800, 598)?;
            assert_eq!(adjusted.position, candidate.position);
            assert_eq!(adjusted.focal_length, candidate.focal_length);
            if z == 5000 {
                assert_eq!(adjusted.angles_q4, [0, 0, 9]);
            }
        }
        Ok(())
    }
    #[test]
    fn centered_shortcuts_and_native_reference_height_are_not_generic_viewport_clamping()
    -> Result<()> {
        let bounds = Plane {
            origin: [500, 400],
            size: [1000, 800],
            depth: 720,
            pitch_bias: 0,
            camera_relative: false,
            offset: [0; 3],
        };
        let camera = Camera {
            angles_q4: [50000, 50000, 0],
            ..Camera::default()
        };
        assert_eq!(
            bounds
                .clamp_camera(camera, (0, 0, 800, 600), 1000, 300)?
                .angles_q4,
            [838, 881, 0]
        );
        Ok(())
    }
}
