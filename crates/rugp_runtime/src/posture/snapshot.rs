//! Preserve sampled delta history and fractional clocks. Calling `new` at the
//! saved elapsed time is NOT equivalent: it resets accumulated channel history.
use super::*;
use rugp_assets::motion::MotionCurve;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostureSnapshot {
    version: u32,
    channels: [Option<MotionCurve>; 3],
    scales: [f32; 3],
    modes: [u16; 3],
    previous: [[i32; 3]; 3],
    elapsed: i32,
    time_scale: f64,
    time_fraction: f64,
}
impl PostureMotion {
    pub fn snapshot(&self) -> Result<PostureSnapshot> {
        let snapshot = PostureSnapshot {
            version: 1,
            channels: self
                .channels
                .each_ref()
                .map(|m| m.as_ref().map(|m| m.snapshot_curve().clone())),
            scales: self.scales,
            modes: self.modes,
            previous: self.previous,
            elapsed: self.elapsed,
            time_scale: self.time_scale,
            time_fraction: self.time_fraction,
        };
        snapshot.payload_bytes()?;
        Ok(snapshot)
    }
}
impl PostureSnapshot {
    /// Aggregate this across actors in the enclosing scene before restoring.
    pub fn payload_bytes(&self) -> Result<usize> {
        ensure!(self.version == 1, "unsupported posture snapshot version");
        ensure!(
            self.scales.iter().all(|f| f.is_finite())
                && self.time_scale.is_finite()
                && self.time_scale > 0.0
                && self.time_scale <= f64::from(f32::MAX)
                && (0.0..1.0).contains(&self.time_fraction),
            "invalid posture snapshot scale/fraction"
        );
        let mut bytes = 0usize;
        for curve in self.channels.iter().flatten() {
            ensure!(
                curve.rate.is_finite() && curve.segments.len() <= 65_536,
                "invalid posture snapshot motion"
            );
            let size =
                curve.segments.len() * std::mem::size_of::<rugp_assets::motion::MotionSegment>();
            bytes = bytes
                .checked_add(size)
                .ok_or_else(|| anyhow::anyhow!("posture snapshot size overflow"))?;
            for segment in &curve.segments {
                ensure!(
                    segment.controls.len() <= 32,
                    "posture snapshot control count exceeds limit"
                );
                bytes = bytes
                    .checked_add(segment.controls.len() * 12)
                    .ok_or_else(|| anyhow::anyhow!("posture snapshot size overflow"))?;
                if let Some(rugp_assets::motion::TimeCurve::Bezier(knots)) = &segment.time_curve {
                    ensure!(
                        knots.len() <= 65_536,
                        "posture snapshot knot count exceeds limit"
                    );
                    bytes = bytes
                        .checked_add(knots.len() * 24)
                        .ok_or_else(|| anyhow::anyhow!("posture snapshot size overflow"))?;
                }
            }
        }
        ensure!(
            bytes <= 64 * 1024 * 1024,
            "posture snapshot motion payload exceeds limit"
        );
        Ok(bytes)
    }
    pub fn restore(&self) -> Result<PostureMotion> {
        self.payload_bytes()?;
        let channels = self.channels.each_ref().map(|curve| {
            curve
                .as_ref()
                .map(|curve| Motion::new(curve.clone()))
                .transpose()
        });
        let [position, angles, other] = channels;
        Ok(PostureMotion {
            channels: [position?, angles?, other?],
            scales: self.scales,
            modes: self.modes,
            previous: self.previous,
            elapsed: self.elapsed,
            time_scale: self.time_scale,
            time_fraction: self.time_fraction,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugp_assets::motion::MotionSegment;
    #[test]
    fn fractional_time_and_delta_history_continue_without_resetting_or_reapplying_motion()
    -> Result<()> {
        let curve = MotionCurve {
            start: [0; 3],
            segments: vec![MotionSegment {
                kind: 2,
                options: 1,
                duration_ms: 100,
                end: [100, -200, 0],
                controls: vec![],
                time_curve: None,
            }],
            timing_mode: 0,
            duration_ms: 100,
            rate: 1000.0,
            dimensions: 1,
            flags: 1,
        };
        let fields = PostureFields {
            state: 0,
            version: 2,
            options: 0,
            base_point: [0; 3],
            elapsed_ms: 0,
            angles: [0; 3],
            position: [0; 3],
            other: [0; 3],
            position_mode: 0,
            angle_mode: 1,
            other_mode: 0,
            scales: [1.0; 3],
        };
        let mut original = PostureMotion::new(&fields, [Some(Motion::new(curve)?), None, None])?;
        original.configure_playback(10, 0.25, [0.0; 3])?;
        let mut camera = original.advance(
            Camera {
                position: [500, 1000, 0],
                ..Default::default()
            },
            5,
        );
        let snapshot = original.snapshot()?;
        let snapshot: PostureSnapshot = serde_json::from_slice(&serde_json::to_vec(&snapshot)?)?;
        let mut restored = snapshot.restore()?;
        let mut copy = camera;
        for dt in [0, 1, 2, 3, 37, 100, 400] {
            camera = original.advance(camera, dt);
            copy = restored.advance(copy, dt);
            assert_eq!(camera, copy);
            assert_eq!(original.elapsed_ms(), restored.elapsed_ms());
            assert_eq!(original.active(), restored.active());
        }
        Ok(())
    }
    #[test]
    fn malformed_fraction_scales_versions_and_motion_are_rejected() -> Result<()> {
        let source = PostureMotion {
            channels: [None, None, None],
            scales: [1.0; 3],
            modes: [0; 3],
            previous: [[0; 3]; 3],
            elapsed: -10,
            time_scale: 1.0,
            time_fraction: 0.5,
        };
        let mut bad = source.snapshot()?;
        bad.time_fraction = 1.0;
        assert!(bad.restore().is_err());
        let mut bad = source.snapshot()?;
        bad.time_scale = f64::NAN;
        assert!(bad.restore().is_err());
        let mut bad = source.snapshot()?;
        bad.scales[0] = f32::INFINITY;
        assert!(bad.restore().is_err());
        let mut bad = source.snapshot()?;
        bad.version = 9;
        assert!(bad.restore().is_err());
        Ok(())
    }
}
