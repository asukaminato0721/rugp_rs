//! CPostureMoment runtime channels, Vm60 1001dca0/1001e360/1001e5d0.
//! Starting a moment clears its sampled history and elapsed time, irrespective
//! of the values serialized by the authoring tool. It does not reset the camera.
use crate::{
    motion::{Motion, ftol},
    world::Camera,
};
use anyhow::{Result, ensure};
use rugp_assets::posture::PostureFields;

#[derive(Debug, Clone)]
pub struct PostureMotion {
    channels: [Option<Motion>; 3],
    scales: [f32; 3],
    modes: [u16; 3],
    previous: [[i32; 3]; 3],
    elapsed: i32,
}
impl PostureMotion {
    pub fn new(fields: &PostureFields, channels: [Option<Motion>; 3]) -> Result<Self> {
        ensure!(
            fields.options == 0,
            "prepared-state posture option {} is not implemented yet",
            fields.options
        );
        ensure!(
            fields.scales.iter().all(|f| f.is_finite()),
            "non-finite posture scale"
        );
        Ok(Self {
            channels,
            scales: fields.scales,
            modes: [fields.position_mode, fields.angle_mode, fields.other_mode],
            previous: [[0; 3]; 3],
            elapsed: 0,
        })
    }
    /// 1001e070 returns the first non-null channel's duration, not the maximum.
    pub fn instantaneous(&self) -> bool {
        self.channels
            .iter()
            .flatten()
            .next()
            .is_none_or(|m| m.duration_ms() == 0)
    }
    pub fn elapsed_ms(&self) -> i32 {
        self.elapsed
    }
    pub fn active(&self) -> bool {
        self.channels
            .iter()
            .flatten()
            .next()
            .is_some_and(Motion::loops)
            || self
                .channels
                .iter()
                .flatten()
                .any(|m| !m.expired(self.elapsed))
    }
    /// Timed sampling uses existing camera values and native wrapping deltas.
    /// The caller applies world bounds *after* all channels have been sampled.
    pub fn advance(&mut self, camera: Camera, dt_ms: u32) -> Camera {
        self.elapsed = self.elapsed.wrapping_add(dt_ms as i32);
        self.apply(camera, false)
    }
    pub fn apply_end(&mut self, camera: Camera) -> Camera {
        self.apply(camera, true)
    }
    fn apply(&mut self, mut camera: Camera, end: bool) -> Camera {
        let mut other = [0, 0, i32::from(camera.focal_length as i16)];
        for channel in 0..3 {
            let Some(motion) = &self.channels[channel] else {
                continue;
            };
            let sample = if end {
                motion.end()
            } else {
                motion.sample(self.elapsed)
            };
            let sample = sample.map(|v| ftol(f64::from(v) * f64::from(self.scales[channel])));
            let output = match channel {
                0 => &mut camera.position,
                1 => &mut camera.angles_q4,
                _ => &mut other,
            };
            let mode = self.modes[channel] & if channel == 0 { 1 } else { 3 };
            let delta = if channel == 0 { mode == 0 } else { mode == 1 };
            if delta {
                for axis in 0..3 {
                    output[axis] = output[axis]
                        .wrapping_add(sample[axis].wrapping_sub(self.previous[channel][axis]));
                }
                self.previous[channel] = sample;
            } else if channel == 0 || mode == 0 {
                *output = sample;
            } // angular/other modes 2 and 3 are native no-ops.
        }
        // Background consumer 10041c55 uses only positive Other.Z and stores
        // its low word. Other.X/Y are not screen translations in this handler.
        if other[2] > 0 {
            camera.focal_length = other[2] as u16;
        }
        camera
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugp_assets::motion::{MotionCurve, MotionSegment};
    fn motion(end: [i32; 3], duration: i32) -> Motion {
        Motion::new(MotionCurve {
            start: [0; 3],
            segments: vec![MotionSegment {
                kind: 2,
                options: 1,
                duration_ms: 100,
                end,
                controls: vec![],
                time_curve: None,
            }],
            timing_mode: 0,
            duration_ms: duration,
            rate: 1000.0,
            dimensions: 1,
            flags: 1,
        })
        .unwrap()
    }
    fn fields() -> PostureFields {
        PostureFields {
            state: 0,
            version: 2,
            options: 0,
            base_point: [123; 3],
            elapsed_ms: 987,
            position: [456; 3],
            angles: [789; 3],
            other: [999; 3],
            position_mode: 0,
            angle_mode: 1,
            other_mode: 1,
            scales: [0.1, 1.0, 1.0],
        }
    }
    #[test]
    fn start_resets_serialized_history_and_repeated_samples_do_not_accumulate() -> Result<()> {
        let mut posture = PostureMotion::new(
            &fields(),
            [
                Some(motion([100, -200, 50], 100)),
                Some(motion([200, -400, 0], 100)),
                Some(motion([999, 888, 100], 100)),
            ],
        )?;
        let mut camera = Camera {
            position: [1000, 2000, 3000],
            angles_q4: [1600, -3200, 0],
            focal_length: 720,
        };
        camera = posture.advance(camera, 50);
        assert_eq!(posture.elapsed_ms(), 50);
        assert_eq!(camera.position, [1005, 1990, 3002]);
        assert_eq!(camera.angles_q4, [1700, -3400, 0]);
        assert_eq!(camera.focal_length, 770);
        assert_eq!(posture.advance(camera, 0), camera);
        camera = posture.advance(camera, 50);
        assert_eq!(camera.position, [1010, 1980, 3005]);
        assert_eq!(camera.angles_q4, [1800, -3600, 0]);
        assert_eq!(camera.focal_length, 820);
        assert!(posture.active()); // inclusive endpoint
        assert_eq!(posture.advance(camera, 1), camera);
        assert!(!posture.active());
        Ok(())
    }
    #[test]
    fn absolute_and_noop_channels_and_instant_endpoint() -> Result<()> {
        let mut f = fields();
        f.position_mode = 1;
        f.angle_mode = 0;
        f.other_mode = 0;
        f.scales = [1.0; 3];
        let mut posture = PostureMotion::new(
            &f,
            [
                Some(motion([1, 2, 3], 0)),
                Some(motion([4, 5, 6], 100)),
                Some(motion([7, 8, -9], 100)),
            ],
        )?;
        assert!(posture.instantaneous());
        let camera = posture.apply_end(Camera::default());
        assert_eq!(camera.position, [1, 2, 3]);
        assert_eq!(camera.angles_q4, [4, 5, 6]);
        assert_eq!(camera.focal_length, 720); // non-positive zoom is ignored
        for mode in [2, 3] {
            f.angle_mode = mode;
            f.other_mode = mode;
            let mut posture = PostureMotion::new(
                &f,
                [
                    None,
                    Some(motion([100; 3], 100)),
                    Some(motion([1000; 3], 100)),
                ],
            )?;
            assert_eq!(posture.advance(camera, 50), camera);
        }
        f.options = 1;
        assert!(PostureMotion::new(&f, [None, None, None]).is_err());
        Ok(())
    }
    #[test]
    fn delta_wraps_and_external_camera_changes_are_preserved() -> Result<()> {
        let mut f = fields();
        f.scales = [1.0; 3];
        let mut posture = PostureMotion::new(&f, [Some(motion([100; 3], 100)), None, None])?;
        let mut camera = Camera {
            position: [i32::MAX; 3],
            ..Camera::default()
        };
        camera = posture.advance(camera, 50);
        assert_eq!(camera.position, [i32::MIN + 49; 3]);
        camera.position = [1000; 3]; // another camera command between samples
        camera = posture.advance(camera, 50);
        assert_eq!(camera.position, [1050; 3]);
        Ok(())
    }
}
