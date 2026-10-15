//! Native absolute camera interpolation (Vm60 10046d3a..1004704d).
use super::Camera;
use anyhow::{Context, Result, ensure};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraTween {
    start: Camera,
    delta_angles: [i32; 3],
    delta_focal: i16,
    style: u8,
}
impl CameraTween {
    pub fn new(start: Camera, target: Camera, style: u8) -> Result<Self> {
        ensure!(style <= 15, "unsupported camera interpolation style");
        ensure!(
            start.focal_length <= 32767 && target.focal_length <= 32767,
            "camera focal length exceeds native signed range"
        );
        let mut delta_angles = [0; 3];
        for (axis, delta) in delta_angles.iter_mut().enumerate() {
            *delta = target.angles_q4[axis]
                .checked_sub(start.angles_q4[axis])
                .context("camera angle delta overflow")?;
        }
        let delta_focal =
            i16::try_from(i32::from(target.focal_length) - i32::from(start.focal_length))?;
        Ok(Self {
            start,
            delta_angles,
            delta_focal,
            style,
        })
    }
    pub fn start(&self) -> Camera {
        self.start
    }
    pub fn target(&self) -> Camera {
        self.sample(65536).expect("validated camera target")
    }
    /// A new capture overwrites the native start slot without overwriting
    /// an already prepared delta. Validate the resulting target first.
    pub fn rebase_start(&self, start: Camera) -> Result<Self> {
        ensure!(
            start.focal_length <= 32767,
            "camera focal length exceeds native signed range"
        );
        let candidate = Self { start, ..*self };
        candidate.sample(65536)?;
        Ok(candidate)
    }
    pub fn sample(&self, progress_q16: i32) -> Result<Camera> {
        ensure!(
            (0..=65536).contains(&progress_q16),
            "invalid camera progress"
        );
        let weight = eased_progress(self.style, progress_q16);
        let mut angles_q4 = [0; 3];
        for (axis, out) in angles_q4.iter_mut().enumerate() {
            let value = i64::from(self.start.angles_q4[axis])
                + ((i64::from(self.delta_angles[axis]) * i64::from(weight)) >> 16);
            *out = i32::try_from(value).context("camera interpolated angle overflow")?;
        }
        let focal =
            i32::from(self.start.focal_length) + ((i32::from(self.delta_focal) * weight) >> 16);
        ensure!(
            (0..=32767).contains(&focal),
            "camera interpolated focal length outside native range"
        );
        Ok(Camera {
            focal_length: focal as u16,
            angles_q4,
        })
    }
}
fn eased_progress(style: u8, progress: i32) -> i32 {
    let strength = match style & 7 {
        0 => 2,
        n => n,
    };
    let accelerate = style & 8 != 0;
    let mut q = if accelerate {
        progress
    } else {
        65536 - progress
    };
    for _ in 1..strength {
        q = ((q >> 2) * q) >> 14;
    }
    if accelerate { q } else { 65536 - q }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_easing_is_repeated_quantized_squaring_not_a_float_power() {
        for (style, expected) in [
            (0, 28672),
            (1, 16384),
            (2, 28672),
            (3, 44800),
            (8, 4096),
            (9, 16384),
            (10, 4096),
            (11, 256),
        ] {
            assert_eq!(eased_progress(style, 16384), expected, "style {style}");
            assert_eq!(eased_progress(style, 0), 0);
            assert_eq!(eased_progress(style, 65536), 65536);
        }
        assert_eq!(eased_progress(10, 32769), 16384); // pre-shift loses low bits
        for style in 0..16 {
            let mut previous = 0;
            for time in 0..=65536 {
                let value = eased_progress(style, time);
                assert!((previous..=65536).contains(&value));
                previous = value;
            }
        }
    }
    #[test]
    fn interpolated_components_use_signed_floor_and_capture_preserves_delta() -> Result<()> {
        let start = Camera {
            focal_length: 1001,
            angles_q4: [1, -1, 9],
        };
        let target = Camera {
            focal_length: 1000,
            angles_q4: [-2, 2, -9],
        };
        let tween = CameraTween::new(start, target, 1)?;
        assert_eq!(tween.start(), start);
        assert_eq!(tween.target(), target);
        assert_eq!(tween.sample(0)?, start);
        assert_eq!(
            tween.sample(32768)?,
            Camera {
                focal_length: 1000,
                angles_q4: [-1, 0, 0]
            }
        );
        let rebased = tween.rebase_start(Camera {
            focal_length: 2,
            angles_q4: [0, 0, 0],
        })?;
        assert_eq!(
            rebased.target(),
            Camera {
                focal_length: 1,
                angles_q4: [-3, 3, -18]
            }
        );
        assert!(
            tween
                .rebase_start(Camera {
                    focal_length: 0,
                    ..start
                })
                .is_err()
        );
        assert!(tween.sample(-1).is_err());
        assert!(tween.sample(65537).is_err());
        assert!(CameraTween::new(start, target, 16).is_err());
        assert!(
            CameraTween::new(
                Camera {
                    angles_q4: [i32::MIN, 0, 0],
                    ..start
                },
                Camera {
                    angles_q4: [i32::MAX, 0, 0],
                    ..target
                },
                1
            )
            .is_err()
        );
        Ok(())
    }
}
